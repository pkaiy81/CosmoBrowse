use saba_app::{
    AppError, AppMetricsSnapshot, AppService, NavigationState, PageViewModel, SabaApp,
    SearchResult, TabSummary,
};
use std::sync::Mutex;

#[derive(Default)]
struct AppState {
    app: Mutex<SabaApp>,
}

#[tauri::command]
fn open_url(state: tauri::State<'_, AppState>, url: String) -> Result<PageViewModel, AppError> {
    let mut app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    app.open_url(&url)
}

#[tauri::command]
fn activate_link(
    state: tauri::State<'_, AppState>,
    frame_id: String,
    href: String,
    target: Option<String>,
) -> Result<PageViewModel, AppError> {
    let mut app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    app.activate_link(&frame_id, &href, target.as_deref())
}

#[tauri::command]
fn get_page_view(state: tauri::State<'_, AppState>) -> Result<PageViewModel, AppError> {
    let app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.get_page_view())
}

#[tauri::command]
fn set_viewport(state: tauri::State<'_, AppState>, width: i64, height: i64) -> Result<PageViewModel, AppError> {
    let mut app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    app.set_viewport(width, height)
}

#[tauri::command]
fn reload(state: tauri::State<'_, AppState>) -> Result<PageViewModel, AppError> {
    let mut app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    app.reload()
}

#[tauri::command]
fn back(state: tauri::State<'_, AppState>) -> Result<PageViewModel, AppError> {
    let mut app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    app.back()
}

#[tauri::command]
fn forward(state: tauri::State<'_, AppState>) -> Result<PageViewModel, AppError> {
    let mut app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    app.forward()
}

#[tauri::command]
fn get_navigation_state(state: tauri::State<'_, AppState>) -> Result<NavigationState, AppError> {
    let app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.get_navigation_state())
}

#[tauri::command]
fn get_metrics(state: tauri::State<'_, AppState>) -> Result<AppMetricsSnapshot, AppError> {
    let app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.get_metrics())
}

#[tauri::command]
fn new_tab(state: tauri::State<'_, AppState>) -> Result<TabSummary, AppError> {
    let mut app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.new_tab())
}

#[tauri::command]
fn switch_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<PageViewModel, AppError> {
    let mut app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    app.switch_tab(id)
}

#[tauri::command]
fn close_tab(state: tauri::State<'_, AppState>, id: u32) -> Result<Vec<TabSummary>, AppError> {
    let mut app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    app.close_tab(id)
}

#[tauri::command]
fn list_tabs(state: tauri::State<'_, AppState>) -> Result<Vec<TabSummary>, AppError> {
    let app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.list_tabs())
}

#[tauri::command]
fn search(state: tauri::State<'_, AppState>, query: String) -> Result<Vec<SearchResult>, AppError> {
    let app = state.app.lock().map_err(|_| AppError::state("Failed to lock app state"))?;
    app.search(&query)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
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
            new_tab,
            switch_tab,
            close_tab,
            list_tabs,
            search
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

