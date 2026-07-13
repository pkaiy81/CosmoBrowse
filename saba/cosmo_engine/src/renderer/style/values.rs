//! CSS declaration-value parsing helpers, extracted verbatim from
//! layout_object.rs (plan 0.5). These parse ComponentValues / HTML attribute
//! strings into typed values; they do no layout.

use crate::renderer::css::cssom::ComponentValue;
use crate::renderer::layout::computed_style::Color;
use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::layout::computed_style::GridTrack;

use std::cell::Cell;

thread_local! {
    /// Viewport (width, height) for resolving vw/vh units during styling.
    /// Set by LayoutView before the style/layout pass; the engine is
    /// single-threaded per page so a thread-local is sufficient (same
    /// transitional pattern as the font-metrics provider).
    static STYLING_VIEWPORT: Cell<(i64, i64)> = const { Cell::new((0, 0)) };
}

pub(crate) fn set_styling_viewport(width: i64, height: i64) {
    STYLING_VIEWPORT.with(|v| v.set((width, height)));
}

pub(crate) fn length_to_px(value: f64, unit: &str, base_font_size: FontSize) -> Option<f64> {
    match unit {
        "px" => Some(value),
        "em" => Some(value * base_font_size.px() as f64),
        "rem" => Some(value * FontSize::Medium.px() as f64),
        // Viewport-relative units resolve against the viewport captured at
        // the start of the pass; unknown (0) viewport skips the declaration.
        "vw" | "vh" | "vmin" | "vmax" => {
            let (vw, vh) = STYLING_VIEWPORT.with(|v| v.get());
            let base = match unit {
                "vw" => vw,
                "vh" => vh,
                "vmin" => vw.min(vh),
                _ => vw.max(vh),
            };
            if base > 0 {
                Some(value * base as f64 / 100.0)
            } else {
                None
            }
        }
        // Absolute lengths, CSS Values & Units §6.2: 1in = 96px = 72pt = 6pc;
        // 1in = 2.54cm = 25.4mm. https://www.w3.org/TR/css-values-4/#absolute-lengths
        "pt" => Some(value * 96.0 / 72.0),
        "pc" => Some(value * 16.0),
        "in" => Some(value * 96.0),
        "cm" => Some(value * 96.0 / 2.54),
        "mm" => Some(value * 96.0 / 25.4),
        _ => None,
    }
}

/// Parse the column tracks declared by a `grid-template-columns` value.
/// Lengths become fixed tracks, `Nfr` flexible tracks, keywords/functions
/// `Auto` (≈1fr); `repeat(N, tracks)` expands to N copies of its track list.
/// Returns an empty Vec when nothing can be recognized.
pub(crate) fn parse_grid_template_tracks(values: &[ComponentValue]) -> Vec<GridTrack> {
    let mut tracks: Vec<GridTrack> = Vec::new();
    let mut i = 0;
    while i < values.len() {
        match &values[i] {
            ComponentValue::Ident(name) if name.eq_ignore_ascii_case("repeat") => {
                // repeat(N, tracks) — recurse on the inner track list.
                if matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis)) {
                    let mut depth = 1;
                    let mut j = i + 2;
                    while j < values.len() && depth > 0 {
                        match &values[j] {
                            ComponentValue::OpenParenthesis => depth += 1,
                            ComponentValue::CloseParenthesis => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    let inner = &values[(i + 2).min(values.len())..(j - 1).min(values.len())];
                    let (n, rest) = match inner.first() {
                        Some(ComponentValue::Number(v)) => (
                            (*v as usize).max(1),
                            inner
                                .iter()
                                .position(|t| *t == ComponentValue::Delim(','))
                                .map(|p| &inner[p + 1..])
                                .unwrap_or(&[]),
                        ),
                        // auto-fill / auto-fit have no fixed count: one copy.
                        _ => (
                            1,
                            inner
                                .iter()
                                .position(|t| *t == ComponentValue::Delim(','))
                                .map(|p| &inner[p + 1..])
                                .unwrap_or(&[]),
                        ),
                    };
                    let unit = parse_grid_template_tracks(rest);
                    for _ in 0..n {
                        tracks.extend(unit.iter().copied());
                    }
                    i = j;
                    continue;
                }
                tracks.push(GridTrack::Auto);
            }
            ComponentValue::Dimension(v, unit) => {
                if unit == "fr" {
                    tracks.push(GridTrack::Fr((*v).max(0.0)));
                } else if unit == "%" {
                    // Percentage of the container ≈ flexible share.
                    tracks.push(GridTrack::Fr((*v / 100.0).max(0.0)));
                } else if let Some(px) = length_to_px(*v, unit, FontSize::Medium) {
                    tracks.push(GridTrack::Px(px.max(0.0)));
                } else {
                    tracks.push(GridTrack::Auto);
                }
            }
            ComponentValue::Number(v) => {
                tracks.push(GridTrack::Px((*v).max(0.0)));
            }
            ComponentValue::Ident(name) if name.eq_ignore_ascii_case("minmax") => {
                // minmax(min, max): size by the max part (a fr max flexes,
                // a px max is fixed). The min clamp is approximated away.
                if matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis)) {
                    let mut depth = 1;
                    let mut j = i + 2;
                    while j < values.len() && depth > 0 {
                        match &values[j] {
                            ComponentValue::OpenParenthesis => depth += 1,
                            ComponentValue::CloseParenthesis => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    let inner = &values[(i + 2).min(values.len())..(j - 1).min(values.len())];
                    let parts = parse_grid_template_tracks(
                        &inner
                            .iter()
                            .filter(|t| **t != ComponentValue::Delim(','))
                            .cloned()
                            .collect::<Vec<_>>(),
                    );
                    tracks.push(parts.last().copied().unwrap_or(GridTrack::Auto));
                    i = j;
                    continue;
                }
                tracks.push(GridTrack::Auto);
            }
            ComponentValue::Ident(_) => {
                tracks.push(GridTrack::Auto);
                // Skip a function's argument list so its contents don't count
                // as extra tracks.
                if matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis)) {
                    let mut depth = 1;
                    let mut j = i + 2;
                    while j < values.len() && depth > 0 {
                        match &values[j] {
                            ComponentValue::OpenParenthesis => depth += 1,
                            ComponentValue::CloseParenthesis => depth -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    i = j;
                    continue;
                }
            }
            _ => {}
        }
        i += 1;
    }
    tracks
}

