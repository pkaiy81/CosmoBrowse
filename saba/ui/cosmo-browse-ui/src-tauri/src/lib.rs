use adapter_native::{BrowserPageDto, CrashReportDto, IpcRequest, IpcResponse, NativeAdapter};
use cosmo_runtime::{
    AppError, DownloadEntry, DownloadPolicySettings, DownloadSavePolicy,
    FrameScrollPositionSnapshot, NavigationState, OmniboxSuggestionSet, SearchResult, TabSummary,
};
use serde::Serialize;
use std::backtrace::Backtrace;
use std::collections::hash_map::DefaultHasher;
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Default)]
struct AppState {
    adapter: NativeAdapter,
}

#[derive(Debug, Default, Clone)]
struct CrashContext {
    active_url: Option<String>,
    last_command: Option<String>,
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
    record_last_command(format!("dispatch_ipc::{}", ipc_payload_label(&request)));
    match &request.payload {
        adapter_native::IpcRequestPayload::OpenUrl { url }
        | adapter_native::IpcRequestPayload::RegisterTlsException { url }
        | adapter_native::IpcRequestPayload::EnqueueDownload { url } => record_active_url(url),
        _ => {}
    }
    let response = state.adapter.dispatch(request)?;
    if let adapter_native::IpcResponsePayload::Page(page) = &response.payload {
        record_active_url(&page.current_url);
    }
    Ok(response)
}

#[tauri::command]
fn open_url(state: tauri::State<'_, AppState>, url: String) -> Result<BrowserPageDto, AppError> {
    record_navigation_context("open_url", Some(&url));
    let page = state.adapter.open_url(&url)?;
    record_active_url(&page.current_url);
    Ok(page)
}

#[tauri::command]
fn activate_link(
    state: tauri::State<'_, AppState>,
    frame_id: String,
    href: String,
    target: Option<String>,
) -> Result<BrowserPageDto, AppError> {
    record_last_command(format!("activate_link:{frame_id}"));
    state
        .adapter
        .activate_link(&frame_id, &href, target.as_deref())
        .inspect(|page| record_active_url(&page.current_url))
}

#[tauri::command]
fn get_page_view(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    record_last_command("get_page_view");
    let page = state.adapter.get_page_view()?;
    record_active_url(&page.current_url);
    Ok(page)
}

#[tauri::command]
fn set_viewport(
    state: tauri::State<'_, AppState>,
    width: i64,
    height: i64,
) -> Result<BrowserPageDto, AppError> {
    record_last_command(format!("set_viewport:{width}x{height}"));
    let page = state.adapter.set_viewport(width, height)?;
    record_active_url(&page.current_url);
    Ok(page)
}

#[tauri::command]
fn reload(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    record_last_command("reload");
    let page = state.adapter.reload()?;
    record_active_url(&page.current_url);
    Ok(page)
}

#[tauri::command]
fn back(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    record_last_command("back");
    let page = state.adapter.back()?;
    record_active_url(&page.current_url);
    Ok(page)
}

#[tauri::command]
fn forward(state: tauri::State<'_, AppState>) -> Result<BrowserPageDto, AppError> {
    record_last_command("forward");
    let page = state.adapter.forward()?;
    record_active_url(&page.current_url);
    Ok(page)
}

#[tauri::command]
fn get_navigation_state(state: tauri::State<'_, AppState>) -> Result<NavigationState, AppError> {
    record_last_command("get_navigation_state");
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
    record_last_command("new_tab");
    state.adapter.new_tab()
}

#[tauri::command]
fn duplicate_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<TabSummary, AppError> {
    record_last_command(format!("duplicate_tab:{id}"));
    state.adapter.duplicate_tab(id)
}

#[tauri::command]
fn switch_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<BrowserPageDto, AppError> {
    record_last_command(format!("switch_tab:{id}"));
    let page = state.adapter.switch_tab(id)?;
    record_active_url(&page.current_url);
    Ok(page)
}

#[tauri::command]
fn close_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<Vec<TabSummary>, AppError> {
    record_last_command(format!("close_tab:{id}"));
    state.adapter.close_tab(id)
}

