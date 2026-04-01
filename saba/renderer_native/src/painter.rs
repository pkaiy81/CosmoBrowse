/// Renders PaintCommands to a tiny-skia Pixmap.

use std::collections::HashMap;

use cosmo_core::paint_commands::{PaintCommand, DrawRect, DrawText, DrawImage};
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};

use crate::color::parse_css_color;
use crate::hit_test::HitRegion;
use crate::text_render::TextRenderer;

/// Cached decoded image (RGBA pixels).
struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Cache for fetched and decoded images, keyed by URL.
pub struct ImageCache {
    cache: HashMap<String, Option<DecodedImage>>,
}

impl ImageCache {
    pub fn new() -> Self {
        Self {
            cache: HashMap::new(),
        }
    }

    fn get_or_fetch(&mut self, src: &str, base_url: &str) -> Option<&DecodedImage> {
        if !self.cache.contains_key(src) {
            let resolved = resolve_url(src, base_url);
            let decoded = fetch_and_decode(&resolved);
            self.cache.insert(src.to_string(), decoded);
        }
        self.cache.get(src).and_then(|v| v.as_ref())
    }
}

fn resolve_url(src: &str, base_url: &str) -> String {
    if src.starts_with("http://") || src.starts_with("https://") || src.starts_with("data:") {
        return src.to_string();
    }
    // Relative URL resolution.
    if let Some(base) = base_url.rfind('/') {
        format!("{}/{}", &base_url[..base], src)
    } else {
        src.to_string()
    }
}

