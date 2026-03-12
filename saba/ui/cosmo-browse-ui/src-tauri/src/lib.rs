use cosmo_runtime::{
    AppError, AppMetricsSnapshot, AppService, NavigationState, OrbitSnapshot, SceneItem,
    SearchResult, StarshipApp, TabSummary,
};
use serde::Serialize;
use std::backtrace::Backtrace;
use std::fs::{self, File};
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct AppState {
    app: Mutex<StarshipApp>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct BrowserPageDto {
    current_url: String,
    title: String,
    diagnostics: Vec<String>,
    content_size: ContentSizeDto,
    network_log: Vec<String>,
    console_log: Vec<String>,
    dom_snapshot: Vec<DomSnapshotEntryDto>,
    root_frame: BrowserFrameDto,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct DomSnapshotEntryDto {
    frame_id: String,
    document_url: String,
    html: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct BrowserFrameDto {
    id: String,
    name: Option<String>,
    current_url: String,
    title: String,
    diagnostics: Vec<String>,
    rect: FrameRectDto,
    render_backend: String,
    document_url: String,
    scene_items: Vec<SceneItem>,
    html_content: Option<String>,
    child_frames: Vec<BrowserFrameDto>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct ContentSizeDto {
    width: i64,
    height: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct FrameRectDto {
    x: i64,
    y: i64,
    width: i64,
    height: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct CrashReportDto {
    path: String,
    crashed_at_ms: u64,
    reason: String,
    reproduction: Vec<String>,
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
            // Rendering contract: UI consumes scene items only. Legacy WebView hints
            // are normalized to native_scene during DTO serialization.
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

#[tauri::command]
fn open_url(state: tauri::State<'_, AppState>, url: String) -> Result<BrowserPageDto, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.open_url(&url).map(BrowserPageDto::from)
}

#[tauri::command]
fn activate_link(
    state: tauri::State<'_, AppState>,
    frame_id: String,
    href: String,
    target: Option<String>,
) -> Result<BrowserPageDto, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.activate_link(&frame_id, &href, target.as_deref())
        .map(BrowserPageDto::from)
}

#[tauri::command]
fn get_page_view(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    let app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(BrowserPageDto::from(app.get_page_view()))
}

#[tauri::command]
fn set_viewport(
    state: tauri::State<'_, AppState>,
    width: i64,
    height: i64,
) -> Result<BrowserPageDto, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.set_viewport(width, height).map(BrowserPageDto::from)
}

#[tauri::command]
fn reload(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.reload().map(BrowserPageDto::from)
}

#[tauri::command]
fn back(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.back().map(BrowserPageDto::from)
}

#[tauri::command]
fn forward(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.forward().map(BrowserPageDto::from)
}

#[tauri::command]
fn get_navigation_state(state: tauri::State<'_, AppState>) -> Result<NavigationState, AppError> {
    let app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.get_navigation_state())
}

#[tauri::command]
fn get_metrics(state: tauri::State<'_, AppState>) -> Result<AppMetricsSnapshot, AppError> {
    let app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.get_metrics())
}

#[tauri::command]
fn get_latest_crash_report() -> Option<CrashReportDto> {
    let path = crash_report_path();
    let content = fs::read_to_string(&path).ok()?;
    serde_json::from_str::<CrashReportDto>(&content).ok()
}

#[tauri::command]
fn new_tab(state: tauri::State<'_, AppState>) -> Result<TabSummary, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.new_tab())
}

#[tauri::command]
fn switch_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<BrowserPageDto, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.switch_tab(id).map(BrowserPageDto::from)
}

#[tauri::command]
fn close_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<Vec<TabSummary>, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.close_tab(id)
}

#[tauri::command]
fn list_tabs(state: tauri::State<'_, AppState>) -> Result<Vec<TabSummary>, AppError> {
    let app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.list_tabs())
}

#[tauri::command]
fn search(state: tauri::State<'_, AppState>, query: String) -> Result<Vec<SearchResult>, AppError> {
    let app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.search(&query)
}

fn install_crash_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let reason = if let Some(message) = panic_info.payload().downcast_ref::<&str>() {
            (*message).to_string()
        } else if let Some(message) = panic_info.payload().downcast_ref::<String>() {
            message.clone()
        } else {
            "panic without message".to_string()
        };
        let location = panic_info
            .location()
            .map(|loc| format!("{}:{}", loc.file(), loc.line()))
            .unwrap_or_else(|| "unknown".to_string());
        let report = CrashReportDto {
            path: crash_report_path().display().to_string(),
            crashed_at_ms: unix_timestamp_ms(),
            reason: format!("{reason} @ {location}"),
            reproduction: vec![
                "Capture URL and last user action before crash".to_string(),
                "Re-run with RUST_BACKTRACE=1 to include full stack traces".to_string(),
                "Attach diagnostics panel output (network/dom/console)".to_string(),
            ],
        };

        if let Ok(payload) = serde_json::to_string_pretty(&report) {
            if let Ok(mut file) = File::create(crash_report_path()) {
                let _ = file.write_all(payload.as_bytes());
            }
            eprintln!("CosmoBrowse crash report saved: {}", report.path);
            eprintln!("{}", payload);
            eprintln!("backtrace:\n{}", Backtrace::force_capture());
        }
    }));
}

fn crash_report_path() -> PathBuf {
    std::env::temp_dir().join("cosmobrowse-crash-report.json")
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    install_crash_hook();
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            open_url,
            activate_link,
            get_page_view,
            set_viewport,
            reload,
            back,
            forward,
            get_navigation_state,
            get_metrics,
            get_latest_crash_report,
            new_tab,
            switch_tab,
            close_tab,
            list_tabs,
            search
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
