/// Text rasterizer using fontdue with font fallback chain.

use cosmo_engine::renderer::layout::computed_style::FontSize;
use cosmo_engine::renderer::text::provider::FontMetricsProvider;
use fontdue::{Font, FontSettings};
use tiny_skia::Pixmap;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Font chains are loaded once and shared between the rasterizer and the
/// layout metrics provider so both measure and draw with the same glyphs.
fn shared_font_chains() -> &'static (Arc<Vec<Font>>, Arc<Vec<Font>>) {
    static CHAINS: OnceLock<(Arc<Vec<Font>>, Arc<Vec<Font>>)> = OnceLock::new();
    CHAINS.get_or_init(|| (Arc::new(load_font_chain()), Arc::new(load_bold_font_chain())))
}

/// Layout-side metrics from the same fontdue fonts the painter draws with.
/// Advances are rounded UP so layout never reserves less than is drawn.
pub struct FontdueMetricsProvider {
    fonts: Arc<Vec<Font>>,
    bold_fonts: Arc<Vec<Font>>,
    cache: Mutex<HashMap<(char, i64, bool), i64>>,
}

impl std::fmt::Debug for FontdueMetricsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontdueMetricsProvider").finish()
    }
}

impl FontdueMetricsProvider {
    pub fn new() -> Self {
        let (fonts, bold_fonts) = shared_font_chains().clone();
        Self {
            fonts,
            bold_fonts,
            cache: Mutex::new(HashMap::new()),
        }
    }
}

impl FontMetricsProvider for FontdueMetricsProvider {
    fn char_advance(&self, c: char, font_size: FontSize, bold: bool) -> i64 {
        let px = font_size.px();
        let key = (c, px, bold);
        if let Some(v) = self.cache.lock().unwrap().get(&key) {
            return *v;
        }
        let primary: &[Font] = if bold && !self.bold_fonts.is_empty() {
            &self.bold_fonts
        } else {
            &self.fonts
        };
        let fallback: &[Font] = if bold && !self.bold_fonts.is_empty() {
            &self.fonts
        } else {
            &[]
        };
        let mut advance = None;
        for font in primary.iter().chain(fallback.iter()) {
            if font.lookup_glyph_index(c) == 0 && c != '\0' {
                continue;
            }
            let m = font.metrics(c, px as f32);
            if m.advance_width > 0.0 || c.is_whitespace() {
                advance = Some(m.advance_width);
                break;
            }
        }
        let advance = advance.unwrap_or_else(|| {
            primary
                .first()
                .map(|f| f.metrics(c, px as f32).advance_width)
                .unwrap_or(px as f32 / 2.0)
        });
        let v = (advance.ceil() as i64).max(1);
        self.cache.lock().unwrap().insert(key, v);
        v
    }
}

pub struct TextRenderer {
    fonts: Arc<Vec<Font>>,
    bold_fonts: Arc<Vec<Font>>,
    glyph_cache: HashMap<(char, u32, bool), GlyphBitmap>,
}

struct GlyphBitmap {
    width: u32,
    height: u32,
    advance_width: f32,
    bitmap: Vec<u8>,
    offset_x: i32,
    offset_y: i32,
}

impl TextRenderer {
    pub fn new() -> Self {
        let (fonts, bold_fonts) = shared_font_chains().clone();
        Self {
            fonts,
            bold_fonts,
            glyph_cache: HashMap::new(),
        }
    }

    pub fn draw_text(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        x: i64,
        y: i64,
        font_px: u32,
        r: u8,
        g: u8,
        b: u8,
        alpha: u8,
        scroll_y: i64,
        bold: bool,
    ) -> i64 {
        self.draw_text_clipped(pixmap, text, x, y, font_px, r, g, b, alpha, scroll_y, bold, None)
    }

