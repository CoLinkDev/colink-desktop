use std::{path::PathBuf, process::Command};

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

#[tauri::command]
pub fn open_received_file(state: State<'_, AppState>, file_id: String) -> Result<(), String> {
    let path = completed_received_file_path(state.inner(), &file_id)?;
    open_path(&path).map_err(|error| error.to_string())
}

#[tauri::command]
pub fn reveal_received_file(state: State<'_, AppState>, file_id: String) -> Result<(), String> {
    let path = completed_received_file_path(state.inner(), &file_id)?;
    reveal_path(&path).map_err(|error| error.to_string())
}

fn completed_received_file_path(state: &AppState, file_id: &str) -> Result<PathBuf, String> {
    let record = state
        .database
        .load_transfer(file_id)
        .map_err(|error| error.to_string())?
        .ok_or_else(|| "transfer record does not exist".to_string())?;
    if record.direction != "inbound" || record.status != "completed" {
        return Err("file is not a completed received transfer".to_string());
    }
    let path = record
        .final_path
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "received file path does not exist".to_string())?;
    if !path.is_file() {
        return Err("received file does not exist".to_string());
    }
    Ok(path)
}

fn open_path(path: &PathBuf) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("rundll32")
            .arg("url.dll,FileProtocolHandler")
            .arg(path)
            .spawn()?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn()?;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(path).spawn()?;
    }

    Ok(())
}

fn reveal_path(path: &PathBuf) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("explorer")
            .arg(format!("/select,{}", path.to_string_lossy()))
            .spawn()?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg("-R").arg(path).spawn()?;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let parent = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(parent).spawn()?;
    }

    Ok(())
}
