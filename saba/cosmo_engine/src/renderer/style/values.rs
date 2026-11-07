//! CSS declaration-value parsing helpers, extracted verbatim from
//! layout_object.rs (plan 0.5). These parse ComponentValues / HTML attribute
//! strings into typed values; they do no layout.

use crate::renderer::css::cssom::ComponentValue;
use crate::renderer::layout::computed_style::Color;
use crate::renderer::layout::computed_style::LinearGradient;
use crate::renderer::layout::computed_style::FontSize;
use crate::renderer::layout::computed_style::{
    AlignContent, AlignItems, FlexWrap, MathExpr, VerticalAlign, GridAutoRepeat, GridLine, GridPlacement, GridTemplate,
    GridTrack, JustifyContent,
};

use std::cell::Cell;
use std::cell::RefCell;

thread_local! {
    /// Viewport (width, height) for resolving vw/vh units during styling.
    /// Set by LayoutView before the style/layout pass; the engine is
    /// single-threaded per page so a thread-local is sufficient (same
    /// transitional pattern as the font-metrics provider).
    static STYLING_VIEWPORT: Cell<(i64, i64)> = const { Cell::new((0, 0)) };

    /// The root element's font size, which `rem` is relative to.
    static ROOT_FONT_PX: Cell<f64> = const { Cell::new(16.0) };

    /// Node identities (`Rc::as_ptr`) of the element under the pointer and its
    /// ancestors — everything `:hover` currently matches. Published by the
    /// renderer before a style pass, same transitional pattern as the viewport
    /// above. Empty = nothing hovered.
    /// Spec: Selectors 4 §9.1 — `:hover` matches an element *and* its
    /// ancestors. https://www.w3.org/TR/selectors-4/#the-hover-pseudo
    static HOVER_CHAIN: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };

    /// The focused element and its ancestors, same pattern as `HOVER_CHAIN`.
    /// The head is the focused element itself: `:focus` matches only that,
    /// `:focus-within` matches the whole chain.
    static FOCUS_CHAIN: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

pub(crate) fn styling_viewport() -> (i64, i64) {
    STYLING_VIEWPORT.with(|v| v.get())
}

pub(crate) fn set_styling_viewport(width: i64, height: i64) {
    STYLING_VIEWPORT.with(|v| v.set((width, height)));
}

/// Publish the hover chain (element under the pointer first, then ancestors)
/// for subsequent style passes. Pass an empty slice when the pointer leaves the
/// page. Returns whether the chain actually changed — the renderer only needs
/// to re-style when it did.
pub fn set_hover_chain(chain: &[usize]) -> bool {
    HOVER_CHAIN.with(|c| {
        let mut current = c.borrow_mut();
        if *current == chain {
            return false;
        }
        current.clear();
        current.extend_from_slice(chain);
        true
    })
}

/// Whether `node_key` (an `Rc::as_ptr` identity) is in the hover chain.
pub(crate) fn is_hovered(node_key: usize) -> bool {
    HOVER_CHAIN.with(|c| c.borrow().contains(&node_key))
}

/// Publish the focus chain: the focused element first, then its ancestors.
/// Empty when nothing is focused. Returns whether it changed.
pub fn set_focus_chain(chain: &[usize]) -> bool {
    FOCUS_CHAIN.with(|c| {
        let mut current = c.borrow_mut();
        if *current == chain {
            return false;
        }
        current.clear();
        current.extend_from_slice(chain);
        true
    })
}

/// Whether `node_key` is *the* focused element — `:focus` matches only it.
pub(crate) fn is_focused(node_key: usize) -> bool {
    FOCUS_CHAIN.with(|c| c.borrow().first() == Some(&node_key))
}

/// Whether `node_key` is the focused element or an ancestor of it —
/// `:focus-within`. Spec: Selectors 4 §9.4.
pub(crate) fn is_focus_within(node_key: usize) -> bool {
    FOCUS_CHAIN.with(|c| c.borrow().contains(&node_key))
}