fn fetch_and_decode(url: &str) -> Option<DecodedImage> {
    let bytes = reqwest::blocking::Client::builder()
        .danger_accept_invalid_certs(false)
        .build()
        .ok()?
        .get(url)
        .send()
        .ok()?
        .bytes()
        .ok()?;

    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    Some(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// Render a list of paint commands to the pixmap.
/// Returns hit regions for clickable elements (links).
pub fn render_commands(
    pixmap: &mut Pixmap,
    commands: &[PaintCommand],
    text_renderer: &mut TextRenderer,
    image_cache: &mut ImageCache,
    base_url: &str,
    scroll_y: i64,
    chrome_height: i64,
    frame_id: &str,
) -> Vec<HitRegion> {
    let mut hit_regions = Vec::new();

    // Sort by z-index, then by paint phase (backgrounds before text/images).
    // This follows CSS painting order: within the same z-index layer,
    // backgrounds (DrawRect) are painted first, then images, then text.
    let mut sorted: Vec<(i32, u8, usize)> = commands
        .iter()
        .enumerate()
        .map(|(i, cmd)| {
            let (z, phase) = match cmd {
                PaintCommand::DrawRect(r) => (r.z_index, 0u8),
                PaintCommand::DrawImage(img) => (img.z_index, 1),
                PaintCommand::DrawText(t) => (t.z_index, 2),
            };
            (z, phase, i)
        })
        .collect();
    sorted.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));

    for &(_, _, idx) in &sorted {
        match &commands[idx] {
            PaintCommand::DrawRect(rect) => {
                draw_rect(pixmap, rect, scroll_y, chrome_height, image_cache, base_url);
            }
            PaintCommand::DrawText(text) => {
                let end_x = draw_text(pixmap, text, text_renderer, scroll_y, chrome_height);
                if let Some(href) = &text.href {
                    let text_width = end_x - text.x;
                    let font_height = text.font_px;
                    hit_regions.push(HitRegion {
                        x: text.x,
                        y: text.y + chrome_height - scroll_y,
                        width: text_width.max(1),
                        height: font_height + 4,
                        href: href.clone(),
                        target: text.target.clone(),
                        frame_id: frame_id.to_string(),
                    });
                }
            }
            PaintCommand::DrawImage(img) => {
                draw_image(pixmap, img, text_renderer, image_cache, base_url, scroll_y, chrome_height);
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

fn draw_rect(pixmap: &mut Pixmap, rect: &DrawRect, scroll_y: i64, chrome_height: i64, image_cache: &mut ImageCache, base_url: &str) {
    let ry = rect.y + chrome_height - scroll_y;
    let screen_clip = rect.clip_rect.map(|(cx, cy, cw, ch)| (cx, cy + chrome_height - scroll_y, cw, ch));
    let clipped = apply_clip(rect.x, ry, rect.width, rect.height, &screen_clip);
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

    // Tile background image if present.
    if let Some(ref bg_src) = rect.background_image {
        if let Some(decoded) = image_cache.get_or_fetch(bg_src, base_url) {
            let pw = pixmap.width() as i64;
            let ph = pixmap.height() as i64;
            let src_w = decoded.width as i64;
            let src_h = decoded.height as i64;
            if src_w > 0 && src_h > 0 {
                let data = pixmap.data_mut();
                // Tile the image across the rect area.
                let mut ty = 0i64;
                while ty < h {
                    let mut tx = 0i64;
                    while tx < w {
                        let tile_w = (src_w).min(w - tx);
                        let tile_h = (src_h).min(h - ty);
                        for dy in 0..tile_h {
                            let py = y + ty + dy;
                            if py < 0 || py >= ph {
                                continue;
                            }
                            for dx in 0..tile_w {
                                let px_x = x + tx + dx;
                                if px_x < 0 || px_x >= pw {
                                    continue;
                                }
                                let si = (dy * src_w + dx) as usize * 4;
                                let sr = decoded.rgba[si];
                                let sg = decoded.rgba[si + 1];
                                let sb = decoded.rgba[si + 2];
                                let sa = decoded.rgba[si + 3];
                                let di = (py * pw + px_x) as usize * 4;
                                if di + 3 >= data.len() {
                                    continue;
                                }
                                if sa == 255 {
                                    data[di] = sr;
                                    data[di + 1] = sg;
                                    data[di + 2] = sb;
                                    data[di + 3] = 255;
                                } else if sa > 0 {
                                    let a = sa as u32;
                                    let inv_a = 255 - a;
                                    data[di] = ((sr as u32 * a + data[di] as u32 * inv_a) / 255) as u8;
                                    data[di + 1] = ((sg as u32 * a + data[di + 1] as u32 * inv_a) / 255) as u8;
                                    data[di + 2] = ((sb as u32 * a + data[di + 2] as u32 * inv_a) / 255) as u8;
                                    data[di + 3] = 255;
                                }
                            }
                        }
                        tx += src_w;
                    }
                    ty += src_h;
                }
            }
        }
    }
}

fn draw_text(
    pixmap: &mut Pixmap,
    text: &DrawText,
    text_renderer: &mut TextRenderer,
    scroll_y: i64,
    chrome_height: i64,
) -> i64 {
    let (r, g, b, a) = parse_css_color(&text.color);
    let alpha = (text.opacity * a as f64).round().clamp(0.0, 255.0) as u8;
    let font_px = text.font_px.max(8) as u32;
    // Layout y is the top of the line box; text_renderer expects the baseline.
    // Approximate baseline = top + font_size (ascent ≈ font_size for most fonts).
    let ty = text.y + chrome_height + font_px as i64;

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

fn draw_image(
    pixmap: &mut Pixmap,
    img: &DrawImage,
    text_renderer: &mut TextRenderer,
    image_cache: &mut ImageCache,
    base_url: &str,
    scroll_y: i64,
    chrome_height: i64,
) {
    let iy = img.y + chrome_height - scroll_y;

    // Try to fetch and render the actual image.
    if !img.src.is_empty() {
        if let Some(decoded) = image_cache.get_or_fetch(&img.src, base_url) {
            let pw = pixmap.width() as i64;
            let ph = pixmap.height() as i64;
            let src_w = decoded.width as i64;
            let src_h = decoded.height as i64;
            let dst_w = img.width;
            let dst_h = img.height;
            let data = pixmap.data_mut();

            for dy in 0..dst_h {
                let py = iy + dy;
                if py < 0 || py >= ph {
                    continue;
                }
                for dx in 0..dst_w {
                    let px = img.x + dx;
                    if px < 0 || px >= pw {
                        continue;
                    }
                    // Nearest-neighbor sampling from source.
                    let sx = (dx * src_w / dst_w).min(src_w - 1);
                    let sy = (dy * src_h / dst_h).min(src_h - 1);
                    let si = (sy * src_w + sx) as usize * 4;
                    let sr = decoded.rgba[si];
                    let sg = decoded.rgba[si + 1];
                    let sb = decoded.rgba[si + 2];
                    let sa = decoded.rgba[si + 3];

                    let di = (py * pw + px) as usize * 4;
                    if di + 3 >= data.len() {
                        continue;
                    }
                    // Alpha compositing.
                    if sa == 255 {
                        data[di] = sr;
                        data[di + 1] = sg;
                        data[di + 2] = sb;
                        data[di + 3] = 255;
                    } else if sa > 0 {
                        let a = sa as u32;
                        let inv_a = 255 - a;
                        data[di] = ((sr as u32 * a + data[di] as u32 * inv_a) / 255) as u8;
                        data[di + 1] = ((sg as u32 * a + data[di + 1] as u32 * inv_a) / 255) as u8;
                        data[di + 2] = ((sb as u32 * a + data[di + 2] as u32 * inv_a) / 255) as u8;
                        data[di + 3] = 255;
                    }
                }
            }
            return;
        }
    }

    // Fallback: gray placeholder with alt text.
    let placeholder = DrawRect {
        x: img.x,
        y: img.y,
        width: img.width,
        height: img.height,
        background_color: "#d0d0d0".to_string(),
        background_image: None,
        opacity: img.opacity,
        z_index: img.z_index,
        clip_rect: img.clip_rect,
    };
    draw_rect(pixmap, &placeholder, scroll_y, chrome_height, image_cache, base_url);

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
