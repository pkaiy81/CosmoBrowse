use crate::layout::{build_layout_scene_with_script_runtime, RelayoutTrigger};
use crate::loader::{
    build_frame_id, fetch_document, parse_frameset_document, prepare_html_for_display,
    register_tls_exception_for_url, resolve_url, FramesetChild, FramesetSpec, LoadedDocument,
};
use crate::model::{
    AppError, AppMetricsSnapshot, AppResult, AppService, ContentSize, ErrorMetric, FrameRect,
    FrameScrollPositionSnapshot, FrameUrlOverrideSnapshot, FrameViewModel, HistoryEntrySnapshot,
    NavigationEvent, NavigationState, NavigationType, PageViewModel, RenderBackendKind,
    ScriptEngine, ScrollPosition, SearchResult, SessionSnapshot, TabSessionSnapshot, TabSummary,
    SESSION_SNAPSHOT_SCHEMA_VERSION,
};
use crate::security::{apply_minimum_csp, enforce_mixed_content_policy, is_same_origin};
use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use url::Url;

pub const DEFAULT_VIEWPORT_WIDTH: i64 = 960;
pub const DEFAULT_VIEWPORT_HEIGHT: i64 = 720;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MinimumScriptEngine;

impl ScriptEngine for MinimumScriptEngine {
    fn name(&self) -> &'static str {
        "saba-js-runtime-minimum"
    }

    fn can_execute(&self, frame: &FrameViewModel) -> bool {
        frame.html_content.is_some()
    }
}

#[derive(Debug)]
pub struct SabaApp {
    tabs: Vec<Tab>,
    active_tab_id: u32,
    next_tab_id: u32,
    metrics: AppMetrics,
}

impl Default for SabaApp {
    fn default() -> Self {
        Self {
            tabs: vec![Tab::new(1)],
            active_tab_id: 1,
            next_tab_id: 2,
            metrics: AppMetrics::default(),
        }
    }
}

impl AppService for SabaApp {
    fn open_url(&mut self, url: &str) -> AppResult<PageViewModel> {
        self.execute_navigation("open_url", Some(url.to_string()), |session| {
            session.open_url(url)
        })
    }

    fn activate_link(
        &mut self,
        frame_id: &str,
        href: &str,
        target: Option<&str>,
    ) -> AppResult<PageViewModel> {
        self.execute_navigation("activate_link", Some(href.to_string()), |session| {
            session.activate_link(frame_id, href, target)
        })
    }

    fn reload(&mut self) -> AppResult<PageViewModel> {
        let url = self
            .active_session()
            .ok()
            .and_then(BrowserSession::current_url);
        self.execute_navigation("reload", url, BrowserSession::reload)
    }

    fn back(&mut self) -> AppResult<PageViewModel> {
        let url = self
            .active_session()
            .ok()
            .and_then(BrowserSession::previous_url);
        self.execute_navigation("back", url, BrowserSession::back)
    }

    fn forward(&mut self) -> AppResult<PageViewModel> {
        let url = self
            .active_session()
            .ok()
            .and_then(BrowserSession::next_url);
        self.execute_navigation("forward", url, BrowserSession::forward)
    }

    fn get_page_view(&self) -> PageViewModel {
        self.active_session()
            .map(BrowserSession::get_page_view)
            .unwrap_or_else(|_| blank_page_view(DEFAULT_VIEWPORT_WIDTH, DEFAULT_VIEWPORT_HEIGHT))
    }

    fn set_viewport(&mut self, width: i64, height: i64) -> AppResult<PageViewModel> {
        self.active_session_mut()?.set_viewport(width, height)
    }

    fn get_navigation_state(&self) -> NavigationState {
        self.active_session()
            .map(BrowserSession::navigation_state)
            .unwrap_or_else(|_| BrowserSession::new().navigation_state())
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
                title: visible_tab_title(&tab.session.get_page_view()),
                url: tab.session.current_url(),
                is_active: tab.id == self.active_tab_id,
            })
            .collect()
    }

    fn search(&self, query: &str) -> AppResult<Vec<SearchResult>> {
        build_search_results(query)
    }

    fn script_engine(&self) -> Box<dyn ScriptEngine> {
        Box::new(MinimumScriptEngine)
    }

    fn export_session_snapshot(&self) -> SessionSnapshot {
        SabaApp::export_session_snapshot(self)
    }

    fn import_session_snapshot(&mut self, snapshot: SessionSnapshot) -> AppResult<()> {
        SabaApp::import_session_snapshot(self, snapshot)
    }

    fn update_scroll_positions(
        &mut self,
        positions: Vec<FrameScrollPositionSnapshot>,
    ) -> AppResult<()> {
        self.active_session_mut()?
            .update_scroll_positions(&positions)
    }
}

impl SabaApp {
    pub fn register_tls_exception(&mut self, url: &str) -> AppResult<()> {
        register_tls_exception_for_url(url)?;
        Ok(())
    }

    pub fn export_session_snapshot(&self) -> SessionSnapshot {
        SessionSnapshot {
            version: SESSION_SNAPSHOT_SCHEMA_VERSION,
            active_tab_id: self.active_tab_id,
            tabs: self.tabs.iter().map(Tab::snapshot).collect(),
        }
    }

