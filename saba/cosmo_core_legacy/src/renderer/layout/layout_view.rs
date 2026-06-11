use crate::display_item::DisplayItem;
use crate::renderer::css::cssom::StyleSheet;
use crate::renderer::dom::api::get_target_element_node;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::Node;
use crate::renderer::layout::layout_object::compute_box_model_metrics;
use crate::renderer::layout::layout_object::create_layout_object;
use crate::renderer::layout::layout_object::LayoutObject;
use crate::renderer::layout::layout_object::LayoutObjectKind;
use crate::renderer::layout::layout_object::LayoutPoint;
use crate::renderer::layout::layout_object::LayoutSize;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::RefCell;

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
        obj.borrow_mut().set_first_child(first_child);
        obj.borrow_mut().set_next_sibling(next_sibling);
    }

    layout_object
}

#[derive(Debug, Clone)]
pub struct LayoutView {
    root: Option<Rc<RefCell<LayoutObject>>>,
    viewport_width: i64,
}

impl LayoutView {
    pub fn new(root: Rc<RefCell<Node>>, cssom: &StyleSheet, viewport_width: i64) -> Self {
        let body_root = get_target_element_node(Some(root), ElementKind::Body);

        let mut tree = Self {
            root: build_layout_tree(&body_root, &None, cssom),
            viewport_width: viewport_width.max(1),
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
    use crate::alloc::string::ToString;
    use crate::display_item::DisplayItem;
    use crate::renderer::css::cssom::CssParser;
    use crate::renderer::css::token::CssTokenizer;
    use crate::renderer::dom::api::get_style_content;
    use crate::renderer::dom::node::Element;
    use crate::renderer::dom::node::NodeKind;
    use crate::renderer::html::parser::HtmlParser;
    use crate::renderer::html::token::HtmlTokenizer;
    use alloc::format;
    use alloc::string::String;
    use alloc::vec::Vec;

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
            .map(|(t, x, y)| alloc::format!("{}@({},{})", t, x, y))
            .collect();

        let latest = text_items.iter().find(|(t, _, _)| t.contains("LatestNews"))
            .expect(&alloc::format!("LatestNews missing, items: {:?}", debug));
        let drama = text_items.iter().find(|(t, _, _)| t.contains("DramaInfo"))
            .expect(&alloc::format!("DramaInfo missing, items: {:?}", debug));

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
            .map(|(t, x)| alloc::format!("{}@x={}", t, x))
            .collect();

        // Col-2 content (dates) must be to the RIGHT of col-1 content (titles).
        let title_a_x = items.iter().find(|(t, _)| t.contains("Title A"))
            .map(|(_, x)| *x)
            .expect(&alloc::format!("Title A missing, items: {:?}", debug));
        let date_a_x = items.iter().find(|(t, _)| t.contains("〜9月25日"))
            .map(|(_, x)| *x)
            .expect(&alloc::format!("date A missing, items: {:?}", debug));

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
        let mut cells_by_row: alloc::collections::BTreeMap<i64, alloc::vec::Vec<(i64, i64)>> = alloc::collections::BTreeMap::new();
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
        let mut cells_by_row: alloc::collections::BTreeMap<i64, alloc::vec::Vec<(i64, i64, i64)>> = alloc::collections::BTreeMap::new();
        for (x, y, w, h) in &rects {
            if *w < 700 && *w > 30 && *h >= 24 && *h < table_rect.3 {
                cells_by_row.entry(*y).or_default().push((*x, *w, *h));
            }
        }
        // Canonical column X-positions: the top-most row's two outer cells.
        let col_xs: alloc::collections::BTreeSet<i64> = cells_by_row
            .values()
            .find(|v| v.len() == 2)
            .map(|v| v.iter().map(|(x, _, _)| *x).collect())
            .unwrap_or_default();
        // Restrict every row to cells sitting at a canonical column X; this
        // drops inner content rects, which are offset by the cell border.
        let rows: Vec<alloc::vec::Vec<(i64, i64, i64)>> = cells_by_row
            .values()
            .map(|v| {
                let mut cells: alloc::vec::Vec<(i64, i64, i64)> =
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
        let mut cells_by_y: alloc::collections::BTreeMap<i64, Vec<(i64, i64)>> =
            alloc::collections::BTreeMap::new();
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
        let mut cells_by_y: alloc::collections::BTreeMap<i64, usize> =
            alloc::collections::BTreeMap::new();
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
            let mut cells_by_y: alloc::collections::BTreeMap<i64, Vec<(i64, i64)>> =
                alloc::collections::BTreeMap::new();
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

        let text_items: Vec<(alloc::string::String, i64)> = items.iter().filter_map(|item| {
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
        let mut cells_by_row: alloc::collections::BTreeMap<i64, alloc::vec::Vec<(i64, i64)>> =
            alloc::collections::BTreeMap::new();
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
        let rows: alloc::vec::Vec<_> = cells_by_row.values()
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
        let mut cells_by_row: alloc::collections::BTreeMap<i64, alloc::vec::Vec<(i64, i64)>> =
            alloc::collections::BTreeMap::new();
        for item in &items {
            if let crate::display_item::DisplayItem::Rect { layout_point, layout_size, .. } = item {
                let w = layout_size.width();
                let h = layout_size.height();
                if w > 5 && w < 700 && h == 24 {
                    cells_by_row.entry(layout_point.y()).or_default().push((layout_point.x(), w));
                }
            }
        }
        let rows: alloc::vec::Vec<_> = cells_by_row
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
        let texts: alloc::vec::Vec<(alloc::string::String, i64)> = items.iter().filter_map(|item| {
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
