use regex::Regex;
use reqwest::header::{
    HeaderMap, HeaderName, HeaderValue, ACCESS_CONTROL_ALLOW_CREDENTIALS,
    ACCESS_CONTROL_ALLOW_HEADERS, ACCESS_CONTROL_ALLOW_METHODS, ACCESS_CONTROL_ALLOW_ORIGIN,
    COOKIE, SET_COOKIE,
};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use url::Url;

use crate::model::{AppError, AppResult};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Origin {
    scheme: String,
    host: String,
    port: u16,
}

impl Origin {
    // Spec: RFC 6454 Origin serialization and same-origin comparison.
    // https://datatracker.ietf.org/doc/html/rfc6454
    pub fn from_url(url: &Url) -> Option<Self> {
        let host = url.host_str()?.to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .unwrap_or_else(|| default_port(url.scheme()));
        Some(Self {
            scheme: url.scheme().to_ascii_lowercase(),
            host,
            port,
        })
    }

    pub fn serialize(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
}

pub fn is_same_origin(source: &str, target: &str) -> bool {
    let Ok(source_url) = Url::parse(source) else {
        return false;
    };
    let Ok(target_url) = Url::parse(target) else {
        return false;
    };

    let Some(source_origin) = Origin::from_url(&source_url) else {
        return false;
    };
    let Some(target_origin) = Origin::from_url(&target_url) else {
        return false;
    };

    source_origin == target_origin
}

pub fn enforce_mixed_content_policy(initiator_url: &str, target_url: &str) -> AppResult<()> {
    let initiator = Url::parse(initiator_url)
        .map_err(|error| AppError::validation(format!("Invalid initiator URL: {error}")))?;
    let target = Url::parse(target_url)
        .map_err(|error| AppError::validation(format!("Invalid target URL: {error}")))?;

    if initiator.scheme() == "https" && target.scheme() == "http" {
        return Err(AppError::validation(format!(
            "Mixed content blocked by security policy: {initiator_url} -> {target_url}"
        )));
    }

    Ok(())
}

// Spec: Fetch CORS protocol with Access-Control-Allow-Origin matching.
// https://fetch.spec.whatwg.org/#http-cors-protocol

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SandboxPolicy {
    AllowAll,
    Strict,
}

#[derive(Debug, Clone)]
pub struct SandboxEvaluation {
    pub policy: SandboxPolicy,
    pub allowed: bool,
    pub diagnostics: Vec<String>,
}

pub fn evaluate_sandbox_policy(initiator_url: &str, target_url: &str) -> SandboxEvaluation {
    // Spec: HTML sandboxing model and opaque origin behavior.
    // https://html.spec.whatwg.org/multipage/iframe-embed-object.html#attr-iframe-sandbox
    let mut diagnostics = Vec::new();
    let same_origin = is_same_origin(initiator_url, target_url);

    if same_origin {
        diagnostics.push(format!(
            "Sandbox policy: same-origin navigation permitted ({initiator_url} -> {target_url})"
        ));
        return SandboxEvaluation {
            policy: SandboxPolicy::AllowAll,
            allowed: true,
            diagnostics,
        };
    }

    diagnostics.push(format!(
        "Sandbox policy: cross-origin target restricted ({initiator_url} -> {target_url})"
    ));

    let mixed_content = enforce_mixed_content_policy(initiator_url, target_url).is_err();
    if mixed_content {
        diagnostics.push("Sandbox policy: mixed-content navigation denied".to_string());
        return SandboxEvaluation {
            policy: SandboxPolicy::Strict,
            allowed: false,
            diagnostics,
        };
    }

    diagnostics.push(
        "Sandbox policy: script and top-navigation privileges withheld for cross-origin content"
            .to_string(),
    );
    SandboxEvaluation {
        policy: SandboxPolicy::Strict,
        allowed: true,
        diagnostics,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredentialsMode {
    Omit,
    Include,
}

#[derive(Debug, Clone)]
pub struct CorsPreflightResult {
    pub status: u16,
    pub headers: HeaderMap,
}

#[derive(Debug, Clone)]
pub struct CorsRequest<'a> {
    pub initiator_url: &'a str,
    pub target_url: &'a str,
    pub method: &'a str,
    pub request_headers: Vec<HeaderName>,
    pub credentials_mode: CredentialsMode,
}

pub fn passes_cors(initiator_url: &str, target_url: &str, response_headers: &HeaderMap) -> bool {
    let request = CorsRequest {
        initiator_url,
        target_url,
        method: "GET",
        request_headers: Vec::new(),
        credentials_mode: CredentialsMode::Omit,
    };
    evaluate_cors_request(&request, response_headers, None).is_ok()
}

// Spec: Fetch Standard CORS-preflight fetch and CORS check algorithm.
// https://fetch.spec.whatwg.org/#cors-preflight-fetch
// https://fetch.spec.whatwg.org/#cors-check
pub fn evaluate_cors_request(
    request: &CorsRequest<'_>,
    response_headers: &HeaderMap,
    preflight: Option<&CorsPreflightResult>,
) -> AppResult<()> {
    if is_same_origin(request.initiator_url, request.target_url) {
        return Ok(());
    }

    let initiator = Url::parse(request.initiator_url)
        .map_err(|error| AppError::validation(format!("Invalid initiator URL: {error}")))?;
    let initiator_origin = Origin::from_url(&initiator)
        .ok_or_else(|| AppError::validation("Initiator URL has opaque origin"))?
        .serialize();

    if requires_preflight(request.method, &request.request_headers) {
        let Some(preflight) = preflight else {
            return Err(AppError::cors_preflight_failed(
                "CORS preflight required but no preflight response was provided",
            ));
        };
        validate_preflight(preflight, request, &initiator_origin)?;
    }

    let allowed_origin = response_headers
        .get(ACCESS_CONTROL_ALLOW_ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            AppError::cors_blocked("Missing Access-Control-Allow-Origin in cross-origin response")
        })?;

    match request.credentials_mode {
        // Spec: Fetch forbids `*` with credentials mode include.
        // https://fetch.spec.whatwg.org/#http-new-header-syntax
        CredentialsMode::Include => {
            if allowed_origin != initiator_origin {
                return Err(AppError::cors_blocked(format!(
                    "Credentials mode include requires exact Access-Control-Allow-Origin match (expected {initiator_origin}, got {allowed_origin})"
                )));
            }
            let allows_credentials = response_headers
                .get(ACCESS_CONTROL_ALLOW_CREDENTIALS)
                .and_then(|value| value.to_str().ok())
                .is_some_and(|value| value.eq_ignore_ascii_case("true"));
            if !allows_credentials {
                return Err(AppError::cors_blocked(
                    "Credentials mode include requires Access-Control-Allow-Credentials: true",
                ));
            }
        }
        CredentialsMode::Omit => {
            if allowed_origin != "*" && allowed_origin != initiator_origin {
                return Err(AppError::cors_blocked(format!(
                    "Access-Control-Allow-Origin does not allow initiator origin {initiator_origin}"
                )));
            }
        }
    }

    Ok(())
}

pub fn requires_preflight(method: &str, request_headers: &[HeaderName]) -> bool {
    let method = method.to_ascii_uppercase();
    if !matches!(method.as_str(), "GET" | "HEAD" | "POST") {
        return true;
    }

    request_headers
        .iter()
        .any(|name| !is_cors_safelisted_header(name.as_str()))
}

fn validate_preflight(
    preflight: &CorsPreflightResult,
    request: &CorsRequest<'_>,
    initiator_origin: &str,
) -> AppResult<()> {
    if !(200..300).contains(&preflight.status) {
        return Err(AppError::cors_preflight_failed(format!(
            "Preflight failed with HTTP status {}",
            preflight.status
        )));
    }

    let allow_origin = preflight
        .headers
        .get(ACCESS_CONTROL_ALLOW_ORIGIN)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            AppError::cors_preflight_failed(
                "Preflight response missing Access-Control-Allow-Origin",
            )
        })?;
    if allow_origin != "*" && allow_origin != initiator_origin {
        return Err(AppError::cors_preflight_failed(format!(
            "Preflight origin mismatch: expected {initiator_origin}, got {allow_origin}"
        )));
    }

    let requested_method = request.method.to_ascii_uppercase();
    let methods = preflight
        .headers
        .get(ACCESS_CONTROL_ALLOW_METHODS)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    if !header_csv_contains_token(methods, &requested_method) {
        return Err(AppError::cors_preflight_failed(format!(
            "Preflight denied method {}",
            request.method
        )));
    }

    let allow_headers = preflight
        .headers
        .get(ACCESS_CONTROL_ALLOW_HEADERS)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default();
    for name in &request.request_headers {
        if !is_cors_safelisted_header(name.as_str())
            && !header_csv_contains_token(allow_headers, name.as_str())
        {
            return Err(AppError::cors_preflight_failed(format!(
                "Preflight denied header {}",
                name
            )));
        }
    }

    Ok(())
}