/// Resolve column tracks to pixel widths for the given content width:
/// gaps and fixed tracks are reserved first, then the remainder is split
/// among Fr/Auto tracks in proportion to their flex factors (Auto = 1fr).
/// https://www.w3.org/TR/css-grid-1/#algo-track-sizing
pub(crate) fn resolve_grid_tracks(tracks: &[GridTrack], available: i64, column_gap: i64) -> Vec<i64> {
    let n = tracks.len().max(1) as i64;
    let gaps = column_gap * (n - 1).max(0);
    let mut remaining = (available - gaps).max(0);
    let mut total_fr = 0.0f64;
    for t in tracks {
        match t {
            GridTrack::Px(px) => remaining -= *px as i64,
            GridTrack::Fr(f) => total_fr += f.max(0.0),
            GridTrack::Auto => total_fr += 1.0,
        }
    }
    let remaining = remaining.max(0);
    tracks
        .iter()
        .map(|t| match t {
            GridTrack::Px(px) => *px as i64,
            GridTrack::Fr(f) => {
                if total_fr > 0.0 {
                    (remaining as f64 * f.max(0.0) / total_fr) as i64
                } else {
                    0
                }
            }
            GridTrack::Auto => {
                if total_fr > 0.0 {
                    (remaining as f64 / total_fr) as i64
                } else {
                    0
                }
            }
        })
        .collect()
}

/// Parse a `transform` value list into (tx, tx_pct, ty, ty_pct, scale).
/// Supports translate/translateX/translateY (px and %) and
/// scale/scaleX/scaleY (uniform: the x factor wins); other functions are
/// ignored (they still set the stacking trigger via has_transform).
/// https://www.w3.org/TR/css-transforms-1/
pub(crate) fn parse_transform_ops(values: &[ComponentValue]) -> Option<(f64, bool, f64, bool, f64)> {
    let mut tx = 0.0f64;
    let mut tx_pct = false;
    let mut ty = 0.0f64;
    let mut ty_pct = false;
    let mut scale = 1.0f64;
    let mut found = false;
    let mut i = 0;
    while i < values.len() {
        let (name, has_args) = match &values[i] {
            ComponentValue::Ident(n)
                if matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis)) =>
            {
                (n.to_lowercase(), true)
            }
            _ => {
                i += 1;
                continue;
            }
        };
        if !has_args {
            i += 1;
            continue;
        }
        // Collect argument components up to the matching close paren.
        let mut args: Vec<(f64, bool)> = Vec::new();
        let mut depth = 1;
        let mut j = i + 2;
        while j < values.len() && depth > 0 {
            match &values[j] {
                ComponentValue::OpenParenthesis => depth += 1,
                ComponentValue::CloseParenthesis => depth -= 1,
                ComponentValue::Number(v) => args.push((*v, false)),
                ComponentValue::Dimension(v, unit) if unit == "%" => args.push((*v, true)),
                ComponentValue::Dimension(v, unit) => {
                    if let Some(px) = length_to_px(*v, unit, FontSize::Medium) {
                        args.push((px, false));
                    }
                }
                _ => {}
            }
            j += 1;
        }
        match name.as_str() {
            "translate" => {
                if let Some((v, p)) = args.first() {
                    tx = *v;
                    tx_pct = *p;
                    found = true;
                }
                if let Some((v, p)) = args.get(1) {
                    ty = *v;
                    ty_pct = *p;
                }
            }
            "translatex" => {
                if let Some((v, p)) = args.first() {
                    tx = *v;
                    tx_pct = *p;
                    found = true;
                }
            }
            "translatey" => {
                if let Some((v, p)) = args.first() {
                    ty = *v;
                    ty_pct = *p;
                    found = true;
                }
            }
            "scale" | "scalex" | "scaley" => {
                if let Some((v, _)) = args.first() {
                    if *v > 0.0 {
                        scale = *v;
                        found = true;
                    }
                }
            }
            _ => {}
        }
        i = j;
    }
    if found {
        Some((tx, tx_pct, ty, ty_pct, scale))
    } else {
        None
    }
}

