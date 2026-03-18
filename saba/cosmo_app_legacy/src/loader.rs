use crate::model::{AppError, AppResult, FrameRect};
use crate::security::{
    apply_set_cookie_headers, attach_cookie_header, cache_permission_decision,
    classify_tls_policy_error, evaluate_cors_request, evaluate_sandbox_policy, has_tls_exception,
    register_tls_exception, CorsRequest, CredentialsMode,
};
use encoding_rs::{Encoding, SHIFT_JIS, UTF_8};
use regex::Regex;
use reqwest::blocking::Client;
use reqwest::header::{
    HeaderMap, HeaderValue, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, IF_NONE_MATCH,
    LOCATION,
};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use url::Url;

const MAX_REDIRECTS: usize = 10;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const TLS_EXCEPTION_TTL: Duration = Duration::from_secs(15 * 60);

#[derive(Debug, Clone)]
struct CachedEntry {
    etag: Option<String>,
    cache_control: Option<String>,
    html: String,
    final_url: String,
    content_type: Option<String>,
    expires_at: Option<Instant>,
}

#[derive(Debug, Clone)]
struct FetchRequest {
    url: String,
}

#[derive(Debug, Clone)]
struct FetchResponse {
    final_url: String,
    content_type: Option<String>,
    body: Vec<u8>,
    diagnostics: Vec<String>,
    headers: HeaderMap,
}

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
    pub children: Vec<FramesetChild>,
    pub noframes_html: Option<String>,
}

#[derive(Debug)]
pub struct FrameSpec {
    pub name: Option<String>,
    pub src: String,
}

#[derive(Debug)]
pub enum FramesetChild {
    Frame(FrameSpec),
    Frameset(FramesetSpec),
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

    // Spec: TLS profile and certificate verification rely on reqwest/rustls defaults.
    // https://datatracker.ietf.org/doc/html/rfc8446
    let request = FetchRequest {
        url: url.to_string(),
    };
    let mut diagnostics = Vec::new();

    // Spec: RFC 9110 request and redirect semantics.
    // https://www.rfc-editor.org/rfc/rfc9110
    // Spec: RFC 9111 cache validators and freshness model.
    // https://www.rfc-editor.org/rfc/rfc9111
    let response = fetch_with_pipeline(&request, &mut diagnostics)?;

    validate_response_security(
        url,
        &response.final_url,
        &response.headers,
        &mut diagnostics,
    );

    let decoded = decode_html_bytes(&response.body, response.content_type.as_deref());
    store_cache_entry(url, &response, &decoded.html);
    diagnostics.extend(response.diagnostics);
    diagnostics.extend(decoded.diagnostics.clone());

    Ok(LoadedDocument {
        final_url: response.final_url,
        title: extract_title(&decoded.html),
        html: decoded.html,
        diagnostics,
    })
}

// Spec: certificate exception is a user-explicit, origin-scoped temporary override.
// Ref: RFC 5280 validation and browser interstitial exception UX constraints.
// https://www.rfc-editor.org/rfc/rfc5280
pub fn register_tls_exception_for_url(url: &str) -> AppResult<String> {
    register_tls_exception(url, TLS_EXCEPTION_TTL)
}

fn shared_http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .pool_max_idle_per_host(8)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build shared HTTP client")
    })
}

fn insecure_http_client() -> &'static Client {
    static CLIENT: OnceLock<Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        Client::builder()
            .pool_max_idle_per_host(2)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("Failed to build insecure HTTP client")
    })
}

fn select_http_client(url: &str, diagnostics: &mut Vec<String>) -> &'static Client {
    // Spec: certificate errors must fail closed unless a user-scoped exception was
    // explicitly registered via interstitial UX.
    // Ref: RFC 5280 certificate validation + RFC 6797 fail-closed transport intent.
    // https://www.rfc-editor.org/rfc/rfc5280
    // https://www.rfc-editor.org/rfc/rfc6797
    if has_tls_exception(url) {
        diagnostics.push(format!(
            "TLS exception applied for origin while fetching {} (temporary override)",
            url
        ));
        return insecure_http_client();
    }
    shared_http_client()
}

