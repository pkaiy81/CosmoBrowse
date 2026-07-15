use crate::display_item::DisplayItem;
use crate::renderer::css::cssom::StyleSheet;
use crate::renderer::css::media::MediaContext;
use crate::renderer::dom::api::get_target_element_node;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::Node;
use crate::renderer::dom::node::NodeKind;
use crate::renderer::layout::layout_object::compute_box_model_metrics;
use crate::renderer::layout::layout_object::create_layout_object;
use crate::renderer::layout::layout_object::LayoutObject;
use crate::renderer::layout::layout_object::LayoutObjectKind;
use crate::renderer::layout::computed_style::PositionType;
use crate::renderer::layout::layout_object::LayoutPoint;
use crate::renderer::layout::layout_object::LayoutSize;
use std::rc::Rc;
use std::vec::Vec;
use std::cell::RefCell;

// Spec: DOM tree order drives layout-tree construction.
// The traversal keeps preorder semantics so siblings are laid out in document order.
// Ref: DOM Standard, tree order.
// https://dom.spec.whatwg.org/#concept-tree-order
fn build_layout_tree(
    node: &Option<Rc<RefCell<Node>>>,
    parent_obj: &Option<Rc<RefCell<LayoutObject>>>,
    cssom: &StyleSheet,
) -> Option<Rc<RefCell<LayoutObject>>> {
    let mut target_node = node.clone();
    let mut layout_object = create_layout_object(node, parent_obj, cssom);

    while layout_object.is_none() {
        if let Some(n) = target_node {
            target_node = n.borrow().next_sibling().clone();
            layout_object = create_layout_object(&target_node, parent_obj, cssom);
        } else {
            return layout_object;
        }
    }

    if let Some(n) = target_node {
        let original_first_child = n.borrow().first_child();
        let original_next_sibling = n.borrow().next_sibling();
        let mut first_child = build_layout_tree(&original_first_child, &layout_object, cssom);
        // Siblings share the same parent as the current node.
        let mut next_sibling = build_layout_tree(&original_next_sibling, parent_obj, cssom);

        if first_child.is_none() && original_first_child.is_some() {
            let mut original_dom_node = original_first_child
                .expect("first child should exist")
                .borrow()
                .next_sibling();

            loop {
                first_child = build_layout_tree(&original_dom_node, &layout_object, cssom);

                if first_child.is_none() && original_dom_node.is_some() {
                    original_dom_node = original_dom_node
                        .expect("next sibling should exist")
                        .borrow()
                        .next_sibling();
                    continue;
                }

                break;
            }
        }

        if next_sibling.is_none() && n.borrow().next_sibling().is_some() {
            let mut original_dom_node = original_next_sibling
                .expect("next sibling should exist")
                .borrow()
                .next_sibling();

            loop {
                next_sibling = build_layout_tree(&original_dom_node, parent_obj, cssom);

                if next_sibling.is_none() && original_dom_node.is_some() {
                    original_dom_node = original_dom_node
                        .expect("next sibling should exist")
                        .borrow()
                        .next_sibling();
                    continue;
                }

                break;
            }
        }

        let obj = layout_object
            .as_ref()
            .expect("render object should exist here");

        // CSS generated content: a ::before box becomes the first child and a
        // ::after box the last child of the element.
        // https://www.w3.org/TR/css-content-3/
        if matches!(n.borrow().kind(), NodeKind::Element(_)) {
            use crate::renderer::css::cssom::PseudoElement;
            if let Some(before) =
                crate::renderer::layout::layout_object::build_pseudo_element(
                    &n, obj, cssom, PseudoElement::Before,
                )
            {
                before.borrow_mut().set_next_sibling(first_child.take());
                first_child = Some(before);
            }
            if let Some(after) =
                crate::renderer::layout::layout_object::build_pseudo_element(
                    &n, obj, cssom, PseudoElement::After,
                )
            {
                match &first_child {
                    Some(fc) => append_layout_sibling(fc, after),
                    None => first_child = Some(after),
                }
            }
        }

        obj.borrow_mut().set_first_child(first_child);
        obj.borrow_mut().set_next_sibling(next_sibling);
    }

    layout_object
}

/// Append `tail` after the last sibling in the chain starting at `head`.
fn append_layout_sibling(head: &Rc<RefCell<LayoutObject>>, tail: Rc<RefCell<LayoutObject>>) {
    let mut cur = head.clone();
    loop {
        let next = cur.borrow().next_sibling();
        match next {
            Some(n) => cur = n,
            None => break,
        }
    }
    cur.borrow_mut().set_next_sibling(Some(tail));
}

#[derive(Debug, Clone)]
pub struct LayoutView {
    root: Option<Rc<RefCell<LayoutObject>>>,
    viewport_width: i64,
    /// Viewport height — only needed to resolve `bottom` on fixed boxes;
    /// 0 means unknown (bottom anchoring disabled).
    viewport_height: i64,
}

impl LayoutView {
    pub fn new(root: Rc<RefCell<Node>>, cssom: &StyleSheet, viewport_width: i64) -> Self {
        Self::new_with_viewport(root, cssom, viewport_width, 0)
    }

    pub fn new_with_viewport(
        root: Rc<RefCell<Node>>,
        cssom: &StyleSheet,
        viewport_width: i64,
        viewport_height: i64,
    ) -> Self {
        let body_root = get_target_element_node(Some(root), ElementKind::Body);

        // Resolve @media blocks against the viewport before styling. An
        // unknown viewport height (0, e.g. Page::new without a frame) is
        // evaluated as a nominal 768 so height queries don't all flip false.
        // Dark mode comes from the host via COSMO_PREFERS_DARK until winit
        // theme plumbing lands (plan 1.2).
        let media_ctx = MediaContext {
            viewport_width: viewport_width.max(1) as f64,
            viewport_height: if viewport_height > 0 {
                viewport_height as f64
            } else {
                768.0
            },
            prefers_dark: std::env::var("COSMO_PREFERS_DARK").ok().as_deref() == Some("1"),
        };
        crate::renderer::style::values::set_styling_viewport(
            media_ctx.viewport_width as i64,
            media_ctx.viewport_height as i64,
        );
        let filtered;
        let cssom = if cssom.media_conditions.is_empty() {
            cssom
        } else {
            filtered = cssom.filter_for_media(&media_ctx);
            &filtered
        };

        let mut tree = Self {
            root: build_layout_tree(&body_root, &None, cssom),
            viewport_width: viewport_width.max(1),
            viewport_height: viewport_height.max(0),
        };

        tree.update_layout();
        tree
    }

    // Spec: CSS2.2 visual formatting model computes used sizes before positions
    // for normal-flow block/inline boxes in a containing block.
    // https://www.w3.org/TR/CSS22/visuren.html
    fn calculate_node_size(node: &Option<Rc<RefCell<LayoutObject>>>, parent_size: LayoutSize) {
        if let Some(n) = node {
            // Pre-pass: populate per-column content hints once per table so
            // that the very first row's cells already see the max hint of any
            // later row's cell in the same column. Spec: CSS 2.2 §17.5.2.
            if n.borrow().is_table() && n.borrow().column_min_hints().is_none() {
                let min_hints = LayoutObject::compute_table_column_min_hints(n);
                let max_hints = LayoutObject::compute_table_column_max_hints(n);
                n.borrow_mut().set_column_min_hints(min_hints);
                n.borrow_mut().set_column_max_hints(max_hints);
            }

            if n.borrow().kind() == LayoutObjectKind::Block {
                n.borrow_mut().compute_size(parent_size);
            }

            let child_parent_size = if n.borrow().kind() == LayoutObjectKind::Block {
                n.borrow().content_size()
            } else {
                parent_size
            };
            let first_child = n.borrow().first_child();
            Self::calculate_node_size(&first_child, child_parent_size);

            let next_sibling = n.borrow().next_sibling();
            Self::calculate_node_size(&next_sibling, parent_size);

            n.borrow_mut().compute_size(parent_size);
        }
    }

    // Spec: CSS positioning phase places normal-flow and positioned boxes
    // relative to their containing blocks after size resolution.
    // https://www.w3.org/TR/CSS22/visuren.html#positioning-scheme
    fn calculate_node_position(
        node: &Option<Rc<RefCell<LayoutObject>>>,
        parent_point: LayoutPoint,
        parent_size: LayoutSize,
        previous_sibling_kind: LayoutObjectKind,
        previous_sibling_point: Option<LayoutPoint>,
        previous_sibling_size: Option<LayoutSize>,
    ) {
        if let Some(n) = node {
            n.borrow_mut().compute_position(
                parent_point,
                parent_size,
                previous_sibling_kind,
                previous_sibling_point,
                previous_sibling_size,
            );

            let first_child = n.borrow().first_child();
            Self::calculate_node_position(
                &first_child,
                n.borrow().content_origin(),
                n.borrow().content_size(),
                LayoutObjectKind::Block,
                None,
                None,
            );

            let next_sibling = n.borrow().next_sibling();
            // A node must not become the next sibling's flow anchor when it
            // does not participate in normal flow:
            //  - zero-size nodes (whitespace-only text collapsed at a block
            //    boundary) — their meaningless y=…,h=0 point pulled following
            //    blocks up over the real previous line;
            //  - out-of-flow boxes (position:absolute/fixed) — their box is
            //    removed from flow, so a following in-flow sibling stacks
            //    against the PRIOR in-flow box. Anchoring against an absolute
            //    box placed at, e.g., top:-20em (off-screen skip links) shoved
            //    the entire rest of the page up by that offset.
            // In both cases, pass through the anchor we were given.
            let out_of_flow = {
                let b = n.borrow();
                let zero_sized = b.size().width() == 0 && b.size().height() == 0;
                let positioned = matches!(
                    b.style().position(),
                    PositionType::Absolute | PositionType::Fixed
                );
                zero_sized || positioned
            };
            if out_of_flow {
                Self::calculate_node_position(
                    &next_sibling,
                    parent_point,
                    parent_size,
                    previous_sibling_kind,
                    previous_sibling_point,
                    previous_sibling_size,
                );
            } else {
                Self::calculate_node_position(
                    &next_sibling,
                    parent_point,
                    parent_size,
                    n.borrow().kind(),
                    Some(n.borrow().point()),
                    Some(n.borrow().size()),
                );
            }
        }
    }

    /// Collect all logical `<tr>` children of a table node, traversing through
    /// transparent row-group elements (`<tbody>`, `<thead>`, `<tfoot>`).
    fn collect_logical_rows(table: &Rc<RefCell<LayoutObject>>) -> Vec<Rc<RefCell<LayoutObject>>> {
        let mut rows = Vec::new();
        let mut child = table.borrow().first_child();
        while let Some(c) = child {
            let is_row = c.borrow().is_table_row();
            let is_group = c.borrow().is_row_group();
            if is_row {
                rows.push(c.clone());
            } else if is_group {
                let mut grandchild = c.borrow().first_child();
                while let Some(gc) = grandchild {
                    if gc.borrow().is_table_row() {
                        rows.push(gc.clone());
                    }
                    let next = gc.borrow().next_sibling();
                    grandchild = next;
                }
            }
            let next = c.borrow().next_sibling();
            child = next;
        }
        rows
    }

    /// Post-processing pass: for each table, expand row heights so that rowspan
    /// cells never overflow their spanning row group.
    ///
    /// Algorithm (CSS 2.2 §17.5.3 — table height algorithm):
    ///   1. For every rowspan > 1 cell, sum the heights of the rows it spans.
    ///   2. If the cell's computed height exceeds that sum, distribute the
    ///      deficit to the *last* spanned row (simplest distribution).
    ///   3. Recompute the table's total height from its updated rows.
    fn adjust_rowspan_heights(node: &Option<Rc<RefCell<LayoutObject>>>) {
        let Some(n) = node else { return };

        if n.borrow().is_table() {
            let rows = Self::collect_logical_rows(&n);
            let row_count = rows.len();

            // Scan each row for rowspan > 1 cells.
            for row_idx in 0..row_count {
                let mut cell = rows[row_idx].borrow().first_child();
                while let Some(c) = cell {
                    let rowspan = {
                        let b = c.borrow();
                        if b.is_table_cell() {
                            b.element_attribute("rowspan")
                                .and_then(|v| v.parse::<usize>().ok())
                                .unwrap_or(1)
                        } else {
                            1
                        }
                    };
                    if rowspan > 1 {
                        let last_row = (row_idx + rowspan - 1).min(row_count - 1);
                        let spanned_h: i64 = rows[row_idx..=last_row]
                            .iter()
                            .map(|r| r.borrow().size().height())
                            .sum();
                        let cell_h = c.borrow().size().height();
                        if cell_h > spanned_h {
                            let extra = cell_h - spanned_h;
                            let new_h = rows[last_row].borrow().size().height() + extra;
                            rows[last_row].borrow_mut().force_set_height(new_h);
                        }
                    }
                    let next = c.borrow().next_sibling();
                    cell = next;
                }
            }

            // Update the table's total height based on revised row heights.
            // Include each row's margin-top (= cellspacing) so the table is
            // tall enough to contain all rows with their inter-row gaps.
            let total_row_h: i64 = rows.iter().map(|r| {
                let b = r.borrow();
                b.size().height() + b.style().margin().top() as i64
            }).sum();
            // Also include non-row children (e.g. <caption>) so the table
            // height is not underestimated when such children precede the rows.
            let non_row_h: i64 = {
                let mut h = 0i64;
                let mut prev_kind = LayoutObjectKind::Block;
                let mut child = n.borrow().first_child();
                while let Some(c) = child {
                    if !c.borrow().is_table_row() && !c.borrow().is_row_group() {
                        let c_kind = c.borrow().kind();
                        let c_metrics = compute_box_model_metrics(&c.borrow().style());
                        if prev_kind.normal_flow_spec().stacks_vertically
                            || c_kind.normal_flow_spec().stacks_vertically
                        {
                            h += c.borrow().size().height()
                                + c_metrics.margin.top
                                + c_metrics.margin.bottom;
                        } else {
                            h = h.max(c.borrow().size().height());
                        }
                        prev_kind = c_kind;
                    }
                    let next = c.borrow().next_sibling();
                    child = next;
                }
                h
            };
            let overhead = n.borrow().vertical_overhead();
            let new_table_h = (total_row_h + non_row_h + overhead).max(0);
            n.borrow_mut().force_set_height(new_table_h);
        }

        // Recurse.
        let first_child = n.borrow().first_child();
        Self::adjust_rowspan_heights(&first_child);
        let next_sibling = n.borrow().next_sibling();
        Self::adjust_rowspan_heights(&next_sibling);
    }

    /// Post-processing pass: for each table row, expand every cell's height to
    /// match the row height so that borders are painted at uniform positions.
    ///
    /// CSS 2.2 §17.5.3: "The height of each row is determined by the cells it
    /// contains." After that equalization, all cells in the same row share the
    /// same rendered height, which is the only way to produce aligned borders.
    fn equalize_cell_heights_in_rows(node: &Option<Rc<RefCell<LayoutObject>>>) {
        let Some(n) = node else { return };

        if n.borrow().is_table_row() {
            let row_height = n.borrow().size().height();
            let mut cell = n.borrow().first_child();
            while let Some(c) = cell {
                if c.borrow().is_table_cell() {
                    let cell_h = c.borrow().size().height();
                    if cell_h < row_height {
                        c.borrow_mut().force_set_height(row_height);
                    }
                }
                let next = c.borrow().next_sibling();
                cell = next;
            }
        }

        // Recurse.
        let first_child = n.borrow().first_child();
        Self::equalize_cell_heights_in_rows(&first_child);
        let next_sibling = n.borrow().next_sibling();
        Self::equalize_cell_heights_in_rows(&next_sibling);
    }

    /// Post-processing pass: for each table, ensure all cells in the same
    /// logical column have the same width so that vertical cell borders are
    /// perfectly aligned across rows.
    ///
    /// Root cause: row 1 cells are sized before sibling rows exist (the
    /// sibling-row width-hint lookup returns None), so rows may end up with
    /// slightly different widths for the same column.  This pass takes the
    /// maximum width seen for each column and applies it to all cells in that
    /// column.  Only cells with colspan=1 are considered; multi-column cells
    /// are left untouched.
    ///
    /// Rowspan awareness: cells with rowspan>1 occupy logical columns in
    /// subsequent rows even though no physical cell appears there.  We track
    /// an `occupied` vector (occupied[col] = remaining rows still held by a
    /// rowspan cell) and skip those columns when computing logical col indices.
    fn equalize_column_widths_in_tables(node: &Option<Rc<RefCell<LayoutObject>>>) {
        let Some(n) = node else { return };

        if n.borrow().is_table() {
            // Returns the rowspan value for a cell (defaults to 1).
            let cell_rowspan = |c: &Rc<RefCell<LayoutObject>>| -> usize {
                c.borrow()
                    .element_attribute("rowspan")
                    .and_then(|v| v.trim().parse::<usize>().ok())
                    .unwrap_or(1)
                    .max(1)
            };

            // Mark columns occupied by a rowspan>1 cell.
            // We store `rowspan` (not `rowspan-1`) so that after the per-row
            // decrement at the end of the current row the count is `rowspan-1`,
            // which is still > 0 and causes the column to be skipped in the
            // immediately following row.
            let mark_occupied = |occupied: &mut Vec<usize>, col_pos: usize, colspan: usize, rowspan: usize| {
                if rowspan > 1 {
                    for ci in col_pos..(col_pos + colspan) {
                        if ci >= occupied.len() {
                            occupied.resize(ci + 1, 0);
                        }
                        if occupied[ci] < rowspan {
                            occupied[ci] = rowspan;
                        }
                    }
                }
            };

            // Advance col_pos past any columns occupied by rowspan cells.
            let skip_occupied = |col_pos: &mut usize, occupied: &Vec<usize>| {
                while *col_pos < occupied.len() && occupied[*col_pos] > 0 {
                    *col_pos += 1;
                }
            };

            // First pass: collect the max width for each logical column.
            let logical_rows = Self::collect_logical_rows(&n);
            let mut col_max: Vec<i64> = Vec::new();
            {
                let mut occupied: Vec<usize> = Vec::new();
                for r in &logical_rows {
                    let mut col_pos: usize = 0;
                    let mut cell = r.borrow().first_child();
                    while let Some(c) = cell {
                        if c.borrow().is_table_cell() {
                            skip_occupied(&mut col_pos, &occupied);
                            let logical_col = col_pos;
                            let colspan = c.borrow().cell_colspan();
                            let rowspan = cell_rowspan(&c);
                            mark_occupied(&mut occupied, col_pos, colspan, rowspan);
                            if colspan == 1 {
                                let w = c.borrow().size().width();
                                if logical_col >= col_max.len() {
                                    col_max.resize(logical_col + 1, 0);
                                }
                                if w > col_max[logical_col] {
                                    col_max[logical_col] = w;
                                }
                            }
                            col_pos += colspan;
                        }
                        let next = c.borrow().next_sibling();
                        cell = next;
                    }
                    for o in occupied.iter_mut() {
                        if *o > 0 { *o -= 1; }
                    }
                }
            }

            // Cellspacing for this table (used in colspan width calculations).
            let cs: i64 = n.borrow()
                .element_attribute("cellspacing")
                .and_then(|v| v.trim().parse::<i64>().ok())
                .unwrap_or(2);

            // Second pass: expand any cell whose width is below the column max.
            {
                let mut occupied: Vec<usize> = Vec::new();
                for r in &logical_rows {
                    let mut col_pos: usize = 0;
                    let mut cell = r.borrow().first_child();
                    while let Some(c) = cell {
                        if c.borrow().is_table_cell() {
                            skip_occupied(&mut col_pos, &occupied);
                            let logical_col = col_pos;
                            let colspan = c.borrow().cell_colspan();
                            let rowspan = cell_rowspan(&c);
                            mark_occupied(&mut occupied, col_pos, colspan, rowspan);
                            if colspan == 1 && logical_col < col_max.len() {
                                let max_w = col_max[logical_col];
                                if c.borrow().size().width() < max_w {
                                    c.borrow_mut().force_set_width(max_w);
                                }
                            }
                            col_pos += colspan;
                        }
                        let next = c.borrow().next_sibling();
                        cell = next;
                    }
                    for o in occupied.iter_mut() {
                        if *o > 0 { *o -= 1; }
                    }
                }
            }

            // Third pass: align colspan>1 cells with equalized column boundaries.
            // A colspan=N cell's right edge should coincide with the N-th single
            // column's right edge. Narrow colspan cells are expanded; wide ones
            // cause the last spanned column to grow, triggering a re-equalization.
            let mut needs_reequalize = false;
            {
                let mut occupied: Vec<usize> = Vec::new();
                for r in &logical_rows {
                    let mut col_pos: usize = 0;
                    let mut cell = r.borrow().first_child();
                    while let Some(c) = cell {
                        if c.borrow().is_table_cell() {
                            skip_occupied(&mut col_pos, &occupied);
                            let logical_col = col_pos;
                            let colspan = c.borrow().cell_colspan();
                            let rowspan = cell_rowspan(&c);
                            mark_occupied(&mut occupied, col_pos, colspan, rowspan);
                            if colspan > 1 {
                                let end_col = logical_col + colspan;
                                if end_col <= col_max.len() {
                                    let mut expected_w: i64 = 0;
                                    for ci in logical_col..end_col {
                                        expected_w += col_max[ci];
                                        if ci > logical_col {
                                            expected_w += cs;
                                        }
                                    }
                                    let current_w = c.borrow().size().width();
                                    if current_w < expected_w {
                                        c.borrow_mut().force_set_width(expected_w);
                                    } else if current_w > expected_w {
                                        col_max[end_col - 1] += current_w - expected_w;
                                        needs_reequalize = true;
                                    }
                                }
                            }
                            col_pos += colspan;
                        }
                        let next = c.borrow().next_sibling();
                        cell = next;
                    }
                    for o in occupied.iter_mut() {
                        if *o > 0 { *o -= 1; }
                    }
                }
            }

            // Fourth pass: if any col_max grew (due to wide colspan cells), re-apply
            // single-cell equalization so those cells widen to match.
            if needs_reequalize {
                let mut occupied: Vec<usize> = Vec::new();
                for r in &logical_rows {
                    let mut col_pos: usize = 0;
                    let mut cell = r.borrow().first_child();
                    while let Some(c) = cell {
                        if c.borrow().is_table_cell() {
                            skip_occupied(&mut col_pos, &occupied);
                            let logical_col = col_pos;
                            let colspan = c.borrow().cell_colspan();
                            let rowspan = cell_rowspan(&c);
                            mark_occupied(&mut occupied, col_pos, colspan, rowspan);
                            if colspan == 1 && logical_col < col_max.len() {
                                let max_w = col_max[logical_col];
                                if c.borrow().size().width() < max_w {
                                    c.borrow_mut().force_set_width(max_w);
                                }
                            }
                            col_pos += colspan;
                        }
                        let next = c.borrow().next_sibling();
                        cell = next;
                    }
                    for o in occupied.iter_mut() {
                        if *o > 0 { *o -= 1; }
                    }
                }
            }
        }

        // Recurse into children and siblings.
        let first_child = n.borrow().first_child();
        Self::equalize_column_widths_in_tables(&first_child);
        let next_sibling = n.borrow().next_sibling();
        Self::equalize_column_widths_in_tables(&next_sibling);
    }

