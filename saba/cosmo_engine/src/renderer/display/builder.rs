//! Display-list emission: walking a laid-out LayoutObject and producing
//! DisplayItems. Extracted verbatim from layout_object.rs (plan 0.5);
//! becomes a self-describing command builder in plan 1.6.

use crate::display_item::{ClipRect, DisplayItem, PaintOrder};
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::NodeKind;
use crate::renderer::layout::computed_style::*;
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
                                stacking_context: self.stacking_context_level(),
                                z_index: self.style.z_index_or_default(),
                            },
                            clip_rect: self
                                .style
                                .final_clip()
                                .map(|(x, y, w, h)| ClipRect {
                                    x: x as i64,
                                    y: y as i64,
                                    width: w as i64,
                                    height: h as i64,
                                })
                                .or_else(|| {
                                    // Clip to ancestor cell so oversized images
                                    // don't overflow their cell boundary.
                                    self.nearest_ancestor_cell().map(|cell| {
                                        let cb = cell.borrow();
                                        ClipRect {
                                            x: cb.point().x(),
                                            y: cb.point().y(),
                                            width: cb.size().width(),
                                            height: cb.size().height(),
                                        }
                                    })
                                }),
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
                                stacking_context: self.stacking_context_level(),
                                z_index: self.style.z_index_or_default(),
                            },
                            clip_rect: self.style.final_clip().map(|(x, y, w, h)| ClipRect {
                                x: x as i64,
                                y: y as i64,
                                width: w as i64,
                                height: h as i64,
                            }),
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
                    let fs = self.style.font_size();
                    let bold = self.style.is_bold();
                    let cw = bold_width_adjust(char_width_px(fs), bold);
                    let lh = styled_line_height(&self.style);
                    let plain_text = self.collapse_text_whitespace(&t);
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
                    let mut lines = if self.style.white_space_nowrap() {
                        // nowrap: a single line (the collapser already turned
                        // newlines into spaces). text-overflow:ellipsis on a
                        // clipping ancestor then truncates it to fit.
                        let mut line = plain_text;
                        if let Some(clip_w) = self.ellipsis_clip_width() {
                            line = truncate_with_ellipsis(&line, fs, bold, clip_w);
                        }
                        vec![line]
                    } else {
                        split_text(plain_text, fs, bold, max_width)
                    };
                    let _ = &mut lines;
                    let href = self.link_href();
                    let target = self.link_target();

                    let bold = self.style.is_bold();
                    for (i, line) in lines.into_iter().enumerate() {
                        let item = DisplayItem::Text {
                            text: line,
                            style: self.style(),
                            layout_point: LayoutPoint::new(
                                self.point().x(),
                                self.point().y() + lh * i as i64,
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
