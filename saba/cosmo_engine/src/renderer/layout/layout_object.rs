// Spec: CSS Box Model — margin/border/padding/content areas and box sizing.
// https://www.w3.org/TR/css-box-4/
// Spec: CSS Display — outer/inner display types and block/inline formatting contexts.
// https://www.w3.org/TR/css-display-3/
// Spec: CSS Cascade — specificity, origin, and inheritance resolution order.
// https://www.w3.org/TR/css-cascade-5/
// Spec: CSS Values and Units — length units (px, em, rem, vh, vw) and numeric types.
// https://www.w3.org/TR/css-values-4/
use crate::renderer::css::cssom::ComponentValue;
use crate::renderer::css::cssom::Declaration;
use crate::renderer::css::cssom::QualifiedRule;
use crate::renderer::css::cssom::Selector;
use crate::renderer::css::cssom::StyleSheet;
use crate::renderer::dom::node::Element;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::Node;
use crate::renderer::dom::node::NodeKind;
use crate::renderer::layout::computed_style::ComputedStyle;
use crate::renderer::layout::computed_style::DisplayType;
use crate::renderer::layout::computed_style::EdgeSize;
use crate::renderer::layout::computed_style::FlexDirection;
use crate::renderer::layout::computed_style::Clear;
use crate::renderer::layout::computed_style::Float;
use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::layout::computed_style::GridTrack;
use crate::renderer::layout::computed_style::PositionType;
use crate::renderer::layout::computed_style::TextAlign;
use crate::renderer::layout::floats::{FloatContext, FloatSide};
use crate::renderer::layout::inline::{
    layout_inline_items_aligned, InlineItem, LineAlign, LineOptions, TextRun,
};
use std::format;
use std::rc::Rc;
use std::rc::Weak;
use std::string::String;
use std::string::ToString;
use std::vec;
use std::vec::Vec;
use std::cell::RefCell;
use crate::renderer::style::selector::dom_node_selected;
use crate::renderer::style::values::{
    parse_dimension_attr, parse_dimension_pct_attr,
    resolve_grid_tracks,
};
use crate::renderer::text::legacy_metrics::{
    bold_width_adjust, char_width_px,
    is_wide_char, measure_text_width, split_text,
    styled_line_height,
};

fn edge_to_i64(value: f64) -> i64 {
    if value <= 0.0 {
        0
    } else {
        value as i64
    }
}




