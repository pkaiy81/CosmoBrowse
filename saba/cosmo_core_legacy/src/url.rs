use alloc::vec::Vec;
use alloc::string::String;
use alloc::string::ToString;

#[derive(Debug, Clone, PartialEq)]
pub struct Url {
    url: String,
    scheme: String,
    host: String,
    port: String,
    path: String,
    searchpart: String,
}

impl Url {
    pub fn new(url: String) -> Self {
        Self { url, scheme: String::new(), host: String::new(), port: String::new(), path: String::new(), searchpart: String::new() }
    }

    pub fn host(&self) -> String { self.host.clone() }
    pub fn port(&self) -> String { self.port.clone() }
    pub fn path(&self) -> String { self.path.clone() }
    pub fn searchpart(&self) -> String { self.searchpart.clone() }

    // Ref: RFC 3986 §3.1 and §3.2.
    // https://datatracker.ietf.org/doc/html/rfc3986#section-3.1
    // https://datatracker.ietf.org/doc/html/rfc3986#section-3.2
    pub fn parse(&mut self) -> Result<Self, String> {
        self.scheme = self.extract_scheme()?;
        self.host = self.extract_host();
        self.port = self.extract_port();
        self.path = self.extract_path();
        self.searchpart = self.extract_searchpart();
        Ok(self.clone())
    }

    fn extract_scheme(&self) -> Result<String, String> {
        if self.url.starts_with("http://") { return Ok("http".to_string()); }
        if self.url.starts_with("https://") { return Ok("https".to_string()); }
        Err("Only HTTP and HTTPS schemes are supported.".to_string())
    }

    fn url_without_scheme(&self) -> String {
        self.url.replacen(&(self.scheme.clone() + "://"), "", 1)
    }

    fn split_authority(&self) -> (String, String) {
        let without_scheme = self.url_without_scheme();
        let mut parts = without_scheme.splitn(2, '/');
        let authority = parts.next().unwrap_or_default().to_string();
        let rest = parts.next().unwrap_or_default().to_string();
        (authority, rest)
    }

    fn extract_host(&self) -> String {
        let (authority, _) = self.split_authority();
        if let Some(index) = authority.find(':') { authority[..index].to_string() } else { authority }
    }

    fn extract_port(&self) -> String {
        let (authority, _) = self.split_authority();
        if let Some(index) = authority.find(':') {
            authority[index + 1..].to_string()
        } else if self.scheme == "https" {
            "443".to_string()
        } else {
            "80".to_string()
        }
    }

    fn extract_path(&self) -> String {
        let (_, rest) = self.split_authority();
        if rest.is_empty() { return String::new(); }
        rest.splitn(2, '?').next().unwrap_or_default().to_string()
    }

    fn extract_searchpart(&self) -> String {
        let (_, rest) = self.split_authority();
        let parts: Vec<&str> = rest.splitn(2, '?').collect();
        if parts.len() < 2 { String::new() } else { parts[1].to_string() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_url_host() {
        let url = "http://example.com".to_string();
        let expected = Ok(Url { url: url.clone(), scheme: "http".to_string(), host: "example.com".to_string(), port: "80".to_string(), path: "".to_string(), searchpart: "".to_string() });
        assert_eq!(expected, Url::new(url).parse());
    }

    #[test]
    fn test_https_default_port() {
        let url = "https://example.com/index.html".to_string();
        let expected = Ok(Url { url: url.clone(), scheme: "https".to_string(), host: "example.com".to_string(), port: "443".to_string(), path: "index.html".to_string(), searchpart: "".to_string() });
        assert_eq!(expected, Url::new(url).parse());
    }

    #[test]
    fn test_url_host_port_path_searchpart() {
        let url = "http://example.com:8888/index.html?a=123&b=456".to_string();
        let expected = Ok(Url { url: url.clone(), scheme: "http".to_string(), host: "example.com".to_string(), port: "8888".to_string(), path: "index.html".to_string(), searchpart: "a=123&b=456".to_string() });
        assert_eq!(expected, Url::new(url).parse());
    }

    #[test]
    fn test_unsupported_scheme() {
        let url = "ftp://example.com".to_string();
        let expected = Err("Only HTTP and HTTPS schemes are supported.".to_string());
        assert_eq!(expected, Url::new(url).parse());
    }
}

