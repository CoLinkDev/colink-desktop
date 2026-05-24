use tauri::State;

use crate::{models::AppSettings, service, state::AppState};

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<AppSettings, String> {
    service::get_settings(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    settings: AppSettings,
) -> Result<AppSettings, String> {
    service::update_settings(state.inner(), settings).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn pick_download_directory(state: State<'_, AppState>) -> Result<Option<String>, String> {
    state
        .runtime
        .pick_folder_path()
        .map_err(|error| error.to_string())
}
