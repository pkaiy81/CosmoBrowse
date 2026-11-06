/// Text rasterizer using fontdue with font fallback chain.

use cosmo_engine::renderer::layout::computed_style::FontSize;
use cosmo_engine::renderer::text::face::{FontClass, TextFace};
use cosmo_engine::renderer::text::provider::FontMetricsProvider;
use fontdue::{Font, FontSettings};
use tiny_skia::Pixmap;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// Which installed font stands in for a face, best first. Liberation's fonts are
/// metric-compatible with Arial, Times New Roman and Courier New, which is what
/// pages are usually laid out against; DejaVu Sans is close to Verdana.
fn candidates(class: FontClass, bold: bool, italic: bool) -> &'static [&'static str] {
    match (class, bold, italic) {
        (FontClass::Sans, false, false) => &[
            "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
            "C:\\Windows\\Fonts\\arial.ttf",
            "/System/Library/Fonts/Supplemental/Arial.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
        ],
        (FontClass::Sans, true, false) => &[
            "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Bold.ttf",
            "C:\\Windows\\Fonts\\arialbd.ttf",
            "/System/Library/Fonts/Supplemental/Arial Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
        ],
        (FontClass::Sans, false, true) => &[
            "/usr/share/fonts/truetype/liberation/LiberationSans-Italic.ttf",
            "/usr/share/fonts/liberation/LiberationSans-Italic.ttf",
            "C:\\Windows\\Fonts\\ariali.ttf",
            "/System/Library/Fonts/Supplemental/Arial Italic.ttf",
        ],
        (FontClass::Sans, true, true) => &[
            "/usr/share/fonts/truetype/liberation/LiberationSans-BoldItalic.ttf",
            "/usr/share/fonts/liberation/LiberationSans-BoldItalic.ttf",
            "C:\\Windows\\Fonts\\arialbi.ttf",
            "/System/Library/Fonts/Supplemental/Arial Bold Italic.ttf",
        ],
        (FontClass::Serif, false, false) => &[
            "/usr/share/fonts/truetype/liberation/LiberationSerif-Regular.ttf",
            "/usr/share/fonts/liberation/LiberationSerif-Regular.ttf",
            "C:\\Windows\\Fonts\\times.ttf",
            "/System/Library/Fonts/Supplemental/Times New Roman.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif.ttf",
            "/usr/share/fonts/TTF/DejaVuSerif.ttf",
        ],
        (FontClass::Serif, true, false) => &[
            "/usr/share/fonts/truetype/liberation/LiberationSerif-Bold.ttf",
            "/usr/share/fonts/liberation/LiberationSerif-Bold.ttf",
            "C:\\Windows\\Fonts\\timesbd.ttf",
            "/System/Library/Fonts/Supplemental/Times New Roman Bold.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSerif-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSerif-Bold.ttf",
        ],
        (FontClass::Serif, false, true) => &[
            "/usr/share/fonts/truetype/liberation/LiberationSerif-Italic.ttf",
            "/usr/share/fonts/liberation/LiberationSerif-Italic.ttf",
            "C:\\Windows\\Fonts\\timesi.ttf",
            "/System/Library/Fonts/Supplemental/Times New Roman Italic.ttf",
        ],
        (FontClass::Serif, true, true) => &[
            "/usr/share/fonts/truetype/liberation/LiberationSerif-BoldItalic.ttf",
            "/usr/share/fonts/liberation/LiberationSerif-BoldItalic.ttf",
            "C:\\Windows\\Fonts\\timesbi.ttf",
            "/System/Library/Fonts/Supplemental/Times New Roman Bold Italic.ttf",
        ],
        (FontClass::Mono, false, false) => &[
            "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
            "/usr/share/fonts/liberation/LiberationMono-Regular.ttf",
            "C:\\Windows\\Fonts\\cour.ttf",
            "C:\\Windows\\Fonts\\consola.ttf",
            "/System/Library/Fonts/Monaco.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
        ],
        (FontClass::Mono, true, false) => &[
            "/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf",
            "/usr/share/fonts/liberation/LiberationMono-Bold.ttf",
            "C:\\Windows\\Fonts\\courbd.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf",
        ],
        (FontClass::Mono, false, true) => &[
            "/usr/share/fonts/truetype/liberation/LiberationMono-Italic.ttf",
            "/usr/share/fonts/liberation/LiberationMono-Italic.ttf",
            "C:\\Windows\\Fonts\\couri.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Oblique.ttf",
        ],
        (FontClass::Mono, true, true) => &[
            "/usr/share/fonts/truetype/liberation/LiberationMono-BoldItalic.ttf",
            "/usr/share/fonts/liberation/LiberationMono-BoldItalic.ttf",
            "C:\\Windows\\Fonts\\courbi.ttf",
            "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-BoldOblique.ttf",
        ],
        (FontClass::WideSans, false, false) => &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
            "/usr/share/fonts/TTF/DejaVuSans.ttf",
            "C:\\Windows\\Fonts\\verdana.ttf",
            "/System/Library/Fonts/Supplemental/Verdana.ttf",
        ],
        (FontClass::WideSans, true, false) => &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Bold.ttf",
            "C:\\Windows\\Fonts\\verdanab.ttf",
            "/System/Library/Fonts/Supplemental/Verdana Bold.ttf",
        ],
        (FontClass::WideSans, false, true) => &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-Oblique.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-Oblique.ttf",
            "C:\\Windows\\Fonts\\verdanai.ttf",
        ],
        (FontClass::WideSans, true, true) => &[
            "/usr/share/fonts/truetype/dejavu/DejaVuSans-BoldOblique.ttf",
            "/usr/share/fonts/TTF/DejaVuSans-BoldOblique.ttf",
            "C:\\Windows\\Fonts\\verdanaz.ttf",
        ],
    }
}

