use tauri::State;

use crate::{models::AppUpdateRelease, service, state::AppState};

#[tauri::command]
pub async fn check_update(state: State<'_, AppState>) -> Result<Option<AppUpdateRelease>, String> {
    service::check_update(state.inner())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn open_update_download(url: String) -> Result<(), String> {
    service::open_update_download_url(&url).map_err(|error| error.to_string())
}