    pub fn import_session_snapshot(&mut self, snapshot: SessionSnapshot) -> AppResult<()> {
        let SessionSnapshot {
            version,
            active_tab_id,
            tabs: tab_snapshots,
        } = snapshot;

        if version != SESSION_SNAPSHOT_SCHEMA_VERSION {
            return Err(AppError::validation(format!(
                "Unsupported session snapshot version: {}",
                version
            )));
        }

        let mut tabs = Vec::with_capacity(tab_snapshots.len().max(1));
        let mut max_tab_id = 0;
        for tab_snapshot in tab_snapshots {
            max_tab_id = max_tab_id.max(tab_snapshot.id);
            tabs.push(Tab::from_snapshot(tab_snapshot)?);
        }
        if tabs.is_empty() {
            tabs.push(Tab::new(1));
            max_tab_id = 1;
        }

        self.tabs = tabs;
        self.active_tab_id = if self.tabs.iter().any(|tab| tab.id == active_tab_id) {
            active_tab_id
        } else {
            self.tabs
                .first()
                .map(|tab| tab.id)
                .ok_or_else(|| AppError::state("Failed to resolve restored active tab"))?
        };
        self.next_tab_id = max_tab_id + 1;
        Ok(())
    }

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
                id,
                title: visible_tab_title(&tab.session.get_page_view()),
                url: tab.session.current_url(),
                is_active: id == self.active_tab_id,
            })
    }

    fn execute_navigation<F>(
        &mut self,
        command: &str,
        url: Option<String>,
        action: F,
    ) -> AppResult<PageViewModel>
    where
        F: FnOnce(&mut BrowserSession) -> AppResult<PageViewModel>,
    {
        let start = Instant::now();
        let result = match self.active_session_mut() {
            Ok(session) => action(session),
            Err(error) => Err(error),
        };
        self.metrics
            .record_navigation(command, url.as_deref(), start.elapsed(), &result);
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
        Self {
            id,
            session: BrowserSession::new(),
        }
    }

    fn snapshot(&self) -> TabSessionSnapshot {
        TabSessionSnapshot {
            id: self.id,
            history: self.session.history_snapshot(),
            history_index: self.session.history_index,
            viewport_width: self.session.viewport_width,
            viewport_height: self.session.viewport_height,
        }
    }

    fn from_snapshot(snapshot: TabSessionSnapshot) -> AppResult<Self> {
        Ok(Self {
            id: snapshot.id,
            session: BrowserSession::from_snapshot(snapshot)?,
        })
    }
}

#[derive(Debug, Clone)]
struct HistoryEntry {
    view: PageViewModel,
    navigation_type: NavigationType,
}

#[derive(Debug, Default)]
struct BrowserSession {
    history: Vec<HistoryEntry>,
    history_index: usize,
    viewport_width: i64,
    viewport_height: i64,
}

impl BrowserSession {
    fn new() -> Self {
        Self {
            history: Vec::new(),
            history_index: 0,
            viewport_width: DEFAULT_VIEWPORT_WIDTH,
            viewport_height: DEFAULT_VIEWPORT_HEIGHT,
        }
    }

    fn open_url(&mut self, url: &str) -> AppResult<PageViewModel> {
        let normalized_url = normalize_url(url)?;
        let view = self.load_page(&normalized_url, None, Some(RelayoutTrigger::DomChanged))?;
        let navigation_type = self.classify_navigation(self.current_page().as_ref(), &view);
        self.push_history(view.clone(), navigation_type);
        Ok(view)
    }

    fn activate_link(
        &mut self,
        frame_id: &str,
        href: &str,
        target: Option<&str>,
    ) -> AppResult<PageViewModel> {
        let normalized_href = normalize_url_like(href)?;
        let current = self
            .current_page()
            .ok_or_else(|| AppError::state("No page loaded"))?;
        let normalized_target = normalize_target_keyword(target);
        if normalized_target.as_deref() == Some("_top")
            || current.root_frame.child_frames.is_empty()
        {
            let view = self.load_page(&normalized_href, None, Some(RelayoutTrigger::DomChanged))?;
            let navigation_type = self.classify_navigation(self.current_page().as_ref(), &view);
            self.push_history(view.clone(), navigation_type);
            return Ok(view);
        }
        let mut overrides = snapshot_frame_urls(&current.root_frame);
        let source_url = frame_current_url(&current.root_frame, frame_id)
            .unwrap_or_else(|| current.current_url.clone());
        enforce_mixed_content_policy(&source_url, &normalized_href)?;
        if !is_same_origin(&source_url, &normalized_href) {
            return Err(AppError::navigation_guard(format!(
                "Navigation blocked by same-origin policy: {} -> {}",
                source_url, normalized_href
            )));
        }
        let destination = resolve_destination_frame_id(
            &current.root_frame,
            frame_id,
            normalized_target.as_deref(),
        )
        .ok_or_else(|| AppError::state("Target frame is unavailable"))?;
        overrides.insert(destination, normalized_href);
        let view = self.load_page(
            &current.current_url,
            Some(&overrides),
            Some(RelayoutTrigger::DomChanged),
        )?;
        let navigation_type = self.classify_navigation(self.current_page().as_ref(), &view);
        self.push_history(view.clone(), navigation_type);
        Ok(view)
    }

    fn get_page_view(&self) -> PageViewModel {
        self.current_page()
            .unwrap_or_else(|| blank_page_view(self.viewport_width, self.viewport_height))
    }

    fn from_snapshot(snapshot: TabSessionSnapshot) -> AppResult<Self> {
        let TabSessionSnapshot {
            id: _,
            history,
            history_index,
            viewport_width,
            viewport_height,
        } = snapshot;
        let mut session = Self {
            history: Vec::with_capacity(history.len()),
            history_index: 0,
            viewport_width: viewport_width.max(320),
            viewport_height: viewport_height.max(200),
        };

        // Spec mapping: each persisted `HistoryEntrySnapshot` corresponds to an HTML
        // session history entry, `history_index` identifies the currently active
        // entry, and restoring by reloading `current_url` plus child-frame URL
        // overrides reconstructs the traversable's active document state before
        // the UI re-applies per-frame scroll offsets.
        // https://html.spec.whatwg.org/multipage/nav-history-apis.html#session-history-entry
        // https://html.spec.whatwg.org/multipage/nav-history-apis.html#history-traversal
        for entry in history {
            session.history.push(session.restore_history_entry(entry)?);
        }

        if !session.history.is_empty() {
            session.history_index = history_index.min(session.history.len() - 1);
        }

        Ok(session)
    }

