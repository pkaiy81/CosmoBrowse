use crate::model::{AppError, AppResult, FrameRect};
use encoding_rs::{Encoding, SHIFT_JIS, UTF_8};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::CONTENT_TYPE;
use std::collections::HashMap;
use std::sync::OnceLock;
use url::Url;

#[derive(Debug)]
pub struct LoadedDocument {
    pub final_url: String,
    pub html: String,
    pub title: Option<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug)]
pub struct FramesetSpec {
    pub cols: Option<Vec<TrackSpec>>,
    pub rows: Option<Vec<TrackSpec>>,
    pub frames: Vec<FrameSpec>,
}

#[derive(Debug)]
pub struct FrameSpec {
    pub name: Option<String>,
    pub src: String,
}

#[derive(Debug, Clone)]
pub enum TrackSpec {
    Percent(f64),
    Star(f64),
    Raw(f64),
}

pub fn fetch_document(url: &str) -> AppResult<LoadedDocument> {
    if let Some(document) = load_fixture_document(url) {
        return Ok(document);
    }

    let client = Client::builder()
        .build()
        .map_err(|error| AppError::network(format!("Failed to build HTTP client: {error}")))?;

    let response = client.get(url).send().map_err(classify_request_error)?;
    let final_url = response.url().to_string();
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = response
        .bytes()
        .map_err(|error| AppError::network(format!("Failed to read response body: {error}")))?;

    let decoded = decode_html_bytes(&bytes, content_type.as_deref());
    Ok(LoadedDocument {
        final_url,
        title: extract_title(&decoded.html),
        html: decoded.html,
        diagnostics: decoded.diagnostics,
    })
}

pub fn parse_frameset_document(html: &str) -> Option<FramesetSpec> {
    let captures = frameset_regex().captures(html)?;
    let attrs = captures.name("attrs")?.as_str();
    let body = captures.name("body")?.as_str();
    let attr_map = parse_attrs(attrs);
    let cols = attr_map.get("cols").map(|value| parse_track_list(value));
    let rows = attr_map.get("rows").map(|value| parse_track_list(value));
    let mut frames = Vec::new();

    for captures in frame_regex().captures_iter(body) {
        let Some(attrs) = captures.name("attrs") else {
            continue;
        };
        let attr_map = parse_attrs(attrs.as_str());
        let Some(src) = attr_map.get("src").cloned() else {
            continue;
        };
        frames.push(FrameSpec {
            name: attr_map.get("name").cloned(),
            src,
        });
    }

    if frames.is_empty() {
        return None;
    }

    Some(FramesetSpec { cols, rows, frames })
}

impl FramesetSpec {
    pub fn child_rects(&self, parent: &FrameRect) -> Vec<FrameRect> {
        if let Some(cols) = &self.cols {
            let widths = resolve_tracks(cols, parent.width);
            let mut x = parent.x;
            return widths
                .into_iter()
                .map(|width| {
                    let rect = FrameRect {
                        x,
                        y: parent.y,
                        width,
                        height: parent.height,
                    };
                    x += width;
                    rect
                })
                .collect();
        }

        if let Some(rows) = &self.rows {
            let heights = resolve_tracks(rows, parent.height);
            let mut y = parent.y;
            return heights
                .into_iter()
                .map(|height| {
                    let rect = FrameRect {
                        x: parent.x,
                        y,
                        width: parent.width,
                        height,
                    };
                    y += height;
                    rect
                })
                .collect();
        }

        vec![parent.clone()]
    }
}

// Ref: RFC 3986 relative reference resolution.
// https://datatracker.ietf.org/doc/html/rfc3986#section-5
pub fn resolve_url(base_url: &str, target: &str) -> AppResult<String> {
    let base = Url::parse(base_url)
        .map_err(|error| AppError::validation(format!("Invalid base URL: {error}")))?;
    let resolved = base
        .join(target)
        .map_err(|error| AppError::validation(format!("Failed to resolve URL: {error}")))?;
    Ok(resolved.to_string())
}