#[tauri::command]
fn move_tab(
    state: tauri::State<'_, AppState>,
    id: u32,
    target_index: usize,
) -> Result<Vec<TabSummary>, AppError> {
    record_last_command(format!("move_tab:{id}->{target_index}"));
    state.adapter.move_tab(id, target_index)
}

#[tauri::command]
fn set_tab_pinned(
    state: tauri::State<'_, AppState>,
    id: u32,
    pinned: bool,
) -> Result<Vec<TabSummary>, AppError> {
    record_last_command(format!("set_tab_pinned:{id}:{pinned}"));
    state.adapter.set_tab_pinned(id, pinned)
}

#[tauri::command]
fn set_tab_muted(
    state: tauri::State<'_, AppState>,
    id: u32,
    muted: bool,
) -> Result<Vec<TabSummary>, AppError> {
    record_last_command(format!("set_tab_muted:{id}:{muted}"));
    state.adapter.set_tab_muted(id, muted)
}

#[tauri::command]
fn list_tabs(state: tauri::State<'_, AppState>) -> Result<Vec<TabSummary>, AppError> {
    record_last_command("list_tabs");
    state.adapter.list_tabs()
}

#[tauri::command]
fn search(state: tauri::State<'_, AppState>, query: String) -> Result<Vec<SearchResult>, AppError> {
    record_last_command("search");
    state.adapter.search(&query)
}

#[tauri::command]
fn omnibox_suggestions(
    state: tauri::State<'_, AppState>,
    query: String,
    current_index: Option<usize>,
) -> Result<OmniboxSuggestionSet, AppError> {
    record_last_command("omnibox_suggestions");
    state.adapter.omnibox_suggestions(&query, current_index)
}

#[tauri::command]
fn update_scroll_positions(
    state: tauri::State<'_, AppState>,
    positions: Vec<FrameScrollPositionSnapshot>,
) -> Result<bool, AppError> {
    record_last_command(format!("update_scroll_positions:{}", positions.len()));
    state.adapter.update_scroll_positions(positions)?;
    Ok(true)
}

#[tauri::command]
fn enqueue_download(
    state: tauri::State<'_, AppState>,
    url: String,
) -> Result<DownloadEntry, AppError> {
    record_navigation_context("enqueue_download", Some(&url));
    state.adapter.enqueue_download(&url)
}

#[tauri::command]
fn list_downloads(state: tauri::State<'_, AppState>) -> Result<Vec<DownloadEntry>, AppError> {
    record_last_command("list_downloads");
    state.adapter.list_downloads()
}

#[tauri::command]
fn get_download_progress(
    state: tauri::State<'_, AppState>,
    id: u64,
) -> Result<DownloadEntry, AppError> {
    record_last_command(format!("get_download_progress:{id}"));
    state.adapter.get_download_progress(id)
}

#[tauri::command]
fn pause_download(state: tauri::State<'_, AppState>, id: u64) -> Result<DownloadEntry, AppError> {
    record_last_command(format!("pause_download:{id}"));
    state.adapter.pause_download(id)
}

#[tauri::command]
fn resume_download(state: tauri::State<'_, AppState>, id: u64) -> Result<DownloadEntry, AppError> {
    record_last_command(format!("resume_download:{id}"));
    state.adapter.resume_download(id)
}

#[tauri::command]
fn cancel_download(state: tauri::State<'_, AppState>, id: u64) -> Result<DownloadEntry, AppError> {
    record_last_command(format!("cancel_download:{id}"));
    state.adapter.cancel_download(id)
}

#[tauri::command]
fn open_download(state: tauri::State<'_, AppState>, id: u64) -> Result<DownloadEntry, AppError> {
    record_last_command(format!("open_download:{id}"));
    state.adapter.open_download(id)
}

#[tauri::command]
fn reveal_download(state: tauri::State<'_, AppState>, id: u64) -> Result<DownloadEntry, AppError> {
    record_last_command(format!("reveal_download:{id}"));
    state.adapter.reveal_download(id)
}

#[tauri::command]
fn get_download_policy_settings(
    state: tauri::State<'_, AppState>,
) -> Result<DownloadPolicySettings, AppError> {
    record_last_command("get_download_policy_settings");
    state.adapter.get_download_policy_settings()
}

