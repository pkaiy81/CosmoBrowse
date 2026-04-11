use cosmo_runtime::{
    scene_items_to_paint_commands, AppError, AppMetricsSnapshot, AppService, DownloadEntry,
    DownloadPolicySettings, DownloadSavePolicy, FrameScrollPositionSnapshot, NavigationState,
    OmniboxSuggestionSet, OrbitSnapshot, PaintCommand, SceneItem, SearchResult, SessionSnapshot,
    StarshipApp, TabSummary,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
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

#[derive(Debug, Clone, Serialize)]
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

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DomSnapshotEntryDto {
    pub frame_id: String,
    pub document_url: String,
    pub html: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct BrowserFrameDto {
    pub id: String,
    pub name: Option<String>,
    pub current_url: String,
    pub title: String,
    pub diagnostics: Vec<String>,
    pub rect: FrameRectDto,
    pub scroll_position: ScrollPositionDto,
    pub render_backend: String,
    pub document_url: String,
    pub scene_items: Vec<SceneItem>,
    pub paint_commands: Vec<PaintCommand>,
    pub render_tree: Option<RenderTreeSnapshotDto>,
    pub html_content: Option<String>,
    pub child_frames: Vec<BrowserFrameDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ContentSizeDto {
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct FrameRectDto {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ScrollPositionDto {
    pub x: i64,
    pub y: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RenderTreeSnapshotDto {
    pub root: Option<RenderNodeDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RenderNodeDto {
    pub kind: RenderNodeKindDto,
    pub node_name: String,
    pub text: Option<String>,
    pub box_info: RenderBoxDto,
    pub style: ResolvedStyleDto,
    pub children: Vec<RenderNodeDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderNodeKindDto {
    Block,
    Inline,
    Text,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RenderBoxDto {
    pub x: i64,
    pub y: i64,
    pub width: i64,
    pub height: i64,
}

#[derive(Debug, Clone, Serialize)]
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
    #[serde(default)]
    pub build_id: String,
    #[serde(default)]
    pub commit_hash: String,
    #[serde(default)]
    pub transport: String,
    #[serde(default)]
    pub active_url: String,
    #[serde(default)]
    pub last_command: String,
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
    DuplicateTab {
        id: u32,
    },
    SwitchTab {
        id: u32,
    },
    CloseTab {
        id: u32,
    },
    MoveTab {
        id: u32,
        target_index: usize,
    },
    SetTabPinned {
        id: u32,
        pinned: bool,
    },
    SetTabMuted {
        id: u32,
        muted: bool,
    },
    ListTabs,
    Search {
        query: String,
    },
    OmniboxSuggestions {
        query: String,
        current_index: Option<usize>,
    },
    UpdateScrollPositions {
        positions: Vec<FrameScrollPositionSnapshot>,
    },
    RegisterTlsException {
        url: String,
    },
    EnqueueDownload {
        url: String,
    },
    ListDownloads,
    GetDownloadProgress {
        id: u64,
    },
    PauseDownload {
        id: u64,
    },
    ResumeDownload {
        id: u64,
    },
    CancelDownload {
        id: u64,
    },
    OpenDownload {
        id: u64,
    },
    RevealDownload {
        id: u64,
    },
    GetDownloadPolicySettings,
    SetDownloadDefaultPolicy {
        policy: DownloadSavePolicy,
    },
    SetDownloadSitePolicy {
        origin: String,
        policy: DownloadSavePolicy,
    },
    ClearDownloadSitePolicy {
        origin: String,
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
    OmniboxSuggestions(OmniboxSuggestionSet),
    Download(DownloadEntry),
    Downloads(Vec<DownloadEntry>),
    DownloadPolicySettings(DownloadPolicySettings),
    Ack(bool),
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
            app: Mutex::new(load_startup_app()),
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
            IpcRequestPayload::DuplicateTab { id } => {
                self.duplicate_tab(id).map(IpcResponsePayload::Tab)
            }
            IpcRequestPayload::SwitchTab { id } => {
                self.switch_tab(id).map(IpcResponsePayload::Page)
            }
            IpcRequestPayload::CloseTab { id } => self.close_tab(id).map(IpcResponsePayload::Tabs),
            IpcRequestPayload::MoveTab { id, target_index } => self
                .move_tab(id, target_index)
                .map(IpcResponsePayload::Tabs),
            IpcRequestPayload::SetTabPinned { id, pinned } => self
                .set_tab_pinned(id, pinned)
                .map(IpcResponsePayload::Tabs),
            IpcRequestPayload::SetTabMuted { id, muted } => {
                self.set_tab_muted(id, muted).map(IpcResponsePayload::Tabs)
            }
            IpcRequestPayload::ListTabs => self.list_tabs().map(IpcResponsePayload::Tabs),
            IpcRequestPayload::Search { query } => {
                self.search(&query).map(IpcResponsePayload::SearchResults)
            }
            IpcRequestPayload::OmniboxSuggestions {
                query,
                current_index,
            } => self
                .omnibox_suggestions(&query, current_index)
                .map(IpcResponsePayload::OmniboxSuggestions),
            IpcRequestPayload::UpdateScrollPositions { positions } => self
                .update_scroll_positions(positions)
                .map(|_| IpcResponsePayload::Ack(true)),
            IpcRequestPayload::RegisterTlsException { url } => self
                .register_tls_exception(&url)
                .map(|_| IpcResponsePayload::Ack(true)),
            IpcRequestPayload::EnqueueDownload { url } => self
                .enqueue_download(&url)
                .map(IpcResponsePayload::Download),
            IpcRequestPayload::ListDownloads => {
                self.list_downloads().map(IpcResponsePayload::Downloads)
            }
            IpcRequestPayload::GetDownloadProgress { id } => self
                .get_download_progress(id)
                .map(IpcResponsePayload::Download),
            IpcRequestPayload::PauseDownload { id } => {
                self.pause_download(id).map(IpcResponsePayload::Download)
            }
            IpcRequestPayload::ResumeDownload { id } => {
                self.resume_download(id).map(IpcResponsePayload::Download)
            }
            IpcRequestPayload::CancelDownload { id } => {
                self.cancel_download(id).map(IpcResponsePayload::Download)
            }
            IpcRequestPayload::OpenDownload { id } => {
                self.open_download(id).map(IpcResponsePayload::Download)
            }
            IpcRequestPayload::RevealDownload { id } => {
                self.reveal_download(id).map(IpcResponsePayload::Download)
            }
            IpcRequestPayload::GetDownloadPolicySettings => self
                .get_download_policy_settings()
                .map(IpcResponsePayload::DownloadPolicySettings),
            IpcRequestPayload::SetDownloadDefaultPolicy { policy } => self
                .set_download_default_policy(policy)
                .map(IpcResponsePayload::DownloadPolicySettings),
            IpcRequestPayload::SetDownloadSitePolicy { origin, policy } => self
                .set_download_site_policy(&origin, policy)
                .map(IpcResponsePayload::DownloadPolicySettings),
            IpcRequestPayload::ClearDownloadSitePolicy { origin } => self
                .clear_download_site_policy(&origin)
                .map(IpcResponsePayload::DownloadPolicySettings),
        }?;

        Ok(IpcResponse {
            version: IPC_SCHEMA_VERSION,
            payload,
        })
    }

    pub fn open_url(&self, url: &str) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        let result = app.open_url(url).map(BrowserPageDto::from);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn activate_link(
        &self,
        frame_id: &str,
        href: &str,
        target: Option<&str>,
    ) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        let result = app
            .activate_link(frame_id, href, target)
            .map(BrowserPageDto::from);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn get_page_view(&self) -> Result<BrowserPageDto, AppError> {
        let app = self.lock_app()?;
        Ok(BrowserPageDto::from(app.get_page_view()))
    }

    pub fn set_viewport(&self, width: i64, height: i64) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        let result = app.set_viewport(width, height).map(BrowserPageDto::from);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn reload(&self) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        let result = app.reload().map(BrowserPageDto::from);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn back(&self) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        let result = app.back().map(BrowserPageDto::from);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn forward(&self) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        let result = app.forward().map(BrowserPageDto::from);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
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
        let path = latest_crash_report_path()?;
        let content = fs::read_to_string(&path).ok()?;
        let mut report = serde_json::from_str::<CrashReportDto>(&content).ok()?;
        if report.path.is_empty() {
            report.path = path.display().to_string();
        }
        Some(report)
    }

    pub fn new_tab(&self) -> Result<TabSummary, AppError> {
        let mut app = self.lock_app()?;
        let summary = app.new_tab();
        drop(app);
        self.persist_latest_session_snapshot(true);
        Ok(summary)
    }

    pub fn duplicate_tab(&self, id: u32) -> Result<TabSummary, AppError> {
        let mut app = self.lock_app()?;
        let result = app.duplicate_tab(id);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn switch_tab(&self, id: u32) -> Result<BrowserPageDto, AppError> {
        let mut app = self.lock_app()?;
        let result = app.switch_tab(id).map(BrowserPageDto::from);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn close_tab(&self, id: u32) -> Result<Vec<TabSummary>, AppError> {
        let mut app = self.lock_app()?;
        let result = app.close_tab(id);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn move_tab(&self, id: u32, target_index: usize) -> Result<Vec<TabSummary>, AppError> {
        let mut app = self.lock_app()?;
        let result = app.move_tab(id, target_index);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn set_tab_pinned(&self, id: u32, pinned: bool) -> Result<Vec<TabSummary>, AppError> {
        let mut app = self.lock_app()?;
        let result = app.set_tab_pinned(id, pinned);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn set_tab_muted(&self, id: u32, muted: bool) -> Result<Vec<TabSummary>, AppError> {
        let mut app = self.lock_app()?;
        let result = app.set_tab_muted(id, muted);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn list_tabs(&self) -> Result<Vec<TabSummary>, AppError> {
        let app = self.lock_app()?;
        Ok(app.list_tabs())
    }

    pub fn search(&self, query: &str) -> Result<Vec<SearchResult>, AppError> {
        let app = self.lock_app()?;
        app.search(query)
    }

    pub fn omnibox_suggestions(
        &self,
        query: &str,
        current_index: Option<usize>,
    ) -> Result<OmniboxSuggestionSet, AppError> {
        let app = self.lock_app()?;
        app.omnibox_suggestions(query, current_index)
    }

    pub fn register_tls_exception(&self, url: &str) -> Result<(), AppError> {
        let mut app = self.lock_app()?;
        let result = app.register_tls_exception(url);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn enqueue_download(&self, url: &str) -> Result<DownloadEntry, AppError> {
        let mut app = self.lock_app()?;
        app.enqueue_download(url)
    }

    pub fn list_downloads(&self) -> Result<Vec<DownloadEntry>, AppError> {
        let app = self.lock_app()?;
        Ok(app.list_downloads())
    }

    pub fn get_download_progress(&self, id: u64) -> Result<DownloadEntry, AppError> {
        let app = self.lock_app()?;
        app.get_download_progress(id)
    }

    pub fn pause_download(&self, id: u64) -> Result<DownloadEntry, AppError> {
        let mut app = self.lock_app()?;
        app.pause_download(id)
    }

    pub fn resume_download(&self, id: u64) -> Result<DownloadEntry, AppError> {
        let mut app = self.lock_app()?;
        app.resume_download(id)
    }

    pub fn cancel_download(&self, id: u64) -> Result<DownloadEntry, AppError> {
        let mut app = self.lock_app()?;
        app.cancel_download(id)
    }

    pub fn open_download(&self, id: u64) -> Result<DownloadEntry, AppError> {
        let app = self.lock_app()?;
        app.open_download(id)
    }

    pub fn reveal_download(&self, id: u64) -> Result<DownloadEntry, AppError> {
        let app = self.lock_app()?;
        app.reveal_download(id)
    }

    pub fn get_download_policy_settings(&self) -> Result<DownloadPolicySettings, AppError> {
        let app = self.lock_app()?;
        Ok(app.get_download_policy_settings())
    }

    pub fn set_download_default_policy(
        &self,
        policy: DownloadSavePolicy,
    ) -> Result<DownloadPolicySettings, AppError> {
        let mut app = self.lock_app()?;
        app.set_download_default_policy(policy)
    }

    pub fn set_download_site_policy(
        &self,
        origin: &str,
        policy: DownloadSavePolicy,
    ) -> Result<DownloadPolicySettings, AppError> {
        let mut app = self.lock_app()?;
        app.set_download_site_policy(origin, policy)
    }

    pub fn clear_download_site_policy(
        &self,
        origin: &str,
    ) -> Result<DownloadPolicySettings, AppError> {
        let mut app = self.lock_app()?;
        app.clear_download_site_policy(origin)
    }

    pub fn update_scroll_positions(
        &self,
        positions: Vec<FrameScrollPositionSnapshot>,
    ) -> Result<(), AppError> {
        let mut app = self.lock_app()?;
        let result = app.update_scroll_positions(positions);
        drop(app);
        self.persist_latest_session_snapshot(result.is_ok());
        result
    }

    pub fn restore_session_snapshot(&self) -> Result<bool, AppError> {
        let Some(snapshot) = read_session_snapshot(session_snapshot_path())? else {
            return Ok(false);
        };
        let mut app = self.lock_app()?;
        app.import_session_snapshot(snapshot)?;
        Ok(true)
    }

    pub fn save_session_snapshot(&self) -> Result<(), AppError> {
        self.persist_session_snapshot(session_snapshot_path())
    }

    pub fn save_crash_snapshot(&self) -> Result<(), AppError> {
        self.persist_session_snapshot(crash_session_snapshot_path())
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

    fn persist_session_snapshot(&self, path: impl AsRef<Path>) -> Result<(), AppError> {
        let app = self.lock_app()?;
        let snapshot = app.export_session_snapshot();
        drop(app);
        write_session_snapshot(path.as_ref(), &snapshot)
    }

    fn persist_latest_session_snapshot(&self, should_save: bool) {
        if !should_save {
            return;
        }
        if let Err(error) = self.save_crash_snapshot() {
            log_recovery_event("session_snapshot_save_failed", None, &error.message);
        }
        if let Err(error) = self.save_session_snapshot() {
            log_recovery_event("session_snapshot_save_failed", None, &error.message);
        }
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
        let _ = self.save_session_snapshot();
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
            scroll_position: ScrollPositionDto {
                x: frame.scroll_position.x,
                y: frame.scroll_position.y,
            },
            render_backend: {
                #[allow(deprecated)]
                match frame.render_backend {
                    cosmo_runtime::RenderBackendKind::WebView
                    | cosmo_runtime::RenderBackendKind::NativeScene => "native_scene".to_string(),
                }
            },
            document_url: frame.document_url,
            scene_items: {
                let (list, _errors) = scene_items_to_paint_commands(&frame.scene_items);
                replay_paint_commands(&list.commands)
            },
            paint_commands: {
                let (list, errors) = scene_items_to_paint_commands(&frame.scene_items);
                for error in errors {
                    log_recovery_event("paint_fallback", None, &error.message);
                }
                list.commands
            },
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

fn load_startup_app() -> StarshipApp {
    match read_session_snapshot(session_snapshot_path()) {
        Ok(Some(snapshot)) => {
            let mut app = StarshipApp::default();
            match app.import_session_snapshot(snapshot) {
                Ok(()) => app,
                Err(error) => {
                    log_recovery_event("session_snapshot_restore_failed", None, &error.message);
                    StarshipApp::default()
                }
            }
        }
        Ok(None) => StarshipApp::default(),
        Err(error) => {
            log_recovery_event("session_snapshot_read_failed", None, &error.message);
            StarshipApp::default()
        }
    }
}

fn crash_repository_dir() -> PathBuf {
    std::env::var("COSMO_CRASH_REPOSITORY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("cosmobrowse-crash-reports"))
}

fn latest_crash_report_path() -> Option<PathBuf> {
    let repo = crash_repository_dir();
    // Privacy note: consumers only read the newest crash report from the
    // repository instead of bulk-loading every historical entry into memory.
    // This keeps diagnostics focused on the latest failure while avoiding
    // unnecessary propagation of older user-adjacent crash metadata.
    let mut entries = fs::read_dir(&repo)
        .ok()?
        .filter_map(|entry| entry.ok().map(|value| value.path()))
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("json"))
        .collect::<Vec<_>>();
    entries.sort();
    entries.pop().or_else(|| {
        let legacy = std::env::temp_dir().join("cosmobrowse-crash-report.json");
        legacy.exists().then_some(legacy)
    })
}

fn session_snapshot_path() -> std::path::PathBuf {
    std::env::var("COSMO_SESSION_SNAPSHOT_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("cosmobrowse-session-snapshot.json"))
}

fn crash_session_snapshot_path() -> std::path::PathBuf {
    std::env::var("COSMO_CRASH_SESSION_SNAPSHOT_PATH")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("cosmobrowse-session-snapshot-crash.json"))
}

fn read_session_snapshot(path: std::path::PathBuf) -> Result<Option<SessionSnapshot>, AppError> {
    let content = match fs::read_to_string(&path) {
        Ok(content) => content,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(AppError::state(format!(
                "Failed to read session snapshot {}: {error}",
                path.display()
            )))
        }
    };
    serde_json::from_str(&content).map(Some).map_err(|error| {
        AppError::state(format!(
            "Failed to parse session snapshot {}: {error}",
            path.display()
        ))
    })
}

fn write_session_snapshot(path: &Path, snapshot: &SessionSnapshot) -> Result<(), AppError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            AppError::state(format!(
                "Failed to prepare session snapshot directory {}: {error}",
                parent.display()
            ))
        })?;
    }
    let payload = serde_json::to_string_pretty(snapshot).map_err(|error| {
        AppError::state(format!("Failed to serialize session snapshot: {error}"))
    })?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, payload).map_err(|error| {
        AppError::state(format!(
            "Failed to write session snapshot {}: {error}",
            tmp_path.display()
        ))
    })?;
    fs::rename(&tmp_path, path).map_err(|error| {
        AppError::state(format!(
            "Failed to finalize session snapshot {}: {error}",
            path.display()
        ))
    })
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
    use cosmo_runtime::ScrollPosition;
    use std::sync::{Arc, OnceLock};

    fn unix_timestamp_ms() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    }

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

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
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

    #[test]
    fn adapter_restores_saved_session_snapshot_on_startup() {
        let _env_guard = env_lock().lock().expect("env lock");
        let snapshot_dir =
            std::env::temp_dir().join(format!("cosmobrowse-session-test-{}", unix_timestamp_ms()));
        let session_path = snapshot_dir.join("session.json");
        let crash_path = snapshot_dir.join("session-crash.json");
        std::env::set_var("COSMO_SESSION_SNAPSHOT_PATH", &session_path);
        std::env::set_var("COSMO_CRASH_SESSION_SNAPSHOT_PATH", &crash_path);

        let mut app = StarshipApp::default();
        let view = app
            .open_url("fixture://abehiroshi/index")
            .expect("fixture should load");
        let left_frame = view.root_frame.child_frames[0].id.clone();
        app.activate_link(&left_frame, "fixture://abehiroshi/prof", Some("right"))
            .expect("targeted navigation should work");
        app.update_scroll_positions(vec![
            FrameScrollPositionSnapshot {
                frame_id: "root".to_string(),
                position: ScrollPosition { x: 0, y: 48 },
            },
            FrameScrollPositionSnapshot {
                frame_id: "root/right".to_string(),
                position: ScrollPosition { x: 4, y: 96 },
            },
        ])
        .expect("scroll positions should update");
        write_session_snapshot(&session_path, &app.export_session_snapshot())
            .expect("snapshot should persist");

        let restored = NativeAdapter::with_process_host(
            Box::new(FakeProcessHost::new(Arc::new(Mutex::new(
                FakeProcessState::default(),
            )))),
            Duration::ZERO,
        );
        let page = restored.get_page_view().expect("restored page view");

        assert_eq!(page.current_url, "fixture://abehiroshi/index");
        assert_eq!(page.root_frame.scroll_position.y, 48);
        assert_eq!(
            page.root_frame.child_frames[1].current_url,
            "fixture://abehiroshi/prof"
        );
        assert_eq!(page.root_frame.child_frames[1].scroll_position.y, 96);

        std::env::remove_var("COSMO_SESSION_SNAPSHOT_PATH");
        std::env::remove_var("COSMO_CRASH_SESSION_SNAPSHOT_PATH");
        let _ = fs::remove_file(&session_path);
        let _ = fs::remove_file(&crash_path);
        let _ = fs::remove_dir_all(&snapshot_dir);
    }

    #[test]
    fn get_latest_crash_report_reads_newest_entry_from_repository() {
        let _env_guard = env_lock().lock().expect("env lock");
        let repo_dir =
            std::env::temp_dir().join(format!("cosmobrowse-crash-test-{}", unix_timestamp_ms()));
        std::env::set_var("COSMO_CRASH_REPOSITORY_DIR", &repo_dir);
        fs::create_dir_all(&repo_dir).expect("repo dir");

        let older = CrashReportDto {
            path: repo_dir.join("crash-1.json").display().to_string(),
            crashed_at_ms: 1,
            reason: "older".to_string(),
            build_id: "build-a".to_string(),
            commit_hash: "aaaa".to_string(),
            transport: "adapter_native".to_string(),
            active_url: "fixture://older".to_string(),
            last_command: "open_url".to_string(),
            reproduction: vec!["step".to_string()],
        };
        let newer = CrashReportDto {
            path: repo_dir.join("crash-2.json").display().to_string(),
            crashed_at_ms: 2,
            reason: "newer".to_string(),
            build_id: "build-b".to_string(),
            commit_hash: "bbbb".to_string(),
            transport: "adapter_native".to_string(),
            active_url: "fixture://newer".to_string(),
            last_command: "reload".to_string(),
            reproduction: vec!["step".to_string()],
        };

        fs::write(
            repo_dir.join("crash-1.json"),
            serde_json::to_string_pretty(&older).expect("serialize older"),
        )
        .expect("write older");
        fs::write(
            repo_dir.join("crash-2.json"),
            serde_json::to_string_pretty(&newer).expect("serialize newer"),
        )
        .expect("write newer");

        let adapter = NativeAdapter::with_process_host(
            Box::new(FakeProcessHost::new(Arc::new(Mutex::new(
                FakeProcessState::default(),
            )))),
            Duration::ZERO,
        );
        let report = adapter.get_latest_crash_report().expect("latest report");
        assert_eq!(report.reason, "newer");
        assert_eq!(report.active_url, "fixture://newer");

        std::env::remove_var("COSMO_CRASH_REPOSITORY_DIR");
        let _ = fs::remove_dir_all(&repo_dir);
    }
}

fn replay_paint_commands(commands: &[PaintCommand]) -> Vec<SceneItem> {
    let mut items = Vec::new();
    for command in commands {
        match command {
            PaintCommand::DrawRect(rect) => items.push(SceneItem::Rect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
                background_color: rect.background_color.clone(),
                background_image: rect.background_image.clone(),
                opacity: rect.opacity,
                z_index: rect.z_index,
                clip_rect: rect.clip_rect,
                anchor_id: rect.anchor_id.clone(),
            }),
            PaintCommand::DrawText(text) => items.push(SceneItem::Text {
                x: text.x,
                y: text.y,
                text: text.text.clone(),
                color: text.color.clone(),
                font_px: text.font_px,
                font_family: text.font_family.clone(),
                underline: text.underline,
                bold: text.bold,
                opacity: text.opacity,
                href: text.href.clone(),
                target: text.target.clone(),
                z_index: text.z_index,
                clip_rect: text.clip_rect,
            }),
            PaintCommand::DrawImage(image) => items.push(SceneItem::Image {
                x: image.x,
                y: image.y,
                width: image.width,
                height: image.height,
                src: image.src.clone(),
                alt: image.alt.clone(),
                opacity: image.opacity,
                href: image.href.clone(),
                target: image.target.clone(),
                z_index: image.z_index,
                clip_rect: image.clip_rect,
            }),
        }
    }
    items
}
