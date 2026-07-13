//! Media queries: prelude AST + evaluation against a `MediaContext`
//! (plan 1.1/1.2). Parsed from the token run between `@media` and `{`.
//!
//! Supported: media types (all/screen/print), `not`/`only` prefixes,
//! comma-separated query lists (OR), `and`-chained features, and the
//! features (min-/max-)width/height, orientation, prefers-color-scheme.
//! Unknown features poison their query to non-matching ("not all"), never
//! the whole list.

use crate::renderer::css::token::CssToken;

/// The environment a stylesheet is being evaluated in.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaContext {
    pub viewport_width: f64,
    pub viewport_height: f64,
    pub prefers_dark: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaQueryList {
    pub queries: Vec<MediaQuery>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaQuery {
    pub negated: bool,
    /// `None` = no type given (features only) — treated as `all`.
    pub media_type: Option<String>,
    pub features: Vec<MediaFeature>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MediaFeature {
    MinWidth(f64),
    MaxWidth(f64),
    Width(f64),
    MinHeight(f64),
    MaxHeight(f64),
    Height(f64),
    Orientation(bool), // true = landscape
    PrefersDark(bool), // true = dark requested by the page
    /// Unrecognized feature: the containing query never matches.
    Unknown,
}

impl MediaQueryList {
    /// An empty prelude (`@media { }`) matches everything.
    pub fn matches(&self, ctx: &MediaContext) -> bool {
        if self.queries.is_empty() {
            return true;
        }
        self.queries.iter().any(|q| q.matches(ctx))
    }
}

impl MediaQuery {
    fn matches(&self, ctx: &MediaContext) -> bool {
        let mut result = match self.media_type.as_deref() {
            None | Some("all") | Some("screen") => true,
            _ => false, // print, speech, unknown types
        };
        if result {
            for f in &self.features {
                let ok = match f {
                    MediaFeature::MinWidth(v) => ctx.viewport_width >= *v,
                    MediaFeature::MaxWidth(v) => ctx.viewport_width <= *v,
                    MediaFeature::Width(v) => ctx.viewport_width == *v,
                    MediaFeature::MinHeight(v) => ctx.viewport_height >= *v,
                    MediaFeature::MaxHeight(v) => ctx.viewport_height <= *v,
                    MediaFeature::Height(v) => ctx.viewport_height == *v,
                    MediaFeature::Orientation(landscape) => {
                        (ctx.viewport_width >= ctx.viewport_height) == *landscape
                    }
                    MediaFeature::PrefersDark(dark) => ctx.prefers_dark == *dark,
                    MediaFeature::Unknown => false,
                };
                if !ok {
                    result = false;
                    break;
                }
            }
        }
        if self.negated {
            !result
        } else {
            result
        }
    }
}

/// Resolve a `<length>` in a media feature to px. Media queries resolve
/// em against the initial font size (16px), never the element font size.
fn length_to_px(value: f64, unit: &str) -> Option<f64> {
    match unit {
        "px" | "" => Some(value),
        "em" | "rem" => Some(value * 16.0),
        _ => None,
    }
}

/// Parse one feature's value tokens (after the `:`) plus its name into a
/// MediaFeature.
fn feature_from(name: &str, value: &[CssToken]) -> MediaFeature {
    let number = value.iter().find_map(|t| match t {
        CssToken::Number(n) => Some((*n, String::new())),
        CssToken::Dimension(n, u) => Some((*n, u.clone())),
        _ => None,
    });
    let ident = value.iter().find_map(|t| match t {
        CssToken::Ident(s) => Some(s.to_ascii_lowercase()),
        _ => None,
    });
    match name {
        "min-width" | "max-width" | "width" | "min-height" | "max-height" | "height" => {
            let px = number.and_then(|(v, u)| length_to_px(v, &u));
            match (name, px) {
                ("min-width", Some(v)) => MediaFeature::MinWidth(v),
                ("max-width", Some(v)) => MediaFeature::MaxWidth(v),
                ("width", Some(v)) => MediaFeature::Width(v),
                ("min-height", Some(v)) => MediaFeature::MinHeight(v),
                ("max-height", Some(v)) => MediaFeature::MaxHeight(v),
                ("height", Some(v)) => MediaFeature::Height(v),
                _ => MediaFeature::Unknown,
            }
        }
        "orientation" => match ident.as_deref() {
            Some("landscape") => MediaFeature::Orientation(true),
            Some("portrait") => MediaFeature::Orientation(false),
            _ => MediaFeature::Unknown,
        },
        "prefers-color-scheme" => match ident.as_deref() {
            Some("dark") => MediaFeature::PrefersDark(true),
            Some("light") => MediaFeature::PrefersDark(false),
            _ => MediaFeature::Unknown,
        },
        _ => MediaFeature::Unknown,
    }
}

/// Parse the token run of an `@media` prelude into a query list.
pub fn parse_media_query_list(tokens: &[CssToken]) -> MediaQueryList {
    let mut queries = Vec::new();
    for part in split_on_commas(tokens) {
        queries.push(parse_single_query(&part));
    }
    MediaQueryList { queries }
}

fn split_on_commas(tokens: &[CssToken]) -> Vec<Vec<CssToken>> {
    let mut out = Vec::new();
    let mut cur = Vec::new();
    let mut depth = 0i32;
    for t in tokens {
        match t {
            CssToken::OpenParenthesis => {
                depth += 1;
                cur.push(t.clone());
            }
            CssToken::CloseParenthesis => {
                depth -= 1;
                cur.push(t.clone());
            }
            CssToken::Delim(',') if depth == 0 => {
                out.push(core::mem::take(&mut cur));
            }
            _ => cur.push(t.clone()),
        }
    }
    if !cur.iter().all(|t| matches!(t, CssToken::Whitespace)) || out.is_empty() {
        out.push(cur);
    }
    out
}

fn parse_single_query(tokens: &[CssToken]) -> MediaQuery {
    let mut negated = false;
    let mut media_type: Option<String> = None;
    let mut features = Vec::new();

    let mut i = 0;
    while i < tokens.len() {
        match &tokens[i] {
            CssToken::Whitespace => i += 1,
            CssToken::Ident(word) => {
                let w = word.to_ascii_lowercase();
                match w.as_str() {
                    "not" => negated = true,
                    "only" | "and" => {}
                    _ => media_type = Some(w),
                }
                i += 1;
            }
            CssToken::OpenParenthesis => {
                // Collect the parenthesized feature: name [: value].
                let mut j = i + 1;
                let mut depth = 1;
                let mut inner = Vec::new();
                while j < tokens.len() && depth > 0 {
                    match &tokens[j] {
                        CssToken::OpenParenthesis => {
                            depth += 1;
                            inner.push(tokens[j].clone());
                        }
                        CssToken::CloseParenthesis => {
                            depth -= 1;
                            if depth > 0 {
                                inner.push(tokens[j].clone());
                            }
                        }
                        t => inner.push(t.clone()),
                    }
                    j += 1;
                }
                features.push(parse_feature(&inner));
                i = j;
            }
            _ => i += 1,
        }
    }

    MediaQuery {
        negated,
        media_type,
        features,
    }
}

fn parse_feature(inner: &[CssToken]) -> MediaFeature {
    // Expect: Ident(name) [ ':' value... ]
    let mut name = String::new();
    let mut value = Vec::new();
    let mut seen_colon = false;
    for t in inner {
        match t {
            CssToken::Whitespace => {}
            CssToken::Colon => seen_colon = true,
            CssToken::Ident(s) if !seen_colon && name.is_empty() => {
                name = s.to_ascii_lowercase();
            }
            t if seen_colon => value.push(t.clone()),
            _ => return MediaFeature::Unknown, // range syntax etc.
        }
    }
    if name.is_empty() {
        return MediaFeature::Unknown;
    }
    feature_from(&name, &value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::css::token::CssTokenizer;

    fn toks(s: &str) -> Vec<CssToken> {
        let mut t = CssTokenizer::new(s.to_string());
        let mut out = Vec::new();
        while let Some(tok) = t.next() {
            out.push(tok);
        }
        out
    }

    const DESKTOP: MediaContext = MediaContext {
        viewport_width: 1024.0,
        viewport_height: 768.0,
        prefers_dark: false,
    };
    const PHONE: MediaContext = MediaContext {
        viewport_width: 390.0,
        viewport_height: 800.0,
        prefers_dark: false,
    };

    #[test]
    fn max_width_matches_by_viewport() {
        let q = parse_media_query_list(&toks("(max-width: 600px)"));
        assert!(!q.matches(&DESKTOP));
        assert!(q.matches(&PHONE));
    }

    #[test]
    fn type_and_feature_chain() {
        let q = parse_media_query_list(&toks("screen and (min-width: 800px) and (max-width: 1200px)"));
        assert!(q.matches(&DESKTOP));
        assert!(!q.matches(&PHONE));
        let p = parse_media_query_list(&toks("print"));
        assert!(!p.matches(&DESKTOP));
    }

    #[test]
    fn comma_list_is_or_and_not_inverts() {
        let q = parse_media_query_list(&toks("print, (max-width: 500px)"));
        assert!(!q.matches(&DESKTOP));
        assert!(q.matches(&PHONE));
        let n = parse_media_query_list(&toks("not screen"));
        assert!(!n.matches(&DESKTOP));
        let np = parse_media_query_list(&toks("not print"));
        assert!(np.matches(&DESKTOP));
    }

    #[test]
    fn prefers_color_scheme_and_orientation() {
        let dark_ctx = MediaContext {
            prefers_dark: true,
            ..DESKTOP
        };
        let q = parse_media_query_list(&toks("(prefers-color-scheme: dark)"));
        assert!(q.matches(&dark_ctx));
        assert!(!q.matches(&DESKTOP));
        let o = parse_media_query_list(&toks("(orientation: landscape)"));
        assert!(o.matches(&DESKTOP)); // 1024x768
        assert!(!o.matches(&PHONE)); // 390x800
    }

    #[test]
    fn unknown_feature_poisons_only_its_query() {
        let q = parse_media_query_list(&toks("(pointer: fine), (max-width: 2000px)"));
        assert!(q.matches(&DESKTOP)); // second query still matches
        let solo = parse_media_query_list(&toks("(pointer: fine)"));
        assert!(!solo.matches(&DESKTOP));
    }

    #[test]
    fn em_lengths_resolve_against_16px() {
        let q = parse_media_query_list(&toks("(max-width: 40em)")); // 640px
        assert!(!q.matches(&DESKTOP));
        assert!(q.matches(&PHONE));
    }
}
