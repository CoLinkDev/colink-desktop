use tauri::State;

use crate::{
    models::{RemoteFilesystemDownload, RemoteFilesystemDownloadPayload, RemoteFilesystemListPayload},
    protocol::{FsListResultPayload, FsRootsResultPayload},
    state::AppState,
};

#[tauri::command]
pub async fn list_remote_filesystem_roots(
    state: State<'_, AppState>,
    device_id: String,
) -> Result<FsRootsResultPayload, String> {
    state
        .runtime
        .list_remote_filesystem_roots(&device_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn list_remote_filesystem(
    state: State<'_, AppState>,
    payload: RemoteFilesystemListPayload,
) -> Result<FsListResultPayload, String> {
    state
        .runtime
        .list_remote_filesystem(&payload.device_id, payload.path, payload.offset)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn download_remote_filesystem_file(
    state: State<'_, AppState>,
    payload: RemoteFilesystemDownloadPayload,
) -> Result<RemoteFilesystemDownload, String> {
    state
        .runtime
        .download_remote_filesystem_file(&payload.device_id, payload.path)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn list_remote_filesystem_downloads(
    state: State<'_, AppState>,
) -> Vec<RemoteFilesystemDownload> {
    state.runtime.remote_filesystem_downloads()
}
