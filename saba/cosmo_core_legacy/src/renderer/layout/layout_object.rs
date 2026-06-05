// Spec: CSS Box Model — margin/border/padding/content areas and box sizing.
// https://www.w3.org/TR/css-box-4/
// Spec: CSS Display — outer/inner display types and block/inline formatting contexts.
// https://www.w3.org/TR/css-display-3/
// Spec: CSS Cascade — specificity, origin, and inheritance resolution order.
// https://www.w3.org/TR/css-cascade-5/
// Spec: CSS Values and Units — length units (px, em, rem, vh, vw) and numeric types.
// https://www.w3.org/TR/css-values-4/
use crate::constants::CHAR_HEIGHT_WITH_PADDING;
use crate::constants::CHAR_WIDTH;
use crate::display_item::ClipRect;
use crate::display_item::DisplayItem;
use crate::display_item::PaintOrder;
use crate::renderer::css::cssom::ComponentValue;
use crate::renderer::css::cssom::Declaration;
use crate::renderer::css::cssom::Selector;
use crate::renderer::css::cssom::StyleSheet;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::Node;
use crate::renderer::dom::node::NodeKind;
use crate::renderer::layout::computed_style::Color;
use crate::renderer::layout::computed_style::ComputedStyle;
use crate::renderer::layout::computed_style::DisplayType;
use crate::renderer::layout::computed_style::FlexDirection;
use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::layout::computed_style::PositionType;
use crate::renderer::layout::computed_style::TextAlign;
use crate::renderer::layout::computed_style::TextDecoration;
use alloc::format;
use alloc::rc::Rc;
use alloc::rc::Weak;
use alloc::string::String;
use alloc::string::ToString;
use alloc::vec;
use alloc::vec::Vec;
use core::cell::RefCell;

fn font_ratio(font_size: FontSize) -> i64 {
    match font_size {
        FontSize::Medium => 1,
        FontSize::XLarge => 2,
        FontSize::XXLarge => 3,
    }
}

fn edge_to_i64(value: f64) -> i64 {
    if value <= 0.0 {
        0
    } else {
        value as i64
    }
}

fn length_to_px(value: f64, unit: &str, base_font_size: FontSize) -> Option<f64> {
    match unit {
        "px" => Some(value),
        "em" => Some(value * base_font_size.px() as f64),
        "rem" => Some(value * FontSize::Medium.px() as f64),
        "vh" | "vw" => Some(value),
        _ => None,
    }
}

fn first_font_family(value: &[ComponentValue]) -> Option<String> {
    value.iter().find_map(|component| match component {
        ComponentValue::Ident(name) | ComponentValue::StringToken(name) => Some(name.clone()),
        _ => None,
    })
}
fn spacing_component_to_px(component: &ComponentValue, base_font_size: FontSize) -> Option<f64> {
    match component {
        ComponentValue::Number(value) => Some(*value),
        ComponentValue::Dimension(value, unit) => length_to_px(*value, unit, base_font_size),
        _ => None,
    }
}

// Ref: CSS Box Model Level 4, margin and padding shorthands.
// https://drafts.csswg.org/css-box-4/#margin-shorthand
// https://drafts.csswg.org/css-box-4/#padding-shorthand
fn parse_spacing_shorthand(
    value: &[ComponentValue],
    base_font_size: FontSize,
) -> Option<(f64, f64, f64, f64)> {
    let components = value
        .iter()
        .filter_map(|component| spacing_component_to_px(component, base_font_size))
        .collect::<Vec<_>>();

    match components.as_slice() {
        [all] => Some((*all, *all, *all, *all)),
        [vertical, horizontal] => Some((*vertical, *horizontal, *vertical, *horizontal)),
        [top, horizontal, bottom] => Some((*top, *horizontal, *bottom, *horizontal)),
        [top, right, bottom, left] => Some((*top, *right, *bottom, *left)),
        _ => None,
    }
}


fn margin_component(component: &ComponentValue, base_font_size: FontSize) -> Option<Option<f64>> {
    match component {
        ComponentValue::Ident(name) if name == "auto" => Some(None),
        _ => spacing_component_to_px(component, base_font_size).map(Some),
    }
}

// Spec: CSS Box Model margin shorthand supports `auto` values, which are positional tokens
// and must not be dropped during 1/2/3/4-value expansion.
// https://drafts.csswg.org/css-box-4/#margin-shorthand
fn parse_margin_shorthand(
    value: &[ComponentValue],
    base_font_size: FontSize,
) -> Option<(Option<f64>, Option<f64>, Option<f64>, Option<f64>)> {
    let components = value
        .iter()
        .map(|component| margin_component(component, base_font_size))
        .collect::<Option<Vec<_>>>()?;

    match components.as_slice() {
        [all] => Some((*all, *all, *all, *all)),
        [vertical, horizontal] => Some((*vertical, *horizontal, *vertical, *horizontal)),
        [top, horizontal, bottom] => Some((*top, *horizontal, *bottom, *horizontal)),
        [top, right, bottom, left] => Some((*top, *right, *bottom, *left)),
        _ => None,
    }
}

fn parse_margin_auto_flags(value: &[ComponentValue]) -> (bool, bool) {
    let flags = value
        .iter()
        .map(|component| matches!(component, ComponentValue::Ident(name) if name == "auto"))
        .collect::<Vec<_>>();

    match flags.as_slice() {
        [all] => (*all, *all),
        [_, horizontal] => (*horizontal, *horizontal),
        [_, horizontal, _] => (*horizontal, *horizontal),
        [_, right, _, left] => (*left, *right),
        _ => (false, false),
    }
}

fn parse_dimension_attr(value: Option<String>) -> Option<i64> {
    let value = value?;
    // Percentage values (e.g. "100%") are not treated as fixed pixel widths.
    // Returning None lets the caller fall through to the available-width default,
    // which gives the correct "fill container" behaviour for width="100%".
    if value.trim_start().contains('%') {
        return None;
    }
    let digits = value
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<i64>().ok()
    }
}

/// Parse an HTML dimension attribute, resolving percentage values against `avail`.
/// `"22%"` with `avail = Some(700)` returns `Some(154)`.
/// Falls back to `parse_dimension_attr` for pixel values.
/// Returns `None` when the value is absent, unparseable, or `avail` is None for a percentage.
fn parse_dimension_pct_attr(value: Option<String>, avail: Option<i64>) -> Option<i64> {
    let value = value?;
    let trimmed = value.trim();
    if let Some(pct_str) = trimmed.strip_suffix('%') {
        let pct: f64 = pct_str.trim().parse().ok()?;
        let avail = avail?;
        Some(((avail as f64) * pct / 100.0) as i64)
    } else {
        parse_dimension_attr(Some(value))
    }
}

fn is_wide_char(c: char) -> bool {
    let cp = c as u32;
    // CJK Unified Ideographs, Hiragana, Katakana, Fullwidth forms, CJK symbols
    (0x3000..=0x9FFF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFF01..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x20000..=0x2FA1F).contains(&cp)
        || (0xAC00..=0xD7AF).contains(&cp) // Hangul
}

fn estimate_text_width_chars(text: &str) -> i64 {
    // Count width units: wide (CJK) chars = 2, narrow chars = 1
    text.chars().map(|c| if is_wide_char(c) { 2 } else { 1 }).sum::<i64>()
}

fn measure_text_width(text: &str, font_size: FontSize) -> i64 {
    estimate_text_width_chars(text) * CHAR_WIDTH * font_ratio(font_size)
}
fn is_break_space(c: char) -> bool {
    c == ' ' || c == '\u{3000}'
}

/// Greedily wrap `line` into visual lines no wider than `max_width`.
///
/// This is a single forward pass: O(n) in the text length. The previous
/// implementation recursed on the un-consumed remainder and, on every line,
/// re-measured the whole remainder and re-collected all of its char indices —
/// O(n²). On real pages that carry a very long unbroken run of text (e.g. a
/// page's minified inline script that ends up in the inline flow) the quadratic
/// cost stalled layout for many seconds and effectively hung the renderer.
///
/// Breaking prefers the last space that still fits on the line (the space is
/// consumed, not rendered); if a single run has no space it is hard-broken at
/// the character that would overflow. Wide (CJK) characters count as two units,
/// matching [`estimate_text_width_chars`].
fn split_text(line: String, char_width: i64, max_width: i64) -> Vec<String> {
    let safe_width = max_width.max(char_width).max(1);
    let max_units = (safe_width / char_width).max(1);

    let mut result: Vec<String> = vec![];
    let mut line_start = 0usize; // byte offset where the current visual line starts
    let mut cur_units = 0i64; // width units accumulated on the current line
    let mut started = false; // whether the current line has consumed any char
    // Last space seen on the current line: (its byte offset, byte offset just
    // after it, units accumulated since it). Used to break at word boundaries.
    let mut last_space: Option<usize> = None;
    let mut byte_after_space = 0usize;
    let mut units_after_space = 0i64;

    for (idx, c) in line.char_indices() {
        let w = if is_wide_char(c) { 2 } else { 1 };

        if started && cur_units + w > max_units {
            match last_space {
                // Break at the last space: it is dropped and the next line
                // starts after it, carrying the text seen since the space.
                Some(space_byte) => {
                    result.push(line[line_start..space_byte].to_string());
                    line_start = byte_after_space;
                    cur_units = units_after_space;
                    last_space = None;
                }
                // No space to break on: hard-break before the overflowing char.
                None => {
                    result.push(line[line_start..idx].to_string());
                    line_start = idx;
                    cur_units = 0;
                }
            }
        }

        cur_units += w;
        started = true;
        if is_break_space(c) {
            last_space = Some(idx);
            byte_after_space = idx + c.len_utf8();
            units_after_space = 0;
        } else {
            units_after_space += w;
        }
    }

    if line_start < line.len() {
        result.push(line[line_start..].to_string());
    }
    result
}

