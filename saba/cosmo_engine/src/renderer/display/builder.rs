//! Display-list emission: walking a laid-out LayoutObject and producing
//! DisplayItems. Extracted verbatim from layout_object.rs (plan 0.5);
//! becomes a self-describing command builder in plan 1.6.

use crate::display_item::{ClipRect, DisplayItem, PaintOrder};
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::NodeKind;
use crate::renderer::layout::computed_style::*;
use crate::renderer::layout::replaced::{serialize_inline_svg, InputKind};
use crate::renderer::layout::layout_object::{
    compute_box_model_metrics, LayoutObject, LayoutObjectKind, LayoutPoint, LayoutSize,
};
use crate::renderer::text::legacy_metrics::*;
use std::vec::Vec;

impl LayoutObject {
    pub fn paint(&mut self) -> Vec<DisplayItem> {
        if self.style.display() == DisplayType::DisplayNone {
            return vec![];
        }
        // visibility:hidden boxes keep their layout space but emit nothing.
        // Paint runs per node (not per subtree), so a descendant that resets
        // visibility:visible still paints via the inherited flag it cleared.
        if self.style.is_visibility_hidden() {
            return vec![];
        }

        match self.kind {
            _ if self.is_replaced_box() => return self.paint_replaced(),
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
                        // The root's background covers the whole canvas, past
                        // `<body>`'s margins, unless `<html>` has its own, which
                        // then takes the canvas and leaves `<body>` to its box.
                        // Spec: CSS Backgrounds §2.11.2.
                        // https://www.w3.org/TR/css-backgrounds-3/#special-backgrounds
                        let is_root = self.parent.upgrade().is_none();
                        let margin = compute_box_model_metrics(&self.style).margin;
                        let (canvas_point, canvas_size) = (
                            LayoutPoint::new(self.point().x() - margin.left, rect_y - margin.top),
                            LayoutSize::new(
                                self.size().width() + margin.left + margin.right,
                                rect_h + margin.top + margin.bottom,
                            ),
                        );
                        let mut canvas_item = None;
                        if let Some(color) = self.html_background.clone() {
                            let mut canvas_style = self.style();
                            canvas_style.set_background_color(color);
                            canvas_item = Some(DisplayItem::Rect {
                                style: canvas_style,
                                layout_point: canvas_point,
                                layout_size: canvas_size,
                                paint_order: PaintOrder {
                                    stacking_context: self.stacking_context_level(),
                                    z_index: self.style.z_index_or_default(),
                                },
                                clip_rect: None,
                                anchor_id: None,
                            });
                        }
                        let fill_canvas = is_root && canvas_item.is_none();
                        let mut items = vec![DisplayItem::Rect {
                            style: self.style(),
                            layout_point: if fill_canvas {
                                canvas_point
                            } else {
                                LayoutPoint::new(self.point().x(), rect_y)
                            },
                            layout_size: if fill_canvas {
                                canvas_size
                            } else {
                                LayoutSize::new(self.size().width(), rect_h)
                            },
                            paint_order: PaintOrder {
                                stacking_context: self.stacking_context_level(),
                                z_index: self.style.z_index_or_default(),
                            },
                            clip_rect: self.style.final_clip().map(|(x, y, w, h)| ClipRect {
                                x: x as i64,
                                y: y as i64,
                                width: w as i64,
                                height: h as i64,
                            }),
                            anchor_id,
                        }];
                        if let Some(canvas) = canvas_item {
                            items.insert(0, canvas);
                        }
                        if let Some(marker) = self.list_marker_text() {
                            let fs = self.style.font_size();
                            let w = measure_text_width(&marker, fs, self.style.text_face());
                            let bold = self.style.is_bold();
                            items.push(DisplayItem::Text {
                                text: marker,
                                style: self.style(),
                                layout_point: LayoutPoint::new(
                                    self.point().x() - w - 6,
                                    self.point().y(),
                                ),
                                href: None,
                                target: None,
                                paint_order: PaintOrder {
                                    stacking_context: self.stacking_context_level(),
                                    z_index: self.style.z_index_or_default(),
                                },
                                clip_rect: self.style.final_clip().map(|(x, y, w, h)| {
                                    ClipRect {
                                        x: x as i64,
                                        y: y as i64,
                                        width: w as i64,
                                        height: h as i64,
                                    }
                                }),
                                bold,
                            });
                        }
                        return items;
                    }
                }
            }
            LayoutObjectKind::Inline => {
                if let NodeKind::Element(_) = self.node_kind() {
                    let mut items = Vec::new();
                    if self.size.width() > 0 && self.size.height() > 0 {
                        let mut anchor_id = self.element_attribute("id");
                        let whole = [(0, 0, self.size().width(), self.size().height())];
                        let boxes: &[(i64, i64, i64, i64)] = if self.inline_line_boxes.is_empty() {
                            &whole
                        } else {
                            &self.inline_line_boxes
                        };
                        for (dx, dy, width, height) in boxes.iter().copied() {
                            items.push(DisplayItem::Rect {
                                style: self.style(),
                                layout_point: LayoutPoint::new(
                                    self.point().x() + dx,
                                    self.point().y() + dy,
                                ),
                                layout_size: LayoutSize::new(width, height),
                                paint_order: PaintOrder {
                                    stacking_context: self.stacking_context_level(),
                                    z_index: self.style.z_index_or_default(),
                                },
                                clip_rect: self.style.final_clip().map(|(x, y, w, h)| ClipRect {
                                    x: x as i64,
                                    y: y as i64,
                                    width: w as i64,
                                    height: h as i64,
                                }),
                                anchor_id: anchor_id.take(),
                            });
                        }
                    }

                    if !items.is_empty() {
                        return items;
                    }
                }
            }
            LayoutObjectKind::Text => {
                if let NodeKind::Text(t) = self.node_kind() {
                    let mut v = vec![];
                    let fs = self.style.font_size();
                    let bold = self.style.text_face();
                    let cw = bold_width_adjust(char_width_px(fs), bold.bold);
                    let lh = styled_line_height(&self.style);
                    let plain_text = self.display_text(&t);
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
                            (cb.size().width() - cm.inner_horizontal()).max(cw)
                        })
                        .unwrap_or_else(|| {
                            if self.text_line_max_width > 0 {
                                self.text_line_max_width
                            } else {
                                self.size().width().max(cw)
                            }
                        });
                    let mut lines = if self.style.white_space_nowrap()
                        && !self.style.white_space_preserves_newlines()
                    {
                        // nowrap: a single line (the collapser already turned
                        // newlines into spaces). text-overflow:ellipsis on a
                        // clipping ancestor then truncates it to fit.
                        let mut line = plain_text;
                        if let Some(clip_w) = self.ellipsis_clip_width() {
                            line = truncate_with_ellipsis(&line, fs, bold, clip_w);
                        }
                        vec![line]
                    } else {
                        self.build_text_lines(&plain_text, fs, bold, max_width)
                    };
                    let _ = &mut lines;
                    let href = self.link_href();
                    let target = self.link_target();

                    let face = bold;
                    let bold = self.style.is_bold();
                    // The painter draws from a baseline one font-size below the
                    // point it is given; this puts the real baseline there.
                    let paint_dy = text_baseline(&self.style) - fs.px();
                    // Phase 2.5: when the inline formatting context placed this
                    // run, paint its fragments where they were placed — a run
                    // that starts mid-line and continues at the left edge below
                    // cannot be expressed by stacking lines at the box origin.
                    if !self.inline_fragments.is_empty() {
                        let href = self.link_href();
                        let target = self.link_target();
                        // `text-overflow: ellipsis` truncates against the
                        // clipping ancestor's right edge, which is only known
                        // once the fragment has a position — so it is applied
                        // here rather than during line breaking.
                        let ellipsis = self.ellipsis_clip_width();
                        for (text, dx, dy) in self.inline_fragments.clone() {
                            let text = match ellipsis {
                                Some(clip) => truncate_with_ellipsis(
                                    &text,
                                    fs,
                                    face,
                                    (clip - dx).max(0),
                                ),
                                None => text,
                            };
                            v.push(DisplayItem::Text {
                                text,
                                style: self.style(),
                                layout_point: LayoutPoint::new(
                                    self.point().x() + dx,
                                    self.point().y() + dy + paint_dy,
                                ),
                                href: href.clone(),
                                target: target.clone(),
                                paint_order: PaintOrder {
                                    stacking_context: self.stacking_context_level(),
                                    z_index: self.style.z_index_or_default(),
                                },
                                clip_rect: self.style.final_clip().map(|(x, y, w, h)| ClipRect {
                                    x: x as i64,
                                    y: y as i64,
                                    width: w as i64,
                                    height: h as i64,
                                }),
                                bold,
                            });
                        }
                        return v;
                    }
                    for (i, line) in lines.into_iter().enumerate() {
                        let item = DisplayItem::Text {
                            text: line,
                            style: self.style(),
                            layout_point: LayoutPoint::new(
                                self.point().x(),
                                self.point().y() + lh * i as i64 + paint_dy,
                            ),
                            href: href.clone(),
                            target: target.clone(),
                            paint_order: PaintOrder {
                                stacking_context: self.stacking_context_level(),
                                z_index: self.style.z_index_or_default(),
                            },
                            clip_rect: self.style.final_clip().map(|(x, y, w, h)| ClipRect {
                                x: x as i64,
                                y: y as i64,
                                width: w as i64,
                                height: h as i64,
                            }),
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
}

impl LayoutObject {
    fn order(&self) -> PaintOrder {
        PaintOrder {
            stacking_context: self.stacking_context_level(),
            z_index: self.style.z_index_or_default(),
        }
    }

    fn own_clip(&self) -> Option<ClipRect> {
        self.style.final_clip().map(|(x, y, w, h)| ClipRect {
            x: x as i64,
            y: y as i64,
            width: w as i64,
            height: h as i64,
        })
    }

    /// A rect drawn over this box in a color of its own, with no border.
    fn fill_rect(&self, color: Color, x: i64, y: i64, width: i64, height: i64) -> DisplayItem {
        let mut style = self.style();
        style.set_background_color(color);
        style.set_border(EdgeSize::zero());
        DisplayItem::Rect {
            style,
            layout_point: LayoutPoint::new(x, y),
            layout_size: LayoutSize::new(width, height),
            paint_order: self.order(),
            clip_rect: self.own_clip(),
            anchor_id: None,
        }
    }

    fn control_text(&self, text: String, x: i64, y: i64, color: Option<Color>) -> DisplayItem {
        let mut style = self.style();
        if let Some(color) = color {
            style.set_color(color);
        }
        style.set_text_decoration(TextDecoration::None);
        let origin = self.point();
        // Text stays inside the control.
        let own = ClipRect {
            x: origin.x(),
            y: origin.y(),
            width: self.size().width(),
            height: self.size().height(),
        };
        let clip = match self.own_clip() {
            Some(outer) => {
                let left = own.x.max(outer.x);
                let top = own.y.max(outer.y);
                let right = (own.x + own.width).min(outer.x + outer.width);
                let bottom = (own.y + own.height).min(outer.y + outer.height);
                ClipRect {
                    x: left,
                    y: top,
                    width: (right - left).max(0),
                    height: (bottom - top).max(0),
                }
            }
            None => own,
        };
        let paint_dy = text_baseline(&self.style) - self.style.font_size().px();
        DisplayItem::Text {
            text,
            style,
            layout_point: LayoutPoint::new(x, y + paint_dy),
            href: None,
            target: None,
            paint_order: self.order(),
            clip_rect: Some(clip),
            bold: self.style.is_bold(),
        }
    }

    /// A replaced element: its box, then whatever stands in for its contents.
    fn paint_replaced(&self) -> Vec<DisplayItem> {
        let mut items = Vec::new();
        if self.size.width() > 0 && self.size.height() > 0 {
            items.push(DisplayItem::Rect {
                style: self.style(),
                layout_point: self.point(),
                layout_size: self.size(),
                paint_order: self.order(),
                clip_rect: self.own_clip(),
                anchor_id: self.element_attribute("id"),
            });
        }
        let (x, y) = (self.point().x(), self.point().y());
        let (width, height) = (self.size().width(), self.size().height());
        let (text_x, text_y) = self.control_text_origin();
        let content = self.content_size();
        match self.tag().as_str() {
            "img" => items.push(DisplayItem::Image {
                src: self.current_image_src(),
                alt: self.element_attribute("alt").unwrap_or_default(),
                layout_point: self.point(),
                layout_size: self.size(),
                style: self.style(),
                href: self.link_href(),
                target: self.link_target(),
                paint_order: self.order(),
                clip_rect: self.own_clip(),
            }),
            "svg" => {
                if width > 0 && height > 0 {
                    let xml = serialize_inline_svg(
                        &self.node,
                        width,
                        height,
                        self.style.color_or_black().code(),
                        self.style.svg_fill(),
                    );
                    items.push(DisplayItem::Image {
                        src: format!("data:image/svg+xml;utf8,{xml}"),
                        alt: String::new(),
                        layout_point: self.point(),
                        layout_size: self.size(),
                        style: self.style(),
                        href: self.link_href(),
                        target: self.link_target(),
                        paint_order: self.order(),
                        clip_rect: self.own_clip(),
                    });
                }
            }
            "input" => match self.input_kind() {
                InputKind::Text | InputKind::Password => {
                    let value = self.element_attribute("value").unwrap_or_default();
                    if value.is_empty() {
                        if let Some(placeholder) = self.element_attribute("placeholder") {
                            let gray = Color::from_code("#757575").ok();
                            items.push(self.control_text(placeholder, text_x, text_y, gray));
                        }
                    } else if self.input_kind() == InputKind::Password {
                        // Never paint what was typed into a password field.
                        let bullets = "\u{2022}".repeat(value.chars().count());
                        items.push(self.control_text(bullets, text_x, text_y, None));
                    } else {
                        items.push(self.control_text(value, text_x, text_y, None));
                    }
                }
                InputKind::Button | InputKind::File => {
                    let label = self.input_button_label();
                    let text_width = measure_text_width(
                        &label,
                        self.style.font_size(),
                        self.style.text_face(),
                    );
                    let centered = text_x + ((content.width() - text_width) / 2).max(0);
                    items.push(self.control_text(label, centered, text_y, None));
                }
                InputKind::Checkbox => {
                    if self.element_attribute("checked").is_some() {
                        let blue = Color::from_code("#0075ff").unwrap_or_else(|_| Color::black());
                        items.push(self.fill_rect(blue, x, y, width, height));
                        let white = Color::from_code("#ffffff").ok();
                        items.push(self.control_text("\u{2713}".to_string(), x + 1, y - 2, white));
                    }
                }
                InputKind::Radio => {
                    if self.element_attribute("checked").is_some() {
                        let blue = Color::from_code("#0075ff").unwrap_or_else(|_| Color::black());
                        let mut dot = self.style();
                        dot.set_background_color(blue);
                        dot.set_border(EdgeSize::zero());
                        dot.set_border_radius(4.0);
                        items.push(DisplayItem::Rect {
                            style: dot,
                            layout_point: LayoutPoint::new(x + 3, y + 3),
                            layout_size: LayoutSize::new(width - 6, height - 6),
                            paint_order: self.order(),
                            clip_rect: self.own_clip(),
                            anchor_id: None,
                        });
                    }
                }
                InputKind::Range => {
                    let parse = |name: &str, default: f64| {
                        self.element_attribute(name)
                            .and_then(|v| v.parse::<f64>().ok())
                            .unwrap_or(default)
                    };
                    let (min, max) = (parse("min", 0.0), parse("max", 100.0));
                    let value = parse("value", (min + max) / 2.0);
                    let fraction = if max > min {
                        ((value - min) / (max - min)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    let track = Color::from_code("#c8c8c8").unwrap_or_else(|_| Color::gray());
                    let blue = Color::from_code("#0075ff").unwrap_or_else(|_| Color::black());
                    let middle = y + height / 2;
                    items.push(self.fill_rect(track, x, middle - 2, width, 4));
                    let thumb_x = x + ((width - 12) as f64 * fraction) as i64;
                    items.push(self.fill_rect(blue.clone(), x, middle - 2, thumb_x - x + 6, 4));
                    items.push(self.fill_rect(blue, thumb_x, y + 1, 12, (height - 2).max(1)));
                }
                InputKind::Color => {
                    let swatch = self
                        .element_attribute("value")
                        .and_then(|v| Color::from_code(&v).ok())
                        .unwrap_or_else(Color::black);
                    items.push(self.fill_rect(swatch, text_x, text_y, content.width(), content.height()));
                }
            },
            "select" => {
                let selected = self.selected_option();
                if let Some((label, _)) = &selected {
                    items.push(self.control_text(label.clone(), text_x, text_y, None));
                }
                let arrow_x = x + width - 15;
                items.push(self.control_text("\u{25BE}".to_string(), arrow_x, text_y, None));
                if self.element_attribute("data-cosmo-open").is_some() {
                    items.extend(self.paint_select_popup(selected.map(|(_, index)| index)));
                }
            }
            "progress" | "meter" => {
                let parse = |name: &str, default: f64| {
                    self.element_attribute(name)
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(default)
                };
                let max = parse("max", 1.0).max(f64::MIN_POSITIVE);
                let min = if self.tag() == "meter" { parse("min", 0.0) } else { 0.0 };
                let fraction = ((parse("value", 0.0) - min) / (max - min)).clamp(0.0, 1.0);
                let track = Color::from_code("#e8e8e8").unwrap_or_else(|_| Color::gray());
                items.push(self.fill_rect(track, x, y, width, height));
                let bar = Color::from_code(if self.tag() == "meter" { "#2e9e4f" } else { "#0075ff" })
                    .unwrap_or_else(|_| Color::black());
                items.push(self.fill_rect(bar, x, y, (width as f64 * fraction) as i64, height));
            }
            _ => {}
        }
        items
    }

    fn paint_select_popup(&self, selected: Option<usize>) -> Vec<DisplayItem> {
        let mut items = Vec::new();
        let rows = self.select_popup_rows();
        let Some(first) = rows.first().copied() else {
            return items;
        };
        let high = PaintOrder {
            stacking_context: 1,
            z_index: i32::MAX / 2,
        };
        let on_top = |mut item: DisplayItem| {
            match &mut item {
                DisplayItem::Rect { paint_order, style, clip_rect, .. } => {
                    *paint_order = high;
                    *clip_rect = None;
                    style.set_paint_z(i32::MAX / 2);
                }
                DisplayItem::Text { paint_order, style, clip_rect, .. } => {
                    *paint_order = high;
                    *clip_rect = None;
                    style.set_paint_z(i32::MAX / 2);
                }
                _ => {}
            }
            item
        };
        let total = first.3 * rows.len() as i64;
        let edge = Color::from_code("#767676").unwrap_or_else(|_| Color::gray());
        items.push(on_top(self.fill_rect(edge, first.0 - 1, first.1 - 1, first.2 + 2, total + 2)));
        let white = Color::from_code("#ffffff").unwrap_or_else(|_| Color::white());
        let highlight = Color::from_code("#0075ff").unwrap_or_else(|_| Color::black());
        for (index, ((label, _), (x, y, width, height))) in
            self.select_options().into_iter().zip(rows).enumerate()
        {
            let is_selected = selected == Some(index);
            let background = if is_selected { highlight.clone() } else { white.clone() };
            items.push(on_top(self.fill_rect(background, x, y, width, height)));
            let color = is_selected.then(|| white.clone());
            items.push(on_top(self.control_text(label, x + 4, y + 2, color)));
        }
        items
    }
}
