// Spec: CSS Box Model — margin/border/padding/content areas and box sizing.
// https://www.w3.org/TR/css-box-4/
// Spec: CSS Display — outer/inner display types and block/inline formatting contexts.
// https://www.w3.org/TR/css-display-3/
// Spec: CSS Cascade — specificity, origin, and inheritance resolution order.
// https://www.w3.org/TR/css-cascade-5/
// Spec: CSS Values and Units — length units (px, em, rem, vh, vw) and numeric types.
// https://www.w3.org/TR/css-values-4/
use crate::constants::CHAR_HEIGHT_WITH_PADDING;
use crate::constants::CHAR_WIDTH;
use crate::display_item::ClipRect;
use crate::display_item::DisplayItem;
use crate::display_item::PaintOrder;
use crate::renderer::css::cssom::ComponentValue;
use crate::renderer::css::cssom::Declaration;
use crate::renderer::css::cssom::QualifiedRule;
use crate::renderer::css::cssom::Selector;
use crate::renderer::css::cssom::StyleSheet;
use crate::renderer::dom::node::Element;
use crate::renderer::dom::node::ElementKind;
use crate::renderer::dom::node::Node;
use crate::renderer::dom::node::NodeKind;
use crate::renderer::layout::computed_style::Color;
use crate::renderer::layout::computed_style::ComputedStyle;
use crate::renderer::layout::computed_style::DisplayType;
use crate::renderer::layout::computed_style::FlexDirection;
use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::layout::computed_style::GridTrack;
use crate::renderer::layout::computed_style::LineHeight;
use crate::renderer::layout::computed_style::PositionType;
use crate::renderer::layout::computed_style::TextAlign;
use crate::renderer::layout::computed_style::TextDecoration;
use std::format;
use std::rc::Rc;
use std::rc::Weak;
use std::string::String;
use std::string::ToString;
use std::vec;
use std::vec::Vec;
use std::cell::RefCell;

