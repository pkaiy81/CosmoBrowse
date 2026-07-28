//! Legacy text metrics: hand-tuned per-character advance tables and the
//! greedy line splitter, extracted verbatim from layout_object.rs (plan 0.5).
//! Replaced by a real FontMetricsProvider in plan 0.6.

use crate::constants::CHAR_HEIGHT_WITH_PADDING;
use crate::constants::CHAR_WIDTH;
use crate::renderer::layout::computed_style::ComputedStyle;
use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::layout::computed_style::LineHeight;

pub(crate) fn font_ratio(font_size: FontSize) -> i64 {
    match font_size {
        FontSize::Medium => 1,
        FontSize::XLarge => 2,
        FontSize::XXLarge => 3,
        // Arbitrary px sizes don't use the legacy integer ratio; callers that
        // need text metrics use char_width_px / line_height_px below.
        FontSize::Px(_) => 1,
    }
}

/// Estimated advance width of one narrow character at the given font size.
/// Legacy named buckets keep their historical integer-ratio metrics so
/// existing layouts are unchanged; arbitrary `Px` sizes scale linearly from
/// the 16px default (CHAR_WIDTH is the advance at 16px).
pub(crate) fn char_width_px(font_size: FontSize) -> i64 {
    match font_size {
        // Round UP: underestimating the advance makes the next inline box
        // overlap the tail of this text when the real font draws wider than
        // the estimate (e.g. Verdana 13px averages ~7px, not 8*13/16 = 6.5).
        FontSize::Px(n) => {
            let base = FontSize::Medium.px();
            ((CHAR_WIDTH * n + base - 1) / base).max(1)
        }
        legacy => CHAR_WIDTH * font_ratio(legacy),
    }
}

/// Line height (glyph height + leading) at the given font size. Mirrors
/// `char_width_px`: legacy buckets keep ratio metrics, `Px` scales linearly.
pub(crate) fn line_height_px(font_size: FontSize) -> i64 {
    match font_size {
        FontSize::Px(n) => (CHAR_HEIGHT_WITH_PADDING * n / FontSize::Medium.px()).max(1),
        legacy => CHAR_HEIGHT_WITH_PADDING * font_ratio(legacy),
    }
}

/// Line box height honoring an explicit `line-height`; falls back to the
/// installed metrics provider's default leading for the font size.
pub(crate) fn styled_line_height(style: &ComputedStyle) -> i64 {
    match style.line_height() {
        Some(LineHeight::Px(px)) => (px as i64).max(1),
        Some(LineHeight::Factor(f)) => {
            ((style.font_size_or_default().px() as f64 * f) as i64).max(1)
        }
        None => super::provider::metrics().line_height(style.font_size_or_default()),
    }
}

/// Advance width of one character from the installed metrics provider.
pub(crate) fn char_advance(c: char, font_size: FontSize, bold: bool) -> i64 {
    super::provider::metrics().char_advance(c, font_size, bold)
}

pub(crate) fn is_wide_char(c: char) -> bool {
    let cp = c as u32;
    // CJK Unified Ideographs, Hiragana, Katakana, Fullwidth forms, CJK symbols
    (0x3000..=0x9FFF).contains(&cp)
        || (0xF900..=0xFAFF).contains(&cp)
        || (0xFF01..=0xFF60).contains(&cp)
        || (0xFFE0..=0xFFE6).contains(&cp)
        || (0x20000..=0x2FA1F).contains(&cp)
        || (0xAC00..=0xD7AF).contains(&cp) // Hangul
}

/// Estimated advance of one character in pixels at the 16px base size,
/// approximating the DejaVu Sans metrics the native renderer draws with
/// (narrow i/l ≈ 4.5, average lowercase ≈ 9.6, m ≈ 15.6). A character-class
/// table beats the old uniform 8px: the flat value underestimated lowercase
/// runs (next inline box overlapped the drawn text) and overestimated
/// punctuation-heavy ones.
pub(crate) fn char_advance_16(c: char) -> i64 {
    if is_wide_char(c) {
        return 16;
    }
    match c {
        'i' | 'j' | 'l' | '\'' | '.' | ',' | ':' | ';' | '!' | '|' => 5,
        'f' | 'I' | ' ' | '(' | ')' | '[' | ']' | '"' | '`' | '*' => 6,
        't' | 'r' | '-' | '/' | '\\' => 7,
        's' | 'J' => 8,
        'm' => 16,
        'M' | 'W' | '@' | '%' => 15,
        'w' | 'O' | 'Q' | 'G' | 'H' | 'N' | 'U' | 'D' | '+' | '=' | '<' | '>' | '~' => 12,
        'A'..='Z' | '0'..='9' | '&' | '#' | '$' | '?' | '_' => 11,
        _ => 10,
    }
}

