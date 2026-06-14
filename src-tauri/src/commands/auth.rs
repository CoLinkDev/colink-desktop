use tauri::State;

use crate::{
    models::{BootstrapPayload, LoginPayload, RegisterPayload, SavedLoginCredentials},
    service,
    state::AppState,
};

const SAVED_LOGIN_SERVICE: &str = "dev.colink.desktop";
const SAVED_LOGIN_ACCOUNT: &str = "saved-login";

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

#[tauri::command]
pub fn get_saved_login() -> Result<Option<SavedLoginCredentials>, String> {
    let entry = keyring::Entry::new(SAVED_LOGIN_SERVICE, SAVED_LOGIN_ACCOUNT)
        .map_err(|error| error.to_string())?;

    match entry.get_password() {
        Ok(value) => serde_json::from_str(&value)
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(keyring::Error::NoEntry) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[tauri::command]
pub fn save_saved_login(payload: SavedLoginCredentials) -> Result<(), String> {
    let entry = keyring::Entry::new(SAVED_LOGIN_SERVICE, SAVED_LOGIN_ACCOUNT)
        .map_err(|error| error.to_string())?;
    let value = serde_json::to_string(&payload).map_err(|error| error.to_string())?;

    entry
        .set_password(&value)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clear_saved_login() -> Result<(), String> {
    let entry = keyring::Entry::new(SAVED_LOGIN_SERVICE, SAVED_LOGIN_ACCOUNT)
        .map_err(|error| error.to_string())?;

    match entry.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}