#[tauri::command]
fn set_download_default_policy(
    state: tauri::State<'_, AppState>,
    policy: DownloadSavePolicy,
) -> Result<DownloadPolicySettings, AppError> {
    record_last_command("set_download_default_policy");
    state.adapter.set_download_default_policy(policy)
}

#[tauri::command]
fn set_download_site_policy(
    state: tauri::State<'_, AppState>,
    origin: String,
    policy: DownloadSavePolicy,
) -> Result<DownloadPolicySettings, AppError> {
    record_last_command(format!("set_download_site_policy:{origin}"));
    state.adapter.set_download_site_policy(&origin, policy)
}

#[tauri::command]
fn clear_download_site_policy(
    state: tauri::State<'_, AppState>,
    origin: String,
) -> Result<DownloadPolicySettings, AppError> {
    record_last_command(format!("clear_download_site_policy:{origin}"));
    state.adapter.clear_download_site_policy(&origin)
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
        let crash_report_path = persist_crash_report_path();
        let crash_context = crash_context()
            .lock()
            .map(|context| context.clone())
            .unwrap_or_default();
        let report = CrashReportDto {
            path: crash_report_path.display().to_string(),
            crashed_at_ms: unix_timestamp_ms(),
            reason: format!("{reason} @ {location}"),
            build_id: build_id(),
            commit_hash: commit_hash(),
            transport: current_transport(),
            active_url: crash_context.active_url.unwrap_or_default(),
            last_command: crash_context.last_command.unwrap_or_default(),
            reproduction: vec![
                "Record the sanitized active URL, transport, and last command from this report"
                    .to_string(),
                "Attach page diagnostics (network/dom/console) from the last successful render"
                    .to_string(),
                "Re-run with RUST_BACKTRACE=1 to include full stack traces".to_string(),
                "Describe the user-visible symptom and the exact rollout percentage/channel"
                    .to_string(),
            ],
        };

        if let Ok(payload) = serde_json::to_string_pretty(&report) {
            if let Ok(mut file) = File::create(&crash_report_path) {
                let _ = file.write_all(payload.as_bytes());
            }
            eprintln!("CosmoBrowse crash report saved: {}", report.path);
            eprintln!("{}", payload);
            eprintln!("backtrace:\n{}", Backtrace::force_capture());
        }
    }));
}

fn crash_context() -> &'static Mutex<CrashContext> {
    static CONTEXT: OnceLock<Mutex<CrashContext>> = OnceLock::new();
    CONTEXT.get_or_init(|| Mutex::new(CrashContext::default()))
}

fn record_last_command(command: impl Into<String>) {
    if let Ok(mut context) = crash_context().lock() {
        context.last_command = Some(command.into());
    }
}

fn record_active_url(url: &str) {
    if let Ok(mut context) = crash_context().lock() {
        context.active_url = Some(sanitize_url_for_crash_report(url));
    }
}

fn record_navigation_context(command: &str, url: Option<&str>) {
    record_last_command(command.to_string());
    if let Some(url) = url {
        record_active_url(url);
    }
}

fn persist_crash_report_path() -> PathBuf {
    let repo = crash_repository_dir();
    let _ = fs::create_dir_all(&repo);
    repo.join(format!("crash-{}.json", unix_timestamp_ms()))
}

fn crash_repository_dir() -> PathBuf {
    std::env::var("COSMO_CRASH_REPOSITORY_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir().join("cosmobrowse-crash-reports"))
}

fn build_id() -> String {
    option_env!("COSMO_BUILD_ID")
        .map(str::to_string)
        .or_else(|| std::env::var("COSMO_BUILD_ID").ok())
        .or_else(|| option_env!("GITHUB_RUN_ID").map(str::to_string))
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_string())
}

fn commit_hash() -> String {
    option_env!("COSMO_COMMIT_HASH")
        .map(str::to_string)
        .or_else(|| std::env::var("COSMO_COMMIT_HASH").ok())
        .or_else(|| option_env!("GITHUB_SHA").map(str::to_string))
        .map(|hash| hash.chars().take(12).collect())
        .unwrap_or_default()
}

fn current_transport() -> String {
    release_rollout_status().assigned_transport
}

