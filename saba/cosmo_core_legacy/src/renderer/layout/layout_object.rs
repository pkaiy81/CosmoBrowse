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

fn measure_text_width(text: &str, font_size: FontSize) -> i64 {
    text.len() as i64 * CHAR_WIDTH * font_ratio(font_size)
}
fn find_index_for_line_break(line: String, max_index: usize) -> usize {
    let chars = line.chars().collect::<Vec<char>>();
    let upper = max_index.min(chars.len().saturating_sub(1));
    for i in (0..=upper).rev() {
        if chars[i] == ' ' {
            return i;
        }
    }
    max_index
}

fn split_text(line: String, char_width: i64, max_width: i64) -> Vec<String> {
    let mut result: Vec<String> = vec![];
    let safe_width = max_width.max(char_width).max(1);
    if line.len() as i64 * char_width > safe_width {
        let split_index =
            find_index_for_line_break(line.clone(), (safe_width / char_width).max(1) as usize);
        let s = line.split_at(split_index.min(line.len()));
        result.push(s.0.to_string());
        result.extend(split_text(s.1.trim().to_string(), char_width, safe_width));
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

    fn element_attribute(&self, name: &str) -> Option<String> {
        match self.node.borrow().kind() {
            NodeKind::Element(ref element) => element.get_attribute(name),
            _ => None,
        }
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

                    if let Some(text) = self.placeholder_text() {
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
                                self.point().y() + CHAR_HEIGHT_WITH_PADDING * i as i64,
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
        let margin = self.style.margin();
        let padding = self.style.padding();
        let margin_h = edge_to_i64(margin.horizontal());
        let padding_h = edge_to_i64(padding.horizontal());
        let padding_v = edge_to_i64(padding.vertical());

        match self.kind() {
            LayoutObjectKind::Block => {
                let available_width = (parent_size.width() - margin_h).max(0);
                let explicit_width = self.resolved_width(parent_size);
                let width = if explicit_width > 0 {
                    explicit_width.min(available_width)
                } else {
                    available_width
                };

                let mut height = padding_v;
                let mut child = self.first_child();
                let mut previous_child_kind = LayoutObjectKind::Block;
                while child.is_some() {
                    let c = child.expect("first child should exist");
                    if previous_child_kind == LayoutObjectKind::Block
                        || c.borrow().kind() == LayoutObjectKind::Block
                    {
                        height += c.borrow().size.height();
                    } else {
                        height = height.max(c.borrow().size.height() + padding_v);
                    }
                    previous_child_kind = c.borrow().kind();
                    child = c.borrow().next_sibling();
                }

                let explicit_height = self.resolved_height(parent_size);
                size.set_width(width.max(0));
                size.set_height(if explicit_height > 0 {
                    explicit_height
                } else {
                    height.max(0)
                });
            }
            LayoutObjectKind::Inline => {
                if let Some(intrinsic) = self.intrinsic_inline_size(parent_size) {
                    size.set_width((intrinsic.width() + padding_h).max(0));
                    size.set_height((intrinsic.height() + padding_v).max(0));
                } else {
                    let mut width = padding_h;
                    let mut height = padding_v;
                    let mut child = self.first_child();
                    while child.is_some() {
                        let c = child.expect("child should exist");
                        width += c.borrow().size.width();
                        height = height.max(c.borrow().size.height() + padding_v);
                        child = c.borrow().next_sibling();
                    }

                    size.set_width(width.max(0));
                    size.set_height(height.max(0));
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
                        (parent_size.width() - margin_h - padding_h).max(CHAR_WIDTH * ratio);
                    let lines = split_text(plain_text.clone(), CHAR_WIDTH * ratio, max_width);
                    let width = lines
                        .iter()
                        .map(|line| line.len() as i64 * CHAR_WIDTH * ratio)
                        .max()
                        .unwrap_or(0);
                    let height = if lines.is_empty() {
                        0
                    } else {
                        CHAR_HEIGHT_WITH_PADDING * ratio * lines.len() as i64
                    };
                    size.set_width(width.max(0));
                    size.set_height(height.max(0));
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
        let margin = self.style.margin();
        let margin_left = edge_to_i64(margin.left());
        let margin_right = edge_to_i64(margin.right());
        let margin_top = edge_to_i64(margin.top());
        let margin_bottom = edge_to_i64(margin.bottom());

        match (self.kind(), previous_sibling_kind) {
            (LayoutObjectKind::Block, _) | (_, LayoutObjectKind::Block) => {
                if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                    point.set_y(pos.y() + size.height() + margin_top + margin_bottom);
                } else {
                    point.set_y(parent_point.y() + margin_top);
                }

                // Ref: CSS 2.1 §10.3.3, auto horizontal margins center a block when width is known.
                // https://www.w3.org/TR/CSS21/visudet.html#blockwidth
                let available_width = parent_size.width() - self.size.width();
                if self.style.margin_horizontal_auto() && available_width > 0 {
                    point.set_x(parent_point.x() + available_width / 2);
                } else if self.style.margin_left_auto() && available_width > margin_right {
                    point.set_x(parent_point.x() + available_width - margin_right);
                } else {
                    point.set_x(parent_point.x() + margin_left);
                }
            }
            (LayoutObjectKind::Inline, LayoutObjectKind::Inline) => {
                if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                    point.set_x(pos.x() + size.width() + margin_left);
                    point.set_y(pos.y() + margin_top);
                } else {
                    point.set_x(parent_point.x() + margin_left);
                    point.set_y(parent_point.y() + margin_top);
                }
            }
            _ => {
                point.set_x(parent_point.x() + margin_left);
                point.set_y(parent_point.y() + margin_top);
            }
        }

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
                        parse_spacing_shorthand(&declaration.value, base_font_size)
                    {
                        self.style.set_margin(
                            crate::renderer::layout::computed_style::EdgeSize::from_values(
                                top, right, bottom, left,
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
        let padding = self.style.padding();
        LayoutPoint::new(
            self.point.x() + edge_to_i64(padding.left()),
            self.point.y() + edge_to_i64(padding.top()),
        )
    }

    pub fn content_size(&self) -> LayoutSize {
        let padding = self.style.padding();
        LayoutSize::new(
            (self.size.width() - edge_to_i64(padding.horizontal())).max(0),
            (self.size.height() - edge_to_i64(padding.vertical())).max(0),
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
