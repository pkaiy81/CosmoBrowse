use saba_app::{BrowserSession, NavigationState, RenderSnapshot};
use std::sync::Mutex;

#[derive(Default)]
struct AppState {
    session: Mutex<BrowserSession>,
}

#[tauri::command]
fn open_url(state: tauri::State<'_, AppState>, url: String) -> Result<RenderSnapshot, String> {
    let mut session = state
        .session
        .lock()
        .map_err(|_| "Failed to lock browser session".to_string())?;
    session.open_url(&url)
}

#[tauri::command]
fn get_render_snapshot(state: tauri::State<'_, AppState>) -> Result<RenderSnapshot, String> {
    let session = state
        .session
        .lock()
        .map_err(|_| "Failed to lock browser session".to_string())?;
    Ok(session.get_render_snapshot())
}

#[tauri::command]
fn get_navigation_state(state: tauri::State<'_, AppState>) -> Result<NavigationState, String> {
    let session = state
        .session
        .lock()
        .map_err(|_| "Failed to lock browser session".to_string())?;
    Ok(session.navigation_state())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .manage(AppState::default())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            open_url,
            get_render_snapshot,
            get_navigation_state
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