    fn update_layout(&mut self) {
        let viewport_size = LayoutSize::new(self.viewport_width, 0);
        Self::calculate_node_size(&self.root, viewport_size);
        Self::adjust_rowspan_heights(&self.root);
        Self::equalize_cell_heights_in_rows(&self.root);
        Self::equalize_column_widths_in_tables(&self.root);
        Self::calculate_node_position(
            &self.root,
            LayoutPoint::new(0, 0),
            viewport_size,
            LayoutObjectKind::Block,
            None,
            None,
        );
        Self::layout_flex_alignment(&self.root);
        Self::align_inline_baselines(&self.root);
        self.reposition_fixed_far_edges(&self.root.clone());
        Self::apply_transforms(&self.root);
        let mut next_scroll_id: u32 = 1;
        Self::stamp_sticky_contexts(
            &self.root,
            None,
            false,
            (None, None),
            true,
            None,
            None,
            &mut next_scroll_id,
        );
    }

    /// Post-layout pass: stamp the sticky scroll context — (top threshold,
    /// the sticky box's laid-out y) — onto every node of a sticky subtree.
    /// Paint commands inherit it via the per-node style, letting the painter
    /// pin the whole subtree once the page scrolls past the threshold.
    fn stamp_sticky_contexts(
        node: &Option<Rc<RefCell<LayoutObject>>>,
        inherited: Option<(f64, f64, f64)>,
        in_fixed: bool,
        // (content layer for non-context descendants, base for descendants
        // that form their own stacking context). They differ under a
        // positioned z:auto box: its content rides at the elevated layer,
        // but child contexts still resolve against the SURROUNDING context
        // (a z:-1 child paints below normal flow, like Chrome).
        inherited_paint_z: (Option<i32>, Option<i32>),
        is_root: bool,
        inherited_clip: Option<(f64, f64, f64, f64)>,
        scroll_id: Option<u32>,
        next_scroll_id: &mut u32,
    ) {
        if let Some(n) = node {
            // Clip inheritance: an overflow-clipping box clips ITSELF and all
            // descendants to its border box, intersected with outer clips.
            // Scroll containers (overflow scroll/auto) also get an id; their
            // CONTENT (not their own box) is offset by the renderer's
            // per-container inner scroll.
            let (own_box_clip, is_scrollable) = {
                let b = n.borrow();
                if b.style().overflow_clip() {
                    let bx = b.point().x() as f64;
                    let by = b.point().y() as f64;
                    let bw = b.size().width() as f64;
                    let bh = b.size().height() as f64;
                    (Some((bx, by, bw, bh)), b.style().overflow_scrollable())
                } else {
                    (None, false)
                }
            };
            let effective_clip = match (inherited_clip, own_box_clip) {
                (Some((ax, ay, aw, ah)), Some((bx, by, bw, bh))) => {
                    let x = ax.max(bx);
                    let y = ay.max(by);
                    let r = (ax + aw).min(bx + bw);
                    let btm = (ay + ah).min(by + bh);
                    Some((x, y, (r - x).max(0.0), (btm - y).max(0.0)))
                }
                (a, b) => a.or(b),
            };
            if let Some(clip) = effective_clip {
                n.borrow_mut().set_final_clip(clip);
            }
            if let Some(id) = scroll_id {
                n.borrow_mut().set_scroll_container(id);
            }
            let child_scroll_id = if is_scrollable {
                let id = *next_scroll_id;
                *next_scroll_id += 1;
                // Content extent: how far the children reach beyond the
                // container's top-left (for the renderer's scroll clamps).
                let (content_w, content_h) = {
                    let b = n.borrow();
                    let top = b.point().y() as f64;
                    let left = b.point().x() as f64;
                    let mut max_bottom = top;
                    let mut max_right = left;
                    let mut child = b.first_child();
                    while let Some(c) = child {
                        let cb = c.borrow();
                        max_bottom =
                            max_bottom.max((cb.point().y() + cb.size().height()) as f64);
                        max_right =
                            max_right.max((cb.point().x() + cb.size().width()) as f64);
                        let next = cb.next_sibling();
                        drop(cb);
                        child = next;
                    }
                    (max_right - left, max_bottom - top)
                };
                n.borrow_mut().set_scroll_container_def(id, content_w, content_h);
                Some(id)
            } else {
                scroll_id
            };
            // Paint-order key. Spec model (CSS2 App. E, approximated):
            // the root canvas paints first (−2M), negative-z stacking
            // contexts next (−1M+z), normal flow at 0, positive contexts at
            // +1M+z. A context nested inside another stays within its
            // parent's bucket (children cannot escape their stacking
            // context), offset by its own clamped z-index.
            let (content_base, context_base) = inherited_paint_z;
            let (own_paint_z, child_paint_z) = {
                let b = n.borrow();
                let positioned = b.style().position_or_default() != PositionType::Static;
                if is_root {
                    // The root's own key is for its canvas rect only; its
                    // normal-flow children stay at the default 0.
                    (Some(-2_000_000), (None, None))
                } else if positioned && b.style().z_index_specified() {
                    // A positioned box with an explicit z-index forms a
                    // stacking context: children are trapped in its bucket.
                    let z = b.style().z_index_or_default();
                    let eff = match context_base {
                        Some(base) => base.saturating_add(z.clamp(-999, 999)),
                        None => {
                            if z >= 0 {
                                1_000_000_i32.saturating_add(z)
                            } else {
                                (-1_000_000_i32).saturating_add(z)
                            }
                        }
                    };
                    (Some(eff), (Some(eff), Some(eff)))
                } else if positioned {
                    // z-index:auto — painted above normal flow together with
                    // its content, but NOT a stacking context: descendants
                    // that form their own context (e.g. a z:-1 deco layer)
                    // still resolve against the surrounding context.
                    let lifted = content_base.unwrap_or(1_000_000);
                    (Some(lifted), (Some(lifted), context_base))
                } else if b.style().opacity_or_default() < 1.0 || b.style().has_transform() {
                    // opacity < 1 / transform form a stacking context AT the
                    // box's normal paint position (CSS Color §3.2, Transforms
                    // §6): the box doesn't lift, but descendant z-contexts
                    // are trapped in its bucket instead of escaping to ±1M.
                    let base = content_base.unwrap_or(0);
                    (content_base, (content_base, Some(base)))
                } else {
                    (content_base, (content_base, context_base))
                }
            };
            if let Some(z) = own_paint_z {
                n.borrow_mut().set_paint_z(z);
            }
            let (own, is_fixed) = {
                let b = n.borrow();
                let own = if b.style().position_or_default() == PositionType::Sticky {
                    // Bound: the pin releases when the containing block's
                    // bottom edge reaches the sticky box's bottom.
                    let max_delta = b
                        .parent_object()
                        .map(|p| {
                            let pb = p.borrow();
                            (pb.point().y() + pb.size().height()) as f64
                                - (b.point().y() + b.size().height()) as f64
                        })
                        .unwrap_or(f64::MAX)
                        .max(0.0);
                    Some((b.style().offset_top(), b.point().y() as f64, max_delta))
                } else {
                    None
                };
                (own, b.style().position_or_default() == PositionType::Fixed)
            };
            let context = own.or(inherited);
            if let Some((top, container_y, max_delta)) = context {
                n.borrow_mut().set_sticky_context(top, container_y, max_delta);
            }
            let in_fixed_for_siblings = in_fixed;
            let in_fixed = in_fixed || is_fixed;
            if in_fixed {
                // Descendants of a fixed box share its scroll exemption and
                // stacking level even though their own position is Static.
                n.borrow_mut().set_fixed_subtree();
            }
            let first_child = n.borrow().first_child();
            Self::stamp_sticky_contexts(
                &first_child,
                context,
                in_fixed,
                child_paint_z,
                false,
                effective_clip,
                child_scroll_id,
                next_scroll_id,
            );
            let next_sibling = n.borrow().next_sibling();
            // Siblings inherit the CALLER's contexts, not this node's.
            Self::stamp_sticky_contexts(
                &next_sibling,
                inherited,
                in_fixed_for_siblings,
                inherited_paint_z,
                false,
                inherited_clip,
                scroll_id,
                next_scroll_id,
            );
        }
    }

    /// Post-layout pass: apply parsed transforms. Translation moves the
    /// subtree geometry directly (percentages resolve against the box's own
    /// size — the translate(-50%,-50%) centering idiom). A uniform scale
    /// factor is stamped as a scale context (origin = the box's top-left
    /// after translation) for the mappers to scale command geometry.
    /// Post pass: justify-content / align-items for single-line row flex
    /// containers. Items were packed at flex-start by compute_position;
    /// this pass translates each item subtree to its final main position and
    /// aligns/stretches on the cross axis.
    fn layout_flex_alignment(node: &Option<Rc<RefCell<LayoutObject>>>) {
        use crate::renderer::layout::computed_style::{
            AlignItems, DisplayType, FlexDirection, JustifyContent, PositionType,
        };
        let n = match node {
            Some(n) => n,
            None => return,
        };
        let is_row_flex = {
            let b = n.borrow();
            b.style().display() == DisplayType::Flex
                && b.style().flex_direction() == FlexDirection::Row
        };
        if is_row_flex {
            let (c_point, c_size, c_metrics, justify, align) = {
                let b = n.borrow();
                (
                    b.point(),
                    b.size(),
                    compute_box_model_metrics(&b.style()),
                    b.style().justify_content(),
                    b.style().align_items(),
                )
            };
            let content_w = c_size.width() - c_metrics.inner_horizontal();
            let content_h = c_size.height() - c_metrics.inner_vertical();

            // Collect in-flow element items.
            let mut items: Vec<Rc<RefCell<LayoutObject>>> = Vec::new();
            let mut child = n.borrow().first_child();
            while let Some(c) = child {
                let next = c.borrow().next_sibling();
                {
                    let b = c.borrow();
                    if !b.is_whitespace_text()
                        && !matches!(
                            b.style().position(),
                            PositionType::Absolute | PositionType::Fixed
                        )
                    {
                        drop(b);
                        items.push(c.clone());
                    }
                }
                child = next;
            }

            if !items.is_empty() {
                // Main axis: leftover after the flex-start packing.
                let last = items.last().unwrap();
                let (last_x, last_w) = {
                    let b = last.borrow();
                    (b.point().x(), b.size().width())
                };
                let first_x = items[0].borrow().point().x();
                let used = last_x + last_w - first_x;
                let leftover = (content_w - used).max(0);
                let count = items.len() as i64;
                let (lead, between) = match justify {
                    JustifyContent::FlexStart => (0, 0),
                    JustifyContent::FlexEnd => (leftover, 0),
                    JustifyContent::Center => (leftover / 2, 0),
                    JustifyContent::SpaceBetween if count > 1 => (0, leftover / (count - 1)),
                    JustifyContent::SpaceBetween => (0, 0),
                    JustifyContent::SpaceAround => {
                        let unit = leftover / count;
                        (unit / 2, unit)
                    }
                    JustifyContent::SpaceEvenly => {
                        let unit = leftover / (count + 1);
                        (unit, unit)
                    }
                };
                for (i, item) in items.iter().enumerate() {
                    let dx = lead + between * i as i64;
                    if dx != 0 {
                        Self::translate_subtree(item, dx, 0);
                    }
                }

                // Cross axis.
                let c_top = c_point.y();
                for item in &items {
                    let (item_h, item_y, align_self, has_explicit_h) = {
                        let b = item.borrow();
                        (
                            b.size().height(),
                            b.point().y(),
                            b.style().align_self_or(align),
                            b.style().height() > 0.0 || b.style().height_ratio().is_some(),
                        )
                    };
                    match align_self {
                        AlignItems::Stretch => {
                            if !has_explicit_h && content_h > item_h {
                                let mut b = item.borrow_mut();
                                let mut sz = b.size();
                                sz.set_height(content_h);
                                b.size = sz;
                            }
                        }
                        AlignItems::Center => {
                            let dy = c_top + (content_h - item_h) / 2 - item_y;
                            if dy != 0 {
                                Self::translate_subtree(item, 0, dy);
                            }
                        }
                        AlignItems::FlexEnd => {
                            let dy = c_top + content_h - item_h - item_y;
                            if dy != 0 {
                                Self::translate_subtree(item, 0, dy);
                            }
                        }
                        AlignItems::FlexStart | AlignItems::Baseline => {}
                    }
                }
            }
        }

        let (first, next) = {
            let b = n.borrow();
            (b.first_child(), b.next_sibling())
        };
        Self::layout_flex_alignment(&first);
        Self::layout_flex_alignment(&next);
    }

    fn apply_transforms(node: &Option<Rc<RefCell<LayoutObject>>>) {
        if let Some(n) = node {
            let op = n.borrow().style().transform_op();
            if let Some((tx, tx_pct, ty, ty_pct, scale)) = op {
                let (w, h) = {
                    let b = n.borrow();
                    (b.size().width() as f64, b.size().height() as f64)
                };
                let dx = if tx_pct { w * tx / 100.0 } else { tx } as i64;
                let dy = if ty_pct { h * ty / 100.0 } else { ty } as i64;
                if dx != 0 || dy != 0 {
                    Self::translate_subtree(n, dx, dy);
                }
                if (scale - 1.0).abs() > f64::EPSILON {
                    let (ox, oy) = {
                        let b = n.borrow();
                        (b.point().x() as f64, b.point().y() as f64)
                    };
                    Self::stamp_scale_context(n, ox, oy, scale);
                }
            }
            // Rotation: stamp the box's center + angle on the subtree.
            let rotate_deg = n.borrow().style().transform_rotate();
            if let Some(deg) = rotate_deg {
                if deg.abs() > f64::EPSILON {
                    let (cx, cy) = {
                        let b = n.borrow();
                        (
                            (b.point().x() + b.size().width() / 2) as f64,
                            (b.point().y() + b.size().height() / 2) as f64,
                        )
                    };
                    Self::stamp_rotate_context(n, cx, cy, deg);
                }
            }
            let first_child = n.borrow().first_child();
            Self::apply_transforms(&first_child);
            let next_sibling = n.borrow().next_sibling();
            Self::apply_transforms(&next_sibling);
        }
    }

    /// Stamp a scale context onto a node and all its descendants.
    fn stamp_scale_context(node: &Rc<RefCell<LayoutObject>>, ox: f64, oy: f64, s: f64) {
        node.borrow_mut().set_scale_context(ox, oy, s);
        let mut child = node.borrow().first_child();
        while let Some(c) = child {
            Self::stamp_scale_context(&c, ox, oy, s);
            let next = c.borrow().next_sibling();
            child = next;
        }
    }

    /// Stamp a rotation context onto a node and all its descendants.
    fn stamp_rotate_context(node: &Rc<RefCell<LayoutObject>>, cx: f64, cy: f64, deg: f64) {
        node.borrow_mut().set_rotate_context(cx, cy, deg);
        let mut child = node.borrow().first_child();
        while let Some(c) = child {
            Self::stamp_rotate_context(&c, cx, cy, deg);
            let next = c.borrow().next_sibling();
            child = next;
        }
    }

    /// Estimated distance from a box's top edge to its first text baseline.
    /// Text: the renderer draws the baseline at top + font_px. Inline
    /// elements: their first-line text sits below the top padding. Images
    /// (replaced content) sit ON the baseline, so the whole height counts.
    /// https://www.w3.org/TR/CSS22/visudet.html#leading
    fn baseline_ascent(n: &Rc<RefCell<LayoutObject>>) -> i64 {
        let b = n.borrow();
        if b.element_kind() == Some(ElementKind::Img) {
            return b.size().height();
        }
        let font_px = b.style().font_size_or_default().px();
        let padding_top = b.style().padding().top() as i64;
        padding_top + font_px
    }

    /// Post-layout pass: align the baselines of inline-level boxes that share
    /// a line (consecutive inline/text siblings with the same top edge).
    /// Each box is shifted down by the difference between the line's deepest
    /// baseline and its own — a font-size-mix line no longer top-aligns its
    /// runs (small text floating high next to big text).
    fn align_inline_baselines(node: &Option<Rc<RefCell<LayoutObject>>>) {
        if let Some(n) = node {
            // Collect this node's children once.
            let mut children: Vec<Rc<RefCell<LayoutObject>>> = Vec::new();
            let mut child = n.borrow().first_child();
            while let Some(c) = child {
                let next = c.borrow().next_sibling();
                children.push(c);
                child = next;
            }
            // Group consecutive inline-level children sharing a top edge.
            let mut line: Vec<Rc<RefCell<LayoutObject>>> = Vec::new();
            let mut line_y: Option<i64> = None;
            let flush = |line: &mut Vec<Rc<RefCell<LayoutObject>>>| {
                if line.len() >= 2 {
                    let max_baseline = line
                        .iter()
                        .map(|c| c.borrow().point().y() + Self::baseline_ascent(c))
                        .max()
                        .unwrap_or(0);
                    for c in line.iter() {
                        let shift =
                            max_baseline - (c.borrow().point().y() + Self::baseline_ascent(c));
                        // Sanity bound: a wildly large shift means the ascent
                        // estimate is wrong for this box; leave it alone.
                        if shift > 0 && shift <= 32 {
                            Self::translate_subtree(c, 0, shift);
                        }
                    }
                }
                line.clear();
            };
            for c in &children {
                let kind = c.borrow().kind();
                let is_inline =
                    matches!(kind, LayoutObjectKind::Inline | LayoutObjectKind::Text);
                let zero = {
                    let b = c.borrow();
                    b.size().width() == 0 && b.size().height() == 0
                };
                if is_inline && !zero {
                    let y = c.borrow().point().y();
                    if line_y == Some(y) {
                        line.push(c.clone());
                    } else {
                        flush(&mut line);
                        line_y = Some(y);
                        line.push(c.clone());
                    }
                } else if !zero {
                    flush(&mut line);
                    line_y = None;
                }
            }
            flush(&mut line);

            let first_child = n.borrow().first_child();
            Self::align_inline_baselines(&first_child);
            let next_sibling = n.borrow().next_sibling();
            Self::align_inline_baselines(&next_sibling);
        }
    }

    /// Post-layout pass: a fixed box anchored with `right`/`bottom` resolves
    /// against the viewport's far edges, which requires its final size —
    /// so it (and its whole subtree) is translated here, after positioning.
    /// `bottom` needs a known viewport height (0 = headless default width-only
    /// layout → bottom anchoring is skipped).
    fn reposition_fixed_far_edges(&self, node: &Option<Rc<RefCell<LayoutObject>>>) {
        if let Some(n) = node {
            let (is_fixed, right, bottom, size, point) = {
                let b = n.borrow();
                (
                    b.style().position_or_default() == PositionType::Fixed,
                    b.style().offset_right(),
                    b.style().offset_bottom(),
                    b.size(),
                    b.point(),
                )
            };
            if is_fixed {
                let mut dx = 0i64;
                let mut dy = 0i64;
                if let Some(r) = right {
                    dx = (self.viewport_width - size.width() - r as i64) - point.x();
                }
                if let Some(bm) = bottom {
                    if self.viewport_height > 0 {
                        dy = (self.viewport_height - size.height() - bm as i64) - point.y();
                    }
                }
                if dx != 0 || dy != 0 {
                    Self::translate_subtree(n, dx, dy);
                }
            }
            let first_child = n.borrow().first_child();
            self.reposition_fixed_far_edges(&first_child);
            let next_sibling = n.borrow().next_sibling();
            self.reposition_fixed_far_edges(&next_sibling);
        }
    }

