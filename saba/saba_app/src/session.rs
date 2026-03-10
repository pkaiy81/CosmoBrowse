use crate::loader::{build_frame_id, fetch_document, parse_frameset_document, prepare_html_for_display, resolve_url};
use crate::model::{
    AppError, AppMetricsSnapshot, AppResult, AppService, ContentSize, ErrorMetric, FrameRect,
    FrameViewModel, NavigationEvent, NavigationState, PageViewModel, SearchResult,
    TabSummary,
};
use std::collections::BTreeMap;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

pub const DEFAULT_VIEWPORT_WIDTH: i64 = 960;
pub const DEFAULT_VIEWPORT_HEIGHT: i64 = 720;

#[derive(Debug)]
pub struct SabaApp {
    tabs: Vec<Tab>,
    active_tab_id: u32,
    next_tab_id: u32,
    metrics: AppMetrics,
}

impl Default for SabaApp {
    fn default() -> Self {
        Self { tabs: vec![Tab::new(1)], active_tab_id: 1, next_tab_id: 2, metrics: AppMetrics::default() }
    }
}

impl AppService for SabaApp {
    fn open_url(&mut self, url: &str) -> AppResult<PageViewModel> {
        self.execute_navigation("open_url", Some(url.to_string()), |session| session.open_url(url))
    }

    fn activate_link(&mut self, frame_id: &str, href: &str, target: Option<&str>) -> AppResult<PageViewModel> {
        self.execute_navigation("activate_link", Some(href.to_string()), |session| session.activate_link(frame_id, href, target))
    }

    fn reload(&mut self) -> AppResult<PageViewModel> {
        let url = self.active_session().ok().and_then(BrowserSession::current_url);
        self.execute_navigation("reload", url, BrowserSession::reload)
    }

    fn back(&mut self) -> AppResult<PageViewModel> {
        let url = self.active_session().ok().and_then(BrowserSession::previous_url);
        self.execute_navigation("back", url, BrowserSession::back)
    }

    fn forward(&mut self) -> AppResult<PageViewModel> {
        let url = self.active_session().ok().and_then(BrowserSession::next_url);
        self.execute_navigation("forward", url, BrowserSession::forward)
    }

    fn get_page_view(&self) -> PageViewModel {
        self.active_session().map(BrowserSession::get_page_view).unwrap_or_else(|_| blank_page_view(DEFAULT_VIEWPORT_WIDTH, DEFAULT_VIEWPORT_HEIGHT))
    }

    fn set_viewport(&mut self, width: i64, height: i64) -> AppResult<PageViewModel> {
        self.active_session_mut()?.set_viewport(width, height)
    }

    fn get_navigation_state(&self) -> NavigationState {
        self.active_session().map(BrowserSession::navigation_state).unwrap_or_else(|_| BrowserSession::new().navigation_state())
    }

    fn get_metrics(&self) -> AppMetricsSnapshot {
        self.metrics.snapshot()
    }

    fn new_tab(&mut self) -> TabSummary {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(Tab::new(id));
        self.active_tab_id = id;
        self.tab_summary(id).expect("new tab should exist")
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
            let replacement = self.next_tab_id;
            self.next_tab_id += 1;
            self.tabs.push(Tab::new(replacement));
        }
        if self.active_tab_id == id {
            self.active_tab_id = self.tabs.get(index.saturating_sub(1)).or_else(|| self.tabs.first()).map(|tab| tab.id).ok_or_else(|| AppError::state("Failed to resolve active tab"))?;
        }
        Ok(self.list_tabs())
    }

    fn list_tabs(&self) -> Vec<TabSummary> {
        self.tabs.iter().map(|tab| TabSummary { id: tab.id, title: visible_tab_title(&tab.session.get_page_view()), url: tab.session.current_url(), is_active: tab.id == self.active_tab_id }).collect()
    }

    fn search(&self, query: &str) -> AppResult<Vec<SearchResult>> {
        build_search_results(query)
    }
}

impl SabaApp {
    fn active_session_mut(&mut self) -> AppResult<&mut BrowserSession> {
        self.tabs.iter_mut().find(|tab| tab.id == self.active_tab_id).map(|tab| &mut tab.session).ok_or_else(|| AppError::state("Active tab is unavailable"))
    }

