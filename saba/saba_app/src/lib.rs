use reqwest::blocking::Client;
use saba_core::display_item::DisplayItem;
use saba_core::http::HttpResponse;
use saba_core::renderer::layout::layout_object::LayoutSize;
use saba_core::renderer::page::Page;
use saba_core::url::Url;
use serde::Serialize;

pub type AppResult<T> = Result<T, AppError>;

const DEFAULT_VIEWPORT_WIDTH: i64 = 960;
const DEFAULT_VIEWPORT_HEIGHT: i64 = 720;

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

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ContentSize {
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SceneItem {
    Rect {
        x: i64,
        y: i64,
        width: i64,
        height: i64,
        background_color: String,
    },
    Text {
        x: i64,
        y: i64,
        text: String,
        color: String,
        font_px: i64,
        underline: bool,
        href: Option<String>,
    },
}

#[derive(Debug, Clone, Serialize)]
pub struct PageViewModel {
    pub current_url: String,
    pub title: String,
    pub diagnostics: Vec<String>,
    pub content_size: ContentSize,
    pub scene_items: Vec<SceneItem>,
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
    fn open_url(&mut self, url: &str) -> AppResult<PageViewModel>;
    fn reload(&mut self) -> AppResult<PageViewModel>;
    fn back(&mut self) -> AppResult<PageViewModel>;
    fn forward(&mut self) -> AppResult<PageViewModel>;
    fn get_page_view(&self) -> PageViewModel;
    fn set_viewport(&mut self, width: i64, height: i64) -> AppResult<PageViewModel>;
    fn get_navigation_state(&self) -> NavigationState;
    fn new_tab(&mut self) -> TabSummary;
    fn switch_tab(&mut self, id: u32) -> AppResult<PageViewModel>;
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
    fn open_url(&mut self, url: &str) -> AppResult<PageViewModel> {
        self.active_session_mut()?.open_url(url)
    }

    fn reload(&mut self) -> AppResult<PageViewModel> {
        self.active_session_mut()?.reload()
    }

    fn back(&mut self) -> AppResult<PageViewModel> {
        self.active_session_mut()?.back()
    }

    fn forward(&mut self) -> AppResult<PageViewModel> {
        self.active_session_mut()?.forward()
    }

    fn get_page_view(&self) -> PageViewModel {
        self.active_session()
            .map(BrowserSession::get_page_view)
            .unwrap_or_else(|_| BrowserSession::new().get_page_view())
    }

    fn set_viewport(&mut self, width: i64, height: i64) -> AppResult<PageViewModel> {
        self.active_session_mut()?.set_viewport(width, height)
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

    fn switch_tab(&mut self, id: u32) -> AppResult<PageViewModel> {
        if self.tabs.iter().any(|tab| tab.id == id) {
            self.active_tab_id = id;
            return self.active_session().map(BrowserSession::get_page_view);
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
                title: visible_tab_title(&tab.session.latest_view),
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
                title: visible_tab_title(&tab.session.latest_view),
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
    latest_view: PageViewModel,
    latest_html: Option<String>,
    latest_extra_style: String,
    viewport_width: i64,
    viewport_height: i64,
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
            latest_view: blank_page_view(),
            latest_html: None,
            latest_extra_style: String::new(),
            viewport_width: DEFAULT_VIEWPORT_WIDTH,
            viewport_height: DEFAULT_VIEWPORT_HEIGHT,
        }
    }

    fn open_url(&mut self, url: &str) -> AppResult<PageViewModel> {
        let normalized_url = normalize_url(url)?;
        let view = self.load_url(&normalized_url)?;
        self.record_history(normalized_url);
        Ok(view)
    }

    fn get_page_view(&self) -> PageViewModel {
        self.latest_view.clone()
    }

    fn set_viewport(&mut self, width: i64, height: i64) -> AppResult<PageViewModel> {
        self.viewport_width = width.max(320);
        self.viewport_height = height.max(200);

        if self.latest_html.is_some() {
            return self.rerender_cached_page();
        }

        Ok(self.latest_view.clone())
    }

    fn reload(&mut self) -> AppResult<PageViewModel> {
        let current = self
            .history
            .get(self.history_index)
            .cloned()
            .ok_or_else(|| AppError::state("No page to reload"))?;
        self.load_url(&current)
    }

    fn back(&mut self) -> AppResult<PageViewModel> {
        if self.history_index == 0 || self.history.is_empty() {
            return Err(AppError::state("No back history"));
        }

        self.navigate_to_history_index(self.history_index - 1)
    }

    fn forward(&mut self) -> AppResult<PageViewModel> {
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

    fn navigate_to_history_index(&mut self, index: usize) -> AppResult<PageViewModel> {
        let target = self
            .history
            .get(index)
            .cloned()
            .ok_or_else(|| AppError::state("History target is unavailable"))?;

        let view = self.load_url(&target)?;
        self.history_index = index;

        Ok(view)
    }

    fn load_url(&mut self, url: &str) -> AppResult<PageViewModel> {
        let response = fetch_http_response(url)?;
        let html = response.body();
        let mut probe_page = Page::new();
        probe_page.receive_response(response.clone(), String::new(), self.viewport_width);

        let mut diagnostics = Vec::new();
        let extra_style = fetch_stylesheet_bundle(url, &probe_page, &mut diagnostics);

        let mut page = Page::new();
        page.receive_response(response, extra_style.clone(), self.viewport_width);

        let view = build_page_view(&page, url.to_string(), diagnostics);
        self.latest_html = Some(html);
        self.latest_extra_style = extra_style;
        self.latest_view = view.clone();
        Ok(view)
    }


    fn rerender_cached_page(&mut self) -> AppResult<PageViewModel> {
        let html = self
            .latest_html
            .clone()
            .ok_or_else(|| AppError::state("No cached page to render"))?;
        let current_url = self
            .history
            .get(self.history_index)
            .cloned()
            .unwrap_or_default();
        let response = http_response_from_html(&html)?;
        let mut page = Page::new();
        page.receive_response(response, self.latest_extra_style.clone(), self.viewport_width);
        let view = build_page_view(&page, current_url, Vec::new());
        self.latest_view = view.clone();
        Ok(view)
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

fn blank_page_view() -> PageViewModel {
    PageViewModel {
        current_url: String::new(),
        title: "New Tab".to_string(),
        diagnostics: vec!["No page loaded".to_string()],
        content_size: ContentSize {
            width: DEFAULT_VIEWPORT_WIDTH,
            height: DEFAULT_VIEWPORT_HEIGHT,
        },
        scene_items: Vec::new(),
    }
}

fn visible_tab_title(view: &PageViewModel) -> String {
    if !view.title.trim().is_empty() {
        return view.title.clone();
    }

    if !view.current_url.trim().is_empty() {
        return view.current_url.clone();
    }

    "New Tab".to_string()
}

fn normalize_url(url: &str) -> AppResult<String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(AppError::validation("URL is empty"));
    }

    let normalized = if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };

    validate_url(&normalized)?;
    Ok(normalized)
}

fn validate_url(url: &str) -> AppResult<()> {
    let candidate = if url.starts_with("https://") {
        url.replacen("https://", "http://", 1)
    } else {
        url.to_string()
    };

    Url::new(candidate)
        .parse()
        .map(|_| ())
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

fn http_response_from_html(html: &str) -> AppResult<HttpResponse> {
    HttpResponse::new(format!("HTTP/1.1 200 OK\n\n{html}"))
        .map_err(|e| AppError::parse(format!("Failed to parse response: {e:?}")))
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

fn fetch_url_text(url: &str) -> AppResult<String> {
    let client = Client::builder()
        .build()
        .map_err(|e| AppError::network(format!("Failed to build HTTP client: {e}")))?;

    client
        .get(url)
        .send()
        .and_then(|response| response.error_for_status())
        .map_err(|e| AppError::network(format!("Failed to fetch resource: {e}")))?
        .text()
        .map_err(|e| AppError::network(format!("Failed to read resource body: {e}")))
}

fn fetch_stylesheet_bundle(url: &str, page: &Page, diagnostics: &mut Vec<String>) -> String {
    let mut styles = Vec::new();
    for href in page.stylesheet_links() {
        match resolve_url(url, &href).and_then(|resolved| fetch_url_text(&resolved)) {
            Ok(sheet) => styles.push(sheet),
            Err(error) => diagnostics.push(format!("Stylesheet load failed for {href}: {}", error.message)),
        }
    }
    styles.join("\n")
}

fn build_page_view(page: &Page, current_url: String, mut diagnostics: Vec<String>) -> PageViewModel {
    let mut scene_items = Vec::new();
    for item in page.display_items() {
        match item {
            DisplayItem::Rect {
                style,
                layout_point,
                layout_size,
            } => {
                if layout_size.width() <= 0 || layout_size.height() <= 0 {
                    continue;
                }
                let background = style.background_color().code().to_string();
                if background == "#ffffff" {
                    continue;
                }
                scene_items.push(SceneItem::Rect {
                    x: layout_point.x(),
                    y: layout_point.y(),
                    width: layout_size.width(),
                    height: layout_size.height(),
                    background_color: background,
                });
            }
            DisplayItem::Text {
                text,
                style,
                layout_point,
                href,
            } => {
                if text.trim().is_empty() {
                    continue;
                }
                scene_items.push(SceneItem::Text {
                    x: layout_point.x(),
                    y: layout_point.y(),
                    text,
                    color: style.color().code().to_string(),
                    font_px: style.font_size().px(),
                    underline: style.text_decoration()
                        == saba_core::renderer::layout::computed_style::TextDecoration::Underline,
                    href: href.and_then(|value| resolve_url(&current_url, &value).ok()),
                });
            }
        }
    }

    if scene_items.is_empty() {
        diagnostics.push("No renderable scene items produced".to_string());
    }

    let content_size = page.content_size();

    PageViewModel {
        current_url,
        title: page.title().unwrap_or_else(|| "CosmoBrowse".to_string()),
        diagnostics,
        content_size: content_size_from_layout(content_size),
        scene_items,
    }
}

fn content_size_from_layout(size: LayoutSize) -> ContentSize {
    ContentSize {
        width: size.width().max(DEFAULT_VIEWPORT_WIDTH),
        height: size.height().max(DEFAULT_VIEWPORT_HEIGHT),
    }
}

fn resolve_url(base_url: &str, target: &str) -> AppResult<String> {
    let target = target.trim();
    if target.is_empty() {
        return Err(AppError::validation("URL is empty"));
    }

    if target.starts_with("http://") || target.starts_with("https://") {
        return Ok(target.to_string());
    }

    let (scheme, authority, base_path) = split_url(base_url)?;

    if target.starts_with("//") {
        return Ok(format!("{scheme}:{target}"));
    }

    if target.starts_with('/') {
        return Ok(format!("{scheme}://{authority}{target}"));
    }

    let mut segments = base_path
        .split('/')
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>();
    if !base_path.ends_with('/') {
        segments.pop();
    }

    for segment in target.split('/') {
        match segment {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            _ => segments.push(segment),
        }
    }

    let resolved_path = if segments.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", segments.join("/"))
    };

    Ok(format!("{scheme}://{authority}{resolved_path}"))
}

fn split_url(url: &str) -> AppResult<(String, String, String)> {
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(AppError::validation("Base URL is missing a scheme"));
    };
    let Some((authority, remainder)) = rest.split_once('/') else {
        return Ok((scheme.to_string(), rest.to_string(), "/".to_string()));
    };
    let path = format!("/{}", remainder.split('?').next().unwrap_or_default());
    Ok((scheme.to_string(), authority.to_string(), path))
}

#[cfg(test)]
mod tests {
    use super::{
        blank_page_view, build_page_view, build_search_results, normalize_url, resolve_url,
        AppService, BrowserSession, SceneItem, SabaApp,
    };
    use saba_core::http::HttpResponse;
    use saba_core::renderer::page::Page;

    fn page_from_html(html: &str) -> Page {
        let mut page = Page::new();
        let response = HttpResponse::new(format!("HTTP/1.1 200 OK\n\n{html}")).expect("valid response");
        page.receive_response(response, String::new(), 800);
        page
    }

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
    fn resolve_relative_url() {
        assert_eq!(
            resolve_url("http://example.com/docs/page.html", "guide/index.css").expect("resolved"),
            "http://example.com/docs/guide/index.css"
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

    #[test]
    fn page_view_contains_resolved_link_and_title() {
        let page = page_from_html(
            r#"<html><head><title>Example</title></head><body><p><a href="/docs">Learn</a></p></body></html>"#,
        );
        let view = build_page_view(&page, "http://example.com".to_string(), Vec::new());

        assert_eq!(view.title, "Example");
        assert!(view.scene_items.iter().any(|item| matches!(
            item,
            SceneItem::Text { href: Some(href), .. } if href == "http://example.com/docs"
        )));
    }


    #[test]
    fn cached_html_rerender_preserves_scene_items() {
        let mut session = BrowserSession::new();
        session.latest_html = Some(
            "<html><head><title>Example</title></head><body><p>Hello world</p></body></html>"
                .to_string(),
        );
        session.history.push("http://example.com".to_string());
        session.history_index = 0;

        let view = session.rerender_cached_page().expect("rerender should succeed");

        assert_eq!(view.current_url, "http://example.com");
        assert_eq!(view.title, "Example");
        assert!(view
            .scene_items
            .iter()
            .any(|item| matches!(item, SceneItem::Text { text, .. } if text.contains("Hello"))));
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
        let mut app = SabaApp::default();
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
    }

    #[test]
    fn closing_last_tab_keeps_one_blank_tab() {
        let mut app = SabaApp::default();
        let id = app.list_tabs()[0].id;

        let listed = app.close_tab(id).expect("close must succeed");
        assert_eq!(listed.len(), 1);
        assert!(listed[0].is_active);
        assert_eq!(blank_page_view().title, "New Tab");
    }

    #[test]
    fn search_returns_results_for_query() {
        let results = build_search_results("rust browser").expect("search should succeed");
        assert!(results.len() >= 2);
        assert!(results.iter().any(|r| r.url.contains("duckduckgo.com")));
        assert!(results.iter().all(|r| r.url.starts_with("http")));
    }

    #[test]
    fn search_empty_query_is_validation_error() {
        let err = build_search_results("  ").expect_err("must fail");
        assert_eq!(err.code, "validation_error");
    }
}