fn font_ratio(font_size: FontSize) -> i64 {
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
fn char_width_px(font_size: FontSize) -> i64 {
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
fn line_height_px(font_size: FontSize) -> i64 {
    match font_size {
        FontSize::Px(n) => (CHAR_HEIGHT_WITH_PADDING * n / FontSize::Medium.px()).max(1),
        legacy => CHAR_HEIGHT_WITH_PADDING * font_ratio(legacy),
    }
}

/// Line box height honoring an explicit `line-height`; falls back to the
/// default leading for the font size.
fn styled_line_height(style: &ComputedStyle) -> i64 {
    match style.line_height() {
        Some(LineHeight::Px(px)) => (px as i64).max(1),
        Some(LineHeight::Factor(f)) => {
            ((style.font_size_or_default().px() as f64 * f) as i64).max(1)
        }
        None => line_height_px(style.font_size_or_default()),
    }
}

fn edge_to_i64(value: f64) -> i64 {
    if value <= 0.0 {
        0
    } else {
        value as i64
    }
}

fn length_to_px(value: f64, unit: &str, base_font_size: FontSize) -> Option<f64> {
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
fn parse_grid_template_tracks(values: &[ComponentValue]) -> Vec<GridTrack> {
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
fn resolve_grid_tracks(tracks: &[GridTrack], available: i64, column_gap: i64) -> Vec<i64> {
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
fn parse_transform_ops(values: &[ComponentValue]) -> Option<(f64, bool, f64, bool, f64)> {
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
fn parse_transform_rotate(values: &[ComponentValue]) -> Option<f64> {
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
fn bg_position_component(v: &ComponentValue) -> Option<(f64, bool, Option<bool>)> {
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
fn assemble_bg_position(
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
fn bg_size_component(v: &ComponentValue) -> Option<(f64, bool)> {
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
fn assemble_bg_size(comps: &[(f64, bool)]) -> Option<(u8, f64, bool, f64, bool)> {
    match comps {
        [] => None,
        [(w, wp)] => Some((0, *w, *wp, -1.0, false)),
        [(w, wp), (h, hp), ..] => Some((0, *w, *wp, *h, *hp)),
    }
}

/// Parse a `background-size` value list: `cover`, `contain`, or 1–2
/// length/percent/auto components.
fn parse_background_size(values: &[ComponentValue]) -> Option<(u8, f64, bool, f64, bool)> {
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
fn scan_background_shorthand(
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
fn extract_css_url(values: &[ComponentValue]) -> Option<String> {
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

fn first_font_family(value: &[ComponentValue]) -> Option<String> {
    value.iter().find_map(|component| match component {
        ComponentValue::Ident(name) | ComponentValue::StringToken(name) => Some(name.clone()),
        _ => None,
    })
}
fn spacing_component_to_px(component: &ComponentValue, base_font_size: FontSize) -> Option<f64> {
    match component {
        ComponentValue::Number(value) => Some(*value),
        ComponentValue::Dimension(value, unit) => length_to_px(*value, unit, base_font_size),
        _ => None,
    }
}

// Ref: CSS Box Model Level 4, margin and padding shorthands.
// https://drafts.csswg.org/css-box-4/#margin-shorthand
// https://drafts.csswg.org/css-box-4/#padding-shorthand
fn parse_spacing_shorthand(
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


fn margin_component(component: &ComponentValue, base_font_size: FontSize) -> Option<Option<f64>> {
    match component {
        ComponentValue::Ident(name) if name == "auto" => Some(None),
        _ => spacing_component_to_px(component, base_font_size).map(Some),
    }
}

// Spec: CSS Box Model margin shorthand supports `auto` values, which are positional tokens
// and must not be dropped during 1/2/3/4-value expansion.
// https://drafts.csswg.org/css-box-4/#margin-shorthand
fn parse_margin_shorthand(
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

fn parse_margin_auto_flags(value: &[ComponentValue]) -> (bool, bool) {
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

fn parse_dimension_attr(value: Option<String>) -> Option<i64> {
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
fn parse_dimension_pct_attr(value: Option<String>, avail: Option<i64>) -> Option<i64> {
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

fn is_wide_char(c: char) -> bool {
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
fn char_advance_16(c: char) -> i64 {
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
fn scale_advance(advance_16: i64, cw: i64) -> i64 {
    (advance_16 * cw + CHAR_WIDTH - 1) / CHAR_WIDTH
}

/// Estimated width of `text` at the effective per-character width `cw`,
/// accumulating PER-CHARACTER rounded advances — the exact accounting
/// `split_text` uses, so a box sized from this never wraps its own content
/// (a one-shot total scale rounds lower and "login" wrapped as "logi/n").
fn text_width_px(text: &str, cw: i64) -> i64 {
    text.chars()
        .map(|c| scale_advance(char_advance_16(c), cw))
        .sum()
}

/// Truncate `text` so it plus a trailing `…` fits within `max_width` px.
/// Returns the original text when it already fits.
fn truncate_with_ellipsis(text: &str, cw: i64, max_width: i64) -> String {
    if text_width_px(text, cw) <= max_width {
        return text.to_string();
    }
    let ellipsis_w = scale_advance(char_advance_16('…'), cw);
    let budget = (max_width - ellipsis_w).max(0);
    let mut acc = 0i64;
    let mut out = String::new();
    for c in text.chars() {
        let w = scale_advance(char_advance_16(c), cw);
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
fn bold_width_adjust(width: i64, bold: bool) -> i64 {
    if bold {
        width + (width + 7) / 8
    } else {
        width
    }
}

fn measure_text_width(text: &str, font_size: FontSize, bold: bool) -> i64 {
    let cw = bold_width_adjust(char_width_px(font_size), bold);
    text_width_px(text, cw)
}
fn is_break_space(c: char) -> bool {
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
fn split_text(line: String, char_width: i64, max_width: i64) -> Vec<String> {
    let safe_width = max_width.max(char_width).max(1);
    // Line capacity in pixels; per-character advances come from the same
    // class table as measurement so wrapping and sizing agree.
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
        let w = scale_advance(char_advance_16(c), char_width);

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

/// Selector matching against a DOM node. Combinators walk DOM relationships
/// (parent / preceding siblings); simple selectors test the element itself.
fn dom_node_selected(node: &Rc<RefCell<Node>>, selector: &Selector) -> bool {
    // Combinators and grouping first: they recurse into simple matching.
    match selector {
        Selector::List(alternatives) => {
            return alternatives.iter().any(|s| dom_node_selected(node, s));
        }
        Selector::Compound(parts) => {
            return parts.iter().all(|s| dom_node_selected(node, s));
        }
        Selector::Child(ancestor, this) => {
            return dom_node_selected(node, this)
                && node
                    .borrow()
                    .parent()
                    .upgrade()
                    .map(|p| dom_node_selected(&p, ancestor))
                    .unwrap_or(false);
        }
        Selector::Descendant(ancestor, this) => {
            if !dom_node_selected(node, this) {
                return false;
            }
            let mut current = node.borrow().parent().upgrade();
            while let Some(p) = current {
                if dom_node_selected(&p, ancestor) {
                    return true;
                }
                let next = p.borrow().parent().upgrade();
                current = next;
            }
            return false;
        }
        Selector::NextSibling(prev, this) | Selector::SubsequentSibling(prev, this) => {
            if !dom_node_selected(node, this) {
                return false;
            }
            let adjacent_only = matches!(selector, Selector::NextSibling(..));
            // Walk the parent's children up to this node, tracking preceding
            // ELEMENT siblings (text nodes are not siblings for + / ~).
            let parent = match node.borrow().parent().upgrade() {
                Some(p) => p,
                None => return false,
            };
            let mut matched_any = false;
            let mut last_was_match = false;
            let mut child = parent.borrow().first_child();
            while let Some(c) = child {
                if Rc::ptr_eq(&c, node) {
                    return if adjacent_only { last_was_match } else { matched_any };
                }
                if matches!(c.borrow().kind(), NodeKind::Element(_)) {
                    let m = dom_node_selected(&c, prev);
                    last_was_match = m;
                    matched_any |= m;
                }
                let next = c.borrow().next_sibling();
                child = next;
            }
            return false;
        }
        Selector::Never => return false,
        // A pseudo-element rule never matches the host element itself (its
        // declarations would otherwise leak); the layout tree builder applies
        // it to a synthesized generated box via pseudo_element_target.
        Selector::PseudoElement(..) => return false,
        Selector::Not(inner) => {
            return matches!(node.borrow().kind(), NodeKind::Element(_))
                && !dom_node_selected(node, inner);
        }
        Selector::PseudoClass(kind) => {
            if !matches!(node.borrow().kind(), NodeKind::Element(_)) {
                return false;
            }
            use crate::renderer::css::cssom::PseudoClassKind;
            // `:root` is the <html> element (it has no element parent).
            if matches!(kind, PseudoClassKind::Root) {
                let parent_is_element = node
                    .borrow()
                    .parent()
                    .upgrade()
                    .map(|p| matches!(p.borrow().kind(), NodeKind::Element(_)))
                    .unwrap_or(false);
                return !parent_is_element;
            }
            // 1-based index of this element among its parent's ELEMENT
            // children (and, for the *-of-type family, among element children
            // with the SAME tag), plus the respective totals.
            let own_tag = match node.borrow().kind() {
                NodeKind::Element(ref e) => e.tag_name().to_string(),
                _ => return false,
            };
            let parent = match node.borrow().parent().upgrade() {
                Some(p) => p,
                None => return false,
            };
            let mut index = 0usize;
            let mut total = 0usize;
            let mut index_of_type = 0usize;
            let mut total_of_type = 0usize;
            let mut child = parent.borrow().first_child();
            while let Some(c) = child {
                if let NodeKind::Element(ref e) = c.borrow().kind() {
                    total += 1;
                    let same_type = e.tag_name() == own_tag;
                    if same_type {
                        total_of_type += 1;
                    }
                    if Rc::ptr_eq(&c, node) {
                        index = total;
                        index_of_type = total_of_type;
                    }
                }
                let next = c.borrow().next_sibling();
                child = next;
            }
            if index == 0 {
                return false;
            }
            // i matches An+B when i = A*n + B for some integer n ≥ 0.
            let nth_matches = |a: i64, b: i64, i: i64| -> bool {
                if a == 0 {
                    i == b
                } else {
                    let d = i - b;
                    d % a == 0 && d / a >= 0
                }
            };
            return match kind {
                PseudoClassKind::Root => unreachable!("handled above"),
                PseudoClassKind::FirstChild => index == 1,
                PseudoClassKind::LastChild => index == total,
                PseudoClassKind::OnlyChild => total == 1,
                PseudoClassKind::NthChild(a, b) => nth_matches(*a, *b, index as i64),
                PseudoClassKind::NthLastChild(a, b) => {
                    nth_matches(*a, *b, (total - index + 1) as i64)
                }
                PseudoClassKind::FirstOfType => index_of_type == 1,
                PseudoClassKind::LastOfType => index_of_type == total_of_type,
                PseudoClassKind::OnlyOfType => total_of_type == 1,
                PseudoClassKind::NthOfType(a, b) => nth_matches(*a, *b, index_of_type as i64),
                PseudoClassKind::NthLastOfType(a, b) => {
                    nth_matches(*a, *b, (total_of_type - index_of_type + 1) as i64)
                }
            };
        }
        _ => {}
    }
    match node.borrow().kind() {
        NodeKind::Element(ref e) => match selector {
            Selector::TypeSelector(type_name) => e.tag_name() == *type_name,
            // An element may carry several space-separated class names;
            // the selector matches any one of them.
            // https://html.spec.whatwg.org/multipage/dom.html#classes
            Selector::ClassSelector(class_name) => e.attributes().iter().any(|attr| {
                attr.name() == "class"
                    && attr.value().split_ascii_whitespace().any(|c| c == class_name)
            }),
            Selector::IdSelector(id_name) => e
                .attributes()
                .iter()
                .any(|attr| attr.name() == "id" && attr.value() == *id_name),
            Selector::Attribute { name, op, value } => e
                .attributes()
                .iter()
                .filter(|attr| attr.name().eq_ignore_ascii_case(name))
                .any(|attr| {
                    let v = attr.value();
                    use crate::renderer::css::cssom::AttrOp;
                    match op {
                        AttrOp::Exists => true,
                        AttrOp::Equals => v == *value,
                        AttrOp::Includes => v.split_ascii_whitespace().any(|w| w == value),
                        AttrOp::DashMatch => {
                            v == *value || v.starts_with(&format!("{}-", value))
                        }
                        AttrOp::Prefix => !value.is_empty() && v.starts_with(value.as_str()),
                        AttrOp::Suffix => !value.is_empty() && v.ends_with(value.as_str()),
                        AttrOp::Substring => !value.is_empty() && v.contains(value.as_str()),
                    }
                }),
            Selector::Universal => true,
            Selector::UnknownSelector => false,
            // Handled above; unreachable here.
            _ => false,
        },
        _ => false,
    }
}

pub fn create_layout_object(
    node: &Option<Rc<RefCell<Node>>>,
    parent_obj: &Option<Rc<RefCell<LayoutObject>>>,
    cssom: &StyleSheet,
) -> Option<Rc<RefCell<LayoutObject>>> {
    if let Some(n) = node {
        let layout_object = Rc::new(RefCell::new(LayoutObject::new(n.clone(), parent_obj)));

        // Parent font size: the base for resolving font-size em/% values.
        let parent_font_size = parent_obj
            .as_ref()
            .map(|p| p.borrow().style().font_size())
            .unwrap_or(FontSize::Medium);

        // Cascade order: matching rules sorted by specificity (ascending) so a
        // more specific selector wins even when it appears earlier in the
        // document; the stable sort keeps document order for equal
        // specificity (later rule wins). Spec: CSS Cascade §6.
        // https://www.w3.org/TR/css-cascade-4/#cascade-specificity
        let mut matched: Vec<&QualifiedRule> = cssom
            .rules
            .iter()
            .filter(|rule| layout_object.borrow().is_node_selected(&rule.selector))
            .collect();
        matched.sort_by_key(|rule| rule.selector.specificity());

        // Inline `style="..."` attribute declarations (parsed once, applied in
        // the importance tiers below).
        // Spec: https://www.w3.org/TR/css-style-attr/#interpret
        let inline_declarations: Vec<Declaration> = match n.borrow().kind() {
            NodeKind::Element(ref element) => element
                .get_attribute("style")
                .map(|style_attr| {
                    use crate::renderer::css::cssom::CssParser;
                    use crate::renderer::css::token::CssTokenizer;
                    CssParser::new(CssTokenizer::new(style_attr)).parse_declaration_list()
                })
                .unwrap_or_default(),
            _ => Vec::new(),
        };

        // Custom-property cascade: inherit the parent's scope; the tree root
        // seeds from the whole stylesheet (this also covers `:root`, which has
        // no layout object — the layout tree is rooted at <body>). Element-
        // level `--name` definitions from matched rules and the inline style
        // copy-on-write into a fresh map, resolving nested var() against the
        // scope built so far. Must be in place BEFORE normal declarations are
        // applied, since their var() references substitute from this scope.
        // https://www.w3.org/TR/css-variables-1/
        {
            use crate::renderer::css::cssom::substitute_vars;
            let inherited = parent_obj
                .as_ref()
                .and_then(|p| p.borrow().style().custom_properties().cloned())
                .unwrap_or_else(|| {
                    // Layout-tree root (<body>): its DOM ancestors (<html>,
                    // matched by `:root`) have no layout objects, so evaluate
                    // their matching rules here to seed the scope.
                    let mut map = std::collections::BTreeMap::new();
                    let mut chain: Vec<Rc<RefCell<Node>>> = Vec::new();
                    let mut current = n.borrow().parent().upgrade();
                    while let Some(p) = current {
                        if matches!(p.borrow().kind(), NodeKind::Element(_)) {
                            chain.push(p.clone());
                        }
                        let next = p.borrow().parent().upgrade();
                        current = next;
                    }
                    for ancestor in chain.iter().rev() {
                        for rule in &cssom.rules {
                            if dom_node_selected(ancestor, &rule.selector) {
                                for d in rule
                                    .declarations
                                    .iter()
                                    .filter(|d| d.property.starts_with("--"))
                                {
                                    let value = substitute_vars(&d.value, &map);
                                    map.insert(d.property.clone(), value);
                                }
                            }
                        }
                    }
                    Rc::new(map)
                });
            let mut own: Option<std::collections::BTreeMap<String, Vec<ComponentValue>>> =
                None;
            for declarations in matched
                .iter()
                .map(|rule| &rule.declarations)
                .chain(std::iter::once(&inline_declarations))
            {
                for d in declarations.iter().filter(|d| d.property.starts_with("--")) {
                    let map = own.get_or_insert_with(|| (*inherited).clone());
                    let value = substitute_vars(&d.value, map);
                    map.insert(d.property.clone(), value);
                }
            }
            let scope = own.map(Rc::new).unwrap_or(inherited);
            layout_object.borrow_mut().style.set_custom_properties(scope);
        }

        // Importance tiers (weakest first; the last write wins in this
        // engine's cascade): normal stylesheet declarations, normal inline
        // declarations, !important stylesheet declarations, !important inline
        // declarations. Spec: CSS Cascade §6.1 — declarations marked
        // !important outrank all normal declarations of the same origin.
        // https://www.w3.org/TR/css-cascade-4/#cascade-origin
        for important in [false, true] {
            for rule in &matched {
                let tier: Vec<Declaration> = rule
                    .declarations
                    .iter()
                    .filter(|d| d.important == important)
                    .cloned()
                    .collect();
                if !tier.is_empty() {
                    layout_object
                        .borrow_mut()
                        .cascading_style(tier, parent_font_size);
                }
            }
            let tier: Vec<Declaration> = inline_declarations
                .iter()
                .filter(|d| d.important == important)
                .cloned()
                .collect();
            if !tier.is_empty() {
                layout_object
                    .borrow_mut()
                    .cascading_style(tier, parent_font_size);
            }
        }

        let parent_style = parent_obj.as_ref().map(|parent| parent.borrow().style());
        layout_object.borrow_mut().defaulting_style(n, parent_style);

        if layout_object.borrow().style().display() == DisplayType::DisplayNone {
            return None;
        }

        layout_object.borrow_mut().update_kind();
        return Some(layout_object);
    }
    None
}

/// Build a `::before` / `::after` generated-content box for `host`, if a
/// matching pseudo-element rule supplies a non-empty `content` string.
/// The synthesized box is an inline element (carrying the pseudo rules'
/// styles, inheriting the host's) wrapping a text node with the content.
/// Spec: CSS Generated Content §2. https://www.w3.org/TR/css-content-3/
pub fn build_pseudo_element(
    host_node: &Rc<RefCell<Node>>,
    host_obj: &Rc<RefCell<LayoutObject>>,
    cssom: &StyleSheet,
    pe: crate::renderer::css::cssom::PseudoElement,
) -> Option<Rc<RefCell<LayoutObject>>> {
    use crate::renderer::css::cssom::Selector;
    // Collect matching pseudo-element rules (host part matches host_node),
    // sorted by specificity.
    let mut matched: Vec<&QualifiedRule> = cssom
        .rules
        .iter()
        .filter(|rule| match &rule.selector {
            Selector::PseudoElement(host_sel, kind) => {
                *kind == pe && dom_node_selected(host_node, host_sel)
            }
            _ => false,
        })
        .collect();
    matched.sort_by_key(|rule| rule.selector.specificity());

    // Resolve the `content` string (last declaration wins). `none`/`normal`
    // suppress the box.
    let mut content: Option<String> = None;
    for rule in &matched {
        for d in &rule.declarations {
            if d.property == "content" {
                content = pseudo_content_string(&d.value);
            }
        }
    }
    let content = content?;

    // Synthesize an inline element node + text child, parented (in the DOM
    // sense) under the host so selector matching of nested rules behaves.
    let span_node = Rc::new(RefCell::new(Node::new(NodeKind::Element(Element::new(
        "span",
        Vec::new(),
    )))));
    span_node
        .borrow_mut()
        .set_parent(Rc::downgrade(host_node));
    let text_node = Rc::new(RefCell::new(Node::new(NodeKind::Text(content))));
    text_node
        .borrow_mut()
        .set_parent(Rc::downgrade(&span_node));
    span_node
        .borrow_mut()
        .set_first_child(Some(text_node.clone()));

    let parent_obj = Some(host_obj.clone());
    let span_obj = Rc::new(RefCell::new(LayoutObject::new(span_node.clone(), &parent_obj)));
    let parent_font_size = host_obj.borrow().style().font_size();

    // Apply the pseudo rules' declarations (skip `content`, not a property).
    for important in [false, true] {
        for rule in &matched {
            let tier: Vec<Declaration> = rule
                .declarations
                .iter()
                .filter(|d| d.important == important && d.property != "content")
                .cloned()
                .collect();
            if !tier.is_empty() {
                span_obj.borrow_mut().cascading_style(tier, parent_font_size);
            }
        }
    }
    let host_style = Some(host_obj.borrow().style());
    span_obj.borrow_mut().defaulting_style(&span_node, host_style);
    if span_obj.borrow().style().display() == DisplayType::DisplayNone {
        return None;
    }
    span_obj.borrow_mut().update_kind();

    // Text child.
    let text_obj = Rc::new(RefCell::new(LayoutObject::new(
        text_node.clone(),
        &Some(span_obj.clone()),
    )));
    let span_style = Some(span_obj.borrow().style());
    text_obj.borrow_mut().defaulting_style(&text_node, span_style);
    text_obj.borrow_mut().update_kind();
    span_obj.borrow_mut().set_first_child(Some(text_obj));

    Some(span_obj)
}

/// Extract the string literal from a `content` value. Returns None for
/// `none`/`normal` or non-string values (url()/counters aren't supported).
fn pseudo_content_string(values: &[ComponentValue]) -> Option<String> {
    let mut out = String::new();
    let mut saw = false;
    for v in values {
        match v {
            ComponentValue::StringToken(s) => {
                out.push_str(s);
                saw = true;
            }
            ComponentValue::Ident(name)
                if name.eq_ignore_ascii_case("none")
                    || name.eq_ignore_ascii_case("normal") =>
            {
                return None;
            }
            _ => {}
        }
    }
    if saw {
        Some(out)
    } else {
        None
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LayoutObjectKind {
    Block,
    Inline,
    Text,
}


#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum LayoutFlow {
    BlockFormattingContext,
    InlineFlow,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct NormalFlowSpec {
    pub flow: LayoutFlow,
    pub stacks_vertically: bool,
    pub keeps_inline_line: bool,
}

impl LayoutObjectKind {
    pub fn normal_flow_spec(&self) -> NormalFlowSpec {
        match self {
            LayoutObjectKind::Block => NormalFlowSpec {
                flow: LayoutFlow::BlockFormattingContext,
                stacks_vertically: true,
                keeps_inline_line: false,
            },
            LayoutObjectKind::Inline | LayoutObjectKind::Text => NormalFlowSpec {
                flow: LayoutFlow::InlineFlow,
                stacks_vertically: false,
                keeps_inline_line: true,
            },
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BoxEdges {
    pub top: i64,
    pub right: i64,
    pub bottom: i64,
    pub left: i64,
}

impl BoxEdges {
    pub fn horizontal(&self) -> i64 {
        self.left + self.right
    }

    pub fn vertical(&self) -> i64 {
        self.top + self.bottom
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BoxModelMetrics {
    pub margin: BoxEdges,
    pub padding: BoxEdges,
    pub border: BoxEdges,
}

impl BoxModelMetrics {
    pub fn outer_horizontal(&self) -> i64 {
        self.margin.horizontal() + self.padding.horizontal() + self.border.horizontal()
    }

    pub fn outer_vertical(&self) -> i64 {
        self.margin.vertical() + self.padding.vertical() + self.border.vertical()
    }

    pub fn inner_horizontal(&self) -> i64 {
        self.padding.horizontal() + self.border.horizontal()
    }

    pub fn inner_vertical(&self) -> i64 {
        self.padding.vertical() + self.border.vertical()
    }
}

pub fn compute_box_model_metrics(style: &ComputedStyle) -> BoxModelMetrics {
    let margin = style.margin();
    let padding = style.padding();
    let border = style.border();

    BoxModelMetrics {
        margin: BoxEdges {
            top: edge_to_i64(margin.top()),
            right: edge_to_i64(margin.right()),
            bottom: edge_to_i64(margin.bottom()),
            left: edge_to_i64(margin.left()),
        },
        padding: BoxEdges {
            top: edge_to_i64(padding.top()),
            right: edge_to_i64(padding.right()),
            bottom: edge_to_i64(padding.bottom()),
            left: edge_to_i64(padding.left()),
        },
        border: BoxEdges {
            top: edge_to_i64(border.top()),
            right: edge_to_i64(border.right()),
            bottom: edge_to_i64(border.bottom()),
            left: edge_to_i64(border.left()),
        },
    }
}

#[derive(Debug, Clone)]
pub struct LayoutObject {
    kind: LayoutObjectKind,
    node: Rc<RefCell<Node>>,
    first_child: Option<Rc<RefCell<LayoutObject>>>,
    next_sibling: Option<Rc<RefCell<LayoutObject>>>,
    parent: Weak<RefCell<LayoutObject>>,
    style: ComputedStyle,
    point: LayoutPoint,
    size: LayoutSize,
    // The max_width used in split_text() during compute_size for Text nodes.
    // Cached here so that paint() uses the identical line-breaking boundary,
    // preventing the double-split divergence that causes text to stack
    // vertically instead of flowing horizontally.
    // Spec: CSS2.2 §9.4.2 — inline formatting context line construction.
    // https://www.w3.org/TR/CSS22/visuren.html#inline-formatting
    text_line_max_width: i64,
    // Per-logical-column max of min_content_width_hint, populated once per
    // table by the pre-pass before any cell sizing.  Only meaningful on table
    // nodes; None elsewhere.  Used by `table_cell_auto_width` so that a row
    // whose cell content is narrow (e.g. &nbsp;) still reserves space for a
    // sibling row whose cell at the same column has substantial content.
    // Spec: CSS 2.2 §17.5.2 — table layout: auto.
    // https://www.w3.org/TR/CSS22/tables.html#auto-table-layout
    column_min_hints: Option<Vec<i64>>,
    // Per-logical-column max of max_content_width (the column's preferred,
    // longest-line width), populated alongside column_min_hints by the
    // pre-pass.  Used by `table_cell_auto_width` to weight surplus distribution
    // by each column's growth headroom (max - min), so a narrow label column
    // (e.g. a rank number) does not absorb surplus meant for a wide text column.
    column_max_hints: Option<Vec<i64>>,
}

impl PartialEq for LayoutObject {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

impl LayoutObject {
    pub fn new(node: Rc<RefCell<Node>>, parent_obj: &Option<Rc<RefCell<LayoutObject>>>) -> Self {
        let parent = match parent_obj {
            Some(p) => Rc::downgrade(p),
            None => Weak::new(),
        };

        Self {
            kind: LayoutObjectKind::Block,
            node: node.clone(),
            first_child: None,
            next_sibling: None,
            parent,
            style: ComputedStyle::new(),
            point: LayoutPoint::new(0, 0),
            size: LayoutSize::new(0, 0),
            text_line_max_width: 0,
            column_min_hints: None,
            column_max_hints: None,
        }
    }

    fn link_href(&self) -> Option<String> {
        let mut current = Some(self.node.clone());
        while let Some(node) = current {
            if let NodeKind::Element(element) = node.borrow().kind() {
                if element.kind() == ElementKind::A {
                    return element.get_attribute("href");
                }
            }
            current = node.borrow().parent().upgrade();
        }
        None
    }

    /// Walk ancestor chain to the nearest `<a>` element and return its `target`
    /// attribute value.
    /// Spec: HTML Living Standard §4.6.21 — the `target` attribute on `<a>`.
    /// https://html.spec.whatwg.org/multipage/links.html#attr-hyperlink-target
    fn link_target(&self) -> Option<String> {
        let mut current = Some(self.node.clone());
        while let Some(node) = current {
            if let NodeKind::Element(element) = node.borrow().kind() {
                if element.kind() == ElementKind::A {
                    return element.get_attribute("target");
                }
            }
            current = node.borrow().parent().upgrade();
        }
        None
    }

    pub fn element_kind(&self) -> Option<ElementKind> {
        self.node.borrow().element_kind()
    }

    pub fn is_table_cell(&self) -> bool {
        matches!(self.element_kind(), Some(ElementKind::Td) | Some(ElementKind::Th))
    }

    pub fn is_table_row(&self) -> bool {
        matches!(self.element_kind(), Some(ElementKind::Tr))
    }

    pub fn is_row_group(&self) -> bool {
        matches!(self.element_kind(),
            Some(ElementKind::Tbody) | Some(ElementKind::Thead) | Some(ElementKind::Tfoot))
    }

    /// Walk up the layout tree to find the nearest ancestor table cell.
    fn nearest_ancestor_cell(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        let mut current = self.parent.upgrade();
        while let Some(node) = current {
            if node.borrow().is_table_cell() {
                return Some(node);
            }
            let next = node.borrow().parent.upgrade();
            current = next;
        }
        None
    }

    /// Walk up the layout tree to find the nearest block-level ancestor (including
    /// table cells), used to derive a reliable max-width for text wrapping.
    fn nearest_block_ancestor_width(&self) -> Option<i64> {
        let mut current = self.parent.upgrade();
        while let Some(node) = current {
            let b = node.borrow();
            if b.is_table_cell() || b.kind() == LayoutObjectKind::Block {
                let cm = compute_box_model_metrics(&b.style);
                let w = b.size().width() - cm.inner_horizontal();
                return if w > 0 { Some(w) } else { None };
            }
            let next = b.parent.upgrade();
            drop(b);
            current = next;
        }
        None
    }

    /// If an ancestor declares `text-overflow: ellipsis`, return the px width
    /// available to this text node before the ancestor's right content edge.
    /// `text-overflow` only takes effect on a clipping container, so the
    /// ancestor must also clip overflow.
    fn ellipsis_clip_width(&self) -> Option<i64> {
        let mut current = self.parent.upgrade();
        while let Some(node) = current {
            let b = node.borrow();
            if b.style().text_overflow_ellipsis() && b.style().overflow_clip() {
                let cm = compute_box_model_metrics(&b.style);
                let right = b.point().x() + b.size().width() - cm.border.right - cm.padding.right;
                let avail = right - self.point().x();
                return Some(avail.max(0));
            }
            let next = b.parent.upgrade();
            drop(b);
            current = next;
        }
        None
    }

    /// Scan descendants (up to `depth` levels) for elements that imply a
    /// minimum width (e.g. `<img width="350">`, `<table width="256">`).
    /// Returns the maximum such hint, or 0 if none found.
    /// If this object's parent is a flex container (`display:flex`), return the
    /// container's main-axis direction; otherwise `None`. Used so a flex item
    /// can size and position itself against the flex algorithm.
    fn parent_flex_direction(&self) -> Option<FlexDirection> {
        let parent = self.parent.upgrade()?;
        let p = parent.borrow();
        if p.style.display() == DisplayType::Flex {
            Some(p.style.flex_direction())
        } else {
            None
        }
    }

    fn is_flex_container(&self) -> bool {
        self.style.display() == DisplayType::Flex
    }

    /// If this object's parent is a grid container (`display:grid`), return
    /// its (column tracks, column gap, row gap); otherwise `None`.
    fn parent_grid_info(&self) -> Option<(Vec<GridTrack>, i64, i64)> {
        let parent = self.parent.upgrade()?;
        let p = parent.borrow();
        if p.style.display() == DisplayType::Grid {
            Some((
                p.style.grid_template_columns(),
                p.style.column_gap(),
                p.style.row_gap(),
            ))
        } else {
            None
        }
    }

    /// True for a whitespace-only text node. Such nodes are formatting
    /// artifacts of the markup (newlines/indentation between elements) and are
    /// not grid items per CSS Grid §6 (only inter-element whitespace that
    /// collapses away).
    fn is_whitespace_text(&self) -> bool {
        match self.node.borrow().kind() {
            NodeKind::Text(ref t) => t.trim().is_empty(),
            _ => false,
        }
    }

    /// 0-based grid-item index of this object among its parent's children:
    /// whitespace-only text siblings are skipped (they are not grid items).
    /// Identified by pointer identity, so it works while `self` is inside an
    /// active borrow.
    fn grid_item_index(&self) -> usize {
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return 0,
        };
        let mut idx = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            if std::ptr::eq(c.as_ptr() as *const LayoutObject, self as *const LayoutObject) {
                return idx;
            }
            let b = c.borrow();
            if !b.is_whitespace_text() {
                idx += 1;
            }
            let next = b.next_sibling();
            drop(b);
            child = next;
        }
        idx
    }

    /// Preferred (max-content) width: the width this subtree would take with no
    /// line wrapping. Inline/text runs accumulate on a line; block children
    /// each take their own line (we take the max). Used to shrink-to-fit flex
    /// row items so siblings sit side by side instead of each filling the row.
    fn max_content_width(&self) -> i64 {
        self.max_content_width_depth(6)
    }

    fn max_content_width_depth(&self, depth: u32) -> i64 {
        if depth == 0 {
            return 0;
        }
        if let Some(w) = parse_dimension_attr(self.element_attribute("width")) {
            return w;
        }
        let mut max_line: i64 = 0; // widest block-level line seen
        let mut cur_line: i64 = 0; // current inline run accumulation
        let mut child = self.first_child();
        while let Some(c) = child {
            let b = c.borrow();
            match b.node_kind() {
                NodeKind::Text(ref t) => {
                    let fs = b.style.font_size();
                    let bold = b.style.is_bold();
                    // Each hard line within the text starts a new line box.
                    let widest = t
                        .split('\n')
                        .map(|line| measure_text_width(line.trim(), fs, bold))
                        .max()
                        .unwrap_or(0);
                    cur_line += widest;
                }
                _ => {
                    let child_pref = b.max_content_width_depth(depth - 1);
                    if b.kind().normal_flow_spec().stacks_vertically {
                        // Block-level child: flush the inline run and stand alone.
                        max_line = max_line.max(cur_line);
                        cur_line = 0;
                        max_line = max_line.max(child_pref);
                    } else {
                        cur_line += child_pref;
                    }
                }
            }
            let next = b.next_sibling();
            drop(b);
            child = next;
        }
        max_line.max(cur_line)
    }

    fn min_content_width_hint(&self) -> i64 {
        self.min_content_width_hint_depth(6)
    }

    fn min_content_width_hint_depth(&self, depth: u32) -> i64 {
        if depth == 0 {
            return 0;
        }
        let mut max_hint: i64 = 0;
        let mut child = self.first_child();
        while let Some(c) = child {
            let borrowed = c.borrow();
            let hint = parse_dimension_attr(borrowed.element_attribute("width")).unwrap_or(0);
            max_hint = max_hint.max(hint);
            // An explicit CSS width on a child box (e.g. HN's
            // `.votearrow{width:10px}`) is part of the column's minimum: the
            // box cannot shrink below it, plus its horizontal margins.
            let css_w = borrowed.style.width() as i64;
            if css_w > 0 {
                let m = borrowed.style.margin();
                max_hint = max_hint.max(css_w + m.left() as i64 + m.right() as i64);
            }
            // For text nodes, use the longest *unbreakable* run.
            // CJK chars can break between any two characters; each is its own
            // minimum unit.  Pure-ASCII runs between wide chars are truly
            // unbreakable and may be wider — take the max of the two.
            if let NodeKind::Text(ref t) = borrowed.node_kind() {
                let font_size = borrowed.style.font_size();
                let bold = borrowed.style.is_bold();
                let longest = t.split(|c: char| c == ' ' || c == '\u{3000}' || c == '\n' || c == '\t')
                    .map(|word| {
                        // Longest ASCII run between wide chars within this word.
                        let ascii_max = word.split(|c: char| is_wide_char(c))
                            .map(|seg| measure_text_width(seg.trim(), font_size, bold))
                            .max()
                            .unwrap_or(0);
                        // Each wide char is its own break unit.  We return
                        // 3×CHAR_WIDTH so that any cell with CJK content exceeds
                        // SPACER_THRESHOLD (20) and is classified as flexible
                        // content rather than a decorative spacer.
                        let wide_min = if word.chars().any(|c| is_wide_char(c)) {
                            3 * char_width_px(font_size)
                        } else {
                            0
                        };
                        ascii_max.max(wide_min)
                    })
                    .max()
                    .unwrap_or(0);
                max_hint = max_hint.max(longest);
            }
            max_hint = max_hint.max(borrowed.min_content_width_hint_depth(depth - 1));
            let next = borrowed.next_sibling();
            drop(borrowed);
            child = next;
        }
        max_hint
    }

    /// Determine this cell's LOGICAL column index (0-based) within its parent
    /// row: preceding cells advance the index by their colspan, so a cell after
    /// a `<td colspan=2>` lead-in is at column 2, not 1. (Rowspan cells from
    /// previous rows are not accounted for here; callers gate on
    /// `rowspan_offset == 0`.)
    fn cell_column_index(&self) -> usize {
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return 0,
        };
        let mut index: usize = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            match c.try_borrow() {
                Ok(borrowed) => {
                    if borrowed.is_table_cell() {
                        index += borrowed.cell_colspan();
                    }
                    let next = borrowed.next_sibling();
                    drop(borrowed);
                    child = next;
                }
                Err(_) => {
                    // This is self.
                    return index;
                }
            }
        }
        index
    }

    /// Look at sibling rows in the same table for an explicit width or already-
    /// computed size at the given column index.  Returns the width if found.
    /// Handles tables with tbody/thead/tfoot row groups transparently.
    fn column_width_from_sibling_rows(&self, col_index: usize) -> Option<i64> {
        let row = self.parent.upgrade()?;
        // When a row group (<tbody>/<thead>/<tfoot>) wraps <tr>, step up one more level.
        let row_parent = row.borrow().parent.upgrade()?;
        let table = if row_parent.borrow().is_row_group() {
            row_parent.borrow().parent.upgrade()?
        } else {
            row_parent
        };
        // Resolve percentage width attributes against the table's explicit pixel width.
        let table_px_width = parse_dimension_attr(table.borrow().element_attribute("width"))
            .or_else(|| {
                let tw = table.borrow().size.width();
                if tw > 0 { Some(tw) } else { None }
            });
        // Collect all logical rows (across all row groups).
        let logical_rows = Self::collect_logical_rows_static(&table);
        let mut best: Option<i64> = None;
        for sr in &logical_rows {
            if Rc::ptr_eq(sr, &row) {
                continue;
            }
            let sr_borrowed = sr.borrow();
            let mut idx: usize = 0;
            let mut cell = sr_borrowed.first_child();
            while let Some(c) = cell {
                let cb = c.borrow();
                if cb.is_table_cell() {
                    if idx == col_index {
                        // A colspan > 1 cell spans multiple logical columns.
                        // Its full width must not be used as a hint for a single
                        // column: doing so would over-allocate that column and
                        // starve subsequent columns of space.
                        if cb.cell_colspan() == 1 {
                            let candidate =
                                parse_dimension_pct_attr(cb.element_attribute("width"), table_px_width)
                                .or_else(|| {
                                    let total = cb.size.width();
                                    if total > 0 {
                                        // Return the CONTENT width (strip border+padding overhead)
                                        // so that each row inherits a consistent column width
                                        // regardless of how many border pixels were already
                                        // accumulated.  Without this, every row would be 2px
                                        // wider than the previous one (border inflation).
                                        let cb_metrics = compute_box_model_metrics(&cb.style);
                                        Some((total - cb_metrics.inner_horizontal()).max(0))
                                    } else {
                                        None
                                    }
                                });
                            if let Some(w) = candidate {
                                best = Some(best.map_or(w, |b: i64| b.max(w)));
                            }
                        }
                    }
                    // Advance by the cell's colspan so that sibling rows with
                    // colspan cells (e.g. a `<td colspan=2>` lead-in) map to
                    // LOGICAL column indices, matching the caller's index.
                    idx += cb.cell_colspan();
                }
                let next = cb.next_sibling();
                drop(cb);
                cell = next;
            }
        }
        best
    }

    /// Collect all logical `<tr>` children of a table, traversing through row groups.
    fn collect_logical_rows_static(table: &Rc<RefCell<LayoutObject>>) -> Vec<Rc<RefCell<LayoutObject>>> {
        let mut rows = Vec::new();
        let mut child = table.borrow().first_child();
        while let Some(c) = child {
            let is_row = c.borrow().is_table_row();
            let is_group = c.borrow().is_row_group();
            if is_row {
                rows.push(c.clone());
            } else if is_group {
                let mut grandchild = c.borrow().first_child();
                while let Some(gc) = grandchild {
                    if gc.borrow().is_table_row() {
                        rows.push(gc.clone());
                    }
                    let next = gc.borrow().next_sibling();
                    grandchild = next;
                }
            }
            let next = c.borrow().next_sibling();
            child = next;
        }
        rows
    }

    /// Public entry point for `column_width_from_sibling_rows` so that sibling
    /// cells can call it during the width-distribution pass.
    pub fn column_width_from_sibling_rows_pub(&self, col_index: usize) -> Option<i64> {
        self.column_width_from_sibling_rows(col_index)
    }

    /// Return the per-column max content hint vector populated by the table
    /// pre-pass, if any. Only set on `<table>` nodes.
    pub fn column_min_hints(&self) -> Option<&[i64]> {
        self.column_min_hints.as_deref()
    }

    /// Store the per-column max content hint vector on this (table) node.
    pub fn set_column_min_hints(&mut self, v: Vec<i64>) {
        self.column_min_hints = Some(v);
    }

    /// Return the per-column preferred (max-content) hint vector populated by
    /// the table pre-pass, if any. Only set on `<table>` nodes.
    pub fn column_max_hints(&self) -> Option<&[i64]> {
        self.column_max_hints.as_deref()
    }

    /// Store the per-column preferred (max-content) hint vector on this table.
    pub fn set_column_max_hints(&mut self, v: Vec<i64>) {
        self.column_max_hints = Some(v);
    }

    /// Walk all logical rows of a `<table>` and compute the maximum
    /// min_content_width_hint (and explicit HTML width attribute) per logical
    /// column index. The vector returned has one entry per column.
    ///
    /// This runs as a pre-pass before any cell sizing, so that `table_cell_auto_width`
    /// in row 1 already knows that some later row has substantial content in
    /// a column that row 1 leaves as a spacer.
    ///
    /// Handles `<tbody>`/`<thead>`/`<tfoot>` row groups transparently via
    /// `collect_logical_rows_static`. Logical column indices track rowspan
    /// occupancy across rows.
    ///
    /// For `colspan > 1` cells, the cell hint is only distributed across the
    /// spanned columns where the existing sum is below the hint, ensuring no
    /// over-allocation. Spec: CSS 2.2 §17.5.2.1.
    pub fn compute_table_column_min_hints(table: &Rc<RefCell<LayoutObject>>) -> Vec<i64> {
        Self::compute_table_column_hints(table, false)
    }

    /// Per-logical-column maximum of each cell's preferred (max-content) width.
    /// Mirror of `compute_table_column_min_hints` using `max_content_width`.
    pub fn compute_table_column_max_hints(table: &Rc<RefCell<LayoutObject>>) -> Vec<i64> {
        Self::compute_table_column_hints(table, true)
    }

    /// Shared implementation of the per-column hint pre-pass. When `use_max` is
    /// false it accumulates `min_content_width_hint` (the column's minimum
    /// width); when true it accumulates `max_content_width` (the column's
    /// preferred width).
    fn compute_table_column_hints(table: &Rc<RefCell<LayoutObject>>, use_max: bool) -> Vec<i64> {
        let table_px_width = parse_dimension_attr(table.borrow().element_attribute("width"));
        let logical_rows = Self::collect_logical_rows_static(table);
        let mut col_min: Vec<i64> = Vec::new();
        // `occupied[i]` is the remaining number of rows that column i is still
        // taken by a rowspan cell from an earlier row.
        let mut occupied: Vec<usize> = Vec::new();
        // Deferred colspan cells: (start_col, end_col_exclusive, hint).
        let mut colspan_cells: Vec<(usize, usize, i64)> = Vec::new();

        for row in &logical_rows {
            let row_b = row.borrow();
            let mut logical_col: usize = 0;
            let mut cell = row_b.first_child();
            while let Some(c) = cell {
                let cb = c.borrow();
                if cb.is_table_cell() {
                    // Skip columns currently occupied by a rowspan cell.
                    while logical_col < occupied.len() && occupied[logical_col] > 0 {
                        logical_col += 1;
                    }
                    let colspan = cb.cell_colspan();
                    let rowspan = parse_dimension_attr(cb.element_attribute("rowspan"))
                        .unwrap_or(1)
                        .max(1) as usize;
                    let end_col = logical_col + colspan;
                    // Ensure vectors have room.
                    if col_min.len() < end_col {
                        col_min.resize(end_col, 0);
                    }
                    if occupied.len() < end_col {
                        occupied.resize(end_col, 0);
                    }
                    // Compute this cell's hint, considering explicit width attr.
                    let attr_w = parse_dimension_pct_attr(
                        cb.element_attribute("width"),
                        table_px_width,
                    )
                    .unwrap_or(0);
                    let content_hint = if use_max {
                        cb.max_content_width()
                    } else {
                        cb.min_content_width_hint()
                    };
                    let cell_hint = attr_w.max(content_hint);
                    if colspan == 1 {
                        col_min[logical_col] = col_min[logical_col].max(cell_hint);
                    } else {
                        colspan_cells.push((logical_col, end_col, cell_hint));
                    }
                    // Mark rowspan occupancy for subsequent rows.
                    if rowspan > 1 {
                        for i in logical_col..end_col {
                            occupied[i] = occupied[i].max(rowspan);
                        }
                    }
                    logical_col = end_col;
                }
                let next = cb.next_sibling();
                drop(cb);
                cell = next;
            }
            // Tick down occupancy after each row.
            for o in occupied.iter_mut() {
                if *o > 0 {
                    *o -= 1;
                }
            }
        }

        // Second sweep: distribute colspan cell hints across spanned columns
        // only where the existing single-cell sum is below the cell hint.
        for (start, end, hint) in colspan_cells {
            if start >= col_min.len() {
                continue;
            }
            let end = end.min(col_min.len());
            if end <= start {
                continue;
            }
            let current_sum: i64 = col_min[start..end].iter().sum();
            if hint > current_sum {
                let deficit = hint - current_sum;
                let span = (end - start) as i64;
                let share = deficit / span;
                let remainder = deficit - share * span;
                for (i, slot) in col_min[start..end].iter_mut().enumerate() {
                    let add = share + if (i as i64) < remainder { 1 } else { 0 };
                    *slot += add;
                }
            }
        }

        col_min
    }

    /// Return this cell's colspan attribute value (defaults to 1).
    pub fn cell_colspan(&self) -> usize {
        parse_dimension_attr(self.element_attribute("colspan"))
            .unwrap_or(1)
            .max(1) as usize
    }

    /// From a table cell, walk up to the containing `<table>` and return its
    /// per-column min content hints (populated by the table pre-pass), if any.
    fn ancestor_table_column_min_hints(&self) -> Option<Vec<i64>> {
        let row = self.parent.upgrade()?;
        let row_parent = row.borrow().parent.upgrade()?;
        let table = if row_parent.borrow().is_row_group() {
            row_parent.borrow().parent.upgrade()?
        } else {
            row_parent
        };
        let t = table.borrow();
        t.column_min_hints().map(|s| s.to_vec())
    }

    /// From a table cell, walk up to the containing `<table>` and return its
    /// per-column preferred (max-content) hints, if any.
    fn ancestor_table_column_max_hints(&self) -> Option<Vec<i64>> {
        let row = self.parent.upgrade()?;
        let row_parent = row.borrow().parent.upgrade()?;
        let table = if row_parent.borrow().is_row_group() {
            row_parent.borrow().parent.upgrade()?
        } else {
            row_parent
        };
        let t = table.borrow();
        t.column_max_hints().map(|s| s.to_vec())
    }

    /// Compute the width this cell should use, accounting for sibling cells
    /// that have explicit HTML width attributes, and rowspan cells from
    /// previous rows that reduce the available width.
    /// Auto-width cells with large intrinsic content (images, nested tables)
    /// receive at least their minimum content width before equal distribution.
    /// Also checks sibling rows for column width hints (including colspan).
    fn table_cell_auto_width(&self, available_width: i64) -> i64 {
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return available_width,
        };
        // Reduce available width by rowspan columns from previous rows.
        let rowspan_offset = parent.borrow().rowspan_column_offset();

        // Per-column max content hints from the table-level pre-pass.  Used to
        // guarantee that a column with substantial content in some other row
        // is not starved by row 1's narrow cell. Spec: CSS 2.2 §17.5.2.
        //
        // IMPORTANT: column_min_hints is indexed by LOGICAL column.  Inside
        // this function we iterate cells with a PHYSICAL counter (`col_idx`).
        // The two coincide only when there are no rowspan offsets from
        // previous rows and no colspan cells in the current row.  Otherwise
        // the indices diverge and using column_min_hints[col_idx] would
        // promote the wrong column.  We compute that safety condition once
        // and gate every column-hint lookup on it.
        let column_min_hints_raw = self.ancestor_table_column_min_hints();
        let row_has_colspan = {
            let mut has = self.cell_colspan() > 1;
            let mut child = parent.borrow().first_child();
            while let Some(c) = child {
                // `self` is already mutably borrowed; skip it with try_borrow.
                match c.try_borrow() {
                    Ok(cb) => {
                        if cb.is_table_cell() && cb.cell_colspan() > 1 {
                            has = true;
                        }
                        let next = cb.next_sibling();
                        drop(cb);
                        child = next;
                    }
                    Err(_) => {
                        // This is self; we already accounted for our own colspan above.
                        let next = unsafe { (*c.as_ptr()).next_sibling() };
                        child = next;
                    }
                }
                if has { break; }
            }
            has
        };
        let column_min_hints = if rowspan_offset == 0 && !row_has_colspan {
            column_min_hints_raw
        } else {
            None
        };
        // Column-level preferred widths, gated identically to the min hints.
        let column_max_hints = if rowspan_offset == 0 && !row_has_colspan {
            self.ancestor_table_column_max_hints()
        } else {
            None
        };

        // Check if sibling rows have explicit widths for the columns this cell
        // spans.  Skip this check when:
        //  - rowspan columns shift cell indices (raw cell_column_index is unreliable), or
        //  - self has rowspan > 1: sibling rows don't have a real cell at this
        //    cell's column — the slot is occupied by the rowspan cell itself, so
        //    sibling-row lookups return data from the WRONG column.
        let self_rowspan = parse_dimension_attr(self.element_attribute("rowspan"))
            .unwrap_or(1)
            .max(1);
        if rowspan_offset == 0 && self_rowspan <= 1 {
            let col_index = self.cell_column_index();
            let colspan = self.cell_colspan();
            let mut total_from_siblings: i64 = 0;
            let mut found_all = true;
            for ci in col_index..(col_index + colspan) {
                if let Some(w) = self.column_width_from_sibling_rows(ci) {
                    total_from_siblings += w;
                } else {
                    found_all = false;
                }
            }
            // If the sibling-row widths are large enough to cover the per-column
            // min hints for the spanned columns, trust them. Otherwise fall
            // through to general distribution so this row can claim adequate
            // space when a later row's content widens the column.
            if found_all && total_from_siblings > 0 {
                let column_min_total: i64 = column_min_hints
                    .as_ref()
                    .map(|h| {
                        (col_index..col_index + colspan)
                            .filter_map(|i| h.get(i).copied())
                            .sum::<i64>()
                    })
                    .unwrap_or(0);
                if total_from_siblings >= column_min_total {
                    return total_from_siblings.min(available_width);
                }
            }
        }

        let effective_width = (available_width - rowspan_offset).max(0);

        // Pre-compute the column index of this cell so we can identify our own slot.
        let my_col_index = if rowspan_offset == 0 { self.cell_column_index() } else { usize::MAX };

        let mut total_explicit: i64 = 0;
        // (is_self, min_hint, max_content) — max_content is the cell's preferred
        // (longest-line) width, used to cap how much surplus a column absorbs so
        // a narrow label column (e.g. a rank number) does not balloon to share
        // surplus equally with a genuinely flexible text column.
        let mut auto_cells: Vec<(bool, i64, i64)> = Vec::new();
        // Sum of every auto cell's horizontal padding+border. The widths
        // returned here are CONTENT widths and compute_size adds each cell's
        // metrics.inner_horizontal() on top; without subtracting that overhead
        // from the budget the row's cell boxes overflow the table's right edge
        // by (cell count × padding).
        let mut auto_box_overhead: i64 = 0;
        let mut self_index: usize = 0;
        let mut col_idx: usize = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            match c.try_borrow() {
                Ok(borrowed) => {
                    if borrowed.is_table_cell() {
                        // Percentage widths are resolved against effective_width here.
                        if let Some(w) = parse_dimension_pct_attr(
                            borrowed.element_attribute("width"),
                            Some(effective_width),
                        ) {
                            total_explicit += w;
                        } else if let Some(w) = (col_idx != my_col_index
                            // Skip the sibling-row lookup for cells with rowspan > 1.
                            // Such cells span into sibling rows where the column index
                            // corresponds to a different cell (shifted by the rowspan
                            // occupying that slot), so the lookup returns wrong widths.
                            && parse_dimension_attr(borrowed.element_attribute("rowspan"))
                                .unwrap_or(1)
                                .max(1)
                                <= 1
                            // Skip sibling-row lookup when this row has a rowspan
                            // offset: the physical col_idx in this row does not match
                            // the logical column in sibling rows (they are shifted by
                            // the number of rowspan-occupied columns), so the lookup
                            // would return the width of the wrong column.
                            && rowspan_offset == 0)
                            .then(|| borrowed.column_width_from_sibling_rows_pub(col_idx))
                            .flatten()
                        {
                            // This sibling has no explicit width in this row but will
                            // inherit one from sibling rows; treat it as "explicit" so
                            // the remaining space is divided correctly.
                            total_explicit += w;
                        } else {
                            let cell_hint = borrowed.min_content_width_hint();
                            // Promote to the column-level min hint if larger:
                            // ensures this cell reserves space for any sibling
                            // row in the same column whose content is wider.
                            let col_hint = column_min_hints
                                .as_ref()
                                .and_then(|h| h.get(col_idx).copied())
                                .unwrap_or(0);
                            let hint = cell_hint.max(col_hint);
                            // Promote to the column-level preferred width so a
                            // spacer cell in this row still reflects how wide a
                            // sibling row's content in the same column wants to be.
                            let col_max = column_max_hints
                                .as_ref()
                                .and_then(|h| h.get(col_idx).copied())
                                .unwrap_or(0);
                            let max_content =
                                borrowed.max_content_width().max(col_max).max(hint);
                            auto_cells.push((false, hint, max_content));
                            auto_box_overhead +=
                                compute_box_model_metrics(&borrowed.style).inner_horizontal();
                        }
                        // Advance by colspan so col_idx stays a LOGICAL column
                        // index, consistent with cell_column_index() and the
                        // hint vectors.
                        col_idx += borrowed.cell_colspan();
                    }
                    let next = borrowed.next_sibling();
                    drop(borrowed);
                    child = next;
                }
                Err(_) => {
                    // This is self — it has no explicit width (caller already checked).
                    let cell_hint = self.min_content_width_hint();
                    let col_hint = column_min_hints
                        .as_ref()
                        .and_then(|h| h.get(col_idx).copied())
                        .unwrap_or(0);
                    let hint = cell_hint.max(col_hint);
                    let col_max = column_max_hints
                        .as_ref()
                        .and_then(|h| h.get(col_idx).copied())
                        .unwrap_or(0);
                    let max_content = self.max_content_width().max(col_max).max(hint);
                    self_index = auto_cells.len();
                    auto_cells.push((true, hint, max_content));
                    auto_box_overhead +=
                        compute_box_model_metrics(&self.style).inner_horizontal();
                    col_idx += self.cell_colspan();
                    let next = c.as_ptr();
                    child = unsafe { (*next).next_sibling() };
                }
            }
        }

        let remaining = (effective_width - total_explicit - auto_box_overhead).max(0);
        let auto_count = auto_cells.len();
        if auto_count == 0 {
            return effective_width;
        }

        let equal_share = remaining / auto_count as i64;
        let total_min: i64 = auto_cells.iter().map(|(_, h, _)| *h).sum();
        if total_min <= remaining && total_min > 0 {
            let surplus = remaining - total_min;
            let (_, my_min, my_max) = auto_cells[self_index];

            // Every auto cell is guaranteed its min hint; the surplus is then
            // distributed in proportion to each cell's growth headroom
            // (max_content - min). Cells whose content cannot use more space —
            // images, &nbsp; spacers, a rank number like "30." — have zero
            // headroom and absorb nothing, while a text column that merely has
            // a long unbreakable word (large min) still grows toward its
            // preferred width. Spec: CSS 2.2 §17.5.2.2 (distribute excess in
            // proportion to the difference between a column's maximum and
            // minimum content widths).
            let my_headroom = (my_max - my_min).max(0);
            let total_headroom: i64 = auto_cells
                .iter()
                .map(|(_, h, mx)| (*mx - *h).max(0))
                .sum();
            if total_headroom > 0 {
                my_min.max(1) + surplus * my_headroom / total_headroom
            } else {
                // No cell can grow: distribute surplus proportionally to mins
                // so the row still fills the table width consistently.
                const DEFAULT_MIN: i64 = 16;
                let total_effective: i64 = auto_cells
                    .iter()
                    .map(|(_, h, _)| (*h).max(DEFAULT_MIN))
                    .sum();
                let my_effective = my_min.max(DEFAULT_MIN);
                let bonus = if total_effective > 0 {
                    surplus * my_effective / total_effective
                } else {
                    0
                };
                my_min.max(1) + bonus
            }
        } else {
            // Simple equal division (no mins, or mins exceed available).
            equal_share
        }
    }

    /// Sum of all explicit HTML width attributes among sibling table cells.
    fn total_sibling_explicit_widths(&self) -> i64 {
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return 0,
        };
        let mut total: i64 = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            match c.try_borrow() {
                Ok(borrowed) => {
                    if borrowed.is_table_cell() {
                        if let Some(w) = parse_dimension_attr(borrowed.element_attribute("width")) {
                            total += w;
                        }
                    }
                    let next = borrowed.next_sibling();
                    drop(borrowed);
                    child = next;
                }
                Err(_) => {
                    // This is self — check our own width attribute.
                    if let Some(w) = parse_dimension_attr(self.element_attribute("width")) {
                        total += w;
                    }
                    let next = c.as_ptr();
                    child = unsafe { (*next).next_sibling() };
                }
            }
        }
        total
    }

    /// Collapse whitespace in a text node per CSS Text §4.1 (simplified):
    /// newlines become spaces and runs of spaces collapse to one. A leading or
    /// trailing space survives only when this text node has an adjacent INLINE
    /// sibling on that side — it is then a word separator between inline boxes
    /// (e.g. `<span>197 points</span> by <a>user</a>`). At block edges the
    /// space is removed, as it would be at a line start/end.
    /// https://www.w3.org/TR/css-text-3/#white-space-phase-2
    fn collapse_text_whitespace(&self, t: &str) -> String {
        let collapsed = t
            .replace('\n', " ")
            .split(' ')
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
        let had_leading = t.starts_with([' ', '\n']);
        let had_trailing = t.ends_with([' ', '\n']);
        if !had_leading && !had_trailing {
            return collapsed;
        }
        // Find this node among its siblings by pointer identity (self may be
        // inside an active borrow during compute_size, so try_borrow-based
        // detection is unreliable here).
        let mut prev_inline = false;
        let mut next_inline = false;
        if let Some(parent) = self.parent.upgrade() {
            let mut prev_kind: Option<LayoutObjectKind> = None;
            let mut child = parent.borrow().first_child();
            while let Some(c) = child {
                if std::ptr::eq(c.as_ptr() as *const LayoutObject, self as *const LayoutObject) {
                    prev_inline = matches!(
                        prev_kind,
                        Some(LayoutObjectKind::Inline) | Some(LayoutObjectKind::Text)
                    );
                    next_inline = self
                        .next_sibling()
                        .map(|n| {
                            matches!(
                                n.borrow().kind(),
                                LayoutObjectKind::Inline | LayoutObjectKind::Text
                            )
                        })
                        .unwrap_or(false);
                    break;
                }
                let b = c.borrow();
                prev_kind = Some(b.kind());
                let next = b.next_sibling();
                drop(b);
                child = next;
            }
        }
        if collapsed.is_empty() {
            // Whitespace-only node: a single separator space when it sits
            // between two inline boxes, nothing otherwise.
            return if prev_inline && next_inline {
                String::from(" ")
            } else {
                String::new()
            };
        }
        let mut result = String::new();
        if had_leading && prev_inline {
            result.push(' ');
        }
        result.push_str(&collapsed);
        if had_trailing && next_inline {
            result.push(' ');
        }
        result
    }

    pub fn element_attribute(&self, name: &str) -> Option<String> {
        match self.node.borrow().kind() {
            NodeKind::Element(ref element) => element.get_attribute(name),
            _ => None,
        }
    }

    /// Returns true if every child layout object of this cell is Inline or Text
    /// (i.e. no block-level children).  Used to detect bullet-marker cells that
    /// should be vertically centred inside the row.
    fn has_only_inline_children(&self) -> bool {
        let mut child = self.first_child();
        let mut found_any = false;
        while let Some(c) = child {
            let b = c.borrow();
            match b.kind() {
                LayoutObjectKind::Block => return false,
                _ => {}
            }
            found_any = true;
            let next = b.next_sibling();
            drop(b);
            child = next;
        }
        found_any
    }

    /// Compute the x-offset for cells in this row due to rowspan cells from
    /// previous sibling rows.  Returns the total width that is "occupied" by
    /// spanning cells, so the first real cell in this row should start at
    /// parent_x + offset.
    fn rowspan_column_offset(&self) -> i64 {
        if !self.is_table_row() {
            return 0;
        }
        // Walk backwards through previous sibling rows.
        let parent = match self.parent.upgrade() {
            Some(p) => p,
            None => return 0,
        };
        // Determine our row index among siblings.
        let mut row_index: usize = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            let is_self = Rc::ptr_eq(&self.node, &c.borrow().node);
            if is_self {
                break;
            }
            if c.borrow().is_table_row() {
                row_index += 1;
            }
            let next = c.borrow().next_sibling();
            child = next;
        }
        if row_index == 0 {
            return 0;
        }
        // Now scan previous rows for cells with rowspan > 1 that extend into us.
        let mut offset: i64 = 0;
        let mut n_rowspan_cols: i64 = 0;
        let mut prev_row_index: usize = 0;
        let mut child = parent.borrow().first_child();
        while let Some(c) = child {
            if c.borrow().is_table_row() {
                if prev_row_index >= row_index {
                    break;
                }
                // Scan cells in this row.
                let mut cell = c.borrow().first_child();
                while let Some(cell_rc) = cell {
                    let cell_borrowed = cell_rc.borrow();
                    if cell_borrowed.is_table_cell() {
                        let rowspan = parse_dimension_attr(cell_borrowed.element_attribute("rowspan"))
                            .unwrap_or(1);
                        if rowspan > 1 && prev_row_index + rowspan as usize > row_index {
                            offset += cell_borrowed.size.width();
                            n_rowspan_cols += 1;
                        }
                    }
                    let next = cell_borrowed.next_sibling();
                    drop(cell_borrowed);
                    cell = next;
                }
                prev_row_index += 1;
            }
            let next = c.borrow().next_sibling();
            child = next;
        }
        // Include the cellspacing gap that precedes each rowspan column so that
        // the first cell placed after them lands at the same X as its counterpart
        // in the rowspan row (which was also offset by cs per preceding cell).
        let cs = self.ancestor_table_cellspacing();
        offset + n_rowspan_cols * cs
    }

    fn placeholder_text(&self) -> Option<String> {
        match self.element_kind()? {
            ElementKind::Img => Some(
                self.element_attribute("alt")
                    .filter(|alt| !alt.trim().is_empty())
                    .unwrap_or_else(|| {
                        self.element_attribute("src")
                            .map(|src| format!("Image: {src}"))
                            .unwrap_or_else(|| "Image".to_string())
                    }),
            ),
            ElementKind::Input => Some(
                self.element_attribute("value")
                    .or_else(|| self.element_attribute("placeholder"))
                    .unwrap_or_else(|| "Input".to_string()),
            ),
            _ => None,
        }
    }

    fn intrinsic_inline_size(&self, parent_size: LayoutSize) -> Option<LayoutSize> {
        let explicit_width = self.resolved_width(parent_size);
        let explicit_height = self.resolved_height(parent_size);
        let width_attr = parse_dimension_attr(self.element_attribute("width"));
        let height_attr = parse_dimension_attr(self.element_attribute("height"));

        match self.element_kind()? {
            ElementKind::Img => Some(LayoutSize::new(
                explicit_width.max(width_attr.unwrap_or(220)),
                explicit_height.max(height_attr.unwrap_or(140)),
            )),
            ElementKind::Input => Some(LayoutSize::new(
                explicit_width.max(width_attr.unwrap_or(220)),
                explicit_height.max(height_attr.unwrap_or(36)),
            )),
            ElementKind::Button => {
                let child_text = self
                    .first_child()
                    .and_then(|child| match child.borrow().node_kind() {
                        NodeKind::Text(ref text) => Some(text.clone()),
                        _ => None,
                    })
                    .unwrap_or_else(|| "Button".to_string());
                Some(LayoutSize::new(
                    explicit_width
                        .max(measure_text_width(&child_text, self.style.font_size(), self.style.is_bold()) + 28),
                    explicit_height.max(36),
                ))
            }
            _ => None,
        }
    }

    fn resolved_width(&self, parent_size: LayoutSize) -> i64 {
        if let Some(ratio) = self.style.width_ratio() {
            return edge_to_i64(parent_size.width() as f64 * ratio);
        }
        edge_to_i64(self.style.width())
    }

    fn resolved_height(&self, parent_size: LayoutSize) -> i64 {
        if let Some(ratio) = self.style.height_ratio() {
            return edge_to_i64(parent_size.height() as f64 * ratio);
        }
        edge_to_i64(self.style.height())
    }

    /// Stacking-context level for paint ordering: 1 when this box is
    /// positioned OR belongs to a sticky subtree (the sticky context is
    /// stamped onto every descendant, whose own position is Static — without
    /// this, a pinned bar's background would paint over its own text).
    fn stacking_context_level(&self) -> i32 {
        if self.style.position() != PositionType::Static
            || self.style.sticky_context().is_some()
            || self.style.fixed_subtree()
        {
            1
        } else {
            0
        }
    }

    pub fn paint(&mut self) -> Vec<DisplayItem> {
        if self.style.display() == DisplayType::DisplayNone {
            return vec![];
        }

        match self.kind {
            LayoutObjectKind::Block => {
                if let NodeKind::Element(_) = self.node_kind() {
                    if self.size.width() > 0 && self.size.height() > 0 {
                        // <caption> children render outside (above) the table border box.
                        // Offset the DrawRect so the border starts below the caption area.
                        // CSS 2.2 §17.4: caption-side:top means the caption is placed
                        // above the table's border/padding/cell area.
                        let caption_h: i64 = if self.is_table() {
                            let mut total = 0i64;
                            let mut child = self.first_child();
                            while let Some(c) = child {
                                if c.borrow().element_kind() == Some(ElementKind::Caption) {
                                    let cb = c.borrow();
                                    let cm = compute_box_model_metrics(&cb.style());
                                    total += cb.size().height()
                                        + cm.margin.top
                                        + cm.margin.bottom;
                                } else if c.borrow().is_table_row() || c.borrow().is_row_group() {
                                    break;
                                }
                                let next = c.borrow().next_sibling();
                                child = next;
                            }
                            total
                        } else {
                            0
                        };
                        let rect_y = self.point().y() + caption_h;
                        let rect_h = self.size().height() - caption_h;
                        if rect_h <= 0 {
                            return vec![];
                        }
                        // Capture the element's `id` attribute so the adapter
                        // can resolve URL fragment anchors to a scroll offset.
                        // Spec: HTML Living Standard §7.4 — navigating to a
                        // fragment identifier within a document.
                        // https://html.spec.whatwg.org/multipage/browsing-the-web.html#scroll-to-fragid
                        let anchor_id = self.element_attribute("id");
                        return vec![DisplayItem::Rect {
                            style: self.style(),
                            layout_point: LayoutPoint::new(self.point().x(), rect_y),
                            layout_size: LayoutSize::new(self.size().width(), rect_h),
                            paint_order: PaintOrder {
                                stacking_context: self.stacking_context_level(),
                                z_index: self.style.z_index_or_default(),
                            },
                            clip_rect: self.style.final_clip().map(|(x, y, w, h)| ClipRect {
                                x: x as i64,
                                y: y as i64,
                                width: w as i64,
                                height: h as i64,
                            }),
                            anchor_id,
                        }];
                    }
                }
            }
            LayoutObjectKind::Inline => {
                if let NodeKind::Element(_) = self.node_kind() {
                    let mut items = Vec::new();
                    if self.size.width() > 0 && self.size.height() > 0 {
                        let anchor_id = self.element_attribute("id");
                        items.push(DisplayItem::Rect {
                            style: self.style(),
                            layout_point: self.point(),
                            layout_size: self.size(),
                            paint_order: PaintOrder {
                                stacking_context: self.stacking_context_level(),
                                z_index: self.style.z_index_or_default(),
                            },
                            clip_rect: self.style.final_clip().map(|(x, y, w, h)| ClipRect {
                                x: x as i64,
                                y: y as i64,
                                width: w as i64,
                                height: h as i64,
                            }),
                            anchor_id,
                        });
                    }

                    if self.element_kind() == Some(ElementKind::Img) {
                        let src = self.element_attribute("src").unwrap_or_default();
                        let alt = self.element_attribute("alt").unwrap_or_default();
                        items.push(DisplayItem::Image {
                            src,
                            alt,
                            layout_point: self.point(),
                            layout_size: self.size(),
                            style: self.style(),
                            href: self.link_href(),
                            target: self.link_target(),
                            paint_order: PaintOrder {
                                stacking_context: self.stacking_context_level(),
                                z_index: self.style.z_index_or_default(),
                            },
                            clip_rect: self
                                .style
                                .final_clip()
                                .map(|(x, y, w, h)| ClipRect {
                                    x: x as i64,
                                    y: y as i64,
                                    width: w as i64,
                                    height: h as i64,
                                })
                                .or_else(|| {
                                    // Clip to ancestor cell so oversized images
                                    // don't overflow their cell boundary.
                                    self.nearest_ancestor_cell().map(|cell| {
                                        let cb = cell.borrow();
                                        ClipRect {
                                            x: cb.point().x(),
                                            y: cb.point().y(),
                                            width: cb.size().width(),
                                            height: cb.size().height(),
                                        }
                                    })
                                }),
                        });
                    } else if let Some(text) = self.placeholder_text() {
                        items.push(DisplayItem::Text {
                            text,
                            style: self.style(),
                            layout_point: LayoutPoint::new(
                                self.point().x() + 10,
                                self.point().y() + 10,
                            ),
                            href: self.link_href(),
                            target: self.link_target(),
                            paint_order: PaintOrder {
                                stacking_context: self.stacking_context_level(),
                                z_index: self.style.z_index_or_default(),
                            },
                            clip_rect: self.style.final_clip().map(|(x, y, w, h)| ClipRect {
                                x: x as i64,
                                y: y as i64,
                                width: w as i64,
                                height: h as i64,
                            }),
                            bold: self.style.is_bold(),
                        });
                    }

                    if !items.is_empty() {
                        return items;
                    }
                }
            }
            LayoutObjectKind::Text => {
                if let NodeKind::Text(t) = self.node_kind() {
                    let mut v = vec![];
                    let cw =
                        bold_width_adjust(char_width_px(self.style.font_size()), self.style.is_bold());
                    let lh = styled_line_height(&self.style);
                    let plain_text = self.collapse_text_whitespace(&t);
                    // Use the max_width that was established during compute_size so
                    // that the line-break boundaries are identical between the sizing
                    // and painting passes.  Recomputing against self.size().width()
                    // here would produce narrower wrapping because size.width() is
                    // the width of the widest *result* line, not the available
                    // container width.
                    // Spec: CSS2.2 §9.4.2 — inline formatting context, line boxes.
                    // https://www.w3.org/TR/CSS22/visuren.html#inline-formatting
                    // Prefer the ancestor cell's current content width so that
                    // text wrapping reflects the post-equalization cell width
                    // rather than the stale cached value from compute_size.
                    let max_width = self.nearest_ancestor_cell()
                        .map(|cell| {
                            let cb = cell.borrow();
                            let cm = compute_box_model_metrics(&cb.style);
                            (cb.size().width() - cm.inner_horizontal()).max(cw)
                        })
                        .unwrap_or_else(|| {
                            if self.text_line_max_width > 0 {
                                self.text_line_max_width
                            } else {
                                self.size().width().max(cw)
                            }
                        });
                    let mut lines = if self.style.white_space_nowrap() {
                        // nowrap: a single line (the collapser already turned
                        // newlines into spaces). text-overflow:ellipsis on a
                        // clipping ancestor then truncates it to fit.
                        let mut line = plain_text;
                        if let Some(clip_w) = self.ellipsis_clip_width() {
                            line = truncate_with_ellipsis(&line, cw, clip_w);
                        }
                        vec![line]
                    } else {
                        split_text(plain_text, cw, max_width)
                    };
                    let _ = &mut lines;
                    let href = self.link_href();
                    let target = self.link_target();

                    let bold = self.style.is_bold();
                    for (i, line) in lines.into_iter().enumerate() {
                        let item = DisplayItem::Text {
                            text: line,
                            style: self.style(),
                            layout_point: LayoutPoint::new(
                                self.point().x(),
                                self.point().y() + lh * i as i64,
                            ),
                            href: href.clone(),
                            target: target.clone(),
                            paint_order: PaintOrder {
                                stacking_context: self.stacking_context_level(),
                                z_index: self.style.z_index_or_default(),
                            },
                            clip_rect: self.style.final_clip().map(|(x, y, w, h)| ClipRect {
                                x: x as i64,
                                y: y as i64,
                                width: w as i64,
                                height: h as i64,
                            }),
                            bold,
                        };
                        v.push(item);
                    }

                    return v;
                }
            }
        }

        vec![]
    }

    pub fn compute_size(&mut self, parent_size: LayoutSize) {
        let mut size = LayoutSize::new(0, 0);
        let metrics = compute_box_model_metrics(&self.style);

        match self.kind() {
            LayoutObjectKind::Block => {
                let available_width = (parent_size.width() - metrics.outer_horizontal()).max(0);
                let explicit_width = self.resolved_width(parent_size);
                // Also check HTML width attribute for block elements (tables, etc.).
                // Percentages resolve against the available (containing) width so
                // `<table width="85%">` and nested `width="100%"` tables expand to
                // a real width instead of collapsing via shrink-to-fit (which on
                // nested auto tables starves content columns to ~min-content).
                let html_width = parse_dimension_pct_attr(
                    self.element_attribute("width"),
                    Some(available_width),
                );

                // Table cells: use width attribute, or allocate remaining width
                // after subtracting explicitly-sized sibling cells.
                // When total explicit widths exceed available space, scale
                // proportionally to fit.
                let content_width = if self.is_table_cell() {
                    // Resolve HTML width attribute — percentage values are computed
                    // against available_width (the parent row's inner width).
                    let attr_width = parse_dimension_pct_attr(
                        self.element_attribute("width"),
                        Some(available_width),
                    );
                    if let Some(w) = attr_width {
                        let rowspan_offset = self
                            .parent
                            .upgrade()
                            .map(|p| p.borrow().rowspan_column_offset())
                            .unwrap_or(0);
                        let effective = (available_width - rowspan_offset).max(0);
                        let total_explicit = self.total_sibling_explicit_widths();
                        if total_explicit > effective && total_explicit > 0 {
                            // Scale down proportionally.
                            (w * effective / total_explicit).max(0)
                        } else {
                            w.min(effective)
                        }
                    } else if explicit_width > 0 {
                        explicit_width.min(available_width)
                    } else {
                        self.table_cell_auto_width(available_width)
                    }
                } else if let Some(FlexDirection::Row) = self.parent_flex_direction() {
                    // Flex item on a row: shrink-to-fit its content (honoring an
                    // explicit width) so siblings sit side by side instead of
                    // each filling the container width like a normal block.
                    let w = if explicit_width > 0 {
                        explicit_width
                    } else if let Some(hw) = html_width {
                        hw
                    } else {
                        self.max_content_width()
                    };
                    w.min(available_width).max(0)
                } else if let Some((tracks, col_gap, _)) = self.parent_grid_info() {
                    // Grid item: the width of its column track; the item's
                    // content box is the track minus its own margins.
                    let widths = resolve_grid_tracks(&tracks, parent_size.width(), col_gap);
                    let col = self.grid_item_index() % tracks.len().max(1);
                    let track =
                        (widths.get(col).copied().unwrap_or(0) - metrics.outer_horizontal())
                            .max(0);
                    if explicit_width > 0 {
                        explicit_width.min(track)
                    } else {
                        track
                    }
                } else if explicit_width > 0 {
                    // Inside an overflow container the explicit width stands
                    // even when wider than the box — that overflow is exactly
                    // what the container clips/scrolls. Elsewhere, clamp to
                    // the available width as an overflow guard.
                    let parent_clips = self
                        .parent_object()
                        .map(|p| p.borrow().style().overflow_clip())
                        .unwrap_or(false);
                    if parent_clips {
                        explicit_width
                    } else {
                        explicit_width.min(available_width)
                    }
                } else if let Some(w) = html_width {
                    w.min(available_width)
                } else if self.element_kind() == Some(ElementKind::Table) {
                    // Tables without explicit width use shrink-to-fit: the width
                    // of the widest row (sum of cell widths), capped at available.
                    let measure_row_width = |r: &Rc<RefCell<LayoutObject>>| -> i64 {
                        let mut row_width: i64 = 0;
                        let mut cell = r.borrow().first_child();
                        while let Some(c) = cell {
                            row_width += c.borrow().size.width();
                            let next = c.borrow().next_sibling();
                            cell = next;
                        }
                        row_width
                    };
                    let mut max_row_width: i64 = 0;
                    let mut child = self.first_child();
                    while let Some(c) = child {
                        if c.borrow().is_table_row() {
                            max_row_width = max_row_width.max(measure_row_width(&c));
                        } else if c.borrow().is_row_group() {
                            let mut row = c.borrow().first_child();
                            while let Some(r) = row {
                                if r.borrow().is_table_row() {
                                    max_row_width = max_row_width.max(measure_row_width(&r));
                                }
                                let next = r.borrow().next_sibling();
                                row = next;
                            }
                        }
                        let next = c.borrow().next_sibling();
                        child = next;
                    }
                    if max_row_width > 0 {
                        max_row_width.min(available_width)
                    } else {
                        available_width
                    }
                } else {
                    available_width
                };

                let mut content_height = 0;
                let mut child = self.first_child();
                let is_row = self.is_table_row();
                // Flex row: items are placed side by side, so the container's
                // height is the tallest item (max), not the sum.
                let is_flex_row = self.is_flex_container()
                    && self.style.flex_direction() == FlexDirection::Row;
                // Grid: children fill rows of N columns; the container's height
                // is the sum of each row's tallest item.
                let grid_cols = if self.style.display() == DisplayType::Grid {
                    Some(self.style.grid_columns())
                } else {
                    None
                };
                let grid_row_gap = self.style.row_gap();
                let mut grid_idx = 0usize;
                let mut grid_rows_done = 0usize;
                let mut grid_row_height = 0i64;
                let mut previous_child_kind = LayoutObjectKind::Block;
                while child.is_some() {
                    let c = child.expect("first child should exist");
                    let c_kind = c.borrow().kind();
                    if is_row {
                        // Table row: height = max of cell heights,
                        // but skip cells with rowspan > 1 (they span multiple rows).
                        let rowspan = parse_dimension_attr(
                            c.borrow().element_attribute("rowspan"),
                        )
                        .unwrap_or(1);
                        if rowspan <= 1 {
                            content_height = content_height.max(c.borrow().size.height());
                        }
                    } else if is_flex_row {
                        let c_metrics = compute_box_model_metrics(&c.borrow().style());
                        content_height = content_height.max(
                            c.borrow().size.height()
                                + c_metrics.margin.top
                                + c_metrics.margin.bottom,
                        );
                    } else if let Some(n) = grid_cols {
                        // Whitespace-only text children are not grid items.
                        if !c.borrow().is_whitespace_text() {
                            let c_metrics = compute_box_model_metrics(&c.borrow().style());
                            grid_row_height = grid_row_height.max(
                                c.borrow().size.height()
                                    + c_metrics.margin.top
                                    + c_metrics.margin.bottom,
                            );
                            grid_idx += 1;
                            if grid_idx % n == 0 {
                                if grid_rows_done > 0 {
                                    content_height += grid_row_gap;
                                }
                                content_height += grid_row_height;
                                grid_rows_done += 1;
                                grid_row_height = 0;
                            }
                        }
                    } else if previous_child_kind.normal_flow_spec().stacks_vertically
                        || c_kind.normal_flow_spec().stacks_vertically
                    {
                        // Include the child's vertical margin so that the parent
                        // is tall enough to contain the child even when it has
                        // margin-top/bottom that shift it downward (e.g. <hr>,
                        // <h1>…<h3>, <p>).  This matches the CSS2 block height
                        // algorithm (§10.6.3) where margin participates in height.
                        let c_metrics = compute_box_model_metrics(&c.borrow().style());
                        content_height += c.borrow().size.height()
                            + c_metrics.margin.top
                            + c_metrics.margin.bottom;
                    } else {
                        content_height = content_height.max(c.borrow().size.height());
                    }
                    previous_child_kind = c_kind;
                    child = c.borrow().next_sibling();
                }
                // Final, possibly partial grid row.
                if grid_row_height > 0 {
                    if grid_rows_done > 0 {
                        content_height += grid_row_gap;
                    }
                    content_height += grid_row_height;
                }

                // <br> and <hr> have intrinsic heights even without children.
                let content_height = if self.element_kind() == Some(ElementKind::Br) {
                    styled_line_height(&self.style)
                } else if self.element_kind() == Some(ElementKind::Hr) {
                    // <hr> renders as a 2px line with 8px margin above/below.
                    2
                } else {
                    let explicit_height = self.resolved_height(parent_size);
                    if explicit_height > 0 {
                        explicit_height
                    } else {
                        content_height.max(0)
                    }
                };
                // For table cells, add cellpadding to both dimensions so that the
                // cell's outer box includes the padding space on all four sides.
                // content_origin() and content_size() subtract/add cp so that
                // children are inset by cp pixels inside the cell.
                let cp = if self.is_table_cell() { self.ancestor_table_cellpadding() } else { 0 };
                size.set_width((content_width + metrics.inner_horizontal()).max(0));
                size.set_height((content_height + metrics.inner_vertical() + 2 * cp).max(0));
            }
            LayoutObjectKind::Inline => {
                if let Some(intrinsic) = self.intrinsic_inline_size(parent_size) {
                    size.set_width((intrinsic.width() + metrics.inner_horizontal()).max(0));
                    size.set_height((intrinsic.height() + metrics.inner_vertical()).max(0));
                } else {
                    // Track per-line width separately and use the maximum across
                    // all lines as the inline element's box width.  Summing widths
                    // across block children (like <br>) produced an inflated width
                    // that caused text-align centering to shift multi-line text
                    // (e.g. <strong>line1<br>line2</strong>) off-screen.
                    let mut max_line_width: i64 = 0;
                    let mut current_line_width: i64 = 0;
                    let mut current_line_height: i64 = 0;
                    let mut content_height: i64 = 0;
                    let mut child = self.first_child();
                    while child.is_some() {
                        let c = child.expect("child should exist");
                        let c_kind = c.borrow().kind();
                        let c_h = c.borrow().size.height();
                        let c_w = c.borrow().size.width();
                        if c_kind.normal_flow_spec().stacks_vertically {
                            // Block child (e.g. <br>): flush the current inline
                            // line, then add the block child's own height.
                            // compute_position places the block immediately
                            // BELOW the preceding inline content (next sibling
                            // y = prev.y + prev.height), so the parent inline's
                            // content box must contain BOTH the line and the
                            // block — sum, don't max.
                            max_line_width = max_line_width.max(current_line_width);
                            current_line_width = 0;
                            content_height += current_line_height + c_h;
                            current_line_height = 0;
                            // The block child forms its own line: its outer
                            // width (incl. margins) is part of this inline's
                            // width. Without it, an <a> whose only child is a
                            // sized block (HN's votearrow div) measures 0 wide
                            // and centering math pushes the child out of the
                            // cell.
                            let c_metrics = compute_box_model_metrics(&c.borrow().style());
                            max_line_width = max_line_width
                                .max(c_w + c_metrics.margin.left + c_metrics.margin.right);
                        } else {
                            current_line_width += c_w;
                            current_line_height = current_line_height.max(c_h);
                        }
                        child = c.borrow().next_sibling();
                    }
                    // Flush any remaining inline content on the last line.
                    max_line_width = max_line_width.max(current_line_width);
                    content_height += current_line_height;


                    // Explicit CSS width/height take precedence (e.g. a 16×16
                    // sprite icon span has no children, so the content-derived
                    // size would be 0×0 and nothing would paint).
                    let explicit_w = self.resolved_width(parent_size);
                    let explicit_h = self.resolved_height(parent_size);
                    let content_w = if explicit_w > 0 {
                        explicit_w
                    } else if self.style.display() == DisplayType::InlineBlock {
                        // inline-block shrink-wraps: preferred (max-content)
                        // width capped by the containing block.
                        // https://www.w3.org/TR/CSS22/visudet.html#shrink-to-fit-float
                        self.max_content_width()
                            .max(max_line_width)
                            .min(parent_size.width().max(0))
                    } else {
                        max_line_width
                    };
                    let content_h = if explicit_h > 0 {
                        explicit_h
                    } else {
                        content_height
                    };
                    size.set_width((content_w + metrics.inner_horizontal()).max(0));
                    size.set_height((content_h + metrics.inner_vertical()).max(0));
                }
            }
            LayoutObjectKind::Text => {
                if let NodeKind::Text(t) = self.node_kind() {
                    let cw =
                        bold_width_adjust(char_width_px(self.style.font_size()), self.style.is_bold());
                    let lh = styled_line_height(&self.style);
                    let plain_text = self.collapse_text_whitespace(&t);
                    // max_width is the available horizontal space for this text
                    // node within its containing block.  Use the nearest block/cell
                    // ancestor's content width so that inline parents (e.g. <a>,
                    // <strong>) don't artificially narrow the wrapping boundary.
                    // Spec: CSS2.2 §10.3.3 — available width in a block
                    // formatting context.
                    // https://www.w3.org/TR/CSS22/visudet.html#blockwidth
                    let max_width = self.nearest_block_ancestor_width()
                        .map(|w| (w - metrics.outer_horizontal()).max(cw))
                        .unwrap_or_else(||
                            (parent_size.width() - metrics.outer_horizontal()).max(cw)
                        );
                    // Cache so paint() uses the identical boundary (see paint Text arm).
                    self.text_line_max_width = max_width;
                    let lines = if self.style.white_space_nowrap() {
                        vec![plain_text.clone()]
                    } else {
                        split_text(plain_text.clone(), cw, max_width)
                    };
                    let width = lines
                        .iter()
                        .map(|line| text_width_px(line, cw))
                        .max()
                        .unwrap_or(0);
                    let height = if lines.is_empty() {
                        0
                    } else {
                        lh * lines.len() as i64
                    };
                    size.set_width((width + metrics.inner_horizontal()).max(0));
                    size.set_height((height + metrics.inner_vertical()).max(0));
                }
            }
        }

        self.size = size;
    }

    pub fn compute_position(
        &mut self,
        parent_point: LayoutPoint,
        parent_size: LayoutSize,
        previous_sibling_kind: LayoutObjectKind,
        previous_sibling_point: Option<LayoutPoint>,
        previous_sibling_size: Option<LayoutSize>,
    ) {
        let mut point = LayoutPoint::new(0, 0);
        let metrics = compute_box_model_metrics(&self.style);

        // Table cells: position horizontally within the row.
        if self.is_table_cell() {
            // Vertical alignment within the row.
            // Honour explicit valign="middle"/"bottom"/"top"; fall back to
            // auto-centering for cells whose content is purely inline/text
            // (e.g. bullet-marker cells like ●), which matches browser behaviour
            // of baseline-aligning small inline cells within a taller row.
            let valign = self
                .element_attribute("valign")
                .map(|v| v.to_lowercase())
                .unwrap_or_default();
            let v_offset = if valign == "middle" || (valign.is_empty() && self.has_only_inline_children()) {
                (parent_size.height() - self.size.height()).max(0) / 2
            } else if valign == "bottom" {
                (parent_size.height() - self.size.height()).max(0)
            } else {
                0
            };

            // Horizontal cellspacing: add a gap between adjacent cells so that
            // their borders don't visually merge into a double line.
            // Spec: HTML4.01 §11.3.3 — cellspacing default is 2.
            let cs = self.ancestor_table_cellspacing();
            if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                point.set_x(pos.x() + size.width() + metrics.margin.left + cs);
                point.set_y(parent_point.y() + metrics.margin.top + v_offset);
            } else {
                // First cell in row: apply rowspan offset from previous rows.
                let rowspan_offset = self
                    .parent
                    .upgrade()
                    .map(|p| p.borrow().rowspan_column_offset())
                    .unwrap_or(0);
                point.set_x(parent_point.x() + rowspan_offset + metrics.margin.left + cs);
                point.set_y(parent_point.y() + metrics.margin.top + v_offset);
            }
        } else if let Some((tracks, col_gap, row_gap)) = self.parent_grid_info() {
            // Grid item placement: row-major into the column tracks.
            let n = tracks.len().max(1);
            let idx = self.grid_item_index();
            let col = idx % n;
            let widths = resolve_grid_tracks(&tracks, parent_size.width(), col_gap);
            let x_offset: i64 =
                widths[..col.min(widths.len())].iter().sum::<i64>() + col as i64 * col_gap;
            point.set_x(parent_point.x() + x_offset + metrics.margin.left);
            if col == 0 {
                // First track: a new grid row below the previous sibling (which
                // ended the previous row), separated by the row gap.
                if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                    point.set_y(
                        pos.y()
                            + size.height()
                            + metrics.margin.top
                            + metrics.margin.bottom
                            + row_gap,
                    );
                } else {
                    point.set_y(parent_point.y() + metrics.margin.top);
                }
            } else {
                // Same row: share the previous sibling's top edge.
                point.set_y(
                    previous_sibling_point
                        .map(|p| p.y())
                        .unwrap_or(parent_point.y() + metrics.margin.top),
                );
            }
        } else if let Some(dir) = self.parent_flex_direction() {
            // Flex item placement (main-axis start packing; no grow/shrink yet).
            match dir {
                FlexDirection::Row => {
                    // Lay out to the right of the previous item, aligned to the
                    // container's top (align-items: flex-start).
                    if let (Some(size), Some(pos)) =
                        (previous_sibling_size, previous_sibling_point)
                    {
                        point.set_x(pos.x() + size.width() + metrics.margin.left);
                    } else {
                        point.set_x(parent_point.x() + metrics.margin.left);
                    }
                    point.set_y(parent_point.y() + metrics.margin.top);
                }
                FlexDirection::Column => {
                    // Stack vertically like a block formatting context.
                    if let (Some(size), Some(pos)) =
                        (previous_sibling_size, previous_sibling_point)
                    {
                        point.set_y(
                            pos.y()
                                + size.height()
                                + metrics.margin.top
                                + metrics.margin.bottom,
                        );
                    } else {
                        point.set_y(parent_point.y() + metrics.margin.top);
                    }
                    point.set_x(parent_point.x() + metrics.margin.left);
                }
            }
        } else {
        match (
            self.kind().normal_flow_spec().flow,
            previous_sibling_kind.normal_flow_spec().flow,
        ) {
            (LayoutFlow::BlockFormattingContext, _) | (_, LayoutFlow::BlockFormattingContext) => {
                if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                    point.set_y(pos.y() + size.height() + metrics.margin.top + metrics.margin.bottom);
                } else {
                    point.set_y(parent_point.y() + metrics.margin.top);
                }

                // Ref: CSS 2.1 §10.3.3, auto horizontal margins center a block when width is known.
                // https://www.w3.org/TR/CSS21/visudet.html#blockwidth
                let available_width = parent_size.width() - self.size.width();
                if self.style.margin_horizontal_auto() && available_width > 0 {
                    point.set_x(parent_point.x() + available_width / 2);
                } else if self.style.margin_left_auto() && available_width > metrics.margin.right {
                    point.set_x(parent_point.x() + available_width - metrics.margin.right);
                } else if !self.kind().normal_flow_spec().stacks_vertically
                    && self.style.text_align() == TextAlign::Center
                    && available_width > 0
                {
                    // Inline/text node after a block: apply text-align centering.
                    point.set_x(parent_point.x() + available_width / 2);
                } else {
                    point.set_x(parent_point.x() + metrics.margin.left);
                }
            }
            (LayoutFlow::InlineFlow, LayoutFlow::InlineFlow) => {
                if let (Some(size), Some(pos)) = (previous_sibling_size, previous_sibling_point) {
                    // Candidate X if we place this element right after the previous one.
                    let candidate_x = pos.x() + size.width() + metrics.margin.left;
                    let right_edge = parent_point.x() + parent_size.width();
                    if parent_size.width() > 0 && candidate_x + self.size.width() > right_edge {
                        // This inline element does not fit on the current line;
                        // wrap it to the start of the next line.
                        // Spec: CSS2.2 §9.4.2 — when an inline box doesn't fit on
                        // the current line box it wraps to a new line box below.
                        // https://www.w3.org/TR/CSS22/visuren.html#inline-formatting
                        point.set_x(parent_point.x() + metrics.margin.left);
                        point.set_y(pos.y() + size.height() + metrics.margin.top);
                    } else {
                        point.set_x(candidate_x);
                        // Same line: top-align with the previous inline box.
                        // (pos.y() already sits below ITS margin-top; adding
                        // our own margin-top on top of that made every
                        // successive sibling creep downward.)
                        point.set_y(pos.y());
                    }
                } else {
                    // First inline child: apply text-align centering if set.
                    match self.style.text_align() {
                        TextAlign::Center => {
                            let available = parent_size.width() - self.size.width();
                            if available > 0 {
                                point.set_x(parent_point.x() + available / 2);
                            } else {
                                point.set_x(parent_point.x() + metrics.margin.left);
                            }
                        }
                        TextAlign::Right => {
                            let available = parent_size.width() - self.size.width();
                            if available > 0 {
                                point.set_x(parent_point.x() + available);
                            } else {
                                point.set_x(parent_point.x() + metrics.margin.left);
                            }
                        }
                        TextAlign::Left => {
                            point.set_x(parent_point.x() + metrics.margin.left);
                        }
                    }
                    point.set_y(parent_point.y() + metrics.margin.top);
                }
            }
        }
        } // end if !table_cell

        match self.style.position() {
            // Sticky flows normally; the painter pins it at scroll time.
            PositionType::Static | PositionType::Sticky => {}
            PositionType::Relative => {
                point.set_x(point.x() + edge_to_i64(self.style.offset_left()));
                point.set_y(point.y() + edge_to_i64(self.style.offset_top()));
            }
            PositionType::Absolute => {
                let dx = self
                    .style
                    .offset_left_ratio()
                    .map(|r| (parent_size.width() as f64 * r) as i64)
                    .unwrap_or_else(|| edge_to_i64(self.style.offset_left()));
                let dy = self
                    .style
                    .offset_top_ratio()
                    .map(|r| (parent_size.height() as f64 * r) as i64)
                    .unwrap_or_else(|| edge_to_i64(self.style.offset_top()));
                point.set_x(parent_point.x() + dx);
                point.set_y(parent_point.y() + dy);
            }
            // Fixed: anchored to the viewport origin; the painter additionally
            // exempts it from the scroll offset.
            PositionType::Fixed => {
                point.set_x(edge_to_i64(self.style.offset_left()));
                point.set_y(edge_to_i64(self.style.offset_top()));
            }
        }

        self.point = point;
    }

    pub fn is_node_selected(&self, selector: &Selector) -> bool {
        // Matching runs against the DOM tree, not the layout tree: during the
        // cascade this layout object is not yet linked into its parent's child
        // list, so sibling combinators (and, in principle, ancestors) must be
        // resolved through the fully-built DOM.
        dom_node_selected(&self.node, selector)
    }

    pub fn cascading_style(&mut self, declarations: Vec<Declaration>, parent_font_size: FontSize) {
        use crate::renderer::css::cssom::{substitute_vars, value_has_var};
        let custom_scope = self.style.custom_properties().cloned();
        for mut declaration in declarations {
            // Custom-property definitions are collected into the element's
            // scope before the cascade (create_layout_object); they are not
            // style properties themselves.
            if declaration.property.starts_with("--") {
                continue;
            }
            // var() references resolve against this element's custom-property
            // scope at computed-value time. CSS Variables §3.
            if value_has_var(&declaration.value) {
                if let Some(scope) = &custom_scope {
                    declaration.value = substitute_vars(&declaration.value, scope);
                }
            }
            let first_value = declaration.first_value();
            match declaration.property.as_str() {
                "background-color" | "background" => {
                    match first_value {
                        Some(ComponentValue::Ident(value)) => {
                            let color = Color::from_name(value).unwrap_or_else(|_| Color::white());
                            self.style.set_background_color(color);
                        }
                        Some(ComponentValue::HashToken(color_code)) => {
                            let color =
                                Color::from_code(color_code).unwrap_or_else(|_| Color::white());
                            self.style.set_background_color(color);
                        }
                        _ => {}
                    }
                    // The background shorthand may also carry an image layer,
                    // a position, and a repeat keyword.
                    if declaration.property == "background" {
                        if let Some(url) = extract_css_url(&declaration.value) {
                            self.style.set_background_image(url);
                        }
                        let (pos, no_repeat, size) =
                            scan_background_shorthand(&declaration.value);
                        if let Some((x, xp, y, yp)) = pos {
                            self.style.set_background_position(x, xp, y, yp);
                        }
                        if no_repeat {
                            self.style.set_background_no_repeat(true);
                        }
                        if let Some(size) = size {
                            self.style.set_background_size(size);
                        }
                    }
                }
                "background-image" => {
                    if let Some(url) = extract_css_url(&declaration.value) {
                        self.style.set_background_image(url);
                    }
                }
                "background-position" => {
                    let comps: Vec<(f64, bool, Option<bool>)> = declaration
                        .value
                        .iter()
                        .filter_map(bg_position_component)
                        .take(2)
                        .collect();
                    if let Some((x, xp, y, yp)) = assemble_bg_position(&comps) {
                        self.style.set_background_position(x, xp, y, yp);
                    }
                }
                "background-repeat" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style
                            .set_background_no_repeat(value.eq_ignore_ascii_case("no-repeat"));
                    }
                }
                "background-size" => {
                    if let Some(size) = parse_background_size(&declaration.value) {
                        self.style.set_background_size(size);
                    }
                }
                "line-height" => match first_value {
                    // A bare number is a factor of the element's own font size.
                    Some(ComponentValue::Number(v)) if *v > 0.0 => {
                        self.style.set_line_height(LineHeight::Factor(*v));
                    }
                    Some(ComponentValue::Dimension(v, unit)) => {
                        if unit == "%" {
                            if *v > 0.0 {
                                self.style.set_line_height(LineHeight::Factor(*v / 100.0));
                            }
                        } else if let Some(px) =
                            length_to_px(*v, unit, self.style.font_size_or_default())
                        {
                            if px > 0.0 {
                                self.style.set_line_height(LineHeight::Px(px));
                            }
                        }
                    }
                    // `normal` resets to the default leading.
                    Some(ComponentValue::Ident(v)) if v.eq_ignore_ascii_case("normal") => {
                        self.style.set_line_height(LineHeight::Factor(1.25));
                    }
                    _ => {}
                },
                "color" => match first_value {
                    Some(ComponentValue::Ident(value)) => {
                        let color = Color::from_name(value).unwrap_or_else(|_| Color::black());
                        self.style.set_color(color);
                    }
                    Some(ComponentValue::HashToken(color_code)) => {
                        let color = Color::from_code(color_code).unwrap_or_else(|_| Color::black());
                        self.style.set_color(color);
                    }
                    _ => {}
                },
                "display" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        let display_type =
                            DisplayType::from_str(value).unwrap_or(DisplayType::Block);
                        self.style.set_display(display_type)
                    }
                }
                "flex-direction" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style.set_flex_direction(FlexDirection::from_str(value));
                    }
                }
                "grid-template-columns" => {
                    let tracks = parse_grid_template_tracks(&declaration.value);
                    self.style.set_grid_template_columns(tracks);
                }
                // gap shorthand: one value = both axes, two = row then column.
                // https://www.w3.org/TR/css-align-3/#gap-shorthand
                "gap" | "grid-gap" => {
                    let px: Vec<f64> = declaration
                        .value
                        .iter()
                        .filter_map(|v| spacing_component_to_px(v, self.style.font_size_or_default()))
                        .collect();
                    match px.as_slice() {
                        [both] => {
                            self.style.set_row_gap(*both);
                            self.style.set_column_gap(*both);
                        }
                        [row, column, ..] => {
                            self.style.set_row_gap(*row);
                            self.style.set_column_gap(*column);
                        }
                        _ => {}
                    }
                }
                "column-gap" | "grid-column-gap" => {
                    if let Some(px) = first_value
                        .and_then(|v| spacing_component_to_px(v, self.style.font_size_or_default()))
                    {
                        self.style.set_column_gap(px);
                    }
                }
                "row-gap" | "grid-row-gap" => {
                    if let Some(px) = first_value
                        .and_then(|v| spacing_component_to_px(v, self.style.font_size_or_default()))
                    {
                        self.style.set_row_gap(px);
                    }
                }
                "width" => match first_value {
                    Some(ComponentValue::Number(value)) => {
                        self.style.set_width(*value);
                    }
                    Some(ComponentValue::Dimension(value, unit)) => match unit.as_str() {
                        "vw" => self.style.set_width_ratio(*value / 100.0),
                        "px" | "em" | "rem" => {
                            if let Some(px) = length_to_px(*value, unit, FontSize::Medium) {
                                self.style.set_width(px);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                },
                "height" => match first_value {
                    Some(ComponentValue::Number(value)) => {
                        self.style.set_height(*value);
                    }
                    Some(ComponentValue::Dimension(value, unit)) => match unit.as_str() {
                        "vh" => self.style.set_height_ratio(*value / 100.0),
                        "px" | "em" | "rem" => {
                            if let Some(px) = length_to_px(*value, unit, FontSize::Medium) {
                                self.style.set_height(px);
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                },
                "position" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        let position =
                            PositionType::from_str(value).unwrap_or(PositionType::Static);
                        self.style.set_position(position);
                    }
                }
                "top" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_offset_top(*value),
                    Some(ComponentValue::Dimension(value, unit)) if unit == "px" => {
                        self.style.set_offset_top(*value)
                    }
                    Some(ComponentValue::Dimension(value, unit)) if unit == "%" => {
                        self.style.set_offset_top_ratio(*value / 100.0)
                    }
                    _ => {}
                },
                "left" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_offset_left(*value),
                    Some(ComponentValue::Dimension(value, unit)) if unit == "px" => {
                        self.style.set_offset_left(*value)
                    }
                    Some(ComponentValue::Dimension(value, unit)) if unit == "%" => {
                        self.style.set_offset_left_ratio(*value / 100.0)
                    }
                    _ => {}
                },
                "right" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_offset_right(*value),
                    Some(ComponentValue::Dimension(value, unit)) if unit == "px" => {
                        self.style.set_offset_right(*value)
                    }
                    _ => {}
                },
                "bottom" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_offset_bottom(*value),
                    Some(ComponentValue::Dimension(value, unit)) if unit == "px" => {
                        self.style.set_offset_bottom(*value)
                    }
                    _ => {}
                },
                "z-index" => match first_value {
                    Some(ComponentValue::Number(value)) => self.style.set_z_index(*value as i32),
                    _ => {}
                },
                // overflow: scroll/auto clip like hidden — without interactive
                // inner scrolling (a renderer-side feature), clipping at the
                // box edge is exactly what an unscrolled scroll container
                // shows. `visible` (or unknown values) leaves content unclipped.
                "overflow" | "overflow-x" | "overflow-y" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style.set_overflow_clip(matches!(
                            value.as_str(),
                            "hidden" | "clip" | "scroll" | "auto"
                        ));
                        self.style.set_overflow_scrollable(matches!(
                            value.as_str(),
                            "scroll" | "auto"
                        ));
                    }
                }
                "margin" => {
                    let base_font_size = self.style.font_size_or_default();
                    if let Some((top, right, bottom, left)) =
                        parse_margin_shorthand(&declaration.value, base_font_size)
                    {
                        // Spec: CSS initial margin is 0, so when cascade runs before defaulting, fallback to 0.
                        // https://www.w3.org/TR/CSS22/box.html#margin-properties
                        let current = self.style.margin_or_default();
                        self.style.set_margin(
                            crate::renderer::layout::computed_style::EdgeSize::from_values(
                                top.unwrap_or(current.top()),
                                right.unwrap_or(current.right()),
                                bottom.unwrap_or(current.bottom()),
                                left.unwrap_or(current.left()),
                            ),
                        );
                    }
                    let (left_auto, right_auto) = parse_margin_auto_flags(&declaration.value);
                    self.style.set_margin_left_auto(left_auto);
                    self.style.set_margin_right_auto(right_auto);
                }
                "padding" => {
                    let base_font_size = self.style.font_size_or_default();
                    if let Some((top, right, bottom, left)) =
                        parse_spacing_shorthand(&declaration.value, base_font_size)
                    {
                        self.style.set_padding(
                            crate::renderer::layout::computed_style::EdgeSize::from_values(
                                top, right, bottom, left,
                            ),
                        );
                    }
                }
                "border" | "border-width" => {
                    let base_font_size = self.style.font_size_or_default();
                    if let Some((top, right, bottom, left)) =
                        parse_spacing_shorthand(&declaration.value, base_font_size)
                    {
                        self.style.set_border(
                            crate::renderer::layout::computed_style::EdgeSize::from_values(
                                top, right, bottom, left,
                            ),
                        );
                    }
                    // The `border` shorthand also carries a color (and style):
                    // pull a color token so the stroke is visible.
                    if declaration.property == "border" {
                        for v in &declaration.value {
                            let c = match v {
                                ComponentValue::HashToken(code) => Color::from_code(code).ok(),
                                ComponentValue::Ident(name) => Color::from_name(name).ok(),
                                _ => None,
                            };
                            if let Some(color) = c {
                                self.style.set_border_color(color);
                                break;
                            }
                        }
                    }
                }
                "border-color" => match first_value {
                    Some(ComponentValue::HashToken(code)) => {
                        if let Ok(c) = Color::from_code(code) {
                            self.style.set_border_color(c);
                        }
                    }
                    Some(ComponentValue::Ident(name)) => {
                        if let Ok(c) = Color::from_name(name) {
                            self.style.set_border_color(c);
                        }
                    }
                    _ => {}
                },
                "border-radius" => {
                    if let Some(px) = first_value
                        .and_then(|v| spacing_component_to_px(v, self.style.font_size_or_default()))
                    {
                        self.style.set_border_radius(px);
                    }
                }
                "white-space" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        // nowrap and pre suppress automatic wrapping at spaces;
                        // normal/pre-wrap/pre-line wrap.
                        self.style.set_white_space_nowrap(matches!(
                            value.as_str(),
                            "nowrap" | "pre"
                        ));
                    }
                }
                "text-overflow" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        self.style
                            .set_text_overflow_ellipsis(value.eq_ignore_ascii_case("ellipsis"));
                    }
                }
                // box-shadow: <dx> <dy> [blur] [spread] <color> (single
                // shadow; inset and shadow lists are ignored).
                "box-shadow" => {
                    let mut lengths: Vec<f64> = Vec::new();
                    let mut color: Option<Color> = None;
                    for v in &declaration.value {
                        match v {
                            ComponentValue::Ident(name)
                                if name.eq_ignore_ascii_case("none")
                                    || name.eq_ignore_ascii_case("inset") =>
                            {
                                lengths.clear();
                                color = None;
                                break;
                            }
                            ComponentValue::HashToken(code) => {
                                color = Color::from_code(code).ok();
                            }
                            ComponentValue::Ident(name) => {
                                if let Ok(c) = Color::from_name(name) {
                                    color = Some(c);
                                }
                            }
                            other => {
                                if lengths.len() < 4 {
                                    if let Some(px) = spacing_component_to_px(
                                        other,
                                        self.style.font_size_or_default(),
                                    ) {
                                        lengths.push(px);
                                    }
                                }
                            }
                        }
                    }
                    if lengths.len() >= 2 {
                        let blur = lengths.get(2).copied().unwrap_or(0.0);
                        let c = color.unwrap_or_else(Color::gray);
                        self.style.set_box_shadow(lengths[0], lengths[1], blur, c);
                    }
                }
                // transform values are not applied (no transform rendering),
                // but a non-none transform forms a stacking context.
                // https://www.w3.org/TR/css-transforms-1/#transform-rendering
                "transform" => {
                    let is_none = matches!(first_value,
                        Some(ComponentValue::Ident(v)) if v.eq_ignore_ascii_case("none"));
                    self.style
                        .set_has_transform(!is_none && first_value.is_some());
                    if !is_none {
                        if let Some(op) = parse_transform_ops(&declaration.value) {
                            self.style.set_transform_op(op);
                        }
                        if let Some(deg) = parse_transform_rotate(&declaration.value) {
                            self.style.set_transform_rotate(deg);
                        }
                    }
                }
                "opacity" => {
                    if let Some(ComponentValue::Number(value)) = first_value {
                        self.style.set_opacity(*value);
                    }
                }
                "font-family" => {
                    if let Some(font_family) = first_font_family(&declaration.value) {
                        self.style.set_font_family(font_family);
                    }
                }
                "font-size" => match first_value {
                    Some(ComponentValue::Ident(value)) => {
                        if let Ok(font_size) = FontSize::from_str(value) {
                            self.style.set_font_size(font_size);
                        }
                    }
                    Some(ComponentValue::Number(value)) => {
                        self.style.set_font_size(FontSize::from_px(*value));
                    }
                    Some(ComponentValue::Dimension(value, unit)) => {
                        // font-size em and % resolve against the PARENT's font
                        // size (CSS 2.2 §15.7); rem and absolute units resolve
                        // via length_to_px against the 16px root default.
                        let px = match unit.as_str() {
                            "em" => Some(*value * parent_font_size.px() as f64),
                            "%" => Some(*value / 100.0 * parent_font_size.px() as f64),
                            _ => length_to_px(*value, unit, parent_font_size),
                        };
                        if let Some(px) = px {
                            self.style.set_font_size(FontSize::from_px(px));
                        }
                    }
                    _ => {}
                },
                "text-decoration" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        if let Ok(decoration) = TextDecoration::from_str(value) {
                            self.style.set_text_decoration(decoration);
                        }
                    }
                }
                "text-align" => {
                    if let Some(ComponentValue::Ident(value)) = first_value {
                        match value.as_str() {
                            "center" => self.style.set_text_align(TextAlign::Center),
                            "right" => self.style.set_text_align(TextAlign::Right),
                            "left" => self.style.set_text_align(TextAlign::Left),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
    }

    pub fn defaulting_style(
        &mut self,
        node: &Rc<RefCell<Node>>,
        parent_style: Option<ComputedStyle>,
    ) {
        self.style.defaulting(node, parent_style);
    }

    pub fn update_kind(&mut self) {
        match self.node_kind() {
            NodeKind::Document => panic!("should not create a layout object for a document node"),
            NodeKind::Element(_) => match self.style.display() {
                // A flex container is itself a block-level box; flex affects how
                // its children are sized and positioned, not its own outer flow.
                DisplayType::Block | DisplayType::Flex | DisplayType::Grid => {
                    self.kind = LayoutObjectKind::Block
                }
                // inline-block flows inline; its block-ish sizing is handled
                // in the Inline compute_size arm.
                DisplayType::Inline | DisplayType::InlineBlock => {
                    self.kind = LayoutObjectKind::Inline
                }
                DisplayType::DisplayNone => {
                    panic!("should not create a layout object for a node with display:none")
                }
            },
            NodeKind::Text(_) => self.kind = LayoutObjectKind::Text,
        }
    }

    pub fn kind(&self) -> LayoutObjectKind {
        self.kind
    }

    pub fn node_kind(&self) -> NodeKind {
        self.node.borrow().kind().clone()
    }

    pub fn set_first_child(&mut self, first_child: Option<Rc<RefCell<LayoutObject>>>) {
        self.first_child = first_child;
    }

    pub fn first_child(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        self.first_child.as_ref().cloned()
    }

    pub fn set_next_sibling(&mut self, next_sibling: Option<Rc<RefCell<LayoutObject>>>) {
        self.next_sibling = next_sibling;
    }

    pub fn next_sibling(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        self.next_sibling.as_ref().cloned()
    }

    pub fn parent(&self) -> Weak<RefCell<Self>> {
        self.parent.clone()
    }

    pub fn style(&self) -> ComputedStyle {
        self.style.clone()
    }

    pub fn point(&self) -> LayoutPoint {
        self.point
    }

    /// Overwrite the computed position (used by post-layout passes such as
    /// fixed-position far-edge anchoring).
    pub fn set_point(&mut self, point: LayoutPoint) {
        self.point = point;
    }

    /// Stamp the sticky scroll context onto this node's style (see
    /// `LayoutView::stamp_sticky_contexts`).
    pub fn set_sticky_context(&mut self, top: f64, container_y: f64, max_delta: f64) {
        self.style.set_sticky_context(top, container_y, max_delta);
    }

    /// Mark this node as part of a position:fixed subtree (see
    /// `LayoutView::stamp_sticky_contexts`).
    pub fn set_fixed_subtree(&mut self) {
        self.style.set_fixed_subtree();
    }

    /// Upgraded parent layout object, when still alive.
    pub fn parent_object(&self) -> Option<Rc<RefCell<LayoutObject>>> {
        self.parent.upgrade()
    }

    /// Stamp the final paint-order key (see `LayoutView::stamp_sticky_contexts`).
    pub fn set_paint_z(&mut self, z: i32) {
        self.style.set_paint_z(z);
    }

    /// Stamp a scale context (see `LayoutView::apply_transforms`).
    pub fn set_scale_context(&mut self, ox: f64, oy: f64, factor: f64) {
        self.style.set_scale_context(ox, oy, factor);
    }

    /// Stamp a rotation context (see `LayoutView::apply_transforms`).
    pub fn set_rotate_context(&mut self, cx: f64, cy: f64, deg: f64) {
        self.style.set_rotate_context(cx, cy, deg);
    }

    /// Stamp the final clip rectangle (intersection of overflow ancestors).
    pub fn set_final_clip(&mut self, clip: (f64, f64, f64, f64)) {
        self.style.set_final_clip(clip);
    }

    /// Stamp the nearest scroll-container id for this box's content.
    pub fn set_scroll_container(&mut self, id: u32) {
        self.style.set_scroll_container(id);
    }

    /// Mark this box as a scroll container (id + scrollable content size).
    pub fn set_scroll_container_def(&mut self, id: u32, content_w: f64, content_h: f64) {
        self.style.set_scroll_container_def(id, content_w, content_h);
    }

    pub fn size(&self) -> LayoutSize {
        self.size
    }

    /// Return the HTML `cellspacing` value from the nearest ancestor `<table>`.
    ///
    /// Walks up at most a few layout-tree steps (td/tr → table) and reads the
    /// table's `cellspacing` attribute.  Returns the HTML4 default of 2 when no
    /// attribute is present.  Returns 0 when no TABLE ancestor is found.
    ///
    /// Spec: HTML4.01 §11.3.3 — cellspacing default is 2.
    /// https://www.w3.org/TR/html4/struct/tables.html#adef-cellspacing
    fn ancestor_table_cellspacing(&self) -> i64 {
        let mut current = self.parent.upgrade();
        let mut steps = 0;
        while let Some(ancestor) = current {
            steps += 1;
            if steps > 5 {
                break;
            }
            let b = ancestor.borrow();
            if b.is_table() {
                return b
                    .element_attribute("cellspacing")
                    .and_then(|v| v.trim().parse::<i64>().ok())
                    .unwrap_or(2); // HTML4 default
            }
            let next = b.parent.upgrade();
            drop(b);
            current = next;
        }
        0
    }

    /// Return the HTML `cellpadding` value from the nearest ancestor `<table>`.
    ///
    /// Walks up at most a few layout-tree steps (td → tr → optional tbody → table)
    /// and reads the table's `cellpadding` attribute.  Returns the HTML4 default
    /// of 1 when no attribute is present, and 0 when the attribute is explicitly "0".
    /// Returns 0 when called on a non-cell element.
    ///
    /// Spec: HTML4.01 §11.3.2 — cellpadding default is 1.
    /// https://www.w3.org/TR/html4/struct/tables.html#adef-cellpadding
    fn ancestor_table_cellpadding(&self) -> i64 {
        let mut current = self.parent.upgrade();
        let mut steps = 0;
        while let Some(ancestor) = current {
            steps += 1;
            if steps > 5 {
                break;
            }
            let b = ancestor.borrow();
            if b.is_table() {
                return b
                    .element_attribute("cellpadding")
                    .and_then(|v| v.trim().parse::<i64>().ok())
                    .unwrap_or(1); // HTML4 default
            }
            let next = b.parent.upgrade();
            drop(b);
            current = next;
        }
        0
    }

    pub fn content_origin(&self) -> LayoutPoint {
        let metrics = compute_box_model_metrics(&self.style);
        let cp = if self.is_table_cell() {
            self.ancestor_table_cellpadding()
        } else {
            0
        };
        LayoutPoint::new(
            self.point.x() + metrics.padding.left + metrics.border.left + cp,
            self.point.y() + metrics.padding.top + metrics.border.top + cp,
        )
    }

    pub fn content_size(&self) -> LayoutSize {
        let metrics = compute_box_model_metrics(&self.style);
        let cp = if self.is_table_cell() {
            self.ancestor_table_cellpadding()
        } else {
            0
        };
        LayoutSize::new(
            (self.size.width() - metrics.inner_horizontal() - 2 * cp).max(0),
            (self.size.height() - metrics.inner_vertical() - 2 * cp).max(0),
        )
    }

    /// Returns the total box model overhead (border + padding) in the vertical axis.
    pub fn vertical_overhead(&self) -> i64 {
        let metrics = compute_box_model_metrics(&self.style);
        metrics.inner_vertical()
    }

    /// Directly overrides the computed height. Used by the rowspan height
    /// expansion pass to expand rows that are constrained by rowspan cells.
    pub fn force_set_height(&mut self, h: i64) {
        self.size.set_height(h);
    }

    /// Directly overrides the computed width. Used by the column width
    /// equalization pass to align cell borders across all rows.
    pub fn force_set_width(&mut self, w: i64) {
        self.size.set_width(w);
    }

    /// Returns true if this layout object corresponds to a `<table>` element.
    pub fn is_table(&self) -> bool {
        self.element_kind() == Some(ElementKind::Table)
    }

}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct LayoutPoint {
    x: i64,
    y: i64,
}

impl LayoutPoint {
    pub fn new(x: i64, y: i64) -> Self {
        Self { x, y }
    }

    pub fn x(&self) -> i64 {
        self.x
    }

    pub fn y(&self) -> i64 {
        self.y
    }

    pub fn set_x(&mut self, x: i64) {
        self.x = x;
    }

    pub fn set_y(&mut self, y: i64) {
        self.y = y;
    }
}

#[derive(Debug, Copy, Clone, PartialEq)]
pub struct LayoutSize {
    width: i64,
    height: i64,
}

impl LayoutSize {
    pub fn new(width: i64, height: i64) -> Self {
        Self { width, height }
    }

    pub fn width(&self) -> i64 {
        self.width
    }

    pub fn height(&self) -> i64 {
        self.height
    }

    pub fn set_width(&mut self, width: i64) {
        self.width = width;
    }

    pub fn set_height(&mut self, height: i64) {
        self.height = height;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::layout::computed_style::EdgeSize;

    #[test]
    fn compute_box_model_metrics_includes_margin_padding_border() {
        let mut style = ComputedStyle::new();
        style.set_margin(EdgeSize::from_values(4.0, 6.0, 8.0, 10.0));
        style.set_padding(EdgeSize::from_values(1.0, 2.0, 3.0, 4.0));
        style.set_border(EdgeSize::from_values(2.0, 2.0, 2.0, 2.0));

        let metrics = compute_box_model_metrics(&style);

        assert_eq!(metrics.outer_horizontal(), 26);
        assert_eq!(metrics.outer_vertical(), 20);
        assert_eq!(metrics.inner_horizontal(), 10);
        assert_eq!(metrics.inner_vertical(), 8);
    }

    #[test]
    fn normal_flow_spec_maps_block_and_inline() {
        assert_eq!(
            LayoutObjectKind::Block.normal_flow_spec(),
            NormalFlowSpec {
                flow: LayoutFlow::BlockFormattingContext,
                stacks_vertically: true,
                keeps_inline_line: false,
            }
        );
        assert_eq!(LayoutObjectKind::Inline.normal_flow_spec().flow, LayoutFlow::InlineFlow);
        assert_eq!(LayoutObjectKind::Text.normal_flow_spec().flow, LayoutFlow::InlineFlow);
    }
}
