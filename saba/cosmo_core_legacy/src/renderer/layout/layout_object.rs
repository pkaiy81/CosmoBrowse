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
/// Find a byte index suitable for line breaking at or before `max_char_index`
/// characters. Returns a byte offset safe for `str::split_at`.
fn find_byte_index_for_line_break(line: &str, max_char_index: usize) -> usize {
    let char_indices: Vec<(usize, char)> = line.char_indices().collect();
    let upper = max_char_index.min(char_indices.len().saturating_sub(1));
    // Prefer breaking at a space.
    for i in (0..=upper).rev() {
        if char_indices[i].1 == ' ' || char_indices[i].1 == '\u{3000}' {
            return char_indices[i].0;
        }
    }
    // No space found; break at the character boundary.
    if upper + 1 < char_indices.len() {
        char_indices[upper + 1].0
    } else {
        line.len()
    }
}

fn split_text(line: String, char_width: i64, max_width: i64) -> Vec<String> {
    let mut result: Vec<String> = vec![];
    let safe_width = max_width.max(char_width).max(1);
    let text_width = estimate_text_width_chars(&line) * char_width;
    if text_width > safe_width {
        // Find how many characters fit within safe_width, accounting for wide chars.
        let max_width_units = (safe_width / char_width).max(1);
        let mut units = 0i64;
        let mut max_chars = 0usize;
        for c in line.chars() {
            let w = if is_wide_char(c) { 2 } else { 1 };
            if units + w > max_width_units {
                break;
            }
            units += w;
            max_chars += 1;
        }
        max_chars = max_chars.max(1);
        let split_byte = find_byte_index_for_line_break(&line, max_chars);
        let split_byte = split_byte.min(line.len());
        let (left, right) = line.split_at(split_byte);
        result.push(left.to_string());
        result.extend(split_text(right.trim().to_string(), char_width, safe_width));
    } else if !line.is_empty() {
        result.push(line);
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

    fn element_kind(&self) -> Option<ElementKind> {
        self.node.borrow().element_kind()
    }

    fn is_table_cell(&self) -> bool {
        matches!(self.element_kind(), Some(ElementKind::Td) | Some(ElementKind::Th))
    }

    fn is_table_row(&self) -> bool {
        matches!(self.element_kind(), Some(ElementKind::Tr))
    }

    fn parent_element_kind(&self) -> Option<ElementKind> {
        self.parent.upgrade()?.borrow().element_kind()
    }

    /// Scan immediate children for elements that imply a minimum width
    /// (e.g. `<img width="350">`, `<table width="256">`).
    /// Returns the maximum such hint, or 0 if none found.
    fn min_content_width_hint(&self) -> i64 {
        let mut max_hint: i64 = 0;
        let mut child = self.first_child();
        while let Some(c) = child {
            let borrowed = c.borrow();
            let hint = parse_dimension_attr(borrowed.element_attribute("width")).unwrap_or(0);
            max_hint = max_hint.max(hint);
            // Also recurse one level for nested containers.
            let mut grandchild = borrowed.first_child();
            while let Some(gc) = grandchild {
                let gc_hint =
                    parse_dimension_attr(gc.borrow().element_attribute("width")).unwrap_or(0);
                max_hint = max_hint.max(gc_hint);
                let next = gc.borrow().next_sibling();
                grandchild = next;
            }
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
    fn column_width_from_sibling_rows(&self, col_index: usize) -> Option<i64> {
        let row = self.parent.upgrade()?;
        let table = row.borrow().parent.upgrade()?;
        let mut sibling_row = table.borrow().first_child();
        while let Some(sr) = sibling_row {
            let sr_borrowed = sr.borrow();
            if sr_borrowed.is_table_row() && !Rc::ptr_eq(&sr, &row) {
                let mut idx: usize = 0;
                let mut cell = sr_borrowed.first_child();
                while let Some(c) = cell {
                    let cb = c.borrow();
                    if cb.is_table_cell() {
                        if idx == col_index {
                            if let Some(w) = parse_dimension_attr(cb.element_attribute("width")) {
                                return Some(w);
                            }
                            if cb.size.width() > 0 {
                                return Some(cb.size.width());
                            }
                        }
                        idx += 1;
                    }
                    let next = cb.next_sibling();
                    drop(cb);
                    cell = next;
                }
            }
            let next = sr_borrowed.next_sibling();
            drop(sr_borrowed);
            sibling_row = next;
        }
        None
    }

    /// Return this cell's colspan attribute value (defaults to 1).
    fn cell_colspan(&self) -> usize {
        parse_dimension_attr(self.element_attribute("colspan"))
            .unwrap_or(1)
            .max(1) as usize
    }

    /// Compute the width this cell should use, accounting for sibling cells
    /// that have explicit HTML width attributes, and rowspan cells from
    /// previous rows that reduce the available width.
    /// Auto-width cells with large intrinsic content (images, nested tables)
    /// receive at least their minimum content width before equal distribution.
    /// Also checks sibling rows for column width hints (including colspan).
    fn table_cell_auto_width(&self, available_width: i64) -> i64 {
        // Check if sibling rows have explicit widths for the columns this cell spans.
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
        if found_all && total_from_siblings > 0 {
            return total_from_siblings.min(available_width);
        }

        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return available_width,
        };
        // Reduce available width by rowspan columns from previous rows.
        let rowspan_offset = parent.borrow().rowspan_column_offset();
        let effective_width = (available_width - rowspan_offset).max(0);

        let mut total_explicit: i64 = 0;
        let mut auto_cells: Vec<(bool, i64)> = Vec::new(); // (is_self, min_hint)
        let mut self_index: usize = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            match c.try_borrow() {
                Ok(borrowed) => {
                    if borrowed.is_table_cell() {
                        if let Some(w) = parse_dimension_attr(borrowed.element_attribute("width")) {
                            total_explicit += w;
                        } else {
                            let hint = borrowed.min_content_width_hint();
                            auto_cells.push((false, hint));
                        }
                    }
                    let next = borrowed.next_sibling();
                    drop(borrowed);
                    child = next;
                }
                Err(_) => {
                    // This is self — it has no explicit width (caller already checked).
                    let hint = self.min_content_width_hint();
                    self_index = auto_cells.len();
                    auto_cells.push((true, hint));
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

        // Check if any auto cell needs more than equal share.
        let equal_share = remaining / auto_count as i64;
        let total_min: i64 = auto_cells.iter().map(|(_, h)| *h).sum();
        if total_min <= remaining && total_min > 0 {
            // Allocate min widths first, then distribute surplus equally
            // among cells whose min is below the equal share.
            let surplus = remaining - total_min;
            let flexible_count = auto_cells
                .iter()
                .filter(|(_, h)| *h <= equal_share)
                .count() as i64;
            let bonus = if flexible_count > 0 {
                surplus / flexible_count
            } else {
                0
            };
            let (is_self, my_min) = auto_cells[self_index];
            let _ = is_self; // suppress unused warning
            if my_min > equal_share {
                // This cell needs its minimum — give it that.
                my_min
            } else {
                // This cell gets its minimum plus a share of the surplus.
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

    fn element_attribute(&self, name: &str) -> Option<String> {
        match self.node.borrow().kind() {
            NodeKind::Element(ref element) => element.get_attribute(name),
            _ => None,
        }
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
        offset
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
                        return vec![DisplayItem::Rect {
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
                        }];
                    }
                }
            }
            LayoutObjectKind::Inline => {
                if let NodeKind::Element(_) = self.node_kind() {
                    let mut items = Vec::new();
                    if self.size.width() > 0 && self.size.height() > 0 {
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
                            paint_order: PaintOrder {
                                stacking_context: if self.style.position() != PositionType::Static {
                                    1
                                } else {
                                    0
                                },
                                z_index: self.style.z_index(),
                            },
                            clip_rect: None,
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
                    let max_width = self.size().width().max(CHAR_WIDTH * ratio);
                    let lines = split_text(plain_text, CHAR_WIDTH * ratio, max_width);
                    let href = self.link_href();

                    for (i, line) in lines.into_iter().enumerate() {
                        let item = DisplayItem::Text {
                            text: line,
                            style: self.style(),
                            layout_point: LayoutPoint::new(
                                self.point().x(),
                                self.point().y() + CHAR_HEIGHT_WITH_PADDING * ratio * i as i64,
                            ),
                            href: href.clone(),
                            paint_order: PaintOrder {
                                stacking_context: if self.style.position() != PositionType::Static {
                                    1
                                } else {
                                    0
                                },
                                z_index: self.style.z_index(),
                            },
                            clip_rect: None,
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
                    let attr_width = parse_dimension_attr(self.element_attribute("width"));
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
                } else if explicit_width > 0 {
                    explicit_width.min(available_width)
                } else if let Some(w) = html_width {
                    w.min(available_width)
                } else if self.element_kind() == Some(ElementKind::Table) {
                    // Tables without explicit width use shrink-to-fit: the width
                    // of the widest row (sum of cell widths), capped at available.
                    let mut max_row_width: i64 = 0;
                    let mut row = self.first_child();
                    while let Some(r) = row {
                        if r.borrow().is_table_row() {
                            let mut row_width: i64 = 0;
                            let mut cell = r.borrow().first_child();
                            while let Some(c) = cell {
                                row_width += c.borrow().size.width();
                                let next = c.borrow().next_sibling();
                                cell = next;
                            }
                            max_row_width = max_row_width.max(row_width);
                        }
                        let next = r.borrow().next_sibling();
                        row = next;
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
                    } else if previous_child_kind.normal_flow_spec().stacks_vertically
                        || c_kind.normal_flow_spec().stacks_vertically
                    {
                        content_height += c.borrow().size.height();
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
                size.set_width((content_width + metrics.inner_horizontal()).max(0));
                size.set_height((content_height + metrics.inner_vertical()).max(0));
            }
            LayoutObjectKind::Inline => {
                if let Some(intrinsic) = self.intrinsic_inline_size(parent_size) {
                    size.set_width((intrinsic.width() + metrics.inner_horizontal()).max(0));
                    size.set_height((intrinsic.height() + metrics.inner_vertical()).max(0));
                } else {
                    let mut content_width = 0;
                    let mut content_height = 0;
                    let mut child = self.first_child();
                    while child.is_some() {
                        let c = child.expect("child should exist");
                        content_width += c.borrow().size.width();
                        content_height = content_height.max(c.borrow().size.height());
                        child = c.borrow().next_sibling();
                    }

                    size.set_width((content_width + metrics.inner_horizontal()).max(0));
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
                    let max_width =
                        (parent_size.width() - metrics.outer_horizontal()).max(CHAR_WIDTH * ratio);
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
            if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                point.set_x(pos.x() + size.width() + metrics.margin.left);
                point.set_y(parent_point.y() + metrics.margin.top);
            } else {
                // First cell in row: apply rowspan offset from previous rows.
                let rowspan_offset = self
                    .parent
                    .upgrade()
                    .map(|p| p.borrow().rowspan_column_offset())
                    .unwrap_or(0);
                point.set_x(parent_point.x() + rowspan_offset + metrics.margin.left);
                point.set_y(parent_point.y() + metrics.margin.top);
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
                    point.set_x(pos.x() + size.width() + metrics.margin.left);
                    point.set_y(pos.y() + metrics.margin.top);
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
                            DisplayType::from_str(value).unwrap_or(DisplayType::DisplayNone);
                        self.style.set_display(display_type)
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
                DisplayType::Block => self.kind = LayoutObjectKind::Block,
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

    pub fn content_origin(&self) -> LayoutPoint {
        let metrics = compute_box_model_metrics(&self.style);
        LayoutPoint::new(
            self.point.x() + metrics.padding.left + metrics.border.left,
            self.point.y() + metrics.padding.top + metrics.border.top,
        )
    }

    pub fn content_size(&self) -> LayoutSize {
        let metrics = compute_box_model_metrics(&self.style);
        LayoutSize::new(
            (self.size.width() - metrics.inner_horizontal()).max(0),
            (self.size.height() - metrics.inner_vertical()).max(0),
        )
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
