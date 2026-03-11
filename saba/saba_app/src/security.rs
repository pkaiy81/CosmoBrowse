use regex::Regex;
use reqwest::header::{HeaderMap, HeaderValue, ACCESS_CONTROL_ALLOW_ORIGIN, SET_COOKIE};
use std::sync::OnceLock;
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
pub fn passes_cors(initiator_url: &str, target_url: &str, response_headers: &HeaderMap) -> bool {
    if is_same_origin(initiator_url, target_url) {
        return true;
    }

    let Some(initiator) = Url::parse(initiator_url).ok() else {
        return false;
    };
    let Some(initiator_origin) = Origin::from_url(&initiator).map(|origin| origin.serialize())
    else {
        return false;
    };
    let Some(value) = response_headers.get(ACCESS_CONTROL_ALLOW_ORIGIN) else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    value == "*" || value == initiator_origin
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
    pub secure: bool,
    pub http_only: bool,
    pub same_site: Option<SameSitePolicy>,
}

// Spec: RFC 6265bis cookie attributes (Secure/HttpOnly/SameSite).
// https://httpwg.org/http-extensions/draft-ietf-httpbis-rfc6265bis.html
pub fn parse_set_cookie(value: &HeaderValue) -> Option<CookieAttributes> {
    let raw = value.to_str().ok()?;
    let mut segments = raw.split(';').map(str::trim);
    let name_value = segments.next()?;
    let (name, _) = name_value.split_once('=')?;
    let mut attrs = CookieAttributes {
        name: name.trim().to_string(),
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
    let secure_context = Url::parse(response_url)
        .ok()
        .map(|url| url.scheme() == "https")
        .unwrap_or(false);

    if attrs.same_site == Some(SameSitePolicy::None) && !attrs.secure {
        return format!(
            "Cookie '{}' rejected: SameSite=None requires Secure",
            attrs.name
        );
    }

    if attrs.secure && !secure_context {
        return format!(
            "Cookie '{}' rejected: Secure attribute on non-HTTPS response",
            attrs.name
        );
    }

    if attrs.http_only {
        return format!(
            "Cookie '{}' accepted with HttpOnly (hidden from scripts)",
            attrs.name
        );
    }

    format!("Cookie '{}' accepted", attrs.name)
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

pub fn classify_tls_policy_error(message: &str) -> AppError {
    let lower = message.to_ascii_lowercase();
    if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
        return AppError::tls(format!(
            "Certificate/TLS policy blocked secure connection: {message}"
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
}