    fn active_session(&self) -> AppResult<&BrowserSession> {
        self.tabs.iter().find(|tab| tab.id == self.active_tab_id).map(|tab| &tab.session).ok_or_else(|| AppError::state("Active tab is unavailable"))
    }

    fn tab_summary(&self, id: u32) -> Option<TabSummary> {
        self.tabs.iter().find(|tab| tab.id == id).map(|tab| TabSummary { id, title: visible_tab_title(&tab.session.get_page_view()), url: tab.session.current_url(), is_active: id == self.active_tab_id })
    }

    fn execute_navigation<F>(&mut self, command: &str, url: Option<String>, action: F) -> AppResult<PageViewModel>
    where
        F: FnOnce(&mut BrowserSession) -> AppResult<PageViewModel>,
    {
        let start = Instant::now();
        let result = match self.active_session_mut() {
            Ok(session) => action(session),
            Err(error) => Err(error),
        };
        self.metrics.record_navigation(command, url.as_deref(), start.elapsed(), &result);
        result
    }
}

#[derive(Debug)]
struct Tab {
    id: u32,
    session: BrowserSession,
}

impl Tab {
    fn new(id: u32) -> Self {
        Self { id, session: BrowserSession::new() }
    }
}

#[derive(Debug, Default)]
struct BrowserSession {
    history: Vec<PageViewModel>,
    history_index: usize,
    viewport_width: i64,
    viewport_height: i64,
}

impl BrowserSession {
    fn new() -> Self {
        Self { history: Vec::new(), history_index: 0, viewport_width: DEFAULT_VIEWPORT_WIDTH, viewport_height: DEFAULT_VIEWPORT_HEIGHT }
    }

    fn open_url(&mut self, url: &str) -> AppResult<PageViewModel> {
        let normalized_url = normalize_url(url)?;
        let view = self.load_page(&normalized_url, None)?;
        self.push_history(view.clone());
        Ok(view)
    }

    fn activate_link(&mut self, frame_id: &str, href: &str, target: Option<&str>) -> AppResult<PageViewModel> {
        let normalized_href = normalize_url_like(href)?;
        let current = self.current_page().ok_or_else(|| AppError::state("No page loaded"))?;
        if target == Some("_top") || current.root_frame.child_frames.is_empty() {
            let view = self.load_page(&normalized_href, None)?;
            self.push_history(view.clone());
            return Ok(view);
        }
        let mut overrides = snapshot_frame_urls(&current.root_frame);
        let destination = resolve_destination_frame_id(&current.root_frame, frame_id, target).ok_or_else(|| AppError::state("Target frame is unavailable"))?;
        overrides.insert(destination, normalized_href);
        let view = self.load_page(&current.current_url, Some(&overrides))?;
        self.push_history(view.clone());
        Ok(view)
    }

    fn get_page_view(&self) -> PageViewModel {
        self.current_page().unwrap_or_else(|| blank_page_view(self.viewport_width, self.viewport_height))
    }

    fn set_viewport(&mut self, width: i64, height: i64) -> AppResult<PageViewModel> {
        self.viewport_width = width.max(320);
        self.viewport_height = height.max(200);
        let Some(current) = self.current_page() else {
            return Ok(blank_page_view(self.viewport_width, self.viewport_height));
        };
        let overrides = snapshot_frame_urls(&current.root_frame);
        let rerendered = self.load_page(&current.current_url, Some(&overrides))?;
        self.history[self.history_index] = rerendered.clone();
        Ok(rerendered)
    }

    fn reload(&mut self) -> AppResult<PageViewModel> {
        let current = self.current_page().ok_or_else(|| AppError::state("No page to reload"))?;
        let overrides = snapshot_frame_urls(&current.root_frame);
        let rerendered = self.load_page(&current.current_url, Some(&overrides))?;
        self.history[self.history_index] = rerendered.clone();
        Ok(rerendered)
    }

    fn back(&mut self) -> AppResult<PageViewModel> {
        if self.history.is_empty() || self.history_index == 0 {
            return Err(AppError::state("No back history"));
        }
        self.history_index -= 1;
        Ok(self.history[self.history_index].clone())
    }

