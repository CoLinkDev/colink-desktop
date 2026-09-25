use tauri::State;

use crate::notes::service::{
    self, ConflictResolutionPayload, AttachmentUploadPayload, NoteUpsertPayload, NotesStorageInfo,
    NotesSyncOutcome,
};
use crate::state::AppState;
use crate::store::notes::{NoteRecord, NoteTagRecord};

#[tauri::command]
pub async fn notes_list(state: State<'_, AppState>) -> Result<Vec<NoteRecord>, String> {
    service::list_notes(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_get(
    state: State<'_, AppState>,
    payload: NoteIdPayload,
) -> Result<Option<NoteRecord>, String> {
    service::get_note(state.inner(), &payload.id).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_upsert(
    state: State<'_, AppState>,
    payload: NoteUpsertPayload,
) -> Result<NoteRecord, String> {
    service::upsert_note(state.inner(), payload).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_delete(
    state: State<'_, AppState>,
    payload: NoteIdPayload,
) -> Result<NoteRecord, String> {
    service::delete_note_locally(state.inner(), &payload.id).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_new_id() -> Result<String, String> {
    Ok(service::new_note_id())
}

#[tauri::command]
pub async fn notes_tags_list(state: State<'_, AppState>) -> Result<Vec<NoteTagRecord>, String> {
    service::list_tags(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_tags_create(
    state: State<'_, AppState>,
    payload: TagNamePayload,
) -> Result<NoteTagRecord, String> {
    service::create_tag(state.inner(), &payload.name).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_tags_rename(
    state: State<'_, AppState>,
    payload: TagRenamePayload,
) -> Result<NoteTagRecord, String> {
    service::rename_tag(state.inner(), &payload.id, &payload.name).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_tags_delete(
    state: State<'_, AppState>,
    payload: NoteIdPayload,
) -> Result<(), String> {
    service::delete_tag_locally(state.inner(), &payload.id).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_attachments_stage(
    state: State<'_, AppState>,
    payload: AttachmentUploadPayload,
) -> Result<crate::store::notes::NoteAttachmentRecord, String> {
    service::stage_attachment(state.inner(), payload).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_attachments_list(
    state: State<'_, AppState>,
) -> Result<Vec<crate::store::notes::NoteAttachmentRecord>, String> {
    service::list_attachments(state.inner()).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_attachments_delete(
    state: State<'_, AppState>,
    payload: NoteIdPayload,
) -> Result<(), String> {
    service::remove_attachment(state.inner(), &payload.id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_attachments_resolve_path(
    state: State<'_, AppState>,
    payload: NoteIdPayload,
) -> Result<String, String> {
    service::resolve_attachment_path(state.inner(), &payload.id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_attachments_open(
    state: State<'_, AppState>,
    payload: NoteIdPayload,
) -> Result<(), String> {
    let path = service::resolve_attachment_open_path(state.inner(), &payload.id)
        .await
        .map_err(|error| error.to_string())?;
    crate::commands::message::open_path(&path).map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_sync(state: State<'_, AppState>) -> Result<NotesSyncOutcome, String> {
    Ok(service::sync(state.inner()).await)
}

#[tauri::command]
pub async fn notes_resolve_conflict(
    state: State<'_, AppState>,
    payload: ConflictResolutionPayload,
) -> Result<NoteRecord, String> {
    service::resolve_conflict(state.inner(), payload)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn notes_storage(state: State<'_, AppState>) -> Result<NotesStorageInfo, String> {
    service::fetch_storage(state.inner())
        .await
        .map_err(|error| error.to_string())
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteIdPayload {
    pub id: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagNamePayload {
    pub name: String,
}

#[derive(Debug, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TagRenamePayload {
    pub id: String,
    pub name: String,
}
