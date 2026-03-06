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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct TabSummary {
    pub id: u32,
    pub title: String,
    pub url: Option<String>,
    pub is_active: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct SearchResult {
    pub id: u32,
    pub title: String,
    pub url: String,
    pub snippet: String,
}

pub trait AppService {
    fn open_url(&mut self, url: &str) -> AppResult<RenderSnapshot>;
    fn reload(&mut self) -> AppResult<RenderSnapshot>;
    fn back(&mut self) -> AppResult<RenderSnapshot>;
    fn forward(&mut self) -> AppResult<RenderSnapshot>;
    fn get_render_snapshot(&self) -> RenderSnapshot;
    fn get_navigation_state(&self) -> NavigationState;
    fn new_tab(&mut self) -> TabSummary;
    fn switch_tab(&mut self, id: u32) -> AppResult<RenderSnapshot>;
    fn close_tab(&mut self, id: u32) -> AppResult<Vec<TabSummary>>;
    fn list_tabs(&self) -> Vec<TabSummary>;
    fn search(&self, query: &str) -> AppResult<Vec<SearchResult>>;
}

#[derive(Debug)]
pub struct SabaApp {
    tabs: Vec<Tab>,
    active_tab_id: u32,
    next_tab_id: u32,
}

impl Default for SabaApp {
    fn default() -> Self {
        let initial_tab = Tab::new(1);
        Self {
            tabs: vec![initial_tab],
            active_tab_id: 1,
            next_tab_id: 2,
        }
    }
}

impl AppService for SabaApp {
    fn open_url(&mut self, url: &str) -> AppResult<RenderSnapshot> {
        self.active_session_mut()?.open_url(url)
    }

    fn reload(&mut self) -> AppResult<RenderSnapshot> {
        self.active_session_mut()?.reload()
    }

    fn back(&mut self) -> AppResult<RenderSnapshot> {
        self.active_session_mut()?.back()
    }

    fn forward(&mut self) -> AppResult<RenderSnapshot> {
        self.active_session_mut()?.forward()
    }

    fn get_render_snapshot(&self) -> RenderSnapshot {
        self.active_session()
            .map(BrowserSession::get_render_snapshot)
            .unwrap_or_else(|_| BrowserSession::new().get_render_snapshot())
    }

    fn get_navigation_state(&self) -> NavigationState {
        self.active_session()
            .map(BrowserSession::navigation_state)
            .unwrap_or_else(|_| BrowserSession::new().navigation_state())
    }

    fn new_tab(&mut self) -> TabSummary {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab::new(id));
        self.active_tab_id = id;
        self.tab_summary(id)
            .expect("newly created tab should be available")
    }

    fn switch_tab(&mut self, id: u32) -> AppResult<RenderSnapshot> {
        if self.tabs.iter().any(|tab| tab.id == id) {
            self.active_tab_id = id;
            return self
                .active_session()
                .map(BrowserSession::get_render_snapshot);
        }

        Err(AppError::state(format!("Tab {id} does not exist")))
    }

    fn close_tab(&mut self, id: u32) -> AppResult<Vec<TabSummary>> {
        let Some(index) = self.tabs.iter().position(|tab| tab.id == id) else {
            return Err(AppError::state(format!("Tab {id} does not exist")));
        };

        self.tabs.remove(index);

        if self.tabs.is_empty() {
            let tab_id = self.next_tab_id;
            self.next_tab_id += 1;
            self.tabs.push(Tab::new(tab_id));
        }

        if self.active_tab_id == id {
            self.active_tab_id = self
                .tabs
                .get(index.saturating_sub(1))
                .or_else(|| self.tabs.first())
                .map(|tab| tab.id)
                .ok_or_else(|| AppError::state("Failed to resolve active tab"))?;
        }

        Ok(self.list_tabs())
    }

    fn list_tabs(&self) -> Vec<TabSummary> {
        self.tabs
            .iter()
            .map(|tab| TabSummary {
                id: tab.id,
                title: tab.session.latest_snapshot.title.clone(),
                url: tab.session.navigation_state().current_url,
                is_active: tab.id == self.active_tab_id,
            })
            .collect()
    }

    fn search(&self, query: &str) -> AppResult<Vec<SearchResult>> {
        build_search_results(query)
    }
}

impl SabaApp {
    fn active_session_mut(&mut self) -> AppResult<&mut BrowserSession> {
        self.tabs
            .iter_mut()
            .find(|tab| tab.id == self.active_tab_id)
            .map(|tab| &mut tab.session)
            .ok_or_else(|| AppError::state("Active tab is unavailable"))
    }

    fn active_session(&self) -> AppResult<&BrowserSession> {
        self.tabs
            .iter()
            .find(|tab| tab.id == self.active_tab_id)
            .map(|tab| &tab.session)
            .ok_or_else(|| AppError::state("Active tab is unavailable"))
    }

    fn tab_summary(&self, id: u32) -> Option<TabSummary> {
        self.tabs
            .iter()
            .find(|tab| tab.id == id)
            .map(|tab| TabSummary {
                id: tab.id,
                title: tab.session.latest_snapshot.title.clone(),
                url: tab.session.navigation_state().current_url,
                is_active: tab.id == self.active_tab_id,
            })
    }
}

#[derive(Debug)]
struct Tab {
    id: u32,
    session: BrowserSession,
}