pub fn create_layout_object(
    node: &Option<Rc<RefCell<Node>>>,
    parent_obj: &Option<Rc<RefCell<LayoutObject>>>,
    cssom: &StyleSheet,
) -> Option<Rc<RefCell<LayoutObject>>> {
    if let Some(n) = node {
        let layout_object = Rc::new(RefCell::new(LayoutObject::new(n.clone(), parent_obj)));

        for rule in &cssom.rules {
            if layout_object.borrow().is_node_selected(&rule.selector) {
                layout_object
                    .borrow_mut()
                    .cascading_style(rule.declarations.clone());
            }
        }

        let parent_style = parent_obj.as_ref().map(|parent| parent.borrow().style());
        layout_object.borrow_mut().defaulting_style(n, parent_style);

        if layout_object.borrow().style().display() == DisplayType::DisplayNone {
            return None;
        }

        layout_object.borrow_mut().update_kind();
        return Some(layout_object);
    }
    None
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LayoutObjectKind {
    Block,
    Inline,
    Text,
}


#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LayoutFlow {
    BlockFormattingContext,
    InlineFlow,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct NormalFlowSpec {
    pub flow: LayoutFlow,
    pub stacks_vertically: bool,
    pub keeps_inline_line: bool,
}

impl LayoutObjectKind {
    pub fn normal_flow_spec(&self) -> NormalFlowSpec {
        match self {
            LayoutObjectKind::Block => NormalFlowSpec {
                flow: LayoutFlow::BlockFormattingContext,
                stacks_vertically: true,
                keeps_inline_line: false,
            },
            LayoutObjectKind::Inline | LayoutObjectKind::Text => NormalFlowSpec {
                flow: LayoutFlow::InlineFlow,
                stacks_vertically: false,
                keeps_inline_line: true,
            },
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BoxEdges {
    pub top: i64,
    pub right: i64,
    pub bottom: i64,
    pub left: i64,
}

impl BoxEdges {
    pub fn horizontal(&self) -> i64 {
        self.left + self.right
    }

    pub fn vertical(&self) -> i64 {
        self.top + self.bottom
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BoxModelMetrics {
    pub margin: BoxEdges,
    pub padding: BoxEdges,
    pub border: BoxEdges,
}

impl BoxModelMetrics {
    pub fn outer_horizontal(&self) -> i64 {
        self.margin.horizontal() + self.padding.horizontal() + self.border.horizontal()
    }

    pub fn outer_vertical(&self) -> i64 {
        self.margin.vertical() + self.padding.vertical() + self.border.vertical()
    }

    pub fn inner_horizontal(&self) -> i64 {
        self.padding.horizontal() + self.border.horizontal()
    }

    pub fn inner_vertical(&self) -> i64 {
        self.padding.vertical() + self.border.vertical()
    }
}

pub fn compute_box_model_metrics(style: &ComputedStyle) -> BoxModelMetrics {
    let margin = style.margin();
    let padding = style.padding();
    let border = style.border();

    BoxModelMetrics {
        margin: BoxEdges {
            top: edge_to_i64(margin.top()),
            right: edge_to_i64(margin.right()),
            bottom: edge_to_i64(margin.bottom()),
            left: edge_to_i64(margin.left()),
        },
        padding: BoxEdges {
            top: edge_to_i64(padding.top()),
            right: edge_to_i64(padding.right()),
            bottom: edge_to_i64(padding.bottom()),
            left: edge_to_i64(padding.left()),
        },
        border: BoxEdges {
            top: edge_to_i64(border.top()),
            right: edge_to_i64(border.right()),
            bottom: edge_to_i64(border.bottom()),
            left: edge_to_i64(border.left()),
        },
    }
}

#[derive(Debug, Clone)]
pub struct LayoutObject {
    kind: LayoutObjectKind,
    node: Rc<RefCell<Node>>,
    first_child: Option<Rc<RefCell<LayoutObject>>>,
    next_sibling: Option<Rc<RefCell<LayoutObject>>>,
    parent: Weak<RefCell<LayoutObject>>,
    style: ComputedStyle,
    point: LayoutPoint,
    size: LayoutSize,
    // The max_width used in split_text() during compute_size for Text nodes.
    // Cached here so that paint() uses the identical line-breaking boundary,
    // preventing the double-split divergence that causes text to stack
    // vertically instead of flowing horizontally.
    // Spec: CSS2.2 §9.4.2 — inline formatting context line construction.
    // https://www.w3.org/TR/CSS22/visuren.html#inline-formatting
    text_line_max_width: i64,
    // Per-logical-column max of min_content_width_hint, populated once per
    // table by the pre-pass before any cell sizing.  Only meaningful on table
    // nodes; None elsewhere.  Used by `table_cell_auto_width` so that a row
    // whose cell content is narrow (e.g. &nbsp;) still reserves space for a
    // sibling row whose cell at the same column has substantial content.
    // Spec: CSS 2.2 §17.5.2 — table layout: auto.
    // https://www.w3.org/TR/CSS22/tables.html#auto-table-layout
    column_min_hints: Option<Vec<i64>>,
}

impl PartialEq for LayoutObject {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl LayoutObject {
    pub fn new(node: Rc<RefCell<Node>>, parent_obj: &Option<Rc<RefCell<LayoutObject>>>) -> Self {
        let parent = match parent_obj {
            Some(p) => Rc::downgrade(p),
            None => Weak::new(),
        };

        Self {
            kind: LayoutObjectKind::Block,
            node: node.clone(),
            first_child: None,
            next_sibling: None,
            parent,
            style: ComputedStyle::new(),
            point: LayoutPoint::new(0, 0),
            size: LayoutSize::new(0, 0),
            text_line_max_width: 0,
            column_min_hints: None,
        }
    }

    fn link_href(&self) -> Option<String> {
        let mut current = Some(self.node.clone());
        while let Some(node) = current {
            if let NodeKind::Element(element) = node.borrow().kind() {
                if element.kind() == ElementKind::A {
                    return element.get_attribute("href");
                }
            }
            current = node.borrow().parent().upgrade();
        }
        None
    }

    /// Walk ancestor chain to the nearest `<a>` element and return its `target`
    /// attribute value.
    /// Spec: HTML Living Standard §4.6.21 — the `target` attribute on `<a>`.
    /// https://html.spec.whatwg.org/multipage/links.html#attr-hyperlink-target
    fn link_target(&self) -> Option<String> {
        let mut current = Some(self.node.clone());
        while let Some(node) = current {
            if let NodeKind::Element(element) = node.borrow().kind() {
                if element.kind() == ElementKind::A {
                    return element.get_attribute("target");
                }
            }
            current = node.borrow().parent().upgrade();
        }
        None
    }

    fn element_kind(&self) -> Option<ElementKind> {
        self.node.borrow().element_kind()
    }

    pub fn is_table_cell(&self) -> bool {
        matches!(self.element_kind(), Some(ElementKind::Td) | Some(ElementKind::Th))
    }

    pub fn is_table_row(&self) -> bool {
        matches!(self.element_kind(), Some(ElementKind::Tr))
    }

    pub fn is_row_group(&self) -> bool {
        matches!(self.element_kind(),
            Some(ElementKind::Tbody) | Some(ElementKind::Thead) | Some(ElementKind::Tfoot))
    }

    /// Walk up the layout tree to find the nearest ancestor table cell.
    fn nearest_ancestor_cell(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        let mut current = self.parent.upgrade();
        while let Some(node) = current {
            if node.borrow().is_table_cell() {
                return Some(node);
            }
            let next = node.borrow().parent.upgrade();
            current = next;
        }
        None
    }

    /// Walk up the layout tree to find the nearest block-level ancestor (including
    /// table cells), used to derive a reliable max-width for text wrapping.
    fn nearest_block_ancestor_width(&self) -> Option<i64> {
        let mut current = self.parent.upgrade();
        while let Some(node) = current {
            let b = node.borrow();
            if b.is_table_cell() || b.kind() == LayoutObjectKind::Block {
                let cm = compute_box_model_metrics(&b.style);
                let w = b.size().width() - cm.inner_horizontal();
                return if w > 0 { Some(w) } else { None };
            }
            let next = b.parent.upgrade();
            drop(b);
            current = next;
        }
        None
    }

    /// Scan descendants (up to `depth` levels) for elements that imply a
    /// minimum width (e.g. `<img width="350">`, `<table width="256">`).
    /// Returns the maximum such hint, or 0 if none found.
    /// If this object's parent is a flex container (`display:flex`), return the
    /// container's main-axis direction; otherwise `None`. Used so a flex item
    /// can size and position itself against the flex algorithm.
    fn parent_flex_direction(&self) -> Option<FlexDirection> {
        let parent = self.parent.upgrade()?;
        let p = parent.borrow();
        if p.style.display() == DisplayType::Flex {
            Some(p.style.flex_direction())
        } else {
            None
        }
    }

    fn is_flex_container(&self) -> bool {
        self.style.display() == DisplayType::Flex
    }

    /// Preferred (max-content) width: the width this subtree would take with no
    /// line wrapping. Inline/text runs accumulate on a line; block children
    /// each take their own line (we take the max). Used to shrink-to-fit flex
    /// row items so siblings sit side by side instead of each filling the row.
    fn max_content_width(&self) -> i64 {
        self.max_content_width_depth(6)
    }

    fn max_content_width_depth(&self, depth: u32) -> i64 {
        if depth == 0 {
            return 0;
        }
        if let Some(w) = parse_dimension_attr(self.element_attribute("width")) {
            return w;
        }
        let mut max_line: i64 = 0; // widest block-level line seen
        let mut cur_line: i64 = 0; // current inline run accumulation
        let mut child = self.first_child();
        while let Some(c) = child {
            let b = c.borrow();
            match b.node_kind() {
                NodeKind::Text(ref t) => {
                    let fs = b.style.font_size();
                    // Each hard line within the text starts a new line box.
                    let widest = t
                        .split('\n')
                        .map(|line| measure_text_width(line.trim(), fs))
                        .max()
                        .unwrap_or(0);
                    cur_line += widest;
                }
                _ => {
                    let child_pref = b.max_content_width_depth(depth - 1);
                    if b.kind().normal_flow_spec().stacks_vertically {
                        // Block-level child: flush the inline run and stand alone.
                        max_line = max_line.max(cur_line);
                        cur_line = 0;
                        max_line = max_line.max(child_pref);
                    } else {
                        cur_line += child_pref;
                    }
                }
            }
            let next = b.next_sibling();
            drop(b);
            child = next;
        }
        max_line.max(cur_line)
    }

    fn min_content_width_hint(&self) -> i64 {
        self.min_content_width_hint_depth(6)
    }

    fn min_content_width_hint_depth(&self, depth: u32) -> i64 {
        if depth == 0 {
            return 0;
        }
        let mut max_hint: i64 = 0;
        let mut child = self.first_child();
        while let Some(c) = child {
            let borrowed = c.borrow();
            let hint = parse_dimension_attr(borrowed.element_attribute("width")).unwrap_or(0);
            max_hint = max_hint.max(hint);
            // For text nodes, use the longest *unbreakable* run.
            // CJK chars can break between any two characters; each is its own
            // minimum unit.  Pure-ASCII runs between wide chars are truly
            // unbreakable and may be wider — take the max of the two.
            if let NodeKind::Text(ref t) = borrowed.node_kind() {
                let font_size = borrowed.style.font_size();
                let longest = t.split(|c: char| c == ' ' || c == '\u{3000}' || c == '\n' || c == '\t')
                    .map(|word| {
                        // Longest ASCII run between wide chars within this word.
                        let ascii_max = word.split(|c: char| is_wide_char(c))
                            .map(|seg| measure_text_width(seg.trim(), font_size))
                            .max()
                            .unwrap_or(0);
                        // Each wide char is its own break unit.  We return
                        // 3×CHAR_WIDTH so that any cell with CJK content exceeds
                        // SPACER_THRESHOLD (20) and is classified as flexible
                        // content rather than a decorative spacer.
                        let wide_min = if word.chars().any(|c| is_wide_char(c)) {
                            3 * CHAR_WIDTH * font_ratio(font_size)
                        } else {
                            0
                        };
                        ascii_max.max(wide_min)
                    })
                    .max()
                    .unwrap_or(0);
                max_hint = max_hint.max(longest);
            }
            max_hint = max_hint.max(borrowed.min_content_width_hint_depth(depth - 1));
            let next = borrowed.next_sibling();
            drop(borrowed);
            child = next;
        }
        max_hint
    }

    /// Determine this cell's column index (0-based) within its parent row.
    fn cell_column_index(&self) -> usize {
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return 0,
        };
        let mut index: usize = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            match c.try_borrow() {
                Ok(borrowed) => {
                    if borrowed.is_table_cell() {
                        index += 1;
                    }
                    let next = borrowed.next_sibling();
                    drop(borrowed);
                    child = next;
                }
                Err(_) => {
                    // This is self.
                    return index;
                }
            }
        }
        index
    }

    /// Look at sibling rows in the same table for an explicit width or already-
    /// computed size at the given column index.  Returns the width if found.
    /// Handles tables with tbody/thead/tfoot row groups transparently.
    fn column_width_from_sibling_rows(&self, col_index: usize) -> Option<i64> {
        let row = self.parent.upgrade()?;
        // When a row group (<tbody>/<thead>/<tfoot>) wraps <tr>, step up one more level.
        let row_parent = row.borrow().parent.upgrade()?;
        let table = if row_parent.borrow().is_row_group() {
            row_parent.borrow().parent.upgrade()?
        } else {
            row_parent
        };
        // Resolve percentage width attributes against the table's explicit pixel width.
        let table_px_width = parse_dimension_attr(table.borrow().element_attribute("width"))
            .or_else(|| {
                let tw = table.borrow().size.width();
                if tw > 0 { Some(tw) } else { None }
            });
        // Collect all logical rows (across all row groups).
        let logical_rows = Self::collect_logical_rows_static(&table);
        let mut best: Option<i64> = None;
        for sr in &logical_rows {
            if Rc::ptr_eq(sr, &row) {
                continue;
            }
            let sr_borrowed = sr.borrow();
            let mut idx: usize = 0;
            let mut cell = sr_borrowed.first_child();
            while let Some(c) = cell {
                let cb = c.borrow();
                if cb.is_table_cell() {
                    if idx == col_index {
                        // A colspan > 1 cell spans multiple logical columns.
                        // Its full width must not be used as a hint for a single
                        // column: doing so would over-allocate that column and
                        // starve subsequent columns of space.
                        if cb.cell_colspan() == 1 {
                            let candidate =
                                parse_dimension_pct_attr(cb.element_attribute("width"), table_px_width)
                                .or_else(|| {
                                    let total = cb.size.width();
                                    if total > 0 {
                                        // Return the CONTENT width (strip border+padding overhead)
                                        // so that each row inherits a consistent column width
                                        // regardless of how many border pixels were already
                                        // accumulated.  Without this, every row would be 2px
                                        // wider than the previous one (border inflation).
                                        let cb_metrics = compute_box_model_metrics(&cb.style);
                                        Some((total - cb_metrics.inner_horizontal()).max(0))
                                    } else {
                                        None
                                    }
                                });
                            if let Some(w) = candidate {
                                best = Some(best.map_or(w, |b: i64| b.max(w)));
                            }
                        }
                    }
                    idx += 1;
                }
                let next = cb.next_sibling();
                drop(cb);
                cell = next;
            }
        }
        best
    }

    /// Collect all logical `<tr>` children of a table, traversing through row groups.
    fn collect_logical_rows_static(table: &Rc<RefCell<LayoutObject>>) -> Vec<Rc<RefCell<LayoutObject>>> {
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

    /// Public entry point for `column_width_from_sibling_rows` so that sibling
    /// cells can call it during the width-distribution pass.
    pub fn column_width_from_sibling_rows_pub(&self, col_index: usize) -> Option<i64> {
        self.column_width_from_sibling_rows(col_index)
    }

    /// Return the per-column max content hint vector populated by the table
    /// pre-pass, if any. Only set on `<table>` nodes.
    pub fn column_min_hints(&self) -> Option<&[i64]> {
        self.column_min_hints.as_deref()
    }

    /// Store the per-column max content hint vector on this (table) node.
    pub fn set_column_min_hints(&mut self, v: Vec<i64>) {
        self.column_min_hints = Some(v);
    }

    /// Walk all logical rows of a `<table>` and compute the maximum
    /// min_content_width_hint (and explicit HTML width attribute) per logical
    /// column index. The vector returned has one entry per column.
    ///
    /// This runs as a pre-pass before any cell sizing, so that `table_cell_auto_width`
    /// in row 1 already knows that some later row has substantial content in
    /// a column that row 1 leaves as a spacer.
    ///
    /// Handles `<tbody>`/`<thead>`/`<tfoot>` row groups transparently via
    /// `collect_logical_rows_static`. Logical column indices track rowspan
    /// occupancy across rows.
    ///
    /// For `colspan > 1` cells, the cell hint is only distributed across the
    /// spanned columns where the existing sum is below the hint, ensuring no
    /// over-allocation. Spec: CSS 2.2 §17.5.2.1.
    pub fn compute_table_column_min_hints(table: &Rc<RefCell<LayoutObject>>) -> Vec<i64> {
        let table_px_width = parse_dimension_attr(table.borrow().element_attribute("width"));
        let logical_rows = Self::collect_logical_rows_static(table);
        let mut col_min: Vec<i64> = Vec::new();
        // `occupied[i]` is the remaining number of rows that column i is still
        // taken by a rowspan cell from an earlier row.
        let mut occupied: Vec<usize> = Vec::new();
        // Deferred colspan cells: (start_col, end_col_exclusive, hint).
        let mut colspan_cells: Vec<(usize, usize, i64)> = Vec::new();

        for row in &logical_rows {
            let row_b = row.borrow();
            let mut logical_col: usize = 0;
            let mut cell = row_b.first_child();
            while let Some(c) = cell {
                let cb = c.borrow();
                if cb.is_table_cell() {
                    // Skip columns currently occupied by a rowspan cell.
                    while logical_col < occupied.len() && occupied[logical_col] > 0 {
                        logical_col += 1;
                    }
                    let colspan = cb.cell_colspan();
                    let rowspan = parse_dimension_attr(cb.element_attribute("rowspan"))
                        .unwrap_or(1)
                        .max(1) as usize;
                    let end_col = logical_col + colspan;
                    // Ensure vectors have room.
                    if col_min.len() < end_col {
                        col_min.resize(end_col, 0);
                    }
                    if occupied.len() < end_col {
                        occupied.resize(end_col, 0);
                    }
                    // Compute this cell's hint, considering explicit width attr.
                    let attr_w = parse_dimension_pct_attr(
                        cb.element_attribute("width"),
                        table_px_width,
                    )
                    .unwrap_or(0);
                    let content_hint = cb.min_content_width_hint();
                    let cell_hint = attr_w.max(content_hint);
                    if colspan == 1 {
                        col_min[logical_col] = col_min[logical_col].max(cell_hint);
                    } else {
                        colspan_cells.push((logical_col, end_col, cell_hint));
                    }
                    // Mark rowspan occupancy for subsequent rows.
                    if rowspan > 1 {
                        for i in logical_col..end_col {
                            occupied[i] = occupied[i].max(rowspan);
                        }
                    }
                    logical_col = end_col;
                }
                let next = cb.next_sibling();
                drop(cb);
                cell = next;
            }
            // Tick down occupancy after each row.
            for o in occupied.iter_mut() {
                if *o > 0 {
                    *o -= 1;
                }
            }
        }

        // Second sweep: distribute colspan cell hints across spanned columns
        // only where the existing single-cell sum is below the cell hint.
        for (start, end, hint) in colspan_cells {
            if start >= col_min.len() {
                continue;
            }
            let end = end.min(col_min.len());
            if end <= start {
                continue;
            }
            let current_sum: i64 = col_min[start..end].iter().sum();
            if hint > current_sum {
                let deficit = hint - current_sum;
                let span = (end - start) as i64;
                let share = deficit / span;
                let remainder = deficit - share * span;
                for (i, slot) in col_min[start..end].iter_mut().enumerate() {
                    let add = share + if (i as i64) < remainder { 1 } else { 0 };
                    *slot += add;
                }
            }
        }

        col_min
    }

    /// Return this cell's colspan attribute value (defaults to 1).
    pub fn cell_colspan(&self) -> usize {
        parse_dimension_attr(self.element_attribute("colspan"))
            .unwrap_or(1)
            .max(1) as usize
    }

    /// From a table cell, walk up to the containing `<table>` and return its
    /// per-column min content hints (populated by the table pre-pass), if any.
    fn ancestor_table_column_min_hints(&self) -> Option<Vec<i64>> {
        let row = self.parent.upgrade()?;
        let row_parent = row.borrow().parent.upgrade()?;
        let table = if row_parent.borrow().is_row_group() {
            row_parent.borrow().parent.upgrade()?
        } else {
            row_parent
        };
        let t = table.borrow();
        t.column_min_hints().map(|s| s.to_vec())
    }

    /// Compute the width this cell should use, accounting for sibling cells
    /// that have explicit HTML width attributes, and rowspan cells from
    /// previous rows that reduce the available width.
    /// Auto-width cells with large intrinsic content (images, nested tables)
    /// receive at least their minimum content width before equal distribution.
    /// Also checks sibling rows for column width hints (including colspan).
    fn table_cell_auto_width(&self, available_width: i64) -> i64 {
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return available_width,
        };
        // Reduce available width by rowspan columns from previous rows.
        let rowspan_offset = parent.borrow().rowspan_column_offset();

        // Per-column max content hints from the table-level pre-pass.  Used to
        // guarantee that a column with substantial content in some other row
        // is not starved by row 1's narrow cell. Spec: CSS 2.2 §17.5.2.
        //
        // IMPORTANT: column_min_hints is indexed by LOGICAL column.  Inside
        // this function we iterate cells with a PHYSICAL counter (`col_idx`).
        // The two coincide only when there are no rowspan offsets from
        // previous rows and no colspan cells in the current row.  Otherwise
        // the indices diverge and using column_min_hints[col_idx] would
        // promote the wrong column.  We compute that safety condition once
        // and gate every column-hint lookup on it.
        let column_min_hints_raw = self.ancestor_table_column_min_hints();
        let row_has_colspan = {
            let mut has = self.cell_colspan() > 1;
            let mut child = parent.borrow().first_child();
            while let Some(c) = child {
                // `self` is already mutably borrowed; skip it with try_borrow.
                match c.try_borrow() {
                    Ok(cb) => {
                        if cb.is_table_cell() && cb.cell_colspan() > 1 {
                            has = true;
                        }
                        let next = cb.next_sibling();
                        drop(cb);
                        child = next;
                    }
                    Err(_) => {
                        // This is self; we already accounted for our own colspan above.
                        let next = unsafe { (*c.as_ptr()).next_sibling() };
                        child = next;
                    }
                }
                if has { break; }
            }
            has
        };
        let column_min_hints = if rowspan_offset == 0 && !row_has_colspan {
            column_min_hints_raw
        } else {
            None
        };

        // Check if sibling rows have explicit widths for the columns this cell
        // spans.  Skip this check when:
        //  - rowspan columns shift cell indices (raw cell_column_index is unreliable), or
        //  - self has rowspan > 1: sibling rows don't have a real cell at this
        //    cell's column — the slot is occupied by the rowspan cell itself, so
        //    sibling-row lookups return data from the WRONG column.
        let self_rowspan = parse_dimension_attr(self.element_attribute("rowspan"))
            .unwrap_or(1)
            .max(1);
        if rowspan_offset == 0 && self_rowspan <= 1 {
            let col_index = self.cell_column_index();
            let colspan = self.cell_colspan();
            let mut total_from_siblings: i64 = 0;
            let mut found_all = true;
            for ci in col_index..(col_index + colspan) {
                if let Some(w) = self.column_width_from_sibling_rows(ci) {
                    total_from_siblings += w;
                } else {
                    found_all = false;
                }
            }
            // If the sibling-row widths are large enough to cover the per-column
            // min hints for the spanned columns, trust them. Otherwise fall
            // through to general distribution so this row can claim adequate
            // space when a later row's content widens the column.
            if found_all && total_from_siblings > 0 {
                let column_min_total: i64 = column_min_hints
                    .as_ref()
                    .map(|h| {
                        (col_index..col_index + colspan)
                            .filter_map(|i| h.get(i).copied())
                            .sum::<i64>()
                    })
                    .unwrap_or(0);
                if total_from_siblings >= column_min_total {
                    return total_from_siblings.min(available_width);
                }
            }
        }

        let effective_width = (available_width - rowspan_offset).max(0);

        // Pre-compute the column index of this cell so we can identify our own slot.
        let my_col_index = if rowspan_offset == 0 { self.cell_column_index() } else { usize::MAX };

        let mut total_explicit: i64 = 0;
        let mut auto_cells: Vec<(bool, i64)> = Vec::new(); // (is_self, min_hint)
        let mut self_index: usize = 0;
        let mut col_idx: usize = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            match c.try_borrow() {
                Ok(borrowed) => {
                    if borrowed.is_table_cell() {
                        // Percentage widths are resolved against effective_width here.
                        if let Some(w) = parse_dimension_pct_attr(
                            borrowed.element_attribute("width"),
                            Some(effective_width),
                        ) {
                            total_explicit += w;
                        } else if let Some(w) = (col_idx != my_col_index
                            // Skip the sibling-row lookup for cells with rowspan > 1.
                            // Such cells span into sibling rows where the column index
                            // corresponds to a different cell (shifted by the rowspan
                            // occupying that slot), so the lookup returns wrong widths.
                            && parse_dimension_attr(borrowed.element_attribute("rowspan"))
                                .unwrap_or(1)
                                .max(1)
                                <= 1
                            // Skip sibling-row lookup when this row has a rowspan
                            // offset: the physical col_idx in this row does not match
                            // the logical column in sibling rows (they are shifted by
                            // the number of rowspan-occupied columns), so the lookup
                            // would return the width of the wrong column.
                            && rowspan_offset == 0)
                            .then(|| borrowed.column_width_from_sibling_rows_pub(col_idx))
                            .flatten()
                        {
                            // This sibling has no explicit width in this row but will
                            // inherit one from sibling rows; treat it as "explicit" so
                            // the remaining space is divided correctly.
                            total_explicit += w;
                        } else {
                            let cell_hint = borrowed.min_content_width_hint();
                            // Promote to the column-level min hint if larger:
                            // ensures this cell reserves space for any sibling
                            // row in the same column whose content is wider.
                            let col_hint = column_min_hints
                                .as_ref()
                                .and_then(|h| h.get(col_idx).copied())
                                .unwrap_or(0);
                            let hint = cell_hint.max(col_hint);
                            auto_cells.push((false, hint));
                        }
                        col_idx += 1;
                    }
                    let next = borrowed.next_sibling();
                    drop(borrowed);
                    child = next;
                }
                Err(_) => {
                    // This is self — it has no explicit width (caller already checked).
                    let cell_hint = self.min_content_width_hint();
                    let col_hint = column_min_hints
                        .as_ref()
                        .and_then(|h| h.get(col_idx).copied())
                        .unwrap_or(0);
                    let hint = cell_hint.max(col_hint);
                    self_index = auto_cells.len();
                    auto_cells.push((true, hint));
                    col_idx += 1;
                    let next = c.as_ptr();
                    child = unsafe { (*next).next_sibling() };
                }
            }
        }

        let remaining = (effective_width - total_explicit).max(0);
        let auto_count = auto_cells.len();
        if auto_count == 0 {
            return effective_width;
        }

        let equal_share = remaining / auto_count as i64;
        let total_min: i64 = auto_cells.iter().map(|(_, h)| *h).sum();
        if total_min <= remaining && total_min > 0 {
            let surplus = remaining - total_min;
            let (_, my_min) = auto_cells[self_index];

            // Cells whose hint is "large" (≥ CONTENT_SIZED_THRESHOLD) represent
            // fixed-size content like images or large explicitly-sized nested tables.
            // These cells should not grow beyond their minimum content width — extra
            // space inside an image cell, for instance, does nothing useful.
            //
            // Cells with hints between SPACER_THRESHOLD and CONTENT_SIZED_THRESHOLD
            // are "flexible" content cells: they grow to fill remaining space.
            //
            // Cells with tiny hints (< SPACER_THRESHOLD) are decorative spacers
            // (e.g. <td>&nbsp;</td>) that must not absorb surplus table width.
            const CONTENT_SIZED_THRESHOLD: i64 = 150;
            const SPACER_THRESHOLD: i64 = 20;
            let total_content_sized: i64 = auto_cells
                .iter()
                .filter(|(_, h)| *h >= CONTENT_SIZED_THRESHOLD)
                .map(|(_, h)| *h)
                .sum();
            let has_flexible = auto_cells
                .iter()
                .any(|(_, h)| *h >= SPACER_THRESHOLD && *h < CONTENT_SIZED_THRESHOLD);

            if my_min >= CONTENT_SIZED_THRESHOLD {
                // Large content hint: do not grow beyond the minimum.
                my_min
            } else if my_min < SPACER_THRESHOLD {
                // Tiny spacer: stay at minimum, never absorb surplus.
                my_min.max(1)
            } else if has_flexible {
                // Flexible content cell: split remaining space after content-sized
                // cells and spacers.
                let total_spacer: i64 = auto_cells
                    .iter()
                    .filter(|(_, h)| *h < SPACER_THRESHOLD)
                    .map(|(_, h)| *h)
                    .sum();
                let remaining_for_flexible =
                    (remaining - total_content_sized - total_spacer).max(0);
                let total_flexible_min: i64 = auto_cells
                    .iter()
                    .filter(|(_, h)| *h >= SPACER_THRESHOLD && *h < CONTENT_SIZED_THRESHOLD)
                    .map(|(_, h)| *h)
                    .sum();
                let flex_surplus =
                    (remaining_for_flexible - total_flexible_min).max(0);
                let flexible_count = auto_cells
                    .iter()
                    .filter(|(_, h)| *h >= SPACER_THRESHOLD && *h < CONTENT_SIZED_THRESHOLD)
                    .count()
                    .max(1);
                my_min + flex_surplus / flexible_count as i64
            } else if my_min > equal_share {
                // All cells are content-sized and this one needs more than equal share.
                my_min
            } else {
                // All cells are content-sized: distribute surplus proportionally.
                const DEFAULT_MIN: i64 = 16;
                let total_effective: i64 = auto_cells
                    .iter()
                    .map(|(_, h)| (*h).max(DEFAULT_MIN))
                    .sum();
                let my_effective = my_min.max(DEFAULT_MIN);
                let bonus = if total_effective > 0 {
                    surplus * my_effective / total_effective
                } else {
                    0
                };
                my_min + bonus
            }
        } else {
            // Simple equal division (no mins, or mins exceed available).
            equal_share
        }
    }

    /// Sum of all explicit HTML width attributes among sibling table cells.
    fn total_sibling_explicit_widths(&self) -> i64 {
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return 0,
        };
        let mut total: i64 = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            match c.try_borrow() {
                Ok(borrowed) => {
                    if borrowed.is_table_cell() {
                        if let Some(w) = parse_dimension_attr(borrowed.element_attribute("width")) {
                            total += w;
                        }
                    }
                    let next = borrowed.next_sibling();
                    drop(borrowed);
                    child = next;
                }
                Err(_) => {
                    // This is self — check our own width attribute.
                    if let Some(w) = parse_dimension_attr(self.element_attribute("width")) {
                        total += w;
                    }
                    let next = c.as_ptr();
                    child = unsafe { (*next).next_sibling() };
                }
            }
        }
        total
    }

    pub fn element_attribute(&self, name: &str) -> Option<String> {
        match self.node.borrow().kind() {
            NodeKind::Element(ref element) => element.get_attribute(name),
            _ => None,
        }
    }

    /// Returns true if every child layout object of this cell is Inline or Text
    /// (i.e. no block-level children).  Used to detect bullet-marker cells that
    /// should be vertically centred inside the row.
    fn has_only_inline_children(&self) -> bool {
        let mut child = self.first_child();
        let mut found_any = false;
        while let Some(c) = child {
            let b = c.borrow();
            match b.kind() {
                LayoutObjectKind::Block => return false,
                _ => {}
            }
            found_any = true;
            let next = b.next_sibling();
            drop(b);
            child = next;
        }
        found_any
    }

    /// Compute the x-offset for cells in this row due to rowspan cells from
    /// previous sibling rows.  Returns the total width that is "occupied" by
    /// spanning cells, so the first real cell in this row should start at
    /// parent_x + offset.
    fn rowspan_column_offset(&self) -> i64 {
        if !self.is_table_row() {
            return 0;
        }
        // Walk backwards through previous sibling rows.
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return 0,
        };
        // Determine our row index among siblings.
        let mut row_index: usize = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            let is_self = Rc::ptr_eq(&self.node, &c.borrow().node);
            if is_self {
                break;
            }
            if c.borrow().is_table_row() {
                row_index += 1;
            }
            let next = c.borrow().next_sibling();
            child = next;
        }
        if row_index == 0 {
            return 0;
        }
        // Now scan previous rows for cells with rowspan > 1 that extend into us.
        let mut offset: i64 = 0;
        let mut n_rowspan_cols: i64 = 0;
        let mut prev_row_index: usize = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            if c.borrow().is_table_row() {
                if prev_row_index >= row_index {
                    break;
                }
                // Scan cells in this row.
                let mut cell = c.borrow().first_child();
                while let Some(cell_rc) = cell {
                    let cell_borrowed = cell_rc.borrow();
                    if cell_borrowed.is_table_cell() {
                        let rowspan = parse_dimension_attr(cell_borrowed.element_attribute("rowspan"))
                            .unwrap_or(1);
                        if rowspan > 1 && prev_row_index + rowspan as usize > row_index {
                            offset += cell_borrowed.size.width();
                            n_rowspan_cols += 1;
                        }
                    }
                    let next = cell_borrowed.next_sibling();
                    drop(cell_borrowed);
                    cell = next;
                }
                prev_row_index += 1;
            }
            let next = c.borrow().next_sibling();
            child = next;
        }
        // Include the cellspacing gap that precedes each rowspan column so that
        // the first cell placed after them lands at the same X as its counterpart
        // in the rowspan row (which was also offset by cs per preceding cell).
        let cs = self.ancestor_table_cellspacing();
        offset + n_rowspan_cols * cs
    }

    fn placeholder_text(&self) -> Option<String> {
        match self.element_kind()? {
            ElementKind::Img => Some(
                self.element_attribute("alt")
                    .filter(|alt| !alt.trim().is_empty())
                    .unwrap_or_else(|| {
                        self.element_attribute("src")
                            .map(|src| format!("Image: {src}"))
                            .unwrap_or_else(|| "Image".to_string())
                    }),
            ),
            ElementKind::Input => Some(
                self.element_attribute("value")
                    .or_else(|| self.element_attribute("placeholder"))
                    .unwrap_or_else(|| "Input".to_string()),
            ),
            _ => None,
        }
    }

    fn intrinsic_inline_size(&self, parent_size: LayoutSize) -> Option<LayoutSize> {
        let explicit_width = self.resolved_width(parent_size);
        let explicit_height = self.resolved_height(parent_size);
        let width_attr = parse_dimension_attr(self.element_attribute("width"));
        let height_attr = parse_dimension_attr(self.element_attribute("height"));

        match self.element_kind()? {
            ElementKind::Img => Some(LayoutSize::new(
                explicit_width.max(width_attr.unwrap_or(220)),
                explicit_height.max(height_attr.unwrap_or(140)),
            )),
            ElementKind::Input => Some(LayoutSize::new(
                explicit_width.max(width_attr.unwrap_or(220)),
                explicit_height.max(height_attr.unwrap_or(36)),
            )),
            ElementKind::Button => {
                let child_text = self
                    .first_child()
                    .and_then(|child| match child.borrow().node_kind() {
                        NodeKind::Text(ref text) => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "Button".to_string());
                Some(LayoutSize::new(
                    explicit_width
                        .max(measure_text_width(&child_text, self.style.font_size()) + 28),
                    explicit_height.max(36),
                ))
            }
            _ => None,
        }
    }

    fn resolved_width(&self, parent_size: LayoutSize) -> i64 {
        if let Some(ratio) = self.style.width_ratio() {
            return edge_to_i64(parent_size.width() as f64 * ratio);
        }
        edge_to_i64(self.style.width())
    }

    fn resolved_height(&self, parent_size: LayoutSize) -> i64 {
        if let Some(ratio) = self.style.height_ratio() {
            return edge_to_i64(parent_size.height() as f64 * ratio);
        }
        edge_to_i64(self.style.height())
    }

    pub fn paint(&mut self) -> Vec<DisplayItem> {
        if self.style.display() == DisplayType::DisplayNone {
            return vec![];
        }

        match self.kind {
            LayoutObjectKind::Block => {
                if let NodeKind::Element(_) = self.node_kind() {
                    if self.size.width() > 0 && self.size.height() > 0 {
                        // <caption> children render outside (above) the table border box.
                        // Offset the DrawRect so the border starts below the caption area.
                        // CSS 2.2 §17.4: caption-side:top means the caption is placed
                        // above the table's border/padding/cell area.
                        let caption_h: i64 = if self.is_table() {
                            let mut total = 0i64;
                            let mut child = self.first_child();
                            while let Some(c) = child {
                                if c.borrow().element_kind() == Some(ElementKind::Caption) {
                                    let cb = c.borrow();
                                    let cm = compute_box_model_metrics(&cb.style());
                                    total += cb.size().height()
                                        + cm.margin.top
                                        + cm.margin.bottom;
                                } else if c.borrow().is_table_row() || c.borrow().is_row_group() {
                                    break;
                                }
                                let next = c.borrow().next_sibling();
                                child = next;
                            }
                            total
                        } else {
                            0
                        };
                        let rect_y = self.point().y() + caption_h;
                        let rect_h = self.size().height() - caption_h;
                        if rect_h <= 0 {
                            return vec![];
                        }
                        // Capture the element's `id` attribute so the adapter
                        // can resolve URL fragment anchors to a scroll offset.
                        // Spec: HTML Living Standard §7.4 — navigating to a
                        // fragment identifier within a document.
                        // https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
                        let anchor_id = self.element_attribute("id");
                        return vec![DisplayItem::Rect {
                            style: self.style(),
                            layout_point: LayoutPoint::new(self.point().x(), rect_y),
                            layout_size: LayoutSize::new(self.size().width(), rect_h),
                            paint_order: PaintOrder {
                                stacking_context: if self.style.position() != PositionType::Static {
                                    1
                                } else {
                                    0
                                },
                                z_index: self.style.z_index(),
                            },
                            clip_rect: if self.style.overflow_clip() {
                                Some(ClipRect {
                                    x: self.point().x(),
                                    y: rect_y,
                                    width: self.size().width(),
                                    height: rect_h,
                                })
                            } else {
                                None
                            },
                            anchor_id,
                        }];
                    }
                }
            }
            LayoutObjectKind::Inline => {
                if let NodeKind::Element(_) = self.node_kind() {
                    let mut items = Vec::new();
                    if self.size.width() > 0 && self.size.height() > 0 {
                        let anchor_id = self.element_attribute("id");
                        items.push(DisplayItem::Rect {
                            style: self.style(),
                            layout_point: self.point(),
                            layout_size: self.size(),
                            paint_order: PaintOrder {
                                stacking_context: if self.style.position() != PositionType::Static {
                                    1
                                } else {
                                    0
                                },
                                z_index: self.style.z_index(),
                            },
                            clip_rect: if self.style.overflow_clip() {
                                Some(ClipRect {
                                    x: self.point().x(),
                                    y: self.point().y(),
                                    width: self.size().width(),
                                    height: self.size().height(),
                                })
                            } else {
                                None
                            },
                            anchor_id,
                        });
                    }

                    if self.element_kind() == Some(ElementKind::Img) {
                        let src = self.element_attribute("src").unwrap_or_default();
                        let alt = self.element_attribute("alt").unwrap_or_default();
                        items.push(DisplayItem::Image {
                            src,
                            alt,
                            layout_point: self.point(),
                            layout_size: self.size(),
                            style: self.style(),
                            href: self.link_href(),
                            target: self.link_target(),
                            paint_order: PaintOrder {
                                stacking_context: if self.style.position() != PositionType::Static {
                                    1
                                } else {
                                    0
                                },
                                z_index: self.style.z_index(),
                            },
                            clip_rect: if self.style.overflow_clip() {
                                Some(ClipRect {
                                    x: self.point().x(),
                                    y: self.point().y(),
                                    width: self.size().width(),
                                    height: self.size().height(),
                                })
                            } else {
                                // Clip to ancestor cell so oversized images don't
                                // overflow their cell boundary.
                                self.nearest_ancestor_cell().map(|cell| {
                                    let cb = cell.borrow();
                                    ClipRect {
                                        x: cb.point().x(),
                                        y: cb.point().y(),
                                        width: cb.size().width(),
                                        height: cb.size().height(),
                                    }
                                })
                            },
                        });
                    } else if let Some(text) = self.placeholder_text() {
                        items.push(DisplayItem::Text {
                            text,
                            style: self.style(),
                            layout_point: LayoutPoint::new(
                                self.point().x() + 10,
                                self.point().y() + 10,
                            ),
                            href: self.link_href(),
                            target: self.link_target(),
                            paint_order: PaintOrder {
                                stacking_context: if self.style.position() != PositionType::Static {
                                    1
                                } else {
                                    0
                                },
                                z_index: self.style.z_index(),
                            },
                            clip_rect: None,
                            bold: self.style.is_bold(),
                        });
                    }

                    if !items.is_empty() {
                        return items;
                    }
                }
            }
            LayoutObjectKind::Text => {
                if let NodeKind::Text(t) = self.node_kind() {
                    let mut v = vec![];
                    let ratio = font_ratio(self.style.font_size());
                    let plain_text = t
                        .replace("\n", " ")
                        .split(' ')
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    // Use the max_width that was established during compute_size so
                    // that the line-break boundaries are identical between the sizing
                    // and painting passes.  Recomputing against self.size().width()
                    // here would produce narrower wrapping because size.width() is
                    // the width of the widest *result* line, not the available
                    // container width.
                    // Spec: CSS2.2 §9.4.2 — inline formatting context, line boxes.
                    // https://www.w3.org/TR/CSS22/visuren.html#inline-formatting
                    // Prefer the ancestor cell's current content width so that
                    // text wrapping reflects the post-equalization cell width
                    // rather than the stale cached value from compute_size.
                    let max_width = self.nearest_ancestor_cell()
                        .map(|cell| {
                            let cb = cell.borrow();
                            let cm = compute_box_model_metrics(&cb.style);
                            (cb.size().width() - cm.inner_horizontal()).max(CHAR_WIDTH * ratio)
                        })
                        .unwrap_or_else(|| {
                            if self.text_line_max_width > 0 {
                                self.text_line_max_width
                            } else {
                                self.size().width().max(CHAR_WIDTH * ratio)
                            }
                        });
                    let lines = split_text(plain_text, CHAR_WIDTH * ratio, max_width);
                    let href = self.link_href();
                    let target = self.link_target();

                    let bold = self.style.is_bold();
                    for (i, line) in lines.into_iter().enumerate() {
                        let item = DisplayItem::Text {
                            text: line,
                            style: self.style(),
                            layout_point: LayoutPoint::new(
                                self.point().x(),
                                self.point().y() + CHAR_HEIGHT_WITH_PADDING * ratio * i as i64,
                            ),
                            href: href.clone(),
                            target: target.clone(),
                            paint_order: PaintOrder {
                                stacking_context: if self.style.position() != PositionType::Static {
                                    1
                                } else {
                                    0
                                },
                                z_index: self.style.z_index(),
                            },
                            clip_rect: None,
                            bold,
                        };
                        v.push(item);
                    }

                    return v;
                }
            }
        }

        vec![]
    }

    pub fn compute_size(&mut self, parent_size: LayoutSize) {
        let mut size = LayoutSize::new(0, 0);
        let metrics = compute_box_model_metrics(&self.style);

        match self.kind() {
            LayoutObjectKind::Block => {
                let available_width = (parent_size.width() - metrics.outer_horizontal()).max(0);
                let explicit_width = self.resolved_width(parent_size);
                // Also check HTML width attribute for block elements (tables, etc.).
                let html_width = parse_dimension_attr(self.element_attribute("width"));

                // Table cells: use width attribute, or allocate remaining width
                // after subtracting explicitly-sized sibling cells.
                // When total explicit widths exceed available space, scale
                // proportionally to fit.
                let content_width = if self.is_table_cell() {
                    // Resolve HTML width attribute — percentage values are computed
                    // against available_width (the parent row's inner width).
                    let attr_width = parse_dimension_pct_attr(
                        self.element_attribute("width"),
                        Some(available_width),
                    );
                    if let Some(w) = attr_width {
                        let rowspan_offset = self
                            .parent
                            .upgrade()
                            .map(|p| p.borrow().rowspan_column_offset())
                            .unwrap_or(0);
                        let effective = (available_width - rowspan_offset).max(0);
                        let total_explicit = self.total_sibling_explicit_widths();
                        if total_explicit > effective && total_explicit > 0 {
                            // Scale down proportionally.
                            (w * effective / total_explicit).max(0)
                        } else {
                            w.min(effective)
                        }
                    } else if explicit_width > 0 {
                        explicit_width.min(available_width)
                    } else {
                        self.table_cell_auto_width(available_width)
                    }
                } else if let Some(FlexDirection::Row) = self.parent_flex_direction() {
                    // Flex item on a row: shrink-to-fit its content (honoring an
                    // explicit width) so siblings sit side by side instead of
                    // each filling the container width like a normal block.
                    let w = if explicit_width > 0 {
                        explicit_width
                    } else if let Some(hw) = html_width {
                        hw
                    } else {
                        self.max_content_width()
                    };
                    w.min(available_width).max(0)
                } else if explicit_width > 0 {
                    explicit_width.min(available_width)
                } else if let Some(w) = html_width {
                    w.min(available_width)
                } else if self.element_kind() == Some(ElementKind::Table) {
                    // Tables without explicit width use shrink-to-fit: the width
                    // of the widest row (sum of cell widths), capped at available.
                    let measure_row_width = |r: &Rc<RefCell<LayoutObject>>| -> i64 {
                        let mut row_width: i64 = 0;
                        let mut cell = r.borrow().first_child();
                        while let Some(c) = cell {
                            row_width += c.borrow().size.width();
                            let next = c.borrow().next_sibling();
                            cell = next;
                        }
                        row_width
                    };
                    let mut max_row_width: i64 = 0;
                    let mut child = self.first_child();
                    while let Some(c) = child {
                        if c.borrow().is_table_row() {
                            max_row_width = max_row_width.max(measure_row_width(&c));
                        } else if c.borrow().is_row_group() {
                            let mut row = c.borrow().first_child();
                            while let Some(r) = row {
                                if r.borrow().is_table_row() {
                                    max_row_width = max_row_width.max(measure_row_width(&r));
                                }
                                let next = r.borrow().next_sibling();
                                row = next;
                            }
                        }
                        let next = c.borrow().next_sibling();
                        child = next;
                    }
                    if max_row_width > 0 {
                        max_row_width.min(available_width)
                    } else {
                        available_width
                    }
                } else {
                    available_width
                };

                let mut content_height = 0;
                let mut child = self.first_child();
                let is_row = self.is_table_row();
                // Flex row: items are placed side by side, so the container's
                // height is the tallest item (max), not the sum.
                let is_flex_row = self.is_flex_container()
                    && self.style.flex_direction() == FlexDirection::Row;
                let mut previous_child_kind = LayoutObjectKind::Block;
                while child.is_some() {
                    let c = child.expect("first child should exist");
                    let c_kind = c.borrow().kind();
                    if is_row {
                        // Table row: height = max of cell heights,
                        // but skip cells with rowspan > 1 (they span multiple rows).
                        let rowspan = parse_dimension_attr(
                            c.borrow().element_attribute("rowspan"),
                        )
                        .unwrap_or(1);
                        if rowspan <= 1 {
                            content_height = content_height.max(c.borrow().size.height());
                        }
                    } else if is_flex_row {
                        let c_metrics = compute_box_model_metrics(&c.borrow().style());
                        content_height = content_height.max(
                            c.borrow().size.height()
                                + c_metrics.margin.top
                                + c_metrics.margin.bottom,
                        );
                    } else if previous_child_kind.normal_flow_spec().stacks_vertically
                        || c_kind.normal_flow_spec().stacks_vertically
                    {
                        // Include the child's vertical margin so that the parent
                        // is tall enough to contain the child even when it has
                        // margin-top/bottom that shift it downward (e.g. <hr>,
                        // <h1>…<h3>, <p>).  This matches the CSS2 block height
                        // algorithm (§10.6.3) where margin participates in height.
                        let c_metrics = compute_box_model_metrics(&c.borrow().style());
                        content_height += c.borrow().size.height()
                            + c_metrics.margin.top
                            + c_metrics.margin.bottom;
                    } else {
                        content_height = content_height.max(c.borrow().size.height());
                    }
                    previous_child_kind = c_kind;
                    child = c.borrow().next_sibling();
                }

                // <br> and <hr> have intrinsic heights even without children.
                let content_height = if self.element_kind() == Some(ElementKind::Br) {
                    let ratio = font_ratio(self.style.font_size());
                    CHAR_HEIGHT_WITH_PADDING * ratio
                } else if self.element_kind() == Some(ElementKind::Hr) {
                    // <hr> renders as a 2px line with 8px margin above/below.
                    2
                } else {
                    let explicit_height = self.resolved_height(parent_size);
                    if explicit_height > 0 {
                        explicit_height
                    } else {
                        content_height.max(0)
                    }
                };
                // For table cells, add cellpadding to both dimensions so that the
                // cell's outer box includes the padding space on all four sides.
                // content_origin() and content_size() subtract/add cp so that
                // children are inset by cp pixels inside the cell.
                let cp = if self.is_table_cell() { self.ancestor_table_cellpadding() } else { 0 };
                size.set_width((content_width + metrics.inner_horizontal()).max(0));
                size.set_height((content_height + metrics.inner_vertical() + 2 * cp).max(0));
            }
            LayoutObjectKind::Inline => {
                if let Some(intrinsic) = self.intrinsic_inline_size(parent_size) {
                    size.set_width((intrinsic.width() + metrics.inner_horizontal()).max(0));
                    size.set_height((intrinsic.height() + metrics.inner_vertical()).max(0));
                } else {
                    // Track per-line width separately and use the maximum across
                    // all lines as the inline element's box width.  Summing widths
                    // across block children (like <br>) produced an inflated width
                    // that caused text-align centering to shift multi-line text
                    // (e.g. <strong>line1<br>line2</strong>) off-screen.
                    let mut max_line_width: i64 = 0;
                    let mut current_line_width: i64 = 0;
                    let mut current_line_height: i64 = 0;
                    let mut content_height: i64 = 0;
                    let mut child = self.first_child();
                    while child.is_some() {
                        let c = child.expect("child should exist");
                        let c_kind = c.borrow().kind();
                        let c_h = c.borrow().size.height();
                        let c_w = c.borrow().size.width();
                        if c_kind.normal_flow_spec().stacks_vertically {
                            // Block child (e.g. <br>): flush the current inline
                            // line, then add the block child's own height.
                            // compute_position places the block immediately
                            // BELOW the preceding inline content (next sibling
                            // y = prev.y + prev.height), so the parent inline's
                            // content box must contain BOTH the line and the
                            // block — sum, don't max.
                            max_line_width = max_line_width.max(current_line_width);
                            current_line_width = 0;
                            content_height += current_line_height + c_h;
                            current_line_height = 0;
                        } else {
                            current_line_width += c_w;
                            current_line_height = current_line_height.max(c_h);
                        }
                        child = c.borrow().next_sibling();
                    }
                    // Flush any remaining inline content on the last line.
                    max_line_width = max_line_width.max(current_line_width);
                    content_height += current_line_height;

                    size.set_width((max_line_width + metrics.inner_horizontal()).max(0));
                    size.set_height((content_height + metrics.inner_vertical()).max(0));
                }
            }
            LayoutObjectKind::Text => {
                if let NodeKind::Text(t) = self.node_kind() {
                    let ratio = font_ratio(self.style.font_size());
                    let plain_text = t
                        .replace("\n", " ")
                        .split(' ')
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ");
                    // max_width is the available horizontal space for this text
                    // node within its containing block.  Use the nearest block/cell
                    // ancestor's content width so that inline parents (e.g. <a>,
                    // <strong>) don't artificially narrow the wrapping boundary.
                    // Spec: CSS2.2 §10.3.3 — available width in a block
                    // formatting context.
                    // https://www.w3.org/TR/CSS22/visudet.html#blockwidth
                    let max_width = self.nearest_block_ancestor_width()
                        .map(|w| (w - metrics.outer_horizontal()).max(CHAR_WIDTH * ratio))
                        .unwrap_or_else(||
                            (parent_size.width() - metrics.outer_horizontal()).max(CHAR_WIDTH * ratio)
                        );
                    // Cache so paint() uses the identical boundary (see paint Text arm).
                    self.text_line_max_width = max_width;
                    let lines = split_text(plain_text.clone(), CHAR_WIDTH * ratio, max_width);
                    let width = lines
                        .iter()
                        .map(|line| estimate_text_width_chars(line) * CHAR_WIDTH * ratio)
                        .max()
                        .unwrap_or(0);
                    let height = if lines.is_empty() {
                        0
                    } else {
                        CHAR_HEIGHT_WITH_PADDING * ratio * lines.len() as i64
                    };
                    size.set_width((width + metrics.inner_horizontal()).max(0));
                    size.set_height((height + metrics.inner_vertical()).max(0));
                }
            }
        }

        self.size = size;
    }

    pub fn compute_position(
        &mut self,
        parent_point: LayoutPoint,
        parent_size: LayoutSize,
        previous_sibling_kind: LayoutObjectKind,
        previous_sibling_point: Option<LayoutPoint>,
        previous_sibling_size: Option<LayoutSize>,
    ) {
        let mut point = LayoutPoint::new(0, 0);
        let metrics = compute_box_model_metrics(&self.style);

        // Table cells: position horizontally within the row.
        if self.is_table_cell() {
            // Vertical alignment within the row.
            // Honour explicit valign="middle"/"bottom"/"top"; fall back to
            // auto-centering for cells whose content is purely inline/text
            // (e.g. bullet-marker cells like ●), which matches browser behaviour
            // of baseline-aligning small inline cells within a taller row.
            let valign = self
                .element_attribute("valign")
                .map(|v| v.to_lowercase())
                .unwrap_or_default();
            let v_offset = if valign == "middle" || (valign.is_empty() && self.has_only_inline_children()) {
                (parent_size.height() - self.size.height()).max(0) / 2
            } else if valign == "bottom" {
                (parent_size.height() - self.size.height()).max(0)
            } else {
                0
            };

            // Horizontal cellspacing: add a gap between adjacent cells so that
            // their borders don't visually merge into a double line.
            // Spec: HTML4.01 §11.3.3 — cellspacing default is 2.
            let cs = self.ancestor_table_cellspacing();
            if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                point.set_x(pos.x() + size.width() + metrics.margin.left + cs);
                point.set_y(parent_point.y() + metrics.margin.top + v_offset);
            } else {
                // First cell in row: apply rowspan offset from previous rows.
                let rowspan_offset = self
                    .parent
                    .upgrade()
                    .map(|p| p.borrow().rowspan_column_offset())
                    .unwrap_or(0);
                point.set_x(parent_point.x() + rowspan_offset + metrics.margin.left + cs);
                point.set_y(parent_point.y() + metrics.margin.top + v_offset);
            }
        } else if let Some(dir) = self.parent_flex_direction() {
            // Flex item placement (main-axis start packing; no grow/shrink yet).
            match dir {
                FlexDirection::Row => {
                    // Lay out to the right of the previous item, aligned to the
                    // container's top (align-items: flex-start).
                    if let (Some(size), Some(pos)) =
                        (previous_sibling_size, previous_sibling_point)
                    {
                        point.set_x(pos.x() + size.width() + metrics.margin.left);
                    } else {
                        point.set_x(parent_point.x() + metrics.margin.left);
                    }
                    point.set_y(parent_point.y() + metrics.margin.top);
                }
                FlexDirection::Column => {
                    // Stack vertically like a block formatting context.
                    if let (Some(size), Some(pos)) =
                        (previous_sibling_size, previous_sibling_point)
                    {
                        point.set_y(
                            pos.y()
                                + size.height()
                                + metrics.margin.top
                                + metrics.margin.bottom,
                        );
                    } else {
                        point.set_y(parent_point.y() + metrics.margin.top);
                    }
                    point.set_x(parent_point.x() + metrics.margin.left);
                }
            }
        } else {
        match (
            self.kind().normal_flow_spec().flow,
            previous_sibling_kind.normal_flow_spec().flow,
        ) {
            (LayoutFlow::BlockFormattingContext, _) | (_, LayoutFlow::BlockFormattingContext) => {
                if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                    point.set_y(pos.y() + size.height() + metrics.margin.top + metrics.margin.bottom);
                } else {
                    point.set_y(parent_point.y() + metrics.margin.top);
                }

                // Ref: CSS 2.1 §10.3.3, auto horizontal margins center a block when width is known.
                // https://www.w3.org/TR/CSS21/visudet.html#blockwidth
                let available_width = parent_size.width() - self.size.width();
                if self.style.margin_horizontal_auto() && available_width > 0 {
                    point.set_x(parent_point.x() + available_width / 2);
                } else if self.style.margin_left_auto() && available_width > metrics.margin.right {
                    point.set_x(parent_point.x() + available_width - metrics.margin.right);
                } else if !self.kind().normal_flow_spec().stacks_vertically
                    && self.style.text_align() == TextAlign::Center
                    && available_width > 0
                {
                    // Inline/text node after a block: apply text-align centering.
                    point.set_x(parent_point.x() + available_width / 2);
                } else {
                    point.set_x(parent_point.x() + metrics.margin.left);
                }
            }
            (LayoutFlow::InlineFlow, LayoutFlow::InlineFlow) => {
                if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                    // Candidate X if we place this element right after the previous one.
                    let candidate_x = pos.x() + size.width() + metrics.margin.left;
                    let right_edge = parent_point.x() + parent_size.width();
                    if parent_size.width() > 0 && candidate_x + self.size.width() > right_edge {
                        // This inline element does not fit on the current line;
                        // wrap it to the start of the next line.
                        // Spec: CSS2.2 §9.4.2 — when an inline box doesn't fit on
                        // the current line box it wraps to a new line box below.
                        // https://www.w3.org/TR/CSS22/visuren.html#inline-formatting
                        point.set_x(parent_point.x() + metrics.margin.left);
                        point.set_y(pos.y() + size.height() + metrics.margin.top);
                    } else {
                        point.set_x(candidate_x);
                        point.set_y(pos.y() + metrics.margin.top);
                    }
                } else {
                    // First inline child: apply text-align centering if set.
                    match self.style.text_align() {
                        TextAlign::Center => {
                            let available = parent_size.width() - self.size.width();
                            if available > 0 {
                                point.set_x(parent_point.x() + available / 2);
                            } else {
                                point.set_x(parent_point.x() + metrics.margin.left);
                            }
                        }
                        TextAlign::Right => {
                            let available = parent_size.width() - self.size.width();
                            if available > 0 {
                                point.set_x(parent_point.x() + available);
                            } else {
                                point.set_x(parent_point.x() + metrics.margin.left);
                            }
                        }
                        TextAlign::Left => {
                            point.set_x(parent_point.x() + metrics.margin.left);
                        }
                    }
                    point.set_y(parent_point.y() + metrics.margin.top);
                }
            }
        }
        } // end if !table_cell

        match self.style.position() {
            PositionType::Static => {}
            PositionType::Relative => {
                point.set_x(point.x() + edge_to_i64(self.style.offset_left()));
                point.set_y(point.y() + edge_to_i64(self.style.offset_top()));
            }
            PositionType::Absolute => {
                point.set_x(parent_point.x() + edge_to_i64(self.style.offset_left()));
                point.set_y(parent_point.y() + edge_to_i64(self.style.offset_top()));
            }
        }

        self.point = point;
    }

    pub fn is_node_selected(&self, selector: &Selector) -> bool {
        match &self.node_kind() {
            NodeKind::Element(e) => match selector {
                Selector::TypeSelector(type_name) => e.kind().to_string() == *type_name,
                Selector::ClassSelector(class_name) => e
                    .attributes()
                    .iter()
                    .any(|attr| attr.name() == "class" && attr.value() == *class_name),
                Selector::IdSelector(id_name) => e
                    .attributes()
                    .iter()
                    .any(|attr| attr.name() == "id" && attr.value() == *id_name),
                Selector::UnknownSelector => false,
            },
            _ => false,
        }
    }

    pub fn cascading_style(&mut self, declarations: Vec<Declaration>) {
        for declaration in declarations {
            let first_value = declaration.first_value();
            match declaration.property.as_str() {
                "background-color" | "background" => match first_value {
                    Some(ComponentValue::Ident(value)) => {
                        let color = Color::from_name(value).unwrap_or_else(|_| Color::white());
                        self.style.set_background_color(color);
                    }
                    Some(ComponentValue::HashToken(color_code)) => {
                        let color = Color::from_code(color_code).unwrap_or_else(|_| Color::white());
                        self.style.set_background_color(color);
                    }
                    _ => {}
                },
                "color" => match first_value {
                    Some(ComponentValue::Ident(value)) => {
                        let color = Color::from_name(value).unwrap_or_else(|_| Color::black());
                        self.style.set_color(color);
                    }
                    Some(ComponentValue::HashToken(color_code)) => {
                        let color = Color::from_code(color_code).unwrap_or_else(|_| Color::black());
                        self.style.set_color(color);
                    }
                    _ => {}
                },
                "display" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        let display_type =
                            DisplayType::from_str(value).unwrap_or(DisplayType::Block);
                        self.style.set_display(display_type)
                    }
                }
                "flex-direction" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style.set_flex_direction(FlexDirection::from_str(value));
                    }
                }
                "width" => match first_value {
                    Some(ComponentValue::Number(value)) => {
                        self.style.set_width(*value);
                    }
                    Some(ComponentValue::Dimension(value, unit)) => match unit.as_str() {
                        "vw" => self.style.set_width_ratio(*value / 100.0),
                        "px" | "em" | "rem" => {
                            if let Some(px) = length_to_px(*value, unit, FontSize::Medium) {
                                self.style.set_width(px);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                },
                "height" => match first_value {
                    Some(ComponentValue::Number(value)) => {
                        self.style.set_height(*value);
                    }
                    Some(ComponentValue::Dimension(value, unit)) => match unit.as_str() {
                        "vh" => self.style.set_height_ratio(*value / 100.0),
                        "px" | "em" | "rem" => {
                            if let Some(px) = length_to_px(*value, unit, FontSize::Medium) {
                                self.style.set_height(px);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                },
                "position" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        let position =
                            PositionType::from_str(value).unwrap_or(PositionType::Static);
                        self.style.set_position(position);
                    }
                }
                "top" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_offset_top(*value),
                    Some(ComponentValue::Dimension(value, unit)) if unit == "px" => {
                        self.style.set_offset_top(*value)
                    }
                    _ => {}
                },
                "left" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_offset_left(*value),
                    Some(ComponentValue::Dimension(value, unit)) if unit == "px" => {
                        self.style.set_offset_left(*value)
                    }
                    _ => {}
                },
                "z-index" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_z_index(*value as i32),
                    _ => {}
                },
                "overflow" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style
                            .set_overflow_clip(value == "hidden" || value == "clip");
                    }
                }
                "margin" => {
                    let base_font_size = self.style.font_size_or_default();
                    if let Some((top, right, bottom, left)) =
                        parse_margin_shorthand(&declaration.value, base_font_size)
                    {
                        // Spec: CSS initial margin is 0, so when cascade runs before defaulting, fallback to 0.
                        // https://www.w3.org/TR/CSS22/box.html#margin-properties
                        let current = self.style.margin_or_default();
                        self.style.set_margin(
                            crate::renderer::layout::computed_style::EdgeSize::from_values(
                                top.unwrap_or(current.top()),
                                right.unwrap_or(current.right()),
                                bottom.unwrap_or(current.bottom()),
                                left.unwrap_or(current.left()),
                            ),
                        );
                    }
                    let (left_auto, right_auto) = parse_margin_auto_flags(&declaration.value);
                    self.style.set_margin_left_auto(left_auto);
                    self.style.set_margin_right_auto(right_auto);
                }
                "padding" => {
                    let base_font_size = self.style.font_size_or_default();
                    if let Some((top, right, bottom, left)) =
                        parse_spacing_shorthand(&declaration.value, base_font_size)
                    {
                        self.style.set_padding(
                            crate::renderer::layout::computed_style::EdgeSize::from_values(
                                top, right, bottom, left,
                            ),
                        );
                    }
                }
                "border" | "border-width" => {
                    let base_font_size = self.style.font_size_or_default();
                    if let Some((top, right, bottom, left)) =
                        parse_spacing_shorthand(&declaration.value, base_font_size)
                    {
                        self.style.set_border(
                            crate::renderer::layout::computed_style::EdgeSize::from_values(
                                top, right, bottom, left,
                            ),
                        );
                    }
                }
                "opacity" => {
                    if let Some(ComponentValue::Number(value)) = first_value {
                        self.style.set_opacity(*value);
                    }
                }
                "font-family" => {
                    if let Some(font_family) = first_font_family(&declaration.value) {
                        self.style.set_font_family(font_family);
                    }
                }
                "font-size" => match first_value {
                    Some(ComponentValue::Ident(value)) => {
                        if let Ok(font_size) = FontSize::from_str(value) {
                            self.style.set_font_size(font_size);
                        }
                    }
                    Some(ComponentValue::Number(value)) => {
                        self.style.set_font_size(FontSize::from_px(*value));
                    }
                    Some(ComponentValue::Dimension(value, unit)) => {
                        if let Some(px) = length_to_px(*value, unit, FontSize::Medium) {
                            self.style.set_font_size(FontSize::from_px(px));
                        }
                    }
                    _ => {}
                },
                "text-decoration" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        if let Ok(decoration) = TextDecoration::from_str(value) {
                            self.style.set_text_decoration(decoration);
                        }
                    }
                }
                "text-align" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        match value.as_str() {
                            "center" => self.style.set_text_align(TextAlign::Center),
                            "right" => self.style.set_text_align(TextAlign::Right),
                            "left" => self.style.set_text_align(TextAlign::Left),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub fn defaulting_style(
        &mut self,
        node: &Rc<RefCell<Node>>,
        parent_style: Option<ComputedStyle>,
    ) {
        self.style.defaulting(node, parent_style);
    }

    pub fn update_kind(&mut self) {
        match self.node_kind() {
            NodeKind::Document => panic!("should not create a layout object for a document node"),
            NodeKind::Element(_) => match self.style.display() {
                // A flex container is itself a block-level box; flex affects how
                // its children are sized and positioned, not its own outer flow.
                DisplayType::Block | DisplayType::Flex => self.kind = LayoutObjectKind::Block,
                DisplayType::Inline => self.kind = LayoutObjectKind::Inline,
                DisplayType::DisplayNone => {
                    panic!("should not create a layout object for a node with display:none")
                }
            },
            NodeKind::Text(_) => self.kind = LayoutObjectKind::Text,
        }
    }

    pub fn kind(&self) -> LayoutObjectKind {
        self.kind
    }

    pub fn node_kind(&self) -> NodeKind {
        self.node.borrow().kind().clone()
    }

    pub fn set_first_child(&mut self, first_child: Option<Rc<RefCell<LayoutObject>>>) {
        self.first_child = first_child;
    }

    pub fn first_child(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        self.first_child.as_ref().cloned()
    }

    pub fn set_next_sibling(&mut self, next_sibling: Option<Rc<RefCell<LayoutObject>>>) {
        self.next_sibling = next_sibling;
    }

    pub fn next_sibling(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        self.next_sibling.as_ref().cloned()
    }

    pub fn parent(&self) -> Weak<RefCell<Self>> {
        self.parent.clone()
    }

    pub fn style(&self) -> ComputedStyle {
        self.style.clone()
    }

    pub fn point(&self) -> LayoutPoint {
        self.point
    }

    pub fn size(&self) -> LayoutSize {
        self.size
    }

    /// Return the HTML `cellspacing` value from the nearest ancestor `<table>`.
    ///
    /// Walks up at most a few layout-tree steps (td/tr → table) and reads the
    /// table's `cellspacing` attribute.  Returns the HTML4 default of 2 when no
    /// attribute is present.  Returns 0 when no TABLE ancestor is found.
    ///
    /// Spec: HTML4.01 §11.3.3 — cellspacing default is 2.
    /// https://www.w3.org/TR/html4/struct/tables.html#adef-cellspacing
    fn ancestor_table_cellspacing(&self) -> i64 {
        let mut current = self.parent.upgrade();
        let mut steps = 0;
        while let Some(ancestor) = current {
            steps += 1;
            if steps > 5 {
                break;
            }
            let b = ancestor.borrow();
            if b.is_table() {
                return b
                    .element_attribute("cellspacing")
                    .and_then(|v| v.trim().parse::<i64>().ok())
                    .unwrap_or(2); // HTML4 default
            }
            let next = b.parent.upgrade();
            drop(b);
            current = next;
        }
        0
    }

    /// Return the HTML `cellpadding` value from the nearest ancestor `<table>`.
    ///
    /// Walks up at most a few layout-tree steps (td → tr → optional tbody → table)
    /// and reads the table's `cellpadding` attribute.  Returns the HTML4 default
    /// of 1 when no attribute is present, and 0 when the attribute is explicitly "0".
    /// Returns 0 when called on a non-cell element.
    ///
    /// Spec: HTML4.01 §11.3.2 — cellpadding default is 1.
    /// https://www.w3.org/TR/html4/struct/tables.html#adef-cellpadding
    fn ancestor_table_cellpadding(&self) -> i64 {
        let mut current = self.parent.upgrade();
        let mut steps = 0;
        while let Some(ancestor) = current {
            steps += 1;
            if steps > 5 {
                break;
            }
            let b = ancestor.borrow();
            if b.is_table() {
                return b
                    .element_attribute("cellpadding")
                    .and_then(|v| v.trim().parse::<i64>().ok())
                    .unwrap_or(1); // HTML4 default
            }
            let next = b.parent.upgrade();
            drop(b);
            current = next;
        }
        0
    }

    pub fn content_origin(&self) -> LayoutPoint {
        let metrics = compute_box_model_metrics(&self.style);
        let cp = if self.is_table_cell() {
            self.ancestor_table_cellpadding()
        } else {
            0
        };
        LayoutPoint::new(
            self.point.x() + metrics.padding.left + metrics.border.left + cp,
            self.point.y() + metrics.padding.top + metrics.border.top + cp,
        )
    }

    pub fn content_size(&self) -> LayoutSize {
        let metrics = compute_box_model_metrics(&self.style);
        let cp = if self.is_table_cell() {
            self.ancestor_table_cellpadding()
        } else {
            0
        };
        LayoutSize::new(
            (self.size.width() - metrics.inner_horizontal() - 2 * cp).max(0),
            (self.size.height() - metrics.inner_vertical() - 2 * cp).max(0),
        )
    }

    /// Returns the total box model overhead (border + padding) in the vertical axis.
    pub fn vertical_overhead(&self) -> i64 {
        let metrics = compute_box_model_metrics(&self.style);
        metrics.inner_vertical()
    }

    /// Directly overrides the computed height. Used by the rowspan height
    /// expansion pass to expand rows that are constrained by rowspan cells.
    pub fn force_set_height(&mut self, h: i64) {
        self.size.set_height(h);
    }

    /// Directly overrides the computed width. Used by the column width
    /// equalization pass to align cell borders across all rows.
    pub fn force_set_width(&mut self, w: i64) {
        self.size.set_width(w);
    }

    /// Returns true if this layout object corresponds to a `<table>` element.
    pub fn is_table(&self) -> bool {
        self.element_kind() == Some(ElementKind::Table)
    }

}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct LayoutPoint {
    x: i64,
    y: i64,
}