/// Scale a 16px-base advance to the effective per-character width `cw`
/// (which already carries the font-size scaling and any bold adjustment;
/// `cw == CHAR_WIDTH` means scale 1). Rounds up so layout never reserves
/// less than the renderer draws.
pub(crate) fn scale_advance(advance_16: i64, cw: i64) -> i64 {
    (advance_16 * cw + CHAR_WIDTH - 1) / CHAR_WIDTH
}

/// Truncate `text` so it plus a trailing `…` fits within `max_width` px.
/// Returns the original text when it already fits.
pub(crate) fn truncate_with_ellipsis(
    text: &str,
    font_size: FontSize,
    bold: bool,
    max_width: i64,
) -> String {
    if measure_text_width(text, font_size, bold) <= max_width {
        return text.to_string();
    }
    let ellipsis_w = char_advance('…', font_size, bold);
    let budget = (max_width - ellipsis_w).max(0);
    let mut acc = 0i64;
    let mut out = String::new();
    for c in text.chars() {
        let w = char_advance(c, font_size, bold);
        if acc + w > budget {
            break;
        }
        acc += w;
        out.push(c);
    }
    out.push('…');
    out
}

/// Bold faces draw roughly 10% wider than the regular face at the same size.
/// Round up so a following inline box never overlaps the bold run's tail.
pub(crate) fn bold_width_adjust(width: i64, bold: bool) -> i64 {
    if bold {
        width + (width + 7) / 8
    } else {
        width
    }
}

/// Width of `text`, accumulating PER-CHARACTER rounded advances — the exact
/// accounting `split_text` uses, so a box sized from this never wraps its own
/// content (a one-shot total scale rounds lower and "login" wrapped as
/// "logi/n").
pub(crate) fn measure_text_width(text: &str, font_size: FontSize, bold: bool) -> i64 {
    text.chars()
        .map(|c| char_advance(c, font_size, bold))
        .sum()
}
pub(crate) fn is_break_space(c: char) -> bool {
    c == ' ' || c == '\u{3000}'
}

/// Greedily wrap `line` into visual lines no wider than `max_width`.
///
/// This is a single forward pass: O(n) in the text length. The previous
/// implementation recursed on the un-consumed remainder and, on every line,
/// re-measured the whole remainder and re-collected all of its char indices —
/// O(n²). On real pages that carry a very long unbroken run of text (e.g. a
/// page's minified inline script that ends up in the inline flow) the quadratic
/// cost stalled layout for many seconds and effectively hung the renderer.
///
/// Breaking prefers the last space that still fits on the line (the space is
/// consumed, not rendered); if a single run has no space it is hard-broken at
/// the character that would overflow. Wide (CJK) characters count as two units,
/// matching [`char_advance_16`].
pub(crate) fn split_text(
    line: String,
    font_size: FontSize,
    bold: bool,
    max_width: i64,
) -> Vec<String> {
    let safe_width = max_width
        .max(bold_width_adjust(char_width_px(font_size), bold))
        .max(1);
    // Line capacity in pixels; per-character advances come from the same
    // provider as measurement so wrapping and sizing agree.
    let max_units = safe_width;

    let mut result: Vec<String> = vec![];
    let mut line_start = 0usize; // byte offset where the current visual line starts
    let mut cur_units = 0i64; // width units accumulated on the current line
    let mut started = false; // whether the current line has consumed any char
    // Last space seen on the current line: (its byte offset, byte offset just
    // after it, units accumulated since it). Used to break at word boundaries.
    let mut last_space: Option<usize> = None;
    let mut byte_after_space = 0usize;
    let mut units_after_space = 0i64;

    for (idx, c) in line.char_indices() {
        let w = char_advance(c, font_size, bold);

        if started && cur_units + w > max_units {
            match last_space {
                // Break at the last space: it is dropped and the next line
                // starts after it, carrying the text seen since the space.
                Some(space_byte) => {
                    result.push(line[line_start..space_byte].to_string());
                    line_start = byte_after_space;
                    cur_units = units_after_space;
                    last_space = None;
                }
                // No space to break on: hard-break before the overflowing char.
                None => {
                    result.push(line[line_start..idx].to_string());
                    line_start = idx;
                    cur_units = 0;
                }
            }
        }

        cur_units += w;
        started = true;
        if is_break_space(c) {
            last_space = Some(idx);
            byte_after_space = idx + c.len_utf8();
            units_after_space = 0;
        } else {
            units_after_space += w;
        }
    }

    if line_start < line.len() {
        result.push(line[line_start..].to_string());
    }
    result
}
