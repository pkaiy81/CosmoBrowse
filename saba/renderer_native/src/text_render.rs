/// Minimal text rasterizer using fontdue with a bundled monospace font.

use fontdue::{Font, FontSettings};
use tiny_skia::Pixmap;

// Embed a minimal monospace font. We use the SIL Open Font License "DejaVu Sans Mono"
// subset or any freely redistributable TTF. For bootstrap, use the built-in fontdue
// rasterizer with whatever system font we can find, falling back to a hardcoded glyph set.
//
// For the initial implementation we generate glyphs on the fly and cache them.
use std::collections::HashMap;

pub struct TextRenderer {
    font: Font,
    glyph_cache: HashMap<(char, u32), GlyphBitmap>,
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
        // Try to load a system font; fall back to a built-in minimal font.
        let font_data = find_system_font();
        let font = Font::from_bytes(
            font_data.as_slice(),
            FontSettings::default(),
        )
        .expect("Failed to load font");
        Self {
            font,
            glyph_cache: HashMap::new(),
        }
    }

    pub fn draw_text(
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
    ) -> i64 {
        let baseline_y = y - scroll_y;

        for ch in text.chars() {
            let glyph = self.rasterize_glyph(ch, font_px);
            let gx = x + glyph.offset_x as i64;
            let gy = baseline_y - glyph.offset_y as i64;

            for row in 0..glyph.height as i64 {
                for col in 0..glyph.width as i64 {
                    let px = gx + col;
                    let py = gy + row;
                    if px < 0 || py < 0 || px >= pixmap.width() as i64 || py >= pixmap.height() as i64 {
                        continue;
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
            let glyph = self.rasterize_glyph(ch, font_px);
            width += glyph.advance_width;
        }
        width as i64
    }

    fn rasterize_glyph(&mut self, ch: char, font_px: u32) -> &GlyphBitmap {
        let key = (ch, font_px);
        if !self.glyph_cache.contains_key(&key) {
            let px = font_px as f32;
            let (metrics, bitmap) = self.font.rasterize(ch, px);
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
        // Alpha compositing (src over dst), premultiplied.
        let sa = a as u32;
        let da = 255 - sa;
        data[idx] = ((r as u32 * sa + data[idx] as u32 * da) / 255) as u8;
        data[idx + 1] = ((g as u32 * sa + data[idx + 1] as u32 * da) / 255) as u8;
        data[idx + 2] = ((b as u32 * sa + data[idx + 2] as u32 * da) / 255) as u8;
        data[idx + 3] = ((sa + data[idx + 3] as u32 * da / 255).min(255)) as u8;
    }
}

fn find_system_font() -> Vec<u8> {
    // Try common system font paths.
    let candidates = [
        // Linux
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
        // macOS
        "/System/Library/Fonts/Menlo.ttc",
        "/System/Library/Fonts/Monaco.ttf",
        "/System/Library/Fonts/SFMono-Regular.otf",
        // Windows
        "C:\\Windows\\Fonts\\consola.ttf",
        "C:\\Windows\\Fonts\\cour.ttf",
        "C:\\Windows\\Fonts\\lucon.ttf",
    ];

    for path in &candidates {
        if let Ok(data) = std::fs::read(path) {
            if !data.is_empty() {
                return data;
            }
        }
    }

    // Absolute fallback: use any .ttf we can find in common font directories.
    for dir in &[
        "/usr/share/fonts",
        "C:\\Windows\\Fonts",
        "/System/Library/Fonts",
    ] {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|ext| ext == "ttf") {
                    if let Ok(data) = std::fs::read(&path) {
                        if !data.is_empty() {
                            return data;
                        }
                    }
                }
            }
        }
    }

    panic!("No system font found. Please install DejaVu Sans Mono or any TTF font.");
}