/// Parse the `rotate(<angle>)` function from a `transform` value list, in
/// degrees (clockwise). Supports deg/rad/turn/grad units.
pub(crate) fn parse_transform_rotate(values: &[ComponentValue]) -> Option<f64> {
    let mut i = 0;
    while i < values.len() {
        let is_rotate = matches!(&values[i],
            ComponentValue::Ident(n) if n.eq_ignore_ascii_case("rotate"))
            && matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis));
        if is_rotate {
            // First numeric/dimension argument is the angle.
            let mut j = i + 2;
            while j < values.len() {
                match &values[j] {
                    ComponentValue::CloseParenthesis => break,
                    ComponentValue::Dimension(v, unit) => {
                        return Some(match unit.to_lowercase().as_str() {
                            "rad" => v * 180.0 / std::f64::consts::PI,
                            "turn" => v * 360.0,
                            "grad" => v * 0.9,
                            _ => *v,
                        });
                    }
                    ComponentValue::Number(v) => return Some(*v),
                    _ => {}
                }
                j += 1;
            }
        }
        i += 1;
    }
    None
}

/// One background-position component: (value, is_percent, axis) where axis is
/// Some(true) for horizontal keywords, Some(false) for vertical, None when the
/// component fits either axis. Keywords map to percentages per CSS Backgrounds
/// §3.6 (left/top = 0%, center = 50%, right/bottom = 100%).
pub(crate) fn bg_position_component(v: &ComponentValue) -> Option<(f64, bool, Option<bool>)> {
    match v {
        ComponentValue::Dimension(value, unit) if unit == "%" => Some((*value, true, None)),
        ComponentValue::Dimension(value, unit) => {
            length_to_px(*value, unit, FontSize::Medium).map(|px| (px, false, None))
        }
        ComponentValue::Number(value) => Some((*value, false, None)),
        ComponentValue::Ident(name) => match name.to_lowercase().as_str() {
            "left" => Some((0.0, true, Some(true))),
            "right" => Some((100.0, true, Some(true))),
            "top" => Some((0.0, true, Some(false))),
            "bottom" => Some((100.0, true, Some(false))),
            "center" => Some((50.0, true, None)),
            _ => None,
        },
        _ => None,
    }
}

/// Assemble up to two position components into (x, x_pct, y, y_pct), honoring
/// axis keywords in either order; a single component centers the other axis.
pub(crate) fn assemble_bg_position(
    comps: &[(f64, bool, Option<bool>)],
) -> Option<(f64, bool, f64, bool)> {
    match comps {
        [] => None,
        [(v, pct, axis)] => Some(if *axis == Some(false) {
            (50.0, true, *v, *pct)
        } else {
            (*v, *pct, 50.0, true)
        }),
        [first, second, ..] => {
            // "top left" order: a vertical keyword first (or horizontal
            // second) swaps the components.
            let swapped = first.2 == Some(false) || second.2 == Some(true);
            let (x, y) = if swapped { (second, first) } else { (first, second) };
            Some((x.0, x.1, y.0, y.1))
        }
    }
}

/// One background-size component: (value, is_percent) — a negative value
/// means `auto` for that axis. https://www.w3.org/TR/css-backgrounds-3/#the-background-size
pub(crate) fn bg_size_component(v: &ComponentValue) -> Option<(f64, bool)> {
    match v {
        ComponentValue::Dimension(value, unit) if unit == "%" => Some((*value, true)),
        ComponentValue::Dimension(value, unit) => {
            length_to_px(*value, unit, FontSize::Medium).map(|px| (px, false))
        }
        ComponentValue::Number(value) => Some((*value, false)),
        ComponentValue::Ident(name) if name.eq_ignore_ascii_case("auto") => Some((-1.0, false)),
        _ => None,
    }
}

/// Assemble up to two size components into the packed background-size tuple
/// (mode 0 = explicit; a single component leaves the height `auto`).
pub(crate) fn assemble_bg_size(comps: &[(f64, bool)]) -> Option<(u8, f64, bool, f64, bool)> {
    match comps {
        [] => None,
        [(w, wp)] => Some((0, *w, *wp, -1.0, false)),
        [(w, wp), (h, hp), ..] => Some((0, *w, *wp, *h, *hp)),
    }
}