// Ref: HTML Living Standard, determining the character encoding.
// https://html.spec.whatwg.org/multipage/parsing.html#determining-the-character-encoding
// Ref: HTML Living Standard, meta charset.
// https://html.spec.whatwg.org/multipage/semantics.html#attr-meta-charset
pub fn decode_html_bytes(bytes: &[u8], content_type: Option<&str>) -> DecodedDocument {
    let mut diagnostics = Vec::new();
    let header_charset = content_type.and_then(extract_charset_from_content_type);
    let meta_charset = sniff_charset_from_html(bytes);
    let charset = header_charset
        .clone()
        .or(meta_charset)
        .unwrap_or_else(|| "utf-8".to_string());
    let encoding = Encoding::for_label(charset.as_bytes()).unwrap_or(UTF_8);
    let (decoded, _, had_errors) = encoding.decode(bytes);

    if had_errors {
        diagnostics.push(format!(
            "Decoded HTML with replacement characters using charset {charset}"
        ));
    }
    if encoding == SHIFT_JIS {
        diagnostics.push("Decoded HTML as Shift_JIS".to_string());
    }

    DecodedDocument {
        html: decoded.into_owned(),
        diagnostics,
    }
}

pub struct DecodedDocument {
    pub html: String,
    pub diagnostics: Vec<String>,
}

pub fn extract_title(html: &str) -> Option<String> {
    title_regex()
        .captures(html)
        .and_then(|caps| caps.name("title"))
        .map(|title| strip_tags(title.as_str()).trim().to_string())
        .filter(|title| !title.is_empty())
}

pub fn prepare_html_for_display(html: &str, base_url: &str, frame_id: &str) -> String {
    let base_tag = format!("<base href=\"{}\">", escape_html_attr(base_url));
    let navigation_script = format!(
        "<script>(function(){{document.addEventListener('click',function(event){{var anchor=event.target&&event.target.closest?event.target.closest('a'):null;if(!anchor)return;if(event.defaultPrevented||event.button!==0||event.metaKey||event.ctrlKey||event.shiftKey||event.altKey)return;var href=anchor.href||anchor.getAttribute('href');if(!href)return;event.preventDefault();window.parent.postMessage({{type:'cosmobrowse:navigate',frameId:'{}',href:href,target:anchor.getAttribute('target')||''}},'*');}});}})();</script>",
        escape_js_string(frame_id)
    );
    let payload = format!("{base_tag}{navigation_script}");

    if let Some(index) = html.to_lowercase().find("</head>") {
        let mut output = String::with_capacity(html.len() + payload.len());
        output.push_str(&html[..index]);
        output.push_str(&payload);
        output.push_str(&html[index..]);
        return output;
    }

    format!("<head>{payload}</head>{html}")
}

pub fn build_frame_id(parent_id: &str, frame_name: Option<&str>, index: usize) -> String {
    match frame_name.map(str::trim).filter(|name| !name.is_empty()) {
        Some(name) => format!("{parent_id}/{name}"),
        None => format!("{parent_id}/frame-{index}"),
    }
}

pub fn load_fixture_document(url: &str) -> Option<LoadedDocument> {
    let (final_url, html) = match url {
        "fixture://abehiroshi/index" => (
            "fixture://abehiroshi/index".to_string(),
            include_str!("../../testdata/abehiroshi/index.htm").to_string(),
        ),
        "fixture://abehiroshi/menu" | "fixture://abehiroshi/menu.htm" => (
            "fixture://abehiroshi/menu".to_string(),
            include_str!("../../testdata/abehiroshi/menu.htm").to_string(),
        ),
        "fixture://abehiroshi/top" | "fixture://abehiroshi/top.htm" => (
            "fixture://abehiroshi/top".to_string(),
            include_str!("../../testdata/abehiroshi/top.htm").to_string(),
        ),
        "fixture://abehiroshi/prof" | "fixture://abehiroshi/prof/prof.htm" => (
            "fixture://abehiroshi/prof".to_string(),
            include_str!("../../testdata/abehiroshi/prof/prof.htm").to_string(),
        ),
        _ => return None,
    };

    Some(LoadedDocument {
        title: extract_title(&html),
        final_url,
        html,
        diagnostics: vec!["Loaded fixture document".to_string()],
    })
}

fn classify_request_error(error: reqwest::Error) -> AppError {
    let message = format!("Failed to fetch URL: {error}");
    if message.to_lowercase().contains("certificate") || message.to_lowercase().contains("tls") {
        return AppError::tls(message);
    }
    AppError::network(message)
}

fn extract_charset_from_content_type(content_type: &str) -> Option<String> {
    content_type
        .split(';')
        .map(str::trim)
        .find_map(|part| {
            part.strip_prefix("charset=")
                .map(|value| value.trim_matches('"').trim().to_string())
        })
}