impl LayoutPoint {
    pub fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }

    pub fn x(&self) -> i64 {
        self.x
    }

    pub fn y(&self) -> i64 {
        self.y
    }

    pub fn set_x(&mut self, x: i64) {
        self.x = x;
    }

    pub fn set_y(&mut self, y: i64) {
        self.y = y;
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct LayoutSize {
    width: i64,
    height: i64,
}

impl LayoutSize {
    pub fn new(width: i64, height: i64) -> Self {
        Self { width, height }
    }

    pub fn width(&self) -> i64 {
        self.width
    }

    pub fn height(&self) -> i64 {
        self.height
    }

    pub fn set_width(&mut self, width: i64) {
        self.width = width;
    }

    pub fn set_height(&mut self, height: i64) {
        self.height = height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::layout::computed_style::EdgeSize;

    #[test]
    fn compute_box_model_metrics_includes_margin_padding_border() {
        let mut style = ComputedStyle::new();
        style.set_margin(EdgeSize::from_values(4.0, 6.0, 8.0, 10.0));
        style.set_padding(EdgeSize::from_values(1.0, 2.0, 3.0, 4.0));
        style.set_border(EdgeSize::from_values(2.0, 2.0, 2.0, 2.0));

        let metrics = compute_box_model_metrics(&style);

        assert_eq!(metrics.outer_horizontal(), 26);
        assert_eq!(metrics.outer_vertical(), 20);
        assert_eq!(metrics.inner_horizontal(), 10);
        assert_eq!(metrics.inner_vertical(), 8);
    }

    #[test]
    fn normal_flow_spec_maps_block_and_inline() {
        assert_eq!(
            LayoutObjectKind::Block.normal_flow_spec(),
            NormalFlowSpec {
                flow: LayoutFlow::BlockFormattingContext,
                stacks_vertically: true,
                keeps_inline_line: false,
            }
        );
        assert_eq!(LayoutObjectKind::Inline.normal_flow_spec().flow, LayoutFlow::InlineFlow);
        assert_eq!(LayoutObjectKind::Text.normal_flow_spec().flow, LayoutFlow::InlineFlow);
    }
}
