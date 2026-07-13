//! The cascade: applying matched declarations onto ComputedStyle, plus the
//! defaulting (inheritance) pass. Extracted verbatim from layout_object.rs
//! (plan 0.5); property parsing will move into a registry in plan 1.3.

use crate::renderer::css::cssom::ComponentValue;
use crate::renderer::css::cssom::Declaration;
use crate::renderer::dom::node::Node;
use std::rc::Rc;
use std::cell::RefCell;
use crate::renderer::layout::computed_style::*;
use crate::renderer::layout::layout_object::LayoutObject;
use crate::renderer::style::values::*;

impl LayoutObject {
    pub fn cascading_style(&mut self, declarations: Vec<Declaration>, parent_font_size: FontSize) {
        use crate::renderer::css::cssom::{substitute_vars, value_has_var};
        let custom_scope = self.style.custom_properties().cloned();
        for mut declaration in declarations {
            // Custom-property definitions are collected into the element's
            // scope before the cascade (create_layout_object); they are not
            // style properties themselves.
            if declaration.property.starts_with("--") {
                continue;
            }
            // var() references resolve against this element's custom-property
            // scope at computed-value time. CSS Variables §3.
            if value_has_var(&declaration.value) {
                if let Some(scope) = &custom_scope {
                    declaration.value = substitute_vars(&declaration.value, scope);
                }
            }
            let first_value = declaration.first_value();
            match declaration.property.as_str() {
                "background-color" | "background" => {
                    match first_value {
                        Some(ComponentValue::Ident(value)) => {
                            let color = Color::from_name(value).unwrap_or_else(|_| Color::white());
                            self.style.set_background_color(color);
                        }
                        Some(ComponentValue::HashToken(color_code)) => {
                            let color =
                                Color::from_code(color_code).unwrap_or_else(|_| Color::white());
                            self.style.set_background_color(color);
                        }
                        _ => {}
                    }
                    // The background shorthand may also carry an image layer,
                    // a position, and a repeat keyword.
                    if declaration.property == "background" {
                        if let Some(url) = extract_css_url(&declaration.value) {
                            self.style.set_background_image(url);
                        }
                        let (pos, no_repeat, size) =
                            scan_background_shorthand(&declaration.value);
                        if let Some((x, xp, y, yp)) = pos {
                            self.style.set_background_position(x, xp, y, yp);
                        }
                        if no_repeat {
                            self.style.set_background_no_repeat(true);
                        }
                        if let Some(size) = size {
                            self.style.set_background_size(size);
                        }
                    }
                }
                "background-image" => {
                    if let Some(url) = extract_css_url(&declaration.value) {
                        self.style.set_background_image(url);
                    }
                }
                "background-position" => {
                    let comps: Vec<(f64, bool, Option<bool>)> = declaration
                        .value
                        .iter()
                        .filter_map(bg_position_component)
                        .take(2)
                        .collect();
                    if let Some((x, xp, y, yp)) = assemble_bg_position(&comps) {
                        self.style.set_background_position(x, xp, y, yp);
                    }
                }
                "background-repeat" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style
                            .set_background_no_repeat(value.eq_ignore_ascii_case("no-repeat"));
                    }
                }
                "background-size" => {
                    if let Some(size) = parse_background_size(&declaration.value) {
                        self.style.set_background_size(size);
                    }
                }
                "line-height" => match first_value {
                    // A bare number is a factor of the element's own font size.
                    Some(ComponentValue::Number(v)) if *v > 0.0 => {
                        self.style.set_line_height(LineHeight::Factor(*v));
                    }
                    Some(ComponentValue::Dimension(v, unit)) => {
                        if unit == "%" {
                            if *v > 0.0 {
                                self.style.set_line_height(LineHeight::Factor(*v / 100.0));
                            }
                        } else if let Some(px) =
                            length_to_px(*v, unit, self.style.font_size_or_default())
                        {
                            if px > 0.0 {
                                self.style.set_line_height(LineHeight::Px(px));
                            }
                        }
                    }
                    // `normal` resets to the default leading.
                    Some(ComponentValue::Ident(v)) if v.eq_ignore_ascii_case("normal") => {
                        self.style.set_line_height(LineHeight::Factor(1.25));
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
                "grid-template-columns" => {
                    let tracks = parse_grid_template_tracks(&declaration.value);
                    self.style.set_grid_template_columns(tracks);
                }
                // gap shorthand: one value = both axes, two = row then column.
                // https://www.w3.org/TR/css-align-3/#gap-shorthand
                "gap" | "grid-gap" => {
                    let px: Vec<f64> = declaration
                        .value
                        .iter()
                        .filter_map(|v| spacing_component_to_px(v, self.style.font_size_or_default()))
                        .collect();
                    match px.as_slice() {
                        [both] => {
                            self.style.set_row_gap(*both);
                            self.style.set_column_gap(*both);
                        }
                        [row, column, ..] => {
                            self.style.set_row_gap(*row);
                            self.style.set_column_gap(*column);
                        }
                        _ => {}
                    }
                }
                "column-gap" | "grid-column-gap" => {
                    if let Some(px) = first_value
                        .and_then(|v| spacing_component_to_px(v, self.style.font_size_or_default()))
                    {
                        self.style.set_column_gap(px);
                    }
                }
                "row-gap" | "grid-row-gap" => {
                    if let Some(px) = first_value
                        .and_then(|v| spacing_component_to_px(v, self.style.font_size_or_default()))
                    {
                        self.style.set_row_gap(px);
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
                    Some(ComponentValue::Dimension(value, unit)) if unit == "%" => {
                        self.style.set_offset_top_ratio(*value / 100.0)
                    }
                    _ => {}
                },
                "left" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_offset_left(*value),
                    Some(ComponentValue::Dimension(value, unit)) if unit == "px" => {
                        self.style.set_offset_left(*value)
                    }
                    Some(ComponentValue::Dimension(value, unit)) if unit == "%" => {
                        self.style.set_offset_left_ratio(*value / 100.0)
                    }
                    _ => {}
                },
                "right" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_offset_right(*value),
                    Some(ComponentValue::Dimension(value, unit)) if unit == "px" => {
                        self.style.set_offset_right(*value)
                    }
                    _ => {}
                },
                "bottom" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_offset_bottom(*value),
                    Some(ComponentValue::Dimension(value, unit)) if unit == "px" => {
                        self.style.set_offset_bottom(*value)
                    }
                    _ => {}
                },
                "z-index" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_z_index(*value as i32),
                    _ => {}
                },
                // overflow: scroll/auto clip like hidden — without interactive
                // inner scrolling (a renderer-side feature), clipping at the
                // box edge is exactly what an unscrolled scroll container
                // shows. `visible` (or unknown values) leaves content unclipped.
                "overflow" | "overflow-x" | "overflow-y" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style.set_overflow_clip(matches!(
                            value.as_str(),
                            "hidden" | "clip" | "scroll" | "auto"
                        ));
                        self.style.set_overflow_scrollable(matches!(
                            value.as_str(),
                            "scroll" | "auto"
                        ));
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
                    // The `border` shorthand also carries a color (and style):
                    // pull a color token so the stroke is visible.
                    if declaration.property == "border" {
                        for v in &declaration.value {
                            let c = match v {
                                ComponentValue::HashToken(code) => Color::from_code(code).ok(),
                                ComponentValue::Ident(name) => Color::from_name(name).ok(),
                                _ => None,
                            };
                            if let Some(color) = c {
                                self.style.set_border_color(color);
                                break;
                            }
                        }
                    }
                }
                "border-color" => match first_value {
                    Some(ComponentValue::HashToken(code)) => {
                        if let Ok(c) = Color::from_code(code) {
                            self.style.set_border_color(c);
                        }
                    }
                    Some(ComponentValue::Ident(name)) => {
                        if let Ok(c) = Color::from_name(name) {
                            self.style.set_border_color(c);
                        }
                    }
                    _ => {}
                },
                "border-radius" => {
                    if let Some(px) = first_value
                        .and_then(|v| spacing_component_to_px(v, self.style.font_size_or_default()))
                    {
                        self.style.set_border_radius(px);
                    }
                }
                "white-space" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        // nowrap and pre suppress automatic wrapping at spaces;
                        // normal/pre-wrap/pre-line wrap.
                        self.style.set_white_space_nowrap(matches!(
                            value.as_str(),
                            "nowrap" | "pre"
                        ));
                    }
                }
                "text-overflow" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style
                            .set_text_overflow_ellipsis(value.eq_ignore_ascii_case("ellipsis"));
                    }
                }
                // box-shadow: <dx> <dy> [blur] [spread] <color> (single
                // shadow; inset and shadow lists are ignored).
                "box-shadow" => {
                    let mut lengths: Vec<f64> = Vec::new();
                    let mut color: Option<Color> = None;
                    for v in &declaration.value {
                        match v {
                            ComponentValue::Ident(name)
                                if name.eq_ignore_ascii_case("none")
                                    || name.eq_ignore_ascii_case("inset") =>
                            {
                                lengths.clear();
                                color = None;
                                break;
                            }
                            ComponentValue::HashToken(code) => {
                                color = Color::from_code(code).ok();
                            }
                            ComponentValue::Ident(name) => {
                                if let Ok(c) = Color::from_name(name) {
                                    color = Some(c);
                                }
                            }
                            other => {
                                if lengths.len() < 4 {
                                    if let Some(px) = spacing_component_to_px(
                                        other,
                                        self.style.font_size_or_default(),
                                    ) {
                                        lengths.push(px);
                                    }
                                }
                            }
                        }
                    }
                    if lengths.len() >= 2 {
                        let blur = lengths.get(2).copied().unwrap_or(0.0);
                        let c = color.unwrap_or_else(Color::gray);
                        self.style.set_box_shadow(lengths[0], lengths[1], blur, c);
                    }
                }
                // transform values are not applied (no transform rendering),
                // but a non-none transform forms a stacking context.
                // https://www.w3.org/TR/css-transforms-1/#transform-rendering
                "transform" => {
                    let is_none = matches!(first_value,
                        Some(ComponentValue::Ident(v)) if v.eq_ignore_ascii_case("none"));
                    self.style
                        .set_has_transform(!is_none && first_value.is_some());
                    if !is_none {
                        if let Some(op) = parse_transform_ops(&declaration.value) {
                            self.style.set_transform_op(op);
                        }
                        if let Some(deg) = parse_transform_rotate(&declaration.value) {
                            self.style.set_transform_rotate(deg);
                        }
                    }
                }
                "opacity" => {
                    if let Some(ComponentValue::Number(value)) = first_value {
                        self.style.set_opacity(*value);
                    }
                }
                "font-weight" => match first_value {
                    // bolder/lighter are resolved as their common effect
                    // (relative-to-parent weights need real weight tracking).
                    Some(ComponentValue::Ident(value)) => match value.as_str() {
                        "bold" | "bolder" => self.style.set_bold(true),
                        "normal" | "lighter" => self.style.set_bold(false),
                        _ => {}
                    },
                    Some(ComponentValue::Number(value)) => {
                        self.style.set_bold(*value >= 600.0);
                    }
                    _ => {}
                },
                "visibility" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        match value.as_str() {
                            "hidden" | "collapse" => self.style.set_visibility_hidden(true),
                            "visible" => self.style.set_visibility_hidden(false),
                            _ => {}
                        }
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
                        // font-size em and % resolve against the PARENT's font
                        // size (CSS 2.2 §15.7); rem and absolute units resolve
                        // via length_to_px against the 16px root default.
                        let px = match unit.as_str() {
                            "em" => Some(*value * parent_font_size.px() as f64),
                            "%" => Some(*value / 100.0 * parent_font_size.px() as f64),
                            _ => length_to_px(*value, unit, parent_font_size),
                        };
                        if let Some(px) = px {
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
}