fn header_csv_contains_token(csv: &str, token: &str) -> bool {
    csv.split(',')
        .map(str::trim)
        .any(|entry| entry.eq_ignore_ascii_case(token))
}

// Spec: Fetch CORS-safelisted request-header names.
// https://fetch.spec.whatwg.org/#cors-safelisted-request-header
fn is_cors_safelisted_header(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "accept" | "accept-language" | "content-language" | "content-type"
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SameSitePolicy {
    Lax,
    Strict,
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CookieAttributes {
    pub name: String,
    pub value: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSitePolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSitePolicy>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PermissionCacheEntry {
    pub initiator_origin: String,
    pub target_origin: String,
    pub allowed: bool,
    pub updated_at: Instant,
}

#[derive(Debug, Default)]
pub struct PersistentSecurityState {
    pub cookie_jar: HashMap<String, Vec<StoredCookie>>,
    pub local_storage: HashMap<String, HashMap<String, String>>,
    pub permission_cache: HashMap<(String, String), PermissionCacheEntry>,
}

fn persistent_security_state() -> &'static Mutex<PersistentSecurityState> {
    static STATE: OnceLock<Mutex<PersistentSecurityState>> = OnceLock::new();
    STATE.get_or_init(|| Mutex::new(PersistentSecurityState::default()))
}

#[cfg(test)]
fn clear_persistent_state_for_tests() {
    if let Ok(mut state) = persistent_security_state().lock() {
        *state = PersistentSecurityState::default();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CookieDisposition {
    Accepted,
    Rejected(String),
}

fn origin_key(url: &str) -> Option<String> {
    let parsed = Url::parse(url).ok()?;
    Origin::from_url(&parsed).map(|origin| origin.serialize())
}

fn is_secure_cookie_context(url: &str) -> bool {
    Url::parse(url)
        .ok()
        .map(|url| url.scheme() == "https")
        .unwrap_or(false)
}

fn is_schemeful_same_site(request_url: &str, site_for_cookies_url: &str) -> bool {
    // Spec: RFC 6265bis defines SameSite in terms of "site for cookies".
    // https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html
    // Compliance note: this legacy app persists cookies in an RFC 6454 origin-keyed jar,
    // so the site-for-cookies check is intentionally implemented as a stricter same-origin
    // comparison instead of registrable-domain matching because no public suffix list exists here.
    // https://datatracker.ietf.org/doc/html/rfc6454
    is_same_origin(request_url, site_for_cookies_url)
}

fn evaluate_cookie_storage(attrs: &CookieAttributes, response_url: &str) -> CookieDisposition {
    if attrs.same_site == Some(SameSitePolicy::None) && !attrs.secure {
        return CookieDisposition::Rejected(format!(
            "Cookie '{}' rejected: SameSite=None requires Secure",
            attrs.name
        ));
    }

    if attrs.secure && !is_secure_cookie_context(response_url) {
        return CookieDisposition::Rejected(format!(
            "Cookie '{}' rejected: Secure attribute on non-HTTPS response",
            attrs.name
        ));
    }

    CookieDisposition::Accepted
}

fn can_send_cookie(cookie: &StoredCookie, request_url: &str, site_for_cookies_url: &str) -> bool {
    if cookie.secure && !is_secure_cookie_context(request_url) {
        return false;
    }

    match cookie.same_site {
        Some(SameSitePolicy::Strict) | Some(SameSitePolicy::Lax) => {
            is_schemeful_same_site(request_url, site_for_cookies_url)
        }
        Some(SameSitePolicy::None) | None => true,
    }
}

// Spec: RFC 6265bis cookie attributes (Secure/HttpOnly/SameSite).
// https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html
pub fn parse_set_cookie(value: &HeaderValue) -> Option<CookieAttributes> {
    let raw = value.to_str().ok()?;
    let mut segments = raw.split(';').map(str::trim);
    let name_value = segments.next()?;
    let (name, value) = name_value.split_once('=')?;
    let mut attrs = CookieAttributes {
        name: name.trim().to_string(),
        value: value.trim().to_string(),
        secure: false,
        http_only: false,
        same_site: None,
    };

    for attr in segments {
        if attr.eq_ignore_ascii_case("secure") {
            attrs.secure = true;
            continue;
        }
        if attr.eq_ignore_ascii_case("httponly") {
            attrs.http_only = true;
            continue;
        }
        if let Some((key, value)) = attr.split_once('=') {
            if key.trim().eq_ignore_ascii_case("samesite") {
                attrs.same_site = match value.trim().to_ascii_lowercase().as_str() {
                    "lax" => Some(SameSitePolicy::Lax),
                    "strict" => Some(SameSitePolicy::Strict),
                    "none" => Some(SameSitePolicy::None),
                    _ => None,
                };
            }
        }
    }

    Some(attrs)
}

pub fn evaluate_cookie(attrs: &CookieAttributes, response_url: &str) -> String {
    match evaluate_cookie_storage(attrs, response_url) {
        CookieDisposition::Rejected(reason) => reason,
        CookieDisposition::Accepted if attrs.http_only => {
            format!(
                "Cookie '{}' accepted with HttpOnly (hidden from scripts)",
                attrs.name
            )
        }
        CookieDisposition::Accepted => format!("Cookie '{}' accepted", attrs.name),
    }
}

// Spec: Content Security Policy Level 3 baseline policy.
// https://www.w3.org/TR/CSP3/
pub fn apply_minimum_csp(html: &str) -> (String, Vec<String>) {
    let minimum = "default-src 'self'; object-src 'none'; base-uri 'self'";
    let mut diagnostics = Vec::new();

    if !contains_csp_meta(html) {
        diagnostics.push("Applied minimum CSP policy: default-src 'self'".to_string());
        let meta = format!(
            "<meta http-equiv=\"Content-Security-Policy\" content=\"{}\">",
            minimum
        );
        let output = if let Some(index) = html.find("</head>") {
            let mut output = String::with_capacity(html.len() + meta.len());
            output.push_str(&html[..index]);
            output.push_str(&meta);
            output.push_str(&html[index..]);
            output
        } else {
            format!("<head>{meta}</head>{html}")
        };

        diagnostics.extend(diagnose_csp_violations(&output));
        return (output, diagnostics);
    }

    diagnostics.extend(diagnose_csp_violations(html));
    (html.to_string(), diagnostics)
}

pub fn diagnose_csp_violations(html: &str) -> Vec<String> {
    let mut diagnostics = Vec::new();
    if inline_script_regex().is_match(html) {
        diagnostics.push(
            "CSP violation (minimum policy): inline <script> requires explicit allowance"
                .to_string(),
        );
    }
    diagnostics
}

pub fn collect_cookie_diagnostics(headers: &HeaderMap, response_url: &str) -> Vec<String> {
    headers
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(parse_set_cookie)
        .map(|cookie| evaluate_cookie(&cookie, response_url))
        .collect()
}

pub fn apply_set_cookie_headers(headers: &HeaderMap, response_url: &str) -> Vec<String> {
    let Some(origin) = origin_key(response_url) else {
        return Vec::new();
    };
    let Ok(mut state) = persistent_security_state().lock() else {
        return collect_cookie_diagnostics(headers, response_url);
    };

    let jar = state.cookie_jar.entry(origin).or_default();
    let mut diagnostics = Vec::new();
    for attrs in headers
        .get_all(SET_COOKIE)
        .iter()
        .filter_map(parse_set_cookie)
    {
        match evaluate_cookie_storage(&attrs, response_url) {
            CookieDisposition::Rejected(reason) => diagnostics.push(reason),
            CookieDisposition::Accepted => {
                if let Some(existing) = jar.iter_mut().find(|cookie| cookie.name == attrs.name) {
                    existing.value = attrs.value.clone();
                    existing.secure = attrs.secure;
                    existing.http_only = attrs.http_only;
                    existing.same_site = attrs.same_site.clone();
                } else {
                    jar.push(StoredCookie {
                        name: attrs.name.clone(),
                        value: attrs.value.clone(),
                        secure: attrs.secure,
                        http_only: attrs.http_only,
                        same_site: attrs.same_site.clone(),
                    });
                }
                diagnostics.push(evaluate_cookie(&attrs, response_url));
            }
        }
    }
    diagnostics
}

pub fn cookie_header_value(request_url: &str, site_for_cookies_url: &str) -> Option<HeaderValue> {
    let origin = origin_key(request_url)?;
    let state = persistent_security_state().lock().ok()?;
    let cookies = state.cookie_jar.get(&origin)?;

    let value = cookies
        .iter()
        .filter(|cookie| can_send_cookie(cookie, request_url, site_for_cookies_url))
        .map(|cookie| format!("{}={}", cookie.name, cookie.value))
        .collect::<Vec<_>>()
        .join("; ");

    if value.is_empty() {
        return None;
    }

    HeaderValue::from_str(&value).ok()
}

pub fn attach_cookie_header(
    headers: &mut HeaderMap,
    request_url: &str,
    site_for_cookies_url: &str,
) -> bool {
    let Some(value) = cookie_header_value(request_url, site_for_cookies_url) else {
        return false;
    };
    headers.insert(COOKIE, value);
    true
}

pub fn cache_permission_decision(initiator_url: &str, target_url: &str, allowed: bool) {
    let Some(initiator_origin) = origin_key(initiator_url) else {
        return;
    };
    let Some(target_origin) = origin_key(target_url) else {
        return;
    };
    let Ok(mut state) = persistent_security_state().lock() else {
        return;
    };
    state.permission_cache.insert(
        (initiator_origin.clone(), target_origin.clone()),
        PermissionCacheEntry {
            initiator_origin,
            target_origin,
            allowed,
            updated_at: Instant::now(),
        },
    );
}

pub fn local_storage_snapshot(url: &str) -> Vec<(String, String)> {
    let Some(origin) = origin_key(url) else {
        return Vec::new();
    };
    let Ok(state) = persistent_security_state().lock() else {
        return Vec::new();
    };
    state
        .local_storage
        .get(&origin)
        .map(|entries| {
            let mut pairs = entries
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect::<Vec<_>>();
            pairs.sort_by(|left, right| left.0.cmp(&right.0));
            pairs
        })
        .unwrap_or_default()
}

pub fn replace_local_storage(url: &str, entries: &[(String, String)]) {
    let Some(origin) = origin_key(url) else {
        return;
    };
    let Ok(mut state) = persistent_security_state().lock() else {
        return;
    };
    let store = state.local_storage.entry(origin).or_default();
    store.clear();
    for (key, value) in entries {
        store.insert(key.clone(), value.clone());
    }
}

fn tls_exception_store() -> &'static Mutex<HashMap<String, Instant>> {
    static STORE: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();
    STORE.get_or_init(|| Mutex::new(HashMap::new()))
}

// Spec: certificate override decisions should be explicit and scoped per-origin,
// with expiration to avoid permanent weakening of transport guarantees.
// Ref: RFC 5280 path validation + browser interstitial UX practice.
// https://www.rfc-editor.org/rfc/rfc5280
pub fn register_tls_exception(url: &str, ttl: Duration) -> AppResult<String> {
    let parsed = Url::parse(url)
        .map_err(|error| AppError::validation(format!("Invalid URL for TLS exception: {error}")))?;
    if parsed.scheme() != "https" {
        return Err(AppError::validation(
            "TLS exceptions are only valid for https origins",
        ));
    }
    let origin = Origin::from_url(&parsed)
        .ok_or_else(|| AppError::validation("TLS exception requires host-based origin"))?
        .serialize();

    let mut store = tls_exception_store()
        .lock()
        .map_err(|_| AppError::state("TLS exception store lock poisoned"))?;
    store.insert(origin.clone(), Instant::now() + ttl);
    Ok(origin)
}

#[cfg(test)]
fn clear_tls_exceptions_for_tests() {
    if let Ok(mut store) = tls_exception_store().lock() {
        store.clear();
    }
}

pub fn has_tls_exception(url: &str) -> bool {
    let Ok(parsed) = Url::parse(url) else {
        return false;
    };
    if parsed.scheme() != "https" {
        return false;
    }
    let Some(origin) = Origin::from_url(&parsed).map(|origin| origin.serialize()) else {
        return false;
    };

    let Ok(mut store) = tls_exception_store().lock() else {
        return false;
    };

    store.retain(|_, expires_at| *expires_at > Instant::now());
    store.contains_key(&origin)
}

pub fn classify_tls_policy_error(message: &str) -> AppError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("certificate has expired") || lower.contains("expired") {
        return AppError::tls_certificate_expired(format!(
            "Certificate validation failed (expired): {message}"
        ));
    }
    if lower.contains("self signed") {
        return AppError::tls_certificate_self_signed(format!(
            "Certificate validation failed (self-signed): {message}"
        ));
    }
    if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
        return AppError::tls(format!(
            "Certificate/TLS policy blocked secure connection: {message}"
        ));
    }
    if lower.contains("timed out") || lower.contains("timeout") {
        return AppError::network_timeout(format!("Network timeout: {message}"));
    }
    if lower.contains("decode") || lower.contains("decompress") {
        return AppError::network_content_decoding(format!(
            "Response content decoding failed: {message}"
        ));
    }
    AppError::network(message.to_string())
}

fn contains_csp_meta(html: &str) -> bool {
    csp_meta_regex().is_match(html)
}

fn csp_meta_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r#"(?is)<meta[^>]+http-equiv\s*=\s*[\"']content-security-policy[\"'][^>]*>"#)
            .expect("valid csp meta regex")
    })
}

