use reqwest::blocking::Client;
use saba_core::display_item::DisplayItem;
use saba_core::http::HttpResponse;
use saba_core::renderer::page::Page;
use saba_core::url::Url;
use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct RenderSnapshot {
    pub current_url: String,
    pub title: String,
    pub text_blocks: Vec<String>,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct NavigationState {
    pub can_back: bool,
    pub can_forward: bool,
    pub current_url: Option<String>,
}

#[derive(Debug)]
pub struct BrowserSession {
    history: Vec<String>,
    history_index: usize,
    latest_snapshot: RenderSnapshot,
}

impl Default for BrowserSession {
    fn default() -> Self {
        Self::new()
    }
}

impl BrowserSession {
    pub fn new() -> Self {
        Self {
            history: Vec::new(),
            history_index: 0,
            latest_snapshot: RenderSnapshot {
                current_url: String::new(),
                title: "CosmoBrowse".to_string(),
                text_blocks: Vec::new(),
                diagnostics: vec!["No page loaded".to_string()],
            },
        }
    }

    pub fn open_url(&mut self, url: &str) -> Result<RenderSnapshot, String> {
        let normalized_url = normalize_url(url)?;
        self.load_url(&normalized_url, true)
    }

    pub fn back(&mut self) -> Result<RenderSnapshot, String> {
        if self.history.is_empty() || self.history_index == 0 {
            return Err("No back history".to_string());
        }

        self.history_index -= 1;
        let url = self
            .history
            .get(self.history_index)
            .cloned()
            .ok_or_else(|| "Failed to read back history URL".to_string())?;

        self.load_url(&url, false)
    }

    pub fn forward(&mut self) -> Result<RenderSnapshot, String> {
        if self.history.is_empty() || self.history_index + 1 >= self.history.len() {
            return Err("No forward history".to_string());
        }

        self.history_index += 1;
        let url = self
            .history
            .get(self.history_index)
            .cloned()
            .ok_or_else(|| "Failed to read forward history URL".to_string())?;

        self.load_url(&url, false)
    }

    pub fn reload(&mut self) -> Result<RenderSnapshot, String> {
        let url = self
            .history
            .get(self.history_index)
            .cloned()
            .ok_or_else(|| "No page to reload".to_string())?;

        self.load_url(&url, false)
    }

    pub fn get_render_snapshot(&self) -> RenderSnapshot {
        self.latest_snapshot.clone()
    }

    pub fn navigation_state(&self) -> NavigationState {
        NavigationState {
            can_back: !self.history.is_empty() && self.history_index > 0,
            can_forward: !self.history.is_empty() && self.history_index + 1 < self.history.len(),
            current_url: self.history.get(self.history_index).cloned(),
        }
    }

    fn load_url(&mut self, url: &str, record_history: bool) -> Result<RenderSnapshot, String> {
        let response = fetch_http_response(url)?;

        let mut page = Page::new();
        page.receive_response(response);

        let snapshot = snapshot_from_page(&page, url.to_string());
        self.latest_snapshot = snapshot.clone();

        if record_history {
            self.record_history(url.to_string());
        }

        Ok(snapshot)
    }

    fn record_history(&mut self, url: String) {
        if self
            .history
            .get(self.history_index)
            .map(|v| v == &url)
            .unwrap_or(false)
        {
            return;
        }

        if self.history_index + 1 < self.history.len() {
            self.history.truncate(self.history_index + 1);
        }

        self.history.push(url);
        self.history_index = self.history.len() - 1;
    }
}

fn normalize_url(url: &str) -> Result<String, String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("URL is empty".to_string());
    }

    let normalized = if trimmed.starts_with("http://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };

    Url::new(normalized.clone())
        .parse()
        .map(|_| normalized)
        .map_err(|e| format!("Invalid URL: {e}"))
}

fn fetch_http_response(url: &str) -> Result<HttpResponse, String> {
    let client = Client::builder()
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {e}"))?;

    let response = client
        .get(url)
        .send()
        .map_err(|e| format!("Failed to fetch URL: {e}"))?;

    let version = format!("{:?}", response.version());
    let status = response.status();
    let reason = status.canonical_reason().unwrap_or("UNKNOWN");

    let mut raw_response = format!("{version} {} {reason}\n", status.as_u16());
    for (name, value) in response.headers() {
        let header_value = value
            .to_str()
            .unwrap_or("<binary>")
            .replace('\n', " ")
            .replace('\r', " ");
        raw_response.push_str(&format!("{}: {header_value}\n", name.as_str()));
    }

    let body = response
        .text()
        .map_err(|e| format!("Failed to read response body: {e}"))?;
    raw_response.push_str("\n");
    raw_response.push_str(&body);

    HttpResponse::new(raw_response).map_err(|e| format!("Failed to parse response: {e}"))
}

fn snapshot_from_page(page: &Page, current_url: String) -> RenderSnapshot {
    let text_blocks = page
        .display_items()
        .into_iter()
        .filter_map(|item| match item {
            DisplayItem::Text { text, .. } => Some(text),
            _ => None,
        })
        .filter(|text| !text.trim().is_empty())
        .collect::<Vec<_>>();

    RenderSnapshot {
        current_url,
        title: "CosmoBrowse".to_string(),
        diagnostics: if text_blocks.is_empty() {
            vec!["No renderable text extracted from page".to_string()]
        } else {
            Vec::new()
        },
        text_blocks,
    }
}

#[cfg(test)]
mod tests {
    use super::{normalize_url, BrowserSession};

    #[test]
    fn normalize_http_url() {
        assert_eq!(
            normalize_url("http://example.com").expect("must be valid"),
            "http://example.com"
        );
    }

    #[test]
    fn normalize_without_scheme() {
        assert_eq!(
            normalize_url("example.com").expect("must be valid"),
            "http://example.com"
        );
    }

    #[test]
    fn record_history_discards_forward_entries() {
        let mut session = BrowserSession::new();
        session.record_history("http://a.com".to_string());
        session.record_history("http://b.com".to_string());
        session.record_history("http://c.com".to_string());

        session.history_index = 1;
        session.record_history("http://d.com".to_string());

        assert_eq!(
            session.history,
            vec!["http://a.com", "http://b.com", "http://d.com"]
        );
        assert_eq!(session.history_index, 2);
    }

    #[test]
    fn navigation_state_reflects_history_position() {
        let mut session = BrowserSession::new();
        session.record_history("http://a.com".to_string());
        session.record_history("http://b.com".to_string());
        session.record_history("http://c.com".to_string());

        session.history_index = 1;
        let nav = session.navigation_state();

        assert!(nav.can_back);
        assert!(nav.can_forward);
        assert_eq!(nav.current_url, Some("http://b.com".to_string()));
    }
}