fn sniff_charset_from_html(bytes: &[u8]) -> Option<String> {
    let prefix = String::from_utf8_lossy(&bytes[..bytes.len().min(4096)]).to_lowercase();
    if let Some(index) = prefix.find("charset=") {
        let rest = &prefix[index + "charset=".len()..];
        let end = rest
            .find(|ch: char| matches!(ch, '"' | '\'' | ' ' | '>' | ';'))
            .unwrap_or(rest.len());
        let candidate = rest[..end]
            .trim_matches('"')
            .trim_matches('\'')
            .trim()
            .to_string();
        if !candidate.is_empty() {
            return Some(candidate);
        }
    }
    None
}

fn parse_track_list(value: &str) -> Vec<TrackSpec> {
    value
        .split(',')
        .filter_map(|part| {
            let trimmed = part.trim();
            if trimmed.is_empty() {
                return None;
            }
            if let Some(value) = trimmed.strip_suffix('%') {
                return value.parse::<f64>().ok().map(TrackSpec::Percent);
            }
            if let Some(value) = trimmed.strip_suffix('*') {
                let weight = if value.trim().is_empty() {
                    1.0
                } else {
                    value.trim().parse::<f64>().ok()?
                };
                return Some(TrackSpec::Star(weight));
            }
            trimmed.parse::<f64>().ok().map(TrackSpec::Raw)
        })
        .collect()
}

fn resolve_tracks(specs: &[TrackSpec], total: i64) -> Vec<i64> {
    let raw_sum = specs
        .iter()
        .filter_map(|spec| match spec {
            TrackSpec::Raw(value) => Some(*value),
            _ => None,
        })
        .sum::<f64>();
    let raw_as_percent = raw_sum > 0.0 && (raw_sum - 100.0).abs() < f64::EPSILON;
    let mut resolved = Vec::with_capacity(specs.len());
    let mut fixed = 0i64;
    let mut star_total = 0.0;

    for spec in specs {
        match spec {
            TrackSpec::Percent(value) => {
                let px = ((total as f64) * (*value / 100.0)).round() as i64;
                fixed += px;
                resolved.push(px);
            }
            TrackSpec::Raw(value) if raw_as_percent => {
                let px = ((total as f64) * (*value / 100.0)).round() as i64;
                fixed += px;
                resolved.push(px);
            }
            TrackSpec::Raw(value) => {
                let px = *value as i64;
                fixed += px;
                resolved.push(px);
            }
            TrackSpec::Star(weight) => {
                star_total += *weight;
                resolved.push(-1);
            }
        }
    }

    if star_total > 0.0 {
        let remaining = (total - fixed).max(0);
        for (index, spec) in specs.iter().enumerate() {
            if let TrackSpec::Star(weight) = spec {
                resolved[index] = ((remaining as f64) * (*weight / star_total)).round() as i64;
            }
        }
    }

    let sum = resolved.iter().sum::<i64>();
    if let Some(last) = resolved.last_mut() {
        *last += total - sum;
    }

    resolved.into_iter().map(|value| value.max(0)).collect()
}

fn parse_attrs(attrs: &str) -> HashMap<String, String> {
    let mut attr_map = HashMap::new();
    for captures in attr_regex().captures_iter(attrs) {
        let Some(name) = captures.name("name") else {
            continue;
        };
        let value = captures
            .name("double")
            .or_else(|| captures.name("single"))
            .or_else(|| captures.name("bare"))
            .map(|value| value.as_str().to_string())
            .unwrap_or_default();
        attr_map.insert(name.as_str().to_lowercase(), value);
    }
    attr_map
}

fn strip_tags(input: &str) -> String {
    tag_regex().replace_all(input, " ").to_string()
}

fn escape_html_attr(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('"', "&quot;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn escape_js_string(input: &str) -> String {
    input.replace('\\', "\\\\").replace('\'', "\\'")
}

fn title_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?is)<title[^>]*>(?P<title>.*?)</title>")
            .expect("valid title regex")
    })
}

fn frameset_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?is)<frameset\b(?P<attrs>[^>]*)>(?P<body>.*?)</frameset>")
            .expect("valid frameset regex")
    })
}

fn frame_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?is)<frame\b(?P<attrs>[^>]*)>").expect("valid frame regex")
    })
}

fn attr_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?is)(?P<name>[a-z_:][-a-z0-9_:.]*)\s*=\s*(?:"(?P<double>[^"]*)"|'(?P<single>[^']*)'|(?P<bare>[^\s>]+))"#)
            .expect("valid attr regex")
    })
}

fn tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?is)<[^>]+>").expect("valid tag regex"))
}


