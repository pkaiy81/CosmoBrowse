use alloc::string::ToString;
use alloc::vec::Vec;
use crate::paint_commands::{DrawImage, DrawRect, DrawText, PaintCommand, PaintCommandList};

use crate::nebula_renderer::layout::computed_style::PositionType;
use crate::nebula_renderer::layout::computed_style::TextDecoration;
use crate::stardust_display::DisplayItem;


/// Apply a stamped scale context to a layout point.
fn scaled_point(ctx: Option<(f64, f64, f64)>, x: i64, y: i64) -> (i64, i64) {
    match ctx {
        Some((ox, oy, s)) => (
            (ox + (x as f64 - ox) * s) as i64,
            (oy + (y as f64 - oy) * s) as i64,
        ),
        None => (x, y),
    }
}

/// Apply a stamped scale context to a length.
fn scaled_len(ctx: Option<(f64, f64, f64)>, v: i64) -> i64 {
    match ctx {
        Some((_, _, s)) => (v as f64 * s) as i64,
        None => v,
    }
}

/// Maps layout paint records into backend-neutral paint commands.
///
/// Spec alignment:
/// - CSS2.2 painting order is preserved by consuming display items in-order.
/// - CSS Overflow clipping is propagated via `clip_rect` so adapters can apply
///   clipping consistently when replaying commands.
/// - Text decoration forwarding keeps CSS Text underline semantics.
pub fn map_display_items_to_paint_commands(
    display_items: &[DisplayItem],
    origin_x: i64,
    origin_y: i64,
) -> PaintCommandList {
    let mut commands = Vec::with_capacity(display_items.len());

    for item in display_items {
        match item {
            DisplayItem::Rect {
                style,
                layout_point,
                layout_size,
                paint_order: _,
                clip_rect,
                anchor_id,
            } => {
                let border = style.border_or_zero();
                // `f64::round` lives in `std`; this crate is `no_std`. Border
                // widths are non-negative, so round-half-up via `+ 0.5`.
                let border_width = (border.top()
                    .max(border.right())
                    .max(border.bottom())
                    .max(border.left())
                    + 0.5) as i64;
                let border_color = style.border_color()
                    .map(|c| c.code().to_string())
                    .unwrap_or_default();
                let ctx = style.scale_context();
                let (lx, ly) = scaled_point(ctx, layout_point.x(), layout_point.y());
                commands.push(PaintCommand::DrawRect(DrawRect {
                    x: origin_x + lx,
                    y: origin_y + ly,
                    width: scaled_len(ctx, layout_size.width()),
                    height: scaled_len(ctx, layout_size.height()),
                    background_color: style.background_color().code().to_string(),
                    background_image: style.background_image().map(|s| s.to_string()),
                    opacity: style.opacity(),
                    // Final paint-order key from the engine's stacking pass
                    // (root canvas −2M, normal flow 0, contexts ±1M+z).
                    z_index: style.paint_z(),
                    clip_rect: clip_rect.map(|c| (c.x, c.y, c.width, c.height)),
                    anchor_id: anchor_id.clone(),
                    border_width,
                    border_color,
                    background_position: style.background_position(),
                    background_no_repeat: style.background_no_repeat(),
                    background_size: style.background_size(),
                    border_radius: scaled_len(ctx, style.border_radius() as i64),
                    box_shadow: style.box_shadow().map(|(dx, dy, b, c)| (dx as i64, dy as i64, b as i64, c.code().to_string())),
                    fixed: style.position_or_default() == PositionType::Fixed || style.fixed_subtree(),
                    sticky: style.sticky_context().map(|(t, y, m)| (t as i64, y as i64, m.min(i64::MAX as f64) as i64)),
                    scroll_container: style.scroll_container(),
                    scroll_container_def: style.scroll_container_def().map(|(i, w, h)| (i, w as i64, h as i64)),
                }));
            }
            DisplayItem::Text {
                text,
                style,
                layout_point,
                href,
                target,
                paint_order: _,
                clip_rect,
                bold,
            } => {
                let font_family = style.font_family();
                if font_family.trim().is_empty() {
                    commands.push(PaintCommand::fallback_text(
                        origin_x + layout_point.x(),
                        origin_y + layout_point.y(),
                        text,
                        style.color().code().to_string(),
                        style.font_size().px(),
                        style.opacity(),
                        href.clone(),
                        style.paint_z(),
                        clip_rect.map(|c| (c.x, c.y, c.width, c.height)),
                    ));
                    continue;
                }

                let ctx = style.scale_context();
                let (lx, ly) = scaled_point(ctx, layout_point.x(), layout_point.y());
                commands.push(PaintCommand::DrawText(DrawText {
                    fixed: style.position_or_default() == PositionType::Fixed || style.fixed_subtree(),
                    sticky: style.sticky_context().map(|(t, y, m)| (t as i64, y as i64, m.min(i64::MAX as f64) as i64)),
                    scroll_container: style.scroll_container(),
                    x: origin_x + lx,
                    y: origin_y + ly,
                    text: text.clone(),
                    color: style.color().code().to_string(),
                    font_px: scaled_len(ctx, style.font_size().px()).max(1),
                    font_family,
                    underline: style.text_decoration() == TextDecoration::Underline,
                    bold: *bold,
                    opacity: style.opacity(),
                    href: href.clone(),
                    target: target.clone(),
                    // Final paint-order key from the engine's stacking pass
                    // (root canvas −2M, normal flow 0, contexts ±1M+z).
                    z_index: style.paint_z(),
                    clip_rect: clip_rect.map(|c| (c.x, c.y, c.width, c.height)),
                }));
            }
            DisplayItem::Image {
                src,
                alt,
                layout_point,
                layout_size,
                style,
                href,
                target,
                paint_order: _,
                clip_rect,
            } => {
                let ctx = style.scale_context();
                let (lx, ly) = scaled_point(ctx, layout_point.x(), layout_point.y());
                commands.push(PaintCommand::DrawImage(DrawImage {
                    fixed: style.position_or_default() == PositionType::Fixed || style.fixed_subtree(),
                    sticky: style.sticky_context().map(|(t, y, m)| (t as i64, y as i64, m.min(i64::MAX as f64) as i64)),
                    scroll_container: style.scroll_container(),
                    x: origin_x + lx,
                    y: origin_y + ly,
                    width: scaled_len(ctx, layout_size.width()),
                    height: scaled_len(ctx, layout_size.height()),
                    src: src.clone(),
                    alt: alt.clone(),
                    opacity: style.opacity(),
                    href: href.clone(),
                    target: target.clone(),
                    // Final paint-order key from the engine's stacking pass
                    // (root canvas −2M, normal flow 0, contexts ±1M+z).
                    z_index: style.paint_z(),
                    clip_rect: clip_rect.map(|c| (c.x, c.y, c.width, c.height)),
                }));
            }
        }
    }

    PaintCommandList {
        commands,
        diagnostics: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;
    use crate::nebula_renderer::layout::computed_style::{Color, ComputedStyle, FontSize, TextDecoration};
    use crate::nebula_renderer::layout::layout_object::{LayoutPoint, LayoutSize};
    use crate::stardust_display::PaintOrder;

    #[test]
    fn paint_commands_snapshot_is_stable() {
        let mut rect_style = ComputedStyle::new();
        rect_style.set_background_color(Color::from_code("#eeeeee").unwrap());
        rect_style.set_opacity(1.0);

        let mut text_style = ComputedStyle::new();
        text_style.set_color(Color::from_code("#111111").unwrap());
        text_style.set_font_size(FontSize::Medium);
        text_style.set_font_family("serif".to_string());
        text_style.set_text_decoration(TextDecoration::Underline);
        text_style.set_opacity(0.9);
        // The paint-order key now rides on the style (stamped by the layout
        // pass), not on PaintOrder.
        text_style.set_paint_z(1);

        let display_items = vec![
            DisplayItem::Rect {
                style: rect_style,
                layout_point: LayoutPoint::new(4, 6),
                layout_size: LayoutSize::new(80, 20),
                paint_order: PaintOrder::root(),
                clip_rect: None,
                anchor_id: None,
            },
            DisplayItem::Text {
                text: "hello".to_string(),
                style: text_style,
                layout_point: LayoutPoint::new(8, 10),
                href: Some("https://example.com".to_string()),
                target: None,
                paint_order: PaintOrder { stacking_context: 0, z_index: 1 },
                clip_rect: None,
                bold: false,
            },
        ];

        let mapped = map_display_items_to_paint_commands(&display_items, 10, 20);
        let actual = serde_json::to_string_pretty(&mapped).unwrap();
        let expected = include_str!("../tests/snapshots/paint_commands_basic.json");
        assert_eq!(actual.trim(), expected.trim());
    }
}
