//! What a run of text is drawn in, apart from its size.

/// The kinds of font a page's `font-family` list can land on. The renderer
/// picks an installed font for each, close in metrics to what pages name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum FontClass {
    /// Times-like; what a page gets if it names nothing.
    #[default]
    Serif,
    /// Arial and Helvetica-like.
    Sans,
    /// Verdana and Tahoma-like, which are wider than `Sans`.
    WideSans,
    Mono,
}

impl FontClass {
    /// The generic family name carried in paint commands.
    pub fn name(self) -> &'static str {
        match self {
            FontClass::Serif => "serif",
            FontClass::Sans => "sans-serif",
            FontClass::WideSans => "wide-sans-serif",
            FontClass::Mono => "monospace",
        }
    }

    pub fn from_name(name: &str) -> Self {
        match name {
            "sans-serif" => FontClass::Sans,
            "wide-sans-serif" => FontClass::WideSans,
            "monospace" => FontClass::Mono,
            _ => FontClass::Serif,
        }
    }

    /// The class of one `font-family` entry, if it is one this engine can match.
    /// Web fonts fall through to the next entry in the list.
    pub fn from_family(family: &str) -> Option<Self> {
        let family = family.trim().trim_matches(|c| c == '"' || c == '\'').to_ascii_lowercase();
        Some(match family.as_str() {
            "serif" | "ui-serif" | "times" | "times new roman" | "georgia" | "cambria"
            | "palatino" | "palatino linotype" | "book antiqua" | "garamond" | "noto serif"
            | "liberation serif" | "dejavu serif" | "linux libertine" | "baskerville"
            | "charter" | "merriweather" | "source serif pro" | "ms serif" => FontClass::Serif,
            "sans-serif" | "ui-sans-serif" | "system-ui" | "-apple-system"
            | "blinkmacsystemfont" | "arial" | "helvetica" | "helvetica neue" | "segoe ui"
            | "roboto" | "noto sans" | "open sans" | "inter" | "lato" | "ubuntu" | "cantarell"
            | "liberation sans" | "arimo" | "oxygen" | "fira sans" | "source sans pro"
            | "montserrat" | "nunito" | "poppins" | "calibri" | "sans" => FontClass::Sans,
            "verdana" | "tahoma" | "geneva" | "dejavu sans" | "bitstream vera sans"
            | "trebuchet ms" | "lucida grande" | "lucida sans unicode" => FontClass::WideSans,
            "monospace" | "ui-monospace" | "mono" | "courier" | "courier new" | "consolas"
            | "menlo" | "monaco" | "sfmono-regular" | "sf mono" | "liberation mono"
            | "dejavu sans mono" | "source code pro" | "fira code" | "fira mono"
            | "roboto mono" | "lucida console" | "ubuntu mono" | "jetbrains mono"
            | "cascadia code" | "andale mono" => FontClass::Mono,
            _ => return None,
        })
    }
}

/// How glyphs in a run are picked and spaced. Spacing is kept in 64ths of a pixel
/// so this stays hashable, for the caches keyed on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct TextFace {
    pub class: FontClass,
    pub bold: bool,
    pub italic: bool,
    pub letter_spacing_64: i16,
    pub word_spacing_64: i16,
}

impl TextFace {
    /// A plain sans-serif, or bold.
    pub fn plain(bold: bool) -> Self {
        Self {
            class: FontClass::Sans,
            bold,
            ..Self::default()
        }
    }

    pub fn with_spacing(mut self, letter: f64, word: f64) -> Self {
        self.letter_spacing_64 = (letter * 64.0) as i16;
        self.word_spacing_64 = (word * 64.0) as i16;
        self
    }

    pub fn letter_spacing(&self) -> f64 {
        self.letter_spacing_64 as f64 / 64.0
    }

    pub fn word_spacing(&self) -> f64 {
        self.word_spacing_64 as f64 / 64.0
    }

    /// What to add to `c`'s advance for `letter-spacing` and `word-spacing`.
    pub fn extra_advance(&self, c: char) -> f64 {
        let word = if c == ' ' || c == '\u{a0}' { self.word_spacing() } else { 0.0 };
        self.letter_spacing() + word
    }
}
