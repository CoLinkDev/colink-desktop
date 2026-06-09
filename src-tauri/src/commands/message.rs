use tauri::State;

use crate::{
    models::{
        FileOfferDecisionPayload, FileOfferRequest, FileTransferRecord, SendFilePayload,
        SendTextPayload, TextMessageRecord,
    },
    state::AppState,
};

#[tauri::command]
pub async fn send_text(
    state: State<'_, AppState>,
    payload: SendTextPayload,
) -> Result<TextMessageRecord, String> {
    state
        .runtime
        .send_text(payload)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn pick_files(state: State<'_, AppState>, multiple: bool) -> Result<Vec<String>, String> {
    state
        .runtime
        .pick_file_paths(multiple)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn send_files(
    state: State<'_, AppState>,
    payload: SendFilePayload,
) -> Result<Vec<FileTransferRecord>, String> {
    state
        .runtime
        .send_files(payload)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn cancel_transfer(state: State<'_, AppState>, file_id: String) -> Result<(), String> {
    state
        .runtime
        .cancel_transfer(&file_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn clear_transfers(state: State<'_, AppState>) -> Result<(), String> {
    state
        .runtime
        .clear_transfers()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn respond_file_offer(
    state: State<'_, AppState>,
    payload: FileOfferDecisionPayload,
) -> Result<(), String> {
    state
        .runtime
        .respond_file_offer(payload)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub fn pending_file_offers(state: State<'_, AppState>) -> Vec<FileOfferRequest> {
    state.runtime.pending_file_offers()
}