/// Parse a `background-size` value list: `cover`, `contain`, or 1–2
/// length/percent/auto components.
pub(crate) fn parse_background_size(values: &[ComponentValue]) -> Option<(u8, f64, bool, f64, bool)> {
    if let Some(ComponentValue::Ident(name)) = values.first() {
        if name.eq_ignore_ascii_case("cover") {
            return Some((1, 0.0, false, 0.0, false));
        }
        if name.eq_ignore_ascii_case("contain") {
            return Some((2, 0.0, false, 0.0, false));
        }
    }
    let comps: Vec<(f64, bool)> = values.iter().filter_map(bg_size_component).take(2).collect();
    assemble_bg_size(&comps)
}

/// Scan a `background` shorthand for position components, a `/ size` segment,
/// and `no-repeat`, skipping parenthesized groups (url/gradient arguments).
#[allow(clippy::type_complexity)]
pub(crate) fn scan_background_shorthand(
    values: &[ComponentValue],
) -> (
    Option<(f64, bool, f64, bool)>,
    bool,
    Option<(u8, f64, bool, f64, bool)>,
) {
    let mut comps: Vec<(f64, bool, Option<bool>)> = Vec::new();
    let mut size_comps: Vec<(f64, bool)> = Vec::new();
    let mut size_keyword: Option<(u8, f64, bool, f64, bool)> = None;
    let mut no_repeat = false;
    let mut after_slash = false;
    let mut i = 0;
    while i < values.len() {
        match &values[i] {
            ComponentValue::OpenParenthesis => {
                let mut depth = 1;
                i += 1;
                while i < values.len() && depth > 0 {
                    match &values[i] {
                        ComponentValue::OpenParenthesis => depth += 1,
                        ComponentValue::CloseParenthesis => depth -= 1,
                        _ => {}
                    }
                    i += 1;
                }
                continue;
            }
            ComponentValue::Delim('/') => after_slash = true,
            ComponentValue::Delim(',') => after_slash = false,
            ComponentValue::Ident(s) if s.eq_ignore_ascii_case("no-repeat") => {
                no_repeat = true;
            }
            ComponentValue::Ident(s)
                if after_slash
                    && (s.eq_ignore_ascii_case("cover") || s.eq_ignore_ascii_case("contain")) =>
            {
                size_keyword = Some(if s.eq_ignore_ascii_case("cover") {
                    (1, 0.0, false, 0.0, false)
                } else {
                    (2, 0.0, false, 0.0, false)
                });
            }
            v => {
                if after_slash {
                    if size_comps.len() < 2 {
                        if let Some(c) = bg_size_component(v) {
                            size_comps.push(c);
                        }
                    }
                } else if comps.len() < 2 {
                    if let Some(c) = bg_position_component(v) {
                        comps.push(c);
                    }
                }
            }
        }
        i += 1;
    }
    let size = size_keyword.or_else(|| assemble_bg_size(&size_comps));
    (assemble_bg_position(&comps), no_repeat, size)
}

/// Extract the first `url(...)` from a declaration's component values.
/// Handles both the quoted form (`url("x.png")` — a StringToken between
/// parens) and the unquoted form (`url(x.png)` — reassembled from the ident/
/// delim/number tokens the tokenizer produced).
/// https://www.w3.org/TR/css-values-4/#urls
pub(crate) fn extract_css_url(values: &[ComponentValue]) -> Option<String> {
    let mut i = 0;
    while i < values.len() {
        let is_url_fn = matches!(&values[i], ComponentValue::Ident(s) if s.eq_ignore_ascii_case("url"))
            && matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis));
        if is_url_fn {
            let mut url = String::new();
            for v in &values[i + 2..] {
                match v {
                    ComponentValue::CloseParenthesis => {
                        let trimmed = url.trim();
                        if trimmed.is_empty() {
                            return None;
                        }
                        return Some(trimmed.to_string());
                    }
                    ComponentValue::StringToken(s) | ComponentValue::Ident(s) => url.push_str(s),
                    ComponentValue::Delim(c) => url.push(*c),
                    ComponentValue::Colon => url.push(':'),
                    ComponentValue::Number(n) => {
                        url.push_str(&format!("{}", n));
                    }
                    ComponentValue::Dimension(n, u) => {
                        url.push_str(&format!("{}{}", n, u));
                    }
                    _ => return None,
                }
            }
            return None;
        }
        i += 1;
    }
    None
}

pub(crate) fn first_font_family(value: &[ComponentValue]) -> Option<String> {
    value.iter().find_map(|component| match component {
        ComponentValue::Ident(name) | ComponentValue::StringToken(name) => Some(name.clone()),
        _ => None,
    })
}
pub(crate) fn spacing_component_to_px(component: &ComponentValue, base_font_size: FontSize) -> Option<f64> {
    match component {
        ComponentValue::Number(value) => Some(*value),
        ComponentValue::Dimension(value, unit) => length_to_px(*value, unit, base_font_size),
        _ => None,
    }
}