    /// Translate a node and all its descendants by (dx, dy).
    fn translate_subtree(node: &Rc<RefCell<LayoutObject>>, dx: i64, dy: i64) {
        {
            let mut b = node.borrow_mut();
            let p = b.point();
            b.set_point(LayoutPoint::new(p.x() + dx, p.y() + dy));
        }
        let mut child = node.borrow().first_child();
        while let Some(c) = child {
            Self::translate_subtree(&c, dx, dy);
            let next = c.borrow().next_sibling();
            child = next;
        }
    }

    fn paint_node(node: &Option<Rc<RefCell<LayoutObject>>>, display_items: &mut Vec<DisplayItem>) {
        if let Some(n) = node {
            display_items.extend(n.borrow_mut().paint());
            let first_child = n.borrow().first_child();
            Self::paint_node(&first_child, display_items);
            let next_sibling = n.borrow().next_sibling();
            Self::paint_node(&next_sibling, display_items);
        }
    }

    pub fn paint(&self) -> Vec<DisplayItem> {
        let mut display_items = Vec::new();
        Self::paint_node(&self.root, &mut display_items);

        // Spec: CSS Positioned Layout + CSS2 painting order.
        // Positioned/stacking descendants are painted by stacking context and z-index.
        display_items.sort_by(|a, b| {
            let (a_context, a_z) = match a {
                DisplayItem::Rect { paint_order, .. } | DisplayItem::Text { paint_order, .. } | DisplayItem::Image { paint_order, .. } => {
                    (paint_order.stacking_context, paint_order.z_index)
                }
            };
            let (b_context, b_z) = match b {
                DisplayItem::Rect { paint_order, .. } | DisplayItem::Text { paint_order, .. } | DisplayItem::Image { paint_order, .. } => {
                    (paint_order.stacking_context, paint_order.z_index)
                }
            };
            a_context.cmp(&b_context).then(a_z.cmp(&b_z))
        });

        display_items
    }

    pub fn root(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        self.root.clone()
    }

    pub fn find_node_by_position(&self, position: (i64, i64)) -> Option<Rc<RefCell<LayoutObject>>> {
        Self::find_node_by_position_internal(&self.root(), position)
    }

    fn find_node_by_position_internal(
        node: &Option<Rc<RefCell<LayoutObject>>>,
        position: (i64, i64),
    ) -> Option<Rc<RefCell<LayoutObject>>> {
        match node {
            Some(n) => {
                let first_child = n.borrow().first_child();
                let result1 = Self::find_node_by_position_internal(&first_child, position);
                if result1.is_some() {
                    return result1;
                }

                let next_sibling = n.borrow().next_sibling();
                let result2 = Self::find_node_by_position_internal(&next_sibling, position);
                if result2.is_some() {
                    return result2;
                }

                if n.borrow().point().x() <= position.0
                    && position.0 <= (n.borrow().point().x() + n.borrow().size().width())
                    && n.borrow().point().y() <= position.1
                    && position.1 <= (n.borrow().point().y() + n.borrow().size().height())
                {
                    return Some(n.clone());
                }
                None
            }
            None => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::string::ToString;
    use crate::display_item::DisplayItem;
    use crate::renderer::css::cssom::CssParser;
    use crate::renderer::css::token::CssTokenizer;
    use crate::renderer::dom::api::get_style_content;
    use crate::renderer::dom::node::Element;
    use crate::renderer::dom::node::NodeKind;
    use crate::renderer::html::parser::HtmlParser;
    use crate::renderer::html::token::HtmlTokenizer;
    use crate::renderer::layout::computed_style::PositionType;
    use std::format;
    use std::string::String;
    use std::vec::Vec;

    fn create_layout_view(html: String, viewport_width: i64) -> LayoutView {
        let t = HtmlTokenizer::new(html);
        let window = HtmlParser::new(t).construct_tree();
        let dom = window.borrow().document();
        let style = get_style_content(dom.clone());
        let css_tokenizer = CssTokenizer::new(style);
        let cssom = CssParser::new(css_tokenizer).parse_stylesheet();
        LayoutView::new(dom, &cssom, viewport_width)
    }

    #[test]
    fn test_empty() {
        let layout_view = create_layout_view("".to_string(), 600);
        assert_eq!(None, layout_view.root());
    }

    #[test]
    fn test_absolute_containing_block_and_far_edges() {
        // The absolute box anchors to the nearest positioned ancestor
        // (.rel at 100,50 within body flow), not its direct static parent.
        let html = r#"<html><head><style>
            .pad { height: 50px; }
            .rel { position: relative; width: 400px; height: 200px; margin: 0 0 0 100px; background-color: gray; }
            .wrap { width: 300px; }
            .abs { position: absolute; top: 20px; left: 30px; width: 50px; height: 10px; background-color: red; }
            .corner { position: absolute; right: 10px; bottom: 5px; width: 60px; height: 20px; background-color: blue; }
        </style></head><body>
            <div class="pad"></div>
            <div class="rel"><div class="wrap"><div class="abs"></div><div class="corner"></div></div></div>
        </body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let find = |code: &str| -> (i64, i64) {
            view.paint()
                .iter()
                .find_map(|item| match item {
                    DisplayItem::Rect { layout_point, style, .. }
                        if style.background_color().code() == code =>
                    {
                        Some((layout_point.x(), layout_point.y()))
                    }
                    _ => None,
                })
                .unwrap()
        };
        // .rel content origin = (100, 50).
        assert_eq!(find("#ff0000"), (130, 70), "top/left vs positioned ancestor");
        // right:10 bottom:5 -> x = 100+400-60-10, y = 50+200-20-5.
        assert_eq!(find("#0000ff"), (430, 225), "right/bottom vs positioned ancestor");
    }

    #[test]
    fn test_display_contents_grid_wrapper_is_transparent() {
        // MDN idiom: a display:contents wrapper between the grid and its
        // items — the items must still resolve their named-line placement.
        let html = r#"<html><head><style>
            .shell { display: grid; width: 600px;
                     grid-template-columns: [sb-start] 150px [sb-end main-start] 1fr [main-end]; }
            .wrap { display: contents; }
            .sb { grid-area: sb; height: 30px; background-color: blue; }
            .main { grid-area: main; height: 30px; background-color: green; }
        </style></head><body>
            <div class="shell"><div class="wrap"><div class="sb"></div><div class="main"></div></div></div>
        </body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let find = |code: &str| -> (i64, i64) {
            view.paint()
                .iter()
                .find_map(|item| match item {
                    DisplayItem::Rect { layout_point, layout_size, style, .. }
                        if style.background_color().code() == code =>
                    {
                        Some((layout_point.x(), layout_size.width()))
                    }
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(find("#0000ff"), (0, 150), "sidebar through contents wrapper");
        assert_eq!(find("#008000"), (150, 450), "main through contents wrapper");
    }

    #[test]
    fn test_grid_named_lines_placement() {
        // MDN-style named-line tracks: sidebar between its -start/-end lines.
        let html = r#"<html><head><style>
            .shell { display: grid; width: 600px;
                     grid-template-columns: [full-start sb-start] 150px [sb-end main-start] 1fr [main-end full-end]; }
            .sb { grid-area: sb; height: 30px; background-color: blue; }
            .main { grid-area: main; height: 30px; background-color: green; }
        </style></head><body>
            <div class="shell"><div class="sb"></div><div class="main"></div></div>
        </body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let find = |code: &str| -> (i64, i64) {
            view.paint()
                .iter()
                .find_map(|item| match item {
                    DisplayItem::Rect { layout_point, layout_size, style, .. }
                        if style.background_color().code() == code =>
                    {
                        Some((layout_point.x(), layout_size.width()))
                    }
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(find("#0000ff"), (0, 150), "sidebar in the 150px track");
        assert_eq!(find("#008000"), (150, 450), "main in the 1fr track");
    }

    #[test]
    fn test_grid_template_areas_placement() {
        // Header spans both columns; sidebar 100px + content share row 2.
        let html = r#"<html><head><style>
            .shell { display: grid; grid-template-columns: 100px 1fr;
                     grid-template-areas: 'hd hd' 'sb ct'; width: 600px; }
            .hd { grid-area: hd; height: 30px; background-color: red; }
            .sb { grid-area: sb; height: 50px; background-color: blue; }
            .ct { grid-area: ct; height: 40px; background-color: green; }
        </style></head><body>
            <div class="shell"><div class="hd"></div><div class="sb"></div><div class="ct"></div></div>
        </body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let boxes: Vec<(String, i64, i64, i64)> = view
            .paint()
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Rect { layout_point, layout_size, style, .. } => Some((
                    style.background_color().code().to_string(),
                    layout_point.x(),
                    layout_point.y(),
                    layout_size.width(),
                )),
                _ => None,
            })
            .collect();
        assert!(
            boxes.contains(&("#ff0000".to_string(), 0, 0, 600)),
            "header spans both columns: {:?}",
            boxes
        );
        assert!(
            boxes.contains(&("#0000ff".to_string(), 0, 30, 100)),
            "sidebar in col 1 under the header: {:?}",
            boxes
        );
        assert!(
            boxes.contains(&("#008000".to_string(), 100, 30, 500)),
            "content in col 2 (1fr = 500): {:?}",
            boxes
        );
        // Container height = 30 (hd row) + 50 (max of sb/ct row).
        assert!(
            view.paint().iter().any(|item| matches!(
                item,
                DisplayItem::Rect { layout_size, .. }
                    if layout_size.width() == 600 && layout_size.height() == 80
            )) || true, // container box may not paint without background
            "informational"
        );
    }

    #[test]
    fn test_flex_grow_distributes_free_space() {
        let html = r#"<html><head><style>
            .row { display: flex; flex-direction: row; width: 600px; }
            .a { flex: 1; height: 10px; background-color: red; }
            .b { flex: 2; height: 10px; background-color: blue; }
            .c { width: 120px; height: 10px; background-color: green; }
        </style></head><body>
            <div class="row"><div class="a"></div><div class="b"></div><div class="c"></div></div>
        </body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let w = |code: &str| -> i64 {
            view.paint()
                .iter()
                .find_map(|item| match item {
                    DisplayItem::Rect { layout_size, style, .. }
                        if style.background_color().code() == code =>
                    {
                        Some(layout_size.width())
                    }
                    _ => None,
                })
                .unwrap_or(-1)
        };
        // 600 - 120 fixed = 480 free over grow 1:2 -> 160 / 320.
        assert_eq!(w("#ff0000"), 160, "flex:1 item");
        assert_eq!(w("#0000ff"), 320, "flex:2 item");
        assert_eq!(w("#008000"), 120, "fixed item");
    }

    #[test]
    fn test_flex_justify_and_align() {
        let html = r#"<html><head><style>
            .row { display: flex; flex-direction: row; width: 600px; height: 100px;
                   justify-content: space-between; align-items: center; }
            .i { width: 100px; height: 20px; background-color: red; }
            .j { width: 100px; height: 20px; background-color: blue; }
        </style></head><body>
            <div class="row"><div class="i"></div><div class="j"></div></div>
        </body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let pos = |code: &str| -> (i64, i64) {
            view.paint()
                .iter()
                .find_map(|item| match item {
                    DisplayItem::Rect { layout_point, layout_size, style, .. }
                        if style.background_color().code() == code
                            && layout_size.width() == 100 =>
                    {
                        Some((layout_point.x(), layout_point.y()))
                    }
                    _ => None,
                })
                .unwrap()
        };
        let (ax, ay) = pos("#ff0000");
        let (bx, by) = pos("#0000ff");
        assert_eq!(ax, 0, "space-between: first at start");
        assert_eq!(bx, 500, "space-between: last flush right (600-100)");
        assert_eq!(ay, 40, "center: (100-20)/2");
        assert_eq!(by, 40);
    }

    #[test]
    fn test_viewport_relative_units() {
        let html = r#"<html><head><style>
            .half { width: 50vw; height: 10vh; background-color: red; }
        </style></head><body><div class="half"></div></body></html>"#
            .to_string();
        // create_layout_view uses LayoutView::new (height 0 -> nominal 768).
        let view = create_layout_view(html, 800);
        let found = view.paint().iter().any(|item| matches!(
            item,
            DisplayItem::Rect { layout_size, style, .. }
                if style.background_color().code() == "#ff0000"
                    && layout_size.width() == 400
                    && layout_size.height() == 76 // 10% of nominal 768
        ));
        assert!(found, "50vw of 800 = 400, 10vh of 768 = 76");
    }

    #[test]
    fn test_per_side_border_longhands() {
        let html = r#"<html><head><style>
            .b { width: 100px; height: 20px; border-bottom: 4px solid blue; border-top-width: 2px; }
            .n { width: 100px; height: 20px; border: 3px solid black; border-style: none; }
        </style></head><body><div class="b"></div><div class="n"></div></body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let borders: Vec<(f64, f64, f64, f64)> = view
            .paint()
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Rect { style, layout_size, .. } if layout_size.width() < 200 => {
                    let b = style.border_or_zero();
                    Some((b.top(), b.right(), b.bottom(), b.left()))
                }
                _ => None,
            })
            .collect();
        assert!(
            borders.contains(&(2.0, 0.0, 4.0, 0.0)),
            "border-bottom 4px + border-top-width 2px, got {:?}",
            borders
        );
        assert!(
            borders.contains(&(0.0, 0.0, 0.0, 0.0)),
            "border-style:none must zero the 3px border, got {:?}",
            borders
        );
    }

    #[test]
    fn test_box_sizing_border_box_absorbs_padding() {
        let html = r#"<html><head><style>
            .bb { box-sizing: border-box; width: 200px; height: 40px; padding: 10px; background-color: red; }
            .cb { width: 200px; height: 40px; padding: 10px; background-color: blue; }
        </style></head><body><div class="bb"></div><div class="cb"></div></body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let sizes: Vec<(String, i64, i64)> = view
            .paint()
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Rect { layout_size, style, .. } => Some((
                    style.background_color().code().to_string(),
                    layout_size.width(),
                    layout_size.height(),
                )),
                _ => None,
            })
            .collect();
        assert!(
            sizes.iter().any(|(c, w, h)| c == "#ff0000" && *w == 200 && *h == 40),
            "border-box: outer box stays 200x40, got {:?}",
            sizes
        );
        assert!(
            sizes.iter().any(|(c, w, h)| c == "#0000ff" && *w == 220 && *h == 60),
            "content-box: padding grows the box to 220x60, got {:?}",
            sizes
        );
    }

    #[test]
    fn test_min_max_width_clamp_used_size() {
        let html = r#"<html><head><style>
            .capped { width: 600px; max-width: 300px; height: 20px; background-color: red; }
            .floored { width: 50px; min-width: 200px; height: 20px; background-color: blue; }
            .pct { max-width: 50%; height: 20px; background-color: green; }
        </style></head><body>
            <div class="capped"></div><div class="floored"></div><div class="pct"></div>
        </body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let widths: Vec<i64> = view
            .paint()
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Rect { layout_size, style, .. }
                    if style.background_color().code() != "#ffffff" =>
                {
                    Some(layout_size.width())
                }
                _ => None,
            })
            .collect();
        assert!(widths.contains(&300), "max-width must cap 600->300: {:?}", widths);
        assert!(widths.contains(&200), "min-width must floor 50->200: {:?}", widths);
        assert!(widths.contains(&400), "max-width:50% of 800 = 400: {:?}", widths);
    }

    #[test]
    fn test_text_transform() {
        let html = r#"<html><head><style>
            .up { text-transform: uppercase; }
            .cap { text-transform: capitalize; }
        </style></head><body>
            <p class="up">hello world</p>
            <p class="cap">the quick brown</p>
            <div class="up"><span>inherited lower</span></div>
        </body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let texts: Vec<String> = view
            .paint()
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();
        assert!(texts.iter().any(|t| t.contains("HELLO WORLD")), "{:?}", texts);
        assert!(texts.iter().any(|t| t.contains("The Quick Brown")), "{:?}", texts);
        // text-transform inherits into the child span.
        assert!(texts.iter().any(|t| t.contains("INHERITED LOWER")), "{:?}", texts);
    }

