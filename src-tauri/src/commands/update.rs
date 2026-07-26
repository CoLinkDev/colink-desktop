use tauri::{AppHandle, State, WebviewWindow};

use crate::{models::AppUpdateRelease, service, state::AppState};

#[tauri::command]
pub async fn check_update(
    state: State<'_, AppState>,
    app: AppHandle,
) -> Result<Option<AppUpdateRelease>, String> {
    service::check_update(state.inner(), &app)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_update_download(url: String) -> Result<(), String> {
    service::open_update_download_url(&url).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn install_tauri_update(
    state: State<'_, AppState>,
    app: AppHandle,
    window: WebviewWindow,
) -> Result<(), String> {
    service::install_tauri_update(state.inner(), &app, &window)
        .await
        .map_err(|error| error.to_string())
}