/// Fonts tried after the face's own, for characters it doesn't have: symbols, then
/// CJK. Separate lists, and loaded only when a character gets that far, because
/// a CJK font takes a long time to parse.
const SYMBOL_FALLBACK: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "C:\\Windows\\Fonts\\seguisym.ttf",
    "C:\\Windows\\Fonts\\arial.ttf",
];
const CJK_FALLBACK: &[&str] = &[
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
const CJK_BOLD_FALLBACK: &[&str] = &[
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Bold.ttc",
    "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
    "/usr/share/fonts/truetype/fonts-japanese-gothic.ttf",
    "/usr/share/fonts/truetype/vlgothic/VL-Gothic-Regular.ttf",
    "/usr/share/fonts/truetype/takao-gothic/TakaoGothic.ttf",
    "/usr/share/fonts/truetype/ipa/ipagp.ttf",
    "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
];

/// Parsed fonts by the candidate list they came from.
fn loaded_font(list: &'static [&'static str]) -> Option<Arc<Font>> {
    static LOADED: OnceLock<Mutex<HashMap<usize, Option<Arc<Font>>>>> = OnceLock::new();
    let cache = LOADED.get_or_init(|| Mutex::new(HashMap::new()));
    let key = list.as_ptr() as usize;
    if let Some(found) = cache.lock().unwrap().get(&key) {
        return found.clone();
    }
    let font = try_load_font(list).map(Arc::new);
    cache.lock().unwrap().insert(key, font.clone());
    font
}

/// A font to draw a character in. `upright` says it has no slant of its own, so
/// italic text has to shear it.
struct Chosen {
    font: Arc<Font>,
    upright: bool,
}

/// The first font in `face`'s chain with a glyph for `ch`.
fn font_for(ch: char, face: TextFace) -> Chosen {
    let own: &'static [&'static str] = candidates(face.class, face.bold, face.italic);
    let upright_own: &'static [&'static str] = candidates(face.class, face.bold, false);
    let cjk = if face.bold { CJK_BOLD_FALLBACK } else { CJK_FALLBACK };
    let chain: [(&'static [&'static str], bool); 4] = [
        (own, false),
        (upright_own, true),
        (SYMBOL_FALLBACK, true),
        (cjk, true),
    ];
    let mut first = None;
    for (list, upright) in chain {
        let Some(font) = loaded_font(list) else { continue };
        if first.is_none() {
            first = Some(Chosen { font: font.clone(), upright });
        }
        if font.lookup_glyph_index(ch) != 0 || ch == '\0' {
            return Chosen { font, upright };
        }
    }
    if let Some(chosen) = first {
        return chosen;
    }
    // Nothing on the usual paths: try any font in the system font directories.
    static ANY: OnceLock<Option<Arc<Font>>> = OnceLock::new();
    let any = ANY.get_or_init(|| {
        ["/usr/share/fonts", "C:\\Windows\\Fonts", "/System/Library/Fonts"]
            .iter()
            .find_map(|dir| find_any_ttf_in_dir(dir))
            .map(Arc::new)
    });
    match any {
        Some(font) => Chosen { font: font.clone(), upright: true },
        None => panic!("No system font found. Please install DejaVu Sans or any TTF font."),
    }
}

/// A face without its spacing, which doesn't affect which glyphs are drawn.
fn glyph_face(face: TextFace) -> (FontClass, bool, bool) {
    (face.class, face.bold, face.italic)
}

/// Layout-side metrics from the same fonts the painter draws with.
pub struct FontdueMetricsProvider {
    advances: Mutex<HashMap<(char, i64, (FontClass, bool, bool)), f64>>,
    extents: Mutex<HashMap<(i64, (FontClass, bool, bool)), (f64, f64, i64)>>,
}

impl std::fmt::Debug for FontdueMetricsProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FontdueMetricsProvider").finish()
    }
}