fn response_cache() -> &'static Mutex<HashMap<String, CachedEntry>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedEntry>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn fetch_with_pipeline(
    request: &FetchRequest,
    diagnostics: &mut Vec<String>,
) -> AppResult<FetchResponse> {
    let mut current_url = request.url.clone();
    let mut redirect_count = 0usize;

    loop {
        let cached = lookup_cache_entry(&current_url);
        let client = select_http_client(&current_url, diagnostics);
        let mut builder = client.get(&current_url);
        let mut request_headers = HeaderMap::new();
        if attach_cookie_header(&mut request_headers, &current_url, &current_url) {
            diagnostics.push(format!("attached Cookie header for {}", current_url));
        }
        if let Some(entry) = &cached {
            if let Some(etag) = &entry.etag {
                if let Ok(value) = HeaderValue::from_str(etag) {
                    request_headers.insert(IF_NONE_MATCH, value);
                    diagnostics.push(format!(
                        "cache revalidation with If-None-Match for {current_url}"
                    ));
                }
            }
        }
        builder = builder.headers(request_headers);

        let response = builder.send().map_err(classify_request_error)?;
        let status = response.status();
        let headers = response.headers().clone();
        diagnostics.extend(apply_set_cookie_headers(&headers, &current_url));
        diagnostics.push(format!("HTTP GET {} -> {}", current_url, status));

        if status.is_redirection() {
            if redirect_count >= MAX_REDIRECTS {
                return Err(AppError::network_redirect_loop(format!(
                    "Redirect limit exceeded while fetching {}",
                    request.url
                )));
            }
            let Some(location) = headers.get(LOCATION).and_then(|value| value.to_str().ok()) else {
                return Err(AppError::network(format!(
                    "Redirect response without Location header at {}",
                    current_url
                )));
            };
            let next_url = resolve_url(&current_url, location)?;
            diagnostics.push(format!(
                "redirect followed: {} -> {}",
                current_url, next_url
            ));
            current_url = next_url;
            redirect_count += 1;
            continue;
        }

        if status.as_u16() == 304 {
            if let Some(entry) = cached {
                diagnostics.push(format!(
                    "cache hit with 304 Not Modified for {}",
                    current_url
                ));
                return Ok(FetchResponse {
                    final_url: entry.final_url,
                    content_type: entry.content_type,
                    body: entry.html.into_bytes(),
                    diagnostics: diagnostics.clone(),
                    headers,
                });
            }
        }

        let final_url = response.url().to_string();
        let content_type = headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        if let Some(content_encoding) = headers
            .get(CONTENT_ENCODING)
            .and_then(|value| value.to_str().ok())
        {
            diagnostics.push(format!("content-encoding negotiated: {content_encoding}"));
        }
        let body = response.bytes().map_err(classify_request_error)?.to_vec();

        return Ok(FetchResponse {
            final_url,
            content_type,
            body,
            diagnostics: diagnostics.clone(),
            headers,
        });
    }
}

fn lookup_cache_entry(url: &str) -> Option<CachedEntry> {
    let cache = response_cache().lock().ok()?;
    let entry = cache.get(url)?.clone();
    if entry.cache_control.as_deref().is_some_and(is_no_store) {
        return None;
    }
    if let Some(expires) = entry.expires_at {
        if Instant::now() > expires {
            return None;
        }
    }
    Some(entry)
}

fn store_cache_entry(original_url: &str, response: &FetchResponse, html: &str) {
    let etag = response
        .headers
        .get(ETAG)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let cache_control = response
        .headers
        .get(CACHE_CONTROL)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);

    if cache_control.as_deref().is_some_and(is_no_store) {
        return;
    }

    let entry = CachedEntry {
        etag,
        cache_control: cache_control.clone(),
        html: html.to_string(),
        final_url: response.final_url.clone(),
        content_type: response.content_type.clone(),
        expires_at: cache_control
            .as_deref()
            .and_then(cache_ttl_from_cache_control)
            .map(|ttl| Instant::now() + ttl),
    };

    if let Ok(mut cache) = response_cache().lock() {
        cache.insert(original_url.to_string(), entry.clone());
        if original_url != response.final_url {
            cache.insert(response.final_url.clone(), entry);
        }
    }
}

