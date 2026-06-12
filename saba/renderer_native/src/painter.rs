/// Renders PaintCommands to a tiny-skia Pixmap.
use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::OnceLock;
use std::time::Duration;

use cosmo_core::paint_commands::{DrawImage, DrawRect, DrawText, PaintCommand};
use tiny_skia::{Color, Paint, Pixmap, Rect, Transform};
use winit::event_loop::EventLoopProxy;

use crate::color::parse_css_color;
use crate::hit_test::HitRegion;
use crate::text_render::TextRenderer;
use crate::UserEvent;

/// Cached decoded image (RGBA pixels).
struct DecodedImage {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

/// Lifecycle of an image in the cache.
enum ImageState {
    /// A background fetch is in flight; paint a placeholder until it resolves.
    Pending,
    /// The fetch finished. `None` means it failed (or was a non-fetchable
    /// scheme); this is cached so a redraw never re-attempts it.
    Done(Option<DecodedImage>),
}

/// Result delivered from a background fetch thread back to the cache.
struct FetchResult {
    key: String,
    decoded: Option<DecodedImage>,
}

/// Cache for fetched and decoded images, keyed by the (unresolved) `src`.
///
/// Images are fetched off the UI thread so a slow or unreachable sub-resource
/// can never freeze the window. When a `notifier` proxy is installed (GUI
/// mode), `get_or_fetch` spawns a background thread, records the entry as
/// `Pending`, and the thread wakes the event loop (`UserEvent::Redraw`) once
/// the bytes arrive. Without a notifier (headless screenshot mode) fetches run
/// synchronously, so the single paint observes the decoded image.
pub struct ImageCache {
    cache: HashMap<String, ImageState>,
    notifier: Option<EventLoopProxy<UserEvent>>,
    result_tx: Sender<FetchResult>,
    result_rx: Receiver<FetchResult>,
}

impl ImageCache {
    pub fn new() -> Self {
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        Self {
            cache: HashMap::new(),
            notifier: None,
            result_tx,
            result_rx,
        }
    }

    /// Switch the cache into asynchronous mode. Image fetches will run on
    /// background threads and `proxy` is signalled whenever one completes so
    /// the window can repaint with the newly-available image.
    pub fn set_notifier(&mut self, proxy: EventLoopProxy<UserEvent>) {
        self.notifier = Some(proxy);
    }

    /// Drain completed background fetches into the cache. Call once at the
    /// start of each paint so freshly-arrived images become visible.
    pub fn integrate_results(&mut self) {
        while let Ok(FetchResult { key, decoded }) = self.result_rx.try_recv() {
            self.cache.insert(key, ImageState::Done(decoded));
        }
    }

    fn get_or_fetch(&mut self, src: &str, base_url: &str) -> Option<&DecodedImage> {
        if !self.cache.contains_key(src) {
            let resolved = resolve_url(src, base_url);
            match self.notifier.clone() {
                // Async: never block the UI thread on network I/O. Mark the
                // entry Pending so subsequent paints don't re-spawn the fetch.
                Some(proxy) => {
                    self.cache.insert(src.to_string(), ImageState::Pending);
                    let key = src.to_string();
                    let tx = self.result_tx.clone();
                    std::thread::spawn(move || {
                        let decoded = fetch_and_decode(&resolved);
                        // A send error means the window has closed; just exit.
                        if tx.send(FetchResult { key, decoded }).is_ok() {
                            let _ = proxy.send_event(UserEvent::Redraw);
                        }
                    });
                }
                // Sync (headless): fetch inline so this paint sees the image.
                None => {
                    let decoded = fetch_and_decode(&resolved);
                    self.cache
                        .insert(src.to_string(), ImageState::Done(decoded));
                }
            }
        }
        match self.cache.get(src) {
            Some(ImageState::Done(Some(img))) => Some(img),
            _ => None,
        }
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

// Images are non-critical sub-resources fetched serially on the UI thread, so
// a request that connects but never delivers data must not stall the renderer.
// These timeouts are deliberately shorter than the page loader's (10s/20s):
// many images per page compound, and a dead host should fail fast.
const IMAGE_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const IMAGE_REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// Shared HTTP client for image fetches. Built once so connections are pooled
/// and reused across the many images on a page.
fn image_http_client() -> &'static reqwest::blocking::Client {
    static CLIENT: OnceLock<reqwest::blocking::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::blocking::Client::builder()
            .connect_timeout(IMAGE_CONNECT_TIMEOUT)
            .timeout(IMAGE_REQUEST_TIMEOUT)
            .build()
            .expect("failed to build image HTTP client")
    })
}

