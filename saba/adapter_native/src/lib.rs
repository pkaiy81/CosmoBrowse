use cosmo_runtime::{
    AppError, AppMetricsSnapshot, AppService, NavigationState, OrbitSnapshot, SceneItem,
    SearchResult, StarshipApp, TabSummary,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::sync::Mutex;

pub const IPC_SCHEMA_VERSION: u32 = 1;

#[derive(Default)]
pub struct NativeAdapter {
    app: Mutex<StarshipApp>,
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
pub struct CrashReportDto {
    pub path: String,
    pub crashed_at_ms: u64,
    pub reason: String,
    pub reproduction: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum IpcRequestPayload {
    OpenUrl { url: String },
    GetPageView,
    SetViewport { width: i64, height: i64 },
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
    SwitchTab { id: u32 },
    CloseTab { id: u32 },
    ListTabs,
    Search { query: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IpcResponse {
    pub version: u32,
    #[serde(flatten)]
    pub payload: IpcResponsePayload,
}

const fn ipc_schema_version() -> u32 {
    IPC_SCHEMA_VERSION
}

impl NativeAdapter {
    pub fn dispatch(&self, request: IpcRequest) -> Result<IpcResponse, AppError> {
        if request.version != IPC_SCHEMA_VERSION {
            return Err(AppError::invalid_input(format!(
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
            IpcRequestPayload::SetViewport { width, height } => {
                self.set_viewport(width, height).map(IpcResponsePayload::Page)
            }
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
            IpcRequestPayload::GetLatestCrashReport => {
                Ok(IpcResponsePayload::CrashReport(self.get_latest_crash_report()))
            }
            IpcRequestPayload::NewTab => self.new_tab().map(IpcResponsePayload::Tab),
            IpcRequestPayload::SwitchTab { id } => self.switch_tab(id).map(IpcResponsePayload::Page),
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
            html_content: frame.html_content,
            child_frames: frame
                .child_frames
                .into_iter()
                .map(BrowserFrameDto::from)
                .collect(),
        }
    }
}

fn collect_dom_snapshots(frame: &cosmo_runtime::FrameViewModel, out: &mut Vec<DomSnapshotEntryDto>) {
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
