use crate::alloc::string::ToString;
use crate::error::Error;
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone)]
pub struct Header {
    name: String,
    value: String,
}

impl Header {
    pub fn new(name: String, value: String) -> Self {
        Self { name, value }
    }

    pub fn name(&self) -> String {
        self.name.clone()
    }

    pub fn value(&self) -> String {
        self.value.clone()
    }
}

#[derive(Debug, Clone)]
pub struct HttpRequest {
    method: String,
    target: String,
    headers: Vec<Header>,
}

impl HttpRequest {
    // Spec: RFC 9110 request method/target semantics.
    // https://www.rfc-editor.org/rfc/rfc9110
    pub fn new(method: String, target: String, headers: Vec<Header>) -> Self {
        Self {
            method,
            target,
            headers,
        }
    }

    pub fn method(&self) -> String {
        self.method.clone()
    }

    pub fn target(&self) -> String {
        self.target.clone()
    }

    pub fn headers(&self) -> Vec<Header> {
        self.headers.clone()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RedirectPolicy {
    Follow,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheMode {
    Default,
    NoStore,
    Revalidate,
}

#[derive(Debug, Clone)]
pub struct FetchPipeline {
    request: HttpRequest,
    redirect_policy: RedirectPolicy,
    cache_mode: CacheMode,
}

impl FetchPipeline {
    pub fn new(request: HttpRequest) -> Self {
        Self {
            request,
            redirect_policy: RedirectPolicy::Follow,
            cache_mode: CacheMode::Default,
        }
    }

    pub fn with_redirect_policy(mut self, policy: RedirectPolicy) -> Self {
        self.redirect_policy = policy;
        self
    }

    pub fn with_cache_mode(mut self, mode: CacheMode) -> Self {
        self.cache_mode = mode;
        self
    }

    pub fn request(&self) -> HttpRequest {
        self.request.clone()
    }

    pub fn redirect_policy(&self) -> RedirectPolicy {
        self.redirect_policy
    }

    pub fn cache_mode(&self) -> CacheMode {
        self.cache_mode
    }
}

#[derive(Debug, Clone)]
pub struct HttpResponse {
    version: String,
    status_code: u32,
    reason: String,
    headers: Vec<Header>,
    body: String,
}

impl HttpResponse {
    pub fn new(raw_response: String) -> Result<Self, Error> {
        let preprocessed_response = raw_response.trim_start().replace("\r\n", "\n");

        let (status_line, remaining) = preprocessed_response.split_once('\n').ok_or_else(|| {
            Error::Network(format!("invalid http response: {}", preprocessed_response))
        })?;

        let (version, status_code, reason) = parse_status_line(status_line)?;

        // Spec: RFC 9110 field-line parsing and representation metadata.
        // https://www.rfc-editor.org/rfc/rfc9110
        let (headers, body) = match remaining.split_once("\n\n") {
            Some((h, b)) => {
                let mut parsed = Vec::new();
                for header in h.split('\n').filter(|line| !line.trim().is_empty()) {
                    let (name, value) = header.split_once(':').ok_or_else(|| {
                        Error::Network(format!("invalid http header line: {}", header))
                    })?;
                    parsed.push(Header::new(
                        String::from(name.trim()),
                        String::from(value.trim()),
                    ));
                }
                (parsed, b)
            }
            None => (Vec::new(), remaining),
        };

        Ok(Self {
            version,
            status_code,
            reason,
            headers,
            body: body.to_string(),
        })
    }

    pub fn is_redirect(&self) -> bool {
        (300..400).contains(&self.status_code)
    }

    // Spec: RFC 9111 cache validators (ETag) and freshness controls.
    // https://www.rfc-editor.org/rfc/rfc9111
    pub fn cache_validator(&self) -> Option<String> {
        self.header_value("ETag").ok()
    }

    pub fn has_cache_control_no_store(&self) -> bool {
        self.header_value("Cache-Control")
            .ok()
            .map(|value| value.to_ascii_lowercase().contains("no-store"))
            .unwrap_or(false)
    }

    // Getters
    pub fn version(&self) -> String {
        self.version.clone()
    }

    pub fn status_code(&self) -> u32 {
        self.status_code
    }

    pub fn reason(&self) -> String {
        self.reason.clone()
    }

    pub fn headers(&self) -> Vec<Header> {
        self.headers.clone()
    }

    pub fn body(&self) -> String {
        self.body.clone()
    }

    // Get header value by name
    pub fn header_value(&self, name: &str) -> Result<String, String> {
        for h in &self.headers {
            if h.name.eq_ignore_ascii_case(name) {
                return Ok(h.value.clone());
            }
        }

        Err(format!("failed to find {} in headers", name))
    }
}

fn parse_status_line(status_line: &str) -> Result<(String, u32, String), Error> {
    let mut parts = status_line.split_whitespace();
    let Some(version) = parts.next() else {
        return Err(Error::Network(format!("invalid status line: {}", status_line)));
    };
    let Some(code) = parts.next() else {
        return Err(Error::Network(format!("invalid status line: {}", status_line)));
    };
    let reason = parts.collect::<Vec<&str>>().join(" ");

    let status_code = code
        .parse()
        .map_err(|_| Error::Network(format!("invalid status code: {}", code)))?;
    Ok((version.to_string(), status_code, reason))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid() {
        let raw = "HTTP/1.1 200 OK".to_string();
        assert!(HttpResponse::new(raw).is_err());
    }

    #[test]
    fn test_status_line_only() {
        let raw = "HTTP/1.1 200 OK\n\n".to_string();
        let res = HttpResponse::new(raw).expect("failed to parse http response");
        assert_eq!(res.version(), "HTTP/1.1");
        assert_eq!(res.status_code(), 200);
        assert_eq!(res.reason(), "OK");
    }

    #[test]
    fn test_two_headers_with_white_space() {
        let raw = "HTTP/1.1 200 OK\nDate: xx xx xx\nContent-Length: 42\n\n".to_string();
        let res = HttpResponse::new(raw).expect("failed to parse http response");
        assert_eq!(res.header_value("date"), Ok("xx xx xx".to_string()));
        assert_eq!(res.header_value("Content-Length"), Ok("42".to_string()));
    }

    #[test]
    fn test_reason_phrase_with_spaces() {
        let raw = "HTTP/1.1 404 Not Found Here\n\n".to_string();
        let res = HttpResponse::new(raw).expect("failed to parse http response");
        assert_eq!(res.status_code(), 404);
        assert_eq!(res.reason(), "Not Found Here");
    }

    #[test]
    fn test_cache_helpers() {
        let raw =
            "HTTP/1.1 200 OK\nETag: \"abc\"\nCache-Control: max-age=120, no-store\n\nbody".to_string();
        let res = HttpResponse::new(raw).expect("failed to parse http response");
        assert_eq!(res.cache_validator(), Some("\"abc\"".to_string()));
        assert!(res.has_cache_control_no_store());
    }
}