    /// Like `draw_text`, but glyph pixels outside `clip` (screen-space
    /// x, y, w, h) are skipped — overflow containers clip their text.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text_clipped(
        &mut self,
        pixmap: &mut Pixmap,
        text: &str,
        mut x: i64,
        y: i64,
        font_px: u32,
        r: u8,
        g: u8,
        b: u8,
        alpha: u8,
        scroll_y: i64,
        bold: bool,
        clip: Option<(i64, i64, i64, i64)>,
    ) -> i64 {
        let baseline_y = y - scroll_y;

        for ch in text.chars() {
            let glyph = self.rasterize_glyph(ch, font_px, bold);
            let gx = x + glyph.offset_x as i64;
            let gy = baseline_y - glyph.offset_y as i64;

            for row in 0..glyph.height as i64 {
                for col in 0..glyph.width as i64 {
                    let px = gx + col;
                    let py = gy + row;
                    if px < 0 || py < 0 || px >= pixmap.width() as i64 || py >= pixmap.height() as i64 {
                        continue;
                    }
                    if let Some((cx, cy, cw, ch_)) = clip {
                        if px < cx || px >= cx + cw || py < cy || py >= cy + ch_ {
                            continue;
                        }
                    }
                    let coverage = glyph.bitmap[(row * glyph.width as i64 + col) as usize];
                    if coverage == 0 {
                        continue;
                    }
                    let a = (coverage as u16 * alpha as u16 / 255) as u8;
                    blend_pixel(pixmap, px as u32, py as u32, r, g, b, a);
                }
            }

            x += glyph.advance_width as i64;
        }
        x
    }

    pub fn measure_text(&mut self, text: &str, font_px: u32) -> i64 {
        let mut width: f32 = 0.0;
        for ch in text.chars() {
            let glyph = self.rasterize_glyph(ch, font_px, false);
            width += glyph.advance_width;
        }
        width as i64
    }

    fn rasterize_glyph(&mut self, ch: char, font_px: u32, bold: bool) -> &GlyphBitmap {
        let key = (ch, font_px, bold);
        if !self.glyph_cache.contains_key(&key) {
            let px = font_px as f32;

            // Select font chain based on bold flag.
            // Fall back to regular fonts if bold chain is empty.
            let primary_chain: &[Font] = if bold && !self.bold_fonts.is_empty() {
                &self.bold_fonts
            } else {
                &self.fonts
            };
            let fallback_chain: &[Font] = if bold && !self.bold_fonts.is_empty() {
                &self.fonts
            } else {
                &[]
            };

            // Try each font in the chain; use the first one that produces a real glyph.
            let mut best_metrics = None;
            let mut best_bitmap = None;
            for font in primary_chain.iter().chain(fallback_chain.iter()) {
                // Check if this font has the glyph (not .notdef).
                let glyph_index = font.lookup_glyph_index(ch);
                if glyph_index == 0 && ch != '\0' {
                    continue; // .notdef — try next font
                }
                let (metrics, bitmap) = font.rasterize(ch, px);
                // Accept if it has actual pixels or if it's a space-like character.
                if metrics.width > 0 || ch.is_whitespace() || metrics.advance_width > 0.0 {
                    best_metrics = Some(metrics);
                    best_bitmap = Some(bitmap);
                    break;
                }
            }

            // Final fallback: rasterize from first available font even if .notdef.
            let first_font = if !self.bold_fonts.is_empty() && bold {
                &self.bold_fonts[0]
            } else {
                &self.fonts[0]
            };
            let (metrics, bitmap) = match (best_metrics, best_bitmap) {
                (Some(m), Some(b)) => (m, b),
                _ => first_font.rasterize(ch, px),
            };

            self.glyph_cache.insert(key, GlyphBitmap {
                width: metrics.width as u32,
                height: metrics.height as u32,
                advance_width: metrics.advance_width,
                bitmap,
                offset_x: metrics.xmin,
                offset_y: metrics.ymin + metrics.height as i32,
            });
        }
        &self.glyph_cache[&key]
    }
}

fn blend_pixel(pixmap: &mut Pixmap, x: u32, y: u32, r: u8, g: u8, b: u8, a: u8) {
    if a == 0 {
        return;
    }
    let idx = (y * pixmap.width() + x) as usize * 4;
    let data = pixmap.data_mut();
    if idx + 3 >= data.len() {
        return;
    }
    if a == 255 {
        data[idx] = r;
        data[idx + 1] = g;
        data[idx + 2] = b;
        data[idx + 3] = 255;
    } else {
        let sa = a as u32;
        let da = 255 - sa;
        data[idx] = ((r as u32 * sa + data[idx] as u32 * da) / 255) as u8;
        data[idx + 1] = ((g as u32 * sa + data[idx + 1] as u32 * da) / 255) as u8;
        data[idx + 2] = ((b as u32 * sa + data[idx + 2] as u32 * da) / 255) as u8;
        data[idx + 3] = ((sa + data[idx + 3] as u32 * da / 255).min(255)) as u8;
    }
}

