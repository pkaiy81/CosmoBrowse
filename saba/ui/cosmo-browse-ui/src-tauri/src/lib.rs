use adapter_native::{BrowserPageDto, CrashReportDto, IpcRequest, IpcResponse, NativeAdapter};
use cosmo_runtime::{AppError, AppMetricsSnapshot, NavigationState, SearchResult, TabSummary};
use std::backtrace::Backtrace;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct AppState {
    adapter: NativeAdapter,
}


#[tauri::command]
fn dispatch_ipc(
    state: tauri::State<'_, AppState>,
    request: IpcRequest,
) -> Result<IpcResponse, AppError> {
    state.adapter.dispatch(request)
}

#[tauri::command]
fn open_url(state: tauri::State<'_, AppState>, url: String) -> Result<BrowserPageDto, AppError> {
    state.adapter.open_url(&url)
}

#[tauri::command]
fn activate_link(
    state: tauri::State<'_, AppState>,
    frame_id: String,
    href: String,
    target: Option<String>,
) -> Result<BrowserPageDto, AppError> {
    state
        .adapter
        .activate_link(&frame_id, &href, target.as_deref())
}

#[tauri::command]
fn get_page_view(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    state.adapter.get_page_view()
}

#[tauri::command]
fn set_viewport(
    state: tauri::State<'_, AppState>,
    width: i64,
    height: i64,
) -> Result<BrowserPageDto, AppError> {
    state.adapter.set_viewport(width, height)
}

#[tauri::command]
fn reload(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    state.adapter.reload()
}

#[tauri::command]
fn back(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    state.adapter.back()
}

#[tauri::command]
fn forward(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    state.adapter.forward()
}

#[tauri::command]
fn get_navigation_state(state: tauri::State<'_, AppState>) -> Result<NavigationState, AppError> {
    state.adapter.get_navigation_state()
}

#[tauri::command]
fn get_metrics(state: tauri::State<'_, AppState>) -> Result<AppMetricsSnapshot, AppError> {
    state.adapter.get_metrics()
}

#[tauri::command]
fn get_latest_crash_report(state: tauri::State<'_, AppState>) -> Option<CrashReportDto> {
    state.adapter.get_latest_crash_report()
}

#[tauri::command]
fn new_tab(state: tauri::State<'_, AppState>) -> Result<TabSummary, AppError> {
    state.adapter.new_tab()
}

#[tauri::command]
fn switch_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<BrowserPageDto, AppError> {
    state.adapter.switch_tab(id)
}

#[tauri::command]
fn close_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<Vec<TabSummary>, AppError> {
    state.adapter.close_tab(id)
}

#[tauri::command]
fn list_tabs(state: tauri::State<'_, AppState>) -> Result<Vec<TabSummary>, AppError> {
    state.adapter.list_tabs()
}

#[tauri::command]
fn search(state: tauri::State<'_, AppState>, query: String) -> Result<Vec<SearchResult>, AppError> {
    state.adapter.search(&query)
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
            dispatch_ipc,
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
