use saba_app::{
    AppError, AppMetricsSnapshot, AppResult, AppService, NavigationState, RenderSnapshot, SabaApp,
    SearchResult, TabSummary,
};
use std::sync::Mutex;

#[derive(Default)]
struct AppState {
    app: Mutex<SabaApp>,
}

fn with_app<T, F>(
    state: tauri::State<'_, AppState>,
    command: &'static str,
    action: F,
) -> AppResult<T>
where
    F: FnOnce(&SabaApp) -> AppResult<T>,
{
    let app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    let result = action(&app);
    log_command(command, &result);
    result
}

fn with_app_mut<T, F>(
    state: tauri::State<'_, AppState>,
    command: &'static str,
    action: F,
) -> AppResult<T>
where
    F: FnOnce(&mut SabaApp) -> AppResult<T>,
{
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    let result = action(&mut app);
    log_command(command, &result);
    result
}

fn log_command<T>(command: &str, result: &AppResult<T>) {
    match result {
        Ok(_) => eprintln!("[adapter_tauri] {command} ok"),
        Err(error) => eprintln!(
            "[adapter_tauri] {command} failed: {} ({})",
            error.code, error.message
        ),
    }
}

#[tauri::command]
fn open_url(state: tauri::State<'_, AppState>, url: String) -> Result<RenderSnapshot, AppError> {
    with_app_mut(state, "open_url", |app| app.open_url(&url))
}

#[tauri::command]
fn get_render_snapshot(state: tauri::State<'_, AppState>) -> Result<RenderSnapshot, AppError> {
    with_app(state, "get_render_snapshot", |app| {
        Ok(app.get_render_snapshot())
    })
}

#[tauri::command]
fn reload(state: tauri::State<'_, AppState>) -> Result<RenderSnapshot, AppError> {
    with_app_mut(state, "reload", AppService::reload)
}

#[tauri::command]
fn back(state: tauri::State<'_, AppState>) -> Result<RenderSnapshot, AppError> {
    with_app_mut(state, "back", AppService::back)
}

#[tauri::command]
fn forward(state: tauri::State<'_, AppState>) -> Result<RenderSnapshot, AppError> {
    with_app_mut(state, "forward", AppService::forward)
}

#[tauri::command]
fn get_navigation_state(state: tauri::State<'_, AppState>) -> Result<NavigationState, AppError> {
    with_app(state, "get_navigation_state", |app| {
        Ok(app.get_navigation_state())
    })
}

#[tauri::command]
fn get_metrics(state: tauri::State<'_, AppState>) -> Result<AppMetricsSnapshot, AppError> {
    with_app(state, "get_metrics", |app| Ok(app.get_metrics()))
}

#[tauri::command]
fn new_tab(state: tauri::State<'_, AppState>) -> Result<TabSummary, AppError> {
    with_app_mut(state, "new_tab", |app| Ok(app.new_tab()))
}

#[tauri::command]
fn switch_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<RenderSnapshot, AppError> {
    with_app_mut(state, "switch_tab", |app| app.switch_tab(id))
}

#[tauri::command]
fn close_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<Vec<TabSummary>, AppError> {
    with_app_mut(state, "close_tab", |app| app.close_tab(id))
}

#[tauri::command]
fn list_tabs(state: tauri::State<'_, AppState>) -> Result<Vec<TabSummary>, AppError> {
    with_app(state, "list_tabs", |app| Ok(app.list_tabs()))
}

#[tauri::command]
fn search(state: tauri::State<'_, AppState>, query: String) -> Result<Vec<SearchResult>, AppError> {
    with_app(state, "search", |app| app.search(&query))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            open_url,
            get_render_snapshot,
            reload,
            back,
            forward,
            get_navigation_state,
            get_metrics,
            new_tab,
            switch_tab,
            close_tab,
            list_tabs,
            search
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