fn load_bold_font_chain() -> Vec<Font> {
    let mut fonts = Vec::new();

    // Bold Latin font.
    let bold_latin_candidates = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        "C:\\Windows\\Fonts\\arialbd.ttf",
    ];
    if let Some(font) = try_load_font(&bold_latin_candidates) {
        eprintln!("[FONT] Bold (Latin) loaded");
        fonts.push(font);
    }

    // Bold CJK font for Japanese/Chinese/Korean bold text.
    let bold_cjk_candidates = [
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
        "/usr/share/fonts/truetype/vlgothic/VL-Gothic-Regular.ttf",
        "/usr/share/fonts/truetype/takao-gothic/TakaoGothic.ttf",
        "/usr/share/fonts/truetype/ipa/ipagp.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
    ];
    if let Some(font) = try_load_font(&bold_cjk_candidates) {
        eprintln!("[FONT] Bold CJK loaded");
        fonts.push(font);
    }

    fonts
}

fn load_font_chain() -> Vec<Font> {
    let mut fonts = Vec::new();

    // Latin/symbol font (primary).
    let latin_candidates = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/System/Library/Fonts/Monaco.ttf",
        "C:\\Windows\\Fonts\\consola.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
    ];
    if let Some(font) = try_load_font(&latin_candidates) {
        eprintln!("[FONT] Primary (Latin) loaded");
        fonts.push(font);
    }

    // CJK font (fallback for Japanese/Chinese/Korean).
    let cjk_candidates = [
        "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
        "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
        "/usr/share/fonts/truetype/vlgothic/VL-Gothic-Regular.ttf",
        "/usr/share/fonts/truetype/takao-gothic/TakaoGothic.ttf",
        "/usr/share/fonts/truetype/ipa/ipag.ttf",
        "/usr/share/fonts/truetype/ipa/ipagp.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "C:\\Windows\\Fonts\\msgothic.ttc",
        "C:\\Windows\\Fonts\\meiryo.ttc",
        "C:\\Windows\\Fonts\\YuGothR.ttc",
    ];
    if let Some(font) = try_load_font(&cjk_candidates) {
        eprintln!("[FONT] Fallback (CJK) loaded");
        fonts.push(font);
    }

    if fonts.is_empty() {
        // Absolute fallback: try any .ttf in font directories.
        for dir in &["/usr/share/fonts/truetype", "C:\\Windows\\Fonts", "/System/Library/Fonts"] {
            if let Some(font) = find_any_ttf_in_dir(dir) {
                eprintln!("[FONT] Emergency fallback loaded from {}", dir);
                fonts.push(font);
                break;
            }
        }
    }

    if fonts.is_empty() {
        panic!("No system font found. Please install DejaVu Sans or any TTF font.");
    }

    fonts
}

fn try_load_font(candidates: &[&str]) -> Option<Font> {
    for path in candidates {
        if let Ok(data) = std::fs::read(path) {
            if data.is_empty() {
                continue;
            }
            // Try default collection index first, then index 0 explicitly.
            for idx in [0u32, 1, 2, 3] {
                let settings = FontSettings {
                    collection_index: idx,
                    scale: 40.0,
                    load_substitutions: false,
                };
                if let Ok(font) = Font::from_bytes(data.as_slice(), settings) {
                    // Verify the font can actually rasterize a basic character.
                    let (metrics, _) = font.rasterize('A', 16.0);
                    if metrics.advance_width > 0.0 {
                        eprintln!("[FONT]   {} (index {})", path, idx);
                        return Some(font);
                    }
                }
            }
        }
    }
    None
}

fn find_any_ttf_in_dir(dir: &str) -> Option<Font> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(font) = find_any_ttf_in_dir(&path.to_string_lossy()) {
                return Some(font);
            }
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "ttf") {
            if let Ok(data) = std::fs::read(&path) {
                if let Ok(font) = Font::from_bytes(data.as_slice(), FontSettings::default()) {
                    let (metrics, _) = font.rasterize('A', 16.0);
                    if metrics.advance_width > 0.0 {
                        return Some(font);
                    }
                }
            }
        }
    }
    None
}
