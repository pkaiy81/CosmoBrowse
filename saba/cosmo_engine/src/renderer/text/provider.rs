//! Font metrics provider boundary (plan 0.6).
//!
//! The engine owns line breaking and line-box construction but delegates
//! per-character advance widths to a `FontMetricsProvider`. The platform
//! layer (renderer_native) installs a provider backed by the real fonts it
//! draws with; when nothing is installed (unit tests, plain engine use) the
//! `FixedMetricsProvider` reproduces the historical hand-tuned advance
//! tables exactly, so existing layout expectations are unchanged.

use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::text::legacy_metrics::{
    bold_width_adjust, char_advance_16, char_width_px, line_height_px, scale_advance,
};
use std::fmt::Debug;
use std::sync::OnceLock;

pub trait FontMetricsProvider: Debug + Send + Sync {
    /// Advance width in px of `c` at the given font size/weight, rounded UP —
    /// layout must never reserve less than the renderer draws, or a box wraps
    /// its own content.
    fn char_advance(&self, c: char, font_size: FontSize, bold: bool) -> i64;

    /// Default line box height (glyph height + leading) for the font size.
    fn line_height(&self, font_size: FontSize) -> i64 {
        line_height_px(font_size)
    }
}

/// Deterministic metrics matching the legacy per-character-class tables.
#[derive(Debug)]
pub struct FixedMetricsProvider;

impl FontMetricsProvider for FixedMetricsProvider {
    fn char_advance(&self, c: char, font_size: FontSize, bold: bool) -> i64 {
        scale_advance(
            char_advance_16(c),
            bold_width_adjust(char_width_px(font_size), bold),
        )
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