pub fn create_layout_object(
    node: &Option<Rc<RefCell<Node>>>,
    parent_obj: &Option<Rc<RefCell<LayoutObject>>>,
    cssom: &StyleSheet,
) -> Option<Rc<RefCell<LayoutObject>>> {
    if let Some(n) = node {
        let layout_object = Rc::new(RefCell::new(LayoutObject::new(n.clone(), parent_obj)));

        // Parent font size: the base for resolving font-size em/% values.
        let parent_font_size = parent_obj
            .as_ref()
            .map(|p| p.borrow().style().font_size())
            .unwrap_or(FontSize::Medium);

        // Cascade order: matching rules sorted by specificity (ascending) so a
        // more specific selector wins even when it appears earlier in the
        // document; the stable sort keeps document order for equal
        // specificity (later rule wins). Spec: CSS Cascade §6.
        // https://www.w3.org/TR/css-cascade-4/#cascade-specificity
        let mut matched: Vec<&QualifiedRule> = cssom
            .rules
            .iter()
            .filter(|rule| layout_object.borrow().is_node_selected(&rule.selector))
            .collect();
        matched.sort_by_key(|rule| rule.selector.specificity());

        // Inline `style="..."` attribute declarations (parsed once, applied in
        // the importance tiers below).
        // Spec: https://www.w3.org/TR/css-style-attr/#interpret
        let inline_declarations: Vec<Declaration> = match n.borrow().kind() {
            NodeKind::Element(ref element) => element
                .get_attribute("style")
                .map(|style_attr| {
                    use crate::renderer::css::cssom::CssParser;
                    use crate::renderer::css::token::CssTokenizer;
                    CssParser::new(CssTokenizer::new(style_attr)).parse_declaration_list()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };

        // Custom-property cascade: inherit the parent's scope; the tree root
        // seeds from the whole stylesheet (this also covers `:root`, which has
        // no layout object — the layout tree is rooted at <body>). Element-
        // level `--name` definitions from matched rules and the inline style
        // copy-on-write into a fresh map, resolving nested var() against the
        // scope built so far. Must be in place BEFORE normal declarations are
        // applied, since their var() references substitute from this scope.
        // https://www.w3.org/TR/css-variables-1/
        {
            use crate::renderer::css::cssom::substitute_vars;
            let inherited = parent_obj
                .as_ref()
                .and_then(|p| p.borrow().style().custom_properties().cloned())
                .unwrap_or_else(|| {
                    // Layout-tree root (<body>): its DOM ancestors (<html>,
                    // matched by `:root`) have no layout objects, so evaluate
                    // their matching rules here to seed the scope.
                    let mut map = std::collections::BTreeMap::new();
                    let mut chain: Vec<Rc<RefCell<Node>>> = Vec::new();
                    let mut current = n.borrow().parent().upgrade();
                    while let Some(p) = current {
                        if matches!(p.borrow().kind(), NodeKind::Element(_)) {
                            chain.push(p.clone());
                        }
                        let next = p.borrow().parent().upgrade();
                        current = next;
                    }
                    for ancestor in chain.iter().rev() {
                        for rule in &cssom.rules {
                            if dom_node_selected(ancestor, &rule.selector) {
                                for d in rule
                                    .declarations
                                    .iter()
                                    .filter(|d| d.property.starts_with("--"))
                                {
                                    let value = substitute_vars(&d.value, &map);
                                    map.insert(d.property.clone(), value);
                                }
                            }
                        }
                    }
                    Rc::new(map)
                });
            let mut own: Option<std::collections::BTreeMap<String, Vec<ComponentValue>>> =
                None;
            for declarations in matched
                .iter()
                .map(|rule| &rule.declarations)
                .chain(std::iter::once(&inline_declarations))
            {
                for d in declarations.iter().filter(|d| d.property.starts_with("--")) {
                    let map = own.get_or_insert_with(|| (*inherited).clone());
                    let value = substitute_vars(&d.value, map);
                    map.insert(d.property.clone(), value);
                }
            }
            let scope = own.map(Rc::new).unwrap_or(inherited);
            layout_object.borrow_mut().style.set_custom_properties(scope);
        }

        // Importance tiers (weakest first; the last write wins in this
        // engine's cascade): normal stylesheet declarations, normal inline
        // declarations, !important stylesheet declarations, !important inline
        // declarations. Spec: CSS Cascade §6.1 — declarations marked
        // !important outrank all normal declarations of the same origin.
        // https://www.w3.org/TR/css-cascade-4/#cascade-origin
        for important in [false, true] {
            for rule in &matched {
                let tier: Vec<Declaration> = rule
                    .declarations
                    .iter()
                    .filter(|d| d.important == important)
                    .cloned()
                    .collect();
                if !tier.is_empty() {
                    layout_object
                        .borrow_mut()
                        .cascading_style(tier, parent_font_size);
                }
            }
            let tier: Vec<Declaration> = inline_declarations
                .iter()
                .filter(|d| d.important == important)
                .cloned()
                .collect();
            if !tier.is_empty() {
                layout_object
                    .borrow_mut()
                    .cascading_style(tier, parent_font_size);
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

/// Build a `::before` / `::after` generated-content box for `host`, if a
/// matching pseudo-element rule supplies a non-empty `content` string.
/// The synthesized box is an inline element (carrying the pseudo rules'
/// styles, inheriting the host's) wrapping a text node with the content.
/// Spec: CSS Generated Content §2. https://www.w3.org/TR/css-content-3/
pub fn build_pseudo_element(
    host_node: &Rc<RefCell<Node>>,
    host_obj: &Rc<RefCell<LayoutObject>>,
    cssom: &StyleSheet,
    pe: crate::renderer::css::cssom::PseudoElement,
) -> Option<Rc<RefCell<LayoutObject>>> {
    use crate::renderer::css::cssom::Selector;
    // Collect matching pseudo-element rules (host part matches host_node),
    // sorted by specificity.
    let mut matched: Vec<&QualifiedRule> = cssom
        .rules
        .iter()
        .filter(|rule| match &rule.selector {
            Selector::PseudoElement(host_sel, kind) => {
                *kind == pe && dom_node_selected(host_node, host_sel)
            }
            _ => false,
        })
        .collect();
    matched.sort_by_key(|rule| rule.selector.specificity());

    // Resolve the `content` string (last declaration wins). `none`/`normal`
    // suppress the box.
    let mut content: Option<String> = None;
    for rule in &matched {
        for d in &rule.declarations {
            if d.property == "content" {
                content = pseudo_content_string(&d.value);
            }
        }
    }
    let content = content?;

    // Synthesize an inline element node + text child, parented (in the DOM
    // sense) under the host so selector matching of nested rules behaves.
    let span_node = Rc::new(RefCell::new(Node::new(NodeKind::Element(Element::new(
        "span",
        Vec::new(),
    )))));
    span_node
        .borrow_mut()
        .set_parent(Rc::downgrade(host_node));
    let text_node = Rc::new(RefCell::new(Node::new(NodeKind::Text(content))));
    text_node
        .borrow_mut()
        .set_parent(Rc::downgrade(&span_node));
    span_node
        .borrow_mut()
        .set_first_child(Some(text_node.clone()));

    let parent_obj = Some(host_obj.clone());
    let span_obj = Rc::new(RefCell::new(LayoutObject::new(span_node.clone(), &parent_obj)));
    let parent_font_size = host_obj.borrow().style().font_size();

    // Apply the pseudo rules' declarations (skip `content`, not a property).
    for important in [false, true] {
        for rule in &matched {
            let tier: Vec<Declaration> = rule
                .declarations
                .iter()
                .filter(|d| d.important == important && d.property != "content")
                .cloned()
                .collect();
            if !tier.is_empty() {
                span_obj.borrow_mut().cascading_style(tier, parent_font_size);
            }
        }
    }
    let host_style = Some(host_obj.borrow().style());
    span_obj.borrow_mut().defaulting_style(&span_node, host_style);
    if span_obj.borrow().style().display() == DisplayType::DisplayNone {
        return None;
    }
    span_obj.borrow_mut().update_kind();

    // Text child.
    let text_obj = Rc::new(RefCell::new(LayoutObject::new(
        text_node.clone(),
        &Some(span_obj.clone()),
    )));
    let span_style = Some(span_obj.borrow().style());
    text_obj.borrow_mut().defaulting_style(&text_node, span_style);
    text_obj.borrow_mut().update_kind();
    span_obj.borrow_mut().set_first_child(Some(text_obj));

    Some(span_obj)
}

/// Extract the string literal from a `content` value. Returns None for
/// `none`/`normal` or non-string values (url()/counters aren't supported).
fn pseudo_content_string(values: &[ComponentValue]) -> Option<String> {
    let mut out = String::new();
    let mut saw = false;
    for v in values {
        match v {
            ComponentValue::StringToken(s) => {
                out.push_str(s);
                saw = true;
            }
            ComponentValue::Ident(name)
                if name.eq_ignore_ascii_case("none")
                    || name.eq_ignore_ascii_case("normal") =>
            {
                return None;
            }
            _ => {}
        }
    }
    if saw {
        Some(out)
    } else {
        None
    }
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
    pub(crate) kind: LayoutObjectKind,
    pub(crate) node: Rc<RefCell<Node>>,
    pub(crate) first_child: Option<Rc<RefCell<LayoutObject>>>,
    pub(crate) next_sibling: Option<Rc<RefCell<LayoutObject>>>,
    pub(crate) parent: Weak<RefCell<LayoutObject>>,
    pub(crate) style: ComputedStyle,
    pub(crate) point: LayoutPoint,
    pub(crate) size: LayoutSize,
    // The max_width used in split_text() during compute_size for Text nodes.
    // Cached here so that paint() uses the identical line-breaking boundary,
    // preventing the double-split divergence that causes text to stack
    // vertically instead of flowing horizontally.
    // Spec: CSS2.2 §9.4.2 — inline formatting context line construction.
    // https://www.w3.org/TR/CSS22/visuren.html#inline-formatting
    pub(crate) text_line_max_width: i64,
    /// Line fragments assigned by the inline formatting context (Phase 2.5),
    /// as (text, x, y) relative to this box's own origin. Empty on the legacy
    /// path, where paint re-splits the text against `text_line_max_width`.
    /// A run breaking over three lines has three fragments, each with its own
    /// x — which is what lets a run start mid-line and continue at the left
    /// edge below.
    pub(crate) inline_fragments: Vec<(String, i64, i64)>,
    /// Offset from the containing block's content origin assigned by the
    /// inline formatting context. When set, `compute_position` uses it instead
    /// of anchoring against the previous sibling.
    pub(crate) inline_offset: Option<(i64, i64)>,
    /// Floats placed in the block formatting context this box establishes, in
    /// coordinates relative to its own content box. Populated between layout
    /// iterations from the positions the previous one produced, and read by
    /// descendants' inline layout to shorten their lines.
    pub(crate) float_context: Option<FloatContext>,
    /// Where normal flow would have put this box's top, recorded before any
    /// float placement overrode it. Float placement must read *this*, not the
    /// position it assigned last time, or each pass takes its own output as the
    /// input and the float can only ever drift downwards.
    pub(crate) flow_y: i64,
    /// Flow-end cursor of a wrapped text run: width of the LAST line, the
    /// number of lines, and the line height used. A following inline sibling
    /// continues after the last line, not at the bounding box's top-right.
    pub(crate) text_last_line_width: i64,
    pub(crate) text_line_count: i64,
    pub(crate) text_line_height: i64,
    // Per-logical-column max of min_content_width_hint, populated once per
    // table by the pre-pass before any cell sizing.  Only meaningful on table
    // nodes; None elsewhere.  Used by `table_cell_auto_width` so that a row
    // whose cell content is narrow (e.g. &nbsp;) still reserves space for a
    // sibling row whose cell at the same column has substantial content.
    // Spec: CSS 2.2 §17.5.2 — table layout: auto.
    // https://www.w3.org/TR/CSS22/tables.html#auto-table-layout
    pub(crate) column_min_hints: Option<Vec<i64>>,
    // Per-logical-column max of max_content_width (the column's preferred,
    // longest-line width), populated alongside column_min_hints by the
    // pre-pass.  Used by `table_cell_auto_width` to weight surplus distribution
    // by each column's growth headroom (max - min), so a narrow label column
    // (e.g. a rank number) does not absorb surplus meant for a wide text column.
    pub(crate) column_max_hints: Option<Vec<i64>>,
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
            inline_fragments: Vec::new(),
            inline_offset: None,
            float_context: None,
            flow_y: 0,
            text_last_line_width: 0,
            text_line_count: 0,
            text_line_height: 0,
            column_min_hints: None,
            column_max_hints: None,
        }
    }

    pub(crate) fn link_href(&self) -> Option<String> {
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
    pub(crate) fn link_target(&self) -> Option<String> {
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

    pub fn element_kind(&self) -> Option<ElementKind> {
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
    pub(crate) fn nearest_ancestor_cell(&self) -> Option<Rc<RefCell<LayoutObject>>> {
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

    /// If an ancestor declares `text-overflow: ellipsis`, return the px width
    /// available to this text node before the ancestor's right content edge.
    /// `text-overflow` only takes effect on a clipping container, so the
    /// ancestor must also clip overflow.
    pub(crate) fn ellipsis_clip_width(&self) -> Option<i64> {
        let mut current = self.parent.upgrade();
        while let Some(node) = current {
            let b = node.borrow();
            if b.style().text_overflow_ellipsis() && b.style().overflow_clip() {
                let cm = compute_box_model_metrics(&b.style);
                let right = b.point().x() + b.size().width() - cm.border.right - cm.padding.right;
                let avail = right - self.point().x();
                return Some(avail.max(0));
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

    /// Resolve this row-flex item's CONTENT width by running the flex line
    /// distribution over all the container's items (each item independently
    /// computes the same distribution — the table-cell pattern).
    fn flex_row_main_size(&self, container_content_width: i64) -> i64 {
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return 0,
        };
        let gap = parent.borrow().style().column_gap();

        struct Item {
            base_content: f64,
            box_overhead: f64, // padding+border+margins
            min_content: f64,
            grow: f64,
            shrink: f64,
            is_self: bool,
        }
        let mut items: Vec<Item> = Vec::new();
        let build = |st: &ComputedStyle,
                     max_content: f64,
                     min_content: f64,
                     is_self: bool|
         -> Item {
            let m = compute_box_model_metrics(st);
            let base_content = if let Some(basis) = st.flex_basis() {
                basis
            } else if let Some(ratio) = st.width_ratio() {
                (container_content_width as f64 * ratio).max(0.0)
            } else if st.width() > 0.0 {
                st.width()
            } else {
                max_content
            };
            Item {
                base_content,
                box_overhead: (m.inner_horizontal() + m.outer_horizontal()) as f64,
                min_content,
                grow: st.flex_grow(),
                shrink: st.flex_shrink(),
                is_self,
            }
        };
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            // compute_size holds `self` mutably borrowed; the failing borrow
            // in this walk IS self — read our own fields directly.
            match c.try_borrow() {
                Ok(b) => {
                    let next = b.next_sibling();
                    if !b.is_whitespace_text()
                        && !matches!(
                            b.style().position(),
                            PositionType::Absolute | PositionType::Fixed
                        )
                    {
                        let st = b.style();
                        let item = build(
                            &st,
                            b.max_content_width() as f64,
                            b.min_content_width_hint() as f64,
                            false,
                        );
                        drop(b);
                        items.push(item);
                    }
                    child = next;
                }
                Err(_) => {
                    let next = self.next_sibling.clone();
                    items.push(build(
                        &self.style,
                        self.max_content_width() as f64,
                        self.min_content_width_hint() as f64,
                        true,
                    ));
                    child = next;
                }
            }
        }
        if items.is_empty() {
            return 0;
        }

        let total: f64 = items.iter().map(|i| i.base_content + i.box_overhead).sum();
        let gaps = (gap * (items.len() as i64 - 1)) as f64;
        let free = container_content_width as f64 - total - gaps;

        let me = items.iter().find(|i| i.is_self);
        let me = match me {
            Some(m) => m,
            None => return 0,
        };
        let mut target = me.base_content;
        if free > 0.0 {
            let total_grow: f64 = items.iter().map(|i| i.grow).sum();
            if total_grow > 0.0 {
                target += free * me.grow / total_grow;
            }
        } else if free < 0.0 {
            let total_scaled: f64 = items
                .iter()
                .map(|i| i.shrink * (i.base_content + i.box_overhead))
                .sum();
            if total_scaled > 0.0 {
                target += free * (me.shrink * (me.base_content + me.box_overhead)) / total_scaled;
            }
        }
        // Never shrink below the item's min-content (long unbreakable words).
        (target.max(me.min_content).max(0.0)) as i64
    }

    fn is_flex_container(&self) -> bool {
        self.style.display() == DisplayType::Flex
    }

    /// If this object's parent is a grid container (`display:grid`), return
    /// its (column tracks, column gap, row gap); otherwise `None`.
    /// Nearest ancestor that generates a real box (skipping display:contents
    /// wrappers) — the parent grid/flex placement resolves against.
    fn effective_placement_parent(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        let mut anc = self.parent.upgrade();
        while let Some(a) = anc {
            let step: Option<Option<Rc<RefCell<LayoutObject>>>> = match a.try_borrow() {
                Ok(b) => {
                    if b.style().display() == DisplayType::Contents {
                        Some(b.parent.upgrade())
                    } else {
                        None
                    }
                }
                Err(_) => None,
            };
            match step {
                None => return Some(a),
                Some(next) => anc = next,
            }
        }
        None
    }

    fn parent_grid_info(&self) -> Option<(Vec<GridTrack>, i64, i64)> {
        let parent = self.effective_placement_parent()?;
        let p = match parent.try_borrow() {
            Ok(p) => p,
            Err(_) => return None,
        };
        if p.style.display() == DisplayType::Grid {
            let mut tracks = p.style.grid_template_columns();
            // grid-template-areas implies a column count. When the explicit
            // grid-template-columns declares fewer tracks than the areas have
            // columns (e.g. Wikipedia's `columns: minmax(0,1fr)` with
            // `areas: 'columnStart pageContent'`), the missing tracks default
            // to `auto` — without this the 2nd-column items get a zero-width
            // out-of-range track and collapse against the right edge.
            if let Some(areas) = p.style.grid_template_areas() {
                let cols = areas.iter().map(|r| r.len()).max().unwrap_or(0);
                while tracks.len() < cols {
                    tracks.push(GridTrack::Auto);
                }
            }
            Some((tracks, p.style.column_gap(), p.style.row_gap()))
        } else {
            None
        }
    }

    /// True for a whitespace-only text node. Such nodes are formatting
    /// artifacts of the markup (newlines/indentation between elements) and are
    /// not grid items per CSS Grid §6 (only inter-element whitespace that
    /// collapses away).
    pub(crate) fn is_whitespace_text(&self) -> bool {
        match self.node.borrow().kind() {
            NodeKind::Text(ref t) => t.trim().is_empty(),
            _ => false,
        }
    }

    /// 0-based grid-item index of this object among its parent's children:
    /// whitespace-only text siblings are skipped (they are not grid items).
    /// Identified by pointer identity, so it works while `self` is inside an
    /// active borrow.
    /// (row_start, row_span, col_start, col_span) of `name` inside a
    /// grid-template-areas matrix.
    pub(crate) fn area_rect_in(
        areas: &[Vec<String>],
        name: &str,
    ) -> Option<(usize, usize, usize, usize)> {
        let (mut r0, mut r1, mut c0, mut c1) = (usize::MAX, 0usize, usize::MAX, 0usize);
        for (r, row) in areas.iter().enumerate() {
            for (c, cell) in row.iter().enumerate() {
                if cell == name {
                    r0 = r0.min(r);
                    r1 = r1.max(r);
                    c0 = c0.min(c);
                    c1 = c1.max(c);
                }
            }
        }
        if r0 == usize::MAX {
            return None;
        }
        Some((r0, r1 - r0 + 1, c0, c1 - c0 + 1))
    }

    /// This item's grid-area rectangle within its parent's template areas,
    /// or (row 0) between the parent's named column lines `name-start` /
    /// `name-end` when no template area defines it.
    fn grid_area_rect(&self) -> Option<(usize, usize, usize, usize)> {
        let name = self.style.grid_area_name()?.to_string();
        let parent = self.effective_placement_parent()?;
        let pstyle = match parent.try_borrow() {
            Ok(p) => p.style(),
            Err(_) => return None,
        };
        if let Some(areas) = pstyle.grid_template_areas() {
            if let Some(r) = Self::area_rect_in(&areas, &name) {
                return Some(r);
            }
        }
        let lines = pstyle.grid_column_line_names()?;
        let start_name = format!("{name}-start");
        let end_name = format!("{name}-end");
        let start = lines
            .iter()
            .position(|ns| ns.contains(&start_name) || ns.contains(&name));
        let end = lines.iter().position(|ns| ns.contains(&end_name));
        match (start, end) {
            (Some(s), Some(e)) if e > s => Some((0, 1, s, e - s)),
            (Some(s), None) => Some((0, 1, s, 1)),
            _ => None,
        }
    }

    /// Heights of the parent grid's area rows: for each template row, the
    /// max outer height of the items whose area starts there. Sizes are
    /// final by position time, and the walk tolerates `self` being mutably
    /// borrowed (try_borrow failure = self).
    fn grid_area_row_heights(&self) -> Vec<i64> {
        let parent = match self.effective_placement_parent() {
            Some(p) => p,
            None => return Vec::new(),
        };
        let areas = match parent.try_borrow().ok().and_then(|p| p.style().grid_template_areas()) {
            Some(a) => a,
            None => return Vec::new(),
        };
        let mut heights = vec![0i64; areas.len()];
        // Walk the grid's items; display:contents children are transparent —
        // descend into them so their children (the real grid items) tally.
        let mut stack: Vec<Option<Rc<RefCell<LayoutObject>>>> =
            vec![parent.try_borrow().ok().and_then(|p| p.first_child())];
        while let Some(slot) = stack.pop() {
            let mut child = slot;
            while let Some(c) = child {
                match c.try_borrow() {
                    Ok(b) => {
                        let next = b.next_sibling();
                        if b.style().display() == DisplayType::Contents {
                            stack.push(b.first_child());
                        } else if !b.is_whitespace_text() {
                            if let Some(name) = b.style().grid_area_name() {
                                if let Some((r0, _, _, _)) = Self::area_rect_in(&areas, name) {
                                    let m = compute_box_model_metrics(&b.style());
                                    heights[r0] = heights[r0]
                                        .max(b.size.height() + m.margin.top + m.margin.bottom);
                                }
                            }
                        }
                        drop(b);
                        child = next;
                    }
                    Err(_) => {
                        if let Some(name) = self.style.grid_area_name() {
                            if let Some((r0, _, _, _)) = Self::area_rect_in(&areas, name) {
                                let m = compute_box_model_metrics(&self.style);
                                heights[r0] = heights[r0]
                                    .max(self.size.height() + m.margin.top + m.margin.bottom);
                            }
                        }
                        child = self.next_sibling.clone();
                    }
                }
            }
        }
        heights
    }

    /// The layout sibling directly before `self`, skipping zero-size boxes
    /// (collapsed whitespace nodes are not flow anchors — same rule the
    /// position walk uses). Pointer identity, so it works while `self` is
    /// mutably borrowed.
    fn previous_layout_sibling(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        let parent = self.parent.upgrade()?;
        let mut prev: Option<Rc<RefCell<LayoutObject>>> = None;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            if std::ptr::eq(c.as_ptr() as *const LayoutObject, self as *const LayoutObject) {
                return prev;
            }
            let b = c.borrow();
            let zero_size = b.size.width() == 0 && b.size.height() == 0;
            let next = b.next_sibling();
            drop(b);
            if !zero_size {
                prev = Some(c);
            }
            child = next;
        }
        None
    }

    /// Containing block for an absolutely positioned box: the content box of
    /// the nearest positioned ancestor. None = no positioned ancestor (the
    /// caller anchors to its direct parent, the legacy approximation of the
    /// initial containing block).
    fn absolute_containing_block(&self) -> Option<(LayoutPoint, LayoutSize)> {
        let mut anc = self.parent.upgrade();
        while let Some(a) = anc {
            let step = match a.try_borrow() {
                Ok(b) => {
                    let positioned = b.style().position() != PositionType::Static;
                    if positioned {
                        return Some((b.content_origin(), b.content_size()));
                    }
                    b.parent.upgrade()
                }
                // An ancestor is mid-borrow (shouldn't happen in the position
                // walk): fall back to the legacy anchor.
                Err(_) => return None,
            };
            anc = step;
        }
        None
    }

    /// Marker text for a <li> with a non-none list-style-type: bullet glyphs,
    /// or "N." for decimal (N = 1-based index among <li> DOM siblings).
    pub(crate) fn list_marker_text(&self) -> Option<String> {
        use crate::renderer::layout::computed_style::ListStyleType;
        if self.node.borrow().element_tag_name().as_deref() != Some("li") {
            return None;
        }
        match self.style.list_style_type() {
            ListStyleType::None => None,
            ListStyleType::Disc => Some("•".to_string()),
            ListStyleType::Circle => Some("◦".to_string()),
            ListStyleType::Square => Some("▪".to_string()),
            ListStyleType::Decimal => {
                let mut index = 1usize;
                let mut prev = self.node.borrow().previous_sibling().upgrade();
                while let Some(p) = prev {
                    if p.borrow().element_tag_name().as_deref() == Some("li") {
                        index += 1;
                    }
                    prev = p.borrow().previous_sibling().upgrade();
                }
                Some(format!("{}.", index))
            }
        }
    }

    fn grid_item_index(&self) -> usize {
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return 0,
        };
        let mut idx = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            if std::ptr::eq(c.as_ptr() as *const LayoutObject, self as *const LayoutObject) {
                return idx;
            }
            let b = c.borrow();
            if !b.is_whitespace_text() {
                idx += 1;
            }
            let next = b.next_sibling();
            drop(b);
            child = next;
        }
        idx
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
                    let bold = b.style.is_bold();
                    // Each hard line within the text starts a new line box.
                    let widest = t
                        .split('\n')
                        .map(|line| measure_text_width(line.trim(), fs, bold))
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
            // An explicit CSS width on a child box (e.g. HN's
            // `.votearrow{width:10px}`) is part of the column's minimum: the
            // box cannot shrink below it, plus its horizontal margins.
            let css_w = borrowed.style.width() as i64;
            if css_w > 0 {
                let m = borrowed.style.margin();
                max_hint = max_hint.max(css_w + m.left() as i64 + m.right() as i64);
            }
            // For text nodes, use the longest *unbreakable* run.
            // CJK chars can break between any two characters; each is its own
            // minimum unit.  Pure-ASCII runs between wide chars are truly
            // unbreakable and may be wider — take the max of the two.
            if let NodeKind::Text(ref t) = borrowed.node_kind() {
                let font_size = borrowed.style.font_size();
                let bold = borrowed.style.is_bold();
                let longest = t.split(|c: char| c == ' ' || c == '\u{3000}' || c == '\n' || c == '\t')
                    .map(|word| {
                        // Longest ASCII run between wide chars within this word.
                        let ascii_max = word.split(|c: char| is_wide_char(c))
                            .map(|seg| measure_text_width(seg.trim(), font_size, bold))
                            .max()
                            .unwrap_or(0);
                        // Each wide char is its own break unit.  We return
                        // 3×CHAR_WIDTH so that any cell with CJK content exceeds
                        // SPACER_THRESHOLD (20) and is classified as flexible
                        // content rather than a decorative spacer.
                        let wide_min = if word.chars().any(|c| is_wide_char(c)) {
                            3 * char_width_px(font_size)
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

    /// Determine this cell's LOGICAL column index (0-based) within its parent
    /// row: preceding cells advance the index by their colspan, so a cell after
    /// a `<td colspan=2>` lead-in is at column 2, not 1. (Rowspan cells from
    /// previous rows are not accounted for here; callers gate on
    /// `rowspan_offset == 0`.)
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
                        index += borrowed.cell_colspan();
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
                    // Advance by the cell's colspan so that sibling rows with
                    // colspan cells (e.g. a `<td colspan=2>` lead-in) map to
                    // LOGICAL column indices, matching the caller's index.
                    idx += cb.cell_colspan();
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

    /// Return the per-column preferred (max-content) hint vector populated by
    /// the table pre-pass, if any. Only set on `<table>` nodes.
    pub fn column_max_hints(&self) -> Option<&[i64]> {
        self.column_max_hints.as_deref()
    }

    /// Store the per-column preferred (max-content) hint vector on this table.
    pub fn set_column_max_hints(&mut self, v: Vec<i64>) {
        self.column_max_hints = Some(v);
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
        Self::compute_table_column_hints(table, false)
    }

    /// Per-logical-column maximum of each cell's preferred (max-content) width.
    /// Mirror of `compute_table_column_min_hints` using `max_content_width`.
    pub fn compute_table_column_max_hints(table: &Rc<RefCell<LayoutObject>>) -> Vec<i64> {
        Self::compute_table_column_hints(table, true)
    }

    /// Shared implementation of the per-column hint pre-pass. When `use_max` is
    /// false it accumulates `min_content_width_hint` (the column's minimum
    /// width); when true it accumulates `max_content_width` (the column's
    /// preferred width).
    fn compute_table_column_hints(table: &Rc<RefCell<LayoutObject>>, use_max: bool) -> Vec<i64> {
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
                    let content_hint = if use_max {
                        cb.max_content_width()
                    } else {
                        cb.min_content_width_hint()
                    };
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

    /// From a table cell, walk up to the containing `<table>` and return its
    /// per-column preferred (max-content) hints, if any.
    fn ancestor_table_column_max_hints(&self) -> Option<Vec<i64>> {
        let row = self.parent.upgrade()?;
        let row_parent = row.borrow().parent.upgrade()?;
        let table = if row_parent.borrow().is_row_group() {
            row_parent.borrow().parent.upgrade()?
        } else {
            row_parent
        };
        let t = table.borrow();
        t.column_max_hints().map(|s| s.to_vec())
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
        // Column-level preferred widths, gated identically to the min hints.
        let column_max_hints = if rowspan_offset == 0 && !row_has_colspan {
            self.ancestor_table_column_max_hints()
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
        // (is_self, min_hint, max_content) — max_content is the cell's preferred
        // (longest-line) width, used to cap how much surplus a column absorbs so
        // a narrow label column (e.g. a rank number) does not balloon to share
        // surplus equally with a genuinely flexible text column.
        let mut auto_cells: Vec<(bool, i64, i64)> = Vec::new();
        // Sum of every auto cell's horizontal padding+border. The widths
        // returned here are CONTENT widths and compute_size adds each cell's
        // metrics.inner_horizontal() on top; without subtracting that overhead
        // from the budget the row's cell boxes overflow the table's right edge
        // by (cell count × padding).
        let mut auto_box_overhead: i64 = 0;
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
                            // Promote to the column-level preferred width so a
                            // spacer cell in this row still reflects how wide a
                            // sibling row's content in the same column wants to be.
                            let col_max = column_max_hints
                                .as_ref()
                                .and_then(|h| h.get(col_idx).copied())
                                .unwrap_or(0);
                            let max_content =
                                borrowed.max_content_width().max(col_max).max(hint);
                            auto_cells.push((false, hint, max_content));
                            auto_box_overhead +=
                                compute_box_model_metrics(&borrowed.style).inner_horizontal();
                        }
                        // Advance by colspan so col_idx stays a LOGICAL column
                        // index, consistent with cell_column_index() and the
                        // hint vectors.
                        col_idx += borrowed.cell_colspan();
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
                    let col_max = column_max_hints
                        .as_ref()
                        .and_then(|h| h.get(col_idx).copied())
                        .unwrap_or(0);
                    let max_content = self.max_content_width().max(col_max).max(hint);
                    self_index = auto_cells.len();
                    auto_cells.push((true, hint, max_content));
                    auto_box_overhead +=
                        compute_box_model_metrics(&self.style).inner_horizontal();
                    col_idx += self.cell_colspan();
                    let next = c.as_ptr();
                    child = unsafe { (*next).next_sibling() };
                }
            }
        }

        let remaining = (effective_width - total_explicit - auto_box_overhead).max(0);
        let auto_count = auto_cells.len();
        if auto_count == 0 {
            return effective_width;
        }

        let equal_share = remaining / auto_count as i64;
        let total_min: i64 = auto_cells.iter().map(|(_, h, _)| *h).sum();
        if total_min <= remaining && total_min > 0 {
            let surplus = remaining - total_min;
            let (_, my_min, my_max) = auto_cells[self_index];

            // Every auto cell is guaranteed its min hint; the surplus is then
            // distributed in proportion to each cell's growth headroom
            // (max_content - min). Cells whose content cannot use more space —
            // images, &nbsp; spacers, a rank number like "30." — have zero
            // headroom and absorb nothing, while a text column that merely has
            // a long unbreakable word (large min) still grows toward its
            // preferred width. Spec: CSS 2.2 §17.5.2.2 (distribute excess in
            // proportion to the difference between a column's maximum and
            // minimum content widths).
            let my_headroom = (my_max - my_min).max(0);
            let total_headroom: i64 = auto_cells
                .iter()
                .map(|(_, h, mx)| (*mx - *h).max(0))
                .sum();
            if total_headroom > 0 {
                my_min.max(1) + surplus * my_headroom / total_headroom
            } else {
                // No cell can grow: distribute surplus proportionally to mins
                // so the row still fills the table width consistently.
                const DEFAULT_MIN: i64 = 16;
                let total_effective: i64 = auto_cells
                    .iter()
                    .map(|(_, h, _)| (*h).max(DEFAULT_MIN))
                    .sum();
                let my_effective = my_min.max(DEFAULT_MIN);
                let bonus = if total_effective > 0 {
                    surplus * my_effective / total_effective
                } else {
                    0
                };
                my_min.max(1) + bonus
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

    /// Collapse whitespace in a text node per CSS Text §4.1 (simplified):
    /// newlines become spaces and runs of spaces collapse to one. A leading or
    /// trailing space survives only when this text node has an adjacent INLINE
    /// sibling on that side — it is then a word separator between inline boxes
    /// (e.g. `<span>197 points</span> by <a>user</a>`). At block edges the
    /// space is removed, as it would be at a line start/end.
    /// https://www.w3.org/TR/css-text-3/#white-space-phase-2
    /// The visual lines of a text run: honors white-space (pre keeps hard
    /// newlines; pre-wrap additionally wraps; nowrap = single line) — the
    /// sizing and paint passes MUST use this same function so boxes never
    /// wrap their own content.
    pub(crate) fn build_text_lines(
        &self,
        plain_text: &str,
        fs: FontSize,
        bold: bool,
        max_width: i64,
    ) -> Vec<String> {
        use crate::renderer::layout::computed_style::WhiteSpace;
        if self.style.white_space_preserves_newlines() {
            let mut out = Vec::new();
            for seg in plain_text.split('\n') {
                if self.style.white_space() == WhiteSpace::PreWrap && !seg.is_empty() {
                    out.extend(split_text(seg.to_string(), fs, bold, max_width));
                } else {
                    out.push(seg.to_string());
                }
            }
            out
        } else if self.style.white_space_nowrap() {
            vec![plain_text.to_string()]
        } else {
            split_text(plain_text.to_string(), fs, bold, max_width)
        }
    }

    /// Text as it should be measured and painted: whitespace collapsed for
    /// the element's white-space mode, then `text-transform` applied. Sizing
    /// and paint MUST both call this so their line breaks agree.
    pub(crate) fn display_text(&self, t: &str) -> String {
        use crate::renderer::layout::computed_style::TextTransform;
        let collapsed = self.collapse_text_whitespace(t);
        match self.style.text_transform() {
            TextTransform::None => collapsed,
            TextTransform::Uppercase => collapsed.to_uppercase(),
            TextTransform::Lowercase => collapsed.to_lowercase(),
            TextTransform::Capitalize => {
                let mut out = String::with_capacity(collapsed.len());
                let mut at_word_start = true;
                for c in collapsed.chars() {
                    if c.is_whitespace() {
                        at_word_start = true;
                        out.push(c);
                    } else if at_word_start {
                        out.extend(c.to_uppercase());
                        at_word_start = false;
                    } else {
                        out.push(c);
                    }
                }
                out
            }
        }
    }

    pub(crate) fn collapse_text_whitespace(&self, t: &str) -> String {
        // pre / pre-wrap: spaces and newlines are content. Tabs render as
        // 4 spaces (a fixed approximation of tab stops).
        if self.style.white_space_preserves_spaces() {
            return t.replace('\r', "").replace('\t', "    ");
        }
        // pre-line: collapse runs within each line, keep the newlines.
        if self.style.white_space_preserves_newlines() {
            return t
                .replace('\r', "")
                .split('\n')
                .map(|line| {
                    line.split([' ', '\t'])
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .collect::<Vec<_>>()
                .join("\n");
        }
        // Collapse ALL document white space (space, tab, CR, LF, FF) — but
        // never U+00A0 NBSP, which is a rendered character. Tabs matter:
        // tab-indented pages (e.g. Wikipedia) otherwise paint thousands of
        // tab-only "lines" that inflate the page by many screens.
        const WS: [char; 5] = [' ', '\t', '\n', '\r', '\u{c}'];
        let collapsed = t
            .split(WS)
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let had_leading = t.starts_with(WS);
        let had_trailing = t.ends_with(WS);
        if !had_leading && !had_trailing {
            return collapsed;
        }
        // Find this node among its siblings by pointer identity (self may be
        // inside an active borrow during compute_size, so try_borrow-based
        // detection is unreliable here).
        let mut prev_inline = false;
        let mut next_inline = false;
        if let Some(parent) = self.parent.upgrade() {
            let mut prev_kind: Option<LayoutObjectKind> = None;
            let mut child = parent.borrow().first_child();
            while let Some(c) = child {
                if std::ptr::eq(c.as_ptr() as *const LayoutObject, self as *const LayoutObject) {
                    prev_inline = matches!(
                        prev_kind,
                        Some(LayoutObjectKind::Inline) | Some(LayoutObjectKind::Text)
                    );
                    next_inline = self
                        .next_sibling()
                        .map(|n| {
                            matches!(
                                n.borrow().kind(),
                                LayoutObjectKind::Inline | LayoutObjectKind::Text
                            )
                        })
                        .unwrap_or(false);
                    break;
                }
                let b = c.borrow();
                prev_kind = Some(b.kind());
                let next = b.next_sibling();
                drop(b);
                child = next;
            }
        }
        if collapsed.is_empty() {
            // Whitespace-only node: a single separator space when it sits
            // between two inline boxes, nothing otherwise.
            return if prev_inline && next_inline {
                String::from(" ")
            } else {
                String::new()
            };
        }
        let mut result = String::new();
        if had_leading && prev_inline {
            result.push(' ');
        }
        result.push_str(&collapsed);
        if had_trailing && next_inline {
            result.push(' ');
        }
        result
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

    pub(crate) fn placeholder_text(&self) -> Option<String> {
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
                        .max(measure_text_width(&child_text, self.style.font_size(), self.style.is_bold()) + 28),
                    explicit_height.max(36),
                ))
            }
            _ => None,
        }
    }

    fn resolved_width(&self, parent_size: LayoutSize) -> i64 {
        let w = if let Some(ratio) = self.style.width_ratio() {
            edge_to_i64(parent_size.width() as f64 * ratio)
        } else {
            edge_to_i64(self.style.width())
        };
        // border-box: the declared width includes padding+border; callers add
        // inner_horizontal back on, so hand them the content width. Keep at
        // least 1px so the "has an explicit width" signal (w > 0) survives.
        if w > 0 && self.style.is_border_box() {
            (w - compute_box_model_metrics(&self.style).inner_horizontal()).max(1)
        } else {
            w
        }
    }

    fn resolved_height(&self, parent_size: LayoutSize) -> i64 {
        let h = if let Some(ratio) = self.style.height_ratio() {
            edge_to_i64(parent_size.height() as f64 * ratio)
        } else {
            edge_to_i64(self.style.height())
        };
        if h > 0 && self.style.is_border_box() {
            (h - compute_box_model_metrics(&self.style).inner_vertical()).max(1)
        } else {
            h
        }
    }

    /// Stacking-context level for paint ordering: 1 when this box is
    /// positioned OR belongs to a sticky subtree (the sticky context is
    /// stamped onto every descendant, whose own position is Static — without
    /// this, a pinned bar's background would paint over its own text).
    pub(crate) fn stacking_context_level(&self) -> i32 {
        if self.style.position() != PositionType::Static
            || self.style.sticky_context().is_some()
            || self.style.fixed_subtree()
        {
            1
        } else {
            0
        }
    }


    pub fn compute_size(&mut self, parent_size: LayoutSize) {
        let mut size = LayoutSize::new(0, 0);
        let metrics = compute_box_model_metrics(&self.style);

        match self.kind() {
            LayoutObjectKind::Block => {
                let available_width = (parent_size.width() - metrics.outer_horizontal()).max(0);
                let explicit_width = self.resolved_width(parent_size);
                // Also check HTML width attribute for block elements (tables, etc.).
                // Percentages resolve against the available (containing) width so
                // `<table width="85%">` and nested `width="100%"` tables expand to
                // a real width instead of collapsing via shrink-to-fit (which on
                // nested auto tables starves content columns to ~min-content).
                let html_width = parse_dimension_pct_attr(
                    self.element_attribute("width"),
                    Some(available_width),
                );

                // Table cells: use width attribute, or allocate remaining width
                // after subtracting explicitly-sized sibling cells.
                // When total explicit widths exceed available space, scale
                // proportionally to fit.
                let content_width = if self.style.display() == DisplayType::Contents {
                    // No box of its own: span the full containing width so
                    // children's grid-column offsets (resolved against the
                    // real grid ancestor) land where the tracks are.
                    available_width
                } else if self.style.explicit_zero_width() {
                    0
                } else if self.is_table_cell() {
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
                    // Flex item on a single-line row: resolve the main size by
                    // distributing the container's free space per
                    // flex-grow/shrink over the items' base sizes.
                    // https://www.w3.org/TR/css-flexbox-1/#resolve-flexible-lengths
                    self.flex_row_main_size(parent_size.width())
                } else if let Some((tracks, col_gap, _)) = self.parent_grid_info() {
                    // Grid item: the width of its column track(s); the item's
                    // content box is the track minus its own margins. An item
                    // with a grid-area spans that area's columns.
                    let widths = resolve_grid_tracks(&tracks, parent_size.width(), col_gap);
                    let (col, span) = self
                        .grid_area_rect()
                        .map(|(_, _, c, sp)| (c, sp))
                        .unwrap_or((self.grid_item_index() % tracks.len().max(1), 1));
                    let end = (col + span).min(widths.len());
                    let spanned: i64 = widths[col.min(widths.len())..end].iter().sum::<i64>()
                        + col_gap * (span as i64 - 1).max(0);
                    let track = (spanned - metrics.outer_horizontal()).max(0);
                    if explicit_width > 0 {
                        explicit_width.min(track)
                    } else {
                        track
                    }
                } else if explicit_width > 0 {
                    // Inside an overflow container the explicit width stands
                    // even when wider than the box — that overflow is exactly
                    // what the container clips/scrolls. Elsewhere, clamp to
                    // the available width as an overflow guard.
                    let parent_clips = self
                        .parent_object()
                        .map(|p| p.borrow().style().overflow_clip())
                        .unwrap_or(false);
                    if parent_clips {
                        explicit_width
                    } else {
                        explicit_width.min(available_width)
                    }
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
                } else if self.style.float_or_default() != Float::None {
                    // A floated box with `width: auto` shrink-wraps its content
                    // rather than filling the containing block — otherwise it
                    // leaves no room beside itself and the whole point of
                    // floating is lost. On Wikipedia, auto-width floats took the
                    // full 912px column, so every band beside them was zero
                    // wide and the article's text wrapped to nothing.
                    //
                    // Spec: CSS 2.2 §10.3.5 — shrink-to-fit is
                    // min(max(preferred minimum, available), preferred).
                    // https://www.w3.org/TR/CSS22/visudet.html#float-width
                    let preferred = self.max_content_width();
                    let preferred_minimum = self.min_content_width_hint();
                    preferred.min(available_width).max(
                        preferred_minimum.min(available_width),
                    )
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
                // Grid: children fill rows of N columns; the container's height
                // is the sum of each row's tallest item.
                let grid_cols = if self.style.display() == DisplayType::Grid {
                    Some(self.style.grid_columns())
                } else {
                    None
                };
                let grid_row_gap = self.style.row_gap();
                let mut area_row_heights: Vec<i64> = Vec::new();
                let mut area_row_heights_extra: i64 = 0;
                let mut grid_idx = 0usize;
                let mut grid_rows_done = 0usize;
                let mut grid_row_height = 0i64;
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
                    } else if let Some(areas) = self.style.grid_template_areas() {
                        // Areas grid: rows are sized after the loop from the
                        // per-row max of the items placed in them.
                        if !c.borrow().is_whitespace_text() {
                            let b = c.borrow();
                            let m = compute_box_model_metrics(&b.style());
                            let outer_h = b.size.height() + m.margin.top + m.margin.bottom;
                            let rect = b
                                .style()
                                .grid_area_name()
                                .and_then(|nm| Self::area_rect_in(&areas, nm));
                            if let Some((r0, _rspan, _, _)) = rect {
                                if area_row_heights.len() < areas.len() {
                                    area_row_heights.resize(areas.len(), 0);
                                }
                                area_row_heights[r0] = area_row_heights[r0].max(outer_h);
                            } else {
                                area_row_heights_extra = area_row_heights_extra.max(outer_h);
                            }
                        }
                    } else if let Some(n) = grid_cols {
                        // Whitespace-only text children are not grid items.
                        if !c.borrow().is_whitespace_text() {
                            let c_metrics = compute_box_model_metrics(&c.borrow().style());
                            grid_row_height = grid_row_height.max(
                                c.borrow().size.height()
                                    + c_metrics.margin.top
                                    + c_metrics.margin.bottom,
                            );
                            grid_idx += 1;
                            if grid_idx % n == 0 {
                                if grid_rows_done > 0 {
                                    content_height += grid_row_gap;
                                }
                                content_height += grid_row_height;
                                grid_rows_done += 1;
                                grid_row_height = 0;
                            }
                        }
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
                // Areas grid: container height = sum of the area rows.
                if !area_row_heights.is_empty() || area_row_heights_extra > 0 {
                    let rows: i64 = area_row_heights.iter().sum();
                    let gaps = grid_row_gap
                        * (area_row_heights.iter().filter(|h| **h > 0).count() as i64 - 1).max(0);
                    content_height += rows + gaps + area_row_heights_extra;
                }
                // Final, possibly partial grid row.
                if grid_row_height > 0 {
                    if grid_rows_done > 0 {
                        content_height += grid_row_gap;
                    }
                    content_height += grid_row_height;
                }

                // <br> and <hr> have intrinsic heights even without children.
                let content_height = if self.element_kind() == Some(ElementKind::Br) {
                    styled_line_height(&self.style)
                } else if self.element_kind() == Some(ElementKind::Hr) {
                    // <hr> renders as a 2px line with 8px margin above/below.
                    2
                } else {
                    let explicit_height = self.resolved_height(parent_size);
                    if explicit_height > 0 || self.style.explicit_zero_height() {
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
                            // The block child forms its own line: its outer
                            // width (incl. margins) is part of this inline's
                            // width. Without it, an <a> whose only child is a
                            // sized block (HN's votearrow div) measures 0 wide
                            // and centering math pushes the child out of the
                            // cell.
                            let c_metrics = compute_box_model_metrics(&c.borrow().style());
                            max_line_width = max_line_width
                                .max(c_w + c_metrics.margin.left + c_metrics.margin.right);
                        } else {
                            current_line_width += c_w;
                            current_line_height = current_line_height.max(c_h);
                        }
                        child = c.borrow().next_sibling();
                    }
                    // Flush any remaining inline content on the last line.
                    max_line_width = max_line_width.max(current_line_width);
                    content_height += current_line_height;


                    // Explicit CSS width/height take precedence (e.g. a 16×16
                    // sprite icon span has no children, so the content-derived
                    // size would be 0×0 and nothing would paint).
                    let explicit_w = self.resolved_width(parent_size);
                    let explicit_h = self.resolved_height(parent_size);
                    let content_w = if explicit_w > 0 {
                        explicit_w
                    } else if self.style.display() == DisplayType::InlineBlock {
                        // inline-block shrink-wraps: preferred (max-content)
                        // width capped by the containing block.
                        // https://www.w3.org/TR/CSS22/visudet.html#shrink-to-fit-float
                        self.max_content_width()
                            .max(max_line_width)
                            .min(parent_size.width().max(0))
                    } else {
                        max_line_width
                    };
                    let content_h = if explicit_h > 0 {
                        explicit_h
                    } else {
                        content_height
                    };
                    size.set_width((content_w + metrics.inner_horizontal()).max(0));
                    size.set_height((content_h + metrics.inner_vertical()).max(0));
                }
            }
            LayoutObjectKind::Text => {
                if let NodeKind::Text(t) = self.node_kind() {
                    let fs = self.style.font_size();
                    let bold = self.style.is_bold();
                    let cw = bold_width_adjust(char_width_px(fs), bold);
                    let lh = styled_line_height(&self.style);
                    let plain_text = self.display_text(&t);
                    // max_width is the available horizontal space for this text
                    // node within its containing block.  Use the nearest block/cell
                    // ancestor's content width so that inline parents (e.g. <a>,
                    // <strong>) don't artificially narrow the wrapping boundary.
                    // Spec: CSS2.2 §10.3.3 — available width in a block
                    // formatting context.
                    // https://www.w3.org/TR/CSS22/visudet.html#blockwidth
                    let max_width = self.nearest_block_ancestor_width()
                        .map(|w| (w - metrics.outer_horizontal()).max(cw))
                        .unwrap_or_else(||
                            (parent_size.width() - metrics.outer_horizontal()).max(cw)
                        );
                    // Cache so paint() uses the identical boundary (see paint Text arm).
                    self.text_line_max_width = max_width;
                    let lines = self.build_text_lines(&plain_text, fs, bold, max_width);
                    let width = lines
                        .iter()
                        .map(|line| measure_text_width(line, fs, bold))
                        .max()
                        .unwrap_or(0);
                    self.text_line_count = lines.len() as i64;
                    self.text_last_line_width = lines
                        .last()
                        .map(|l| measure_text_width(l, fs, bold))
                        .unwrap_or(0);
                    self.text_line_height = lh;
                    let height = if lines.is_empty() {
                        0
                    } else {
                        lh * lines.len() as i64
                    };
                    size.set_width((width + metrics.inner_horizontal()).max(0));
                    size.set_height((height + metrics.inner_vertical()).max(0));
                }
            }
        }

        // min-/max-width/height clamp the used size (CSS2.2 §10.4/§10.7).
        // Not applied to text runs (their size IS the measured lines) or
        // non-replaced inline boxes, where the properties have no effect.
        if self.style.has_size_limits()
            && !matches!(self.kind, LayoutObjectKind::Text)
            && self.style.display() != DisplayType::Inline
        {
            size.set_width(self.style.clamp_width(size.width(), parent_size.width()));
            size.set_height(self.style.clamp_height(size.height(), parent_size.height()));
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
        // A box placed by the inline formatting context already knows where it
        // goes relative to its containing block; pairwise sibling anchoring
        // would undo that.
        // A float still runs the normal-flow computation below so its flow
        // position stays available; everything else placed by inline layout
        // takes its offset directly.
        let float_placement = if self.style.float_or_default() != Float::None {
            self.inline_offset
        } else {
            None
        };
        if float_placement.is_none() {
            if let Some((dx, dy)) = self.inline_offset {
                self.point.set_x(parent_point.x() + dx);
                self.point.set_y(parent_point.y() + dy);
                return;
            }
        }

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
        } else if let Some((tracks, col_gap, row_gap)) = self.parent_grid_info() {
            let widths = resolve_grid_tracks(&tracks, parent_size.width(), col_gap);
            if let Some((row_start, _row_span, col_start, _col_span)) = self.grid_area_rect() {
                // Area placement: x from the column prefix, y from the
                // heights of the area rows above (each item computes the
                // row table independently from its siblings' sizes).
                let x_offset: i64 = widths[..col_start.min(widths.len())].iter().sum::<i64>()
                    + col_start as i64 * col_gap;
                point.set_x(parent_point.x() + x_offset + metrics.margin.left);
                let row_heights = self.grid_area_row_heights();
                let mut y_off = 0i64;
                for h in row_heights.iter().take(row_start) {
                    if *h > 0 {
                        y_off += h + row_gap;
                    }
                }
                point.set_y(parent_point.y() + y_off + metrics.margin.top);
            } else {
            // Row-major placement into the column tracks.
            let n = tracks.len().max(1);
            let idx = self.grid_item_index();
            let col = idx % n;
            let x_offset: i64 =
                widths[..col.min(widths.len())].iter().sum::<i64>() + col as i64 * col_gap;
            point.set_x(parent_point.x() + x_offset + metrics.margin.left);
            if col == 0 {
                // First track: a new grid row below the previous sibling (which
                // ended the previous row), separated by the row gap.
                if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                    point.set_y(
                        pos.y()
                            + size.height()
                            + metrics.margin.top
                            + metrics.margin.bottom
                            + row_gap,
                    );
                } else {
                    point.set_y(parent_point.y() + metrics.margin.top);
                }
            } else {
                // Same row: share the previous sibling's top edge.
                point.set_y(
                    previous_sibling_point
                        .map(|p| p.y())
                        .unwrap_or(parent_point.y() + metrics.margin.top),
                );
            }
            }
        } else if let Some(dir) = self.parent_flex_direction() {
            // Flex item placement (main-axis start packing; no grow/shrink yet).
            match dir {
                FlexDirection::Row => {
                    // Lay out to the right of the previous item (plus the
                    // container's column-gap), aligned to the container's top;
                    // justify-content/align-items adjust in a post pass.
                    let gap = self
                        .parent
                        .upgrade()
                        .map(|p| p.borrow().style().column_gap())
                        .unwrap_or(0);
                    if let (Some(size), Some(pos)) =
                        (previous_sibling_size, previous_sibling_point)
                    {
                        point.set_x(pos.x() + size.width() + gap + metrics.margin.left);
                    } else {
                        point.set_x(parent_point.x() + metrics.margin.left);
                    }
                    point.set_y(parent_point.y() + metrics.margin.top);
                }
                FlexDirection::Column => {
                    // Stack vertically (plus row-gap) like a block context.
                    let gap = self
                        .parent
                        .upgrade()
                        .map(|p| p.borrow().style().row_gap())
                        .unwrap_or(0);
                    if let (Some(size), Some(pos)) =
                        (previous_sibling_size, previous_sibling_point)
                    {
                        point.set_y(
                            pos.y()
                                + size.height()
                                + gap
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
                    // A wrapped multi-line text sibling ends at its LAST
                    // line's cursor, not at its bounding box's top-right —
                    // continue the inline flow from there.
                    let (mut pos, mut size) = (pos, size);
                    if let Some(prev) = self.previous_layout_sibling() {
                        if let Ok(b) = prev.try_borrow() {
                            if b.text_line_count > 1 {
                                let mut p = b.point();
                                p.set_y(p.y() + (b.text_line_count - 1) * b.text_line_height);
                                pos = p;
                                size =
                                    LayoutSize::new(b.text_last_line_width, b.text_line_height);
                            }
                        }
                    }
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
                        // Same line: top-align with the previous inline box.
                        // (pos.y() already sits below ITS margin-top; adding
                        // our own margin-top on top of that made every
                        // successive sibling creep downward.)
                        point.set_y(pos.y());
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
            // Sticky flows normally; the painter pins it at scroll time.
            PositionType::Static | PositionType::Sticky => {}
            PositionType::Relative => {
                point.set_x(point.x() + self.style.offset_left() as i64);
                point.set_y(point.y() + self.style.offset_top() as i64);
            }
            PositionType::Absolute => {
                // Containing block: the nearest positioned ancestor's content
                // box (CSS2.2 §10.1); the direct parent's box when none.
                let (cb_point, cb_size) = self
                    .absolute_containing_block()
                    .unwrap_or((parent_point, parent_size));
                if self.style.offset_left_author() {
                    let dx = self
                        .style
                        .offset_left_ratio()
                        .map(|r| (cb_size.width() as f64 * r) as i64)
                        .unwrap_or_else(|| self.style.offset_left() as i64);
                    point.set_x(cb_point.x() + dx);
                } else if let Some(r) = self.style.offset_right() {
                    point.set_x(cb_point.x() + cb_size.width() - self.size.width() - r as i64);
                }
                // left/right both auto: keep the static (flow) x.
                if self.style.offset_top_author() {
                    let dy = self
                        .style
                        .offset_top_ratio()
                        .map(|r| (cb_size.height() as f64 * r) as i64)
                        .unwrap_or_else(|| self.style.offset_top() as i64);
                    point.set_y(cb_point.y() + dy);
                } else if let Some(b) = self.style.offset_bottom() {
                    point.set_y(cb_point.y() + cb_size.height() - self.size.height() - b as i64);
                }
            }
            // Fixed: anchored to the viewport origin; the painter additionally
            // exempts it from the scroll offset.
            PositionType::Fixed => {
                point.set_x(edge_to_i64(self.style.offset_left()));
                point.set_y(edge_to_i64(self.style.offset_top()));
            }
        }

        // `clear` pushes an in-flow box below the floats on the cleared side.
        // Spec: CSS 2.2 §9.5.2 — clearance.
        // https://www.w3.org/TR/CSS22/visuren.html#flow-control
        let clear = self.style.clear_or_default();
        if clear != Clear::None && self.style.position() == PositionType::Static {
            if let Some(parent) = self.parent.upgrade() {
                let context = parent.borrow().float_context.clone();
                if let Some(context) = context {
                    let origin = parent.borrow().content_origin();
                    let relative = point.y() - origin.y();
                    let cleared = context.clearance(clear, relative);
                    if cleared > relative {
                        point.set_y(origin.y() + cleared);
                    }
                }
            }
        }

        self.flow_y = point.y();
        if let Some((dx, dy)) = float_placement {
            point.set_x(parent_point.x() + dx);
            point.set_y(parent_point.y() + dy);
        }
        self.point = point;
    }

    pub fn is_node_selected(&self, selector: &Selector) -> bool {
        // Matching runs against the DOM tree, not the layout tree: during the
        // cascade this layout object is not yet linked into its parent's child
        // list, so sibling combinators (and, in principle, ancestors) must be
        // resolved through the fully-built DOM.
        dom_node_selected(&self.node, selector)
    }


    pub fn update_kind(&mut self) {
        match self.node_kind() {
            NodeKind::Document => panic!("should not create a layout object for a document node"),
            NodeKind::Element(_) => match self.style.display() {
                // A flex container is itself a block-level box; flex affects how
                // its children are sized and positioned, not its own outer flow.
                DisplayType::Block | DisplayType::Flex | DisplayType::Grid => {
                    self.kind = LayoutObjectKind::Block
                }
                // display:contents — no box of its own: approximate with a
                // zero-decoration block; placement helpers skip through it.
                DisplayType::Contents => {
                    self.kind = LayoutObjectKind::Block;
                    self.style.set_margin(EdgeSize::zero());
                    self.style.set_padding(EdgeSize::zero());
                    self.style.set_border(EdgeSize::zero());
                }
                // inline-block flows inline; its block-ish sizing is handled
                // in the Inline compute_size arm.
                DisplayType::Inline | DisplayType::InlineBlock => {
                    self.kind = LayoutObjectKind::Inline
                }
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

    /// The DOM node this box was generated from (identity is what the runtime's
    /// transition driver keys its per-element animation state on).
    pub fn node_ref(&self) -> Rc<RefCell<Node>> {
        self.node.clone()
    }

    /// Gather this box's inline-level content as a flat item list for an
    /// inline formatting context, together with the boxes each item came from.
    /// Returns `None` when the box does not establish one — any block-level
    /// child means its children stack vertically and the old path applies.
    ///
    /// Inline *elements* are flattened: their leaf children become the items,
    /// which is how a line can hold text from several elements. An inline box
    /// with no children of its own (an image, a sized span) is atomic.
    /// Spec: CSS 2.2 §9.2.2. https://www.w3.org/TR/CSS22/visuren.html#inline-boxes
    pub(crate) fn collect_inline_items(
        &self,
    ) -> Option<(Vec<Rc<RefCell<LayoutObject>>>, Vec<InlineItem>)> {
        let mut boxes = Vec::new();
        let mut items = Vec::new();
        let mut child = self.first_child();
        while let Some(c) = child {
            // Floats are out of flow: they are placed separately and the lines
            // flow *around* them, so they contribute no inline item — and a
            // floated block child must not disqualify the context either.
            let floated = c.borrow().style.float_or_default() != Float::None;
            if !floated {
                if c.borrow().kind().normal_flow_spec().stacks_vertically {
                    return None;
                }
                if !collect_inline_items_from(&c, &mut boxes, &mut items) {
                    return None;
                }
            }
            let next = c.borrow().next_sibling();
            child = next;
        }
        if items.is_empty() {
            None
        } else {
            Some((boxes, items))
        }
    }

    /// This block's floated children, in document order.
    fn collect_floats(&self) -> Vec<Rc<RefCell<LayoutObject>>> {
        let mut floats = Vec::new();
        let mut child = self.first_child();
        while let Some(c) = child {
            if c.borrow().style.float_or_default() != Float::None {
                floats.push(c.clone());
            }
            let next = c.borrow().next_sibling();
            child = next;
        }
        floats
    }

    /// Lay this block's inline children out as line boxes `content_width` wide,
    /// writing each participating box its position (and, for text, its per-line
    /// fragments). Returns the content height the lines occupy, or `None` when
    /// the box has no inline formatting context to lay out.
    ///
    /// Takes the boxes by handle rather than `&mut self`: collecting the items
    /// reads each child's text, and that reaches back up to this box to decide
    /// whitespace collapsing — which would panic against an outstanding mutable
    /// borrow.
    fn layout_inline_context_inner(
        root: &Rc<RefCell<LayoutObject>>,
        boxes: Vec<Rc<RefCell<LayoutObject>>>,
        items: Vec<InlineItem>,
        content_width: i64,
        options: LineOptions,
        floats: &FloatContext,
    ) -> Option<i64> {
        let lines =
            layout_inline_items_aligned(&items, floats, content_width, 0, 0, options);

        // Fragments per item, in block coordinates, shifted onto their line's
        // shared baseline so items of different heights line up.
        let mut fragments: Vec<Vec<(String, i64, i64)>> = vec![Vec::new(); boxes.len()];
        for line in &lines {
            for fragment in &line.fragments {
                let y = fragment.y + (line.baseline - items[fragment.item].baseline_offset());
                fragments[fragment.item].push((
                    fragment.text.clone().unwrap_or_default(),
                    fragment.x,
                    y,
                ));
            }
        }

        // Every participating box's rect in block coordinates: a leaf's is the
        // union of its fragments, an inline element's the union of everything
        // inside it (so its background/underline covers the whole run).
        let mut rects: Vec<(*const LayoutObject, Rect)> = Vec::new();
        for (index, boxed) in boxes.iter().enumerate() {
            let Some(rect) = fragment_rect(boxed, &fragments[index]) else {
                continue;
            };
            union_into(&mut rects, boxed, rect);
            for ancestor in inline_ancestors(boxed, root) {
                union_into(&mut rects, &ancestor, rect);
            }
        }
        let rect_of = |node: &Rc<RefCell<LayoutObject>>| -> Option<Rect> {
            let key = node.as_ptr() as *const LayoutObject;
            rects.iter().find(|(k, _)| *k == key).map(|(_, r)| *r)
        };

        // Offsets are expressed against each box's *own* parent, because that
        // is what the position pass adds them to: an inline element between a
        // run and the block would otherwise be applied twice.
        let mut assign = |node: &Rc<RefCell<LayoutObject>>| -> Option<Rect> {
            let rect = rect_of(node)?;
            let origin = node
                .borrow()
                .parent
                .upgrade()
                .and_then(|p| rect_of(&p))
                .map(|r| (r.x, r.y))
                .unwrap_or((0, 0));
            node.borrow_mut().inline_offset = Some((rect.x - origin.0, rect.y - origin.1));
            Some(rect)
        };

        for (index, boxed) in boxes.iter().enumerate() {
            let Some(rect) = assign(boxed) else { continue };
            let mut b = boxed.borrow_mut();
            // Fragment positions are stored relative to the box's own origin so
            // paint can emit them without knowing the containing block.
            b.inline_fragments = fragments[index]
                .iter()
                .map(|(text, x, y)| (text.clone(), x - rect.x, y - rect.y))
                .collect();
            if b.kind() == LayoutObjectKind::Text {
                b.size.set_width(rect.width());
                b.size.set_height(rect.height());
                b.text_line_count = fragments[index].len() as i64;
                b.text_line_height = styled_line_height(&b.style);
            }
        }
        for boxed in boxes.iter() {
            for ancestor in inline_ancestors(boxed, root) {
                let Some(rect) = assign(&ancestor) else { continue };
                let mut b = ancestor.borrow_mut();
                b.size.set_width(rect.width());
                b.size.set_height(rect.height());
            }
        }
        Some(lines.iter().map(|l| l.height).sum())
    }

    /// Run the inline formatting context for `block`, if it establishes one,
    /// and give the block the height its lines occupy. Borrows are taken and
    /// released around each step so children can reach back up to the block.
    pub(crate) fn layout_inline_context(block: &Rc<RefCell<LayoutObject>>) -> bool {
        Self::layout_inline_context_with(block, None)
    }

    /// As [`layout_inline_context`], starting from `inherited` — the floats of
    /// an enclosing block formatting context, already translated into this
    /// box's coordinates.
    pub(crate) fn layout_inline_context_with(
        block: &Rc<RefCell<LayoutObject>>,
        inherited: Option<&FloatContext>,
    ) -> bool {
        let is_flex_or_grid_item = block
            .borrow()
            .parent
            .upgrade()
            .map(|p| {
                let p = p.borrow();
                p.is_flex_container() || p.style.display() == DisplayType::Grid
            })
            .unwrap_or(false);
        {
            let b = block.borrow();
            // Flex and grid items are blockified whatever their own `display`
            // says, so an inline item still establishes an inline formatting
            // context for its contents. Without this a `<span>` flex item kept
            // the legacy path, where its inline content overlaps.
            // Spec: CSS Flexbox §4 / CSS Grid §6 — blockification.
            // https://www.w3.org/TR/css-flexbox-1/#flex-items
            if b.kind() != LayoutObjectKind::Block && !is_flex_or_grid_item {
                return false;
            }
            // Contexts this pass does not own yet:
            //  - tables size their columns from content, so re-breaking text at
            //    the assigned cell width hard-breaks words the column algorithm
            //    sized to fit;
            //  - (list items are fine: their marker is painted by the item
            //    itself, outside its content box, not as a child in the flow.)
            if b.is_table() || b.is_table_row() || b.is_row_group() {
                return false;
            }
            // A flex or grid container's children are flex/grid items — they
            // are blockified regardless of their `display`, so a container of
            // inline `<span>`s is emphatically NOT an inline formatting
            // context. Spec: CSS Flexbox §4 / Grid §6 (blockification).
            if b.is_flex_container()
                || matches!(
                    b.style.display(),
                    DisplayType::Grid | DisplayType::Contents
                )
            {
                return false;
            }
        }
        let content_width = block.borrow().content_size().width();
        let Some((boxes, items)) = block.borrow().collect_inline_items() else {
            return false;
        };
        // Floats from an enclosing block formatting context shorten this box's
        // lines. They are stored on the BFC root in its own coordinates, so
        // they are re-expressed against this box's content origin.
        let inherited = inherited.cloned().or_else(|| ancestor_float_context(block));
        let options = LineOptions {
            align: match block.borrow().style.text_align() {
                TextAlign::Center => LineAlign::Center,
                TextAlign::Right => LineAlign::End,
                _ => LineAlign::Start,
            },
            wrap: !block.borrow().style.white_space_nowrap(),
        };
        // Place this context's floats first: the lines below then flow around
        // them, because line breaking asks the same context for each line's
        // usable span. Spec: CSS 2.2 §9.5. Floats declared by an ancestor in
        // the same formatting context are inherited (already re-expressed in
        // this box's coordinates).
        // This box's own floats, kept apart from the inherited ones: only the
        // former decide how tall a block formatting context must be to contain
        // them. Mixing them made a box stretch to reach an ancestor's float —
        // which on a page with floats far down the document meant heights in
        // the hundreds of thousands of pixels.
        let mut own_floats = FloatContext::new(content_width);
        Self::place_floats_into(block, &mut own_floats);
        let mut floats = inherited.unwrap_or_else(|| FloatContext::new(content_width));
        for placed in own_floats.placed() {
            floats.adopt(*placed);
        }
        let Some(height) =
            Self::layout_inline_context_inner(block, boxes, items, content_width, options, &floats)
        else {
            return false;
        };
        let mut b = block.borrow_mut();
        // An explicit height wins over the content's own.
        if b.style.has_author_height() {
            return true;
        }
        // A box that establishes a block formatting context contains its
        // floats; an ordinary block lets them overflow (CSS 2.2 §10.6.7).
        let height = if b.establishes_block_formatting_context() {
            height.max(own_floats.lowest_bottom())
        } else {
            height
        };
        let metrics = compute_box_model_metrics(&b.style);
        // Table cells carry cellpadding on top of the box model's own padding
        // (see the Block arm of compute_size); content_origin/content_size
        // inset children by it, so the outer box has to include it here too.
        let cellpadding = if b.is_table_cell() {
            2 * b.ancestor_table_cellpadding()
        } else {
            0
        };
        b.size
            .set_height((height + metrics.inner_vertical() + cellpadding).max(0));
        true
    }

    /// Place a single floated box into `context` at or below `top`.
    fn place_one_float(
        child: &Rc<RefCell<LayoutObject>>,
        context: &mut FloatContext,
        top: i64,
    ) {
        let (side, width, height, clear, margin) = {
            let c = child.borrow();
            let Some(side) = FloatSide::from_float(c.style.float_or_default()) else {
                return;
            };
            let metrics = compute_box_model_metrics(&c.style);
            (
                side,
                c.size().width() + metrics.margin.left + metrics.margin.right,
                c.size().height() + metrics.margin.top + metrics.margin.bottom,
                c.style.clear_or_default(),
                (metrics.margin.left, metrics.margin.top),
            )
        };
        let placed = context.place(side, width, height, top, clear);
        child.borrow_mut().inline_offset = Some((placed.x + margin.0, placed.y + margin.1));
    }

    /// Place each floated child into `context`, giving it its position. Sizes
    /// come from the size pass, which has already run for the children.
    fn place_floats_into(block: &Rc<RefCell<LayoutObject>>, context: &mut FloatContext) {
        Self::place_floats_at(block, context, 0);
    }

    /// As `place_floats_into`, placing no higher than `top`.
    fn place_floats_at(
        block: &Rc<RefCell<LayoutObject>>,
        context: &mut FloatContext,
        top: i64,
    ) {
        for child in block.borrow().collect_floats() {
            Self::place_one_float(&child, context, top);
        }
    }

    /// Whether this box establishes a block formatting context: floats inside
    /// it are contained by it, and floats outside never intrude.
    ///
    /// Spec: CSS 2.2 §9.4.1 — the root element, floats, absolutely positioned
    /// boxes, inline-blocks, table cells, and boxes with `overflow` other than
    /// `visible` all establish one. Flex and grid containers do too (their
    /// children are not in a block formatting context at all).
    /// https://www.w3.org/TR/CSS22/visuren.html#block-formatting
    pub fn establishes_block_formatting_context(&self) -> bool {
        use crate::renderer::layout::computed_style::{DisplayType, Float, PositionType};
        if self.parent.upgrade().is_none() {
            return true; // the layout-tree root
        }
        if self.style.float_or_default() != Float::None {
            return true;
        }
        if matches!(
            self.style.position(),
            PositionType::Absolute | PositionType::Fixed
        ) {
            return true;
        }
        if self.style.overflow_clip() || self.style.overflow_scrollable() {
            return true;
        }
        // Table cells establish one too, but this engine has no cell kind —
        // cells are Blocks tagged by their element, so check the node.
        if matches!(
            self.node.borrow().element_kind(),
            Some(ElementKind::Td) | Some(ElementKind::Th)
        ) {
            return true;
        }
        matches!(
            self.style.display(),
            DisplayType::InlineBlock | DisplayType::Flex | DisplayType::Grid
        )
    }

    /// A detached box over the same node carrying `style`, for evaluating
    /// declarations against this element's computed style without disturbing
    /// the real one — `@keyframes` blocks are resolved this way, so they go
    /// through the ordinary cascade.
    pub fn scratch_with_style(&self, style: ComputedStyle) -> LayoutObject {
        let mut scratch = LayoutObject::new(self.node.clone(), &None);
        scratch.style = style;
        scratch
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

    /// Overwrite the computed position (used by post-layout passes such as
    /// fixed-position far-edge anchoring).
    pub fn set_point(&mut self, point: LayoutPoint) {
        self.point = point;
    }

    /// Stamp the sticky scroll context onto this node's style (see
    /// `LayoutView::stamp_sticky_contexts`).
    pub fn set_sticky_context(&mut self, top: f64, container_y: f64, max_delta: f64) {
        self.style.set_sticky_context(top, container_y, max_delta);
    }

    /// Mark this node as part of a position:fixed subtree (see
    /// `LayoutView::stamp_sticky_contexts`).
    pub fn set_fixed_subtree(&mut self) {
        self.style.set_fixed_subtree();
    }

    /// Upgraded parent layout object, when still alive.
    pub fn parent_object(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        self.parent.upgrade()
    }

    /// Stamp the final paint-order key (see `LayoutView::stamp_sticky_contexts`).
    pub fn set_paint_z(&mut self, z: i32) {
        self.style.set_paint_z(z);
    }

    /// Stamp a scale context (see `LayoutView::apply_transforms`).
    pub fn set_scale_context(&mut self, ox: f64, oy: f64, factor: f64) {
        self.style.set_scale_context(ox, oy, factor);
    }

    /// Stamp a rotation context (see `LayoutView::apply_transforms`).
    pub fn set_rotate_context(&mut self, cx: f64, cy: f64, deg: f64) {
        self.style.set_rotate_context(cx, cy, deg);
    }

    /// Stamp the final clip rectangle (intersection of overflow ancestors).
    pub fn set_final_clip(&mut self, clip: (f64, f64, f64, f64)) {
        self.style.set_final_clip(clip);
    }

    /// Stamp the nearest scroll-container id for this box's content.
    pub fn set_scroll_container(&mut self, id: u32) {
        self.style.set_scroll_container(id);
    }

    /// Mark this box as a scroll container (id + scrollable content size).
    pub fn set_scroll_container_def(&mut self, id: u32, content_w: f64, content_h: f64) {
        self.style.set_scroll_container_def(id, content_w, content_h);
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


/// The floats of the nearest ancestor block formatting context, translated
/// into `block`'s own coordinates. `None` when no ancestor has any.
fn ancestor_float_context(block: &Rc<RefCell<LayoutObject>>) -> Option<FloatContext> {
    let own_origin = block.borrow().content_origin();
    let mut ancestor = block.borrow().parent.upgrade();
    while let Some(a) = ancestor {
        let context = a.borrow().float_context.clone();
        if let Some(context) = context {
            let origin = a.borrow().content_origin();
            // The context is in the BFC root's coordinates; this box starts
            // `dy` below it.
            return Some(context.translated(own_origin.y() - origin.y()));
        }
        if a.borrow().establishes_block_formatting_context() {
            // A BFC root with no floats blocks the search: floats never cross
            // a formatting context boundary.
            return None;
        }
        let next = a.borrow().parent.upgrade();
        ancestor = next;
    }
    None
}

/// Whether the Phase 2.5 inline formatting context replaces the legacy
/// per-text-node wrapping. On by default; `COSMO_LEGACY_INLINE=1` (or
/// `COSMO_NEW_INLINE=0`) restores the old path for A/B comparison.
pub(crate) fn use_new_inline() -> bool {
    if std::env::var("COSMO_LEGACY_INLINE").as_deref() == Ok("1") {
        return false;
    }
    std::env::var("COSMO_NEW_INLINE").as_deref() != Ok("0")
}

/// Walk one inline-level box into the item list, flattening inline elements.
/// Returns false when something block-level turns up (the caller falls back to
/// the legacy path).
fn collect_inline_items_from(
    node: &Rc<RefCell<LayoutObject>>,
    boxes: &mut Vec<Rc<RefCell<LayoutObject>>>,
    items: &mut Vec<InlineItem>,
) -> bool {
    let kind = node.borrow().kind();
    if kind.normal_flow_spec().stacks_vertically {
        return false;
    }
    if kind == LayoutObjectKind::Text {
        {
            // Preserved white-space is handled here (newlines become
            // mandatory breaks and the run's own spaces survive collapsing),
            // `nowrap` makes the run unbreakable, and paint truncates for
            // `text-overflow: ellipsis` once fragments have positions.
            let _ = &node;
        }
        let text = match node.borrow().node_kind() {
            NodeKind::Text(t) => node.borrow().display_text(&t),
            _ => String::new(),
        };
        if text.is_empty() {
            return true;
        }
        let b = node.borrow();
        boxes.push(node.clone());
        items.push(InlineItem::Text(TextRun {
            text,
            font_size: b.style.font_size(),
            bold: b.style.is_bold(),
            line_height: styled_line_height(&b.style),
            breakable: !b.style.white_space_nowrap(),
            hard_breaks: b.style.white_space_preserves_newlines(),
            preserves_spaces: b.style.white_space_preserves_spaces(),
        }));
        return true;
    }
    // An inline box with children contributes its children; one without (an
    // image, a sized span) is atomic.
    let first_child = node.borrow().first_child();
    if first_child.is_none() {
        let b = node.borrow();
        let size = b.size();
        boxes.push(node.clone());
        items.push(InlineItem::Atomic {
            width: size.width(),
            height: size.height(),
            baseline: size.height(),
        });
        return true;
    }
    let mut child = first_child;
    while let Some(c) = child {
        if !collect_inline_items_from(&c, boxes, items) {
            return false;
        }
        let next = c.borrow().next_sibling();
        child = next;
    }
    true
}

/// A rect in the inline formatting context's (block-relative) coordinates.
#[derive(Debug, Clone, Copy)]
struct Rect {
    x: i64,
    y: i64,
    right: i64,
    bottom: i64,
}

impl Rect {
    fn width(&self) -> i64 {
        (self.right - self.x).max(0)
    }

    fn height(&self) -> i64 {
        (self.bottom - self.y).max(0)
    }

    fn union(self, other: Rect) -> Rect {
        Rect {
            x: self.x.min(other.x),
            y: self.y.min(other.y),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

/// The union of `placed`, measured in `node`'s own font for text.
fn fragment_rect(node: &Rc<RefCell<LayoutObject>>, placed: &[(String, i64, i64)]) -> Option<Rect> {
    let first = placed.first()?;
    let b = node.borrow();
    let is_text = b.kind() == LayoutObjectKind::Text;
    let (font_size, bold) = (b.style.font_size(), b.style.is_bold());
    let line_height = styled_line_height(&b.style);
    let (own_width, own_height) = (b.size().width(), b.size().height());
    drop(b);
    let extent = |text: &str| {
        if is_text {
            (measure_text_width(text, font_size, bold), line_height)
        } else {
            (own_width, own_height)
        }
    };
    let (w, h) = extent(&first.0);
    let mut rect = Rect {
        x: first.1,
        y: first.2,
        right: first.1 + w,
        bottom: first.2 + h,
    };
    for (text, x, y) in placed.iter().skip(1) {
        let (w, h) = extent(text);
        rect = rect.union(Rect {
            x: *x,
            y: *y,
            right: x + w,
            bottom: y + h,
        });
    }
    Some(rect)
}

/// Merge `rect` into `node`'s entry.
fn union_into(
    rects: &mut Vec<(*const LayoutObject, Rect)>,
    node: &Rc<RefCell<LayoutObject>>,
    rect: Rect,
) {
    let key = node.as_ptr() as *const LayoutObject;
    match rects.iter_mut().find(|(k, _)| *k == key) {
        Some((_, existing)) => *existing = existing.union(rect),
        None => rects.push((key, rect)),
    }
}

/// The inline element boxes between `node` and the box establishing its inline
/// formatting context.
///
/// Stopping at `root` matters as much as stopping at a block: an inline-kind
/// root (a `<span>` flex item, which is blockified) would otherwise be handed
/// an `inline_offset` of its own, and that overrides the position pass — which
/// is where flex placement happens. Every chip then landed on the containing
/// block's origin, stacked on top of each other.
fn inline_ancestors(
    node: &Rc<RefCell<LayoutObject>>,
    root: &Rc<RefCell<LayoutObject>>,
) -> Vec<Rc<RefCell<LayoutObject>>> {
    let mut chain = Vec::new();
    let mut ancestor = node.borrow().parent.upgrade();
    while let Some(a) = ancestor {
        if Rc::ptr_eq(&a, root) || a.borrow().kind() == LayoutObjectKind::Block {
            break;
        }
        chain.push(a.clone());
        let next = a.borrow().parent.upgrade();
        ancestor = next;
    }
    chain
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
