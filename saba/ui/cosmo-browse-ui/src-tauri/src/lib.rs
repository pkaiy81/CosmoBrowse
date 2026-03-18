use adapter_native::{BrowserPageDto, CrashReportDto, IpcRequest, IpcResponse, NativeAdapter};
use cosmo_runtime::{
    AppError, FrameScrollPositionSnapshot, NavigationState, OmniboxSuggestionSet, SearchResult,
    TabSummary,
};
use serde::Serialize;
use std::backtrace::Backtrace;
use std::collections::hash_map::DefaultHasher;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct AppState {
    adapter: NativeAdapter,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
struct ReleaseRolloutStatus {
    channel: String,
    native_default_enabled: bool,
    adapter_tauri_fallback_enabled: bool,
    rollout_percentage: u8,
    assignment_bucket: u8,
    assigned_transport: String,
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
fn release_rollout_status() -> ReleaseRolloutStatus {
    let channel = std::env::var("COSMO_RELEASE_CHANNEL").unwrap_or_else(|_| "stable".to_string());
    let rollout_percentage = std::env::var("COSMO_NATIVE_DEFAULT_ROLLOUT_PERCENT")
        .ok()
        .and_then(|value| value.parse::<u8>().ok())
        .map(|value| value.min(100))
        .unwrap_or_else(|| default_rollout_percentage(&channel));
    let assignment_bucket = stable_rollout_bucket();
    let adapter_tauri_fallback_enabled = std::env::var("COSMO_DISABLE_ADAPTER_TAURI_FALLBACK")
        .map(|value| value != "1")
        .unwrap_or(true);
    let assigned_transport = std::env::var("COSMO_FORCE_TRANSPORT")
        .ok()
        .filter(|value| matches!(value.as_str(), "adapter_native" | "adapter_tauri"));

    let native_default_enabled = match assigned_transport.as_deref() {
        Some("adapter_native") => true,
        Some("adapter_tauri") => false,
        _ => assignment_bucket < rollout_percentage,
    };

    ReleaseRolloutStatus {
        channel,
        native_default_enabled,
        adapter_tauri_fallback_enabled,
        rollout_percentage,
        assignment_bucket,
        assigned_transport: if native_default_enabled {
            "adapter_native".to_string()
        } else {
            "adapter_tauri".to_string()
        },
    }
}

#[tauri::command]
fn new_tab(state: tauri::State<'_, AppState>) -> Result<TabSummary, AppError> {
    state.adapter.new_tab()
}

#[tauri::command]
fn duplicate_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<TabSummary, AppError> {
    state.adapter.duplicate_tab(id)
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
fn move_tab(
    state: tauri::State<'_, AppState>,
    id: u32,
    target_index: usize,
) -> Result<Vec<TabSummary>, AppError> {
    state.adapter.move_tab(id, target_index)
}

#[tauri::command]
fn set_tab_pinned(
    state: tauri::State<'_, AppState>,
    id: u32,
    pinned: bool,
) -> Result<Vec<TabSummary>, AppError> {
    state.adapter.set_tab_pinned(id, pinned)
}

#[tauri::command]
fn set_tab_muted(
    state: tauri::State<'_, AppState>,
    id: u32,
    muted: bool,
) -> Result<Vec<TabSummary>, AppError> {
    state.adapter.set_tab_muted(id, muted)
}

#[tauri::command]
fn list_tabs(state: tauri::State<'_, AppState>) -> Result<Vec<TabSummary>, AppError> {
    state.adapter.list_tabs()
}

#[tauri::command]
fn search(state: tauri::State<'_, AppState>, query: String) -> Result<Vec<SearchResult>, AppError> {
    state.adapter.search(&query)
}

#[tauri::command]
fn omnibox_suggestions(
    state: tauri::State<'_, AppState>,
    query: String,
    current_index: Option<usize>,
) -> Result<OmniboxSuggestionSet, AppError> {
    state.adapter.omnibox_suggestions(&query, current_index)
}

#[tauri::command]
fn update_scroll_positions(
    state: tauri::State<'_, AppState>,
    positions: Vec<FrameScrollPositionSnapshot>,
) -> Result<bool, AppError> {
    state.adapter.update_scroll_positions(positions)?;
    Ok(true)
}

fn install_crash_hook() {
    std::panic::set_hook(Box::new(|panic_info| {
        let _ = persist_pre_crash_snapshot();
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

fn session_snapshot_path() -> PathBuf {
    std::env::var("COSMO_SESSION_SNAPSHOT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("cosmobrowse-session-snapshot.json"))
}

fn crash_session_snapshot_path() -> PathBuf {
    std::env::var("COSMO_CRASH_SESSION_SNAPSHOT_PATH")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("cosmobrowse-session-snapshot-crash.json"))
}

fn persist_pre_crash_snapshot() -> std::io::Result<()> {
    let source = session_snapshot_path();
    let destination = crash_session_snapshot_path();
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let payload = std::fs::read(&source)?;
    std::fs::write(destination, payload)
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn default_rollout_percentage(channel: &str) -> u8 {
    match channel {
        "dev" | "nightly" => 100,
        "beta" => 50,
        _ => 10,
    }
}

fn stable_rollout_bucket() -> u8 {
    let fingerprint = std::env::var("COSMO_ROLLOUT_DEVICE_ID")
        .or_else(|_| std::env::var("HOSTNAME"))
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .or_else(|_| std::env::var("USER"))
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "anonymous-device".to_string());
    let mut hasher = DefaultHasher::new();
    // Release rollout buckets must remain stable across launches so the same
    // device stays on the same transport until operators intentionally change
    // the percentage or force a rollback.
    fingerprint.hash(&mut hasher);
    (hasher.finish() % 100) as u8
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
            release_rollout_status,
            new_tab,
            duplicate_tab,
            switch_tab,
            close_tab,
            move_tab,
            set_tab_pinned,
            set_tab_muted,
            list_tabs,
            search,
            omnibox_suggestions,
            update_scroll_positions
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