fn fetch_and_decode(url: &str) -> Option<DecodedImage> {
    let bytes = image_http_client()
        .get(url)
        .send()
        .ok()?
        .bytes()
        .ok()?;

    // SVG (e.g. Hacker News' votearrow triangle.svg) — rasterize at the
    // document's intrinsic size via resvg; the raster decoders below can't
    // handle it.
    let head = &bytes[..bytes.len().min(512)];
    let looks_like_svg = url.split('?').next().is_some_and(|p| p.ends_with(".svg"))
        || head.starts_with(b"<svg")
        || (head.starts_with(b"<?xml") && bytes.windows(4).take(512).any(|w| w == b"<svg"));
    if looks_like_svg {
        return decode_svg(&bytes);
    }

    let img = image::load_from_memory(&bytes).ok()?;
    let rgba = img.to_rgba8();
    Some(DecodedImage {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

fn decode_svg(bytes: &[u8]) -> Option<DecodedImage> {
    let tree = resvg::usvg::Tree::from_data(bytes, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let (w, h) = (size.width().ceil() as u32, size.height().ceil() as u32);
    if w == 0 || h == 0 || w > 4096 || h > 4096 {
        return None;
    }
    let mut pixmap = tiny_skia::Pixmap::new(w, h)?;
    resvg::render(&tree, tiny_skia::Transform::identity(), &mut pixmap.as_mut());
    // tiny-skia stores PREMULTIPLIED rgba; the blit/blend paths in this file
    // expect straight alpha (like the `image` crate produces) — unpremultiply.
    let mut rgba = pixmap.take();
    for px in rgba.chunks_exact_mut(4) {
        let a = px[3] as u32;
        if a > 0 && a < 255 {
            px[0] = ((px[0] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[1] = ((px[1] as u32 * 255 + a / 2) / a).min(255) as u8;
            px[2] = ((px[2] as u32 * 255 + a / 2) / a).min(255) as u8;
        }
    }
    Some(DecodedImage {
        width: w,
        height: h,
        rgba,
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
        // position:fixed boxes are viewport-anchored: paint (and hit-test)
        // them with a zero scroll offset so they stay put while the page
        // scrolls underneath.
        match &commands[idx] {
            PaintCommand::DrawRect(rect) => {
                let scroll = if rect.fixed { 0 } else { scroll_y };
                draw_rect(pixmap, rect, scroll, chrome_height, image_cache, base_url);
            }
            PaintCommand::DrawText(text) => {
                let scroll = if text.fixed { 0 } else { scroll_y };
                let end_x = draw_text(pixmap, text, text_renderer, scroll, chrome_height);
                if let Some(href) = &text.href {
                    let text_width = end_x - text.x;
                    let font_height = text.font_px;
                    hit_regions.push(HitRegion {
                        x: text.x,
                        y: text.y + chrome_height - scroll,
                        width: text_width.max(1),
                        height: font_height + 4,
                        href: href.clone(),
                        target: text.target.clone(),
                        frame_id: frame_id.to_string(),
                    });
                }
            }
            PaintCommand::DrawImage(img) => {
                let scroll = if img.fixed { 0 } else { scroll_y };
                draw_image(
                    pixmap,
                    img,
                    text_renderer,
                    image_cache,
                    base_url,
                    scroll,
                    chrome_height,
                );
                if let Some(href) = &img.href {
                    hit_regions.push(HitRegion {
                        x: img.x,
                        y: img.y + chrome_height - scroll,
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

fn apply_clip(
    x: i64,
    y: i64,
    w: i64,
    h: i64,
    clip: &Option<(i64, i64, i64, i64)>,
) -> Option<(i64, i64, i64, i64)> {
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

fn draw_rect(
    pixmap: &mut Pixmap,
    rect: &DrawRect,
    scroll_y: i64,
    chrome_height: i64,
    image_cache: &mut ImageCache,
    base_url: &str,
) {
    // Spec: CSS Backgrounds §2.11.2 — the background of the root element is
    // propagated to the viewport canvas, and must cover the entire viewport
    // regardless of the document's computed height.
    // https://www.w3.org/TR/css-backgrounds-3/#special-backgrounds
    // Detect the page-canvas background: a full-width rect at document top
    // (y=0 in absolute coordinates) with no explicit clip.  The layout engine
    // starts from the <body> element, so in a frameset the body rect for the
    // right frame starts at its frame x-offset (e.g. x=184), not at x=0.
    // We therefore only check y=0 and large-width, not x=0.
    // Use 1/8 of viewport width as threshold to also cover narrow frames
    // (e.g. 18% nav frames) while still excluding small element backgrounds.
    // A position:fixed box is never the page canvas, even when it sits at
    // y=0 spanning the viewport (a fixed nav bar would be stretched into a
    // full-screen slab by this propagation).
    let effective_height = if rect.y == 0
        && !rect.fixed
        && rect.clip_rect.is_none()
        && rect.width >= pixmap.width() as i64 / 8
    {
        let min_fill = pixmap.height() as i64 + scroll_y - chrome_height;
        rect.height.max(min_fill)
    } else {
        rect.height
    };

    let ry = rect.y + chrome_height - scroll_y;
    let screen_clip = rect
        .clip_rect
        .map(|(cx, cy, cw, ch)| (cx, cy + chrome_height - scroll_y, cw, ch));
    let clipped = apply_clip(rect.x, ry, rect.width, effective_height, &screen_clip);
    let Some((x, y, w, h)) = clipped else { return };

    let (r, g, b, a) = parse_css_color(&rect.background_color);
    let opacity = (rect.opacity * a as f64 / 255.0).clamp(0.0, 1.0) as f32;

    let Some(skia_rect) = Rect::from_xywh(x as f32, y as f32, w as f32, h as f32) else {
        return;
    };

    let mut paint = Paint::default();
    paint.set_color(
        Color::from_rgba(
            r as f32 / 255.0,
            g as f32 / 255.0,
            b as f32 / 255.0,
            opacity,
        )
        .unwrap_or(Color::BLACK),
    );
    paint.anti_alias = false;

    pixmap.fill_rect(skia_rect, &paint, Transform::identity(), None);

    // Draw border strokes (CSS box-model: border inside the element's box).
    if rect.border_width > 0 && !rect.border_color.is_empty() {
        let bw = rect.border_width;
        let (br, bg_c, bb, _) = parse_css_color(&rect.border_color);
        let border_opacity = (rect.opacity).clamp(0.0, 1.0) as f32;
        let mut bp = Paint::default();
        bp.set_color(
            Color::from_rgba(
                br as f32 / 255.0,
                bg_c as f32 / 255.0,
                bb as f32 / 255.0,
                border_opacity,
            )
            .unwrap_or(Color::BLACK),
        );
        bp.anti_alias = false;

        // Top border
        if let Some((cx, cy, cw, ch)) = apply_clip(x, y, w, bw, &screen_clip) {
            if let Some(sr) = Rect::from_xywh(cx as f32, cy as f32, cw as f32, ch as f32) {
                pixmap.fill_rect(sr, &bp, Transform::identity(), None);
            }
        }
        // Bottom border
        if let Some((cx, cy, cw, ch)) = apply_clip(x, y + h - bw, w, bw, &screen_clip) {
            if let Some(sr) = Rect::from_xywh(cx as f32, cy as f32, cw as f32, ch as f32) {
                pixmap.fill_rect(sr, &bp, Transform::identity(), None);
            }
        }
        // Left border
        if let Some((cx, cy, cw, ch)) = apply_clip(x, y, bw, h, &screen_clip) {
            if let Some(sr) = Rect::from_xywh(cx as f32, cy as f32, cw as f32, ch as f32) {
                pixmap.fill_rect(sr, &bp, Transform::identity(), None);
            }
        }
        // Right border
        if let Some((cx, cy, cw, ch)) = apply_clip(x + w - bw, y, bw, h, &screen_clip) {
            if let Some(sr) = Rect::from_xywh(cx as f32, cy as f32, cw as f32, ch as f32) {
                pixmap.fill_rect(sr, &bp, Transform::identity(), None);
            }
        }
    }

    // Draw the background image if present.
    if let Some(ref bg_src) = rect.background_image {
        if let Some(decoded) = image_cache.get_or_fetch(bg_src, base_url) {
            let pw = pixmap.width() as i64;
            let ph = pixmap.height() as i64;
            let src_w = decoded.width as i64;
            let src_h = decoded.height as i64;
            if src_w > 0 && src_h > 0 {
                // Resolve background-size into the painted image dimensions.
                // Spec: CSS Backgrounds §3.9.
                let (target_w, target_h) = match rect.background_size {
                    // cover: fill the box, preserving ratio (crop overflow).
                    Some((1, ..)) => {
                        let s = (w as f64 / src_w as f64).max(h as f64 / src_h as f64);
                        ((src_w as f64 * s) as i64, (src_h as f64 * s) as i64)
                    }
                    // contain: fit within the box, preserving ratio.
                    Some((2, ..)) => {
                        let s = (w as f64 / src_w as f64).min(h as f64 / src_h as f64);
                        ((src_w as f64 * s) as i64, (src_h as f64 * s) as i64)
                    }
                    // Explicit dimensions; a negative value means auto, which
                    // preserves the ratio against the other axis (both auto =
                    // intrinsic size).
                    Some((_, sw, swp, sh, shp)) => {
                        let rw = if sw < 0.0 {
                            None
                        } else if swp {
                            Some((w as f64 * sw / 100.0) as i64)
                        } else {
                            Some(sw as i64)
                        };
                        let rh = if sh < 0.0 {
                            None
                        } else if shp {
                            Some((h as f64 * sh / 100.0) as i64)
                        } else {
                            Some(sh as i64)
                        };
                        match (rw, rh) {
                            (Some(tw), Some(th)) => (tw, th),
                            (Some(tw), None) => (tw, (src_h * tw) / src_w.max(1)),
                            (None, Some(th)) => ((src_w * th) / src_h.max(1), th),
                            (None, None) => (src_w, src_h),
                        }
                    }
                    // No background-size: an image larger than the rect with
                    // no explicit position is an icon — scale to fit (legacy
                    // behavior); otherwise draw at intrinsic size.
                    None => {
                        if rect.background_position.is_none() && (src_w > w || src_h > h) {
                            (w, h)
                        } else {
                            (src_w, src_h)
                        }
                    }
                };
                let (target_w, target_h) = (target_w.max(1), target_h.max(1));
                // background-position: percentages resolve against
                // (box − painted image), pixel offsets pass through (negative
                // offsets crop into a sprite sheet). Spec: CSS Backgrounds §3.6.
                let resolve = |v: f64, is_pct: bool, box_dim: i64, img_dim: i64| -> i64 {
                    if is_pct {
                        ((box_dim - img_dim) as f64 * v / 100.0) as i64
                    } else {
                        v as i64
                    }
                };
                let (pos_x, pos_y) = match rect.background_position {
                    Some((px, pxp, py, pyp)) => {
                        (resolve(px, pxp, w, target_w), resolve(py, pyp, h, target_h))
                    }
                    None => (0, 0),
                };
                let no_repeat = rect.background_no_repeat;
                let data = pixmap.data_mut();
                for dy in 0..h {
                    let py = y + dy;
                    if py < 0 || py >= ph {
                        continue;
                    }
                    for dx in 0..w {
                        let px_x = x + dx;
                        if px_x < 0 || px_x >= pw {
                            continue;
                        }
                        // Position offset in painted-image space, then sample
                        // the source scaled to the target dimensions.
                        let rel_x = dx - pos_x;
                        let rel_y = dy - pos_y;
                        if no_repeat
                            && (rel_x < 0 || rel_x >= target_w || rel_y < 0 || rel_y >= target_h)
                        {
                            continue;
                        }
                        let rel_x = rel_x.rem_euclid(target_w);
                        let rel_y = rel_y.rem_euclid(target_h);
                        let sx = (rel_x * src_w / target_w).min(src_w - 1);
                        let sy = (rel_y * src_h / target_h).min(src_h - 1);
                        let si = (sy * src_w + sx) as usize * 4;
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
                            data[di + 1] =
                                ((sg as u32 * a + data[di + 1] as u32 * inv_a) / 255) as u8;
                            data[di + 2] =
                                ((sb as u32 * a + data[di + 2] as u32 * inv_a) / 255) as u8;
                            data[di + 3] = 255;
                        }
                    }
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

    let end_x = text_renderer.draw_text(
        pixmap, &text.text, text.x, ty, font_px, r, g, b, alpha, scroll_y, text.bold,
    );

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
        anchor_id: None,
        border_width: 0,
        border_color: String::new(),
        background_position: None,
        background_no_repeat: false,
        background_size: None,
        fixed: img.fixed,
    };
    draw_rect(
        pixmap,
        &placeholder,
        scroll_y,
        chrome_height,
        image_cache,
        base_url,
    );

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
        false,
    );
}
