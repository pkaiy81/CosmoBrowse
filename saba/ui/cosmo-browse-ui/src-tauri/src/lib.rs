use saba_app::{AppError, AppService, RenderSnapshot, SabaApp};
use std::sync::Mutex;

#[derive(Default)]
struct AppState {
    app: Mutex<SabaApp>,
}

#[tauri::command]
fn open_url(state: tauri::State<'_, AppState>, url: String) -> Result<RenderSnapshot, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.open_url(&url)
}

#[tauri::command]
fn get_render_snapshot(state: tauri::State<'_, AppState>) -> Result<RenderSnapshot, AppError> {
    let app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.get_render_snapshot())
}

#[tauri::command]
fn reload(state: tauri::State<'_, AppState>) -> Result<RenderSnapshot, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.reload()
}

#[tauri::command]
fn back(state: tauri::State<'_, AppState>) -> Result<RenderSnapshot, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.back()
}

#[tauri::command]
fn forward(state: tauri::State<'_, AppState>) -> Result<RenderSnapshot, AppError> {
    let mut app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    app.forward()
}

#[tauri::command]
fn get_navigation_state(
    state: tauri::State<'_, AppState>,
) -> Result<saba_app::NavigationState, AppError> {
    let app = state
        .app
        .lock()
        .map_err(|_| AppError::state("Failed to lock app state"))?;
    Ok(app.get_navigation_state())
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
            get_navigation_state
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