pub(crate) fn length_to_px(value: f64, unit: &str, base_font_size: FontSize) -> Option<f64> {
    match unit {
        "px" => Some(value),
        "em" => Some(value * base_font_size.px() as f64),
        "rem" => Some(value * ROOT_FONT_PX.with(|root| root.get())),
        // Approximations: an `ex` is about half an em, a `ch` the width of "0".
        "ex" | "ch" => Some(value * base_font_size.px() as f64 * 0.5),
        "pt" => Some(value * 96.0 / 72.0),
        "pc" => Some(value * 16.0),
        "in" => Some(value * 96.0),
        "cm" => Some(value * 96.0 / 2.54),
        "mm" => Some(value * 96.0 / 25.4),
        "q" | "Q" => Some(value * 96.0 / 101.6),
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

/// The values between the first pair of parentheses starting at `open`, and the
/// index just past the closing one.
fn function_args(values: &[ComponentValue], open: usize) -> (&[ComponentValue], usize) {
    let mut depth = 1;
    let mut j = open + 1;
    while j < values.len() && depth > 0 {
        match &values[j] {
            ComponentValue::OpenParenthesis => depth += 1,
            ComponentValue::CloseParenthesis => depth -= 1,
            _ => {}
        }
        j += 1;
    }
    let end = if depth == 0 { j - 1 } else { j };
    (&values[(open + 1).min(values.len())..end], j)
}

fn split_args(values: &[ComponentValue]) -> Vec<&[ComponentValue]> {
    let mut parts = Vec::new();
    let mut depth = 0;
    let mut start = 0;
    for (i, value) in values.iter().enumerate() {
        match value {
            ComponentValue::OpenParenthesis => depth += 1,
            ComponentValue::CloseParenthesis => depth -= 1,
            ComponentValue::Delim(',') if depth == 0 => {
                parts.push(&values[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&values[start..]);
    parts
}

/// A value list split at each top-level `/`, without white space.
pub(crate) fn split_on_slash(values: &[ComponentValue]) -> Vec<Vec<ComponentValue>> {
    let mut parts = vec![Vec::new()];
    for value in values {
        match value {
            ComponentValue::Whitespace => {}
            ComponentValue::Delim('/') => parts.push(Vec::new()),
            other => parts.last_mut().expect("never empty").push(other.clone()),
        }
    }
    parts
}

/// Parse a track list: `[names] <track> ... repeat(...) ...`.
/// https://www.w3.org/TR/css-grid-1/#track-sizing
pub(crate) fn parse_grid_template(values: &[ComponentValue]) -> GridTemplate {
    let mut template = GridTemplate {
        tracks: Vec::new(),
        line_names: vec![Vec::new()],
        auto_repeat: None,
    };
    let values: Vec<ComponentValue> = values
        .iter()
        .filter(|v| !matches!(v, ComponentValue::Whitespace))
        .cloned()
        .collect();
    parse_track_list(&values, &mut template);
    template
}

fn parse_track_list(values: &[ComponentValue], template: &mut GridTemplate) {
    let mut i = 0;
    while i < values.len() {
        match &values[i] {
            ComponentValue::Delim('[') => {
                i += 1;
                while i < values.len() && !matches!(&values[i], ComponentValue::Delim(']')) {
                    if let ComponentValue::Ident(name) = &values[i] {
                        template
                            .line_names
                            .last_mut()
                            .expect("line_names is never empty")
                            .push(name.to_ascii_lowercase());
                    }
                    i += 1;
                }
                i += 1;
            }
            ComponentValue::Ident(name)
                if name.eq_ignore_ascii_case("repeat")
                    && matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis)) =>
            {
                let (args, next) = function_args(values, i + 1);
                i = next;
                let parts = split_args(args);
                let (Some(count), Some(body)) = (parts.first(), parts.get(1)) else {
                    continue;
                };
                let mut unit = GridTemplate {
                    tracks: Vec::new(),
                    line_names: vec![Vec::new()],
                    auto_repeat: None,
                };
                parse_track_list(body, &mut unit);
                match count.first() {
                    Some(ComponentValue::Ident(keyword)) => {
                        template.auto_repeat = Some(GridAutoRepeat {
                            at: template.tracks.len(),
                            tracks: unit.tracks,
                            fit: keyword.eq_ignore_ascii_case("auto-fit"),
                        });
                    }
                    Some(ComponentValue::Number(n)) => {
                        // More tracks than this is not a layout anyone wants;
                        // the bound keeps a hostile count from exhausting memory.
                        for _ in 0..(*n as usize).clamp(1, 1000) {
                            for (t, track) in unit.tracks.iter().enumerate() {
                                template.tracks.push(track.clone());
                                let names = unit.line_names.get(t + 1).cloned().unwrap_or_default();
                                template.line_names.push(names);
                            }
                        }
                    }
                    _ => {}
                }
            }
            _ => {
                let (track, next) = parse_track(values, i);
                i = next;
                if let Some(track) = track {
                    template.tracks.push(track);
                    template.line_names.push(Vec::new());
                }
            }
        }
    }
}

fn parse_track(values: &[ComponentValue], i: usize) -> (Option<GridTrack>, usize) {
    match &values[i] {
        ComponentValue::Dimension(v, unit) => (
            Some(if unit == "fr" {
                GridTrack::Fr(v.max(0.0))
            } else if unit == "%" {
                GridTrack::Percent((*v / 100.0).max(0.0))
            } else if let Some(px) = length_to_px(*v, unit, FontSize::Medium) {
                GridTrack::Px(px.max(0.0))
            } else {
                GridTrack::Auto
            }),
            i + 1,
        ),
        ComponentValue::Number(v) => (Some(GridTrack::Px(v.max(0.0))), i + 1),
        ComponentValue::Ident(name)
            if matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis)) =>
        {
            let (args, next) = function_args(values, i + 1);
            let parts = split_args(args);
            let one = |part: Option<&&[ComponentValue]>| {
                part.filter(|p| !p.is_empty())
                    .and_then(|p| parse_track(p, 0).0)
                    .unwrap_or(GridTrack::Auto)
            };
            let track = if name.eq_ignore_ascii_case("minmax") {
                GridTrack::MinMax(
                    Box::new(one(parts.first())),
                    Box::new(one(parts.get(1))),
                )
            } else if name.eq_ignore_ascii_case("fit-content") {
                match one(parts.first()) {
                    GridTrack::Px(px) => GridTrack::FitContent(px),
                    _ => GridTrack::Auto,
                }
            } else {
                GridTrack::Auto
            };
            (Some(track), next)
        }
        ComponentValue::Ident(name) => (
            match name.to_ascii_lowercase().as_str() {
                "min-content" => Some(GridTrack::MinContent),
                "max-content" => Some(GridTrack::MaxContent),
                "auto" => Some(GridTrack::Auto),
                // `none` and anything else declares no tracks.
                _ => None,
            },
            i + 1,
        ),
        _ => (None, i + 1),
    }
}

/// `<line>`: `auto` | `<integer>` | `span <integer>` | `<name> [<integer>]`.
pub(crate) fn parse_grid_line(values: &[ComponentValue]) -> GridLine {
    let parts: Vec<&ComponentValue> = values
        .iter()
        .filter(|v| !matches!(v, ComponentValue::Whitespace))
        .collect();
    let number = parts.iter().find_map(|v| match v {
        ComponentValue::Number(n) => Some(*n as i32),
        _ => None,
    });
    let idents: Vec<&String> = parts
        .iter()
        .filter_map(|v| match v {
            ComponentValue::Ident(name) => Some(name),
            _ => None,
        })
        .collect();
    let spanning = idents.iter().any(|name| name.eq_ignore_ascii_case("span"));
    let name = idents
        .iter()
        .find(|name| !name.eq_ignore_ascii_case("span") && !name.eq_ignore_ascii_case("auto"));
    match (spanning, name, number) {
        (true, _, n) => GridLine::Span(n.unwrap_or(1).max(1)),
        (false, Some(name), n) => GridLine::Name(name.to_ascii_lowercase(), n.unwrap_or(1)),
        (false, None, Some(n)) if n != 0 => GridLine::Number(n),
        _ => GridLine::Auto,
    }
}

/// `grid-column`/`grid-row`: `<line> [ / <line> ]`. A named start with no end
/// repeats on the end, where it resolves to that area's `-end` line.
pub(crate) fn parse_grid_placement(values: &[ComponentValue]) -> GridPlacement {
    let parts = split_on_slash(values);
    let start = parts.first().map(|p| parse_grid_line(p)).unwrap_or_default();
    let end = match parts.get(1) {
        Some(part) => parse_grid_line(part),
        None => match &start {
            GridLine::Name(..) => start.clone(),
            _ => GridLine::Auto,
        },
    };
    GridPlacement { start, end }
}

pub(crate) fn parse_flex_wrap(value: &str) -> Option<FlexWrap> {
    match value {
        "nowrap" => Some(FlexWrap::NoWrap),
        "wrap" => Some(FlexWrap::Wrap),
        "wrap-reverse" => Some(FlexWrap::WrapReverse),
        _ => None,
    }
}

pub(crate) fn parse_justify_content(value: &str) -> Option<JustifyContent> {
    match value {
        "normal" | "stretch" => Some(JustifyContent::Normal),
        "flex-start" | "start" | "left" => Some(JustifyContent::FlexStart),
        "flex-end" | "end" | "right" => Some(JustifyContent::FlexEnd),
        "center" => Some(JustifyContent::Center),
        "space-between" => Some(JustifyContent::SpaceBetween),
        "space-around" => Some(JustifyContent::SpaceAround),
        "space-evenly" => Some(JustifyContent::SpaceEvenly),
        _ => None,
    }
}

pub(crate) fn parse_align_content(value: &str) -> Option<AlignContent> {
    match value {
        "normal" | "stretch" => Some(AlignContent::Stretch),
        "flex-start" | "start" => Some(AlignContent::FlexStart),
        "flex-end" | "end" => Some(AlignContent::FlexEnd),
        "center" => Some(AlignContent::Center),
        "space-between" => Some(AlignContent::SpaceBetween),
        "space-around" => Some(AlignContent::SpaceAround),
        "space-evenly" => Some(AlignContent::SpaceEvenly),
        _ => None,
    }
}

pub(crate) fn parse_align_keyword(value: &str) -> Option<AlignItems> {
    match value {
        "stretch" | "normal" => Some(AlignItems::Stretch),
        "flex-start" | "start" | "self-start" | "left" => Some(AlignItems::FlexStart),
        "center" => Some(AlignItems::Center),
        "flex-end" | "end" | "self-end" | "right" => Some(AlignItems::FlexEnd),
        "baseline" => Some(AlignItems::Baseline),
        _ => None,
    }
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

pub(crate) fn set_root_font_px(px: f64) {
    ROOT_FONT_PX.with(|root| root.set(px));
}

/// The first entry of a `font-family` list this engine can match, as a generic
/// family name. Tokens of an unquoted name (`Helvetica Neue`) join into one entry.
pub(crate) fn first_font_family(value: &[ComponentValue]) -> Option<String> {
    let mut entry = String::new();
    let mut settle = |entry: &mut String| {
        let class = crate::renderer::text::face::FontClass::from_family(entry);
        entry.clear();
        class
    };
    for component in value {
        match component {
            ComponentValue::Ident(name) | ComponentValue::StringToken(name) => {
                if !entry.is_empty() {
                    entry.push(' ');
                }
                entry.push_str(name);
            }
            ComponentValue::Delim(',') => {
                if let Some(class) = settle(&mut entry) {
                    return Some(class.name().to_string());
                }
            }
            _ => {}
        }
    }
    settle(&mut entry).map(|class| class.name().to_string())
}

/// `vertical-align`. https://www.w3.org/TR/CSS22/visudet.html#propdef-vertical-align
pub(crate) fn parse_vertical_align(
    value: Option<&ComponentValue>,
    base: FontSize,
) -> Option<VerticalAlign> {
    match value? {
        ComponentValue::Ident(keyword) => Some(match keyword.as_str() {
            "baseline" => VerticalAlign::Baseline,
            "sub" => VerticalAlign::Sub,
            "super" => VerticalAlign::Super,
            "middle" => VerticalAlign::Middle,
            "top" => VerticalAlign::Top,
            "bottom" => VerticalAlign::Bottom,
            "text-top" => VerticalAlign::TextTop,
            "text-bottom" => VerticalAlign::TextBottom,
            _ => return None,
        }),
        ComponentValue::Number(v) => Some(VerticalAlign::Raise(*v)),
        ComponentValue::Dimension(v, unit) if unit == "%" => {
            Some(VerticalAlign::RaiseRatio(*v / 100.0))
        }
        ComponentValue::Dimension(v, unit) => {
            length_to_px(*v, unit, base).map(VerticalAlign::Raise)
        }
        _ => None,
    }
}

/// `aspect-ratio`: `<width> [ / <height> ]`, or `auto`.
pub(crate) fn parse_aspect_ratio(values: &[ComponentValue]) -> Option<f64> {
    let numbers: Vec<f64> = values
        .iter()
        .filter_map(|v| match v {
            ComponentValue::Number(n) => Some(*n),
            _ => None,
        })
        .collect();
    match numbers.as_slice() {
        [w] if *w > 0.0 => Some(*w),
        [w, h] if *w > 0.0 && *h > 0.0 => Some(*w / *h),
        _ => None,
    }
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

/// Which of (top, right, bottom, left) a `margin` shorthand sets to `auto`.
pub(crate) fn parse_margin_auto_flags(value: &[ComponentValue]) -> (bool, bool, bool, bool) {
    let flags = value
        .iter()
        .filter(|component| !matches!(component, ComponentValue::Whitespace))
        .map(|component| matches!(component, ComponentValue::Ident(name) if name == "auto"))
        .collect::<Vec<_>>();

    match flags.as_slice() {
        [all] => (*all, *all, *all, *all),
        [vertical, horizontal] => (*vertical, *horizontal, *vertical, *horizontal),
        [top, horizontal, bottom] => (*top, *horizontal, *bottom, *horizontal),
        [top, right, bottom, left] => (*top, *right, *bottom, *left),
        _ => (false, false, false, false),
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
    fn parses_linear_gradient() {
        let g = parse_linear_gradient(&vals("linear-gradient(90deg, red, blue)")).unwrap();
        assert_eq!(g.angle_deg, 90.0);
        assert_eq!(g.stops.len(), 2);
        assert_eq!(g.stops[0].0.code(), "#ff0000");
        assert_eq!(g.stops[0].1, 0.0);
        assert_eq!(g.stops[1].0.code(), "#0000ff");
        assert_eq!(g.stops[1].1, 1.0);

        // `to right` = 90deg; three evenly-spaced stops.
        let g = parse_linear_gradient(&vals("linear-gradient(to right, #000, #888, #fff)")).unwrap();
        assert_eq!(g.angle_deg, 90.0);
        assert_eq!(g.stops.len(), 3);
        assert!((g.stops[1].1 - 0.5).abs() < 1e-9);

        // Default direction (to bottom = 180deg) + explicit positions.
        let g = parse_linear_gradient(&vals("linear-gradient(red 20%, blue 80%)")).unwrap();
        assert_eq!(g.angle_deg, 180.0);
        assert!((g.stops[0].1 - 0.2).abs() < 1e-9);
        assert!((g.stops[1].1 - 0.8).abs() < 1e-9);

        // Not a gradient / single stop -> None.
        assert!(parse_linear_gradient(&vals("url(x.png)")).is_none());
        assert!(parse_linear_gradient(&vals("linear-gradient(red)")).is_none());
    }

    #[test]
    fn calc_folds_absolute_lengths() {
        use crate::renderer::layout::computed_style::FontSize;
        let folded = fold_calc(&vals("calc(20em * -1)"), FontSize::Medium);
        assert_eq!(folded, vals("-320px"), "{:?}", folded);
        let folded = fold_calc(&vals("calc(100px + 2em - (4px / 2))"), FontSize::Medium);
        assert_eq!(folded, vals("130px"), "{:?}", folded);
        let folded = fold_calc(&vals("clamp(10px, max(5px, 2em), 20px)"), FontSize::Medium);
        assert_eq!(folded, vals("20px"), "{:?}", folded);
    }

    #[test]
    fn math_with_a_percentage_waits_for_the_containing_block() {
        use crate::renderer::layout::computed_style::FontSize;
        let expr = parse_math_value(&vals("calc(100% - 20px)"), FontSize::Medium).unwrap();
        assert_eq!(expr.eval(200.0), 180.0);
        let expr = parse_math_value(&vals("min(50%, 300px)"), FontSize::Medium).unwrap();
        assert_eq!(expr.eval(200.0), 100.0);
        assert_eq!(expr.eval(1000.0), 300.0);
    }

    #[test]
    fn font_family_takes_the_first_entry_it_knows() {
        let family = |css: &str| first_font_family(&vals(css));
        assert_eq!(family("Inter, Georgia, serif").as_deref(), Some("sans-serif"));
        assert_eq!(family("WebFontX, Georgia, sans-serif").as_deref(), Some("serif"));
        assert_eq!(family("'Courier New', monospace").as_deref(), Some("monospace"));
        assert_eq!(family("Helvetica Neue, Arial").as_deref(), Some("sans-serif"));
        assert_eq!(family("Verdana").as_deref(), Some("wide-sans-serif"));
        assert_eq!(family("WebFontX"), None);
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

/// Whether a declaration value contains a math function to fold.
pub(crate) fn has_math_function(values: &[ComponentValue]) -> bool {
    values.windows(2).any(|pair| {
        matches!(&pair[0], ComponentValue::Ident(name) if is_math_function(name))
            && matches!(pair[1], ComponentValue::OpenParenthesis)
    })
}

fn is_math_function(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "calc" | "min" | "max" | "clamp" | "-webkit-calc" | "-moz-calc"
    )
}

/// Parse the math function at the start of `values`, if that's what's there.
pub(crate) fn parse_math_value(values: &[ComponentValue], base: FontSize) -> Option<MathExpr> {
    if !matches!(values.first(), Some(ComponentValue::Ident(name)) if is_math_function(name)) {
        return None;
    }
    let mut it = values.iter().peekable();
    let expr = parse_math_atom(&mut it, base)?;
    Some(expr)
}

/// Fold every math function in a declaration value into a plain px length.
/// A percentage has nothing to resolve against here, so it counts against the
/// viewport's width, which is the right order of magnitude; `width` and `height`
/// avoid this by keeping the expression for layout.
/// Runs after var() substitution so `calc(var(--x)*2)` folds too.
pub(crate) fn fold_calc(values: &[ComponentValue], base_font_size: FontSize) -> Vec<ComponentValue> {
    let (viewport_width, _) = STYLING_VIEWPORT.with(|v| v.get());
    let mut out = Vec::with_capacity(values.len());
    let mut i = 0;
    while i < values.len() {
        let is_math = matches!(&values[i], ComponentValue::Ident(s) if is_math_function(s))
            && matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis));
        if is_math {
            let (_, next) = function_args(values, i + 1);
            match parse_math_value(&values[i..next], base_font_size) {
                Some(expr) => out.push(ComponentValue::Dimension(
                    expr.eval(viewport_width as f64),
                    "px".to_string(),
                )),
                // Unresolvable: keep the original tokens (arms will skip).
                None => out.extend_from_slice(&values[i..next]),
            }
            i = next;
        } else {
            out.push(values[i].clone());
            i += 1;
        }
    }
    out
}

type CalcIter<'a> = core::iter::Peekable<core::slice::Iter<'a, ComponentValue>>;

fn skip_math_whitespace(it: &mut CalcIter) {
    while matches!(it.peek(), Some(ComponentValue::Whitespace)) {
        it.next();
    }
}

fn parse_math_sum(it: &mut CalcIter, base: FontSize) -> Option<MathExpr> {
    let mut acc = parse_math_product(it, base)?;
    loop {
        skip_math_whitespace(it);
        let plus = match it.peek() {
            Some(ComponentValue::Delim('+')) => true,
            Some(ComponentValue::Delim('-')) => false,
            // The CSS tokenizer's negative-number handling emits a lone
            // minus/plus between spaces as an Ident.
            Some(ComponentValue::Ident(op)) if op == "+" => true,
            Some(ComponentValue::Ident(op)) if op == "-" => false,
            // A sign the tokenizer folded into the next number: `10px -2px`
            // arrives as two lengths.
            Some(ComponentValue::Dimension(v, _)) | Some(ComponentValue::Number(v))
                if *v < 0.0 =>
            {
                let rhs = parse_math_product(it, base)?;
                acc = MathExpr::Add(Box::new(acc), Box::new(rhs));
                continue;
            }
            _ => return Some(acc),
        };
        it.next();
        let rhs = parse_math_product(it, base)?;
        acc = if plus {
            MathExpr::Add(Box::new(acc), Box::new(rhs))
        } else {
            MathExpr::Sub(Box::new(acc), Box::new(rhs))
        };
    }
}

fn parse_math_product(it: &mut CalcIter, base: FontSize) -> Option<MathExpr> {
    let mut acc = parse_math_atom(it, base)?;
    loop {
        skip_math_whitespace(it);
        match it.peek() {
            Some(ComponentValue::Delim('*')) => {
                it.next();
                let rhs = parse_math_atom(it, base)?;
                acc = MathExpr::Mul(Box::new(acc), Box::new(rhs));
            }
            Some(ComponentValue::Delim('/')) => {
                it.next();
                let rhs = parse_math_atom(it, base)?;
                acc = MathExpr::Div(Box::new(acc), Box::new(rhs));
            }
            _ => return Some(acc),
        }
    }
}

fn parse_math_args(it: &mut CalcIter, base: FontSize) -> Option<Vec<MathExpr>> {
    let mut args = Vec::new();
    loop {
        args.push(parse_math_sum(it, base)?);
        skip_math_whitespace(it);
        match it.next()? {
            ComponentValue::Delim(',') => {}
            ComponentValue::CloseParenthesis => return Some(args),
            _ => return None,
        }
    }
}

fn parse_math_atom(it: &mut CalcIter, base: FontSize) -> Option<MathExpr> {
    skip_math_whitespace(it);
    match it.next()? {
        ComponentValue::Number(n) => Some(MathExpr::Number(*n)),
        ComponentValue::Dimension(n, unit) if unit == "%" => Some(MathExpr::Ratio(*n / 100.0)),
        ComponentValue::Dimension(n, unit) => length_to_px(*n, unit, base).map(MathExpr::Px),
        ComponentValue::OpenParenthesis => {
            let inner = parse_math_sum(it, base)?;
            skip_math_whitespace(it);
            match it.next() {
                Some(ComponentValue::CloseParenthesis) => Some(inner),
                _ => None,
            }
        }
        ComponentValue::Ident(name) if matches!(it.peek(), Some(ComponentValue::OpenParenthesis)) => {
            it.next();
            let mut args = parse_math_args(it, base)?;
            match name.to_ascii_lowercase().as_str() {
                "calc" | "-webkit-calc" | "-moz-calc" if args.len() == 1 => args.pop(),
                "min" => Some(MathExpr::Min(args)),
                "max" => Some(MathExpr::Max(args)),
                "clamp" if args.len() == 3 => {
                    let max = args.pop()?;
                    let value = args.pop()?;
                    let min = args.pop()?;
                    Some(MathExpr::Clamp(Box::new(min), Box::new(value), Box::new(max)))
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Parse a `linear-gradient(...)` from a declaration value (the first one
/// found). Supports an optional leading `<angle>deg` or `to <side>`
/// direction, followed by comma-separated color stops (each an optional
/// position in %). Returns None when no gradient is present or it is
/// unparseable. Repeating/radial/conic gradients are not handled.
pub(crate) fn parse_linear_gradient(values: &[ComponentValue]) -> Option<LinearGradient> {
    // Locate `linear-gradient(` and collect its balanced argument tokens.
    let mut i = 0;
    while i < values.len() {
        let is_grad = matches!(&values[i], ComponentValue::Ident(s)
            if s.eq_ignore_ascii_case("linear-gradient"))
            && matches!(values.get(i + 1), Some(ComponentValue::OpenParenthesis));
        if is_grad {
            break;
        }
        i += 1;
    }
    if i >= values.len() {
        return None;
    }
    let mut depth = 1;
    let mut j = i + 2;
    let mut inner: Vec<ComponentValue> = Vec::new();
    while j < values.len() && depth > 0 {
        match &values[j] {
            ComponentValue::OpenParenthesis => {
                depth += 1;
                inner.push(values[j].clone());
            }
            ComponentValue::CloseParenthesis => {
                depth -= 1;
                if depth > 0 {
                    inner.push(values[j].clone());
                }
            }
            t => inner.push(t.clone()),
        }
        j += 1;
    }

    // Split the argument list on top-level commas.
    let mut segments: Vec<Vec<ComponentValue>> = vec![Vec::new()];
    let mut d = 0i32;
    for t in &inner {
        match t {
            ComponentValue::OpenParenthesis => {
                d += 1;
                segments.last_mut().unwrap().push(t.clone());
            }
            ComponentValue::CloseParenthesis => {
                d -= 1;
                segments.last_mut().unwrap().push(t.clone());
            }
            ComponentValue::Delim(',') if d == 0 => segments.push(Vec::new()),
            _ => segments.last_mut().unwrap().push(t.clone()),
        }
    }

    // The first segment may be a direction (angle or `to <side>`).
    let mut angle_deg = 180.0; // CSS default direction: to bottom.
    let mut start = 0;
    if let Some(first) = segments.first() {
        if let Some(a) = parse_gradient_direction(first) {
            angle_deg = a;
            start = 1;
        }
    }

    let stop_segments = &segments[start..];
    let mut stops: Vec<(Color, f64)> = Vec::new();
    let n = stop_segments.len();
    for (idx, seg) in stop_segments.iter().enumerate() {
        let color = parse_color_value(seg)?;
        // Explicit position (%), else evenly distributed.
        let pos = seg
            .iter()
            .find_map(|t| match t {
                ComponentValue::Dimension(v, u) if u == "%" => Some((*v / 100.0) as f64),
                _ => None,
            })
            .unwrap_or(if n <= 1 { 0.0 } else { idx as f64 / (n as f64 - 1.0) });
        stops.push((color, pos.clamp(0.0, 1.0)));
    }
    if stops.len() < 2 {
        return None;
    }
    Some(LinearGradient { angle_deg, stops })
}

fn parse_gradient_direction(seg: &[ComponentValue]) -> Option<f64> {
    // `<angle>deg` (also rad/turn/grad) or `to <side...>`.
    for t in seg {
        match t {
            ComponentValue::Dimension(v, unit) => {
                return Some(match unit.as_str() {
                    "deg" => *v,
                    "rad" => *v * 180.0 / core::f64::consts::PI,
                    "turn" => *v * 360.0,
                    "grad" => *v * 0.9,
                    _ => return None,
                });
            }
            ComponentValue::Ident(k) if k.eq_ignore_ascii_case("to") => {
                let sides: Vec<String> = seg
                    .iter()
                    .filter_map(|t| match t {
                        ComponentValue::Ident(s) if !s.eq_ignore_ascii_case("to") => {
                            Some(s.to_ascii_lowercase())
                        }
                        _ => None,
                    })
                    .collect();
                let has = |k: &str| sides.iter().any(|s| s == k);
                return Some(match (has("top"), has("right"), has("bottom"), has("left")) {
                    (true, false, false, false) => 0.0,
                    (false, true, false, false) => 90.0,
                    (false, false, true, false) => 180.0,
                    (false, false, false, true) => 270.0,
                    (true, true, _, _) => 45.0,
                    (false, true, true, _) => 135.0,
                    (false, false, true, true) => 225.0,
                    (true, false, _, true) => 315.0,
                    _ => 180.0,
                });
            }
            _ => {}
        }
    }
    None
}