    fn set_viewport(&mut self, width: i64, height: i64) -> AppResult<PageViewModel> {
        self.viewport_width = width.max(320);
        self.viewport_height = height.max(200);
        let Some(current) = self.current_page() else {
            return Ok(blank_page_view(self.viewport_width, self.viewport_height));
        };
        let overrides = snapshot_frame_urls(&current.root_frame);
        let rerendered = self.load_page(
            &current.current_url,
            Some(&overrides),
            Some(RelayoutTrigger::ViewportChanged),
        )?;
        self.history[self.history_index].view = rerendered.clone();
        Ok(rerendered)
    }

    fn reload(&mut self) -> AppResult<PageViewModel> {
        let current = self
            .current_page()
            .ok_or_else(|| AppError::state("No page to reload"))?;
        let overrides = snapshot_frame_urls(&current.root_frame);
        let rerendered = self.load_page(
            &current.current_url,
            Some(&overrides),
            Some(RelayoutTrigger::DomChanged),
        )?;
        self.history[self.history_index].view = rerendered.clone();
        Ok(rerendered)
    }

    fn back(&mut self) -> AppResult<PageViewModel> {
        let Some(previous_index) = self.previous_document_index() else {
            return Err(AppError::state("No back history"));
        };
        self.history_index = previous_index;
        Ok(self.history[self.history_index].view.clone())
    }

    fn forward(&mut self) -> AppResult<PageViewModel> {
        let Some(next_index) = self.next_document_index() else {
            return Err(AppError::state("No forward history"));
        };
        self.history_index = next_index;
        Ok(self.history[self.history_index].view.clone())
    }

    fn navigation_state(&self) -> NavigationState {
        NavigationState {
            can_back: self.previous_document_index().is_some(),
            can_forward: self.next_document_index().is_some(),
            current_url: self.current_url(),
            current_navigation_type: self.current_navigation_type(),
        }
    }

    fn current_page(&self) -> Option<PageViewModel> {
        self.history
            .get(self.history_index)
            .map(|entry| entry.view.clone())
    }

    fn current_url(&self) -> Option<String> {
        self.current_page().map(|page| page.current_url)
    }

    fn previous_url(&self) -> Option<String> {
        self.previous_document_index()
            .and_then(|index| self.history.get(index))
            .map(|entry| entry.view.current_url.clone())
    }

    fn next_url(&self) -> Option<String> {
        self.next_document_index()
            .and_then(|index| self.history.get(index))
            .map(|entry| entry.view.current_url.clone())
    }

    // Redirect chains are collapsed into a single history entry.
    // The entry keeps only the final response URL and is tagged as `Redirect`.
    fn classify_navigation(
        &self,
        previous: Option<&PageViewModel>,
        current: &PageViewModel,
    ) -> NavigationType {
        if current
            .diagnostics
            .iter()
            .any(|entry| entry.starts_with("redirect followed:"))
        {
            return NavigationType::Redirect;
        }
        if let Some(previous) = previous {
            if is_hash_navigation(&previous.current_url, &current.current_url) {
                return NavigationType::Hash;
            }
        }
        NavigationType::Document
    }

    fn current_navigation_type(&self) -> Option<NavigationType> {
        self.history
            .get(self.history_index)
            .map(|entry| entry.navigation_type)
    }

    fn previous_document_index(&self) -> Option<usize> {
        if self.history.is_empty() || self.history_index == 0 {
            return None;
        }
        (0..self.history_index)
            .rev()
            .find(|index| self.history[*index].navigation_type != NavigationType::Hash)
    }

    fn next_document_index(&self) -> Option<usize> {
        if self.history.is_empty() || self.history_index + 1 >= self.history.len() {
            return None;
        }
        ((self.history_index + 1)..self.history.len())
            .find(|index| self.history[*index].navigation_type != NavigationType::Hash)
    }

    fn load_page(
        &self,
        url: &str,
        overrides: Option<&BTreeMap<String, String>>,
        relayout_trigger: Option<RelayoutTrigger>,
    ) -> AppResult<PageViewModel> {
        let root_rect = FrameRect {
            x: 0,
            y: 0,
            width: self.viewport_width.max(320),
            height: self.viewport_height.max(200),
        };
        let root_frame = load_frame_recursive("root", None, url, root_rect.clone(), overrides)?;
        let mut diagnostics = collect_diagnostics(&root_frame);
        if let Some(trigger) = relayout_trigger {
            diagnostics.push(trigger.as_diagnostic().to_string());
        }
        Ok(PageViewModel {
            current_url: root_frame.current_url.clone(),
            title: page_title_from_root(&root_frame),
            diagnostics,
            content_size: ContentSize {
                width: root_rect.width,
                height: root_rect.height,
            },
            scene_items: Vec::new(),
            root_frame,
        })
    }

    fn push_history(&mut self, view: PageViewModel, navigation_type: NavigationType) {
        if self.history_index + 1 < self.history.len() {
            self.history.truncate(self.history_index + 1);
        }
        self.history.push(HistoryEntry {
            view,
            navigation_type,
        });
        self.history_index = self.history.len() - 1;
    }

    fn history_snapshot(&self) -> Vec<HistoryEntrySnapshot> {
        self.history
            .iter()
            .map(|entry| HistoryEntrySnapshot {
                current_url: entry.view.current_url.clone(),
                navigation_type: entry.navigation_type,
                frame_url_overrides: snapshot_frame_url_overrides(&entry.view.root_frame),
                scroll_positions: snapshot_scroll_positions(&entry.view.root_frame),
            })
            .collect()
    }

