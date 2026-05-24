use tauri::State;

use crate::{
    models::{BootstrapPayload, LoginPayload, RegisterPayload},
    service,
    state::AppState,
};

#[tauri::command]
pub async fn login(
    state: State<'_, AppState>,
    payload: LoginPayload,
) -> Result<BootstrapPayload, String> {
    service::login(state.inner(), payload)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn register_account(
    state: State<'_, AppState>,
    payload: RegisterPayload,
) -> Result<BootstrapPayload, String> {
    service::register_account(state.inner(), payload)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn logout(state: State<'_, AppState>) -> Result<(), String> {
    service::logout(state.inner())
        .await
        .map_err(|error| error.to_string())
}