// Ref: CSS Box Model Level 4, margin and padding shorthands.
// https://drafts.csswg.org/css-box-4/#margin-shorthand
// https://drafts.csswg.org/css-box-4/#padding-shorthand
pub(crate) fn parse_spacing_shorthand(
    value: &[ComponentValue],
    base_font_size: FontSize,
) -> Option<(f64, f64, f64, f64)> {
    let components = value
        .iter()
        .filter_map(|component| spacing_component_to_px(component, base_font_size))
        .collect::<Vec<_>>();

    match components.as_slice() {
        [all] => Some((*all, *all, *all, *all)),
        [vertical, horizontal] => Some((*vertical, *horizontal, *vertical, *horizontal)),
        [top, horizontal, bottom] => Some((*top, *horizontal, *bottom, *horizontal)),
        [top, right, bottom, left] => Some((*top, *right, *bottom, *left)),
        _ => None,
    }
}


pub(crate) fn margin_component(component: &ComponentValue, base_font_size: FontSize) -> Option<Option<f64>> {
    match component {
        ComponentValue::Ident(name) if name == "auto" => Some(None),
        _ => spacing_component_to_px(component, base_font_size).map(Some),
    }
}

// Spec: CSS Box Model margin shorthand supports `auto` values, which are positional tokens
// and must not be dropped during 1/2/3/4-value expansion.
// https://drafts.csswg.org/css-box-4/#margin-shorthand
pub(crate) fn parse_margin_shorthand(
    value: &[ComponentValue],
    base_font_size: FontSize,
) -> Option<(Option<f64>, Option<f64>, Option<f64>, Option<f64>)> {
    let components = value
        .iter()
        .map(|component| margin_component(component, base_font_size))
        .collect::<Option<Vec<_>>>()?;

    match components.as_slice() {
        [all] => Some((*all, *all, *all, *all)),
        [vertical, horizontal] => Some((*vertical, *horizontal, *vertical, *horizontal)),
        [top, horizontal, bottom] => Some((*top, *horizontal, *bottom, *horizontal)),
        [top, right, bottom, left] => Some((*top, *right, *bottom, *left)),
        _ => None,
    }
}

pub(crate) fn parse_margin_auto_flags(value: &[ComponentValue]) -> (bool, bool) {
    let flags = value
        .iter()
        .map(|component| matches!(component, ComponentValue::Ident(name) if name == "auto"))
        .collect::<Vec<_>>();

    match flags.as_slice() {
        [all] => (*all, *all),
        [_, horizontal] => (*horizontal, *horizontal),
        [_, horizontal, _] => (*horizontal, *horizontal),
        [_, right, _, left] => (*left, *right),
        _ => (false, false),
    }
}

pub(crate) fn parse_dimension_attr(value: Option<String>) -> Option<i64> {
    let value = value?;
    // Percentage values (e.g. "100%") are not treated as fixed pixel widths.
    // Returning None lets the caller fall through to the available-width default,
    // which gives the correct "fill container" behaviour for width="100%".
    if value.trim_start().contains('%') {
        return None;
    }
    let digits = value
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect::<String>();
    if digits.is_empty() {
        None
    } else {
        digits.parse::<i64>().ok()
    }
}

/// Parse an HTML dimension attribute, resolving percentage values against `avail`.
/// `"22%"` with `avail = Some(700)` returns `Some(154)`.
/// Falls back to `parse_dimension_attr` for pixel values.
/// Returns `None` when the value is absent, unparseable, or `avail` is None for a percentage.
pub(crate) fn parse_dimension_pct_attr(value: Option<String>, avail: Option<i64>) -> Option<i64> {
    let value = value?;
    let trimmed = value.trim();
    if let Some(pct_str) = trimmed.strip_suffix('%') {
        let pct: f64 = pct_str.trim().parse().ok()?;
        let avail = avail?;
        Some(((avail as f64) * pct / 100.0) as i64)
    } else {
        parse_dimension_attr(Some(value))
    }
}