fn cache_ttl_from_cache_control(value: &str) -> Option<Duration> {
    value
        .split(',')
        .map(str::trim)
        .find_map(|directive| directive.strip_prefix("max-age="))
        .and_then(|seconds| seconds.parse::<u64>().ok())
        .map(Duration::from_secs)
}

fn is_no_store(value: &str) -> bool {
    value
        .split(',')
        .map(str::trim)
        .any(|directive| directive.eq_ignore_ascii_case("no-store"))
}

fn validate_response_security(
    initiator_url: &str,
    final_url: &str,
    headers: &HeaderMap,
    diagnostics: &mut Vec<String>,
) {
    let cors_request = CorsRequest {
        initiator_url,
        target_url: final_url,
        method: "GET",
        request_headers: Vec::new(),
        credentials_mode: CredentialsMode::Omit,
    };
    if let Err(error) = evaluate_cors_request(&cors_request, headers, None) {
        diagnostics.push(format!(
            "CORS evaluation blocked cross-origin read: {} -> {} ({})",
            initiator_url, final_url, error.code
        ));
    }

    let sandbox = evaluate_sandbox_policy(initiator_url, final_url);
    cache_permission_decision(initiator_url, final_url, sandbox.allowed);
    diagnostics.extend(sandbox.diagnostics);
}