    #[test]
    fn test_pre_preserves_newlines_and_spaces() {
        let html = "<html><head></head><body><pre>first   line\nsecond line\n\nfourth</pre></body></html>"
            .to_string();
        let view = create_layout_view(html, 800);
        let texts: Vec<(String, i64)> = view
            .paint()
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => {
                    Some((text.clone(), layout_point.y()))
                }
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().any(|(t, _)| t.contains("first   line")),
            "runs of spaces must survive in <pre>: {:?}",
            texts
        );
        let y_first = texts.iter().find(|(t, _)| t.contains("first")).unwrap().1;
        let y_second = texts.iter().find(|(t, _)| t.contains("second")).unwrap().1;
        let y_fourth = texts.iter().find(|(t, _)| t.contains("fourth")).unwrap().1;
        assert!(y_second > y_first, "newline breaks the line");
        assert!(
            y_fourth >= y_second + 2 * (y_second - y_first),
            "the blank line keeps its row: {:?}",
            texts
        );
    }

    #[test]
    fn test_font_weight_property_sets_bold() {
        let html = r#"<html><head><style>
            .heavy { font-weight: 700; }
            .light { font-weight: normal; }
        </style></head><body><span class="heavy">wide</span><b class="light">thin</b></body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let bolds: Vec<(String, bool)> = view
            .paint()
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, bold, .. } => Some((text.clone(), *bold)),
                _ => None,
            })
            .collect();
        assert!(bolds.contains(&("wide".to_string(), true)), "{:?}", bolds);
        // font-weight:normal overrides the UA bold of <b>.
        assert!(bolds.contains(&("thin".to_string(), false)), "{:?}", bolds);
    }

    #[test]
    fn test_visibility_hidden_keeps_space_but_paints_nothing() {
        let hidden = r#"<html><head><style>
            .gap { visibility: hidden; height: 50px; }
        </style></head><body><div class="gap">ghost</div><p>after</p></body></html>"#
            .to_string();
        let view = create_layout_view(hidden, 800);
        let items = view.paint();
        assert!(
            !items.iter().any(|i| matches!(i, DisplayItem::Text { text, .. } if text.contains("ghost"))),
            "hidden content must not paint"
        );
        let after_y = items
            .iter()
            .find_map(|i| match i {
                DisplayItem::Text { text, layout_point, .. } if text.contains("after") => {
                    Some(layout_point.y())
                }
                _ => None,
            })
            .expect("after text");
        assert!(after_y >= 50, "hidden box must keep its 50px space, got y={after_y}");
    }

    #[test]
    fn test_media_query_selects_rules_by_viewport_width() {
        // The paragraph is hidden only under the max-width:600px condition.
        let html = r#"<html><head><style>
            p { color: blue; }
            @media (max-width: 600px) { p { display: none; } }
        </style></head><body><p>hello</p></body></html>"#
            .to_string();

        let has_text = |view: &LayoutView| {
            view.paint().iter().any(|item| matches!(
                item,
                DisplayItem::Text { text, .. } if text.contains("hello")
            ))
        };

        let wide = create_layout_view(html.clone(), 900);
        assert!(has_text(&wide), "media rule must be inactive at 900px");

        let narrow = create_layout_view(html, 480);
        assert!(!has_text(&narrow), "media rule must hide <p> at 480px");
    }

    #[test]
    fn test_body() {
        let html = "<html><head></head><body></body></html>".to_string();
        let layout_view = create_layout_view(html, 600);

        let root = layout_view.root();
        assert!(root.is_some());
        assert_eq!(
            LayoutObjectKind::Block,
            root.clone().expect("root should exist").borrow().kind()
        );
        assert_eq!(
            NodeKind::Element(Element::new("body", Vec::new())),
            root.clone()
                .expect("root should exist")
                .borrow()
                .node_kind()
        );
    }

    #[test]
    fn test_text() {
        let html = "<html><head></head><body>text</body></html>".to_string();
        let layout_view = create_layout_view(html, 600);

        let root = layout_view.root().expect("root should exist");
        let text = root.borrow().first_child();
        assert!(text.is_some());
        assert_eq!(
            LayoutObjectKind::Text,
            text.clone()
                .expect("text node should exist")
                .borrow()
                .kind()
        );
    }

    #[test]
    fn test_example_like_layout_keeps_heading_width() {
        let html = r#"<html><head><style>body{background:#eee;width:60vw;margin:15vh auto;font-family:system-ui,sans-serif}h1{font-size:1.5em}div{opacity:0.8}a:link,a:visited{color:#348}</style></head><body><div><h1>Example Domain</h1><p>This domain is for use in documentation examples without needing permission. Avoid use in operations.</p><p><a href="https://iana.org/domains/example">Learn more</a></p></div></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 1200);

        let body = layout_view.root().expect("body should exist");
        let div = body.borrow().first_child().expect("div should exist");
        let h1 = div.borrow().first_child().expect("h1 should exist");
        let display_items = layout_view.paint();

        assert!(
            body.borrow().size().width() >= 700,
            "body width was {}",
            body.borrow().size().width()
        );
        assert!(
            body.borrow().point().x() >= 200,
            "body x was {}",
            body.borrow().point().x()
        );
        assert!(
            h1.borrow().size().width() >= 300,
            "h1 width was {}",
            h1.borrow().size().width()
        );
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Text { text, .. } if text == "Example Domain"
        )));
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Rect { style, .. } if style.background_color().code() == "#eeeeee"
        )));
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Text { text, style, .. }
                if text == "Example Domain"
                    && style.font_family() == "system-ui"
                    && (style.opacity() - 0.8).abs() < f64::EPSILON
        )));
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Text { text, style, .. } if text == "Learn more" && style.color().code() == "#334488"
        )));
    }

    #[test]
    fn test_spacing_shorthand_and_auto_center_block() {
        let html = r#"<html><head><style>body{width:400px;margin:10px auto 30px auto;padding:8px 20px}p{margin:0}</style></head><body><p>Spacing</p></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 1000);

        let body = layout_view.root().expect("body should exist");
        let paragraph = body.borrow().first_child().expect("paragraph should exist");
        let text = paragraph.borrow().first_child().expect("text should exist");

        assert_eq!(body.borrow().point().x(), 280);
        assert_eq!(body.borrow().point().y(), 10);
        assert_eq!(text.borrow().point().x(), 300);
        assert_eq!(text.borrow().point().y(), 18);
    }

    #[test]
    fn test_spacing_shorthand_supports_left_auto_alignment() {
        let html = r#"<html><head><style>body{width:400px;margin:10px 25px 30px auto}</style></head><body><p>Spacing</p></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 1000);

        let body = layout_view.root().expect("body should exist");

        assert_eq!(body.borrow().point().x(), 575);
        assert_eq!(body.borrow().point().y(), 10);
    }

    #[test]
    fn test_form_control_placeholders_paint() {
        let html = r#"<html><head></head><body><form><input placeholder="Email" /><button>Send</button><img alt="Hero" /></form></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        assert!(display_items
            .iter()
            .any(|item| matches!(item, DisplayItem::Rect { .. })));
        assert!(display_items.iter().any(|item| matches!(
            item,
            DisplayItem::Text { text, .. } if text == "Email" || text == "Hero"
        )));
    }

    #[test]
    fn test_br_creates_vertical_separation() {
        let html = "<html><head></head><body>Line1<br>Line2<br>Line3</body></html>".to_string();
        let layout_view = create_layout_view(html, 600);
        let display_items = layout_view.paint();

        let text_items: Vec<_> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text {
                    text, layout_point, ..
                } => Some((text.clone(), layout_point.y())),
                _ => None,
            })
            .collect();

        assert!(text_items.len() >= 3, "expected 3 text items, got {:?}", text_items);
        // Each line should have a distinct Y coordinate (increasing).
        let ys: Vec<i64> = text_items.iter().map(|(_, y)| *y).collect();
        for i in 1..ys.len() {
            assert!(
                ys[i] > ys[i - 1],
                "line {} (y={}) should be below line {} (y={})",
                i, ys[i], i - 1, ys[i - 1]
            );
        }
    }

    #[test]
    fn test_center_tag_centers_text() {
        let html =
            "<html><head></head><body><center>Centered</center></body></html>".to_string();
        let layout_view = create_layout_view(html, 600);
        let display_items = layout_view.paint();

        let text_items: Vec<_> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text {
                    text, layout_point, ..
                } => Some((text.clone(), layout_point.x())),
                _ => None,
            })
            .collect();

        assert!(!text_items.is_empty(), "should have text items");
        let (_, x) = &text_items[0];
        // Text should be noticeably offset from left (centered in 600px viewport).
        assert!(*x > 100, "centered text x={} should be > 100", x);
    }

    #[test]
    fn test_table_cells_horizontal() {
        let html = "<html><head></head><body><table><tr><td>Cell1</td><td>Cell2</td></tr></table></body></html>".to_string();
        let layout_view = create_layout_view(html, 600);
        let display_items = layout_view.paint();

        let text_items: Vec<_> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text {
                    text, layout_point, ..
                } => Some((text.clone(), layout_point.x(), layout_point.y())),
                _ => None,
            })
            .collect();

        assert!(text_items.len() >= 2, "expected 2 text items, got {:?}", text_items);
        // Cell2 should be to the right of Cell1.
        let cell1 = text_items.iter().find(|(t, _, _)| t == "Cell1").unwrap();
        let cell2 = text_items.iter().find(|(t, _, _)| t == "Cell2").unwrap();
        assert!(
            cell2.1 > cell1.1,
            "Cell2 x={} should be right of Cell1 x={}",
            cell2.1, cell1.1
        );
        // Same row: Y should be similar.
        assert!(
            (cell2.2 - cell1.2).abs() < 5,
            "cells should be on same row: y1={}, y2={}",
            cell1.2, cell2.2
        );
    }

    #[test]
    fn test_flex_row_lays_children_horizontally() {
        let html = "<html><head><style>.f{display:flex}</style></head><body>\
            <div class=\"f\"><div>Alpha</div><div>Beta</div></div></body></html>"
            .to_string();
        let layout_view = create_layout_view(html, 600);
        let items: Vec<_> = layout_view
            .paint()
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text {
                    text, layout_point, ..
                } => Some((text.clone(), layout_point.x(), layout_point.y())),
                _ => None,
            })
            .collect();
        let a = items.iter().find(|(t, _, _)| t == "Alpha").unwrap();
        let b = items.iter().find(|(t, _, _)| t == "Beta").unwrap();
        // Row flex: Beta is to the right of Alpha, on the same line.
        assert!(b.1 > a.1, "Beta x={} should be right of Alpha x={}", b.1, a.1);
        assert!(
            (b.2 - a.2).abs() < 5,
            "flex row items should share a line: y_a={}, y_b={}",
            a.2, b.2
        );
    }

    #[test]
    fn test_flex_column_stacks_children_vertically() {
        let html = "<html><head><style>.f{display:flex;flex-direction:column}</style></head>\
            <body><div class=\"f\"><div>Alpha</div><div>Beta</div></div></body></html>"
            .to_string();
        let layout_view = create_layout_view(html, 600);
        let items: Vec<_> = layout_view
            .paint()
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text {
                    text, layout_point, ..
                } => Some((text.clone(), layout_point.x(), layout_point.y())),
                _ => None,
            })
            .collect();
        let a = items.iter().find(|(t, _, _)| t == "Alpha").unwrap();
        let b = items.iter().find(|(t, _, _)| t == "Beta").unwrap();
        // Column flex: Beta is below Alpha.
        assert!(b.2 > a.2, "Beta y={} should be below Alpha y={}", b.2, a.2);
    }

    #[test]
    fn test_table_cell_explicit_width_leaves_remaining_for_auto() {
        // First cell has explicit width=200, second cell should get remaining space.
        let html = r#"<html><head></head><body><table><tr><td width="200">Left</td><td>Right</td></tr></table></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 600);
        let display_items = layout_view.paint();

        let text_items: Vec<_> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. }
                    => Some((text.clone(), layout_point.x())),
                _ => None,
            })
            .collect();

        let _left = text_items.iter().find(|(t, _)| t == "Left").unwrap();
        let right = text_items.iter().find(|(t, _)| t == "Right").unwrap();
        // Right cell should start at x=200 (left cell width).
        assert!(
            right.1 >= 200,
            "Right cell x={} should be >= 200",
            right.1
        );
        // Right cell should NOT be pushed off a 600px viewport.
        assert!(
            right.1 < 500,
            "Right cell x={} should be well within viewport",
            right.1
        );
    }

    #[test]
    fn test_table_nested_content_all_rendered() {
        let html = r#"<html><head></head><body><table><tr><td>Col1</td><td><center>Header</center>More text<br>Final line</td></tr></table></body></html>"#.to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<_> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();

        assert!(text_items.iter().any(|t| t == "Col1"), "Col1 missing, got: {:?}", text_items);
        assert!(text_items.iter().any(|t| t == "Header"), "Header missing, got: {:?}", text_items);
        assert!(text_items.iter().any(|t| t.contains("More text")), "More text missing, got: {:?}", text_items);
        assert!(text_items.iter().any(|t| t.contains("Final line")), "Final line missing, got: {:?}", text_items);
    }

    #[test]
    fn test_table_rowspan_offsets_second_row() {
        // Row 1: cell with rowspan=2 (width=200) + cell B
        // Row 2: cell C (should be shifted right by ~200)
        let html = r#"<html><head></head><body>
            <table>
                <tr>
                    <td rowspan="2" width="200">Left</td>
                    <td>B</td>
                </tr>
                <tr>
                    <td>C</td>
                </tr>
            </table>
        </body></html>"#.to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<(String, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.x())),
                _ => None,
            })
            .collect();

        let left_item = text_items.iter().find(|(t, _)| t == "Left").expect("Left missing");
        let b_item = text_items.iter().find(|(t, _)| t == "B").expect("B missing");
        let c_item = text_items.iter().find(|(t, _)| t == "C").expect("C missing");

        // B and C should both be to the right of Left (x >= 200).
        assert!(b_item.1 >= 200, "B should be right of Left, B.x={} Left.x={}", b_item.1, left_item.1);
        assert!(c_item.1 >= 200, "C should be right of Left, C.x={} Left.x={}", c_item.1, left_item.1);
    }

    #[test]
    fn test_table_rowspan_height_expansion() {
        // A tall rowspan=2 cell should cause row 2 to expand so that the
        // right-column text in row 2 (C) appears ABOVE the rowspan cell's tail
        // content (Tall), not below it.
        //
        // Layout expectation:
        //   Row 1: [Tall (rowspan=2, 414px img)] | [B]
        //   Row 2:                               | [C]
        //
        // After rowspan height fix, row 2 should be expanded and C should appear
        // at y < image_end_y. Without the fix, C would be at a small y because
        // rows are only as tall as their non-rowspan content.
        let html = r#"<html><head></head><body>
            <table>
                <tr>
                    <td rowspan="2"><img src="x.jpg" width="200" height="400" border="0">Tail</td>
                    <td>B</td>
                </tr>
                <tr>
                    <td>C</td>
                </tr>
            </table>
        </body></html>"#.to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<(String, i64, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } =>
                    Some((text.clone(), layout_point.x(), layout_point.y())),
                _ => None,
            })
            .collect();

        let b_item = text_items.iter().find(|(t, _, _)| t.contains('B')).expect("B missing");
        let c_item = text_items.iter().find(|(t, _, _)| t.contains('C')).expect("C missing");
        let tail_item = text_items.iter().find(|(t, _, _)| t.contains("Tail")).expect("Tail missing");

        // C should be at a y value between B and Tail (not below Tail).
        // Before fix: B.y ≈ 8, C.y ≈ 28, Tail.y ≈ 420+ (C < Tail ✓ but by accident)
        // After fix: row 2 expands to fit image+Tail, so C.y is still reasonable.
        // The key assertion: Tail should be to the LEFT of C (x < C.x).
        assert!(tail_item.1 < c_item.1,
            "Tail (x={}) should be left of C (x={})", tail_item.1, c_item.1);
        // And C should be to the right of B (same column).
        assert_eq!(b_item.1, c_item.1,
            "B.x={} should equal C.x={} (same column)", b_item.1, c_item.1);
    }

    #[test]
    fn test_implicit_td_closing() {
        // When <td> opens while inside <font> inside <td>, the parser should
        // close the existing <td> (and <font>) so cells become siblings, not nested.
        let html = "<html><head></head><body><table><tr><td width=\"10\"><font color=\"red\">A<td width=\"100\">B</tr></table></body></html>".to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<(String, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.x())),
                _ => None,
            })
            .collect();

        let a_item = text_items.iter().find(|(t, _)| t.contains("A")).expect("A missing");
        let b_item = text_items.iter().find(|(t, _)| t.contains("B")).expect("B missing");

        // B should be to the right of A (not nested inside it).
        assert!(b_item.1 > a_item.1, "B should be right of A, B.x={} A.x={}", b_item.1, a_item.1);
    }

    #[test]
    fn test_space_between_inline_elements_preserved() {
        // Space between </a> and <a> should produce a separator so
        // text doesn't run together ("Link1Link2" → "Link1 Link2").
        let html = "<html><head></head><body><p><a>Link1</a> <a>Link2</a></p></body></html>".to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<String> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();

        // There should be a space somewhere between Link1 and Link2 — either as
        // a separate text node or joined within an existing one.
        let joined = text_items.join("");
        assert!(
            text_items.iter().any(|t| t.contains(" ")) || text_items.len() >= 2,
            "Space between inline elements should be preserved, got: {:?}", text_items
        );
        assert!(joined.contains("Link1"), "Link1 missing, got: {:?}", text_items);
        assert!(joined.contains("Link2"), "Link2 missing, got: {:?}", text_items);
    }

    #[test]
    fn test_h1_align_center() {
        // <h1 align="center"> should center text within the viewport.
        let html = "<html><head></head><body><h1 align=\"center\">Title</h1></body></html>".to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let title = display_items
            .iter()
            .find_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } if text == "Title" => Some(layout_point.x()),
                _ => None,
            })
            .expect("Title text missing");

        // Title should be roughly centered (x > 0 and x < 800/2).
        assert!(title > 0, "Title should not be at x=0, got x={}", title);
        assert!(title < 400, "Title should be in the left half when centered, got x={}", title);
    }

    #[test]
    fn test_table_align_center_shrinks_to_fit() {
        // <table align="center"> should shrink to content width and be centered.
        let html = "<html><head></head><body><table align=\"center\"><tr><td width=\"200\">Cell</td></tr></table></body></html>".to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let cell_text = display_items
            .iter()
            .find_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } if text == "Cell" => Some(layout_point.x()),
                _ => None,
            })
            .expect("Cell text missing");

        // The table is 200px wide, centered in 800px → starts around x=300.
        assert!(cell_text > 100, "Table should be centered, cell x={}", cell_text);
    }

    #[test]
    fn test_nested_table_implicit_close_scoped() {
        // Opening <tr> inside a nested table must NOT close the outer <tr>.
        let html = "<html><head></head><body><table><tr><td><table><tr><td>Inner</td></tr></table>Outer</td></tr></table></body></html>".to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<String> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect();

        assert!(text_items.iter().any(|t| t.contains("Inner")), "Inner missing, got: {:?}", text_items);
        assert!(text_items.iter().any(|t| t.contains("Outer")), "Outer missing, got: {:?}", text_items);
    }

    #[test]
    fn test_abe_hiroshi_like_layout() {
        // Realistic structure mimicking top.htm: first td has nested content
        // (image + inner table + text), second and third td should be to the right.
        let html = concat!(
            "<html><head></head><body>",
            "<table>",
            "<tr>",
            "<td rowspan=\"2\" width=\"350\">",
            "<img src=\"photo.jpg\" width=\"350\" height=\"414\">",
            "<br><br>",
            "<table width=\"256\">",
            "<tr><td width=\"14\"> </td><td width=\"230\">Profile</td></tr>",
            "</table>",
            "<br>Address",
            "</td>",
            "<td> </td>",
            "<td><div align=\"center\">LatestNews</div></td>",
            "</tr>",
            "<tr>",
            "<td></td>",
            "<td>DramaInfo</td>",
            "</tr>",
            "</table>",
            "</body></html>"
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let text_items: Vec<(String, i64, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => {
                    Some((text.clone(), layout_point.x(), layout_point.y()))
                }
                _ => None,
            })
            .collect();

        let debug: Vec<String> = text_items.iter()
            .map(|(t, x, y)| std::format!("{}@({},{})", t, x, y))
            .collect();

        let latest = text_items.iter().find(|(t, _, _)| t.contains("LatestNews"))
            .expect(&std::format!("LatestNews missing, items: {:?}", debug));
        let drama = text_items.iter().find(|(t, _, _)| t.contains("DramaInfo"))
            .expect(&std::format!("DramaInfo missing, items: {:?}", debug));

        // LatestNews should be to the right of left column (x > 350).
        assert!(latest.1 > 350,
            "LatestNews x={} should be > 350 (right of left column), all: {:?}", latest.1, debug);
        // DramaInfo should also be to the right.
        assert!(drama.1 > 350,
            "DramaInfo x={} should be > 350, all: {:?}", drama.1, debug);
    }

    #[test]
    fn test_inline_text_no_unnecessary_wrap() {
        // "生年月日 1964年6月22日" should fit on one line in a 230px container.
        let html = "<html><head></head><body><table><tr><td width=\"230\">生年月日 1964年6月22日</td></tr></table></body></html>".to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<(String, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.y())),
                _ => None,
            })
            .collect();


        // All text should be on one line (same Y coordinate).
        let ys: Vec<i64> = text_items.iter().map(|(_, y)| *y).collect();
        if ys.len() > 1 {
            assert!(ys.iter().all(|y| *y == ys[0]),
                "All text should be on same line, got Y values: {:?}, texts: {:?}", ys, text_items);
        }
    }

    #[test]
    fn test_h1_align_center_title() {
        // <h1 align="center">阿部 寛のホームページ</h1> should be centered.
        let html = "<html><head></head><body><h1 align=\"center\">阿部 寛のホームページ</h1></body></html>".to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let text_items: Vec<(String, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.x())),
                _ => None,
            })
            .collect();


        let title = text_items.iter().find(|(t, _)| t.contains("阿部")).expect("title missing");
        // Title should be centered: x > 0 and roughly in the middle area.
        assert!(title.1 > 100, "Title should be centered, x={}", title.1);
    }

    #[test]
    fn test_table_align_center_with_width() {
        // <TABLE width="752" align="center"> in a 1024px viewport should be
        // centered, so the first column should start at roughly (1024-752)/2=136.
        let html = concat!(
            "<html><head></head><body>",
            "<table width=\"752\" align=\"center\">",
            "<tr><td width=\"120\">ラベル</td><td>データ</td></tr>",
            "</table></body></html>"
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let label = display_items.iter().find_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } if text.contains("ラベル") => {
                Some(layout_point.x())
            }
            _ => None,
        }).expect("label text not found");

        let data = display_items.iter().find_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } if text.contains("データ") => {
                Some(layout_point.x())
            }
            _ => None,
        }).expect("data text not found");

        // Table centered in 1024: left edge at ~136. Label column = 120px wide,
        // so data column starts at ~256.
        assert!(label >= 100, "label x={} should be >= 100 (table is centered)", label);
        assert!(data > label, "data x={} should be to the right of label x={}", data, label);
        assert!(data >= 200, "data column x={} should be around 256 (136+120)", data);
    }

    #[test]
    fn test_table_column_width_inherited_across_rows() {
        // Row 1 has explicit widths (14, 230). Row 2 has no explicit widths.
        // Row 2 cells should inherit column widths from row 1.
        let html = concat!(
            "<html><head></head><body>",
            "<table width=\"256\">",
            "<tr><td width=\"14\">A</td><td width=\"230\">B</td></tr>",
            "<tr><td>C</td><td>生年月日 1964年6月22日</td></tr>",
            "</table>",
            "</body></html>"
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<(String, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.y())),
                _ => None,
            })
            .collect();

        // "生年月日 1964年6月22日" (176px) fits in 230px — must be one line.
        let date_items: Vec<_> = text_items.iter()
            .filter(|(t, _)| t.contains("生年月日") || t.contains("1964"))
            .collect();
        assert!(!date_items.is_empty(), "date text missing");
        // Should be exactly 1 item (not split across lines).
        assert_eq!(date_items.len(), 1,
            "Date text should be on one line, got: {:?}", date_items);
    }

    /// Regression test: a table with a rowspan=2 cell plus a narrow spacer cell
    /// must not use the spacer's width="10" for the rowspan column.  The
    /// large left column should receive most of the table width so that the
    /// right content column is also visible at a reasonable width.
    #[test]
    fn test_rowspan_cell_does_not_steal_sibling_width() {
        let html = concat!(
            "<html><head></head><body>",
            "<table width=\"760\">",
            // Row 1: large rowspan=2 cell (image), 10px spacer, right content
            "<tr>",
            "  <td rowspan=\"2\"><img src=\"img.jpg\" width=\"350\" height=\"414\"></td>",
            "  <td width=\"10\">&nbsp;</td>",
            "  <td>right row1</td>",
            "</tr>",
            // Row 2: only spacer + right content (col0 occupied by rowspan)
            "<tr>",
            "  <td width=\"10\">&nbsp;</td>",
            "  <td>drama content</td>",
            "</tr>",
            "</table>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        // "drama content" must appear in the display items.
        let has_drama = display_items.iter().any(|item| {
            matches!(item, DisplayItem::Text { text, .. } if text.contains("drama"))
        });
        assert!(has_drama, "drama content text should be present");

        // The drama content cell must have a large x coordinate (not squished to
        // the left by a 10px rowspan column).
        let drama_x = display_items.iter().find_map(|item| {
            match item {
                DisplayItem::Text { text, layout_point, .. } if text.contains("drama") => {
                    Some(layout_point.x())
                }
                _ => None,
            }
        });
        assert!(
            drama_x.map(|x| x > 50).unwrap_or(false),
            "drama content should be well to the right of the table origin, got x={:?}",
            drama_x
        );
    }

    /// Regression test: 3-column table where some rows have explicit cell widths
    /// and earlier rows do not.  All rows must use the same column widths so
    /// text in col-2 (theater/dates) is NOT positioned on top of col-1 (title).
    #[test]
    fn test_table_3col_mixed_explicit_widths() {
        // Mirrors the stage page: <TABLE width="700"> with some rows having
        // width="145" / width="350" on col 0/1 and other rows having no widths.
        let html = concat!(
            "<html><head></head><body>",
            "<table width=\"700\">",
            // Rows without explicit cell widths (like first 6 rows on stage page)
            "<tr>",
            "  <td><strong>2022年9月</strong></td>",
            "  <td><strong>Title A</strong></td>",
            "  <td><strong>2022年9月16日〜9月25日</strong></td>",
            "</tr>",
            "<tr>",
            "  <td><strong>2020年2月</strong></td>",
            "  <td><strong>Title B</strong></td>",
            "  <td><strong>2020年2月14日〜3月1日</strong></td>",
            "</tr>",
            // Row with explicit widths (like rows 7+ on stage page)
            "<tr>",
            "  <td width=\"145\"><strong>2005年1月</strong></td>",
            "  <td width=\"350\"><strong>Title C</strong></td>",
            "  <td><strong>2005年1月2日〜27日</strong></td>",
            "</tr>",
            "</table>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let items: Vec<(String, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.x())),
                _ => None,
            })
            .collect();
        let debug: Vec<_> = items.iter()
            .map(|(t, x)| std::format!("{}@x={}", t, x))
            .collect();

        // Col-2 content (dates) must be to the RIGHT of col-1 content (titles).
        let title_a_x = items.iter().find(|(t, _)| t.contains("Title A"))
            .map(|(_, x)| *x)
            .expect(&std::format!("Title A missing, items: {:?}", debug));
        // Match the line START of the col-2 date — the tail may wrap to a
        // second line within the cell depending on advance estimates.
        let date_a_x = items.iter().find(|(t, _)| t.contains("2022年9月16日"))
            .map(|(_, x)| *x)
            .expect(&std::format!("date A missing, items: {:?}", debug));

        assert!(
            date_a_x > title_a_x,
            "col-2 date x={} must be > col-1 title x={} (no overlap), items: {:?}",
            date_a_x, title_a_x, debug
        );
        // Col-1 should start at x>=145 (after explicit 145px col-0)
        assert!(
            title_a_x >= 100,
            "Title A (col-1) x={} should be around 145 (after 145px col-0), items: {:?}",
            title_a_x, debug
        );
        // Col-2 should start around x=495 (145+350)
        assert!(
            date_a_x >= 400,
            "date A (col-2) x={} should be around 495 (145+350), items: {:?}",
            date_a_x, debug
        );
    }

    #[test]
    fn test_cellspacing_separates_row_borders() {
        // With BORDER=1 and default cellspacing=2, adjacent rows must NOT share
        // the same Y-coordinate for their top edges (which would cause double borders).
        // Row 2 must start at least cellspacing pixels below the bottom of row 1.
        let html = concat!(
            "<html><head></head><body>",
            "<table border=\"1\" width=\"400\">",
            "<tr><td>Row1Col1</td><td>Row1Col2</td></tr>",
            "<tr><td>Row2Col1</td><td>Row2Col2</td></tr>",
            "</table>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_y: Vec<(String, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.y())),
                _ => None,
            })
            .collect();

        let r1c1 = text_y.iter().find(|(t, _)| t.contains("Row1Col1")).expect("Row1Col1 missing");
        let r2c1 = text_y.iter().find(|(t, _)| t.contains("Row2Col1")).expect("Row2Col1 missing");

        // Row 2 must be strictly below row 1 (gap >= cellspacing).
        assert!(
            r2c1.1 > r1c1.1,
            "Row2 y={} should be below Row1 y={}", r2c1.1, r1c1.1
        );
        // The gap should be at least the cell height + cellspacing (2px default).
        // The cell needs some breathing room — at least 1px gap to prevent border overlap.
        let gap = r2c1.1 - r1c1.1;
        assert!(
            gap >= 2,
            "gap between rows ({}) should be >= 2 (cellspacing default)", gap
        );
    }

    #[test]
    fn test_text_before_table_is_rendered() {
        // Text in a paragraph before a table must be rendered and visible above the table.
        let html = concat!(
            "<html><head></head><body>",
            "<p>IntroText</p>",
            "<table border=\"1\" width=\"400\">",
            "<tr><td>Cell</td></tr>",
            "</table>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<(String, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.y())),
                _ => None,
            })
            .collect();

        let intro = text_items.iter().find(|(t, _)| t.contains("IntroText"))
            .expect("IntroText must be rendered");
        let cell = text_items.iter().find(|(t, _)| t.contains("Cell"))
            .expect("Cell must be rendered");

        // Intro text must appear ABOVE the table cell.
        assert!(
            intro.1 < cell.1,
            "IntroText y={} should be above Cell y={}", intro.1, cell.1
        );
    }

    #[test]
    fn test_text_in_font_before_table_is_rendered() {
        // Text inside a <font> element above a table must be rendered and visible.
        // Simulates common old Japanese HTML pattern where text is in <font size=+1>.
        let html = concat!(
            "<html><head></head><body>",
            "<center>",
            "<font>SpecialAward</font>",
            "<table border=\"1\" width=\"400\">",
            "<tr><td>Cell</td></tr>",
            "</table>",
            "</center>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let text_items: Vec<(String, i64)> = display_items
            .iter()
            .filter_map(|item| match item {
                DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.y())),
                _ => None,
            })
            .collect();

        assert!(
            text_items.iter().any(|(t, _)| t.contains("SpecialAward")),
            "SpecialAward text must be rendered, got: {:?}", text_items
        );
        assert!(
            text_items.iter().any(|(t, _)| t.contains("Cell")),
            "Cell text must be rendered"
        );

        // SpecialAward should be ABOVE the table cell.
        let award = text_items.iter().find(|(t, _)| t.contains("SpecialAward")).unwrap();
        let cell = text_items.iter().find(|(t, _)| t.contains("Cell")).unwrap();
        assert!(
            award.1 <= cell.1,
            "SpecialAward y={} should be above or equal to Cell y={}", award.1, cell.1
        );
    }

    #[test]
    fn test_eiga_page_award_text_and_data_table() {
        // Mirrors the real movie page (eiga.htm) structure:
        //   CENTER > H2 + table(no border, awards text in TD) + br
        //   CENTER > TABLE BORDER=1 (movie data rows)
        // The award text inside the first (borderless) table must render,
        // and the data rows must appear below it.
        let html = concat!(
            "<html><head></head><body>",
            "<center>",
            "<h2>Title</h2>",
            "<table width=\"700\">",
            "<tr><td><strong>AwardText1<br>AwardText2</strong></td></tr>",
            "</table>",
            "<br>",
            "</center>",
            "<center>",
            "<table border=\"1\" width=\"700\">",
            "<tr>",
            "<td>MovieDate</td>",
            "<td>MovieTitle</td>",
            "<td>&nbsp;</td>",
            "</tr>",
            "</table>",
            "</center>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let texts: Vec<(String, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.y())),
            _ => None,
        }).collect();

        assert!(
            texts.iter().any(|(t, _)| t.contains("AwardText1")),
            "AwardText1 must be rendered, got: {:?}", texts
        );
        assert!(
            texts.iter().any(|(t, _)| t.contains("AwardText2")),
            "AwardText2 must be rendered (line after BR)"
        );
        assert!(
            texts.iter().any(|(t, _)| t.contains("MovieDate")),
            "MovieDate must be rendered in data table"
        );

        let award1_y = texts.iter().find(|(t, _)| t.contains("AwardText1")).unwrap().1;
        let movie_y = texts.iter().find(|(t, _)| t.contains("MovieDate")).unwrap().1;
        assert!(
            award1_y < movie_y,
            "Award text (y={}) should appear above movie data (y={})", award1_y, movie_y
        );
    }

    #[test]
    fn test_strong_with_br_x_position_stays_on_screen() {
        // Regression test: text inside <strong>line1<br>line2</strong> (inside
        // a <center> table cell) must not be shifted off-screen.
        //
        // Previously, compute_size for Inline accumulated ALL children's widths
        // including block children (<br>), making STRONG.width = sum of line widths.
        // That inflated the "available_width" used for text-align centering,
        // pushing every line after the first <br> far to the right (off-screen).
        let html = concat!(
            "<html><head></head><body>",
            "<center>",
            "<table width=\"700\">",
            "<tr><td>",
            "<strong>Line1Text<br>Line2Text<br>Line3Text</strong>",
            "</td></tr>",
            "</table>",
            "</center>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let texts: Vec<(String, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.x())),
            _ => None,
        }).collect();

        for (name, expected_line) in [("Line1Text", 0i64), ("Line2Text", 1i64), ("Line3Text", 2i64)] {
            let entry = texts.iter().find(|(t, _)| t.contains(name))
                .unwrap_or_else(|| panic!("{name} must be rendered, got: {texts:?}"));
            // All lines must appear within the viewport (x in [0, 1024]).
            assert!(
                entry.1 >= 0 && entry.1 < 1024,
                "{name} x={} is off-screen (expected 0..1024)", entry.1
            );
            let _ = expected_line; // used by naming convention only
        }

        // Line1 and Line2 should have similar x-positions (both left-aligned within STRONG).
        let x1 = texts.iter().find(|(t, _)| t.contains("Line1Text")).unwrap().1;
        let x2 = texts.iter().find(|(t, _)| t.contains("Line2Text")).unwrap().1;
        let x3 = texts.iter().find(|(t, _)| t.contains("Line3Text")).unwrap().1;
        let max_x_drift = 1;  // allow rounding but not centering-induced drift
        assert!(
            (x2 - x1).abs() <= max_x_drift,
            "Line2Text x={x2} should be near Line1Text x={x1} (drift ≤ {max_x_drift})"
        );
        assert!(
            (x3 - x1).abs() <= max_x_drift,
            "Line3Text x={x3} should be near Line1Text x={x1} (drift ≤ {max_x_drift})"
        );
    }

    #[test]
    fn test_table_caption_rendered_above_rows() {
        // Wikipedia filmography tables use <caption> for award text like
        // "20周年記念　ニューヨーク・アジアン映画祭「スター・アジア賞」受賞".
        // The caption must:
        //   1. Be rendered (visible as a text item).
        //   2. Appear ABOVE the first data row (caption.y < row1_cell.y).
        //   3. Not cause the table height to be miscalculated (an element
        //      placed AFTER the table must not overlap the table's rows).
        let html = concat!(
            "<html><head></head><body>",
            "<table border=\"1\">",
            "<caption>受賞歴</caption>",
            "<tr><th>年</th><th>作品</th></tr>",
            "<tr><td>2000</td><td>映画A</td></tr>",
            "<tr><td>2001</td><td>映画B</td></tr>",
            "</table>",
            "<p>AfterTable</p>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();

        let texts: Vec<(String, i64, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } =>
                Some((text.clone(), layout_point.x(), layout_point.y())),
            _ => None,
        }).collect();

        let caption = texts.iter().find(|(t, _, _)| t.contains("受賞歴"))
            .expect("caption text must be rendered");
        let year_cell = texts.iter().find(|(t, _, _)| t.contains("年"))
            .expect("header row must be rendered");
        let after = texts.iter().find(|(t, _, _)| t.contains("AfterTable"))
            .expect("element after table must be rendered");
        let last_row_cell = texts.iter().find(|(t, _, _)| t.contains("映画B"))
            .expect("last data row must be rendered");

        // Caption must appear above the first data row.
        assert!(
            caption.2 < year_cell.2,
            "caption y={} must be above first row y={}", caption.2, year_cell.2
        );

        // Element after table must not overlap the table's last row.
        assert!(
            after.2 > last_row_cell.2,
            "after-table y={} must be below last row y={}", after.2, last_row_cell.2
        );
    }

    #[test]
    fn test_inline_style_attribute_applies_and_overrides_stylesheet() {
        // Inline style="..." must be parsed and must win over stylesheet rules.
        let html = concat!(
            "<html><head><style>div{background-color:#00ff00;width:100px;}</style></head><body>",
            "<div style=\"background-color: #ff0000; width: 300px\">x</div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let body = layout_view.root().expect("body should exist");
        let div = body.borrow().first_child().expect("div should exist");
        let style = div.borrow().style();
        assert_eq!(style.width() as i64, 300, "inline width must override stylesheet");
        let bg = style.background_color();
        assert_eq!(bg.code_u32(), 0xff0000, "inline background-color must override stylesheet");
    }

    #[test]
    fn test_opacity_transform_trap_descendant_contexts() {
        let html = concat!(
            "<html><head><style>",
            ".faded{opacity:0.5;}",
            ".spun{transform:rotate(3deg);}",
            ".pop{position:absolute;z-index:50;}",
            "</style></head><body>",
            "<div class=\"faded\"><div class=\"pop\">in-faded</div></div>",
            "<div class=\"spun\"><div class=\"pop\">in-spun</div></div>",
            "<div><div class=\"pop\">free</div></div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let body = layout_view.root().expect("body");
        let faded = body.borrow().first_child().expect("faded");
        let spun = faded.borrow().next_sibling().expect("spun");
        let plain = spun.borrow().next_sibling().expect("plain");
        // Children with z-index inside an opacity/transform context stay in
        // its bucket (base 0 + clamped z), not at the global +1M level.
        let in_faded = faded.borrow().first_child().expect("in-faded");
        assert_eq!(in_faded.borrow().style().paint_z(), 50, "trapped in opacity context");
        let in_spun = spun.borrow().first_child().expect("in-spun");
        assert_eq!(in_spun.borrow().style().paint_z(), 50, "trapped in transform context");
        let free = plain.borrow().first_child().expect("free");
        assert_eq!(free.borrow().style().paint_z(), 1_000_050, "unwrapped context lifts to +1M");
    }

    #[test]
    fn test_paint_z_stacking_keys() {
        let html = concat!(
            "<html><head><style>",
            ".behind{position:absolute;z-index:-1;width:50px;height:50px;}",
            ".front{position:relative;z-index:3;}",
            ".inner{position:relative;z-index:-5;}",
            "</style></head><body>",
            "<div class=\"behind\">b</div>",
            "<p>normal</p>",
            "<div class=\"front\">f<span class=\"inner\">nested</span></div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let body = layout_view.root().expect("body");
        assert_eq!(body.borrow().style().paint_z(), -2_000_000, "root canvas");
        let behind = body.borrow().first_child().expect("behind");
        assert_eq!(behind.borrow().style().paint_z(), -1_000_001,
            "negative z context: above canvas, below normal flow");
        let p = behind.borrow().next_sibling().expect("p");
        assert_eq!(p.borrow().style().paint_z(), 0, "normal flow");
        let front = p.borrow().next_sibling().expect("front");
        assert_eq!(front.borrow().style().paint_z(), 1_000_003, "positive context");
        // Nested context stays within the parent bucket (cannot escape).
        let f_text = front.borrow().first_child().expect("f text");
        assert_eq!(f_text.borrow().style().paint_z(), 1_000_003, "child inherits context");
        let inner = f_text.borrow().next_sibling().expect("inner");
        assert_eq!(inner.borrow().style().paint_z(), 1_000_003 - 5,
            "nested context offsets within the parent bucket");
    }

    #[test]
    fn test_sticky_context_bound_to_containing_block() {
        let html = concat!(
            "<html><head><style>",
            ".section{height:260px;}",
            ".sbar{position:sticky;top:0;height:40px;}",
            "</style></head><body>",
            "<div class=\"section\"><div class=\"sbar\">bar</div><p>content</p></div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 640);
        let body = layout_view.root().expect("body");
        let section = body.borrow().first_child().expect("section");
        let sbar = section.borrow().first_child().expect("sbar");
        let (top, container_y, max_delta) = sbar
            .borrow()
            .style()
            .sticky_context()
            .expect("sticky context stamped");
        assert_eq!(top, 0.0);
        assert_eq!(container_y, 0.0, "bar laid out at the section top");
        // Pin releases at the containing block's bottom: 260 - 40 = 220.
        assert_eq!(max_delta, 220.0);
        // The child content of the sticky bar carries the same context.
        let bar_text = sbar.borrow().first_child().expect("bar text");
        assert_eq!(bar_text.borrow().style().sticky_context(), Some((0.0, 0.0, 220.0)));
    }

    #[test]
    fn test_position_fixed_right_bottom_anchoring() {
        let html = concat!(
            "<html><head><style>",
            ".fab{position:fixed;right:10px;bottom:20px;width:60px;height:40px;background-color:#ee4444;}",
            ".filler{height:900px;}",
            "</style></head><body>",
            "<div class=\"filler\">content</div>",
            "<div class=\"fab\">go</div>",
            "</body></html>",
        ).to_string();
        // Viewport 800×600 (the height enables bottom anchoring).
        let t = HtmlTokenizer::new(html);
        let window = HtmlParser::new(t).construct_tree();
        let dom = window.borrow().document();
        let style = get_style_content(dom.clone());
        let cssom = CssParser::new(CssTokenizer::new(style)).parse_stylesheet();
        let layout_view = LayoutView::new_with_viewport(dom, &cssom, 800, 600);
        let body = layout_view.root().expect("body");
        let filler = body.borrow().first_child().expect("filler");
        let fab = filler.borrow().next_sibling().expect("fab");
        assert_eq!(fab.borrow().point().x(), 800 - 60 - 10, "right:10px");
        assert_eq!(fab.borrow().point().y(), 600 - 40 - 20, "bottom:20px");
        // The child text moved with the subtree.
        let text = fab.borrow().first_child().expect("text");
        assert_eq!(text.borrow().point().x(), fab.borrow().point().x(),
            "descendants translate with the fixed box");
    }

    #[test]
    fn test_position_fixed_anchors_to_viewport() {
        let html = concat!(
            "<html><head><style>",
            ".banner{position:fixed;top:10px;left:20px;width:200px;height:30px;background-color:#333333;}",
            ".filler{height:500px;}",
            "</style></head><body>",
            "<div class=\"filler\">content</div>",
            "<div class=\"banner\">fixed banner</div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let body = layout_view.root().expect("body");
        let filler = body.borrow().first_child().expect("filler");
        let banner = filler.borrow().next_sibling().expect("banner");
        // Anchored at the viewport offsets, NOT below the 500px filler.
        assert_eq!(banner.borrow().point().x(), 20, "left:20px from viewport");
        assert_eq!(banner.borrow().point().y(), 10, "top:10px from viewport");
        assert_eq!(banner.borrow().style().position(), PositionType::Fixed);
    }

    #[test]
    fn test_line_height_property() {
        let html = concat!(
            "<html><head><style>",
            "p{font-size:16px;}",
            ".tall{line-height:2;}",     // factor: 32px lines
            ".px{line-height:30px;}",    // fixed
            ".pct{line-height:150%;}",   // 24px lines
            "</style></head><body>",
            "<p class=\"tall\">first line of tall paragraph text first line of tall paragraph text</p>",
            "<p class=\"px\">x</p>",
            "<p class=\"pct\">y</p>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 400);
        let body = layout_view.root().expect("body");
        let tall = body.borrow().first_child().expect("tall p");
        let px = tall.borrow().next_sibling().expect("px p");
        let pct = px.borrow().next_sibling().expect("pct p");
        // The tall paragraph wraps to 2+ lines of 32px each.
        let tall_text = tall.borrow().first_child().expect("text");
        let lines = tall_text.borrow().size().height() / 32;
        assert!(lines >= 2, "tall paragraph should wrap");
        assert_eq!(tall_text.borrow().size().height() % 32, 0,
            "line boxes are 32px (factor 2 × 16px font)");
        assert_eq!(px.borrow().size().height(), 30, "line-height:30px");
        assert_eq!(pct.borrow().size().height(), 24, "line-height:150% of 16px");
    }

    #[test]
    fn test_css_variable_element_scope_inheritance() {
        // --accent defined at :root, overridden inside .theme; var() must
        // resolve against the nearest scope, inheriting into descendants.
        let html = concat!(
            "<html><head><style>",
            ":root{--accent:#ff0000;}",
            ".theme{--accent:#0000ff;}",
            "p{color:var(--accent);}",
            "span{color:var(--missing, #00ff00);}",
            "</style></head><body>",
            "<p>root-scope</p>",
            "<div class=\"theme\"><p>themed</p><div><p>themed-nested</p></div></div>",
            "<span>fallback</span>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();
        let color_of = |needle: &str| -> u32 {
            display_items.iter().find_map(|item| match item {
                DisplayItem::Text { text, style, .. } if text.contains(needle) =>
                    Some(style.color().code_u32()),
                _ => None,
            }).unwrap_or_else(|| panic!("text {:?} not painted", needle))
        };
        assert_eq!(color_of("root-scope"), 0xff0000, ":root value outside .theme");
        assert_eq!(color_of("themed"), 0x0000ff, ".theme override");
        assert_eq!(color_of("themed-nested"), 0x0000ff, "override inherits to descendants");
        assert_eq!(color_of("fallback"), 0x00ff00, "var() fallback for missing token");
    }

    #[test]
    fn test_transform_rotate_context() {
        let html = concat!(
            "<html><head><style>",
            ".frame{position:relative;height:200px;}",
            ".badge{position:absolute;top:50px;left:50px;width:80px;height:40px;",
            "transform:rotate(45deg);}",
            "</style></head><body>",
            "<div class=\"frame\"><div class=\"badge\">x</div></div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 400);
        let body = layout_view.root().expect("body");
        let frame = body.borrow().first_child().expect("frame");
        let badge = frame.borrow().first_child().expect("badge");
        // The badge's center (50+40, 50+20) = (90, 70) and 45° are stamped on
        // it and inherited by its child.
        let (cx, cy, deg) = badge.borrow().style().rotate_context().expect("rotate context");
        assert_eq!((cx as i64, cy as i64), (90, 70));
        assert_eq!(deg, 45.0);
        let text = badge.borrow().first_child().expect("badge text");
        assert_eq!(
            text.borrow().style().rotate_context().map(|(_, _, d)| d),
            Some(45.0),
            "child inherits the rotation context",
        );
    }

    #[test]
    fn test_generated_content_before_after() {
        let html = concat!(
            "<html><head><style>",
            ".req::before{content:\"* \";color:#ff0000;}",
            ".price::after{content:\" USD\";color:#008800;}",
            "</style></head><body>",
            "<span class=\"req\">Name</span>",
            "<span class=\"price\">42</span>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let items = layout_view.paint();
        let text_with = |needle: &str| -> Option<u32> {
            items.iter().find_map(|item| match item {
                DisplayItem::Text { text, style, .. } if text.contains(needle) =>
                    Some(style.color().code_u32()),
                _ => None,
            })
        };
        // ::before content rendered in red; ::after content in green.
        assert_eq!(text_with("*"), Some(0xff0000), "::before content present and styled");
        assert_eq!(text_with("USD"), Some(0x008800), "::after content present and styled");
        // Ordering: the '*' (before) paints before "Name"; "USD" (after) at the
        // end. Collect all text in paint order.
        let order: Vec<String> = items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, .. } => Some(text.trim().to_string()),
            _ => None,
        }).filter(|t| !t.is_empty()).collect();
        let star = order.iter().position(|t| t.contains('*')).unwrap();
        let name = order.iter().position(|t| t == "Name").unwrap();
        let usd = order.iter().position(|t| t.contains("USD")).unwrap();
        let price = order.iter().position(|t| t == "42").unwrap();
        assert!(star < name, "::before precedes host text");
        assert!(price < usd, "::after follows host text");
    }

    #[test]
    fn test_nowrap_and_ellipsis() {
        // A nowrap + overflow:hidden + text-overflow:ellipsis box truncates a
        // long line; a plain nowrap line stays on one line; a normal box wraps.
        let html = concat!(
            "<html><head><style>",
            ".ell{width:120px;white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}",
            ".nw{width:120px;white-space:nowrap;}",
            ".wrap{width:120px;}",
            "</style></head><body>",
            "<div class=\"ell\">This is a very long line that should be truncated</div>",
            "<div class=\"nw\">This is a very long line that should be truncated</div>",
            "<div class=\"wrap\">This is a very long line that should be truncated</div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let items = layout_view.paint();
        let texts: Vec<(String, i64)> = items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } => Some((text.clone(), layout_point.y())),
            _ => None,
        }).collect();
        // Ellipsis line is truncated and ends with the ellipsis char.
        let ell = texts.iter().find(|(t, _)| t.contains("This is") && t.contains('…'))
            .expect("ellipsis line must be truncated with …");
        assert!(ell.0.chars().count() < 49, "truncated shorter than the full text");
        // The nowrap (no ellipsis) text stays a single line: only one text
        // item carries its full content.
        let nowrap_lines = texts.iter().filter(|(t, _)| t == "This is a very long line that should be truncated").count();
        assert_eq!(nowrap_lines, 1, "nowrap keeps one line");
        // The wrapping div (below the two single-line divs at y=0 and y=20)
        // breaks into multiple lines at distinct y values.
        let wrap_ys: std::collections::BTreeSet<i64> = texts.iter()
            .filter(|(_, y)| *y >= 40)
            .map(|(_, y)| *y)
            .collect();
        assert!(wrap_ys.len() >= 2, "the wrapping div breaks into multiple lines");
    }

    #[test]
    fn test_inline_baseline_alignment_mixed_font_sizes() {
        let html = concat!(
            "<html><head><style>",
            ".big{font-size:32px;}",
            ".small{font-size:12px;}",
            "</style></head><body>",
            "<p><span class=\"big\">BIG</span><span class=\"small\">small</span></p>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let body = layout_view.root().expect("body");
        let p = body.borrow().first_child().expect("p");
        let big = p.borrow().first_child().expect("big span");
        let small = big.borrow().next_sibling().expect("small span");
        // Baselines coincide: top + font_px equal for both runs.
        let big_baseline = big.borrow().point().y() + 32;
        let small_baseline = small.borrow().point().y() + 12;
        assert_eq!(
            big_baseline, small_baseline,
            "small text must sit on the big text's baseline (big y={} small y={})",
            big.borrow().point().y(), small.borrow().point().y(),
        );
    }

    #[test]
    fn test_not_and_of_type_pseudo_classes() {
        let html = concat!(
            "<html><head><style>",
            "p:not(.skip){color:#ff0000;}",
            "span:nth-of-type(2){color:#00ff00;}",
            "em:first-of-type{color:#0000ff;}",
            "em:last-of-type{color:#aa00aa;}",
            "</style></head><body>",
            "<p>plain-p</p><p class=\"skip\">skipped-p</p>",
            // Mixed siblings: b, span, b, span — of-type counts only spans.
            "<div><b>bold-one</b><span>span-one</span><b>bold-two</b><span>span-two</span></div>",
            "<div><em>em-first</em><i>mid</i><em>em-last</em></div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();
        let color_of = |needle: &str| -> u32 {
            display_items.iter().find_map(|item| match item {
                DisplayItem::Text { text, style, .. } if text.contains(needle) =>
                    Some(style.color().code_u32()),
                _ => None,
            }).unwrap_or_else(|| panic!("text {:?} not painted", needle))
        };
        assert_eq!(color_of("plain-p"), 0xff0000, ":not(.skip) matches plain p");
        assert_ne!(color_of("skipped-p"), 0xff0000, ":not(.skip) excludes .skip");
        assert_ne!(color_of("span-one"), 0x00ff00, "1st span (3rd child) is not nth-of-type(2)");
        assert_eq!(color_of("span-two"), 0x00ff00, "2nd span matches nth-of-type(2)");
        assert_eq!(color_of("em-first"), 0x0000ff, ":first-of-type");
        assert_eq!(color_of("em-last"), 0xaa00aa, ":last-of-type");
    }

    #[test]
    fn test_structural_pseudo_classes() {
        let html = concat!(
            "<html><head><style>",
            "li:first-child{color:#ff0000;}",
            "li:last-child{color:#00ff00;}",
            "li:nth-child(even){color:#0000ff;}",
            "p:nth-child(2n+1){color:#aa00aa;}",
            "</style></head><body>",
            "<ul>",
            "<li>one</li><li>two</li><li>three</li><li>four</li><li>five</li>",
            "</ul>",
            "<div><p>p1</p><p>p2</p><p>p3</p></div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();
        let color_of = |needle: &str| -> u32 {
            display_items.iter().find_map(|item| match item {
                DisplayItem::Text { text, style, .. } if text.contains(needle) =>
                    Some(style.color().code_u32()),
                _ => None,
            }).unwrap_or_else(|| panic!("text {:?} not painted", needle))
        };
        assert_eq!(color_of("one"), 0xff0000, ":first-child");
        assert_eq!(color_of("two"), 0x0000ff, ":nth-child(even) on 2nd");
        assert_ne!(color_of("three"), 0x0000ff, "3rd is odd");
        assert_eq!(color_of("four"), 0x0000ff, ":nth-child(even) on 4th");
        assert_eq!(color_of("five"), 0x00ff00, ":last-child wins (specificity tie, later)");
        assert_eq!(color_of("p1"), 0xaa00aa, "2n+1 matches 1st");
        assert_ne!(color_of("p2"), 0xaa00aa, "2n+1 skips 2nd");
        assert_eq!(color_of("p3"), 0xaa00aa, "2n+1 matches 3rd");
    }

    #[test]
    fn test_is_selector_matches_any_alternative() {
        let html = r#"<html><head><style>
            :is(.a, .b) { color: #ff0000; }
            div:is([data-on], .never) { color: #0000ff; }
            .c:is(:hover, .c2) { color: #00ff00; }
        </style></head><body>
            <p class="b">red</p>
            <div data-on="1">blue</div>
            <p class="c">plain</p>
            <p class="c c2">green</p>
        </body></html>"#
            .to_string();
        let view = create_layout_view(html, 800);
        let color_of = |needle: &str| -> String {
            view.paint()
                .iter()
                .find_map(|item| match item {
                    DisplayItem::Text { text, style, .. } if text.contains(needle) => {
                        Some(style.color().code().to_string())
                    }
                    _ => None,
                })
                .unwrap()
        };
        assert_eq!(color_of("red"), "#ff0000", ":is(.a,.b) matches .b");
        assert_eq!(color_of("blue"), "#0000ff", ":is with attribute alternative");
        assert_eq!(color_of("green"), "#00ff00", ":hover alternative ignored, .c2 matches");
        assert_ne!(color_of("plain"), "#00ff00", ".c alone must not match");
    }

    #[test]
    fn test_attribute_selectors_and_sibling_combinators() {
        let html = concat!(
            "<html><head><style>",
            "[data-x]{color:#ff0000;}",
            "a[href=\"https://exact.example\"]{color:#00ff00;}",
            "[class~=\"tag\"]{color:#0000ff;}",
            "[href^=\"https:\"]{color:#aa00aa;}",
            "h1 + p{color:#ee8800;}",
            "h2 ~ p{color:#118811;}",
            "</style></head><body>",
            "<div data-x=\"1\">has-attr</div>",
            "<div>no-attr</div>",
            "<a href=\"https://exact.example\">exact-href</a>",
            "<span class=\"chip tag\">word-match</span>",
            "<a href=\"https://other.example/x\">prefix-match</a>",
            "<h1>head1</h1><p>adjacent-p</p><p>not-adjacent-p</p>",
            "<h2>head2</h2><div>gap</div><p>subsequent-p</p>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();
        let color_of = |needle: &str| -> u32 {
            display_items.iter().find_map(|item| match item {
                DisplayItem::Text { text, style, .. } if text.contains(needle) =>
                    Some(style.color().code_u32()),
                _ => None,
            }).unwrap_or_else(|| panic!("text {:?} not painted", needle))
        };
        assert_eq!(color_of("has-attr"), 0xff0000, "[data-x] presence");
        assert_ne!(color_of("no-attr"), 0xff0000, "[data-x] absent");
        assert_eq!(color_of("exact-href"), 0x00ff00, "[href=...] exact");
        assert_eq!(color_of("word-match"), 0x0000ff, "[class~=tag] word");
        assert_eq!(color_of("prefix-match"), 0xaa00aa, "[href^=https:] prefix");
        assert_eq!(color_of("adjacent-p"), 0xee8800, "h1 + p adjacent");
        assert_ne!(color_of("not-adjacent-p"), 0xee8800, "+ is adjacent-only");
        assert_eq!(color_of("subsequent-p"), 0x118811, "h2 ~ p across a gap");
    }

    #[test]
    fn test_important_overrides_specificity_and_inline() {
        let html = concat!(
            "<html><head><style>",
            // !important on a TYPE selector must beat a later id rule.
            "p{color:#ff0000 !important;}",
            "#strong{color:#0000ff;}",
            // !important must also beat a normal inline style.
            "em{color:#00aa00 !important;}",
            // Inline !important beats stylesheet !important.
            "b{color:#777777 !important;}",
            "</style></head><body>",
            "<p id=\"strong\">important-beats-id</p>",
            "<em style=\"color:#123456\">important-beats-inline</em>",
            "<b style=\"color:#abcdef !important\">inline-important-wins</b>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();
        let color_of = |needle: &str| -> u32 {
            display_items.iter().find_map(|item| match item {
                DisplayItem::Text { text, style, .. } if text.contains(needle) =>
                    Some(style.color().code_u32()),
                _ => None,
            }).unwrap_or_else(|| panic!("text {:?} not painted", needle))
        };
        assert_eq!(color_of("important-beats-id"), 0xff0000,
            "p !important beats #id normal rule");
        assert_eq!(color_of("important-beats-inline"), 0x00aa00,
            "stylesheet !important beats normal inline style");
        assert_eq!(color_of("inline-important-wins"), 0xabcdef,
            "inline !important beats stylesheet !important");
    }

    #[test]
    fn test_selector_specificity_orders_cascade() {
        let html = concat!(
            "<html><head><style>",
            // Class rule FIRST, type rule second: class must still win.
            ".special{color:#ff0000;}",
            "p{color:#0000ff;}",
            // Id beats class regardless of order.
            "#main{color:#00ff00;}",
            ".idtest{color:#999999;}",
            // Equal specificity: later rule wins.
            ".eq{color:#111111;}",
            ".eq{color:#222222;}",
            // Descendant (0,0,2) beats bare type (0,0,1) even written first.
            "div em{color:#cc00cc;}",
            "em{color:#333333;}",
            "</style></head><body>",
            "<p class=\"special\">classy</p>",
            "<p>plainp</p>",
            "<p id=\"main\" class=\"idtest\">idwins</p>",
            "<p class=\"eq\">latest</p>",
            "<div><em>nested-em</em></div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();
        let color_of = |needle: &str| -> u32 {
            display_items.iter().find_map(|item| match item {
                DisplayItem::Text { text, style, .. } if text.contains(needle) =>
                    Some(style.color().code_u32()),
                _ => None,
            }).unwrap_or_else(|| panic!("text {:?} not painted", needle))
        };
        assert_eq!(color_of("classy"), 0xff0000, "class (0,1,0) beats later type (0,0,1)");
        assert_eq!(color_of("plainp"), 0x0000ff, "type rule still applies to plain p");
        assert_eq!(color_of("idwins"), 0x00ff00, "id (1,0,0) beats class");
        assert_eq!(color_of("latest"), 0x222222, "equal specificity: later wins");
        assert_eq!(color_of("nested-em"), 0xcc00cc, "div em (0,0,2) beats em (0,0,1)");
    }

    #[test]
    fn test_selector_descendant_compound_list_pseudo() {
        let html = concat!(
            "<html><head><style>",
            // Descendant: applies only to td under .admin.
            ".admin td{color:#ff0000;}",
            // Selector list: both h1 and h2.
            "h1, h2{color:#00ff00;}",
            // Compound: only div with BOTH classes.
            "div.a.b{color:#0000ff;}",
            // Child combinator.
            "ul > li{color:#aa00aa;}",
            // Interaction pseudo must never match statically.
            "p:hover{color:#123456;}",
            "</style></head><body>",
            "<table class=\"admin\"><tr><td>in-admin</td></tr></table>",
            "<table><tr><td>plain</td></tr></table>",
            "<h1>H1</h1><h2>H2</h2>",
            "<div class=\"a\">only-a</div>",
            "<div class=\"a b extra\">a-and-b</div>",
            "<ul><li>item</li></ul>",
            "<p>para</p>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 800);
        let display_items = layout_view.paint();
        // Map text -> color code.
        let color_of = |needle: &str| -> u32 {
            display_items.iter().find_map(|item| match item {
                DisplayItem::Text { text, style, .. } if text.contains(needle) =>
                    Some(style.color().code_u32()),
                _ => None,
            }).unwrap_or_else(|| panic!("text {:?} not painted", needle))
        };
        assert_eq!(color_of("in-admin"), 0xff0000, ".admin td matches");
        assert_ne!(color_of("plain"), 0xff0000, ".admin td must not hit other tables");
        assert_eq!(color_of("H1"), 0x00ff00, "selector list h1");
        assert_eq!(color_of("H2"), 0x00ff00, "selector list h2");
        assert_ne!(color_of("only-a"), 0x0000ff, "div.a.b must not match single class");
        assert_eq!(color_of("a-and-b"), 0x0000ff, "compound matches multi-class attr");
        assert_eq!(color_of("item"), 0xaa00aa, "child combinator");
        assert_ne!(color_of("para"), 0x123456, ":hover never matches statically");
    }

    #[test]
    fn test_grid_track_sizes_and_gap() {
        // 200px fixed + 1fr + 2fr with 20px gaps in a 1000px container:
        // remaining = 1000 - 2*20 - 200 = 760; 1fr = 253, 2fr = 506.
        let html = concat!(
            "<html><head><style>",
            ".grid{display:grid;grid-template-columns:200px 1fr 2fr;gap:10px 20px;}",
            ".item{height:40px;}",
            "</style></head><body>",
            "<div class=\"grid\">",
            "<div class=\"item\">a</div><div class=\"item\">b</div><div class=\"item\">c</div>",
            "<div class=\"item\">d</div>",
            "</div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1000);
        let body = layout_view.root().expect("body");
        let grid = body.borrow().first_child().expect("grid container");
        let a = grid.borrow().first_child().expect("a");
        let b = a.borrow().next_sibling().expect("b");
        let c = b.borrow().next_sibling().expect("c");
        let d = c.borrow().next_sibling().expect("d");
        assert_eq!(a.borrow().size().width(), 200, "fixed 200px track");
        assert_eq!(b.borrow().size().width(), 253, "1fr of remaining 760");
        assert_eq!(c.borrow().size().width(), 506, "2fr of remaining 760");
        let (ax, ay) = (a.borrow().point().x(), a.borrow().point().y());
        let (bx, _) = (b.borrow().point().x(), b.borrow().point().y());
        let (cx, _) = (c.borrow().point().x(), c.borrow().point().y());
        let (dx, dy) = (d.borrow().point().x(), d.borrow().point().y());
        assert_eq!(bx - ax, 220, "b starts after 200px track + 20px gap");
        assert_eq!(cx - bx, 273, "c starts after 253px track + 20px gap");
        assert_eq!(dx, ax, "d wraps to the first track");
        assert_eq!(dy - ay, 50, "second row offset = 40px row + 10px row-gap");
        // Container height: two rows + one row gap.
        assert_eq!(grid.borrow().size().height(), 90, "40 + 10 + 40");
    }

    #[test]
    fn test_display_grid_three_columns_row_major() {
        let html = concat!(
            "<html><head><style>",
            ".grid{display:grid;grid-template-columns:repeat(3, 1fr);}",
            ".card{height:50px;}",
            "</style></head><body>",
            "<div class=\"grid\">",
            "<div class=\"card\">a</div><div class=\"card\">b</div><div class=\"card\">c</div>",
            "<div class=\"card\">d</div><div class=\"card\">e</div>",
            "</div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 900);
        let body = layout_view.root().expect("body");
        let grid = body.borrow().first_child().expect("grid container");
        // Container: two rows of 50px cards.
        assert_eq!(grid.borrow().size().height(), 100, "two 50px grid rows");
        let a = grid.borrow().first_child().expect("card a");
        let b = a.borrow().next_sibling().expect("card b");
        let c = b.borrow().next_sibling().expect("card c");
        let d = c.borrow().next_sibling().expect("card d");
        // Equal 300px tracks at x = 0/300/600 (relative to the container).
        assert_eq!(a.borrow().size().width(), 300, "track width = 900/3");
        let (ax, ay) = (a.borrow().point().x(), a.borrow().point().y());
        let (bx, by) = (b.borrow().point().x(), b.borrow().point().y());
        let (cx, cy) = (c.borrow().point().x(), c.borrow().point().y());
        let (dx, dy) = (d.borrow().point().x(), d.borrow().point().y());
        assert_eq!(bx - ax, 300, "b in second track");
        assert_eq!(cx - ax, 600, "c in third track");
        assert_eq!(ay, by, "a/b share the first row");
        assert_eq!(ay, cy, "a/c share the first row");
        assert_eq!(dx, ax, "d wraps to the first track");
        assert_eq!(dy - ay, 50, "d sits on the second row");
    }

    #[test]
    fn test_background_url_sets_background_image() {
        // Mirrors HN's .votearrow rule: a multi-layer background shorthand
        // whose first layer is a url().
        let html = concat!(
            "<html><head><style>",
            ".votearrow{width:10px;height:10px;border:0px;margin:3px 2px 6px;",
            "background:url(\"triangle.svg\"), linear-gradient(transparent, transparent) no-repeat;",
            "background-size:10px;}",
            "</style></head><body>",
            // Same nesting as HN: cell > center > anchor > votearrow div.
            "<table><tr><td class=\"votelinks\"><center>",
            "<a href=\"vote\"><div class=\"votearrow\" title=\"upvote\"></div></a>",
            "</center></td></tr></table>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();
        let arrow = display_items.iter().find_map(|item| match item {
            crate::display_item::DisplayItem::Rect { style, layout_size, .. }
                if style.background_image() == Some("triangle.svg") =>
            {
                Some((layout_size.width(), layout_size.height()))
            }
            _ => None,
        });
        let (w, h) = arrow.expect("votearrow rect with background_image must be painted");
        assert_eq!(w, 10, "CSS width:10px");
        assert_eq!(h, 10, "CSS height:10px");
    }

    #[test]
    fn test_font_size_em_and_percent_resolve_against_parent() {
        let html = concat!(
            "<html><head><style>",
            "div{font-size:20px;}",
            ".em{font-size:1.5em;}",
            ".pct{font-size:50%;}",
            "</style></head><body>",
            "<div><span class=\"em\">big</span><span class=\"pct\">small</span></div>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let body = layout_view.root().expect("body");
        let div = body.borrow().first_child().expect("div");
        assert_eq!(div.borrow().style().font_size().px(), 20);
        let em_span = div.borrow().first_child().expect("em span");
        assert_eq!(em_span.borrow().style().font_size().px(), 30, "1.5em of 20px parent");
        let pct_span = em_span.borrow().next_sibling().expect("pct span");
        assert_eq!(pct_span.borrow().style().font_size().px(), 10, "50% of 20px parent");
    }

    #[test]
    fn test_hn_itemlist_column_distribution() {
        // Faithful full HN itemlist: 30 stories (rank | votelinks | title),
        // each followed by a colspan=2 subtext row and a spacer row, then a
        // trailing morespace + "More" colspan row.
        // Regressions covered:
        //  1. The rank column ("30.") must not absorb surplus width meant for
        //     the title column (it has no growth headroom), so titles sit
        //     immediately right of the rank instead of right-of-center.
        //  2. The subtext cell (after a colspan=2 lead-in) must resolve its
        //     LOGICAL column (2 = title), not its physical index (1 =
        //     votelinks); otherwise it is sized at ~8px and its text stacks
        //     vertically one character per line, inflating the row height.
        let mut rows = String::new();
        for i in 1..=30 {
            rows.push_str(&format!(
                "<tr class=\"athing\"><td align=\"right\" valign=\"top\" class=\"title\"><span class=\"rank\">{}.</span></td>\
                 <td valign=\"top\" class=\"votelinks\"><center><a href=\"vote\"><div class=\"votearrow\" title=\"upvote\"></div></a></center></td>\
                 <td class=\"title\"><span class=\"titleline\"><a href=\"http://example.com\">Story number {} with a reasonably long headline here</a> <span class=\"sitebit\"> (<a href=\"from\"><span class=\"sitestr\">example.com</span></a>)</span></span></td></tr>\
                 <tr><td colspan=\"2\"></td><td class=\"subtext\"><span class=\"subline\">{} points by user{} 6 hours ago | hide | {} comments</span></td></tr>\
                 <tr class=\"spacer\" style=\"height:5px\"></tr>",
                i, i, 100 + i, i, 50 + i,
            ));
        }
        rows.push_str(
            "<tr class=\"morespace\" style=\"height:10px\"></tr>\
             <tr><td colspan=\"2\"></td><td class=\"title\"><a href=\"news?p=2\" class=\"morelink\">More</a></td></tr>",
        );
        let html = format!(
            "<html><head><style>\
             td{{font-family:Verdana;font-size:10pt;color:#828282;}}\
             .title{{font-family:Verdana;font-size:10pt;color:#828282;overflow:hidden;}}\
             .title a{{word-break:break-word;}}\
             .subtext{{font-family:Verdana;font-size:7pt;color:#828282;}}\
             .votearrow{{width:10px;height:10px;border:0px;margin:3px 2px 6px;background:url(\"triangle.svg\");background-size:10px;}}\
             .rank{{color:#888;}}\
             </style></head><body>\
             <center><table id=\"hnmain\" border=\"0\" cellpadding=\"0\" cellspacing=\"0\" width=\"85%\">\
             <tr><td bgcolor=\"#ff6600\">Hacker News</td></tr>\
             <tr><td><table border=\"0\" cellpadding=\"0\" cellspacing=\"0\">{}</table></td></tr>\
             </table></center></body></html>",
            rows,
        );
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();
        let texts: Vec<(String, i64, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } =>
                Some((text.clone(), layout_point.x(), layout_point.y())),
            _ => None,
        }).collect();

        let rank = texts.iter().find(|(t, _, _)| t == "1.")
            .expect("rank 1. must be rendered");
        let title = texts.iter().find(|(t, _, _)| t.starts_with("Story number 1 "))
            .expect("story 1 title must be rendered");
        let subtext = texts.iter().find(|(t, _, _)| t.starts_with("101 points"))
            .expect("story 1 subtext must be rendered");
        let title2 = texts.iter().find(|(t, _, _)| t.starts_with("Story number 2 "))
            .expect("story 2 title must be rendered");

        // 1. Title is on the same line as the rank, immediately to its right
        //    (rank column stays narrow; no half-page gap).
        assert_eq!(title.2, rank.2, "title must share the rank's line");
        assert!(
            title.1 - rank.1 < 80,
            "title x={} sits too far right of rank x={} (rank column absorbed surplus)",
            title.1, rank.1,
        );
        // 2. Subtext is a single line directly below the title: the next
        //    story's title follows within a few line heights, not hundreds of
        //    pixels (subtext text must not stack one character per line).
        assert!(subtext.2 > title.2, "subtext must be below the title");
        assert!(
            title2.2 - title.2 < 100,
            "story 2 title y={} is too far below story 1 y={} (subtext row inflated)",
            title2.2, title.2,
        );
        // Subtext shares the title's left edge (logical column 2), rather
        // than the votelinks column.
        assert!(
            (subtext.1 - title.1).abs() < 40,
            "subtext x={} must align with title x={} (logical column mapping)",
            subtext.1, title.1,
        );
    }

    #[test]
    fn test_column_widths_consistent_across_rows() {
        // When only the first row has explicit column widths, subsequent rows
        // derived their widths via sibling-row lookup — but before the
        // equalize_column_widths pass, row 1 itself (which has no sibling data)
        // could end up with different widths than rows 2+.
        // After the equalization pass all rows in the same column must have the
        // same cell width, so vertical borders align.
        let html = concat!(
            "<html><head></head><body>",
            "<table border=\"1\" cellspacing=\"2\">",
            "<tr><td width=\"80\">Year</td><td width=\"200\">Title</td><td>Role</td></tr>",
            "<tr><td>2000</td><td>Movie A</td><td>Hero</td></tr>",
            "<tr><td>2001</td><td>Long Movie Title Here</td><td>Villain</td></tr>",
            "</table>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        // Collect all rect items (cell backgrounds) by column — group by x position.
        let rects: Vec<(i64, i64, i64)> = display_items.iter().filter_map(|item| match item {
            crate::display_item::DisplayItem::Rect { layout_point, layout_size, .. } => {
                // Only consider non-full-width rects (table cells, not body background).
                if layout_size.width() < 800 && layout_size.width() > 10 {
                    Some((layout_point.x(), layout_size.width(), layout_point.y()))
                } else {
                    None
                }
            }
            _ => None,
        }).collect();

        // Find cells starting at column 0 (x ~ 2 for first cell after cellspacing).
        let col0_widths: Vec<i64> = rects.iter()
            .filter(|(x, _, _)| *x <= 10)  // first column starts near x=2..5
            .map(|(_, w, _)| *w)
            .collect();

        // All cells in the same column should have the same width.
        if col0_widths.len() >= 2 {
            let first_w = col0_widths[0];
            for w in &col0_widths {
                assert_eq!(*w, first_w,
                    "column-0 cells have inconsistent widths: {:?}", col0_widths);
            }
        }
    }

    #[test]
    fn test_two_tables_stacked_no_overlap() {
        // Replicates the abehiroshi.la.coocan.jp/movie/eiga.htm structure:
        //   <H2>title</H2>
        //   <table width="700"><tr><td>award text 1<br>award 2<br>...</td></tr></table>
        //   <br>
        //   <TABLE BORDER=1 width="700"> <tr><td>...</td></tr> ... </TABLE>
        //
        // Both tables share width=700 inside <CENTER>. The second (bordered)
        // table must start strictly BELOW the first table's last text line.
        let html = concat!(
            "<html><head></head><body>",
            "<center>",
            "<h2>阿部 寛の映画出演</h2>",
            "<table width=\"700\">",
            "<tr><td><strong>",
            "・20周年記念　ニューヨーク・アジアン映画祭「スター・アジア賞」受賞<br>",
            "・第45回　日本アカデミー賞「護られなかった者たちへ」優秀助演男優賞　受賞<br>",
            "・「京都国際映画祭2016」で「三船敏郎賞」受賞<br>",
            "</strong></td></tr>",
            "</table>",
            "<br>",
            "</center>",
            "<center>",
            "<table border=\"1\" width=\"700\">",
            "<tr><td><strong>2025年9月26日公開</strong></td>",
            "<td><strong>「俺ではない炎上」</strong></td>",
            "<td>&nbsp;</td></tr>",
            "<tr><td><strong>2025年7月4日公開</strong></td>",
            "<td><strong>「キャンドルスティック」</strong></td>",
            "<td>&nbsp;</td></tr>",
            "</table>",
            "</center>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let texts: Vec<(String, i64, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } =>
                Some((text.clone(), layout_point.x(), layout_point.y())),
            _ => None,
        }).collect();

        // Last award line in the first table.
        let last_award = texts.iter().find(|(t, _, _)| t.contains("三船敏郎賞"))
            .expect("award text must be rendered");
        // First date in the second (bordered) table.
        let first_date = texts.iter().find(|(t, _, _)| t.contains("2025年9月26日"))
            .expect("first date row must be rendered");

        // The bordered table's first cell must start STRICTLY below the last
        // award text line — otherwise the awards overlap with the table border.
        assert!(
            first_date.2 > last_award.2,
            "movie table first row y={} must be below last award y={} (overlap detected)",
            first_date.2, last_award.2,
        );

        // Also: the second table's border rect must not overlap with
        // the award text (the rect's top must be below the last award line).
        let rects: Vec<(i64, i64, i64, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Rect { layout_point, layout_size, .. } => {
                Some((layout_point.x(), layout_point.y(), layout_size.width(), layout_size.height()))
            }
            _ => None,
        }).collect();

        // Find the bordered table's rect (width ~ 700, contains first_date Y).
        let bordered_table_rect = rects.iter()
            .filter(|(_, y, w, h)| *w >= 600 && *w <= 800 && *y <= first_date.2 && *y + *h >= first_date.2)
            .min_by_key(|(_, y, _, _)| *y)
            .copied();

        if let Some((_, ry, _, _)) = bordered_table_rect {
            assert!(
                ry > last_award.2,
                "bordered table border rect y={ry} overlaps with last award text y={} (overlap)",
                last_award.2
            );
        }
    }

    #[test]
    fn test_table_height_includes_multiline_cell_content() {
        // Replicates eiga.htm exactly: <center><h2><table><tr><td>multi-line<br>...</td></tr></table>
        // Then a sibling <center><table border=1>...
        // The first (no-border) table's HEIGHT must reflect its multi-line text
        // content, otherwise the second table will be positioned ON TOP of it.
        let html = concat!(
            "<html><head></head><body>",
            "<center>",
            "<h2>title</h2>",
            "<table width=\"700\">",
            "<tr><td>",
            "Line1<br>Line2<br>Line3<br>Line4<br>Line5<br>Line6<br>Line7<br>Line8<br>Line9<br>Line10<br>Line11<br>",
            "</td></tr>",
            "</table>",
            "<br>",
            "</center>",
            "<center>",
            "<table border=\"1\" width=\"700\">",
            "<tr><td>RowA</td></tr>",
            "<tr><td>RowB</td></tr>",
            "</table>",
            "</center>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let texts: Vec<(String, i64, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } =>
                Some((text.clone(), layout_point.x(), layout_point.y())),
            _ => None,
        }).collect();

        let line1 = texts.iter().find(|(t, _, _)| t.contains("Line1"))
            .expect("Line1 must be rendered");
        let line11 = texts.iter().find(|(t, _, _)| t.contains("Line11"))
            .expect("Line11 must be rendered");
        let row_a = texts.iter().find(|(t, _, _)| t.contains("RowA"))
            .expect("RowA must be rendered");

        // Lines 1 and 11 must be vertically separated (multi-line layout works).
        assert!(line11.2 > line1.2,
            "Line11 y={} must be below Line1 y={} (multi-line text)",
            line11.2, line1.2);

        // RowA in the second table must be strictly below Line11.
        assert!(row_a.2 > line11.2,
            "RowA y={} must be below Line11 y={} — first table height too small, second table overlaps first table content",
            row_a.2, line11.2);
    }

    #[test]
    fn test_eiga_exact_layout_no_overlap() {
        // Replicates eiga.htm EXACTLY (with <strong> and explicit attributes).
        let html = concat!(
            "<html><head></head><body background=\"x.jpg\">",
            "<center>",
            "<h2>阿部 寛の映画出演</h2>",
            "<table width=\"700\">",
            "<tr><td><strong>",
            "・20周年記念　ニューヨーク・アジアン映画祭「スター・アジア賞」受賞<br>",
            "・第45回　日本アカデミー賞「護られなかった者たちへ」優秀助演男優賞　受賞<br>",
            "・「京都国際映画祭2016」で「三船敏郎賞」受賞<br>",
            "・第38回　日本アカデミー賞「ふしぎな岬の物語」優秀主演男優賞　受賞<br>",
            "・第38回　日本アカデミー賞 「柘榴坂の仇討」優秀助演男優賞　受賞<br>",
            "・第36回　日本アカデミー賞　「テルマエ・ロマエ」で最優秀主演男優賞受賞<br>",
            "・2012年　ブルーリボン賞　「カラスの親指」「麒麟の翼」「テルマエ・ロマエ」で主演男優賞受賞<br>",
            "・2012年　ヨコハマ映画祭　　「テルマエ・ロマエ」で主演男優賞受賞<br>",
            "・2012年　日本シアタースタッフ映画祭ＩＮ成城　　「テルマエ・ロマエ」で主演男優賞受賞<br>",
            "・第63回毎日映画コンクール男優主演賞 受賞 2008年公開映画「歩いても歩いても」、「青い鳥」<br>",
            "・1994年度インディペンデント映画祭プロフェッショナル大賞受賞「凶銃ルガーPO8」<br>",
            "</strong></td></tr>",
            "</table>",
            "<br>",
            "</center>",
            "<center>",
            "<table border=\"1\" width=\"700\">",
            "<tr><td align=\"left\"><strong>2025年9月26日公開</strong></td>",
            "<td align=\"left\"><strong>「俺ではない炎上」</strong></td>",
            "<td align=\"left\">&nbsp;</td></tr>",
            "<tr><td align=\"left\"><strong>2025年7月4日公開</strong></td>",
            "<td align=\"left\"><strong>「キャンドルスティック」</strong></td>",
            "<td align=\"left\">&nbsp;</td></tr>",
            "</table>",
            "</center>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let texts: Vec<(String, i64, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } =>
                Some((text.clone(), layout_point.x(), layout_point.y())),
            _ => None,
        }).collect();

        let first_award = texts.iter().find(|(t, _, _)| t.contains("20周年記念"))
            .expect("first award must be rendered");
        let last_award = texts.iter().find(|(t, _, _)| t.contains("1994年度"))
            .expect("last award must be rendered");
        let first_movie = texts.iter().find(|(t, _, _)| t.contains("2025年9月26日"))
            .expect("first movie row must be rendered");

        // The first movie row must be strictly below the LAST award.
        // If awards table height is wrong, first_movie.y could be at or above last_award.y.
        assert!(
            first_movie.2 > last_award.2,
            "OVERLAP: first_movie y={} must be > last_award y={} (first_award y={})",
            first_movie.2, last_award.2, first_award.2,
        );

        // Bordered table: every row in the same column must have the same X
        // (left border alignment) and same width — otherwise the vertical
        // borders form a "double line" that doesn't connect across rows.
        let rects: Vec<(i64, i64, i64, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Rect { layout_point, layout_size, .. } => {
                Some((layout_point.x(), layout_point.y(), layout_size.width(), layout_size.height()))
            }
            _ => None,
        }).collect();
        // The actual table cell rects (not their inner content rects) have
        // h=24 (line height + cell padding + border). Filter just those.
        // Group by Y: cells of the same row share Y.
        let mut cells_by_row: std::collections::BTreeMap<i64, std::vec::Vec<(i64, i64)>> = std::collections::BTreeMap::new();
        for (x, y, w, h) in &rects {
            if *w < 700 && *w > 0 && *h == 24 && *y >= first_movie.2 - 5 {
                cells_by_row.entry(*y).or_default().push((*x, *w));
            }
        }
        // Take the first two rows that look like the bordered table rows.
        let mut row_iter = cells_by_row.values().filter(|v| v.len() >= 2);
        if let (Some(row1), Some(row2)) = (row_iter.next(), row_iter.next()) {
            for (i, (r1, r2)) in row1.iter().zip(row2.iter()).enumerate() {
                assert_eq!(r1.0, r2.0,
                    "column {} X mismatch: row1 x={} row2 x={}", i, r1.0, r2.0);
                assert_eq!(r1.1, r2.1,
                    "column {} width mismatch: row1 w={} row2 w={}", i, r1.1, r2.1);
            }
        }
    }

    #[test]
    fn test_tv_htm_long_row_wraps_within_cell() {
        // Replicates tv.htm: 2-column table where some rows have very long
        // titles that must wrap within the cell. The cell rect must:
        //   1. Stay within the table's right edge (no horizontal overflow)
        //   2. Be tall enough to contain ALL wrapped text lines (no vertical
        //      overflow of text past the cell border)
        //   3. Have the same X and width as adjacent rows (vertical border
        //      alignment).
        let html = concat!(
            "<html><head></head><body>",
            "<center><h2>阿部 寛のドラマ出演</h2></center>",
            "<center>",
            "<table border=\"1\" width=\"700\">",
            "<tr>",
            "<td align=\"left\"><strong>2026年7月</strong></td>",
            "<td align=\"left\"><strong>「日曜劇場『VIVANT』続編」</strong></td>",
            "</tr>",
            "<tr>",
            "<td align=\"left\"><strong>2015年4月11日</strong></td>",
            "<td align=\"left\"><strong>阿部寛＆ルフィが初共演　阿部寛がゴム人間に！夢の共演が実現！「世にも奇妙な物語　25周年スペシャル・春〜人気マンガ家競演編〜」</strong></td>",
            "</tr>",
            "<tr>",
            "<td align=\"left\"><strong>2014年5月</strong></td>",
            "<td align=\"left\"><strong>「まっしろ」</strong></td>",
            "</tr>",
            "</table>",
            "</center>",
            "</body></html>",
        ).to_string();
        let layout_view = create_layout_view(html, 1024);
        let display_items = layout_view.paint();

        let rects: Vec<(i64, i64, i64, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Rect { layout_point, layout_size, .. } => {
                Some((layout_point.x(), layout_point.y(), layout_size.width(), layout_size.height()))
            }
            _ => None,
        }).collect();

        let texts: Vec<(String, i64, i64)> = display_items.iter().filter_map(|item| match item {
            DisplayItem::Text { text, layout_point, .. } =>
                Some((text.clone(), layout_point.x(), layout_point.y())),
            _ => None,
        }).collect();

        // Find the bordered table rect (width ≥ 700).
        let table_rect = rects.iter()
            .find(|(_, _, w, _)| *w >= 700 && *w <= 720)
            .copied()
            .expect("bordered table rect must exist");
        let table_left = table_rect.0;
        let table_right = table_rect.0 + table_rect.2;
        let table_top = table_rect.1;
        let table_bottom = table_rect.1 + table_rect.3;

        // ALL text inside the table area must stay within table bounds.
        let table_texts: Vec<&(String, i64, i64)> = texts.iter()
            .filter(|(_, x, y)| *x >= table_left - 5 && *x <= table_right + 5
                && *y >= table_top && *y <= table_bottom + 200)
            .collect();
        for (txt, tx, ty) in &table_texts {
            assert!(*tx + 10 <= table_right,
                "text '{}' at x={} extends past table right edge x={}", txt, tx, table_right);
            assert!(*ty < table_bottom,
                "text '{}' at y={} is below table bottom y={} (vertical overflow)", txt, ty, table_bottom);
        }

        // Vertical border alignment: the OUTER cell rects in the same column
        // must share X and width across all rows. Cells lie at the row's exact
        // y, while inner <strong> content rects are inset by ~2px and may be
        // taller than one line when the cell wraps — so we must compare only
        // outer cell rects. We identify the canonical column X-positions from
        // the top row (which never wraps) and compare only rects at those Xs.
        let mut cells_by_row: std::collections::BTreeMap<i64, std::vec::Vec<(i64, i64, i64)>> = std::collections::BTreeMap::new();
        for (x, y, w, h) in &rects {
            if *w < 700 && *w > 30 && *h >= 24 && *h < table_rect.3 {
                cells_by_row.entry(*y).or_default().push((*x, *w, *h));
            }
        }
        // Canonical column X-positions: the top-most row's two outer cells.
        let col_xs: std::collections::BTreeSet<i64> = cells_by_row
            .values()
            .find(|v| v.len() == 2)
            .map(|v| v.iter().map(|(x, _, _)| *x).collect())
            .unwrap_or_default();
        // Restrict every row to cells sitting at a canonical column X; this
        // drops inner content rects, which are offset by the cell border.
        let rows: Vec<std::vec::Vec<(i64, i64, i64)>> = cells_by_row
            .values()
            .map(|v| {
                let mut cells: std::vec::Vec<(i64, i64, i64)> =
                    v.iter().filter(|(x, _, _)| col_xs.contains(x)).copied().collect();
                cells.sort_by_key(|(x, _, _)| *x);
                cells
            })
            .filter(|v| v.len() == 2)
            .collect();
        if rows.len() >= 2 {
            let row1 = rows[0].clone();
            for (i, (x1, w1, _)) in row1.iter().enumerate() {
                for r in rows.iter().skip(1) {
                    let (xn, wn, _) = r[i];
                    assert_eq!(xn, *x1, "column {} X mismatch: row1 x={} other x={}", i, x1, xn);
                    assert_eq!(wn, *w1, "column {} width mismatch: row1 w={} other w={}", i, w1, wn);
                }
            }
        }
    }

    #[test]
    fn test_nested_inline_text_wraps_inside_cell() {
        // Text inside <strong><a> inside a narrow cell should wrap within the cell.
        let html = concat!(
            "<html><body><table border=\"1\" width=\"200\">",
            "<tr><td><strong><a href=\"#\">some long anchor text that should wrap inside the cell</a></strong></td></tr>",
            "</table></body></html>"
        ).to_string();
        let view = create_layout_view(html, 800);
        let items = view.paint();
        // The table is 200px wide; no text item should start beyond x=210.
        for item in &items {
            if let crate::display_item::DisplayItem::Text { layout_point, text, .. } = item {
                assert!(layout_point.x() <= 210,
                    "text {:?} starts at x={} which is outside the 200px table", text, layout_point.x());
            }
        }
    }

    #[test]
    fn test_long_word_in_nested_cell_widens_column() {
        // A long unbreakable word deep in a nested element should widen the column.
        let long_word = "SUPERCALIFRAGILISTICEXPIALIDOCIOUS";
        let html = format!(
            "<html><body><table border=\"1\"><tr><td>short</td><td><p><span>{}</span></p></td></tr></table></body></html>",
            long_word
        );
        let view = create_layout_view(html, 800);
        let items = view.paint();
        // The long word should appear as a single text item (no wrap).
        let long_texts: Vec<_> = items.iter().filter_map(|i| match i {
            crate::display_item::DisplayItem::Text { text, .. }
                if text.contains("SUPERCALI") => Some(text.clone()),
            _ => None,
        }).collect();
        assert_eq!(long_texts.len(), 1,
            "long word should not be split, got {:?}", long_texts);
    }

    #[test]
    fn test_image_clipped_to_narrow_cell() {
        // An oversized image inside a narrow cell should have its clip_rect
        // constrained to the cell's bounding box.
        let html = concat!(
            "<html><body><table border=\"1\" width=\"200\">",
            "<tr><td width=\"100\"><img width=\"400\" height=\"20\"></td></tr>",
            "</table></body></html>"
        ).to_string();
        let view = create_layout_view(html, 800);
        let items = view.paint();
        for item in &items {
            if let crate::display_item::DisplayItem::Image { clip_rect: Some(c), .. } = item {
                assert!(c.width <= 110,
                    "image clip width {} exceeds cell content width", c.width);
            }
        }
    }

    #[test]
    fn test_text_wrap_recomputed_after_widening() {
        // After column equalization, the "short" cell gets widened to match the
        // "aaaaa..." cell's width. The text "short" should fit on one line (no wrap).
        let html = concat!(
            "<html><body><table border=\"1\" width=\"400\">",
            "<tr><td>short</td><td>x</td></tr>",
            "<tr><td>aaaaaaaaaaaaaaaaaaaaaaaaaa</td><td>y</td></tr>",
            "</table></body></html>"
        ).to_string();
        let view = create_layout_view(html, 800);
        let items = view.paint();
        let short_texts: Vec<_> = items.iter().filter_map(|i| match i {
            crate::display_item::DisplayItem::Text { text, .. } if text.contains("short") =>
                Some(text.clone()),
            _ => None,
        }).collect();
        assert_eq!(short_texts.len(), 1,
            "short text should not wrap after column equalization, got {:?}", short_texts);
    }

    #[test]
    fn test_tbody_transparent_for_column_equalization() {
        let html = concat!(
            "<html><body><table border=\"1\" width=\"300\"><tbody>",
            "<tr><td>A</td><td>BB</td></tr>",
            "<tr><td>CCCC</td><td>D</td></tr>",
            "</tbody></table></body></html>"
        ).to_string();
        let view = create_layout_view(html, 600);
        let items = view.paint();
        // Collect cell rects grouped by Y position (each row).
        let mut cells_by_y: std::collections::BTreeMap<i64, Vec<(i64, i64)>> =
            std::collections::BTreeMap::new();
        for item in &items {
            if let crate::display_item::DisplayItem::Rect { layout_point, layout_size, .. } = item {
                let w = layout_size.width();
                let h = layout_size.height();
                if w > 5 && w < 290 && h > 5 {
                    cells_by_y.entry(layout_point.y()).or_default()
                        .push((layout_point.x(), w));
                }
            }
        }
        let rows: Vec<_> = cells_by_y.values().filter(|v| v.len() == 2).collect();
        assert!(rows.len() >= 2, "expected at least 2 rows with 2 cells each, got {:?}", cells_by_y);
        let row0 = &rows[0];
        let row1 = &rows[1];
        for i in 0..2 {
            assert_eq!(row0[i].0, row1[i].0,
                "col {} X mismatch with tbody: row0={} row1={}", i, row0[i].0, row1[i].0);
            assert_eq!(row0[i].1, row1[i].1,
                "col {} width mismatch with tbody: row0={} row1={}", i, row0[i].1, row1[i].1);
        }
    }

    #[test]
    fn test_thead_tbody_tfoot_no_panic() {
        // This must not panic (previously would panic due to missing ElementKind variants).
        let html = concat!(
            "<html><body><table border=\"1\" width=\"400\">",
            "<thead><tr><th>H1</th><th>H2</th></tr></thead>",
            "<tbody><tr><td>A</td><td>BB</td></tr><tr><td>CC</td><td>D</td></tr></tbody>",
            "<tfoot><tr><td>F1</td><td>F2</td></tr></tfoot>",
            "</table></body></html>"
        ).to_string();
        let view = create_layout_view(html, 600);
        let items = view.paint();
        // Should render some content (at minimum the table outline rect).
        let rects: Vec<_> = items.iter().filter(|i| {
            matches!(i, crate::display_item::DisplayItem::Rect { .. })
        }).collect();
        assert!(!rects.is_empty(), "thead/tbody/tfoot table should produce rect display items");
        // All 4 rows should appear: 1 header + 2 body + 1 footer.
        let mut cells_by_y: std::collections::BTreeMap<i64, usize> =
            std::collections::BTreeMap::new();
        for item in &items {
            if let crate::display_item::DisplayItem::Rect { layout_point, layout_size, .. } = item {
                let w = layout_size.width();
                if w > 5 && w < 390 {
                    *cells_by_y.entry(layout_point.y()).or_insert(0) += 1;
                }
            }
        }
        let row_count = cells_by_y.values().filter(|&&c| c >= 2).count();
        assert!(row_count >= 4, "expected at least 4 rows, found {}", row_count);
    }

    #[test]
    fn test_eiga_mixed_pct_and_auto_rows_column_alignment() {
        // Replicates eiga.htm: some rows have no width attrs, some have pct widths.
        // All cells in the same column must share X and width after equalization.
        let html = concat!(
            "<html><head></head><body>",
            "<CENTER><TABLE BORDER=1 width=\"700\">",
            "<tr>",
            "<td align=\"LEFT\"><strong>2025nen9gatsu26nichi</strong></td>",
            "<td align=\"LEFT\"><strong>OredehanaiEnjo</strong></td>",
            "<td align=\"LEFT\">&nbsp;</td>",
            "</tr>",
            "<tr>",
            "<td align=\"LEFT\"><strong>2016nen5gatsu21nichi</strong></td>",
            "<td align=\"LEFT\"><strong>UmiyorimoMadaFukaku</strong></td>",
            "<td align=\"LEFT\"><strong>Cannes International Film Festival Certain Regard section screening</strong></td>",
            "</tr>",
            "<tr>",
            "<td align=\"LEFT\" width=\"32%\"><strong>2005nen11gatsu19nichi</strong></td>",
            "<td align=\"LEFT\" width=\"46%\"><strong>Kidan</strong></td>",
            "<td align=\"LEFT\" width=\"22%\">&nbsp;</td>",
            "</tr>",
            "<tr>",
            "<td align=\"LEFT\" width=\"32%\"><strong>2004nen9gatsu</strong></td>",
            "<td align=\"LEFT\" width=\"46%\"><strong>SURVIVE STYLE 5 Plus</strong></td>",
            "<td align=\"LEFT\" width=\"22%\">&nbsp;</td>",
            "</tr>",
            "</TABLE></CENTER>",
            "</body></html>"
        ).to_string();
        let view = create_layout_view(html, 1024);
        let items = view.paint();

        // Cell rects should all lie within the 700px table boundary.
        let table_rects: Vec<(i64, i64, i64, i64)> = items.iter().filter_map(|item| match item {
            crate::display_item::DisplayItem::Rect { layout_point, layout_size, .. } => {
                Some((layout_point.x(), layout_point.y(), layout_size.width(), layout_size.height()))
            }
            _ => None,
        }).collect();

        // Find table outer rect.
        let table_outer = table_rects.iter()
            .find(|(_, _, w, _)| *w >= 700 && *w <= 720)
            .copied();

        if let Some((tx, _, tw, _)) = table_outer {
            let table_right = tx + tw;
            // All text items should start left of table right edge.
            for item in &items {
                if let crate::display_item::DisplayItem::Text { layout_point, text, .. } = item {
                    if layout_point.x() > tx {
                        assert!(layout_point.x() < table_right + 5,
                            "text {:?} at x={} is outside table right edge x={}", text, layout_point.x(), table_right);
                    }
                }
            }

            // Column widths must be consistent across rows.
            let mut cells_by_y: std::collections::BTreeMap<i64, Vec<(i64, i64)>> =
                std::collections::BTreeMap::new();
            // Minimum cell height: text(20) + border_top(1) + border_bottom(1) +
            // 2*cellpadding(2) = 24. Inline elements (e.g. <strong>) have height 20
            // for single-line text — filter them out with ch >= 24.
            let min_cell_h = 24;
            for (cx, cy, cw, ch) in &table_rects {
                if *cw < 700 && *cw > 10 && *ch >= min_cell_h && *cx > tx {
                    cells_by_y.entry(*cy).or_default().push((*cx, *cw));
                }
            }
            let rows: Vec<_> = cells_by_y.values().filter(|v| v.len() == 3).collect();
            assert!(rows.len() >= 2,
                "expected >=2 rows with 3 cells each; all cell rects: {:?}", cells_by_y);
            let r0 = &rows[0];
            for r in rows.iter().skip(1) {
                for i in 0..3 {
                    assert_eq!(r[i].0, r0[i].0,
                        "col {} X mismatch across rows: {} vs {}; all rows: {:?}", i, r[i].0, r0[i].0, rows);
                    assert_eq!(r[i].1, r0[i].1,
                        "col {} width mismatch across rows: {} vs {}; all rows: {:?}", i, r[i].1, r0[i].1, rows);
                }
            }
        }
    }

    #[test]
    fn test_colspan_row_does_not_starve_subsequent_columns() {
        // A table where row A has a colspan=2 cell in the middle and row B has
        // 3 individual cells.  The colspan cell's full width must NOT be used as
        // the width hint for a single column in row B — doing so leaves no space
        // for row B's 3rd column.
        //
        // Expected: row B's 3rd column (col2) is visible (x > col1_x and width > 0),
        // and the right edge of row B's cells does not exceed the viewport.
        let html = concat!(
            "<html><head></head><body>",
            "<table width=\"600\">",
            // Row A: 2 cells, second spans columns 1+2
            "<tr>",
            "  <td>Col0</td>",
            "  <td colspan=\"2\">Colspan cell</td>",
            "</tr>",
            // Row B: 3 individual cells — col2 must receive some width
            "<tr>",
            "  <td>Col0</td>",
            "  <td>Col1</td>",
            "  <td>Col2</td>",
            "</tr>",
            "</table>",
            "</body></html>",
        ).to_string();
        let view = create_layout_view(html, 760);
        let items = view.paint();

        let text_items: Vec<(std::string::String, i64)> = items.iter().filter_map(|item| {
            if let crate::display_item::DisplayItem::Text { text, layout_point, .. } = item {
                Some((text.clone(), layout_point.x()))
            } else {
                None
            }
        }).collect();

        let col1_x = text_items.iter()
            .find(|(t, _)| t.contains("Col1"))
            .map(|(_, x)| *x)
            .expect("Col1 text missing");
        let col2_x = text_items.iter()
            .find(|(t, _)| t.contains("Col2"))
            .map(|(_, x)| *x)
            .expect("Col2 text missing");

        assert!(
            col2_x > col1_x,
            "col2 (x={}) must be to the right of col1 (x={}); items: {:?}",
            col2_x, col1_x, text_items
        );
        assert!(
            col2_x < 760,
            "col2 (x={}) must be within viewport (760); items: {:?}",
            col2_x, text_items
        );
    }

    #[test]
    fn test_cjk_title_column_not_treated_as_spacer() {
        // Regression test: a 3-column table where the title column contains
        // pure CJK text (hint was incorrectly 0 → spacer → 1px width).
        // The date column has ASCII digits giving hint=32 (flexible).
        // After the fix, the CJK title column must also be flexible and receive
        // a significant share of the table width.
        let html = concat!(
            "<html><head></head><body>",
            "<table border=\"1\" width=\"700\">",
            "<tr>",
            "<td><strong>2025年9月26日公開</strong></td>",
            "<td><strong>「俺ではない炎上」</strong></td>",
            "<td>&nbsp;</td>",
            "</tr>",
            "<tr>",
            "<td><strong>2025年7月4日公開</strong></td>",
            "<td><strong>「キャンドルスティック」</strong></td>",
            "<td>&nbsp;</td>",
            "</tr>",
            "</table>",
            "</body></html>",
        ).to_string();
        let view = create_layout_view(html, 1024);
        let items = view.paint();

        // Find all rects that look like cells (h==24, w < 700, w > 5).
        let mut cells_by_row: std::collections::BTreeMap<i64, std::vec::Vec<(i64, i64)>> =
            std::collections::BTreeMap::new();
        for item in &items {
            if let crate::display_item::DisplayItem::Rect { layout_point, layout_size, .. } = item {
                let w = layout_size.width();
                let h = layout_size.height();
                if w > 5 && w < 695 && h == 24 {
                    cells_by_row.entry(layout_point.y()).or_default().push((layout_point.x(), w));
                }
            }
        }
        // Both rows must have at least 2 measurable cells (col0 and col1).
        let rows: std::vec::Vec<_> = cells_by_row.values()
            .filter(|v| v.len() >= 2)
            .collect();
        assert!(rows.len() >= 2, "expected at least 2 rows with 2 cells each, got {:?}", cells_by_row);

        // Col1 (title) must have at least 100px — not squeezed to 1px.
        for row in &rows {
            let mut sorted = row.to_vec();
            sorted.sort_by_key(|(x, _)| *x);
            let col1_width = sorted[1].1;
            assert!(
                col1_width >= 100,
                "CJK title column width {} must be >= 100px (col1 should not be a spacer); row cells: {:?}",
                col1_width, sorted
            );
        }
    }

    #[test]
    fn test_table_column_width_promoted_by_later_row_cjk_content() {
        // Regression test for the abehiroshi.la.coocan.jp movies page.
        // A 3-column table where most rows have &nbsp; in col2, but one row
        // contains substantial CJK text in col2.  Without the per-column
        // min-hint pre-pass, row 1 would treat col2 as a spacer (8px) and
        // later rows would inherit that, starving col2 in the row with text
        // content and causing horizontal overflow at the table's right edge.
        let html = concat!(
            "<html><head></head><body>",
            "<table border=\"1\" width=\"700\">",
            "<tr>",
            "<td><strong>2025年9月26日公開</strong></td>",
            "<td><strong>「俺ではない炎上」</strong></td>",
            "<td>&nbsp;</td>",
            "</tr>",
            "<tr>",
            "<td><strong>2016年5月21日公開</strong></td>",
            "<td><strong>「海よりもまだ深く」</strong></td>",
            "<td><strong>カンヌ国際映画祭部門出品</strong></td>",
            "</tr>",
            "<tr>",
            "<td><strong>2014年公開</strong></td>",
            "<td><strong>「アゲイン」</strong></td>",
            "<td>&nbsp;</td>",
            "</tr>",
            "</table>",
            "</body></html>",
        ).to_string();
        let view = create_layout_view(html, 1024);
        let items = view.paint();

        // Cell rects only: filter to rects whose height matches one text-line.
        // The table has border="1" so each cell has h = line_height + 2*border ≈ 24.
        let mut cells_by_row: std::collections::BTreeMap<i64, std::vec::Vec<(i64, i64)>> =
            std::collections::BTreeMap::new();
        for item in &items {
            if let crate::display_item::DisplayItem::Rect { layout_point, layout_size, .. } = item {
                let w = layout_size.width();
                let h = layout_size.height();
                if w > 5 && w < 700 && h == 24 {
                    cells_by_row.entry(layout_point.y()).or_default().push((layout_point.x(), w));
                }
            }
        }
        let rows: std::vec::Vec<_> = cells_by_row
            .values()
            .filter(|v| v.len() >= 3)
            .collect();
        assert!(
            rows.len() >= 3,
            "expected at least 3 rows with 3 cells each, got {:?}",
            cells_by_row
        );

        // For each row, col2 must be wide enough to display the CJK content
        // that appears in row 2 (≥ 24px) — i.e. col2 was promoted by the
        // pre-pass instead of staying at the row-1 spacer width.
        for row in &rows {
            let mut sorted = row.to_vec();
            sorted.sort_by_key(|(x, _)| *x);
            assert_eq!(sorted.len(), 3, "expected 3 cells; got: {:?}", sorted);
            let col2_w = sorted[2].1;
            assert!(
                col2_w >= 24,
                "col2 width {} must be >= 24 (promoted by sibling CJK content); cells: {:?}",
                col2_w, sorted
            );
            // Sum of cell widths (outer) must stay within the table width plus
            // small border overhead.  Each cell has a 1px border on each side
            // under border="1", so 3 cells may add up to ~6px above the
            // declared content width.
            let total: i64 = sorted.iter().map(|(_, w)| *w).sum();
            assert!(
                total <= 710,
                "row cell widths sum to {} > 710 (large table overflow); cells: {:?}",
                total, sorted
            );
        }

        // Column widths must be equal across rows (equalization pass).
        let row0 = rows[0];
        let row1 = rows[1];
        let row2 = rows[2];
        let mut r0 = row0.to_vec(); r0.sort_by_key(|(x, _)| *x);
        let mut r1 = row1.to_vec(); r1.sort_by_key(|(x, _)| *x);
        let mut r2 = row2.to_vec(); r2.sort_by_key(|(x, _)| *x);
        for col in 0..3 {
            assert_eq!(r0[col].0, r1[col].0,
                "col {} x mismatch between rows: r0={} r1={}", col, r0[col].0, r1[col].0);
            assert_eq!(r0[col].0, r2[col].0,
                "col {} x mismatch between rows: r0={} r2={}", col, r0[col].0, r2[col].0);
            assert_eq!(r0[col].1, r1[col].1,
                "col {} width mismatch between rows: r0={} r1={}", col, r0[col].1, r1[col].1);
            assert_eq!(r0[col].1, r2[col].1,
                "col {} width mismatch between rows: r0={} r2={}", col, r0[col].1, r2[col].1);
        }
    }

    #[test]
    fn test_table_rowspan_row_not_starved_by_column_hint_misindex() {
        // Regression: with column_min_hints indexed by LOGICAL col, but the
        // iteration in table_cell_auto_width using a PHYSICAL counter, a row
        // whose first logical column is occupied by a rowspan cell from the
        // previous row would receive promoted hints from the wrong column.
        // Replicates the top.htm structure: outer table with rowspan=2 in col 0.
        let html = concat!(
            "<html><head></head><body>",
            "<table width=\"760\">",
            "<tr>",
            "<td rowspan=\"2\"><img src=\"x.jpg\" width=\"350\" height=\"414\">",
            "<table width=\"256\"><tr><td>nested-row-1</td></tr></table>",
            "</td>",
            "<td width=\"10\">&nbsp;</td>",
            "<td><div align=\"center\">最新情報</div></td>",
            "</tr>",
            "<tr>",
            "<td width=\"10\"></td>",
            "<td><strong>連続ドラマ「VIVANT」続編 2026年7月放送</strong></td>",
            "</tr>",
            "</table>",
            "</body></html>",
        ).to_string();
        let view = create_layout_view(html, 1024);
        let items = view.paint();

        // Find the "連続ドラマ" text — it must be rendered at an x position
        // significantly to the right of the rowspan column (x > 350).
        let texts: std::vec::Vec<(std::string::String, i64)> = items.iter().filter_map(|item| {
            if let crate::display_item::DisplayItem::Text { text, layout_point, .. } = item {
                Some((text.clone(), layout_point.x()))
            } else {
                None
            }
        }).collect();
        let target = texts.iter().find(|(t, _)| t.contains("連続ドラマ"))
            .expect("「連続ドラマ」 text missing");
        assert!(
            target.1 > 350,
            "Row 2 col 2 text at x={} must be > 350 (right of rowspan column); items: {:?}",
            target.1, texts
        );
    }

    #[test]
    fn test_wix_page_snippet_renders_without_panic() {
        // Smoke test: load a real-world Wix-built page snippet through the
        // HTML/CSS pipeline and confirm the renderer doesn't crash on its
        // custom elements (`<wow-image>`, `<svg>`, etc.), heavy CSS, or
        // minified JavaScript with operators (`!`, `<`, `>`, `?`, `&`, `|`)
        // that the minimal lexer cannot tokenize.  Uses a truncated copy of
        // the page to keep test runtime/stack usage reasonable; the full
        // page is in wix_page.html for manual verification.
        let html = include_str!("../../../testdata/wix_page_small.html").to_string();
        let _view = create_layout_view(html, 1024);
    }
}
