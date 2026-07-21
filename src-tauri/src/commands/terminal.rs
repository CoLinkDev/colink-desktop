use tauri::State;
use crate::{runtime::RemoteTerminalSupport, state::AppState};

#[tauri::command]
pub fn get_remote_terminal_support(state: State<'_, AppState>, device_id: String) -> RemoteTerminalSupport {
    state.runtime.remote_terminal_support(&device_id)
}

#[tauri::command]
pub async fn open_terminal(state: State<'_, AppState>, device_id: String, cols: u16, rows: u16) -> Result<String, String> {
    state.runtime.open_remote_terminal(&device_id, cols, rows).await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn write_terminal(state: State<'_, AppState>, device_id: String, session_id: String, data: String) -> Result<(), String> {
    state.runtime.write_remote_terminal(&device_id, &session_id, data).await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn resize_terminal(state: State<'_, AppState>, device_id: String, session_id: String, cols: u16, rows: u16) -> Result<(), String> {
    state.runtime.resize_remote_terminal(&device_id, &session_id, cols, rows).await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn close_terminal(state: State<'_, AppState>, device_id: String, session_id: String) -> Result<(), String> {
    state.runtime.close_remote_terminal(&device_id, &session_id).await.map_err(|error| error.to_string())
}