    fn forward(&mut self) -> AppResult<PageViewModel> {
        if self.history.is_empty() || self.history_index + 1 >= self.history.len() {
            return Err(AppError::state("No forward history"));
        }
        self.history_index += 1;
        Ok(self.history[self.history_index].clone())
    }

    fn navigation_state(&self) -> NavigationState {
        NavigationState { can_back: !self.history.is_empty() && self.history_index > 0, can_forward: !self.history.is_empty() && self.history_index + 1 < self.history.len(), current_url: self.current_url() }
    }

    fn current_page(&self) -> Option<PageViewModel> {
        self.history.get(self.history_index).cloned()
    }

    fn current_url(&self) -> Option<String> {
        self.current_page().map(|page| page.current_url)
    }

    fn previous_url(&self) -> Option<String> {
        self.history.get(self.history_index.saturating_sub(1)).map(|page| page.current_url.clone())
    }

    fn next_url(&self) -> Option<String> {
        self.history.get(self.history_index + 1).map(|page| page.current_url.clone())
    }

    fn load_page(&self, url: &str, overrides: Option<&BTreeMap<String, String>>) -> AppResult<PageViewModel> {
        let root_rect = FrameRect { x: 0, y: 0, width: self.viewport_width.max(320), height: self.viewport_height.max(200) };
        let root_frame = load_frame_recursive("root", None, url, root_rect.clone(), overrides)?;
        Ok(PageViewModel {
            current_url: root_frame.current_url.clone(),
            title: page_title_from_root(&root_frame),
            diagnostics: collect_diagnostics(&root_frame),
            content_size: ContentSize { width: root_rect.width, height: root_rect.height },
            scene_items: Vec::new(),
            root_frame,
        })
    }

    fn push_history(&mut self, view: PageViewModel) {
        if self.history_index + 1 < self.history.len() {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(view);
        self.history_index = self.history.len() - 1;
    }
}

#[derive(Debug, Default)]
struct AppMetrics {
    total_navigations: u64,
    successful_navigations: u64,
    failed_navigations: u64,
    total_duration_ms: u128,
    last_duration_ms: Option<u64>,
    error_counts: BTreeMap<String, u64>,
    recent_events: Vec<NavigationEvent>,
}

impl AppMetrics {
    fn record_navigation(&mut self, command: &str, url: Option<&str>, duration: Duration, result: &AppResult<PageViewModel>) {
        let duration_ms = duration.as_millis().min(u64::MAX as u128) as u64;
        self.total_navigations += 1;
        self.total_duration_ms += duration.as_millis();
        self.last_duration_ms = Some(duration_ms);
        let (success, error_code) = match result {
            Ok(_) => { self.successful_navigations += 1; (true, None) }
            Err(error) => {
                self.failed_navigations += 1;
                *self.error_counts.entry(error.code.clone()).or_insert(0) += 1;
                (false, Some(error.code.clone()))
            }
        };
        self.recent_events.push(NavigationEvent { command: command.to_string(), url: url.map(str::to_string), duration_ms, success, error_code, recorded_at_ms: unix_timestamp_ms() });
        if self.recent_events.len() > 20 { self.recent_events.remove(0); }
    }

