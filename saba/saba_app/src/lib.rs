use reqwest::blocking::Client;
use saba_core::display_item::DisplayItem;
use saba_core::http::HttpResponse;
use saba_core::renderer::page::Page;
use saba_core::url::Url;
use serde::Serialize;

pub type AppResult<T> = Result<T, AppError>;

#[derive(Debug, Clone, Serialize)]
pub struct AppError {
    pub code: String,
    pub message: String,
    pub retryable: bool,
}

impl AppError {
    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            code: "validation_error".to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn network(message: impl Into<String>) -> Self {
        Self {
            code: "network_error".to_string(),
            message: message.into(),
            retryable: true,
        }
    }

    pub fn parse(message: impl Into<String>) -> Self {
        Self {
            code: "parse_error".to_string(),
            message: message.into(),
            retryable: false,
        }
    }

    pub fn state(message: impl Into<String>) -> Self {
        Self {
            code: "invalid_state".to_string(),
            message: message.into(),
            retryable: false,
        }
    }
}

impl core::fmt::Display for AppError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for AppError {}

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

pub trait AppService {
    fn open_url(&mut self, url: &str) -> AppResult<RenderSnapshot>;
    fn get_render_snapshot(&self) -> RenderSnapshot;
    fn get_navigation_state(&self) -> NavigationState;
}

#[derive(Debug, Default)]
pub struct SabaApp {
    session: BrowserSession,
}

impl AppService for SabaApp {
    fn open_url(&mut self, url: &str) -> AppResult<RenderSnapshot> {
        self.session.open_url(url)
    }

    fn get_render_snapshot(&self) -> RenderSnapshot {
        self.session.get_render_snapshot()
    }

    fn get_navigation_state(&self) -> NavigationState {
        self.session.navigation_state()
    }
}

#[derive(Debug)]
struct BrowserSession {
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
    fn new() -> Self {
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

    fn open_url(&mut self, url: &str) -> AppResult<RenderSnapshot> {
        let normalized_url = normalize_url(url)?;
        let snapshot = self.load_url(&normalized_url)?;
        self.record_history(normalized_url);
        Ok(snapshot)
    }

    fn get_render_snapshot(&self) -> RenderSnapshot {
        self.latest_snapshot.clone()
    }

    fn navigation_state(&self) -> NavigationState {
        NavigationState {
            can_back: !self.history.is_empty() && self.history_index > 0,
            can_forward: !self.history.is_empty() && self.history_index + 1 < self.history.len(),
            current_url: self.history.get(self.history_index).cloned(),
        }
    }

    fn load_url(&mut self, url: &str) -> AppResult<RenderSnapshot> {
        let response = fetch_http_response(url)?;

        let mut page = Page::new();
        page.receive_response(response);

        let snapshot = snapshot_from_page(&page, url.to_string());
        self.latest_snapshot = snapshot.clone();

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

fn normalize_url(url: &str) -> AppResult<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(AppError::validation("URL is empty"));
    }

    let normalized = if trimmed.starts_with("http://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };

    Url::new(normalized.clone())
        .parse()
        .map(|_| normalized)
        .map_err(|e| AppError::validation(format!("Invalid URL: {e}")))
}

fn fetch_http_response(url: &str) -> AppResult<HttpResponse> {
    let client = Client::builder()
        .build()
        .map_err(|e| AppError::network(format!("Failed to build HTTP client: {e}")))?;

    let response = client
        .get(url)
        .send()
        .map_err(|e| AppError::network(format!("Failed to fetch URL: {e}")))?;

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
        .map_err(|e| AppError::network(format!("Failed to read response body: {e}")))?;
    raw_response.push_str("\n");
    raw_response.push_str(&body);

    HttpResponse::new(raw_response).map_err(|e| AppError::parse(format!("Failed to parse response: {e:?}")))
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
            normalize_url("http://example.com")
                .expect("must be valid"),
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
    fn normalize_empty_url_returns_validation_error() {
        let error = normalize_url(" ").expect_err("must fail");
        assert_eq!(error.code, "validation_error");
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

    #[test]
    fn app_error_display_includes_code_and_message() {
        let error = super::AppError::state("boom");
        assert_eq!(error.to_string(), "invalid_state: boom");
    }
}