    fn restore_history_entry(&self, snapshot: HistoryEntrySnapshot) -> AppResult<HistoryEntry> {
        let overrides = frame_url_overrides_map(&snapshot.frame_url_overrides);
        let mut view = self.load_page(
            &snapshot.current_url,
            Some(&overrides),
            Some(RelayoutTrigger::DomChanged),
        )?;
        apply_scroll_snapshot(&mut view.root_frame, &snapshot.scroll_positions);
        Ok(HistoryEntry {
            view,
            navigation_type: snapshot.navigation_type,
        })
    }

    fn update_scroll_positions(
        &mut self,
        positions: &[FrameScrollPositionSnapshot],
    ) -> AppResult<()> {
        let current = self
            .history
            .get_mut(self.history_index)
            .ok_or_else(|| AppError::state("No page loaded"))?;
        apply_scroll_snapshot(&mut current.view.root_frame, positions);
        Ok(())
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
    fn record_navigation(
        &mut self,
        command: &str,
        url: Option<&str>,
        duration: Duration,
        result: &AppResult<PageViewModel>,
    ) {
        let duration_ms = duration.as_millis().min(u64::MAX as u128) as u64;
        self.total_navigations += 1;
        self.total_duration_ms += duration.as_millis();
        self.last_duration_ms = Some(duration_ms);
        let (success, error_code) = match result {
            Ok(_) => {
                self.successful_navigations += 1;
                (true, None)
            }
            Err(error) => {
                self.failed_navigations += 1;
                *self.error_counts.entry(error.code.clone()).or_insert(0) += 1;
                (false, Some(error.code.clone()))
            }
        };
        self.recent_events.push(NavigationEvent {
            command: command.to_string(),
            url: url.map(str::to_string),
            duration_ms,
            success,
            error_code,
            recorded_at_ms: unix_timestamp_ms(),
        });
        if self.recent_events.len() > 20 {
            self.recent_events.remove(0);
        }
    }

    fn snapshot(&self) -> AppMetricsSnapshot {
        let average_duration_ms = if self.total_navigations == 0 {
            0
        } else {
            (self.total_duration_ms / u128::from(self.total_navigations)) as u64
        };
        AppMetricsSnapshot {
            total_navigations: self.total_navigations,
            successful_navigations: self.successful_navigations,
            failed_navigations: self.failed_navigations,
            average_duration_ms,
            last_duration_ms: self.last_duration_ms,
            error_counts: self
                .error_counts
                .iter()
                .map(|(code, count)| ErrorMetric {
                    code: code.clone(),
                    count: *count,
                })
                .collect(),
            recent_events: self.recent_events.clone(),
        }
    }
}

fn load_frame_recursive(
    frame_id: &str,
    frame_name: Option<String>,
    url: &str,
    rect: FrameRect,
    overrides: Option<&BTreeMap<String, String>>,
) -> AppResult<FrameViewModel> {
    let loaded = fetch_document(url)?;
    let title = loaded
        .title
        .clone()
        .unwrap_or_else(|| loaded.final_url.clone());
    if let Some(frameset) = parse_frameset_document(&loaded.html) {
        return build_frameset_view(
            frame_id, frame_name, rect, loaded, title, &frameset, overrides,
        );
    }

    Ok(build_leaf_frame_view(
        frame_id,
        frame_name,
        &loaded.final_url,
        title,
        loaded.diagnostics,
        rect,
        &loaded.html,
    ))
}

fn build_frameset_view(
    frame_id: &str,
    frame_name: Option<String>,
    rect: FrameRect,
    loaded: LoadedDocument,
    title: String,
    frameset: &FramesetSpec,
    overrides: Option<&BTreeMap<String, String>>,
) -> AppResult<FrameViewModel> {
    build_inline_frameset_view(
        frame_id,
        frame_name,
        rect,
        loaded.final_url,
        title,
        loaded.diagnostics,
        frameset,
        overrides,
    )
}

// Spec: HTML Standard obsolete frames features, where `noframes` provides fallback content.
// https://html.spec.whatwg.org/multipage/obsolete.html
fn build_inline_frameset_view(
    frame_id: &str,
    frame_name: Option<String>,
    rect: FrameRect,
    current_url: String,
    title: String,
    diagnostics: Vec<String>,
    frameset: &FramesetSpec,
    overrides: Option<&BTreeMap<String, String>>,
) -> AppResult<FrameViewModel> {
    let child_rects = frameset.child_rects(&rect);
    let mut child_frames = Vec::new();

    for (index, child) in frameset.children.iter().enumerate() {
        let child_rect = child_rects.get(index).cloned().unwrap_or(FrameRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        });
        let child_id = build_frame_id(frame_id, child_frame_name(child), index);
        let child_view = match child {
            FramesetChild::Frame(spec) => {
                load_external_frame(&child_id, child_rect, &current_url, spec, overrides)?
            }
            FramesetChild::Frameset(nested) => build_inline_frameset_view(
                &child_id,
                None,
                child_rect,
                current_url.clone(),
                title.clone(),
                Vec::new(),
                nested,
                overrides,
            )?,
        };
        child_frames.push(child_view);
    }

    if child_frames.is_empty() {
        if let Some(noframes_html) = frameset.noframes_html.as_deref() {
            return Ok(build_leaf_frame_view(
                frame_id,
                frame_name,
                &current_url,
                title,
                diagnostics,
                rect,
                noframes_html,
            ));
        }
    }

    Ok(FrameViewModel {
        id: frame_id.to_string(),
        name: frame_name,
        current_url: current_url.clone(),
        title,
        diagnostics,
        rect: rect.clone(),
        content_size: ContentSize {
            width: rect.width,
            height: rect.height,
        },
        scroll_position: ScrollPosition::default(),
        render_backend: RenderBackendKind::NativeScene,
        document_url: current_url,
        scene_items: Vec::new(),
        render_tree: None,
        html_content: None,
        child_frames,
    })
}