    fn snapshot(&self) -> AppMetricsSnapshot {
        let average_duration_ms = if self.total_navigations == 0 { 0 } else { (self.total_duration_ms / u128::from(self.total_navigations)) as u64 };
        AppMetricsSnapshot {
            total_navigations: self.total_navigations,
            successful_navigations: self.successful_navigations,
            failed_navigations: self.failed_navigations,
            average_duration_ms,
            last_duration_ms: self.last_duration_ms,
            error_counts: self.error_counts.iter().map(|(code, count)| ErrorMetric { code: code.clone(), count: *count }).collect(),
            recent_events: self.recent_events.clone(),
        }
    }
}

fn load_frame_recursive(frame_id: &str, frame_name: Option<String>, url: &str, rect: FrameRect, overrides: Option<&BTreeMap<String, String>>) -> AppResult<FrameViewModel> {
    let loaded = fetch_document(url)?;
    let title = loaded.title.clone().unwrap_or_else(|| loaded.final_url.clone());
    if let Some(frameset) = parse_frameset_document(&loaded.html) {
        let child_rects = frameset.child_rects(&rect);
        let mut child_frames = Vec::new();
        for (index, spec) in frameset.frames.iter().enumerate() {
            let child_rect = child_rects.get(index).cloned().unwrap_or(FrameRect { x: rect.x, y: rect.y, width: rect.width, height: rect.height });
            let child_id = build_frame_id(frame_id, spec.name.as_deref(), index);
            let default_url = resolve_url(&loaded.final_url, &spec.src)?;
            let child_url = overrides.and_then(|map| map.get(&child_id).cloned()).unwrap_or(default_url);
            child_frames.push(load_frame_recursive(&child_id, spec.name.clone(), &child_url, child_rect, overrides)?);
        }
        return Ok(FrameViewModel { id: frame_id.to_string(), name: frame_name, current_url: loaded.final_url, title, diagnostics: loaded.diagnostics, rect: rect.clone(), content_size: ContentSize { width: rect.width, height: rect.height }, scene_items: Vec::new(), html_content: None, child_frames });
    }

    let prepared_html = prepare_html_for_display(&loaded.html, &loaded.final_url, frame_id);
    Ok(FrameViewModel { id: frame_id.to_string(), name: frame_name, current_url: loaded.final_url, title, diagnostics: loaded.diagnostics, rect: rect.clone(), content_size: ContentSize { width: rect.width, height: rect.height }, scene_items: Vec::new(), html_content: Some(prepared_html), child_frames: Vec::new() })
}

fn blank_page_view(width: i64, height: i64) -> PageViewModel {
    PageViewModel {
        current_url: String::new(),
        title: "New Tab".to_string(),
        diagnostics: vec!["No page loaded".to_string()],
        content_size: ContentSize { width, height },
        scene_items: Vec::new(),
        root_frame: FrameViewModel {
            id: "root".to_string(),
            name: None,
            current_url: String::new(),
            title: "New Tab".to_string(),
            diagnostics: vec!["No page loaded".to_string()],
            rect: FrameRect { x: 0, y: 0, width, height },
            content_size: ContentSize { width, height },
            scene_items: Vec::new(),
            html_content: Some("<html><head><meta charset=\"utf-8\"></head><body></body></html>".to_string()),
            child_frames: Vec::new(),
        },
    }
}

fn visible_tab_title(view: &PageViewModel) -> String {
    if !view.title.trim().is_empty() { return view.title.clone(); }
    if !view.current_url.trim().is_empty() { return view.current_url.clone(); }
    "New Tab".to_string()
}

fn page_title_from_root(root: &FrameViewModel) -> String {
    if !root.title.trim().is_empty() { return root.title.clone(); }
    root.child_frames.iter().find_map(|child| if child.title.trim().is_empty() { None } else { Some(child.title.clone()) }).unwrap_or_else(|| "CosmoBrowse".to_string())
}

fn collect_diagnostics(frame: &FrameViewModel) -> Vec<String> {
    let mut diagnostics = frame.diagnostics.clone();
    for child in &frame.child_frames { diagnostics.extend(collect_diagnostics(child)); }
    diagnostics
}

fn snapshot_frame_urls(frame: &FrameViewModel) -> BTreeMap<String, String> {
    let mut urls = BTreeMap::new();
    collect_frame_urls(frame, &mut urls);
    urls
}

fn collect_frame_urls(frame: &FrameViewModel, urls: &mut BTreeMap<String, String>) {
    if !frame.current_url.is_empty() { urls.insert(frame.id.clone(), frame.current_url.clone()); }
    for child in &frame.child_frames { collect_frame_urls(child, urls); }
}

fn resolve_destination_frame_id(root: &FrameViewModel, source_frame_id: &str, target: Option<&str>) -> Option<String> {
    let target = target.unwrap_or("_self").trim();
    if target.is_empty() || target == "_self" || target == "_parent" { return Some(source_frame_id.to_string()); }
    find_frame_id_by_name(root, target)
}

fn find_frame_id_by_name(frame: &FrameViewModel, target: &str) -> Option<String> {
    if frame.name.as_deref() == Some(target) { return Some(frame.id.clone()); }
    for child in &frame.child_frames {
        if let Some(id) = find_frame_id_by_name(child, target) { return Some(id); }
    }
    None
}

// Ref: RFC 3986 scheme handling for absolute URIs.
// https://datatracker.ietf.org/doc/html/rfc3986#section-3.1
fn normalize_url(input: &str) -> AppResult<String> {
    let trimmed = input.trim();
    if trimmed.starts_with("fixture://") {
        return Ok(trimmed.to_string());
    }
    if trimmed.is_empty() {
        return Err(AppError::validation("URL is empty"));
    }
    let candidate = if trimmed.find(':').is_some_and(|index| index > 0) {
        trimmed.to_string()
    } else {
        format!("http://{trimmed}")
    };
    let parsed = Url::parse(&candidate)
        .map_err(|error| AppError::validation(format!("Invalid URL: {error}")))?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed.to_string()),
        other => Err(AppError::validation(format!("Unsupported scheme: {other}"))),
    }
}

