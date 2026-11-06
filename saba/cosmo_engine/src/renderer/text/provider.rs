//! Font metrics provider boundary (plan 0.6).
//!
//! The engine owns line breaking and line-box construction but delegates
//! per-character advance widths to a `FontMetricsProvider`. The platform
//! layer (renderer_native) installs a provider backed by the real fonts it
//! draws with; when nothing is installed (unit tests, plain engine use) the
//! `FixedMetricsProvider` reproduces the historical hand-tuned advance
//! tables exactly, so existing layout expectations are unchanged.

use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::text::face::TextFace;
use crate::renderer::text::legacy_metrics::{
    bold_width_adjust, char_advance_16, char_width_px, line_height_px, scale_advance,
};
use std::fmt::Debug;
use std::sync::OnceLock;

pub trait FontMetricsProvider: Debug + Send + Sync {
    /// Advance width in px of `c` at the given font size/weight, unrounded.
    /// Runs add these up and round once at the end: rounding per character
    /// makes layout reserve more than the painter draws, a pixel or so per
    /// glyph, which shows up as a gap wherever an inline element follows text.
    fn advance(&self, c: char, font_size: FontSize, face: TextFace) -> f64;

    /// Distance from the baseline up to the top of the font's glyph area, and
    /// down to its bottom. Half the difference between their sum and the
    /// line-height goes above the text, half below (CSS 2.2 §10.8.1).
    fn ascent_descent(&self, font_size: FontSize, _face: TextFace) -> (f64, f64) {
        let px = font_size.px() as f64;
        (px, (line_height_px(font_size) as f64 - px).max(0.0))
    }

    /// Default line box height (glyph height + leading) for the font size.
    fn line_height(&self, font_size: FontSize, _face: TextFace) -> i64 {
        line_height_px(font_size)
    }
}

/// Deterministic metrics matching the legacy per-character-class tables.
#[derive(Debug)]
pub struct FixedMetricsProvider;

impl FontMetricsProvider for FixedMetricsProvider {
    fn advance(&self, c: char, font_size: FontSize, face: TextFace) -> f64 {
        scale_advance(
            char_advance_16(c),
            bold_width_adjust(char_width_px(font_size), face.bold),
        ) as f64
    }
}

static PROVIDER: OnceLock<Box<dyn FontMetricsProvider>> = OnceLock::new();
static FIXED: FixedMetricsProvider = FixedMetricsProvider;

/// Install the process-wide metrics provider. Returns false if one was
/// already installed (the first installation wins). Must be called before
/// the first layout to affect all measurements.
pub fn set_font_metrics_provider(provider: Box<dyn FontMetricsProvider>) -> bool {
    PROVIDER.set(provider).is_ok()
}

pub(crate) fn metrics() -> &'static dyn FontMetricsProvider {
    match PROVIDER.get() {
        Some(p) => p.as_ref(),
        None => &FIXED,
    }
}