fn load_external_frame(
    frame_id: &str,
    rect: FrameRect,
    base_url: &str,
    spec: &crate::loader::FrameSpec,
    overrides: Option<&BTreeMap<String, String>>,
) -> AppResult<FrameViewModel> {
    let default_url = resolve_url(base_url, &spec.src)?;
    let child_url = overrides
        .and_then(|map| map.get(frame_id).cloned())
        .unwrap_or(default_url);
    enforce_mixed_content_policy(base_url, &child_url)?;
    load_frame_recursive(frame_id, spec.name.clone(), &child_url, rect, overrides)
}

fn child_frame_name(child: &FramesetChild) -> Option<&str> {
    match child {
        FramesetChild::Frame(spec) => spec.name.as_deref(),
        FramesetChild::Frameset(_) => None,
    }
}

fn build_leaf_frame_view(
    frame_id: &str,
    frame_name: Option<String>,
    current_url: &str,
    title: String,
    diagnostics: Vec<String>,
    rect: FrameRect,
    html: &str,
) -> FrameViewModel {
    let (csp_html, csp_diagnostics) = apply_minimum_csp(html);
    let prepared_html = prepare_html_for_display(&csp_html, current_url, frame_id);
    let mut diagnostics = diagnostics;
    diagnostics.extend(csp_diagnostics);
    // Spec mapping: HTML LS parsing + DOM Standard tree updates happen in the
    // layout/JS runtime stage, and resulting computed boxes are painted in
    // CSS Display + CSS2 visual formatting model order as `scene_items`.
    let script_layout = build_layout_scene_with_script_runtime(html, &rect);
    diagnostics.extend(script_layout.diagnostics.clone());
    if script_layout.dom_updated {
        diagnostics.push(RelayoutTrigger::DomChanged.as_diagnostic().to_string());
    }
    FrameViewModel {
        id: frame_id.to_string(),
        name: frame_name,
        current_url: current_url.to_string(),
        title,
        diagnostics,
        rect: rect.clone(),
        content_size: script_layout.layout_scene.content_size,
        scroll_position: ScrollPosition::default(),
        render_backend: RenderBackendKind::NativeScene,
        document_url: current_url.to_string(),
        scene_items: script_layout.layout_scene.scene_items,
        render_tree: Some(script_layout.render_tree),
        html_content: Some(prepared_html),
        child_frames: Vec::new(),
    }
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
            rect: FrameRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            content_size: ContentSize { width, height },
            scroll_position: ScrollPosition::default(),
            render_backend: RenderBackendKind::NativeScene,
            document_url: String::new(),
            scene_items: Vec::new(),
            render_tree: None,
            html_content: Some(
                "<html><head><meta charset=\"utf-8\"></head><body></body></html>".to_string(),
            ),
            child_frames: Vec::new(),
        },
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

fn page_title_from_root(root: &FrameViewModel) -> String {
    if !root.title.trim().is_empty() {
        return root.title.clone();
    }
    root.child_frames
        .iter()
        .find_map(|child| {
            if child.title.trim().is_empty() {
                None
            } else {
                Some(child.title.clone())
            }
        })
        .unwrap_or_else(|| "CosmoBrowse".to_string())
}

fn frame_current_url(frame: &FrameViewModel, target_id: &str) -> Option<String> {
    if frame.id == target_id {
        return Some(frame.current_url.clone());
    }
    for child in &frame.child_frames {
        if let Some(url) = frame_current_url(child, target_id) {
            return Some(url);
        }
    }
    None
}

fn collect_diagnostics(frame: &FrameViewModel) -> Vec<String> {
    let mut diagnostics = Vec::new();
    let mut seen = HashSet::new();
    collect_diagnostics_recursive(frame, &mut diagnostics, &mut seen);
    diagnostics
}

fn collect_diagnostics_recursive(
    frame: &FrameViewModel,
    diagnostics: &mut Vec<String>,
    seen: &mut HashSet<String>,
) {
    for entry in &frame.diagnostics {
        if seen.insert(entry.clone()) {
            diagnostics.push(entry.clone());
        }
    }
    for child in &frame.child_frames {
        collect_diagnostics_recursive(child, diagnostics, seen);
    }
}

fn snapshot_frame_urls(frame: &FrameViewModel) -> BTreeMap<String, String> {
    let mut urls = BTreeMap::new();
    collect_frame_urls(frame, &mut urls);
    urls
}

fn collect_frame_urls(frame: &FrameViewModel, urls: &mut BTreeMap<String, String>) {
    if !frame.current_url.is_empty() {
        urls.insert(frame.id.clone(), frame.current_url.clone());
    }
    for child in &frame.child_frames {
        collect_frame_urls(child, urls);
    }
}

fn snapshot_frame_url_overrides(frame: &FrameViewModel) -> Vec<FrameUrlOverrideSnapshot> {
    snapshot_frame_urls(frame)
        .into_iter()
        .map(|(frame_id, current_url)| FrameUrlOverrideSnapshot {
            frame_id,
            current_url,
        })
        .collect()
}

fn frame_url_overrides_map(overrides: &[FrameUrlOverrideSnapshot]) -> BTreeMap<String, String> {
    overrides
        .iter()
        .map(|entry| (entry.frame_id.clone(), entry.current_url.clone()))
        .collect()
}

fn snapshot_scroll_positions(frame: &FrameViewModel) -> Vec<FrameScrollPositionSnapshot> {
    let mut positions = Vec::new();
    collect_scroll_positions(frame, &mut positions);
    positions
}

