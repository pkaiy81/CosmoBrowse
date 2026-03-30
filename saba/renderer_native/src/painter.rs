/// Renders PaintCommands to a tiny-skia Pixmap.

use cosmo_core::paint_commands::{PaintCommand, DrawRect, DrawText, DrawImage};
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

use crate::color::parse_css_color;
use crate::hit_test::HitRegion;
use crate::text_render::TextRenderer;

/// Render a list of paint commands to the pixmap.
/// Returns hit regions for clickable elements (links).
pub fn render_commands(
    pixmap: &mut Pixmap,
    commands: &[PaintCommand],
    text_renderer: &mut TextRenderer,
    scroll_y: i64,
    chrome_height: i64,
    frame_id: &str,
) -> Vec<HitRegion> {
    let mut hit_regions = Vec::new();

    // Sort by z-index for correct layering.
    let mut sorted: Vec<(i32, usize)> = commands
        .iter()
        .enumerate()
        .map(|(i, cmd)| {
            let z = match cmd {
                PaintCommand::DrawRect(r) => r.z_index,
                PaintCommand::DrawText(t) => t.z_index,
                PaintCommand::DrawImage(img) => img.z_index,
            };
            (z, i)
        })
        .collect();
    sorted.sort_by_key(|(z, _)| *z);

    for (_, idx) in &sorted {
        match &commands[*idx] {
            PaintCommand::DrawRect(rect) => {
                draw_rect(pixmap, rect, scroll_y, chrome_height);
            }
            PaintCommand::DrawText(text) => {
                let end_x = draw_text(pixmap, text, text_renderer, scroll_y, chrome_height);
                if let Some(href) = &text.href {
                    let text_width = end_x - text.x;
                    let font_height = text.font_px;
                    hit_regions.push(HitRegion {
                        x: text.x,
                        y: text.y - font_height + chrome_height - scroll_y,
                        width: text_width.max(1),
                        height: font_height + 4,
                        href: href.clone(),
                        target: text.target.clone(),
                        frame_id: frame_id.to_string(),
                    });
                }
            }
            PaintCommand::DrawImage(img) => {
                draw_image_placeholder(pixmap, img, text_renderer, scroll_y, chrome_height);
                if let Some(href) = &img.href {
                    hit_regions.push(HitRegion {
                        x: img.x,
                        y: img.y + chrome_height - scroll_y,
                        width: img.width,
                        height: img.height,
                        href: href.clone(),
                        target: img.target.clone(),
                        frame_id: frame_id.to_string(),
                    });
                }
            }
        }
    }

    hit_regions
}

fn apply_clip(x: i64, y: i64, w: i64, h: i64, clip: &Option<(i64, i64, i64, i64)>) -> Option<(i64, i64, i64, i64)> {
    if let Some((cx, cy, cw, ch)) = clip {
        let left = x.max(*cx);
        let top = y.max(*cy);
        let right = (x + w).min(cx + cw);
        let bottom = (y + h).min(cy + ch);
        if right > left && bottom > top {
            Some((left, top, right - left, bottom - top))
        } else {
            None
        }
    } else {
        Some((x, y, w, h))
    }
}

fn draw_rect(pixmap: &mut Pixmap, rect: &DrawRect, scroll_y: i64, chrome_height: i64) {
    let ry = rect.y + chrome_height - scroll_y;
    let clipped = apply_clip(rect.x, ry, rect.width, rect.height, &rect.clip_rect);
    let Some((x, y, w, h)) = clipped else { return };

    let (r, g, b, a) = parse_css_color(&rect.background_color);
    let opacity = (rect.opacity * a as f64 / 255.0).clamp(0.0, 1.0) as f32;

    let Some(skia_rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(Color::from_rgba(
        r as f32 / 255.0,
        g as f32 / 255.0,
        b as f32 / 255.0,
        opacity,
    ).unwrap_or(Color::BLACK));
    paint.anti_alias = false;

    pixmap.fill_rect(skia_rect, &paint, Transform::identity(), None);
}

fn draw_text(
    pixmap: &mut Pixmap,
    text: &DrawText,
    text_renderer: &mut TextRenderer,
    scroll_y: i64,
    chrome_height: i64,
) -> i64 {
    let (r, g, b, a) = parse_css_color(&text.color);
    let alpha = (text.opacity * a as f64 / 255.0).round().clamp(0.0, 255.0) as u8;
    let font_px = text.font_px.max(8) as u32;
    let ty = text.y + chrome_height;

    let end_x = text_renderer.draw_text(pixmap, &text.text, text.x, ty, font_px, r, g, b, alpha, scroll_y);

    // Draw underline for links.
    if text.underline || text.href.is_some() {
        let uy = ty - scroll_y + 2;
        let width = end_x - text.x;
        let pw = pixmap.width() as i64;
        let ph = pixmap.height() as i64;
        if uy >= 0 && uy < ph && width > 0 {
            let uy = uy as u32;
            let data = pixmap.data_mut();
            for col in text.x.max(0)..end_x.min(pw) {
                let idx = (uy * pw as u32 + col as u32) as usize * 4;
                if idx + 3 < data.len() {
                    data[idx] = r;
                    data[idx + 1] = g;
                    data[idx + 2] = b;
                    data[idx + 3] = alpha;
                }
            }
        }
    }

    end_x
}

fn draw_image_placeholder(
    pixmap: &mut Pixmap,
    img: &DrawImage,
    text_renderer: &mut TextRenderer,
    scroll_y: i64,
    chrome_height: i64,
) {
    // Draw a gray placeholder box with alt text.
    let placeholder = DrawRect {
        x: img.x,
        y: img.y,
        width: img.width,
        height: img.height,
        background_color: "#d0d0d0".to_string(),
        opacity: img.opacity,
        z_index: img.z_index,
        clip_rect: img.clip_rect,
    };
    draw_rect(pixmap, &placeholder, scroll_y, chrome_height);

    let label = if img.alt.is_empty() {
        "[image]"
    } else {
        &img.alt
    };
    text_renderer.draw_text(
        pixmap,
        label,
        img.x + 4,
        img.y + 14 + chrome_height,
        12,
        0x44,
        0x44,
        0x44,
        255,
        scroll_y,
    );
}