/// The full set of CSS named colors (Color Module Level 4 / X11 names) as
/// 6-digit hex codes, plus a `transparent` special case handled by callers.
pub(crate) fn named_color_code(name: &str) -> Option<&'static str> {
    Some(match name {
        "aliceblue" => "#f0f8ff", "antiquewhite" => "#faebd7", "aqua" => "#00ffff",
        "aquamarine" => "#7fffd4", "azure" => "#f0ffff", "beige" => "#f5f5dc",
        "bisque" => "#ffe4c4", "black" => "#000000", "blanchedalmond" => "#ffebcd",
        "blue" => "#0000ff", "blueviolet" => "#8a2be2", "brown" => "#a52a2a",
        "burlywood" => "#deb887", "cadetblue" => "#5f9ea0", "chartreuse" => "#7fff00",
        "chocolate" => "#d2691e", "coral" => "#ff7f50", "cornflowerblue" => "#6495ed",
        "cornsilk" => "#fff8dc", "crimson" => "#dc143c", "cyan" => "#00ffff",
        "darkblue" => "#00008b", "darkcyan" => "#008b8b", "darkgoldenrod" => "#b8860b",
        "darkgray" => "#a9a9a9", "darkgreen" => "#006400", "darkgrey" => "#a9a9a9",
        "darkkhaki" => "#bdb76b", "darkmagenta" => "#8b008b", "darkolivegreen" => "#556b2f",
        "darkorange" => "#ff8c00", "darkorchid" => "#9932cc", "darkred" => "#8b0000",
        "darksalmon" => "#e9967a", "darkseagreen" => "#8fbc8f", "darkslateblue" => "#483d8b",
        "darkslategray" => "#2f4f4f", "darkslategrey" => "#2f4f4f", "darkturquoise" => "#00ced1",
        "darkviolet" => "#9400d3", "deeppink" => "#ff1493", "deepskyblue" => "#00bfff",
        "dimgray" => "#696969", "dimgrey" => "#696969", "dodgerblue" => "#1e90ff",
        "firebrick" => "#b22222", "floralwhite" => "#fffaf0", "forestgreen" => "#228b22",
        "fuchsia" => "#ff00ff", "gainsboro" => "#dcdcdc", "ghostwhite" => "#f8f8ff",
        "gold" => "#ffd700", "goldenrod" => "#daa520", "gray" => "#808080",
        "green" => "#008000", "greenyellow" => "#adff2f", "grey" => "#808080",
        "honeydew" => "#f0fff0", "hotpink" => "#ff69b4", "indianred" => "#cd5c5c",
        "indigo" => "#4b0082", "ivory" => "#fffff0", "khaki" => "#f0e68c",
        "lavender" => "#e6e6fa", "lavenderblush" => "#fff0f5", "lawngreen" => "#7cfc00",
        "lemonchiffon" => "#fffacd", "lightblue" => "#add8e6", "lightcoral" => "#f08080",
        "lightcyan" => "#e0ffff", "lightgoldenrodyellow" => "#fafad2", "lightgray" => "#d3d3d3",
        "lightgreen" => "#90ee90", "lightgrey" => "#d3d3d3", "lightpink" => "#ffb6c1",
        "lightsalmon" => "#ffa07a", "lightseagreen" => "#20b2aa", "lightskyblue" => "#87cefa",
        "lightslategray" => "#778899", "lightslategrey" => "#778899", "lightsteelblue" => "#b0c4de",
        "lightyellow" => "#ffffe0", "lime" => "#00ff00", "limegreen" => "#32cd32",
        "linen" => "#faf0e6", "magenta" => "#ff00ff", "maroon" => "#800000",
        "mediumaquamarine" => "#66cdaa", "mediumblue" => "#0000cd", "mediumorchid" => "#ba55d3",
        "mediumpurple" => "#9370db", "mediumseagreen" => "#3cb371", "mediumslateblue" => "#7b68ee",
        "mediumspringgreen" => "#00fa9a", "mediumturquoise" => "#48d1cc", "mediumvioletred" => "#c71585",
        "midnightblue" => "#191970", "mintcream" => "#f5fffa", "mistyrose" => "#ffe4e1",
        "moccasin" => "#ffe4b5", "navajowhite" => "#ffdead", "navy" => "#000080",
        "oldlace" => "#fdf5e6", "olive" => "#808000", "olivedrab" => "#6b8e23",
        "orange" => "#ffa500", "orangered" => "#ff4500", "orchid" => "#da70d6",
        "palegoldenrod" => "#eee8aa", "palegreen" => "#98fb98", "paleturquoise" => "#afeeee",
        "palevioletred" => "#db7093", "papayawhip" => "#ffefd5", "peachpuff" => "#ffdab9",
        "peru" => "#cd853f", "pink" => "#ffc0cb", "plum" => "#dda0dd",
        "powderblue" => "#b0e0e6", "purple" => "#800080", "rebeccapurple" => "#663399",
        "red" => "#ff0000", "rosybrown" => "#bc8f8f", "royalblue" => "#4169e1",
        "saddlebrown" => "#8b4513", "salmon" => "#fa8072", "sandybrown" => "#f4a460",
        "seagreen" => "#2e8b57", "seashell" => "#fff5ee", "sienna" => "#a0522d",
        "silver" => "#c0c0c0", "skyblue" => "#87ceeb", "slateblue" => "#6a5acd",
        "slategray" => "#708090", "slategrey" => "#708090", "snow" => "#fffafa",
        "springgreen" => "#00ff7f", "steelblue" => "#4682b4", "tan" => "#d2b48c",
        "teal" => "#008080", "thistle" => "#d8bfd8", "tomato" => "#ff6347",
        "turquoise" => "#40e0d0", "violet" => "#ee82ee", "wheat" => "#f5deb3",
        "white" => "#ffffff", "whitesmoke" => "#f5f5f5", "yellow" => "#ffff00",
        "yellowgreen" => "#9acd32",
        _ => return None,
    })
}