fn collect_scroll_positions(
    frame: &FrameViewModel,
    positions: &mut Vec<FrameScrollPositionSnapshot>,
) {
    positions.push(FrameScrollPositionSnapshot {
        frame_id: frame.id.clone(),
        position: frame.scroll_position,
    });
    for child in &frame.child_frames {
        collect_scroll_positions(child, positions);
    }
}

fn apply_scroll_snapshot(frame: &mut FrameViewModel, positions: &[FrameScrollPositionSnapshot]) {
    let scroll_positions = positions
        .iter()
        .map(|entry| (entry.frame_id.as_str(), entry.position))
        .collect::<BTreeMap<_, _>>();
    apply_scroll_snapshot_recursive(frame, &scroll_positions);
}

fn apply_scroll_snapshot_recursive(
    frame: &mut FrameViewModel,
    positions: &BTreeMap<&str, ScrollPosition>,
) {
    if let Some(position) = positions.get(frame.id.as_str()) {
        frame.scroll_position = *position;
    }
    for child in &mut frame.child_frames {
        apply_scroll_snapshot_recursive(child, positions);
    }
}

// Spec: HTML Living Standard, rules for choosing a navigable target from `target`.
// https://html.spec.whatwg.org/multipage/browsing-the-web.html#valid-browsing-context-name-or-keyword
fn resolve_destination_frame_id(
    root: &FrameViewModel,
    source_frame_id: &str,
    target: Option<&str>,
) -> Option<String> {
    let target = target.unwrap_or("_self").trim();
    match target {
        "" | "_self" => Some(source_frame_id.to_string()),
        "_parent" => {
            Some(parent_frame_id(source_frame_id).unwrap_or_else(|| source_frame_id.to_string()))
        }
        _ => find_frame_id_by_name(root, target),
    }
}

// Spec: HTML Living Standard browsing context keywords are matched using ASCII case-insensitive
// comparisons after attribute value processing.
// https://html.spec.whatwg.org/multipage/browsing-the-web.html#valid-browsing-context-name-or-keyword
fn normalize_target_keyword(target: Option<&str>) -> Option<String> {
    let target = target?.trim();
    if target.is_empty() {
        return None;
    }

    if target.starts_with('_') {
        return Some(target.to_ascii_lowercase());
    }

    Some(target.to_string())
}

fn parent_frame_id(frame_id: &str) -> Option<String> {
    let (parent, _) = frame_id.rsplit_once('/')?;
    Some(parent.to_string())
}

fn find_frame_id_by_name(frame: &FrameViewModel, target: &str) -> Option<String> {
    if frame.name.as_deref() == Some(target) {
        return Some(frame.id.clone());
    }
    for child in &frame.child_frames {
        if let Some(id) = find_frame_id_by_name(child, target) {
            return Some(id);
        }
    }
    None
}

// Spec: RFC 3986 scheme handling for absolute URIs.
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
        "mailto" | "javascript" | "data" | "file" => Err(AppError::navigation_guard(format!(
            "Navigation blocked for dangerous scheme: {}",
            parsed.scheme()
        ))),
        other => Err(AppError::validation(format!("Unsupported scheme: {other}"))),
    }
}

fn normalize_url_like(input: &str) -> AppResult<String> {
    normalize_url(input)
}

fn is_hash_navigation(previous_url: &str, current_url: &str) -> bool {
    if previous_url == current_url {
        return false;
    }
    strip_fragment(previous_url) == strip_fragment(current_url)
}