fn inline_script_regex() -> &'static Regex {
    static REGEX: OnceLock<Regex> = OnceLock::new();
    REGEX.get_or_init(|| {
        Regex::new(r"(?is)<script(?P<attrs>[^>]*)>(?P<body>.*?)</script>")
            .expect("valid inline script regex")
    })
}

fn default_port(scheme: &str) -> u16 {
    match scheme {
        "http" => 80,
        "https" => 443,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_origin_matches_default_ports() {
        assert!(is_same_origin(
            "https://example.com",
            "https://example.com:443/path"
        ));
        assert!(!is_same_origin("https://example.com", "http://example.com"));
    }

    #[test]
    fn minimum_csp_reports_inline_script() {
        let (_, diagnostics) =
            apply_minimum_csp("<html><head></head><body><script>alert(1)</script></body></html>");
        assert!(diagnostics.iter().any(|d| d.contains("CSP violation")));
    }

    #[test]
    fn set_cookie_updates_origin_scoped_jar_and_request_header() {
        clear_persistent_state_for_tests();

        let mut headers = HeaderMap::new();
        headers.insert(
            SET_COOKIE,
            HeaderValue::from_static("sid=abc; Secure; HttpOnly; SameSite=None"),
        );

        let diagnostics = apply_set_cookie_headers(&headers, "https://example.com/login");
        assert!(diagnostics
            .iter()
            .any(|message| message.contains("HttpOnly")));

        let header = cookie_header_value("https://example.com/app", "https://example.com/app")
            .expect("cookie header");
        assert_eq!(header.to_str().expect("cookie header str"), "sid=abc");
    }

    #[test]
    fn insecure_same_site_none_cookie_is_rejected_and_not_stored() {
        clear_persistent_state_for_tests();

        let mut headers = HeaderMap::new();
        headers.insert(
            SET_COOKIE,
            HeaderValue::from_static("sid=abc; SameSite=None"),
        );

        let diagnostics = apply_set_cookie_headers(&headers, "https://example.com/login");
        assert!(diagnostics
            .iter()
            .any(|message| message.contains("requires Secure")));
        assert!(
            cookie_header_value("https://example.com/app", "https://example.com/app").is_none()
        );
    }

    #[test]
    fn local_storage_snapshots_are_origin_scoped() {
        clear_persistent_state_for_tests();

        replace_local_storage(
            "https://example.com/app",
            &[("theme".to_string(), "dark".to_string())],
        );

        assert_eq!(
            local_storage_snapshot("https://example.com/other"),
            vec![("theme".to_string(), "dark".to_string())]
        );
        assert!(local_storage_snapshot("https://example.org/other").is_empty());
    }

    #[test]
    fn sandbox_policy_restricts_cross_origin() {
        let evaluation = evaluate_sandbox_policy(
            "https://app.example.com/root",
            "https://cdn.example.net/embedded",
        );
        assert_eq!(evaluation.policy, SandboxPolicy::Strict);
        assert!(evaluation.allowed);
        assert!(evaluation
            .diagnostics
            .iter()
            .any(|message| message.contains("cross-origin")));
    }

    #[test]
    fn sandbox_policy_blocks_mixed_content() {
        let evaluation =
            evaluate_sandbox_policy("https://secure.example.com", "http://legacy.test");
        assert_eq!(evaluation.policy, SandboxPolicy::Strict);
        assert!(!evaluation.allowed);
        assert!(evaluation
            .diagnostics
            .iter()
            .any(|message| message.contains("mixed-content")));
    }
    #[test]
    fn cors_requires_preflight_for_non_simple_method() {
        let request_headers = vec![HeaderName::from_static("x-request-id")];
        assert!(requires_preflight("PUT", &request_headers));
        assert!(!requires_preflight("GET", &[]));
    }

    #[test]
    fn cors_blocks_wildcard_with_credentials() {
        let request = CorsRequest {
            initiator_url: "https://app.example.com/page",
            target_url: "https://api.example.net/data",
            method: "GET",
            request_headers: Vec::new(),
            credentials_mode: CredentialsMode::Include,
        };
        let mut headers = HeaderMap::new();
        headers.insert(ACCESS_CONTROL_ALLOW_ORIGIN, HeaderValue::from_static("*"));
        headers.insert(
            ACCESS_CONTROL_ALLOW_CREDENTIALS,
            HeaderValue::from_static("true"),
        );

        let error = evaluate_cors_request(&request, &headers, None).expect_err("must block");
        assert_eq!(error.code, "cors_blocked");
    }

    #[test]
    fn cors_preflight_fails_when_header_not_allowed() {
        let request = CorsRequest {
            initiator_url: "https://app.example.com/page",
            target_url: "https://api.example.net/data",
            method: "PUT",
            request_headers: vec![HeaderName::from_static("x-token")],
            credentials_mode: CredentialsMode::Omit,
        };

        let mut response_headers = HeaderMap::new();
        response_headers.insert(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("https://app.example.com:443"),
        );

        let mut preflight_headers = HeaderMap::new();
        preflight_headers.insert(
            ACCESS_CONTROL_ALLOW_ORIGIN,
            HeaderValue::from_static("https://app.example.com:443"),
        );
        preflight_headers.insert(
            ACCESS_CONTROL_ALLOW_METHODS,
            HeaderValue::from_static("PUT"),
        );
        preflight_headers.insert(
            ACCESS_CONTROL_ALLOW_HEADERS,
            HeaderValue::from_static("content-type"),
        );
        let preflight = CorsPreflightResult {
            status: 204,
            headers: preflight_headers,
        };

        let error = evaluate_cors_request(&request, &response_headers, Some(&preflight))
            .expect_err("preflight should fail");
        assert_eq!(error.code, "cors_preflight_failed");
    }

    #[test]
    fn tls_exception_is_origin_scoped_and_expires() {
        clear_tls_exceptions_for_tests();

        let origin = register_tls_exception("https://example.com/path", Duration::from_millis(25))
            .expect("register tls exception");
        assert_eq!(origin, "https://example.com:443");
        assert!(has_tls_exception("https://example.com/other"));
        assert!(!has_tls_exception("https://example.org/other"));

        std::thread::sleep(Duration::from_millis(30));
        assert!(!has_tls_exception("https://example.com/other"));
    }
}
