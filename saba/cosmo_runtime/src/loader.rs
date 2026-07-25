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
    HeaderMap, HeaderValue, CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_ENCODING, CONTENT_TYPE,
    ETAG, IF_NONE_MATCH, LOCATION,
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

    if is_attachment_response(&response.headers) {
        return Err(AppError::download_required(format!(
            "The response requested download handling via Content-Disposition attachment for {}. Use the explicit download pipeline instead.",
            response.final_url
        )));
    }

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

        if status.is_redirection() && status.as_u16() != 304 {
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
                // Cached HTML is already decoded to UTF-8, so force charset=utf-8
                // to prevent double-decoding (e.g. Shift_JIS → UTF-8 → Shift_JIS mojibake).
                return Ok(FetchResponse {
                    final_url: entry.final_url,
                    content_type: Some("text/html; charset=utf-8".to_string()),
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

fn is_attachment_response(headers: &HeaderMap) -> bool {
    // Spec: RFC 9110 defines `Content-Disposition: attachment` as representation
    // metadata that instructs user agents to treat the response as a download rather
    // than inline navigation content. The navigation pipeline therefore rejects it
    // and lets the explicit download manager own persistence and progress UI.
    // https://www.rfc-editor.org/rfc/rfc9110.html#field.content-disposition
    headers
        .get(CONTENT_DISPOSITION)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|value| value.to_ascii_lowercase().starts_with("attachment"))
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

/// Width of the visual separator drawn between frameset panes.
/// Matches the default HTML frameset border width used by Netscape/IE.
pub const FRAMESET_BORDER_WIDTH: i64 = 6;

impl FramesetSpec {
    pub fn child_rects(&self, parent: &FrameRect) -> Vec<FrameRect> {
        if let Some(cols) = &self.cols {
            let n_borders = (cols.len() as i64 - 1).max(0);
            let available = (parent.width - n_borders * FRAMESET_BORDER_WIDTH).max(0);
            let widths = resolve_tracks(cols, available);
            let n = cols.len();
            let mut x = parent.x;
            return widths
                .into_iter()
                .enumerate()
                .map(|(i, width)| {
                    let rect = FrameRect {
                        x,
                        y: parent.y,
                        width,
                        height: parent.height,
                    };
                    x += width;
                    if i + 1 < n {
                        x += FRAMESET_BORDER_WIDTH;
                    }
                    rect
                })
                .collect();
        }

        if let Some(rows) = &self.rows {
            let n_borders = (rows.len() as i64 - 1).max(0);
            let available = (parent.height - n_borders * FRAMESET_BORDER_WIDTH).max(0);
            let heights = resolve_tracks(rows, available);
            let n = rows.len();
            let mut y = parent.y;
            return heights
                .into_iter()
                .enumerate()
                .map(|(i, height)| {
                    let rect = FrameRect {
                        x: parent.x,
                        y,
                        width: parent.width,
                        height,
                    };
                    y += height;
                    if i + 1 < n {
                        y += FRAMESET_BORDER_WIDTH;
                    }
                    rect
                })
                .collect();
        }

        vec![parent.clone()]
    }
}

// Spec: RFC 3986 relative reference resolution.
// https://datatracker.ietf.org/doc/html/rfc3986#section-5
/// Bytes accepted from a single external stylesheet, and the combined cap per
/// document. The cascade is O(rules × nodes), so total CSS is bounded to keep
/// layout responsive on pages that ship megabytes of CSS.
const MAX_STYLESHEET_BYTES: usize = 2 * 1024 * 1024;
const MAX_TOTAL_STYLESHEET_BYTES: usize = 6 * 1024 * 1024;

fn stylesheet_cache() -> &'static Mutex<HashMap<String, String>> {
    static CACHE: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fetch and concatenate the CSS for external stylesheet `hrefs`, each resolved
/// against `base_url`. Best-effort: non-HTTP(S) schemes, duplicate URLs, and
/// network/HTTP failures are skipped. Results are cached by resolved URL so
/// relayout (e.g. on window resize) does not re-fetch over the network.
pub fn fetch_external_stylesheets(base_url: &str, hrefs: &[String]) -> String {
    // Only documents loaded over HTTP(S) have resolvable external stylesheets;
    // skip for about:blank, fixtures, and tests so no network I/O happens.
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return String::new();
    }
    let mut combined = String::new();
    let mut seen: HashMap<String, ()> = HashMap::new();
    for href in hrefs {
        if combined.len() >= MAX_TOTAL_STYLESHEET_BYTES {
            break;
        }
        let Ok(url) = resolve_url(base_url, href) else {
            continue;
        };
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            continue;
        }
        if seen.insert(url.clone(), ()).is_some() {
            continue;
        }
        let css = fetch_one_stylesheet(&url);
        if !css.is_empty() {
            combined.push_str(&css);
            combined.push('\n');
        }
    }
    combined
}

fn fetch_one_stylesheet(url: &str) -> String {
    if let Some(cached) = stylesheet_cache()
        .lock()
        .ok()
        .and_then(|cache| cache.get(url).cloned())
    {
        return cached;
    }
    let mut diagnostics = Vec::new();
    let client = select_http_client(url, &mut diagnostics);
    let css = match client.get(url).send() {
        Ok(resp) if resp.status().is_success() => match resp.bytes() {
            Ok(bytes) => {
                let slice = &bytes[..bytes.len().min(MAX_STYLESHEET_BYTES)];
                String::from_utf8_lossy(slice).into_owned()
            }
            Err(_) => String::new(),
        },
        _ => String::new(),
    };
    // Cache successes and failures alike (failure -> empty) so a dead or slow
    // stylesheet is not re-requested on every relayout.
    if let Ok(mut cache) = stylesheet_cache().lock() {
        cache.insert(url.to_string(), css.clone());
    }
    css
}

const MAX_FETCH_BYTES: usize = 4 * 1024 * 1024;

/// A completion notifier: called (from the worker thread) once a fetch has a
/// response ready, so an idle UI event loop can wake and pump/re-layout.
pub type FetchWaker = std::sync::Arc<dyn Fn() + Send + Sync>;

/// Network backend for `cosmo_script`'s `fetch`/XHR, bound to a document's URL
/// for relative-URL resolution. Each request runs on its own worker thread and
/// reports back over a channel, so script execution never blocks on the socket.
pub struct RuntimeFetchEngine {
    base_url: String,
    waker: Option<FetchWaker>,
}

/// Build a fetch backend for scripts running in the document at `base_url`.
pub fn make_fetch_engine(base_url: &str) -> Box<dyn cosmo_script::FetchEngine> {
    Box::new(RuntimeFetchEngine {
        base_url: base_url.to_string(),
        waker: None,
    })
}

/// Build a fetch backend that additionally calls `waker` when a response is
/// ready, so the render loop can wake to pump completions (progressive render).
pub fn make_fetch_engine_with_waker(
    base_url: &str,
    waker: FetchWaker,
) -> Box<dyn cosmo_script::FetchEngine> {
    Box::new(RuntimeFetchEngine {
        base_url: base_url.to_string(),
        waker: Some(waker),
    })
}

impl cosmo_script::FetchEngine for RuntimeFetchEngine {
    fn start(
        &self,
        req: cosmo_script::FetchRequest,
    ) -> std::sync::mpsc::Receiver<cosmo_script::FetchResponse> {
        let (tx, rx) = std::sync::mpsc::channel();
        let base = self.base_url.clone();
        let waker = self.waker.clone();
        std::thread::spawn(move || {
            let _ = tx.send(do_fetch(&base, req));
            // Wake the render loop *after* the response is queued so the pump
            // that follows finds it ready.
            if let Some(waker) = waker {
                waker();
            }
        });
        rx
    }
}

/// CORS-safelisted request-header names (lowercased). A cross-origin request
/// using only these (plus a simple method) needs no preflight.
/// Spec: https://fetch.spec.whatwg.org/#cors-safelisted-request-header
const CORS_SAFELISTED_HEADERS: &[&str] = &[
    "accept",
    "accept-language",
    "content-language",
    "content-type",
];

/// Whether a cross-origin request is "simple" (GET/HEAD/POST + only safelisted
/// headers) and so may skip the CORS preflight.
fn is_simple_cors_request(method: &str, headers: &[(String, String)]) -> bool {
    let m = method.to_ascii_uppercase();
    if !matches!(m.as_str(), "GET" | "HEAD" | "POST") {
        return false;
    }
    headers
        .iter()
        .all(|(name, _)| CORS_SAFELISTED_HEADERS.contains(&name.to_ascii_lowercase().as_str()))
}

/// Send a CORS preflight OPTIONS and verify the server allows the intended
/// method and headers. Returns Err(reason) if the preflight is not approved.
fn cors_preflight(
    client: &reqwest::blocking::Client,
    initiator: &str,
    url: &str,
    method: &str,
    headers: &[(String, String)],
) -> Result<(), String> {
    let requested_headers: Vec<String> =
        headers.iter().map(|(n, _)| n.to_ascii_lowercase()).collect();
    let mut builder = client
        .request(reqwest::Method::OPTIONS, url)
        .header("Access-Control-Request-Method", method.to_ascii_uppercase());
    if !requested_headers.is_empty() {
        builder = builder.header("Access-Control-Request-Headers", requested_headers.join(","));
    }
    let resp = builder
        .send()
        .map_err(|e| format!("CORS preflight failed: {e}"))?;

    // Origin must be allowed.
    if !crate::security::passes_cors(initiator, url, resp.headers()) {
        return Err("CORS preflight blocked (origin not allowed)".to_string());
    }
    let header_val = |name: &str| -> String {
        resp.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase()
    };
    // Method must be allowed.
    let allow_methods = header_val("access-control-allow-methods");
    let m = method.to_ascii_lowercase();
    if allow_methods != "*" && !allow_methods.split(',').any(|x| x.trim() == m) {
        return Err(format!("CORS preflight: method {method} not allowed"));
    }
    // Every requested header must be allowed.
    let allow_headers = header_val("access-control-allow-headers");
    if allow_headers != "*" {
        let allowed: Vec<&str> = allow_headers.split(',').map(|x| x.trim()).collect();
        for h in &requested_headers {
            if !allowed.contains(&h.as_str()) {
                return Err(format!("CORS preflight: header {h} not allowed"));
            }
        }
    }
    Ok(())
}

fn do_fetch(base: &str, req: cosmo_script::FetchRequest) -> cosmo_script::FetchResponse {
    let reject = |url: String, msg: String| cosmo_script::FetchResponse {
        ok: false,
        status: 0,
        status_text: String::new(),
        url,
        body: String::new(),
        error: Some(msg),
    };
    let url = match resolve_url(base, &req.url) {
        Ok(u) => u,
        Err(e) => return reject(req.url.clone(), format!("invalid URL: {e}")),
    };
    // Only http(s) is fetchable; file:// and other schemes are blocked (matches
    // the document loader's security posture).
    if !(url.starts_with("http://") || url.starts_with("https://")) {
        return reject(url.clone(), "unsupported URL scheme".to_string());
    }

    // Mixed content: an https document must not fetch http subresources.
    // Spec: W3C Mixed Content. https://www.w3.org/TR/mixed-content/
    if base.starts_with("https://") && url.starts_with("http://") {
        return reject(url.clone(), "mixed content blocked (https -> http)".to_string());
    }

    // CORS: cross-origin responses are only readable when the server opts in
    // via Access-Control-Allow-Origin. Only enforced when the document has a
    // real http(s) origin (opaque bases like about:blank are permissive).
    // Spec: Fetch CORS protocol. https://fetch.spec.whatwg.org/#http-cors-protocol
    let enforce_cors = (base.starts_with("http://") || base.starts_with("https://"))
        && !crate::security::is_same_origin(base, &url);

    let mut diagnostics = Vec::new();
    let client = select_http_client(&url, &mut diagnostics);

    // A non-simple cross-origin request (non-simple method or non-safelisted
    // header) requires a CORS preflight (OPTIONS) that the server must approve
    // before the actual request is sent.
    // Spec: https://fetch.spec.whatwg.org/#cors-preflight-fetch
    if enforce_cors && !is_simple_cors_request(&req.method, &req.headers) {
        if let Err(msg) = cors_preflight(client, base, &url, &req.method, &req.headers) {
            return reject(url, msg);
        }
    }
    let mut builder = match req.method.to_ascii_uppercase().as_str() {
        "POST" => client.post(&url),
        "PUT" => client.put(&url),
        "DELETE" => client.delete(&url),
        "HEAD" => client.head(&url),
        _ => client.get(&url),
    };
    for (name, value) in &req.headers {
        builder = builder.header(name.as_str(), value.as_str());
    }
    if let Some(body) = req.body {
        builder = builder.body(body);
    }
    match builder.send() {
        Ok(resp) => {
            // CORS check on the response headers before exposing the body.
            if enforce_cors && !crate::security::passes_cors(base, &url, resp.headers()) {
                return reject(
                    url,
                    "blocked by CORS policy (missing/!matching Access-Control-Allow-Origin)"
                        .to_string(),
                );
            }
            let status = resp.status().as_u16();
            let status_text = resp
                .status()
                .canonical_reason()
                .unwrap_or("")
                .to_string();
            let ok = resp.status().is_success();
            let body = match resp.bytes() {
                Ok(bytes) => {
                    let slice = &bytes[..bytes.len().min(MAX_FETCH_BYTES)];
                    String::from_utf8_lossy(slice).into_owned()
                }
                Err(e) => return reject(url, format!("body read failed: {e}")),
            };
            cosmo_script::FetchResponse {
                ok,
                status,
                status_text,
                url,
                body,
                error: None,
            }
        }
        Err(e) => reject(url, format!("request failed: {e}")),
    }
}

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

/// A link activation requested by the navigation shim that
/// [`prepare_html_for_display`] injects: the shim intercepts clicks on anchors,
/// calls `preventDefault()`, and posts the request out to the host.
#[derive(Debug, Clone, PartialEq)]
pub struct NavigateRequest {
    pub frame_id: String,
    pub href: String,
    /// `target` attribute, if the anchor had a non-empty one.
    pub target: Option<String>,
}

/// Parse one `postMessage` payload drained from a page. Returns `None` for
/// anything that isn't the shim's `cosmobrowse:navigate` request (pages post
/// their own messages too).
pub fn parse_navigate_message(message: &str) -> Option<NavigateRequest> {
    let value: serde_json::Value = serde_json::from_str(message).ok()?;
    if value.get("type")?.as_str()? != "cosmobrowse:navigate" {
        return None;
    }
    let href = value.get("href")?.as_str()?.to_string();
    if href.is_empty() {
        return None;
    }
    Some(NavigateRequest {
        frame_id: value
            .get("frameId")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        href,
        target: value
            .get("target")
            .and_then(|v| v.as_str())
            .filter(|t| !t.is_empty())
            .map(str::to_string),
    })
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
        "fixture://abehiroshi/butai" | "fixture://abehiroshi/stage/butai.htm" => (
            "fixture://abehiroshi/butai".to_string(),
            include_str!("../../testdata/abehiroshi/butai.htm").to_string(),
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

    /// Serve one HTTP request with the given extra header line (e.g. an ACAO
    /// header) and return the bound port. Runs on a worker thread.
    fn serve_once(extra_header: Option<&'static str>) -> u16 {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            if let Some(Ok(mut stream)) = listener.incoming().next() {
                let mut buf = [0u8; 1024];
                let _ = stream.read(&mut buf);
                let body = "{\"ok\":1}";
                let acao = extra_header.map(|h| format!("{h}\r\n")).unwrap_or_default();
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n{acao}Connection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        port
    }

    fn get(base: &str, url: &str) -> cosmo_script::FetchResponse {
        do_fetch(
            base,
            cosmo_script::FetchRequest {
                url: url.to_string(),
                method: "GET".to_string(),
                body: None,
                headers: Vec::new(),
            },
        )
    }

    #[test]
    fn cors_allows_same_origin_and_gates_cross_origin() {
        // Same-origin request always succeeds (no ACAO needed).
        let a = serve_once(None);
        let base = format!("http://127.0.0.1:{a}/");
        let same = get(&base, &format!("http://127.0.0.1:{a}/data.json"));
        assert!(same.error.is_none() && same.ok, "same-origin should succeed: {:?}", same.error);

        // Cross-origin without ACAO is blocked.
        let b = serve_once(None);
        let cross = get(&base, &format!("http://127.0.0.1:{b}/data.json"));
        assert!(
            cross.error.as_deref().unwrap_or("").contains("CORS"),
            "cross-origin without ACAO should be CORS-blocked, got: {:?}",
            cross.error
        );

        // Cross-origin WITH `Access-Control-Allow-Origin: *` is allowed.
        let c = serve_once(Some("Access-Control-Allow-Origin: *"));
        let allowed = get(&base, &format!("http://127.0.0.1:{c}/data.json"));
        assert!(
            allowed.error.is_none() && allowed.ok,
            "cross-origin with ACAO:* should succeed, got: {:?}",
            allowed.error
        );
    }

    /// Serve up to 4 requests; answer OPTIONS (preflight) with the given
    /// allow-headers value (None = no CORS headers → preflight fails), and GET
    /// with a body + ACAO:*. Returns the port.
    fn serve_preflight(allow_headers: Option<&'static str>) -> u16 {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for _ in 0..4 {
                let Some(Ok(mut stream)) = listener.incoming().next() else {
                    break;
                };
                let mut buf = [0u8; 1024];
                let n = stream.read(&mut buf).unwrap_or(0);
                let req = String::from_utf8_lossy(&buf[..n]);
                let is_options = req.starts_with("OPTIONS");
                let resp = if is_options {
                    match allow_headers {
                        Some(h) => format!(
                            "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: *\r\nAccess-Control-Allow-Methods: *\r\nAccess-Control-Allow-Headers: {h}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        ),
                        None => "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                    }
                } else {
                    let body = "{\"ok\":1}";
                    format!(
                        "HTTP/1.1 200 OK\r\nAccess-Control-Allow-Origin: *\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                let _ = stream.write_all(resp.as_bytes());
            }
        });
        port
    }

    fn get_with_header(base: &str, url: &str) -> cosmo_script::FetchResponse {
        do_fetch(
            base,
            cosmo_script::FetchRequest {
                url: url.to_string(),
                method: "GET".to_string(),
                body: None,
                headers: vec![("X-Custom".to_string(), "1".to_string())],
            },
        )
    }

    #[test]
    fn cors_preflight_gates_custom_header_requests() {
        // Cross-origin GET with a non-safelisted header requires a preflight.
        let base = "http://127.0.0.1:1/".to_string(); // opaque-ish distinct origin

        // Preflight allows the custom header -> request succeeds.
        let ok_port = serve_preflight(Some("x-custom"));
        let ok = get_with_header(&base, &format!("http://127.0.0.1:{ok_port}/d"));
        assert!(ok.error.is_none() && ok.ok, "preflight-approved request should succeed: {:?}", ok.error);

        // Preflight does NOT allow the header -> request blocked.
        let no_hdr = serve_preflight(Some("x-other"));
        let blocked = get_with_header(&base, &format!("http://127.0.0.1:{no_hdr}/d"));
        assert!(
            blocked.error.as_deref().unwrap_or("").contains("preflight"),
            "preflight without the header should block, got: {:?}",
            blocked.error
        );

        // Preflight returns no CORS headers -> blocked.
        let none = serve_preflight(None);
        let blocked2 = get_with_header(&base, &format!("http://127.0.0.1:{none}/d"));
        assert!(
            blocked2.error.as_deref().unwrap_or("").contains("preflight"),
            "preflight with no CORS headers should block, got: {:?}",
            blocked2.error
        );
    }

    #[test]
    fn mixed_content_blocked_from_https() {
        let r = get("https://secure.example/", "http://insecure.example/x");
        assert!(
            r.error.as_deref().unwrap_or("").contains("mixed content"),
            "https->http should be blocked, got: {:?}",
            r.error
        );
    }

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