impl Tab {
    fn new(id: u32) -> Self {
        Self {
            id,
            session: BrowserSession::new(),
        }
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

    fn reload(&mut self) -> AppResult<RenderSnapshot> {
        let current = self
            .history
            .get(self.history_index)
            .cloned()
            .ok_or_else(|| AppError::state("No page to reload"))?;
        self.load_url(&current)
    }

    fn back(&mut self) -> AppResult<RenderSnapshot> {
        if self.history_index == 0 || self.history.is_empty() {
            return Err(AppError::state("No back history"));
        }

        self.navigate_to_history_index(self.history_index - 1)
    }

    fn forward(&mut self) -> AppResult<RenderSnapshot> {
        if self.history.is_empty() || self.history_index + 1 >= self.history.len() {
            return Err(AppError::state("No forward history"));
        }

        self.navigate_to_history_index(self.history_index + 1)
    }

    fn navigation_state(&self) -> NavigationState {
        NavigationState {
            can_back: !self.history.is_empty() && self.history_index > 0,
            can_forward: !self.history.is_empty() && self.history_index + 1 < self.history.len(),
            current_url: self.history.get(self.history_index).cloned(),
        }
    }

    fn navigate_to_history_index(&mut self, index: usize) -> AppResult<RenderSnapshot> {
        let target = self
            .history
            .get(index)
            .cloned()
            .ok_or_else(|| AppError::state("History target is unavailable"))?;

        let snapshot = self.load_url(&target)?;
        self.history_index = index;

        Ok(snapshot)
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

fn build_search_results(query: &str) -> AppResult<Vec<SearchResult>> {
    let normalized = query.trim();
    if normalized.is_empty() {
        return Err(AppError::validation("Search query is empty"));
    }

    let encoded = normalized.replace(' ', "+");
    let mut results = vec![
        SearchResult {
            id: 1,
            title: format!("Search \"{normalized}\" on DuckDuckGo"),
            url: format!("https://duckduckgo.com/?q={encoded}"),
            snippet: "Open web search results in DuckDuckGo".to_string(),
        },
        SearchResult {
            id: 2,
            title: format!("Search \"{normalized}\" on Wikipedia"),
            url: format!("https://en.wikipedia.org/w/index.php?search={encoded}"),
            snippet: "Open Wikipedia search results".to_string(),
        },
    ];

    if !normalized.contains(' ') && normalized.contains('.') {
        let candidate = if normalized.starts_with("http://") || normalized.starts_with("https://") {
            normalized.to_string()
        } else {
            format!("http://{normalized}")
        };

        if normalize_url(&candidate).is_ok() {
            results.insert(
                0,
                SearchResult {
                    id: 0,
                    title: format!("Open {normalized}"),
                    url: candidate,
                    snippet: "Detected URL-like input".to_string(),
                },
            );
        }
    }

    Ok(results)
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

    HttpResponse::new(raw_response)
        .map_err(|e| AppError::parse(format!("Failed to parse response: {e:?}")))
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
    use super::{build_search_results, normalize_url, AppService, BrowserSession};

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

    #[test]
    fn reload_without_history_returns_error() {
        let mut session = BrowserSession::new();

        let error = session.reload().expect_err("must fail");
        assert_eq!(error.code, "invalid_state");
    }

    #[test]
    fn back_without_history_returns_error() {
        let mut session = BrowserSession::new();
        let error = session.back().expect_err("must fail");

        assert_eq!(error.code, "invalid_state");
    }

    #[test]
    fn forward_without_history_returns_error() {
        let mut session = BrowserSession::new();
        let error = session.forward().expect_err("must fail");

        assert_eq!(error.code, "invalid_state");
    }

    #[test]
    fn navigation_failure_keeps_history_index_unchanged() {
        let mut session = BrowserSession::new();
        session.record_history("http://127.0.0.1:1".to_string());
        session.record_history("http://127.0.0.1:2".to_string());
        session.history_index = 1;

        let _ = session.back();

        assert_eq!(session.history_index, 1);
    }

    #[test]
    fn tab_lifecycle_switch_close_and_list() {
        let mut app = super::SabaApp::default();
        let first = app.list_tabs();
        assert_eq!(first.len(), 1);
        assert!(first[0].is_active);

        let new_tab = app.new_tab();
        assert!(new_tab.is_active);
        assert_eq!(app.list_tabs().len(), 2);

        let first_id = first[0].id;
        app.switch_tab(first_id).expect("switch must succeed");
        let listed = app.list_tabs();
        assert!(listed.iter().any(|t| t.id == first_id && t.is_active));

        let listed = app.close_tab(first_id).expect("close must succeed");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].is_active);
    }

    #[test]
    fn closing_last_tab_keeps_one_blank_tab() {
        let mut app = super::SabaApp::default();
        let id = app.list_tabs()[0].id;

        let listed = app.close_tab(id).expect("close must succeed");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].is_active);
    }

    #[test]
    fn search_returns_results_for_query() {
        let results = build_search_results("rust browser").expect("search should succeed");
        assert!(results.len() >= 2);
        assert!(results.iter().any(|r| r.url.contains("duckduckgo.com")));
    }

    #[test]
    fn search_empty_query_is_validation_error() {
        let err = build_search_results("  ").expect_err("must fail");
        assert_eq!(err.code, "validation_error");
    }
}