// Spec: HTML Standard obsolete frames features and the `in frameset` parsing mode.
// https://html.spec.whatwg.org/multipage/obsolete.html
// https://html.spec.whatwg.org/multipage/parsing.html
pub fn parse_frameset_document(html: &str) -> Option<FramesetSpec> {
    let opening = frameset_open_regex().find(html)?;
    let (frameset, _) = parse_frameset_at(html, opening.start())?;
    Some(frameset)
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

// Spec: RFC 3986 relative reference resolution.
// https://datatracker.ietf.org/doc/html/rfc3986#section-5
pub fn resolve_url(base_url: &str, target: &str) -> AppResult<String> {
    let base = Url::parse(base_url)
        .map_err(|error| AppError::validation(format!("Invalid base URL: {error}")))?;
    let resolved = base
        .join(target)
        .map_err(|error| AppError::validation(format!("Failed to resolve URL: {error}")))?;
    Ok(resolved.to_string())
}

// Spec: HTML Living Standard, determining the character encoding.
// https://html.spec.whatwg.org/multipage/parsing.html#determining-the-character-encoding
// Spec: HTML Living Standard, meta charset.
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

// Spec: HTML Living Standard, the document base URL and the `iframe srcdoc` document model.
// https://html.spec.whatwg.org/multipage/urls-and-fetching.html#document-base-url
// https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-srcdoc
pub fn prepare_html_for_display(html: &str, base_url: &str, frame_id: &str) -> String {
    let base_tag = format!("<base href=\"{}\">", escape_html_attr(base_url));
    let navigation_script = format!(
        "<script>(function(){{document.addEventListener('click',function(event){{var anchor=event.target&&event.target.closest?event.target.closest('a'):null;if(!anchor)return;if(event.defaultPrevented||event.button!==0||event.metaKey||event.ctrlKey||event.shiftKey||event.altKey)return;var href=anchor.href||anchor.getAttribute('href');if(!href)return;event.preventDefault();window.parent.postMessage({{type:'cosmobrowse:navigate',frameId:'{}',href:href,target:anchor.getAttribute('target')||''}},'*');}});}})();</script>",
        escape_js_string(frame_id)
    );
    let payload = format!("{base_tag}{navigation_script}");

    if let Some(index) = find_head_close_index(html) {
        let mut output = String::with_capacity(html.len() + payload.len());
        output.push_str(&html[..index]);
        output.push_str(&payload);
        output.push_str(&html[index..]);
        return output;
    }

    if let Some(index) = find_html_open_end_index(html) {
        let mut output = String::with_capacity(html.len() + payload.len() + "<head></head>".len());
        output.push_str(&html[..index]);
        output.push_str("<head>");
        output.push_str(&payload);
        output.push_str("</head>");
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
        "fixture://legacy_frames/nested" => (
            "fixture://legacy_frames/nested".to_string(),
            include_str!("../../testdata/legacy_frames/nested.htm").to_string(),
        ),
        "fixture://legacy_frames/menu" | "fixture://legacy_frames/menu.htm" => (
            "fixture://legacy_frames/menu".to_string(),
            include_str!("../../testdata/legacy_frames/menu.htm").to_string(),
        ),
        "fixture://legacy_frames/top" | "fixture://legacy_frames/top.htm" => (
            "fixture://legacy_frames/top".to_string(),
            include_str!("../../testdata/legacy_frames/top.htm").to_string(),
        ),
        "fixture://legacy_frames/prof" | "fixture://legacy_frames/prof.htm" => (
            "fixture://legacy_frames/prof".to_string(),
            include_str!("../../testdata/legacy_frames/prof.htm").to_string(),
        ),
        "fixture://legacy_frames/noframes" => (
            "fixture://legacy_frames/noframes".to_string(),
            include_str!("../../testdata/legacy_frames/noframes.htm").to_string(),
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
    if error.is_timeout() {
        return AppError::network_timeout(message);
    }
    if error.is_decode() {
        return AppError::network_content_decoding(message);
    }
    classify_tls_policy_error(&message)
}

fn extract_charset_from_content_type(content_type: &str) -> Option<String> {
    content_type.split(';').map(str::trim).find_map(|part| {
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

fn parse_frameset_at(html: &str, start_index: usize) -> Option<(FramesetSpec, usize)> {
    let remaining = &html[start_index..];
    let opening = frameset_open_regex().find(remaining)?;
    if opening.start() != 0 {
        return None;
    }

    let opening_html = &remaining[..opening.end()];
    let attrs = frameset_open_regex()
        .captures(opening_html)?
        .name("attrs")?
        .as_str();
    let attr_map = parse_attrs(attrs);
    let cols = attr_map.get("cols").map(|value| parse_track_list(value));
    let rows = attr_map.get("rows").map(|value| parse_track_list(value));
    let mut children = Vec::new();
    let mut noframes_html = None;
    let mut cursor = start_index + opening.end();

    while cursor < html.len() {
        let segment = &html[cursor..];
        let captures = frameset_token_regex().captures(segment)?;
        let matched = captures.get(0)?;
        cursor += matched.start();
        let is_closing = captures.name("closing").is_some();
        let tag = captures.name("tag")?.as_str().to_ascii_lowercase();

        match (is_closing, tag.as_str()) {
            (true, "frameset") => {
                cursor += matched.as_str().len();
                return Some((
                    FramesetSpec {
                        cols,
                        rows,
                        children,
                        noframes_html,
                    },
                    cursor,
                ));
            }
            (false, "frame") => {
                let attr_map = parse_attrs(captures.name("attrs")?.as_str());
                if let Some(src) = attr_map.get("src").cloned() {
                    children.push(FramesetChild::Frame(FrameSpec {
                        name: attr_map.get("name").cloned(),
                        src,
                    }));
                }
                cursor += matched.as_str().len();
            }
            (false, "frameset") => {
                let (nested, next_cursor) = parse_frameset_at(html, cursor)?;
                children.push(FramesetChild::Frameset(nested));
                cursor = next_cursor;
            }
            (false, "noframes") => {
                let start = cursor + matched.as_str().len();
                let closing = noframes_close_regex().find(&html[start..])?;
                let end = start + closing.start();
                let fallback = html[start..end].trim();
                if !fallback.is_empty() {
                    noframes_html = Some(fallback.to_string());
                }
                cursor = start + closing.end();
            }
            _ => {
                cursor += matched.as_str().len();
            }
        }
    }

    None
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
        Regex::new(r"(?is)<title[^>]*>(?P<title>.*?)</title>").expect("valid title regex")
    })
}

fn frameset_open_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?is)<frameset\b(?P<attrs>[^>]*)>").expect("valid frameset opening regex")
    })
}

