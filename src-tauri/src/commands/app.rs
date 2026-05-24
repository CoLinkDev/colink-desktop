use tauri::State;

use crate::{models::BootstrapPayload, service, state::AppState};

#[tauri::command]
pub async fn bootstrap_app(state: State<'_, AppState>) -> Result<BootstrapPayload, String> {
    service::bootstrap(state.inner())
        .await
        .map_err(|error| error.to_string())
}
