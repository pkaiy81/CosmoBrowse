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
            // Fold resolvable calc() into plain px so every property arm
            // sees ordinary Dimension tokens.
            if declaration
                .value
                .iter()
                .any(|v| matches!(v, ComponentValue::Ident(s) if s.eq_ignore_ascii_case("calc")))
            {
                declaration.value =
                    fold_calc(&declaration.value, self.style.font_size_or_default());
            }
            let first_value = declaration.first_value();
            match declaration.property.as_str() {
                "background-color" | "background" => {
                    if let Some(color) = parse_color_value(&declaration.value) {
                        self.style.set_background_color(color);
                    }
                    if let Some(grad) = parse_linear_gradient(&declaration.value) {
                        self.style.set_background_gradient(grad);
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
                    if let Some(grad) = parse_linear_gradient(&declaration.value) {
                        self.style.set_background_gradient(grad);
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
                "color" => {
                    if let Some(color) = parse_color_value(&declaration.value) {
                        self.style.set_color(color);
                    }
                }
                "display" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        let display_type =
                            DisplayType::from_str(value).unwrap_or(DisplayType::Block);
                        self.style.set_display(display_type)
                    }
                }
                "flex-grow" => {
                    if let Some(ComponentValue::Number(v)) = first_value {
                        self.style.set_flex_grow(*v);
                    }
                }
                "flex-shrink" => {
                    if let Some(ComponentValue::Number(v)) = first_value {
                        self.style.set_flex_shrink(*v);
                    }
                }
                "flex-basis" => match first_value {
                    Some(ComponentValue::Ident(v)) if v == "auto" || v == "content" => {
                        self.style.set_flex_basis(None);
                    }
                    Some(ComponentValue::Number(v)) if *v == 0.0 => {
                        self.style.set_flex_basis(Some(0.0));
                    }
                    Some(ComponentValue::Dimension(v, unit)) if unit != "%" => {
                        if let Some(px) =
                            length_to_px(*v, unit, self.style.font_size_or_default())
                        {
                            self.style.set_flex_basis(Some(px));
                        }
                    }
                    _ => {}
                },
                // flex shorthand: none | auto | <grow> [<shrink>] [<basis>]
                // https://www.w3.org/TR/css-flexbox-1/#flex-property
                "flex" => {
                    let non_ws: Vec<&ComponentValue> = declaration
                        .value
                        .iter()
                        .filter(|v| !matches!(v, ComponentValue::Whitespace))
                        .collect();
                    match non_ws.as_slice() {
                        [ComponentValue::Ident(v)] if v == "none" => {
                            self.style.set_flex_grow(0.0);
                            self.style.set_flex_shrink(0.0);
                            self.style.set_flex_basis(None);
                        }
                        [ComponentValue::Ident(v)] if v == "auto" => {
                            self.style.set_flex_grow(1.0);
                            self.style.set_flex_shrink(1.0);
                            self.style.set_flex_basis(None);
                        }
                        _ => {
                            let mut numbers = non_ws.iter().filter_map(|v| match v {
                                ComponentValue::Number(n) => Some(*n),
                                _ => None,
                            });
                            if let Some(grow) = numbers.next() {
                                self.style.set_flex_grow(grow);
                                self.style.set_flex_shrink(numbers.next().unwrap_or(1.0));
                                // A unitless single value implies flex-basis 0.
                                self.style.set_flex_basis(Some(0.0));
                            }
                            if let Some(px) = non_ws.iter().find_map(|v| match v {
                                ComponentValue::Dimension(n, unit) if unit != "%" => {
                                    length_to_px(*n, unit, self.style.font_size_or_default())
                                }
                                _ => None,
                            }) {
                                self.style.set_flex_basis(Some(px));
                            }
                        }
                    }
                }
                "justify-content" => {
                    if let Some(ComponentValue::Ident(v)) = first_value {
                        let jc = match v.as_str() {
                            "flex-start" | "start" | "left" | "normal" => {
                                Some(JustifyContent::FlexStart)
                            }
                            "flex-end" | "end" | "right" => Some(JustifyContent::FlexEnd),
                            "center" => Some(JustifyContent::Center),
                            "space-between" => Some(JustifyContent::SpaceBetween),
                            "space-around" => Some(JustifyContent::SpaceAround),
                            "space-evenly" => Some(JustifyContent::SpaceEvenly),
                            _ => None,
                        };
                        if let Some(jc) = jc {
                            self.style.set_justify_content(jc);
                        }
                    }
                }
                "align-items" | "align-self" => {
                    if let Some(ComponentValue::Ident(v)) = first_value {
                        let ai = match v.as_str() {
                            "stretch" | "normal" => Some(AlignItems::Stretch),
                            "flex-start" | "start" | "self-start" => Some(AlignItems::FlexStart),
                            "center" => Some(AlignItems::Center),
                            "flex-end" | "end" | "self-end" => Some(AlignItems::FlexEnd),
                            "baseline" => Some(AlignItems::Baseline),
                            _ => None,
                        };
                        if let Some(ai) = ai {
                            if declaration.property == "align-items" {
                                self.style.set_align_items(ai);
                            } else {
                                self.style.set_align_self(ai);
                            }
                        }
                    }
                }
                "flex-direction" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style.set_flex_direction(FlexDirection::from_str(value));
                    }
                }
                "grid-template-areas" => {
                    let rows: Vec<Vec<String>> = declaration
                        .value
                        .iter()
                        .filter_map(|v| match v {
                            ComponentValue::StringToken(s) => Some(
                                s.split_ascii_whitespace()
                                    .map(|c| c.to_string())
                                    .collect::<Vec<_>>(),
                            ),
                            _ => None,
                        })
                        .filter(|r| !r.is_empty())
                        .collect();
                    if !rows.is_empty() {
                        self.style.set_grid_template_areas(rows);
                    }
                }
                // grid-area: <name> (line-number forms are not supported yet).
                "grid-area" => {
                    if let Some(ComponentValue::Ident(name)) = first_value {
                        self.style.set_grid_area_name(name.clone());
                    }
                }
                "grid-template-columns" => {
                    let (tracks, lines) =
                        parse_grid_template_tracks_with_lines(&declaration.value);
                    self.style.set_grid_template_columns(tracks);
                    self.style.set_grid_column_line_names(lines);
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
                    // Percentages resolve against the containing block at
                    // sizing time; every other unit (px/em/rem/vw/vh/pt/...)
                    // resolves to px here.
                    Some(ComponentValue::Dimension(value, unit)) if unit == "%" => {
                        self.style.set_width_ratio(*value / 100.0);
                    }
                    Some(ComponentValue::Dimension(value, unit)) => {
                        if let Some(px) = length_to_px(*value, unit, FontSize::Medium) {
                            self.style.set_width(px);
                        }
                    }
                    _ => {}
                },
                "height" => match first_value {
                    Some(ComponentValue::Number(value)) => {
                        self.style.set_height(*value);
                    }
                    Some(ComponentValue::Dimension(value, unit)) if unit == "%" => {
                        self.style.set_height_ratio(*value / 100.0);
                    }
                    Some(ComponentValue::Dimension(value, unit)) => {
                        if let Some(px) = length_to_px(*value, unit, FontSize::Medium) {
                            self.style.set_height(px);
                        }
                    }
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
                // Per-side margin/padding longhands.
                "margin-top" | "margin-right" | "margin-bottom" | "margin-left"
                | "padding-top" | "padding-right" | "padding-bottom" | "padding-left" => {
                    let base = self.style.font_size_or_default();
                    let is_margin = declaration.property.starts_with("margin");
                    let auto = matches!(
                        first_value,
                        Some(ComponentValue::Ident(v)) if v == "auto"
                    );
                    if is_margin && auto {
                        if declaration.property.ends_with("left") {
                            self.style.set_margin_left_auto(true);
                        } else if declaration.property.ends_with("right") {
                            self.style.set_margin_right_auto(true);
                        }
                    } else if let Some(px) =
                        first_value.and_then(|v| spacing_component_to_px(v, base))
                    {
                        let cur = if is_margin {
                            self.style.margin_or_default()
                        } else {
                            self.style.padding_or_zero()
                        };
                        let (mut t, mut r, mut b, mut l) =
                            (cur.top(), cur.right(), cur.bottom(), cur.left());
                        match declaration.property.as_str() {
                            p if p.ends_with("top") => t = px,
                            p if p.ends_with("right") => r = px,
                            p if p.ends_with("bottom") => b = px,
                            _ => l = px,
                        }
                        let edges =
                            crate::renderer::layout::computed_style::EdgeSize::from_values(
                                t, r, b, l,
                            );
                        if is_margin {
                            self.style.set_margin(edges);
                        } else {
                            self.style.set_padding(edges);
                        }
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
                        if let Some(color) = parse_color_value(&declaration.value) {
                            self.style.set_border_color(color);
                        }
                    }
                }
                // Per-side border shorthands and width longhands. The engine
                // keeps ONE border color for the whole box, so the color from
                // the most recent side shorthand wins (common pages use the
                // same color on every side).
                "border-top" | "border-right" | "border-bottom" | "border-left"
                | "border-top-width" | "border-right-width" | "border-bottom-width"
                | "border-left-width" => {
                    let side = match declaration.property.as_str() {
                        p if p.starts_with("border-top") => 0,
                        p if p.starts_with("border-right") => 1,
                        p if p.starts_with("border-bottom") => 2,
                        _ => 3,
                    };
                    let base = self.style.font_size_or_default();
                    let mut width = declaration
                        .value
                        .iter()
                        .find_map(|v| spacing_component_to_px(v, base));
                    // `border-top: none` / style `none` zeroes the side.
                    if declaration
                        .value
                        .iter()
                        .any(|v| matches!(v, ComponentValue::Ident(s) if s == "none" || s == "hidden"))
                    {
                        width = Some(0.0);
                    }
                    if let Some(px) = width {
                        self.style.set_border_side(side, px.max(0.0));
                    }
                    if !declaration.property.ends_with("-width") {
                        if let Some(color) = parse_color_value(&declaration.value) {
                            self.style.set_border_color(color);
                        }
                    }
                }
                "border-style" => {
                    // Approximation: any visible style keeps the stroke;
                    // none/hidden removes the border entirely (computed
                    // border-width becomes 0 per spec).
                    if let Some(ComponentValue::Ident(v)) = first_value {
                        if v == "none" || v == "hidden" {
                            self.style.set_border(EdgeSize::zero());
                        }
                    }
                }
                "border-color" => {
                    if let Some(c) = parse_color_value(&declaration.value) {
                        self.style.set_border_color(c);
                    }
                }
                "border-radius" => {
                    if let Some(px) = first_value
                        .and_then(|v| spacing_component_to_px(v, self.style.font_size_or_default()))
                    {
                        self.style.set_border_radius(px);
                    }
                }
                "white-space" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        let ws = match value.as_str() {
                            "normal" => Some(WhiteSpace::Normal),
                            "nowrap" => Some(WhiteSpace::Nowrap),
                            "pre" => Some(WhiteSpace::Pre),
                            "pre-wrap" => Some(WhiteSpace::PreWrap),
                            "pre-line" => Some(WhiteSpace::PreLine),
                            _ => None,
                        };
                        if let Some(ws) = ws {
                            self.style.set_white_space(ws);
                        }
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
                "min-width" | "max-width" | "min-height" | "max-height" => {
                    // Outer None = unparseable (leave untouched);
                    // Some(None) = explicit `none` (clears an earlier rule's limit).
                    let parsed: Option<Option<SizeLimit>> = match first_value {
                        Some(ComponentValue::Dimension(v, unit)) if unit == "%" => {
                            Some(Some(SizeLimit::Ratio(*v / 100.0)))
                        }
                        Some(ComponentValue::Dimension(v, unit)) => {
                            length_to_px(*v, unit, self.style.font_size_or_default())
                                .map(|px| Some(SizeLimit::Px(px)))
                        }
                        Some(ComponentValue::Number(v)) if *v == 0.0 => {
                            Some(Some(SizeLimit::Px(0.0)))
                        }
                        Some(ComponentValue::Ident(v)) if v == "none" => Some(None),
                        _ => None,
                    };
                    if let Some(limit) = parsed {
                        match declaration.property.as_str() {
                            "min-width" => self.style.set_min_width(limit),
                            "max-width" => self.style.set_max_width(limit),
                            "min-height" => self.style.set_min_height(limit),
                            _ => self.style.set_max_height(limit),
                        }
                    }
                }
                "box-sizing" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        match value.as_str() {
                            "border-box" => self.style.set_border_box(true),
                            "content-box" => self.style.set_border_box(false),
                            _ => {}
                        }
                    }
                }
                "list-style-type" | "list-style" => {
                    // The shorthand also carries position/image; we read the
                    // first recognized type keyword (none included).
                    let ty = declaration.value.iter().find_map(|v| match v {
                        ComponentValue::Ident(k) => match k.as_str() {
                            "none" => Some(ListStyleType::None),
                            "disc" => Some(ListStyleType::Disc),
                            "circle" => Some(ListStyleType::Circle),
                            "square" => Some(ListStyleType::Square),
                            "decimal" => Some(ListStyleType::Decimal),
                            _ => None,
                        },
                        _ => None,
                    });
                    if let Some(ty) = ty {
                        self.style.set_list_style_type(ty);
                    }
                }
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