fn frameset_token_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?is)<(?P<closing>/)?(?P<tag>frameset|frame|noframes)\b(?P<attrs>[^>]*)>")
            .expect("valid frameset token regex")
    })
}

fn noframes_close_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?is)</noframes\s*>").expect("valid noframes closing regex"))
}

fn attr_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?is)(?P<name>[a-z_:][-a-z0-9_:.]*)\s*=\s*(?:\"(?P<double>[^\"]*)\"|'(?P<single>[^']*)'|(?P<bare>[^\s>]+))"#)
            .expect("valid attr regex")
    })
}

fn tag_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?is)<[^>]+>").expect("valid tag regex"))
}

fn head_close_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?is)</head\s*>").expect("valid head closing regex"))
}

fn html_open_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| Regex::new(r"(?is)<html\b[^>]*>").expect("valid html opening regex"))
}

fn find_head_close_index(html: &str) -> Option<usize> {
    head_close_regex().find(html).map(|matched| matched.start())
}

fn find_html_open_end_index(html: &str) -> Option<usize> {
    html_open_regex().find(html).map(|matched| matched.end())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_html_for_display_injects_before_case_insensitive_head_close() {
        let html = "<html><HEAD><title>Example</title></HEAD><body><a href=\"next.html\">Next</a></body></html>";
        let prepared =
            prepare_html_for_display(html, "https://example.com/root/index.html", "root/right");

        assert!(prepared.contains("<base href=\"https://example.com/root/index.html\">"));
        assert!(prepared.contains("frameId:'root/right'"));
        assert!(prepared.contains("</HEAD>"));
        assert!(
            prepared.find("<base href=").expect("base tag should exist")
                < prepared.find("</HEAD>").expect("closing HEAD should exist")
        );
    }

    #[test]
    fn prepare_html_for_display_creates_head_inside_html_when_missing() {
        let html = "<html><body>Example</body></html>";
        let prepared =
            prepare_html_for_display(html, "https://example.com/root/index.html", "root");

        assert!(
            prepared.starts_with("<html><head><base href=\"https://example.com/root/index.html\">")
        );
        assert!(prepared.contains("</head><body>Example</body></html>"));
    }

    #[test]
    fn parse_frameset_document_preserves_noframes_fallback() {
        let html = "<html><frameset cols=\"50,50\"><noframes><body><p>Fallback</p></body></noframes></frameset></html>";
        let frameset = parse_frameset_document(html).expect("frameset should parse");

        assert_eq!(frameset.children.len(), 0);
        assert_eq!(
            frameset.noframes_html.as_deref(),
            Some("<body><p>Fallback</p></body>")
        );
    }

    #[test]
    fn parse_frameset_document_supports_nested_framesets() {
        let html = "<html><frameset cols=\"20,80\"><frame src=\"menu.htm\" name=\"left\"><frameset rows=\"40,60\"><frame src=\"top.htm\" name=\"upper\"><frame src=\"prof.htm\" name=\"lower\"></frameset></frameset></html>";
        let frameset = parse_frameset_document(html).expect("frameset should parse");

        assert_eq!(frameset.children.len(), 2);
        assert!(matches!(
            &frameset.children[0],
            FramesetChild::Frame(FrameSpec {
                name: Some(name),
                src
            }) if name == "left" && src == "menu.htm"
        ));
        match &frameset.children[1] {
            FramesetChild::Frameset(nested) => assert_eq!(nested.children.len(), 2),
            FramesetChild::Frame(_) => panic!("expected nested frameset child"),
        }
    }

    #[test]
    fn cache_control_parser_extracts_max_age() {
        let ttl = cache_ttl_from_cache_control("public, max-age=120, must-revalidate")
            .expect("ttl should parse");
        assert_eq!(ttl.as_secs(), 120);
    }

    #[test]
    fn cache_control_parser_detects_no_store() {
        assert!(is_no_store("private, no-store"));
        assert!(!is_no_store("max-age=60"));
    }
}