fn normalize_url_like(input: &str) -> AppResult<String> {
    if input.trim().starts_with("mailto:") { return Ok(input.trim().to_string()); }
    normalize_url(input)
}

fn build_search_results(query: &str) -> AppResult<Vec<SearchResult>> {
    let normalized = query.trim();
    if normalized.is_empty() { return Err(AppError::validation("Search query is empty")); }
    let encoded = normalized.replace(' ', "+");
    let mut results = vec![
        SearchResult { id: 1, title: format!("Search \"{normalized}\" on DuckDuckGo"), url: format!("https://duckduckgo.com/?q={encoded}"), snippet: "Open web search results in DuckDuckGo".to_string() },
        SearchResult { id: 2, title: format!("Search \"{normalized}\" on Wikipedia"), url: format!("https://en.wikipedia.org/w/index.php?search={encoded}"), snippet: "Open Wikipedia search results".to_string() },
    ];
    if !normalized.contains(' ') && normalized.contains('.') {
        if let Ok(candidate) = normalize_url(normalized) {
            results.insert(0, SearchResult { id: 0, title: format!("Open {normalized}"), url: candidate, snippet: "Detected URL-like input".to_string() });
        }
    }
    Ok(results)
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|duration| duration.as_millis().min(u64::MAX as u128) as u64).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{decode_html_bytes, load_fixture_document, parse_frameset_document};
    use encoding_rs::SHIFT_JIS;

    #[test]
    fn normalize_https_url_keeps_scheme() {
        assert_eq!(normalize_url("https://abehiroshi.la.coocan.jp/").expect("valid url"), "https://abehiroshi.la.coocan.jp/");
    }

    #[test]
    fn decode_shift_jis_from_meta() {
        let (encoded, _, _) = SHIFT_JIS.encode("<meta charset=Shift_JIS><title>阿部寛</title>");
        let decoded = decode_html_bytes(encoded.as_ref(), Some("text/html"));
        assert!(decoded.html.contains("阿部寛"));
    }

    #[test]
    fn parse_fixture_frameset() {
        let fixture = load_fixture_document("fixture://abehiroshi/index").expect("fixture should load");
        let frameset = parse_frameset_document(&fixture.html).expect("frameset should parse");
        assert_eq!(frameset.frames.len(), 2);
    }

    #[test]
    fn open_fixture_root_loads_named_frames() {
        let mut session = BrowserSession::new();
        let view = session.open_url("fixture://abehiroshi/index").expect("fixture should load");
        assert_eq!(view.root_frame.child_frames.len(), 2);
        assert_eq!(view.root_frame.child_frames[0].name.as_deref(), Some("left"));
        assert_eq!(view.root_frame.child_frames[1].name.as_deref(), Some("right"));
    }

    #[test]
    fn activating_named_frame_replaces_only_target_frame() {
        let mut session = BrowserSession::new();
        let view = session.open_url("fixture://abehiroshi/index").expect("fixture should load");
        let left = view.root_frame.child_frames[0].id.clone();
        let right_before = view.root_frame.child_frames[1].current_url.clone();
        let next = session.activate_link(&left, "fixture://abehiroshi/prof", Some("right")).expect("targeted navigation should work");
        assert_eq!(next.root_frame.child_frames[0].current_url, view.root_frame.child_frames[0].current_url);
        assert_ne!(next.root_frame.child_frames[1].current_url, right_before);
    }
}