fn ipc_payload_label(request: &IpcRequest) -> &'static str {
    match &request.payload {
        adapter_native::IpcRequestPayload::OpenUrl { .. } => "open_url",
        adapter_native::IpcRequestPayload::GetPageView => "get_page_view",
        adapter_native::IpcRequestPayload::SetViewport { .. } => "set_viewport",
        adapter_native::IpcRequestPayload::Reload => "reload",
        adapter_native::IpcRequestPayload::Back => "back",
        adapter_native::IpcRequestPayload::Forward => "forward",
        adapter_native::IpcRequestPayload::ActivateLink { .. } => "activate_link",
        adapter_native::IpcRequestPayload::GetNavigationState => "get_navigation_state",
        adapter_native::IpcRequestPayload::GetMetrics => "get_metrics",
        adapter_native::IpcRequestPayload::GetLatestCrashReport => "get_latest_crash_report",
        adapter_native::IpcRequestPayload::NewTab => "new_tab",
        adapter_native::IpcRequestPayload::DuplicateTab { .. } => "duplicate_tab",
        adapter_native::IpcRequestPayload::SwitchTab { .. } => "switch_tab",
        adapter_native::IpcRequestPayload::CloseTab { .. } => "close_tab",
        adapter_native::IpcRequestPayload::MoveTab { .. } => "move_tab",
        adapter_native::IpcRequestPayload::SetTabPinned { .. } => "set_tab_pinned",
        adapter_native::IpcRequestPayload::SetTabMuted { .. } => "set_tab_muted",
        adapter_native::IpcRequestPayload::ListTabs => "list_tabs",
        adapter_native::IpcRequestPayload::Search { .. } => "search",
        adapter_native::IpcRequestPayload::OmniboxSuggestions { .. } => "omnibox_suggestions",
        adapter_native::IpcRequestPayload::UpdateScrollPositions { .. } => {
            "update_scroll_positions"
        }
        adapter_native::IpcRequestPayload::RegisterTlsException { .. } => "register_tls_exception",
        adapter_native::IpcRequestPayload::EnqueueDownload { .. } => "enqueue_download",
        adapter_native::IpcRequestPayload::ListDownloads => "list_downloads",
        adapter_native::IpcRequestPayload::GetDownloadProgress { .. } => "get_download_progress",
        adapter_native::IpcRequestPayload::PauseDownload { .. } => "pause_download",
        adapter_native::IpcRequestPayload::ResumeDownload { .. } => "resume_download",
        adapter_native::IpcRequestPayload::CancelDownload { .. } => "cancel_download",
        adapter_native::IpcRequestPayload::OpenDownload { .. } => "open_download",
        adapter_native::IpcRequestPayload::RevealDownload { .. } => "reveal_download",
        adapter_native::IpcRequestPayload::GetDownloadPolicySettings => {
            "get_download_policy_settings"
        }
        adapter_native::IpcRequestPayload::SetDownloadDefaultPolicy { .. } => {
            "set_download_default_policy"
        }
        adapter_native::IpcRequestPayload::SetDownloadSitePolicy { .. } => "set_download_site_policy",
        adapter_native::IpcRequestPayload::ClearDownloadSitePolicy { .. } => {
            "clear_download_site_policy"
        }
    }
}

fn sanitize_url_for_crash_report(url: &str) -> String {
    // Spec note: RFC 3986 separates query (`?`) and fragment (`#`) components from
    // the hierarchical part of a URI. Crash telemetry keeps only the stable
    // navigation target and drops those components to avoid over-retaining user
    // content such as search terms, tokens, or in-document state.
    // https://www.rfc-editor.org/rfc/rfc3986#section-3
    let without_fragment = url.split_once('#').map_or(url, |(prefix, _)| prefix);
    let without_query = without_fragment
        .split_once('?')
        .map_or(without_fragment, |(prefix, _)| prefix);
    without_query.to_string()
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
            update_scroll_positions,
            enqueue_download,
            list_downloads,
            get_download_progress,
            pause_download,
            resume_download,
            cancel_download,
            open_download,
            reveal_download,
            get_download_policy_settings,
            set_download_default_policy,
            set_download_site_policy,
            clear_download_site_policy
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
