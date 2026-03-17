use cosmo_runtime::{
    AppError, AppMetricsSnapshot, AppService, NavigationState, OrbitSnapshot, SceneItem,
    SearchResult, StarshipApp, TabSummary,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const IPC_SCHEMA_VERSION: u32 = 1;

pub struct NativeAdapter {
    app: Mutex<StarshipApp>,
    renderer: Mutex<RendererProcessManager>,
}

pub trait ProcessHost: Send {
    fn spawn(&mut self) -> Result<u32, AppError>;
    fn kill(&mut self) -> Result<(), AppError>;
    fn healthcheck(&mut self) -> Result<(), AppError>;
    fn restart(&mut self) -> Result<u32, AppError>;
}

struct RendererProcessManager {
    host: Box<dyn ProcessHost>,
    healthcheck_interval: Duration,
    last_healthcheck: Option<Instant>,
}

impl RendererProcessManager {
    fn new(mut host: Box<dyn ProcessHost>, healthcheck_interval: Duration) -> Self {
        if let Err(error) = host.spawn() {
            log_recovery_event("renderer_spawn_failed", None, &error.message);
        }

        Self {
            host,
            healthcheck_interval,
            last_healthcheck: None,
        }
    }

    fn ensure_healthy(&mut self) -> Result<(), AppError> {
        let now = Instant::now();
        if self
            .last_healthcheck
            .is_some_and(|last| now.duration_since(last) < self.healthcheck_interval)
        {
            return Ok(());
        }
        self.last_healthcheck = Some(now);

        if self.host.healthcheck().is_ok() {
            return Ok(());
        }

        match self.host.restart() {
            Ok(pid) => {
                log_recovery_event(
                    "renderer_recovered",
                    Some(pid),
                    "renderer restarted after failed healthcheck",
                );
                Err(AppError::recovering(
                    "renderer process was restarted; retry the request",
                ))
            }
            Err(error) => {
                log_recovery_event("renderer_recovery_failed", None, &error.message);
                Err(AppError::recovering(format!(
                    "renderer recovery in progress: {}",
                    error.message
                )))
            }
        }
    }
}

struct StdProcessHost {
    child: Option<Child>,
    command: String,
    args: Vec<String>,
}

impl StdProcessHost {
    fn from_env() -> Self {
        let command = std::env::var("COSMO_RENDERER_PATH").unwrap_or_else(|_| "sleep".to_string());
        let args = std::env::var("COSMO_RENDERER_ARGS")
            .unwrap_or_else(|_| "3600".to_string())
            .split_whitespace()
            .map(str::to_string)
            .collect();
        Self {
            child: None,
            command,
            args,
        }
    }
}

impl ProcessHost for StdProcessHost {
    fn spawn(&mut self) -> Result<u32, AppError> {
        let child = Command::new(&self.command)
            .args(&self.args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|error| {
                AppError::state(format!("Failed to spawn renderer process: {error}"))
            })?;
        let pid = child.id();
        self.child = Some(child);
        Ok(pid)
    }

    fn kill(&mut self) -> Result<(), AppError> {
        if let Some(mut child) = self.child.take() {
            child.kill().map_err(|error| {
                AppError::state(format!("Failed to kill renderer process: {error}"))
            })?;
            let _ = child.wait();
        }
        Ok(())
    }

    fn healthcheck(&mut self) -> Result<(), AppError> {
        let Some(child) = self.child.as_mut() else {
            return Err(AppError::state("Renderer process is not running"));
        };

        match child
            .try_wait()
            .map_err(|error| AppError::state(format!("Renderer healthcheck failed: {error}")))?
        {
            Some(status) => Err(AppError::state(format!(
                "Renderer process exited unexpectedly: {status}"
            ))),
            None => Ok(()),
        }
    }

    fn restart(&mut self) -> Result<u32, AppError> {
        let _ = self.kill();
        self.spawn()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BrowserPageDto {
    pub current_url: String,
    pub title: String,
    pub diagnostics: Vec<String>,
    pub content_size: ContentSizeDto,
    pub network_log: Vec<String>,
    pub console_log: Vec<String>,
    pub dom_snapshot: Vec<DomSnapshotEntryDto>,
    pub root_frame: BrowserFrameDto,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct DomSnapshotEntryDto {
    pub frame_id: String,
    pub document_url: String,
    pub html: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct BrowserFrameDto {
    pub id: String,
    pub name: Option<String>,
    pub current_url: String,
    pub title: String,
    pub diagnostics: Vec<String>,
    pub rect: FrameRectDto,
    pub render_backend: String,
    pub document_url: String,
    pub scene_items: Vec<SceneItem>,
    pub render_tree: Option<RenderTreeSnapshotDto>,
    pub html_content: Option<String>,
    pub child_frames: Vec<BrowserFrameDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ContentSizeDto {
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct FrameRectDto {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RenderTreeSnapshotDto {
    pub root: Option<RenderNodeDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RenderNodeDto {
    pub kind: RenderNodeKindDto,
    pub node_name: String,
    pub text: Option<String>,
    pub box_info: RenderBoxDto,
    pub style: ResolvedStyleDto,
    pub children: Vec<RenderNodeDto>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderNodeKindDto {
    Block,
    Inline,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct RenderBoxDto {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ResolvedStyleDto {
    pub display: String,
    pub position: String,
    pub color: String,
    pub background_color: String,
    pub font_px: i64,
    pub font_family: String,
    pub opacity: f64,
    pub z_index: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct CrashReportDto {
    pub path: String,
    pub crashed_at_ms: u64,
    pub reason: String,
    pub reproduction: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IpcRequestPayload {
    OpenUrl {
        url: String,
    },
    GetPageView,
    SetViewport {
        width: i64,
        height: i64,
    },
    Reload,
    // Spec: navigation traversal should follow HTML Standard history traversal
    // semantics when integrating with joint session history.
    // https://html.spec.whatwg.org/multipage/nav-history-apis.html#history-traversal
    Back,
    // Spec: forward traversal is paired with history entry activation rules from
    // HTML Standard navigation/history sections.
    // https://html.spec.whatwg.org/multipage/history.html
    Forward,
    // Spec: target resolution and navigation timing must stay aligned with HTML
    // navigation algorithms for nested browsing contexts.
    // https://html.spec.whatwg.org/multipage/document-sequences.html#navigate
    ActivateLink {
        frame_id: String,
        href: String,
        target: Option<String>,
    },
    GetNavigationState,
    GetMetrics,
    GetLatestCrashReport,
    NewTab,
    SwitchTab {
        id: u32,
    },
    CloseTab {
        id: u32,
    },
    ListTabs,
    Search {
        query: String,
    },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IpcResponsePayload {
    Page(BrowserPageDto),
    NavigationState(NavigationState),
    Metrics(AppMetricsSnapshot),
    CrashReport(Option<CrashReportDto>),
    Tab(TabSummary),
    Tabs(Vec<TabSummary>),
    SearchResults(Vec<SearchResult>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcRequest {
    #[serde(default = "ipc_schema_version")]
    pub version: u32,
    #[serde(flatten)]
    pub payload: IpcRequestPayload,
}

#[derive(Debug, Clone, Serialize)]
pub struct IpcResponse {
    pub version: u32,
    #[serde(flatten)]
    pub payload: IpcResponsePayload,
}

const fn ipc_schema_version() -> u32 {
    IPC_SCHEMA_VERSION
}

impl NativeAdapter {
    pub fn with_process_host(
        process_host: Box<dyn ProcessHost>,
        healthcheck_interval: Duration,
    ) -> Self {
        Self {
            app: Mutex::new(StarshipApp::default()),
            renderer: Mutex::new(RendererProcessManager::new(
                process_host,
                healthcheck_interval,
            )),
        }
    }

    pub fn dispatch(&self, request: IpcRequest) -> Result<IpcResponse, AppError> {
        self.ensure_renderer_healthy()?;

        if request.version != IPC_SCHEMA_VERSION {
            return Err(AppError::validation(format!(
                "Unsupported IPC version: {} (expected {})",
                request.version, IPC_SCHEMA_VERSION
            )));
        }

        // Spec: IPC requests are processed serially so browser state updates are
        // observed in deterministic task order, mirroring DOM event loop task
        // execution constraints where tasks run to completion.
        // https://html.spec.whatwg.org/multipage/webappapis.html#event-loop-processing-model
        let payload = match request.payload {
            IpcRequestPayload::OpenUrl { url } => self.open_url(&url).map(IpcResponsePayload::Page),
            IpcRequestPayload::GetPageView => self.get_page_view().map(IpcResponsePayload::Page),
            IpcRequestPayload::SetViewport { width, height } => self
                .set_viewport(width, height)
                .map(IpcResponsePayload::Page),
            IpcRequestPayload::Reload => self.reload().map(IpcResponsePayload::Page),
            IpcRequestPayload::Back => self.back().map(IpcResponsePayload::Page),
            IpcRequestPayload::Forward => self.forward().map(IpcResponsePayload::Page),
            IpcRequestPayload::ActivateLink {
                frame_id,
                href,
                target,
            } => self
                .activate_link(&frame_id, &href, target.as_deref())
                .map(IpcResponsePayload::Page),
            IpcRequestPayload::GetNavigationState => self
                .get_navigation_state()
                .map(IpcResponsePayload::NavigationState),
            IpcRequestPayload::GetMetrics => self.get_metrics().map(IpcResponsePayload::Metrics),
            IpcRequestPayload::GetLatestCrashReport => Ok(IpcResponsePayload::CrashReport(
                self.get_latest_crash_report(),
            )),
            IpcRequestPayload::NewTab => self.new_tab().map(IpcResponsePayload::Tab),
            IpcRequestPayload::SwitchTab { id } => {
                self.switch_tab(id).map(IpcResponsePayload::Page)
            }
            IpcRequestPayload::CloseTab { id } => self.close_tab(id).map(IpcResponsePayload::Tabs),
            IpcRequestPayload::ListTabs => self.list_tabs().map(IpcResponsePayload::Tabs),
            IpcRequestPayload::Search { query } => {
                self.search(&query).map(IpcResponsePayload::SearchResults)
            }
        }?;

        Ok(IpcResponse {
            version: IPC_SCHEMA_VERSION,
            payload,
        })
    }

    pub fn open_url(&self, url: &str) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        app.open_url(url).map(BrowserPageDto::from)
    }

    pub fn activate_link(
        &self,
        frame_id: &str,
        href: &str,
        target: Option<&str>,
    ) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        app.activate_link(frame_id, href, target)
            .map(BrowserPageDto::from)
    }

    pub fn get_page_view(&self) -> Result<BrowserPageDto, AppError> {
        let app = self.lock_app()?;
        Ok(BrowserPageDto::from(app.get_page_view()))
    }

    pub fn set_viewport(&self, width: i64, height: i64) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        app.set_viewport(width, height).map(BrowserPageDto::from)
    }

    pub fn reload(&self) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        app.reload().map(BrowserPageDto::from)
    }

    pub fn back(&self) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        app.back().map(BrowserPageDto::from)
    }

    pub fn forward(&self) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        app.forward().map(BrowserPageDto::from)
    }

    pub fn get_navigation_state(&self) -> Result<NavigationState, AppError> {
        let app = self.lock_app()?;
        Ok(app.get_navigation_state())
    }

    pub fn get_metrics(&self) -> Result<AppMetricsSnapshot, AppError> {
        let app = self.lock_app()?;
        Ok(app.get_metrics())
    }

    pub fn get_latest_crash_report(&self) -> Option<CrashReportDto> {
        let content = fs::read_to_string(crash_report_path()).ok()?;
        serde_json::from_str::<CrashReportDto>(&content).ok()
    }

    pub fn new_tab(&self) -> Result<TabSummary, AppError> {
        let mut app = self.lock_app()?;
        Ok(app.new_tab())
    }

    pub fn switch_tab(&self, id: u32) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        app.switch_tab(id).map(BrowserPageDto::from)
    }

    pub fn close_tab(&self, id: u32) -> Result<Vec<TabSummary>, AppError> {
        let mut app = self.lock_app()?;
        app.close_tab(id)
    }

    pub fn list_tabs(&self) -> Result<Vec<TabSummary>, AppError> {
        let app = self.lock_app()?;
        Ok(app.list_tabs())
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError> {
        let app = self.lock_app()?;
        app.search(query)
    }

    fn lock_app(&self) -> Result<std::sync::MutexGuard<'_, StarshipApp>, AppError> {
        self.app
            .lock()
            .map_err(|_| AppError::state("Failed to lock app state"))
    }

    fn ensure_renderer_healthy(&self) -> Result<(), AppError> {
        let mut renderer = self
            .renderer
            .lock()
            .map_err(|_| AppError::state("Failed to lock renderer process state"))?;
        renderer.ensure_healthy()
    }
}

impl Default for NativeAdapter {
    fn default() -> Self {
        let interval_ms = std::env::var("COSMO_RENDERER_HEALTHCHECK_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(1000);
        Self::with_process_host(
            Box::new(StdProcessHost::from_env()),
            Duration::from_millis(interval_ms),
        )
    }
}

impl Drop for NativeAdapter {
    fn drop(&mut self) {
        if let Ok(mut renderer) = self.renderer.lock() {
            let _ = renderer.host.kill();
        }
    }
}

impl From<OrbitSnapshot> for BrowserPageDto {
    fn from(page: OrbitSnapshot) -> Self {
        let mut dom_snapshot = Vec::new();
        collect_dom_snapshots(&page.root_frame, &mut dom_snapshot);
        let network_log = page
            .diagnostics
            .iter()
            .filter(|entry| is_network_log_entry(entry))
            .cloned()
            .collect();
        let console_log = page
            .diagnostics
            .iter()
            .filter(|entry| is_console_log_entry(entry))
            .cloned()
            .collect();

        Self {
            current_url: page.current_url,
            title: page.title,
            diagnostics: page.diagnostics,
            content_size: ContentSizeDto {
                width: page.content_size.width,
                height: page.content_size.height,
            },
            network_log,
            console_log,
            dom_snapshot,
            root_frame: page.root_frame.into(),
        }
    }
}

impl From<cosmo_runtime::FrameViewModel> for BrowserFrameDto {
    fn from(frame: cosmo_runtime::FrameViewModel) -> Self {
        Self {
            id: frame.id,
            name: frame.name,
            current_url: frame.current_url,
            title: frame.title,
            diagnostics: frame.diagnostics,
            rect: FrameRectDto {
                x: frame.rect.x,
                y: frame.rect.y,
                width: frame.rect.width,
                height: frame.rect.height,
            },
            render_backend: {
                #[allow(deprecated)]
                match frame.render_backend {
                    cosmo_runtime::RenderBackendKind::WebView
                    | cosmo_runtime::RenderBackendKind::NativeScene => "native_scene".to_string(),
                }
            },
            document_url: frame.document_url,
            scene_items: frame.scene_items,
            render_tree: frame.render_tree.map(RenderTreeSnapshotDto::from),
            html_content: frame.html_content,
            child_frames: frame
                .child_frames
                .into_iter()
                .map(BrowserFrameDto::from)
                .collect(),
        }
    }
}

impl From<cosmo_runtime::RenderTreeSnapshot> for RenderTreeSnapshotDto {
    fn from(snapshot: cosmo_runtime::RenderTreeSnapshot) -> Self {
        Self {
            root: snapshot.root.map(RenderNodeDto::from),
        }
    }
}

impl From<cosmo_runtime::RenderNode> for RenderNodeDto {
    fn from(node: cosmo_runtime::RenderNode) -> Self {
        Self {
            kind: node.kind.into(),
            node_name: node.node_name,
            text: node.text,
            box_info: RenderBoxDto {
                x: node.box_info.x,
                y: node.box_info.y,
                width: node.box_info.width,
                height: node.box_info.height,
            },
            style: ResolvedStyleDto {
                display: node.style.display,
                position: node.style.position,
                color: node.style.color,
                background_color: node.style.background_color,
                font_px: node.style.font_px,
                font_family: node.style.font_family,
                opacity: node.style.opacity,
                z_index: node.style.z_index,
            },
            children: node.children.into_iter().map(RenderNodeDto::from).collect(),
        }
    }
}

impl From<cosmo_runtime::RenderNodeKind> for RenderNodeKindDto {
    fn from(kind: cosmo_runtime::RenderNodeKind) -> Self {
        match kind {
            cosmo_runtime::RenderNodeKind::Block => Self::Block,
            cosmo_runtime::RenderNodeKind::Inline => Self::Inline,
            cosmo_runtime::RenderNodeKind::Text => Self::Text,
        }
    }
}

fn collect_dom_snapshots(
    frame: &cosmo_runtime::FrameViewModel,
    out: &mut Vec<DomSnapshotEntryDto>,
) {
    if let Some(html) = frame.html_content.as_ref() {
        out.push(DomSnapshotEntryDto {
            frame_id: frame.id.clone(),
            document_url: frame.document_url.clone(),
            html: html.clone(),
        });
    }
    for child in &frame.child_frames {
        collect_dom_snapshots(child, out);
    }
}

fn is_network_log_entry(entry: &str) -> bool {
    let lower = entry.to_ascii_lowercase();
    lower.contains("http")
        || lower.contains("cors")
        || lower.contains("cookie")
        || lower.contains("charset")
        || lower.contains("tls")
}

fn is_console_log_entry(entry: &str) -> bool {
    let lower = entry.to_ascii_lowercase();
    lower.contains("script") || lower.contains("unsupported browser api") || lower.contains("dom")
}

fn crash_report_path() -> std::path::PathBuf {
    std::env::temp_dir().join("cosmobrowse-crash-report.json")
}

fn log_recovery_event(event: &str, pid: Option<u32>, detail: &str) {
    #[derive(Serialize)]
    struct RecoveryLog<'a> {
        event: &'a str,
        component: &'a str,
        pid: Option<u32>,
        detail: &'a str,
        timestamp_ms: u128,
    }

    let timestamp_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();

    let line = serde_json::to_string(&RecoveryLog {
        event,
        component: "renderer_process_manager",
        pid,
        detail,
        timestamp_ms,
    })
    .unwrap_or_else(|_| {
        format!(
            "{{\"event\":\"{}\",\"component\":\"renderer_process_manager\",\"detail\":\"{}\"}}",
            event, detail
        )
    });
    eprintln!("{line}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Default)]
    struct FakeProcessState {
        running: bool,
        spawn_count: u32,
        restart_count: u32,
    }

    struct FakeProcessHost {
        state: Arc<Mutex<FakeProcessState>>,
    }

    impl FakeProcessHost {
        fn new(state: Arc<Mutex<FakeProcessState>>) -> Self {
            Self { state }
        }
    }

    impl ProcessHost for FakeProcessHost {
        fn spawn(&mut self) -> Result<u32, AppError> {
            let mut state = self.state.lock().expect("state lock");
            state.running = true;
            state.spawn_count += 1;
            Ok(state.spawn_count)
        }

        fn kill(&mut self) -> Result<(), AppError> {
            let mut state = self.state.lock().expect("state lock");
            state.running = false;
            Ok(())
        }

        fn healthcheck(&mut self) -> Result<(), AppError> {
            let state = self.state.lock().expect("state lock");
            if state.running {
                Ok(())
            } else {
                Err(AppError::state("renderer has exited"))
            }
        }

        fn restart(&mut self) -> Result<u32, AppError> {
            let mut state = self.state.lock().expect("state lock");
            state.restart_count += 1;
            state.running = true;
            state.spawn_count += 1;
            Ok(state.spawn_count)
        }
    }

    fn page_view_request() -> IpcRequest {
        IpcRequest {
            version: IPC_SCHEMA_VERSION,
            payload: IpcRequestPayload::GetPageView,
        }
    }

    #[test]
    fn forced_renderer_exit_triggers_restart_and_allows_retry_success() {
        let state = Arc::new(Mutex::new(FakeProcessState::default()));
        let adapter = NativeAdapter::with_process_host(
            Box::new(FakeProcessHost::new(state.clone())),
            Duration::ZERO,
        );

        {
            let state = state.lock().expect("state lock");
            assert_eq!(state.spawn_count, 1);
            assert!(state.running);
        }

        {
            let mut state = state.lock().expect("state lock");
            state.running = false;
        }

        let error = adapter
            .dispatch(page_view_request())
            .expect_err("first request should surface recovering error");
        assert_eq!(error.code, "renderer_recovering");
        assert!(error.retryable);

        {
            let state = state.lock().expect("state lock");
            assert_eq!(state.restart_count, 1);
            assert_eq!(state.spawn_count, 2);
            assert!(state.running);
        }

        let retry = adapter.dispatch(page_view_request());
        assert!(
            retry.is_ok(),
            "retry should succeed after renderer recovery"
        );
    }
}
