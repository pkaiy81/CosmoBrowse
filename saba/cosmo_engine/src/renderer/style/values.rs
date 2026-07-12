//! CSS declaration-value parsing helpers, extracted verbatim from
//! layout_object.rs (plan 0.5). These parse ComponentValues / HTML attribute
//! strings into typed values; they do no layout.

use crate::renderer::css::cssom::ComponentValue;
use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::layout::computed_style::GridTrack;

pub(crate) fn length_to_px(value: f64, unit: &str, base_font_size: FontSize) -> Option<f64> {
    match unit {
        "px" => Some(value),
        "em" => Some(value * base_font_size.px() as f64),
        "rem" => Some(value * FontSize::Medium.px() as f64),
        "vh" | "vw" => Some(value),
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
            ComponentValue::Ident(_) => {
                tracks.push(GridTrack::Auto);
                // Skip a function's argument list (e.g. minmax(0, 1fr)) so its
                // contents don't count as extra tracks.
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