fn strip_fragment(url: &str) -> String {
    if let Ok(mut parsed) = Url::parse(url) {
        parsed.set_fragment(None);
        return parsed.to_string();
    }
    url.split('#').next().unwrap_or(url).to_string()
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
        if let Ok(candidate) = normalize_url(normalized) {
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

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loader::{decode_html_bytes, load_fixture_document, parse_frameset_document};
    use encoding_rs::SHIFT_JIS;

    #[test]
    fn normalize_https_url_keeps_scheme() {
        assert_eq!(
            normalize_url("https://abehiroshi.la.coocan.jp/").expect("valid url"),
            "https://abehiroshi.la.coocan.jp/"
        );
    }

    #[test]
    fn normalize_url_like_blocks_mailto_scheme() {
        let error = normalize_url_like("mailto:test@example.com").expect_err("mailto blocked");
        assert_eq!(error.code, "navigation_guard_blocked");
        assert!(error
            .message
            .contains("Navigation blocked for dangerous scheme: mailto"));
    }

    #[test]
    fn normalize_url_blocks_javascript_scheme() {
        let error = normalize_url("javascript:alert(1)").expect_err("javascript blocked");
        assert_eq!(error.code, "navigation_guard_blocked");
        assert!(error
            .message
            .contains("Navigation blocked for dangerous scheme: javascript"));
    }

    #[test]
    fn decode_shift_jis_from_meta() {
        let (encoded, _, _) =
            SHIFT_JIS.encode("<meta charset=Shift_JIS><title>\u{963F}\u{90E8}\u{5BDB}</title>");
        let decoded = decode_html_bytes(encoded.as_ref(), Some("text/html"));
        assert!(decoded.html.contains("\u{963F}\u{90E8}\u{5BDB}"));
    }

    #[test]
    fn parse_fixture_frameset() {
        let fixture =
            load_fixture_document("fixture://abehiroshi/index").expect("fixture should load");
        let frameset = parse_frameset_document(&fixture.html).expect("frameset should parse");
        assert_eq!(frameset.children.len(), 2);
    }

    #[test]
    fn open_fixture_root_loads_named_frames() {
        let mut session = BrowserSession::new();
        let view = session
            .open_url("fixture://abehiroshi/index")
            .expect("fixture should load");
        assert_eq!(view.root_frame.child_frames.len(), 2);
        assert_eq!(
            view.root_frame.child_frames[0].name.as_deref(),
            Some("left")
        );
        assert_eq!(
            view.root_frame.child_frames[1].name.as_deref(),
            Some("right")
        );
    }

    #[test]
    fn activating_named_frame_replaces_only_target_frame() {
        let mut session = BrowserSession::new();
        let view = session
            .open_url("fixture://abehiroshi/index")
            .expect("fixture should load");
        let left = view.root_frame.child_frames[0].id.clone();
        let right_before = view.root_frame.child_frames[1].current_url.clone();
        let next = session
            .activate_link(&left, "fixture://abehiroshi/prof", Some("right"))
            .expect("targeted navigation should work");
        assert_eq!(
            next.root_frame.child_frames[0].current_url,
            view.root_frame.child_frames[0].current_url
        );
        assert_ne!(next.root_frame.child_frames[1].current_url, right_before);
    }

    #[test]
    fn open_nested_frameset_fixture_builds_recursive_frame_tree() {
        let mut session = BrowserSession::new();
        let view = session
            .open_url("fixture://legacy_frames/nested")
            .expect("fixture should load");

        assert_eq!(view.root_frame.child_frames.len(), 2);
        assert_eq!(
            view.root_frame.child_frames[0].name.as_deref(),
            Some("left")
        );
        assert_eq!(view.root_frame.child_frames[1].child_frames.len(), 2);
        assert_eq!(
            view.root_frame.child_frames[1].child_frames[0]
                .name
                .as_deref(),
            Some("upper")
        );
        assert_eq!(
            view.root_frame.child_frames[1].child_frames[1]
                .name
                .as_deref(),
            Some("lower")
        );
    }

    #[test]
    fn open_noframes_fixture_renders_fallback_leaf() {
        let mut session = BrowserSession::new();
        let view = session
            .open_url("fixture://legacy_frames/noframes")
            .expect("fixture should load");

        assert!(view.root_frame.child_frames.is_empty());
        let html = view
            .root_frame
            .html_content
            .as_deref()
            .expect("fallback HTML should render as a leaf document");
        assert!(html.contains("Fallback only content"));
        assert!(html.contains("<base href=\"fixture://legacy_frames/noframes\">"));
    }

    #[test]
    fn collect_diagnostics_deduplicates_repeated_messages() {
        let mut root = sample_nested_root_frame();
        root.diagnostics = vec!["Decoded HTML as Shift_JIS".to_string()];
        root.child_frames[0].diagnostics = vec!["Decoded HTML as Shift_JIS".to_string()];
        root.child_frames[1].diagnostics = vec![
            "Decoded HTML as Shift_JIS".to_string(),
            "Loaded fixture document".to_string(),
        ];

        assert_eq!(
            collect_diagnostics(&root),
            vec![
                "Decoded HTML as Shift_JIS".to_string(),
                "Loaded fixture document".to_string(),
            ]
        );
    }

    #[test]
    fn resolve_parent_target_uses_parent_frame() {
        let root = sample_nested_root_frame();
        assert_eq!(
            resolve_destination_frame_id(&root, "root/right/inner", Some("_parent")),
            Some("root/right".to_string())
        );
    }

    #[test]
    fn resolve_parent_target_uses_ascii_case_insensitive_keyword() {
        let root = sample_nested_root_frame();
        let normalized_target = normalize_target_keyword(Some("  _PARENT  "));
        assert_eq!(
            resolve_destination_frame_id(&root, "root/right/inner", normalized_target.as_deref()),
            Some("root/right".to_string())
        );
    }

    #[test]
    fn activate_link_treats_top_keyword_as_ascii_case_insensitive() {
        let mut session = BrowserSession::new();
        let _ = session
            .open_url("fixture://abehiroshi/index")
            .expect("fixture should load");

        let next = session
            .activate_link("root/left", "fixture://abehiroshi/prof", Some("  _TOP "))
            .expect("top navigation should reload the root document");

        assert_eq!(next.current_url, "fixture://abehiroshi/prof");
        assert!(next.root_frame.child_frames.is_empty());
        assert!(next.root_frame.current_url.ends_with("/prof"));
    }

    #[test]
    fn navigation_state_for_normal_document_transition() {
        let mut session = BrowserSession::new();
        session.push_history(
            sample_page("https://example.com/a", vec![]),
            NavigationType::Document,
        );
        session.push_history(
            sample_page("https://example.com/b", vec![]),
            NavigationType::Document,
        );

        let state = session.navigation_state();
        assert!(state.can_back);
        assert!(!state.can_forward);
        assert_eq!(
            state.current_navigation_type,
            Some(NavigationType::Document)
        );

        let back = session
            .back()
            .expect("back should move to previous document");
        assert_eq!(back.current_url, "https://example.com/a");
    }

    #[test]
    fn hash_navigation_is_skipped_by_back_forward_state() {
        let mut session = BrowserSession::new();
        session.push_history(
            sample_page("https://example.com/doc", vec![]),
            NavigationType::Document,
        );
        session.push_history(
            sample_page("https://example.com/doc#section-1", vec![]),
            NavigationType::Hash,
        );
        session.push_history(
            sample_page("https://example.com/doc#section-2", vec![]),
            NavigationType::Hash,
        );

        let state = session.navigation_state();
        assert!(state.can_back);
        assert!(!state.can_forward);
        assert_eq!(state.current_navigation_type, Some(NavigationType::Hash));

        let back = session.back().expect("back should jump over hash entries");
        assert_eq!(back.current_url, "https://example.com/doc");
    }

    #[test]
    fn redirect_navigation_keeps_single_step_in_back_history() {
        let mut session = BrowserSession::new();
        session.push_history(
            sample_page("https://example.com/start", vec![]),
            NavigationType::Document,
        );
        session.push_history(
            sample_page(
                "https://example.com/final",
                vec!["redirect followed: https://example.com/start -> https://example.com/final"],
            ),
            NavigationType::Redirect,
        );
        session.push_history(
            sample_page("https://example.com/next", vec![]),
            NavigationType::Document,
        );

        let back = session
            .back()
            .expect("first back should land on redirect target document");
        assert_eq!(back.current_url, "https://example.com/final");
        assert_eq!(
            session.navigation_state().current_navigation_type,
            Some(NavigationType::Redirect)
        );

        let back_again = session
            .back()
            .expect("second back should land on origin document");
        assert_eq!(back_again.current_url, "https://example.com/start");
    }

    #[test]
    fn classify_navigation_marks_hash_and_redirect() {
        let session = BrowserSession::new();
        let previous = sample_page("https://example.com/doc", vec![]);
        let hash = sample_page("https://example.com/doc#x", vec![]);
        let redirect = sample_page(
            "https://example.com/final",
            vec!["redirect followed: https://example.com/doc -> https://example.com/final"],
        );

        assert_eq!(
            session.classify_navigation(Some(&previous), &hash),
            NavigationType::Hash
        );
        assert_eq!(
            session.classify_navigation(Some(&previous), &redirect),
            NavigationType::Redirect
        );
    }

    #[test]
    fn session_snapshot_round_trips_history_and_scroll_positions() {
        let mut app = SabaApp::default();
        let first = app
            .open_url("fixture://abehiroshi/index")
            .expect("fixture should load");
        let left_frame = first.root_frame.child_frames[0].id.clone();
        let _ = app
            .activate_link(&left_frame, "fixture://abehiroshi/prof", Some("right"))
            .expect("targeted navigation should work");
        app.update_scroll_positions(vec![
            FrameScrollPositionSnapshot {
                frame_id: "root".to_string(),
                position: ScrollPosition { x: 0, y: 120 },
            },
            FrameScrollPositionSnapshot {
                frame_id: "root/right".to_string(),
                position: ScrollPosition { x: 8, y: 64 },
            },
        ])
        .expect("scroll positions should update");

        let snapshot = app.export_session_snapshot();

        let mut restored = SabaApp::default();
        restored
            .import_session_snapshot(snapshot)
            .expect("snapshot should restore");

        let restored_view = restored.get_page_view();
        assert_eq!(restored.active_tab_id, 1);
        assert_eq!(
            restored_view.root_frame.scroll_position,
            ScrollPosition { x: 0, y: 120 }
        );
        assert_eq!(
            restored_view.root_frame.child_frames[1].scroll_position,
            ScrollPosition { x: 8, y: 64 }
        );
        assert_eq!(
            restored_view.root_frame.child_frames[1].current_url,
            "fixture://abehiroshi/prof"
        );
        assert_eq!(
            restored.get_navigation_state().current_url.as_deref(),
            Some("fixture://abehiroshi/index")
        );
    }

    fn sample_page(url: &str, diagnostics: Vec<&str>) -> PageViewModel {
        PageViewModel {
            current_url: url.to_string(),
            title: url.to_string(),
            diagnostics: diagnostics.into_iter().map(str::to_string).collect(),
            content_size: ContentSize {
                width: 960,
                height: 720,
            },
            scene_items: Vec::new(),
            root_frame: sample_leaf_frame("root", None, url),
        }
    }

    fn sample_nested_root_frame() -> FrameViewModel {
        FrameViewModel {
            id: "root".to_string(),
            name: None,
            current_url: "fixture://abehiroshi/index".to_string(),
            title: "Root".to_string(),
            diagnostics: Vec::new(),
            rect: FrameRect {
                x: 0,
                y: 0,
                width: 960,
                height: 720,
            },
            content_size: ContentSize {
                width: 960,
                height: 720,
            },
            scroll_position: ScrollPosition::default(),
            render_backend: RenderBackendKind::NativeScene,
            document_url: "fixture://abehiroshi/index".to_string(),
            scene_items: Vec::new(),
            render_tree: None,
            html_content: None,
            child_frames: vec![
                sample_leaf_frame("root/left", Some("left"), "fixture://abehiroshi/menu"),
                FrameViewModel {
                    id: "root/right".to_string(),
                    name: Some("right".to_string()),
                    current_url: "fixture://abehiroshi/top".to_string(),
                    title: "Right".to_string(),
                    diagnostics: Vec::new(),
                    rect: FrameRect {
                        x: 240,
                        y: 0,
                        width: 720,
                        height: 720,
                    },
                    content_size: ContentSize {
                        width: 720,
                        height: 720,
                    },
                    scroll_position: ScrollPosition::default(),
                    render_backend: RenderBackendKind::NativeScene,
                    document_url: "fixture://abehiroshi/top".to_string(),
                    scene_items: Vec::new(),
                    render_tree: None,
                    html_content: None,
                    child_frames: vec![sample_leaf_frame(
                        "root/right/inner",
                        Some("inner"),
                        "fixture://abehiroshi/prof",
                    )],
                },
            ],
        }
    }

    fn sample_leaf_frame(id: &str, name: Option<&str>, current_url: &str) -> FrameViewModel {
        FrameViewModel {
            id: id.to_string(),
            name: name.map(str::to_string),
            current_url: current_url.to_string(),
            title: current_url.to_string(),
            diagnostics: Vec::new(),
            rect: FrameRect {
                x: 0,
                y: 0,
                width: 320,
                height: 240,
            },
            content_size: ContentSize {
                width: 320,
                height: 240,
            },
            scroll_position: ScrollPosition::default(),
            render_backend: RenderBackendKind::NativeScene,
            document_url: current_url.to_string(),
            scene_items: Vec::new(),
            render_tree: None,
            html_content: Some("<html></html>".to_string()),
            child_frames: Vec::new(),
        }
    }
}
