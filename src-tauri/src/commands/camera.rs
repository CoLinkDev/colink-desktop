use tauri::State;

use crate::{protocol::CameraEntry, runtime::RemoteCameraSupport, state::AppState};

#[tauri::command]
pub fn get_remote_camera_support(state: State<'_, AppState>, device_id: String) -> RemoteCameraSupport {
    state.runtime.remote_camera_support(&device_id)
}

#[tauri::command]
pub async fn list_remote_cameras(state: State<'_, AppState>, device_id: String) -> Result<Vec<CameraEntry>, String> {
    state.runtime.list_remote_cameras(&device_id).await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn open_remote_camera(state: State<'_, AppState>, device_id: String, camera_id: String, preferred_codecs: Vec<String>) -> Result<String, String> {
    state.runtime.open_remote_camera(&device_id, camera_id, preferred_codecs).await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn send_camera_alive(state: State<'_, AppState>, device_id: String, session_id: String) -> Result<(), String> {
    state.runtime.send_camera_alive(&device_id, &session_id).await.map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn close_remote_camera(state: State<'_, AppState>, device_id: String, session_id: String) -> Result<(), String> {
    state.runtime.close_remote_camera(&device_id, &session_id).await.map_err(|error| error.to_string())
}
