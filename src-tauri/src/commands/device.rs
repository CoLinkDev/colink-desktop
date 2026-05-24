use tauri::State;

use crate::{models::DeviceInfo, service, state::AppState};

#[tauri::command]
pub async fn list_devices(state: State<'_, AppState>) -> Result<Vec<DeviceInfo>, String> {
    service::list_devices(state.inner())
        .await
        .map_err(|error| error.to_string())
}
