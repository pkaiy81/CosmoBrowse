use cosmo_runtime::{DrawRect, DrawText, PaintCommand, PaintCommandList};

use crate::nebula_renderer::layout::computed_style::TextDecoration;
use crate::stardust_display::DisplayItem;

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
                paint_order,
                clip_rect,
            } => {
                commands.push(PaintCommand::DrawRect(DrawRect {
                    x: origin_x + layout_point.x(),
                    y: origin_y + layout_point.y(),
                    width: layout_size.width(),
                    height: layout_size.height(),
                    background_color: style.background_color().code().to_string(),
                    opacity: style.opacity(),
                    z_index: paint_order.z_index,
                    clip_rect: clip_rect.map(|c| (c.x, c.y, c.width, c.height)),
                }));
            }
            DisplayItem::Text {
                text,
                style,
                layout_point,
                href,
                paint_order,
                clip_rect,
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
                        paint_order.z_index,
                        clip_rect.map(|c| (c.x, c.y, c.width, c.height)),
                    ));
                    continue;
                }

                commands.push(PaintCommand::DrawText(DrawText {
                    x: origin_x + layout_point.x(),
                    y: origin_y + layout_point.y(),
                    text: text.clone(),
                    color: style.color().code().to_string(),
                    font_px: style.font_size().px(),
                    font_family,
                    underline: style.text_decoration() == TextDecoration::Underline,
                    opacity: style.opacity(),
                    href: href.clone(),
                    target: None,
                    z_index: paint_order.z_index,
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

        let display_items = vec![
            DisplayItem::Rect {
                style: rect_style,
                layout_point: LayoutPoint::new(4, 6),
                layout_size: LayoutSize::new(80, 20),
                paint_order: PaintOrder::root(),
                clip_rect: None,
            },
            DisplayItem::Text {
                text: "hello".to_string(),
                style: text_style,
                layout_point: LayoutPoint::new(8, 10),
                href: Some("https://example.com".to_string()),
                paint_order: PaintOrder { stacking_context: 0, z_index: 1 },
                clip_rect: None,
            },
        ];

        let mapped = map_display_items_to_paint_commands(&display_items, 10, 20);
        let actual = serde_json::to_string_pretty(&mapped).unwrap();
        let expected = include_str!("../tests/snapshots/paint_commands_basic.json");
        assert_eq!(actual.trim(), expected.trim());
    }
}