impl FontdueMetricsProvider {
    pub fn new() -> Self {
        Self {
            advances: Mutex::new(HashMap::new()),
            extents: Mutex::new(HashMap::new()),
        }
    }

    /// (ascent, descent, normal line-height) for a size and face.
    fn extent(&self, font_size: FontSize, face: TextFace) -> (f64, f64, i64) {
        let px = font_size.px();
        let key = (px, glyph_face(face));
        if let Some(v) = self.extents.lock().unwrap().get(&key) {
            return *v;
        }
        let font = font_for('x', face).font;
        let v = match font.horizontal_line_metrics(px as f32) {
            Some(m) => (
                m.ascent as f64,
                -m.descent as f64,
                (m.new_line_size.ceil() as i64).max(1),
            ),
            None => (px as f64, px as f64 * 0.25, (px as f64 * 1.25) as i64),
        };
        self.extents.lock().unwrap().insert(key, v);
        v
    }
}

impl FontMetricsProvider for FontdueMetricsProvider {
    fn advance(&self, c: char, font_size: FontSize, face: TextFace) -> f64 {
        let px = font_size.px();
        let key = (c, px, glyph_face(face));
        if let Some(v) = self.advances.lock().unwrap().get(&key) {
            return *v;
        }
        let v = font_for(c, face).font.metrics(c, px as f32).advance_width as f64;
        self.advances.lock().unwrap().insert(key, v);
        v
    }

    fn ascent_descent(&self, font_size: FontSize, face: TextFace) -> (f64, f64) {
        let (ascent, descent, _) = self.extent(font_size, face);
        (ascent, descent)
    }

    fn line_height(&self, font_size: FontSize, face: TextFace) -> i64 {
        self.extent(font_size, face).2
    }
}

pub struct TextRenderer {
    glyph_cache: HashMap<(char, u32, (FontClass, bool, bool)), GlyphBitmap>,
}

struct GlyphBitmap {
    width: u32,
    height: u32,
    advance_width: f32,
    bitmap: Vec<u8>,
    offset_x: i32,
    offset_y: i32,
    /// Slant it when drawing: the font has no italic of its own.
    shear: bool,
}

impl TextRenderer {
    pub fn new() -> Self {
        Self {
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
        self.draw_text_clipped(
            pixmap,
            text,
            x,
            y,
            font_px,
            r,
            g,
            b,
            alpha,
            scroll_y,
            TextFace::plain(bold),
            None,
        )
    }

    /// Like `draw_text`, in a given face, with glyph pixels outside `clip`
    /// (screen-space x, y, w, h) skipped: overflow containers clip their text.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_text_clipped(
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
        face: TextFace,
        clip: Option<(i64, i64, i64, i64)>,
    ) -> i64 {
        let baseline_y = y - scroll_y;
        // The pen keeps its fraction, as layout's measurements do, and each
        // glyph lands on the nearest pixel. Truncating every advance instead
        // would paint a run narrower than the width layout gave it.
        let mut pen = x as f64;

        for ch in text.chars() {
            let glyph = self.rasterize_glyph(ch, font_px, face);
            let gx = pen.round() as i64 + glyph.offset_x as i64;
            let gy = baseline_y - glyph.offset_y as i64;

            for row in 0..glyph.height as i64 {
                // An upright font's italic: one pixel across for every five up.
                let slant = if glyph.shear {
                    (baseline_y - (gy + row)) / 5
                } else {
                    0
                };
                for col in 0..glyph.width as i64 {
                    let px = gx + col + slant;
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

            pen += glyph.advance_width as f64 + face.extra_advance(ch);
        }
        pen.round() as i64
    }

    pub fn measure_text(&mut self, text: &str, font_px: u32) -> i64 {
        let mut width: f32 = 0.0;
        for ch in text.chars() {
            let glyph = self.rasterize_glyph(ch, font_px, TextFace::plain(false));
            width += glyph.advance_width;
        }
        width as i64
    }

    fn rasterize_glyph(&mut self, ch: char, font_px: u32, face: TextFace) -> &GlyphBitmap {
        let key = (ch, font_px, glyph_face(face));
        self.glyph_cache.entry(key).or_insert_with(|| {
            let chosen = font_for(ch, face);
            let (metrics, bitmap) = chosen.font.rasterize(ch, font_px as f32);
            GlyphBitmap {
                width: metrics.width as u32,
                height: metrics.height as u32,
                advance_width: metrics.advance_width,
                bitmap,
                offset_x: metrics.xmin,
                offset_y: metrics.ymin + metrics.height as i32,
                shear: face.italic && chosen.upright,
            }
        })
    }
}

pub(crate) fn blend_pixel(pixmap: &mut Pixmap, x: u32, y: u32, r: u8, g: u8, b: u8, a: u8) {
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