/// Find the first color in a declaration value: hex, named color, or an
/// rgb()/rgba()/hsl()/hsla() function. Non-color tokens (url(...), keywords
/// like `no-repeat`, `inherit`) are skipped; returns None when nothing
/// color-shaped is present.
pub(crate) fn parse_color_value(values: &[ComponentValue]) -> Option<Color> {
    let mut i = 0;
    while i < values.len() {
        match &values[i] {
            ComponentValue::HashToken(code) => {
                if let Ok(c) = Color::from_code(code) {
                    return Some(c);
                }
            }
            ComponentValue::Ident(name) => {
                let lower = name.to_ascii_lowercase();
                let is_fn = matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis));
                if is_fn && matches!(lower.as_str(), "rgb" | "rgba" | "hsl" | "hsla") {
                    let mut nums: Vec<(f64, bool)> = Vec::new();
                    let mut j = i + 2;
                    while j < values.len() {
                        match &values[j] {
                            ComponentValue::CloseParenthesis => break,
                            ComponentValue::Number(n) => nums.push((*n, false)),
                            ComponentValue::Dimension(n, u) if u == "%" => nums.push((*n, true)),
                            _ => {}
                        }
                        j += 1;
                    }
                    if let Some(c) = color_from_function(&lower, &nums) {
                        return Some(c);
                    }
                    i = j;
                } else if is_fn {
                    // Skip over an unrelated function (url(...), var(...)).
                    let mut j = i + 2;
                    while j < values.len()
                        && !matches!(values[j], ComponentValue::CloseParenthesis)
                    {
                        j += 1;
                    }
                    i = j;
                } else if lower == "transparent" {
                    return Color::from_name("transparent").ok();
                } else if let Some(code) = named_color_code(&lower) {
                    return Color::from_code(code).ok();
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

fn color_from_function(name: &str, nums: &[(f64, bool)]) -> Option<Color> {
    let channel = |v: f64, pct: bool| -> u8 {
        let n = if pct { v * 255.0 / 100.0 } else { v };
        n.round().clamp(0.0, 255.0) as u8
    };
    let alpha = |v: f64, pct: bool| -> u8 {
        let n = if pct { v / 100.0 } else { v };
        (n * 255.0).round().clamp(0.0, 255.0) as u8
    };
    match name {
        "rgb" | "rgba" => {
            if nums.len() < 3 {
                return None;
            }
            let (r, g, b) = (
                channel(nums[0].0, nums[0].1),
                channel(nums[1].0, nums[1].1),
                channel(nums[2].0, nums[2].1),
            );
            let a = nums.get(3).map(|&(v, p)| alpha(v, p)).unwrap_or(255);
            Some(Color::from_rgba(r, g, b, a))
        }
        "hsl" | "hsla" => {
            if nums.len() < 3 {
                return None;
            }
            let h = nums[0].0.rem_euclid(360.0);
            let s = (nums[1].0 / 100.0).clamp(0.0, 1.0);
            let l = (nums[2].0 / 100.0).clamp(0.0, 1.0);
            let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
            let x = c * (1.0 - ((h / 60.0).rem_euclid(2.0) - 1.0).abs());
            let m = l - c / 2.0;
            let (r1, g1, b1) = match h as u32 {
                0..=59 => (c, x, 0.0),
                60..=119 => (x, c, 0.0),
                120..=179 => (0.0, c, x),
                180..=239 => (0.0, x, c),
                240..=299 => (x, 0.0, c),
                _ => (c, 0.0, x),
            };
            let to8 = |v: f64| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
            let a = nums.get(3).map(|&(v, p)| alpha(v, p)).unwrap_or(255);
            Some(Color::from_rgba(to8(r1), to8(g1), to8(b1), a))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::css::token::CssTokenizer;

    fn vals(s: &str) -> Vec<ComponentValue> {
        let mut t = CssTokenizer::new(s.to_string());
        let mut out = Vec::new();
        while let Some(tok) = t.next() {
            out.push(tok);
        }
        out
    }

    #[test]
    fn calc_folds_absolute_lengths() {
        use crate::renderer::layout::computed_style::FontSize;
        let folded = fold_calc(&vals("calc(20em * -1)"), FontSize::Medium);
        assert_eq!(folded, vals("-320px"), "{:?}", folded);
        let folded = fold_calc(&vals("calc(100px + 2em - (4px / 2))"), FontSize::Medium);
        assert_eq!(folded, vals("130px"), "{:?}", folded);
        // Percentages are left unresolved.
        let kept = fold_calc(&vals("calc(100% - 20px)"), FontSize::Medium);
        assert!(kept.iter().any(|v| matches!(v, ComponentValue::Ident(s) if s == "calc")));
    }

    #[test]
    fn parses_rgb_and_rgba_functions() {
        assert_eq!(
            parse_color_value(&vals("rgb(255, 0, 0)")).unwrap().code(),
            "#ff0000"
        );
        assert_eq!(
            parse_color_value(&vals("rgba(0, 128, 255, 0.5)")).unwrap().code(),
            "#0080ff80"
        );
        assert_eq!(
            parse_color_value(&vals("rgb(100%, 0%, 50%)")).unwrap().code(),
            "#ff0080"
        );
    }

    #[test]
    fn parses_hsl_function() {
        assert_eq!(
            parse_color_value(&vals("hsl(0, 100%, 50%)")).unwrap().code(),
            "#ff0000"
        );
        assert_eq!(
            parse_color_value(&vals("hsl(120, 100%, 25%)")).unwrap().code(),
            "#008000"
        );
    }

    #[test]
    fn parses_extended_named_colors_and_skips_keywords() {
        assert_eq!(
            parse_color_value(&vals("rebeccapurple")).unwrap().code(),
            "#663399"
        );
        assert_eq!(
            parse_color_value(&vals("dimgray")).unwrap().code(),
            "#696969"
        );
        // Keywords and unrelated functions are not colors.
        assert!(parse_color_value(&vals("inherit")).is_none());
        assert!(parse_color_value(&vals("url(bg.png) no-repeat")).is_none());
        // ...but a color after an unrelated function is still found.
        assert_eq!(
            parse_color_value(&vals("url(bg.png) crimson")).unwrap().code(),
            "#dc143c"
        );
    }
}

/// Fold every resolvable `calc(...)` in a declaration value into a plain
/// px Dimension token (absolute lengths and numbers only; percentages and
/// unknown units leave the calc unresolved and untouched). Runs after var()
/// substitution so `calc(var(--x)*2)` folds too.
pub(crate) fn fold_calc(values: &[ComponentValue], base_font_size: FontSize) -> Vec<ComponentValue> {
    let mut out = Vec::with_capacity(values.len());
    let mut i = 0;
    while i < values.len() {
        let is_calc = matches!(&values[i], ComponentValue::Ident(s) if s.eq_ignore_ascii_case("calc"))
            && matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis));
        if is_calc {
            // Find the matching close paren.
            let mut depth = 1;
            let mut j = i + 2;
            while j < values.len() && depth > 0 {
                match &values[j] {
                    ComponentValue::OpenParenthesis => depth += 1,
                    ComponentValue::CloseParenthesis => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            let inner = &values[i + 2..(j - 1).max(i + 2)];
            if let Some(px) = eval_calc_sum(&mut inner.iter().peekable(), base_font_size) {
                out.push(ComponentValue::Dimension(px, "px".to_string()));
            } else {
                // Unresolvable: keep the original tokens (arms will skip).
                out.extend_from_slice(&values[i..j]);
            }
            i = j;
        } else {
            out.push(values[i].clone());
            i += 1;
        }
    }
    out
}

type CalcIter<'a> = core::iter::Peekable<core::slice::Iter<'a, ComponentValue>>;

fn eval_calc_sum(it: &mut CalcIter, base: FontSize) -> Option<f64> {
    let mut acc = eval_calc_product(it, base)?;
    loop {
        match it.peek() {
            Some(ComponentValue::Delim(op @ ('+' | '-'))) => {
                let op = *op;
                it.next();
                let rhs = eval_calc_product(it, base)?;
                if op == '+' {
                    acc += rhs;
                } else {
                    acc -= rhs;
                }
            }
            // The CSS tokenizer's negative-number handling emits a lone
            // minus/plus between spaces as an Ident.
            Some(ComponentValue::Ident(ops)) if ops == "-" || ops == "+" => {
                let op = if ops == "-" { '-' } else { '+' };
                it.next();
                let rhs = eval_calc_product(it, base)?;
                if op == '+' {
                    acc += rhs;
                } else {
                    acc -= rhs;
                }
            }
            Some(ComponentValue::Whitespace) => {
                it.next();
            }
            Some(ComponentValue::CloseParenthesis) | None => return Some(acc),
            _ => return None,
        }
    }
}

fn eval_calc_product(it: &mut CalcIter, base: FontSize) -> Option<f64> {
    let mut acc = eval_calc_atom(it, base)?;
    loop {
        match it.peek() {
            Some(ComponentValue::Delim(op @ ('*' | '/'))) => {
                let op = *op;
                it.next();
                let rhs = eval_calc_atom(it, base)?;
                if op == '*' {
                    acc *= rhs;
                } else if rhs != 0.0 {
                    acc /= rhs;
                } else {
                    return None;
                }
            }
            Some(ComponentValue::Whitespace) => {
                it.next();
            }
            _ => return Some(acc),
        }
    }
}

fn eval_calc_atom(it: &mut CalcIter, base: FontSize) -> Option<f64> {
    while matches!(it.peek(), Some(ComponentValue::Whitespace)) {
        it.next();
    }
    match it.next()? {
        ComponentValue::Number(n) => Some(*n),
        ComponentValue::Dimension(n, unit) if unit != "%" => length_to_px(*n, unit, base),
        ComponentValue::OpenParenthesis => {
            let v = eval_calc_sum(it, base)?;
            match it.next() {
                Some(ComponentValue::CloseParenthesis) => Some(v),
                _ => None,
            }
        }
        _ => None,
    }
}
