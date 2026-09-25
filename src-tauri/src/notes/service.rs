use std::collections::HashSet;
use std::io::{Read, Write};
use std::path::PathBuf;

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tokio::io::AsyncWriteExt;
use url::Url;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::notes::api::{
    sha256_hex, AttachmentDto, ChangesDto, CreateNoteRequest, NoteDeleteDto, NoteDto,
    NotesHttpClient, SnapshotDto, StorageDto, TagDeleteDto, TagDto, TagListDto,
    UpdateNoteRequest, CODE_ATTACHMENT_ID_UNAVAILABLE, CODE_ATTACHMENT_NOT_FOUND,
    CODE_INVALID_NOTE_REFERENCE, CODE_NOTE_NOT_FOUND, CODE_REVISION_CONFLICT,
    CODE_SYNC_CURSOR_EXPIRED, CODE_TAG_NOT_FOUND, NOTE_ATTACHMENTS_PATH, NOTES_CHANGES_PATH,
    NOTES_PATH, NOTES_SNAPSHOT_PATH, NOTES_STORAGE_PATH, NOTE_TAGS_PATH,
};
use crate::notes::merge::{merge_field, merge_markdown, merge_set, FieldMerge};
use crate::state::AppState;
use crate::store::db::Database;
use crate::store::notes::{
    normalize_tag_name, now_millis, NoteAttachmentRecord, NoteRecord, NoteTagRecord,
    CONFLICT_KIND_CLOUD_DELETED, CONFLICT_KIND_DELETE, CONFLICT_KIND_EDIT, NOTE_STATE_CONFLICT,
    NOTE_STATE_CONFLICT_DELETE, NotesTransaction, NOTE_STATE_PENDING, NOTE_STATE_PENDING_DELETE,
    NOTE_STATE_SYNCED, LOCAL_ACCOUNT_SCOPE,
};

pub(crate) const NOTES_UPDATED_EVENT: &str = "notes-updated";

const SYNC_PAGE_LIMIT: u32 = 200;
const MAX_ATTACHMENT_REKEY_ATTEMPTS: usize = 3;

const CODE_TAG_NAME_CONFLICT: i32 = 6004;
const CODE_ATTACHMENT_IN_USE: i32 = 6006;

// ---------------------------------------------------------------------------
// Public API used by commands
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesSyncOutcome {
    pub status: String,
    pub message: Option<String>,
    pub pushed_notes: u32,
    pub pushed_tags: u32,
    pub pushed_attachments: u32,
    pub pulled_notes: u32,
    pub pulled_tags: u32,
    pub conflicts: u32,
    pub repaired_references: u32,
}

impl NotesSyncOutcome {
    fn empty(status: &str) -> Self {
        Self {
            status: status.to_string(),
            message: None,
            pushed_notes: 0,
            pushed_tags: 0,
            pushed_attachments: 0,
            pulled_notes: 0,
            pulled_tags: 0,
            conflicts: 0,
            repaired_references: 0,
        }
    }

    fn offline() -> Self {
        Self::empty("offline")
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NoteUpsertPayload {
    pub id: Option<String>,
    pub title: String,
    pub markdown: String,
    pub tag_ids: Vec<String>,
    pub attachment_ids: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConflictResolutionPayload {
    pub note_id: String,
    pub resolution: String,
    pub title: Option<String>,
    pub markdown: Option<String>,
    pub tag_ids: Option<Vec<String>>,
    pub attachment_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AttachmentUploadPayload {
    pub path: String,
    pub kind: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NotesStorageInfo {
    pub used_bytes: i64,
    pub limit_bytes: i64,
    pub remaining_bytes: i64,
    pub attachment_bytes: i64,
    pub markdown_bytes: i64,
    pub max_attachment_bytes: i64,
    pub max_markdown_bytes: i64,
}

pub fn list_notes(state: &AppState) -> AppResult<Vec<NoteRecord>> {
    state.database.load_notes()
}

pub fn get_note(state: &AppState, id: &str) -> AppResult<Option<NoteRecord>> {
    state.database.load_note(id)
}

pub fn list_tags(state: &AppState) -> AppResult<Vec<NoteTagRecord>> {
    state.database.load_note_tags()
}

pub fn list_attachments(state: &AppState) -> AppResult<Vec<NoteAttachmentRecord>> {
    state.database.load_note_attachments()
}

pub fn new_note_id() -> String {
    Uuid::new_v4().to_string()
}

pub fn claim_local_data(state: &AppState, target_scope: &str) -> AppResult<()> {
    transfer_scope_data(
        &state.app,
        &state.database,
        LOCAL_ACCOUNT_SCOPE,
        target_scope,
    )
}

pub(crate) fn release_account_data(
    app: &AppHandle,
    database: &Database,
    source_scope: &str,
) -> AppResult<()> {
    transfer_scope_data(app, database, source_scope, LOCAL_ACCOUNT_SCOPE)
}

pub(crate) fn release_current_account_data(
    app: &AppHandle,
    database: &Database,
) -> AppResult<()> {
    let Some(session) = database.load_session()? else {
        return Ok(());
    };
    let settings = database
        .load_settings()?
        .ok_or_else(|| AppError::message("application settings are missing"))?;
    let source_scope = crate::store::notes::account_notes_scope(&settings, &session);
    release_account_data(app, database, &source_scope)
}

fn transfer_scope_data(
    app: &AppHandle,
    database: &Database,
    source_scope: &str,
    target_scope: &str,
) -> AppResult<()> {
    let app_dir = crate::state::app_data_dir(app)?;
    let mut staged_targets = Vec::new();
    let transfer_result = database.transfer_notes_scope_with(
        source_scope,
        target_scope,
        |claimed_attachments| {
            for claimed in claimed_attachments {
                let Some(source) = attachment_transfer_source(
                    &app_dir,
                    source_scope,
                    &claimed.source_id,
                ) else {
                    continue;
                };
                let target =
                    attachment_cache_path_for_scope(&app_dir, target_scope, &claimed.target_id);
                if target.is_file() {
                    continue;
                }
                staged_targets.push(target.clone());
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::copy(source, target)?;
            }
            Ok(())
        },
    );
    let claimed_attachments = match transfer_result {
        Ok(claimed) => claimed,
        Err(error) => {
            for target in staged_targets {
                let _ = std::fs::remove_file(target);
            }
            return Err(error);
        }
    };

    for claimed in claimed_attachments {
        if let Some(source) =
            attachment_transfer_source(&app_dir, source_scope, &claimed.source_id)
        {
            if let Err(error) = std::fs::remove_file(source) {
                tracing::warn!(%error, "failed to remove transferred notes attachment source");
            }
        }
    }
    notify_notes_changed(app);
    Ok(())
}

fn attachment_transfer_source(
    app_dir: &std::path::Path,
    source_scope: &str,
    source_id: &str,
) -> Option<PathBuf> {
    let scoped_source = attachment_cache_path_for_scope(app_dir, source_scope, source_id);
    if scoped_source.is_file() {
        return Some(scoped_source);
    }
    let legacy_source = app_dir.join("notes-attachments").join(source_id);
    legacy_source.is_file().then_some(legacy_source)
}

/// Creates or updates a note locally. Writes are offline-first: they land in
/// SQLite immediately and enter the upload queue via the pending state.
pub fn upsert_note(state: &AppState, payload: NoteUpsertPayload) -> AppResult<NoteRecord> {
    for id in payload.tag_ids.iter().chain(payload.attachment_ids.iter()) {
        if Uuid::parse_str(id).is_err() {
            return Err(AppError::message("invalid reference id"));
        }
    }

    let now = now_millis();
    let id = payload.id.unwrap_or_else(|| Uuid::new_v4().to_string());

    let mut record = state
        .database
        .load_note(&id)?
        .unwrap_or_else(|| NoteRecord::new(id.clone(), now));

    if record.sync_state == NOTE_STATE_CONFLICT || record.sync_state == NOTE_STATE_CONFLICT_DELETE {
        return Err(AppError::message(
            "resolve the conflict before editing this note",
        ));
    }

    record.title = payload.title;
    record.markdown = payload.markdown;
    record.tag_ids = normalize_id_list(payload.tag_ids);
    record.attachment_ids = normalize_id_list(payload.attachment_ids);
    record.sync_state = NOTE_STATE_PENDING.to_string();
    record.updated_at = now;

    state.database.save_note(&record)?;
    notify_notes_changed(&state.app);
    Ok(record)
}

/// Deletes a note locally. Notes that never reached the server are dropped
/// immediately; everything else enters the pending-delete queue.
pub fn delete_note_locally(state: &AppState, id: &str) -> AppResult<NoteRecord> {
    let record = state
        .database
        .load_note(id)?
        .ok_or_else(|| AppError::message("note not found"))?;

    if record.revision == 0 {
        state.database.delete_note_row(id)?;
        notify_notes_changed(&state.app);
        return Ok(record);
    }

    let mut record = record;
    record.sync_state = NOTE_STATE_PENDING_DELETE.to_string();
    record.updated_at = now_millis();
    state.database.save_note(&record)?;
    notify_notes_changed(&state.app);
    Ok(record)
}

pub fn create_tag(state: &AppState, name: &str) -> AppResult<NoteTagRecord> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::message("tag name must not be empty"));
    }
    if let Some(existing) = state.database.find_note_tag_by_name(trimmed)? {
        return Ok(existing);
    }

    let now = now_millis();
    let record = NoteTagRecord {
        id: Uuid::new_v4().to_string(),
        name: trimmed.to_string(),
        revision: 0,
        base_revision: 0,
        sync_state: NOTE_STATE_PENDING.to_string(),
        deleted: false,
        created_at: now,
        updated_at: now,
    };
    state.database.save_note_tag(&record)?;
    notify_notes_changed(&state.app);
    Ok(record)
}

pub fn rename_tag(state: &AppState, id: &str, name: &str) -> AppResult<NoteTagRecord> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(AppError::message("tag name must not be empty"));
    }

    let mut record = state
        .database
        .load_note_tag(id)?
        .ok_or_else(|| AppError::message("tag not found"))?;

    if let Some(existing) = state.database.find_note_tag_by_name(trimmed)? {
        if existing.id != id {
            return Err(AppError::message("tag name already exists"));
        }
    }

    record.name = trimmed.to_string();
    record.sync_state = NOTE_STATE_PENDING.to_string();
    record.updated_at = now_millis();
    state.database.save_note_tag(&record)?;
    notify_notes_changed(&state.app);
    Ok(record)
}

pub fn delete_tag_locally(state: &AppState, id: &str) -> AppResult<()> {
    let record = state
        .database
        .load_note_tag(id)?
        .ok_or_else(|| AppError::message("tag not found"))?;

    if record.revision == 0 {
        state.database.with_notes_transaction(|store| {
            remove_tag_everywhere(store, id)?;
            store.delete_note_tag_row(id)
        })?;
    } else {
        let mut record = record;
        record.sync_state = NOTE_STATE_PENDING_DELETE.to_string();
        record.updated_at = now_millis();
        state.database.save_note_tag(&record)?;
    }

    notify_notes_changed(&state.app);
    Ok(())
}

/// Stages an attachment for upload from a picked file path. The content is
/// copied into the local cache so later edits of the source file cannot
/// corrupt the queued upload.
pub fn stage_attachment(
    state: &AppState,
    payload: AttachmentUploadPayload,
) -> AppResult<NoteAttachmentRecord> {
    if payload.kind != "image" && payload.kind != "file" {
        return Err(AppError::message("invalid attachment kind"));
    }

    let file_name = std::path::Path::new(&payload.path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "attachment".to_string());

    let id = Uuid::new_v4().to_string();
    let cache_path = attachment_cache_path(state, &id)?;
    if let Some(parent) = cache_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temp_path = cache_path.with_extension("part");
    let mut source = std::fs::File::open(&payload.path)?;
    let mut target = std::fs::File::create(&temp_path)?;
    let mut hasher = Sha256::new();
    let mut size = 0_i64;
    let mut buffer = [0_u8; 64 * 1024];
    let copy_result = (|| -> AppResult<()> {
        loop {
            let read = source.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            target.write_all(&buffer[..read])?;
            hasher.update(&buffer[..read]);
            size += read as i64;
        }
        target.flush()?;
        Ok(())
    })();
    if let Err(error) = copy_result {
        let _ = std::fs::remove_file(&temp_path);
        return Err(error);
    }
    std::fs::rename(&temp_path, &cache_path)?;

    let now = now_millis();
    let record = NoteAttachmentRecord {
        id,
        kind: payload.kind,
        file_name,
        media_type: String::new(),
        size,
        sha256: format!("{:x}", hasher.finalize()),
        sync_state: NOTE_STATE_PENDING.to_string(),
        deleted: false,
        created_at: now,
    };
    if let Err(error) = state.database.save_note_attachment(&record) {
        let _ = std::fs::remove_file(&cache_path);
        return Err(error);
    }

    notify_notes_changed(&state.app);
    Ok(record)
}

/// Removes an attachment locally and, when it was already uploaded, on the
/// server as well.
pub async fn remove_attachment(state: &AppState, id: &str) -> AppResult<()> {
    let record = state
        .database
        .load_note_attachment(id)?
        .ok_or_else(|| AppError::message("attachment not found"))?;

    if record.sync_state != NOTE_STATE_PENDING && state.database.load_session()?.is_some() {
        let settings = load_settings(state)?;
        if let Some(session) = current_session_opt(state, &settings).await {
            let http = NotesHttpClient::new()?;
            let path = format!("{NOTE_ATTACHMENTS_PATH}/{id}");
            if let Err(error) = http
                .delete_ok(&settings.server_url, &path, &session.access_token)
                .await
            {
                let protocol_in_use = matches!(
                    &error,
                    AppError::Protocol { code, .. } if *code == CODE_ATTACHMENT_IN_USE
                );
                if !protocol_in_use {
                    return Err(error);
                }
                return Err(AppError::message(
                    "attachment is still referenced by a note",
                ));
            }
        }
    }

    state.database.delete_note_attachment_row(id)?;
    let cache_path = attachment_cache_path(state, id)?;
    let _ = std::fs::remove_file(cache_path);
    notify_notes_changed(&state.app);
    Ok(())
}

pub async fn fetch_storage(state: &AppState) -> AppResult<NotesStorageInfo> {
    let settings = load_settings(state)?;
    let session = current_session(state, &settings).await?;
    let http = NotesHttpClient::new()?;
    let dto: StorageDto = http
        .get(&settings.server_url, NOTES_STORAGE_PATH, &session.access_token)
        .await?;

    Ok(NotesStorageInfo {
        used_bytes: dto.used_bytes,
        limit_bytes: dto.limit_bytes,
        remaining_bytes: dto.remaining_bytes,
        attachment_bytes: dto.attachment_bytes,
        markdown_bytes: dto.markdown_bytes,
        max_attachment_bytes: dto.max_attachment_bytes,
        max_markdown_bytes: dto.max_markdown_bytes,
    })
}

/// Returns the local cache path of an attachment content, downloading it on
/// demand with sha256 integrity verification.
pub async fn resolve_attachment_path(state: &AppState, id: &str) -> AppResult<String> {
    let metadata = state
        .database
        .load_note_attachment(id)?
        .ok_or_else(|| AppError::message("attachment not found"))?;
    if metadata.deleted {
        return Err(AppError::message("attachment not found"));
    }

    let path = attachment_cache_path(state, id)?;
    let cached_is_valid = if path.is_file() {
        let metadata_matches = metadata.size < 0 || path.metadata()?.len() == metadata.size as u64;
        metadata_matches && (metadata.sha256.is_empty() || sha256_file(&path)? == metadata.sha256)
    } else {
        false
    };
    if !cached_is_valid {
        if path.exists() {
            std::fs::remove_file(&path)?;
        }
        if state.database.load_session()?.is_none() {
            return Err(AppError::message("attachment not found"));
        }
        let settings = load_settings(state)?;
        let session = current_session(state, &settings).await?;
        let http = NotesHttpClient::new()?;
        let content_path = format!("{NOTE_ATTACHMENTS_PATH}/{id}/content");
        let response = match http
            .download_attachment(
                &settings.server_url,
                &content_path,
                &session.access_token,
                None,
            )
            .await?
        {
            Some(response) => response,
            None => {
                // 304 cannot happen without a cached body; treat it as a
                // missing-attachment condition.
                return Err(AppError::message("attachment not found"));
            }
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp_path = path.with_extension("part");
        let mut output = tokio::fs::File::create(&temp_path).await?;
        let mut stream = response.bytes_stream();
        let mut hasher = Sha256::new();
        let mut size = 0_i64;
        let download_result = async {
            while let Some(chunk) = stream.next().await {
                let chunk = chunk?;
                output.write_all(&chunk).await?;
                hasher.update(&chunk);
                size += chunk.len() as i64;
            }
            output.flush().await?;
            Ok::<(), AppError>(())
        }
        .await;
        drop(output);
        if let Err(error) = download_result {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(error);
        }
        let digest = format!("{:x}", hasher.finalize());
        if (metadata.size >= 0 && size != metadata.size)
            || (!metadata.sha256.is_empty() && digest != metadata.sha256)
        {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(AppError::message("attachment integrity check failed"));
        }
        std::fs::rename(&temp_path, &path)?;
    }

    Ok(path.to_string_lossy().to_string())
}

pub async fn resolve_attachment_open_path(state: &AppState, id: &str) -> AppResult<PathBuf> {
    let source = PathBuf::from(resolve_attachment_path(state, id).await?);
    let metadata = state
        .database
        .load_note_attachment(id)?
        .ok_or_else(|| AppError::message("attachment not found"))?;
    let file_name = std::path::Path::new(&metadata.file_name)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "attachment".to_string());
    let open_dir = source
        .parent()
        .ok_or_else(|| AppError::message("attachment cache path is invalid"))?
        .join("open");
    std::fs::create_dir_all(&open_dir)?;
    let target = open_dir.join(format!("{id}-{file_name}"));
    if !target.is_file() || target.metadata()?.len() != source.metadata()?.len() {
        if target.exists() {
            std::fs::remove_file(&target)?;
        }
        if std::fs::hard_link(&source, &target).is_err() {
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(target)
}

// ---------------------------------------------------------------------------
// Synchronization engine
// ---------------------------------------------------------------------------

pub async fn sync(state: &AppState) -> NotesSyncOutcome {
    let outcome = run_sync(state).await.unwrap_or_else(|error| {
        tracing::warn!(%error, "notes sync aborted");
        NotesSyncOutcome {
            status: "error".to_string(),
            message: Some(error.to_string()),
            ..NotesSyncOutcome::empty("error")
        }
    });
    let _ = state.app.emit(NOTES_UPDATED_EVENT, &outcome);
    outcome
}

async fn run_sync(state: &AppState) -> AppResult<NotesSyncOutcome> {
    let settings = load_settings(state)?;
    let Some(session) = current_session_opt(state, &settings).await else {
        return Ok(NotesSyncOutcome::offline());
    };

    let mut context = SyncContext {
        http: NotesHttpClient::new()?,
        base_url: settings.server_url.clone(),
        token: session.access_token.clone(),
        pushed_notes: 0,
        pushed_tags: 0,
        pushed_attachments: 0,
        pulled_notes: 0,
        pulled_tags: 0,
        conflicts: 0,
        repaired_references: 0,
    };

    push_attachments(state, &mut context).await?;
    push_tags(state, &mut context).await?;
    push_notes(state, &mut context).await?;

    match state.database.load_notes_sync_cursor()? {
        Some(cursor) => match pull_incremental(state, &mut context, cursor).await {
            Ok(()) => {}
            Err(AppError::Protocol { code, .. }) if code == CODE_SYNC_CURSOR_EXPIRED => {
                state.database.clear_notes_sync_cursor()?;
                pull_snapshot(state, &mut context).await?;
            }
            Err(error) => return Err(error),
        },
        None => pull_snapshot(state, &mut context).await?,
    }

    // A second push pass uploads notes whose base revision advanced during
    // the pull (clean three-way merges).
    push_attachments(state, &mut context).await?;
    push_tags(state, &mut context).await?;
    push_notes(state, &mut context).await?;

    Ok(outcome_from(&context))
}

fn outcome_from(context: &SyncContext) -> NotesSyncOutcome {
    NotesSyncOutcome {
        status: "ok".to_string(),
        message: None,
        pushed_notes: context.pushed_notes,
        pushed_tags: context.pushed_tags,
        pushed_attachments: context.pushed_attachments,
        pulled_notes: context.pulled_notes,
        pulled_tags: context.pulled_tags,
        conflicts: context.conflicts,
        repaired_references: context.repaired_references,
    }
}

struct SyncContext {
    http: NotesHttpClient,
    base_url: String,
    token: String,
    pushed_notes: u32,
    pushed_tags: u32,
    pushed_attachments: u32,
    pulled_notes: u32,
    pulled_tags: u32,
    conflicts: u32,
    repaired_references: u32,
}

impl SyncContext {
    async fn get<T: serde::de::DeserializeOwned>(&self, path: &str) -> AppResult<T> {
        self.http.get(&self.base_url, path, &self.token).await
    }

    fn count_conflict(&mut self) {
        self.conflicts += 1;
    }
}

fn url_query(base_url: &str, path: &str, query: &[(&str, &str)]) -> AppResult<String> {
    let mut url = Url::parse(base_url)?;
    url.set_path(path.trim_start_matches('/'));
    for (key, value) in query {
        url.query_pairs_mut().append_pair(key, value);
    }
    Ok(url.to_string())
}

async fn push_attachments(state: &AppState, context: &mut SyncContext) -> AppResult<()> {
    let records = state.database.load_note_attachments()?;
    for record in records {
        if (record.sync_state != NOTE_STATE_PENDING && record.sync_state != NOTE_STATE_CONFLICT)
            || record.deleted
        {
            continue;
        }
        let _notify_on_completion = NotesUpdateOnDrop(&state.app);
        let cache_path = attachment_cache_path(state, &record.id)?;
        if !cache_path.exists() {
            remove_current_attachment_references(state, &record.id)?;
            state.database.delete_note_attachment_row(&record.id)?;
            context.repaired_references += 1;
            continue;
        }
        push_attachment_with_rekeys(state, context, record).await?;
    }

    Ok(())
}

async fn push_attachment_with_rekeys(
    state: &AppState,
    context: &mut SyncContext,
    mut record: NoteAttachmentRecord,
) -> AppResult<()> {
    let mut rekey_attempts = 0;
    loop {
        if record.sync_state == NOTE_STATE_CONFLICT {
            if rekey_attempts >= MAX_ATTACHMENT_REKEY_ATTEMPTS {
                return Err(AppError::message(format!(
                    "attachment ID unavailable after {MAX_ATTACHMENT_REKEY_ATTEMPTS} replacement attempts"
                )));
            }
            record = rekey_attachment(state, record)?;
            rekey_attempts += 1;
        }

        match upload_attachment_record(state, context, &record).await {
            Ok(()) => return Ok(()),
            Err(AppError::Protocol { code, .. }) if code == CODE_ATTACHMENT_ID_UNAVAILABLE => {
                let path = format!("{NOTE_ATTACHMENTS_PATH}/{}", record.id);
                let should_rekey = match context.get::<AttachmentDto>(&path).await {
                    Ok(existing)
                        if existing.kind == record.kind
                            && existing.size == record.size
                            && existing.sha256 == record.sha256 =>
                    {
                        let mut updated = record;
                        updated.sync_state = NOTE_STATE_SYNCED.to_string();
                        updated.media_type = existing.media_type;
                        context.pushed_attachments += 1;
                        state.database.save_note_attachment(&updated)?;
                        return Ok(());
                    }
                    Ok(_) => true,
                    Err(AppError::Protocol { code, .. })
                        if code == CODE_ATTACHMENT_NOT_FOUND =>
                    {
                        true
                    }
                    Err(error) => return Err(error),
                };

                if should_rekey {
                    if rekey_attempts >= MAX_ATTACHMENT_REKEY_ATTEMPTS {
                        return Err(AppError::message(format!(
                            "attachment ID unavailable after {MAX_ATTACHMENT_REKEY_ATTEMPTS} replacement attempts"
                        )));
                    }
                    record = rekey_attachment(state, record)?;
                    rekey_attempts += 1;
                }
            }
            Err(error) => return Err(error),
        }
    }
}

trait NotesStore {
    fn load_note(&self, id: &str) -> AppResult<Option<NoteRecord>>;
    fn load_all_note_rows(&self) -> AppResult<Vec<NoteRecord>>;
    fn save_note(&self, record: &NoteRecord) -> AppResult<()>;
    fn delete_note_row(&self, id: &str) -> AppResult<()>;
    fn load_note_tag(&self, id: &str) -> AppResult<Option<NoteTagRecord>>;
    fn load_all_note_tag_rows(&self) -> AppResult<Vec<NoteTagRecord>>;
    fn save_note_tag(&self, record: &NoteTagRecord) -> AppResult<()>;
    fn delete_note_tag_row(&self, id: &str) -> AppResult<()>;
    fn load_note_attachment(&self, id: &str) -> AppResult<Option<NoteAttachmentRecord>>;
    fn save_note_attachment(&self, record: &NoteAttachmentRecord) -> AppResult<()>;
}

macro_rules! impl_notes_store {
    ($type:ty) => {
        impl NotesStore for $type {
            fn load_note(&self, id: &str) -> AppResult<Option<NoteRecord>> { self.load_note(id) }
            fn load_all_note_rows(&self) -> AppResult<Vec<NoteRecord>> { self.load_all_note_rows() }
            fn save_note(&self, record: &NoteRecord) -> AppResult<()> { self.save_note(record) }
            fn delete_note_row(&self, id: &str) -> AppResult<()> { self.delete_note_row(id) }
            fn load_note_tag(&self, id: &str) -> AppResult<Option<NoteTagRecord>> { self.load_note_tag(id) }
            fn load_all_note_tag_rows(&self) -> AppResult<Vec<NoteTagRecord>> { self.load_all_note_tag_rows() }
            fn save_note_tag(&self, record: &NoteTagRecord) -> AppResult<()> { self.save_note_tag(record) }
            fn delete_note_tag_row(&self, id: &str) -> AppResult<()> { self.delete_note_tag_row(id) }
            fn load_note_attachment(&self, id: &str) -> AppResult<Option<NoteAttachmentRecord>> { self.load_note_attachment(id) }
            fn save_note_attachment(&self, record: &NoteAttachmentRecord) -> AppResult<()> { self.save_note_attachment(record) }
        }
    };
}

impl_notes_store!(Database);
impl_notes_store!(NotesTransaction<'_>);

async fn upload_attachment_record(
    state: &AppState,
    context: &mut SyncContext,
    record: &NoteAttachmentRecord,
) -> AppResult<()> {
    let cache_path = attachment_cache_path(state, &record.id)?;
    let dto = context
        .http
        .upload_attachment(
            &context.base_url,
            NOTE_ATTACHMENTS_PATH,
            &context.token,
            &record.id,
            &record.kind,
            &record.sha256,
            &record.file_name,
            &cache_path,
            record.size.max(0) as u64,
        )
        .await?;
    let mut updated = record.clone();
    updated.sync_state = NOTE_STATE_SYNCED.to_string();
    updated.media_type = dto.media_type;
    state.database.save_note_attachment(&updated)?;
    context.pushed_attachments += 1;
    Ok(())
}

fn validate_delete_response(
    resource: &str,
    actual_id: &str,
    revision: i64,
    deleted_at: Option<&str>,
    expected_id: &str,
    base_revision: i64,
) -> AppResult<()> {
    if actual_id != expected_id {
        return Err(AppError::message(format!(
            "{resource} delete response id mismatch"
        )));
    }
    if revision <= base_revision {
        return Err(AppError::message(format!(
            "{resource} delete response revision did not advance"
        )));
    }
    tracing::debug!(resource, resource_id = actual_id, revision, ?deleted_at, "confirmed cloud deletion");
    Ok(())
}

async fn push_tags(state: &AppState, context: &mut SyncContext) -> AppResult<()> {
    let records = state.database.load_all_note_tag_rows()?;
    for record in records {
        if record.deleted {
            continue;
        }
        if record.sync_state != NOTE_STATE_PENDING_DELETE
            && record.sync_state != NOTE_STATE_PENDING
        {
            continue;
        }
        let _notify_on_completion = NotesUpdateOnDrop(&state.app);

        if record.sync_state == NOTE_STATE_PENDING_DELETE {
            let path = format!("{NOTE_TAGS_PATH}/{}?baseRevision={}", record.id, record.base_revision);
            match context
                .http
                .delete::<TagDeleteDto>(&context.base_url, &path, &context.token)
                .await
            {
                Ok(deleted) => {
                    validate_delete_response(
                        "tag",
                        &deleted.tag_id,
                        deleted.revision,
                        deleted.deleted_at.as_deref(),
                        &record.id,
                        record.base_revision,
                    )?;
                    state.database.delete_note_tag_row(&record.id)?;
                    remove_tag_everywhere(&state.database, &record.id)?;
                    context.pushed_tags += 1;
                }
                Err(AppError::Protocol { code, .. }) if code == CODE_REVISION_CONFLICT => {
                    let response = context.get::<TagListDto>(NOTE_TAGS_PATH).await?;
                    if let Some(cloud) = response.tags.into_iter().find(|tag| tag.tag_id == record.id) {
                        let mut updated = record;
                        updated.name = cloud.name;
                        updated.revision = cloud.revision;
                        updated.base_revision = cloud.revision;
                        updated.sync_state = NOTE_STATE_CONFLICT.to_string();
                        updated.created_at = parse_timestamp_millis(&cloud.created_at, updated.created_at);
                        updated.updated_at = parse_timestamp_millis(&cloud.updated_at, now_millis());
                        state.database.save_note_tag(&updated)?;
                        context.count_conflict();
                    } else {
                        state.database.delete_note_tag_row(&record.id)?;
                        remove_tag_everywhere(&state.database, &record.id)?;
                    }
                }
                Err(AppError::Protocol { code, .. }) if code == CODE_TAG_NOT_FOUND => {
                    state.database.delete_note_tag_row(&record.id)?;
                    remove_tag_everywhere(&state.database, &record.id)?;
                }
                Err(error) => return Err(error),
            }
            continue;
        }

        if record.base_revision == 0 {
            match context
                .http
                .post::<serde_json::Value, TagDto>(
                    &context.base_url,
                    NOTE_TAGS_PATH,
                    &serde_json::json!({ "tagId": record.id, "name": record.name }),
                    &context.token,
                )
                .await
            {
                Ok(dto) => {
                    let mut updated = record;
                    updated.revision = dto.revision;
                    updated.base_revision = dto.revision;
                    updated.sync_state = NOTE_STATE_SYNCED.to_string();
                    updated.created_at = parse_timestamp_millis(&dto.created_at, updated.created_at);
                    updated.updated_at = parse_timestamp_millis(&dto.updated_at, now_millis());
                    state.database.save_note_tag(&updated)?;
                    context.pushed_tags += 1;
                }
                Err(AppError::Protocol { code, .. })
                    if code == CODE_TAG_NAME_CONFLICT || code == 4002 =>
                {
                    handle_tag_create_conflict(state, context, &record).await?;
                }
                Err(error) => return Err(error),
            }
            continue;
        }

        // Rename.
        let path = format!("{NOTE_TAGS_PATH}/{}", record.id);
        match context
            .http
            .put::<serde_json::Value, TagDto>(
                &context.base_url,
                &path,
                &serde_json::json!({ "baseRevision": record.base_revision, "name": record.name }),
                &context.token,
            )
            .await
        {
            Ok(dto) => {
                let mut updated = record;
                updated.name = dto.name;
                updated.revision = dto.revision;
                updated.base_revision = dto.revision;
                updated.sync_state = NOTE_STATE_SYNCED.to_string();
                updated.created_at = parse_timestamp_millis(&dto.created_at, updated.created_at);
                updated.updated_at = parse_timestamp_millis(&dto.updated_at, now_millis());
                state.database.save_note_tag(&updated)?;
                context.pushed_tags += 1;
            }
            Err(AppError::Protocol { code, .. }) if code == CODE_REVISION_CONFLICT => {
                match context.get::<TagListDto>(NOTE_TAGS_PATH).await {
                    Ok(response) => {
                        let Some(cloud) = response.tags.into_iter().find(|tag| tag.tag_id == record.id) else {
                            state.database.delete_note_tag_row(&record.id)?;
                    remove_tag_everywhere(&state.database, &record.id)?;
                            continue;
                        };
                        let mut updated = record;
                        if cloud.name == updated.name {
                            // Same final name: adopt the cloud revision.
                            updated.revision = cloud.revision;
                            updated.base_revision = cloud.revision;
                            updated.sync_state = NOTE_STATE_SYNCED.to_string();
                            context.pushed_tags += 1;
                        } else {
                            // Two different renames raced: keep the cloud
                            // name and let the user rename again if needed.
                            updated.name = cloud.name;
                            updated.revision = cloud.revision;
                            updated.base_revision = cloud.revision;
                            updated.sync_state = NOTE_STATE_SYNCED.to_string();
                            context.count_conflict();
                        }
                        updated.created_at = parse_timestamp_millis(&cloud.created_at, updated.created_at);
                        updated.updated_at = parse_timestamp_millis(&cloud.updated_at, now_millis());
                        state.database.save_note_tag(&updated)?;
                    }
                    Err(AppError::Protocol { code, .. }) if code == CODE_TAG_NOT_FOUND => {
                        state.database.delete_note_tag_row(&record.id)?;
                        remove_tag_everywhere(&state.database, &record.id)?;
                    }
                    Err(error) => return Err(error),
                }
            }
            Err(AppError::Protocol { code, .. }) if code == CODE_TAG_NOT_FOUND => {
                state.database.delete_note_tag_row(&record.id)?;
                remove_tag_everywhere(&state.database, &record.id)?;
            }
            Err(error) => return Err(error),
        }
    }

    Ok(())
}

async fn handle_tag_create_conflict(
    state: &AppState,
    context: &mut SyncContext,
    record: &NoteTagRecord,
) -> AppResult<()> {
    let tags = match context.get::<TagListDto>(NOTE_TAGS_PATH).await {
        Ok(response) => response.tags,
        Err(error) => return Err(error),
    };

    let normalized = normalize_tag_name(record.name.trim());
    if let Some(existing) = tags
        .iter()
        .find(|tag| normalize_tag_name(tag.name.trim()) == normalized)
    {
        if existing.tag_id == record.id {
            let mut updated = record.clone();
            updated.name = existing.name.clone();
            updated.revision = existing.revision;
            updated.base_revision = existing.revision;
            updated.sync_state = NOTE_STATE_SYNCED.to_string();
            updated.created_at = parse_timestamp_millis(&existing.created_at, updated.created_at);
            updated.updated_at = parse_timestamp_millis(&existing.updated_at, now_millis());
            state.database.save_note_tag(&updated)?;
            context.pushed_tags += 1;
        } else {
            state.database.with_notes_transaction(|store| {
                retarget_tag_references(store, &record.id, &existing.tag_id)?;
                store.delete_note_tag_row(&record.id)
            })?;
            context.count_conflict();
        }
        return Ok(());
    }

    // The id belongs to another tag or a permanent tombstone. Preserve the
    // local tag and every note association under a fresh client-generated id.
    let mut replacement = record.clone();
    replacement.id = Uuid::new_v4().to_string();
    replacement.revision = 0;
    replacement.base_revision = 0;
    replacement.sync_state = NOTE_STATE_PENDING.to_string();
    replacement.updated_at = now_millis();
    state.database.with_notes_transaction(|store| {
        store.save_note_tag(&replacement)?;
        retarget_tag_references(store, &record.id, &replacement.id)?;
        store.delete_note_tag_row(&record.id)
    })?;

    let dto = context
        .http
        .post::<serde_json::Value, TagDto>(
            &context.base_url,
            NOTE_TAGS_PATH,
            &serde_json::json!({ "tagId": replacement.id, "name": replacement.name }),
            &context.token,
        )
        .await?;
    replacement.name = dto.name;
    replacement.revision = dto.revision;
    replacement.base_revision = dto.revision;
    replacement.sync_state = NOTE_STATE_SYNCED.to_string();
    replacement.created_at = parse_timestamp_millis(&dto.created_at, replacement.created_at);
    replacement.updated_at = parse_timestamp_millis(&dto.updated_at, now_millis());
    state.database.save_note_tag(&replacement)?;
    context.pushed_tags += 1;
    Ok(())
}

async fn push_notes(state: &AppState, context: &mut SyncContext) -> AppResult<()> {
    let records = state.database.load_all_note_rows()?;
    for record in records {
        if record.deleted {
            continue;
        }
        if record.sync_state != NOTE_STATE_PENDING_DELETE
            && record.sync_state != NOTE_STATE_PENDING
        {
            continue;
        }
        let _notify_on_completion = NotesUpdateOnDrop(&state.app);
        match record.sync_state.as_str() {
            NOTE_STATE_PENDING_DELETE => push_note_delete(state, context, record).await?,
            NOTE_STATE_PENDING => {
                if record.base_revision == 0 {
                    push_note_create(state, context, record, true).await?;
                } else {
                    push_note_update(state, context, record, true).await?;
                }
            }
            _ => {}
        }
    }

    Ok(())
}

async fn push_note_create(
    state: &AppState,
    context: &mut SyncContext,
    record: NoteRecord,
    allow_reference_recovery: bool,
) -> AppResult<()> {
    let request = CreateNoteRequest {
        note_id: &record.id,
        title: &record.title,
        markdown: &record.markdown,
        tag_ids: &record.tag_ids,
        attachment_ids: &record.attachment_ids,
    };

    match context
        .http
        .post::<CreateNoteRequest<'_>, NoteDto>(
            &context.base_url,
            NOTES_PATH,
            &request,
            &context.token,
        )
        .await
    {
        Ok(dto) => {
            mark_note_synced(state, record, &dto)?;
            context.pushed_notes += 1;
        }
        Err(AppError::Protocol { code, .. }) if code == 4002 => {
            // Uncertain outcome: inspect the server state before retrying.
            let path = format!("{NOTES_PATH}/{}", record.id);
            match context.get::<NoteDto>(&path).await {
                Ok(cloud) => {
                    if cloud_content_matches(&record, &cloud) {
                        mark_note_synced(state, record, &cloud)?;
                        context.pushed_notes += 1;
                    } else {
                        enter_conflict(&state.database, record, &cloud, CONFLICT_KIND_EDIT)?;
                        context.count_conflict();
                    }
                }
                Err(AppError::Protocol { code, .. }) if code == CODE_NOTE_NOT_FOUND => {
                    // The id is free: retry once with the original id.
                    let dto = context
                        .http
                        .post::<CreateNoteRequest<'_>, NoteDto>(
                            &context.base_url,
                            NOTES_PATH,
                            &request,
                            &context.token,
                        )
                        .await?;
                    mark_note_synced(state, record, &dto)?;
                    context.pushed_notes += 1;
                }
                Err(error) => return Err(error),
            }
        }
        Err(error @ AppError::Protocol { code: CODE_INVALID_NOTE_REFERENCE, .. }) => {
            if !allow_reference_recovery {
                return Err(error);
            }
            recover_invalid_references(state, context, record).await?;
        }
        Err(error) => return Err(error),
    }

    Ok(())
}

async fn push_note_update(
    state: &AppState,
    context: &mut SyncContext,
    record: NoteRecord,
    allow_reference_recovery: bool,
) -> AppResult<()> {
    let path = format!("{NOTES_PATH}/{}", record.id);
    let request = UpdateNoteRequest {
        base_revision: record.base_revision,
        title: &record.title,
        markdown: &record.markdown,
        tag_ids: &record.tag_ids,
        attachment_ids: &record.attachment_ids,
    };

    match context
        .http
        .put::<UpdateNoteRequest<'_>, NoteDto>(
            &context.base_url,
            &path,
            &request,
            &context.token,
        )
        .await
    {
        Ok(dto) => {
            mark_note_synced(state, record, &dto)?;
            context.pushed_notes += 1;
        }
        Err(AppError::Protocol { code, .. }) if code == CODE_REVISION_CONFLICT => {
            match context.get::<NoteDto>(&path).await {
                Ok(cloud) => {
                    if cloud_content_matches(&record, &cloud) {
                        mark_note_synced(state, record, &cloud)?;
                        context.pushed_notes += 1;
                    } else {
                        attempt_merged_push(state, context, record, &cloud).await?;
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(AppError::Protocol { code, .. }) if code == CODE_NOTE_NOT_FOUND => {
            // Cloud deleted while local edits are pending.
            enter_cloud_deleted_conflict(state, record)?;
            context.count_conflict();
        }
        Err(error @ AppError::Protocol { code: CODE_INVALID_NOTE_REFERENCE, .. }) => {
            if !allow_reference_recovery {
                return Err(error);
            }
            recover_invalid_references(state, context, record).await?;
        }
        Err(error) => return Err(error),
    }

    Ok(())
}

async fn push_note_delete(
    state: &AppState,
    context: &mut SyncContext,
    record: NoteRecord,
) -> AppResult<()> {
    let path = format!(
        "{NOTES_PATH}/{}?baseRevision={}",
        record.id, record.base_revision
    );

    match context
        .http
        .delete::<NoteDeleteDto>(&context.base_url, &path, &context.token)
        .await
    {
        Ok(deleted) => {
            validate_delete_response(
                "note",
                &deleted.note_id,
                deleted.revision,
                deleted.deleted_at.as_deref(),
                &record.id,
                record.base_revision,
            )?;
            state.database.delete_note_row(&record.id)?;
            context.pushed_notes += 1;
        }
        Err(AppError::Protocol { code, .. }) if code == CODE_REVISION_CONFLICT => {
            let note_path = format!("{NOTES_PATH}/{}", record.id);
            match context.get::<NoteDto>(&note_path).await {
                Ok(cloud) => {
                    persist_cloud_attachments(&state.database, &cloud.attachments)?;
                    let mut updated = record;
                    updated.sync_state = NOTE_STATE_CONFLICT_DELETE.to_string();
                    updated.conflict_kind = Some(CONFLICT_KIND_DELETE.to_string());
                    updated.conflict_title = Some(cloud.title.clone());
                    updated.conflict_markdown = Some(cloud.markdown.clone());
                    updated.conflict_tag_ids = Some(normalize_id_list(cloud.tag_ids.clone()));
                    updated.conflict_attachment_ids = Some(attachment_ids_of(&cloud));
                    updated.conflict_revision = Some(cloud.revision);
                    updated.updated_at = now_millis();
                    state.database.save_note(&updated)?;
                    context.count_conflict();
                }
                Err(AppError::Protocol { code, .. }) if code == CODE_NOTE_NOT_FOUND => {
                    // Gone on the server after all: deletion succeeded.
                    state.database.delete_note_row(&record.id)?;
                    context.pushed_notes += 1;
                }
                Err(error) => return Err(error),
            }
        }
        Err(AppError::Protocol { code, .. }) if code == CODE_NOTE_NOT_FOUND => {
            state.database.delete_note_row(&record.id)?;
            context.pushed_notes += 1;
        }
        Err(error) => return Err(error),
    }

    Ok(())
}

/// Attempts an automatic three-way merge and, when clean, submits again
/// with the cloud revision as the new base.
async fn attempt_merged_push(
    state: &AppState,
    context: &mut SyncContext,
    record: NoteRecord,
    cloud: &NoteDto,
) -> AppResult<()> {
    let note_id = record.id.clone();
    let merge = compute_merge(&record, cloud);

    match merge.resolved_markdown {
        Some(merged_markdown) if merge.title.is_resolved() => {
            let mut updated = record;
            updated.title = match &merge.title {
                FieldMerge::Resolved(title) => title.clone(),
                FieldMerge::Conflict { local, .. } => local.clone(),
            };
            updated.markdown = merged_markdown;
            updated.tag_ids = merge.tag_ids;
            updated.attachment_ids = merge.attachment_ids;
            updated.base_revision = cloud.revision;
            updated.updated_at = now_millis();
            state.database.save_note(&updated)?;

            let path = format!("{NOTES_PATH}/{note_id}");
            let request = UpdateNoteRequest {
                base_revision: cloud.revision,
                title: &updated.title,
                markdown: &updated.markdown,
                tag_ids: &updated.tag_ids,
                attachment_ids: &updated.attachment_ids,
            };
            match context
                .http
                .put::<UpdateNoteRequest<'_>, NoteDto>(
                    &context.base_url,
                    &path,
                    &request,
                    &context.token,
                )
                .await
            {
                Ok(dto) => {
                    mark_note_synced(state, updated, &dto)?;
                    context.pushed_notes += 1;
                }
                Err(AppError::Protocol { code: CODE_INVALID_NOTE_REFERENCE, .. }) => {
                    recover_invalid_references(state, context, updated).await?;
                }
                Err(error) => return Err(error),
            }
        }
        _ => {
            enter_conflict(&state.database, record, cloud, CONFLICT_KIND_EDIT)?;
            context.count_conflict();
        }
    }

    Ok(())
}

/// Repairs invalid references and resubmits exactly once.
async fn recover_invalid_references(
    state: &AppState,
    context: &mut SyncContext,
    mut record: NoteRecord,
) -> AppResult<()> {
    refresh_tag_list(state, context).await?;

    let live_tags: HashSet<String> = state
        .database
        .load_note_tags()?
        .into_iter()
        .filter(|tag| !tag.deleted && tag.sync_state != NOTE_STATE_PENDING_DELETE)
        .map(|tag| tag.id)
        .collect();
    record.tag_ids.retain(|id| live_tags.contains(id));
    state.database.save_note(&record)?;

    for attachment_id in record.attachment_ids.clone() {
        let metadata_path = format!("{NOTE_ATTACHMENTS_PATH}/{attachment_id}");
        match context.get::<AttachmentDto>(&metadata_path).await {
            Ok(metadata) => persist_cloud_attachments(&state.database, &[metadata])?,
            Err(AppError::Protocol { code, .. }) if code == CODE_ATTACHMENT_NOT_FOUND => {
                let local = state.database.load_note_attachment(&attachment_id)?;
                let cache_path = attachment_cache_path(state, &attachment_id)?;
                if let Some(local) = local.filter(|_| cache_path.is_file()) {
                    let replacement = rekey_attachment(state, local)?;
                    upload_attachment_record(state, context, &replacement).await?;
                } else {
                    remove_current_attachment_references(state, &attachment_id)?;
                    state.database.delete_note_attachment_row(&attachment_id)?;
                    context.repaired_references += 1;
                }
                record = state.database.load_note(&record.id)?.unwrap_or(record);
            }
            Err(error) => return Err(error),
        }
    }

    if record.base_revision == 0 {
        state.database.save_note(&record)?;
        return Box::pin(push_note_create(state, context, record, false)).await;
    }

    let note_path = format!("{NOTES_PATH}/{}", record.id);
    match context.get::<NoteDto>(&note_path).await {
        Ok(cloud) => {
            if record.base_revision != cloud.revision {
                let merge = compute_merge(&record, &cloud);
                match merge.resolved_markdown {
                    Some(merged_markdown) if merge.title.is_resolved() => {
                        record.title = match &merge.title {
                            FieldMerge::Resolved(title) => title.clone(),
                            FieldMerge::Conflict { local, .. } => local.clone(),
                        };
                        record.markdown = merged_markdown;
                        record.tag_ids = merge.tag_ids;
                        record.attachment_ids = merge.attachment_ids;
                        record.base_revision = cloud.revision;
                    }
                    _ => {
                        enter_conflict(&state.database, record, &cloud, CONFLICT_KIND_EDIT)?;
                        context.count_conflict();
                        return Ok(());
                    }
                }
            }
            state.database.save_note(&record)?;
            Box::pin(push_note_update(state, context, record, false)).await
        }
        Err(AppError::Protocol { code, .. }) if code == CODE_NOTE_NOT_FOUND => {
            enter_cloud_deleted_conflict(state, record)?;
            context.count_conflict();
            Ok(())
        }
        Err(error) => Err(error),
    }
}

async fn refresh_tag_list(
    state: &AppState,
    context: &mut SyncContext,
) -> AppResult<()> {
    let tags = match context.get::<TagListDto>(NOTE_TAGS_PATH).await {
        Ok(response) => response.tags,
        Err(error) => return Err(error),
    };

    let cloud_ids: HashSet<String> = tags.iter().map(|tag| tag.tag_id.clone()).collect();
    let now = now_millis();
    for tag in tags {
        let existing = state.database.load_note_tag(&tag.tag_id)?;
        let record = match existing {
            Some(record) => {
                let mut record = record;
                if record.sync_state == NOTE_STATE_SYNCED {
                    record.name = tag.name;
                    record.revision = tag.revision;
                    record.base_revision = tag.revision;
                    record.created_at = parse_timestamp_millis(&tag.created_at, record.created_at);
                    record.updated_at = parse_timestamp_millis(&tag.updated_at, now);
                    Some(record)
                } else {
                    None
                }
            }
            None => Some(NoteTagRecord {
                id: tag.tag_id,
                name: tag.name,
                revision: tag.revision,
                base_revision: tag.revision,
                sync_state: NOTE_STATE_SYNCED.to_string(),
                deleted: false,
                created_at: parse_timestamp_millis(&tag.created_at, now),
                updated_at: parse_timestamp_millis(&tag.updated_at, now),
            }),
        };
        if let Some(record) = record {
            state.database.save_note_tag(&record)?;
        }
    }

    // Remove local synced tags that no longer exist on the server.
    for tag in state.database.load_all_note_tag_rows()? {
        if tag.sync_state == NOTE_STATE_SYNCED && !cloud_ids.contains(&tag.id) {
            state.database.delete_note_tag_row(&tag.id)?;
            remove_tag_everywhere(&state.database, &tag.id)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Pull
// ---------------------------------------------------------------------------

async fn pull_snapshot(state: &AppState, context: &mut SyncContext) -> AppResult<()> {
    let mut page_token: Option<String> = None;
    let mut seen_notes: HashSet<String> = HashSet::new();
    let mut seen_tags: HashSet<String> = HashSet::new();
    let limit_string = SYNC_PAGE_LIMIT.to_string();

    let cursor = loop {
        let mut query: Vec<(&str, &str)> = vec![("limit", &limit_string)];
        let token_holder;
        if let Some(token) = &page_token {
            token_holder = token.clone();
            query.push(("pageToken", &token_holder));
        }

        let path = url_query(&context.base_url, NOTES_SNAPSHOT_PATH, &query)?;
        let page: SnapshotDto = context.http.get_raw(&path, &context.token).await?;

        for tag in &page.tags {
            apply_tag_upsert(&state.database, tag, context)?;
            seen_tags.insert(tag.tag_id.clone());
        }
        for note in &page.notes {
            apply_note_upsert(&state.database, note, context)?;
            seen_notes.insert(note.note_id.clone());
        }
        notify_notes_changed(&state.app);

        match page.next_page_token {
            Some(token) => page_token = Some(token),
            None => break page.cursor,
        }
    };

    state.database.with_notes_transaction(|store| {
        reconcile_snapshot(store, &seen_notes, &seen_tags)?;
        store.save_cursor(&cursor)
    })?;
    notify_notes_changed(&state.app);
    pull_incremental(state, context, cursor).await?;

    Ok(())
}

async fn pull_incremental(
    state: &AppState,
    context: &mut SyncContext,
    cursor: String,
) -> AppResult<()> {
    let mut cursor = cursor;
    let limit_string = SYNC_PAGE_LIMIT.to_string();
    loop {
        let path = url_query(
            &context.base_url,
            NOTES_CHANGES_PATH,
            &[("cursor", &cursor), ("limit", &limit_string)],
        )?;
        let page: ChangesDto = context.http.get_raw(&path, &context.token).await?;

        state.database.with_notes_transaction(|store| {
            for entry in &page.changes {
                match (entry.resource_type.as_str(), entry.operation.as_str()) {
                    ("note", "upsert") => apply_note_upsert(
                        store,
                        entry.note.as_ref().ok_or_else(|| {
                            AppError::message("note upsert change is missing note")
                        })?,
                        context,
                    )?,
                    ("note", "delete") => apply_note_delete(
                        store,
                        entry.note_id.as_deref().ok_or_else(|| {
                            AppError::message("note delete change is missing noteId")
                        })?,
                        entry.revision.ok_or_else(|| {
                            AppError::message("note delete change is missing revision")
                        })?,
                        context,
                    )?,
                    ("tag", "upsert") => apply_tag_upsert(
                        store,
                        entry.tag.as_ref().ok_or_else(|| {
                            AppError::message("tag upsert change is missing tag")
                        })?,
                        context,
                    )?,
                    ("tag", "delete") => {
                        let tag_id = entry.tag_id.as_deref().ok_or_else(|| {
                            AppError::message("tag delete change is missing tagId")
                        })?;
                        entry.revision.ok_or_else(|| {
                            AppError::message("tag delete change is missing revision")
                        })?;
                        apply_tag_delete(store, tag_id, context)?;
                    }
                    _ => return Err(AppError::message("unknown notes change entry")),
                }
            }
            store.save_cursor(&page.next_cursor)
        })?;
        notify_notes_changed(&state.app);

        cursor = page.next_cursor;

        if !page.has_more {
            break;
        }
    }

    Ok(())
}

fn apply_note_upsert(
    store: &impl NotesStore,
    dto: &NoteDto,
    context: &mut SyncContext,
) -> AppResult<()> {
    persist_cloud_attachments(store, &dto.attachments)?;
    let local = store.load_note(&dto.note_id)?;

    let Some(record) = local else {
        let record = cloud_to_record(dto, now_millis());
        store.save_note(&record)?;
        context.pulled_notes += 1;
        return Ok(());
    };

    if record.sync_state == NOTE_STATE_PENDING_DELETE {
        if dto.revision == record.base_revision {
            // Server unchanged; the queued delete still applies.
            return Ok(());
        }
        let mut updated = record;
        updated.sync_state = NOTE_STATE_CONFLICT_DELETE.to_string();
        updated.conflict_kind = Some(CONFLICT_KIND_DELETE.to_string());
        updated.conflict_title = Some(dto.title.clone());
        updated.conflict_markdown = Some(dto.markdown.clone());
        updated.conflict_tag_ids = Some(normalize_id_list(dto.tag_ids.clone()));
        updated.conflict_attachment_ids = Some(attachment_ids_of(dto));
        updated.conflict_revision = Some(dto.revision);
        updated.updated_at = now_millis();
        store.save_note(&updated)?;
        context.count_conflict();
        return Ok(());
    }

    if record.sync_state == NOTE_STATE_SYNCED {
        if record.revision >= dto.revision {
            return Ok(());
        }
        let updated = cloud_to_record(dto, record.created_at);
        store.save_note(&updated)?;
        context.pulled_notes += 1;
        return Ok(());
    }

    // Local pending or conflicted edits: attempt a three-way merge.
    if dto.revision <= record.base_revision {
        return Ok(());
    }

    let merge = compute_merge(&record, dto);
    match merge.resolved_markdown {
        Some(merged_markdown) if merge.title.is_resolved() => {
            let mut updated = record;
            updated.title = match &merge.title {
                FieldMerge::Resolved(title) => title.clone(),
                FieldMerge::Conflict { local, .. } => local.clone(),
            };
            updated.markdown = merged_markdown;
            updated.tag_ids = merge.tag_ids;
            updated.attachment_ids = merge.attachment_ids;
            updated.base_revision = dto.revision;
            updated.ancestor_title = dto.title.clone();
            updated.ancestor_markdown = dto.markdown.clone();
            updated.ancestor_tag_ids = normalize_id_list(dto.tag_ids.clone());
            updated.ancestor_attachment_ids = attachment_ids_of(dto);
            updated.ancestor_revision = dto.revision;
            updated.updated_at = now_millis();
            store.save_note(&updated)?;
            context.pulled_notes += 1;
        }
        _ => {
            enter_conflict(store, record, dto, CONFLICT_KIND_EDIT)?;
            context.count_conflict();
        }
    }

    Ok(())
}

fn apply_note_delete(
    store: &impl NotesStore,
    note_id: &str,
    revision: i64,
    context: &mut SyncContext,
) -> AppResult<()> {
    let Some(record) = store.load_note(note_id)? else {
        return Ok(());
    };

    match record.sync_state.as_str() {
        NOTE_STATE_PENDING | NOTE_STATE_CONFLICT => {
            if revision > record.base_revision {
                // Deleted remotely while local edits exist: retain content
                // and prompt the user instead of discarding silently.
                let mut updated = record;
                updated.sync_state = NOTE_STATE_CONFLICT.to_string();
                updated.conflict_kind = Some(CONFLICT_KIND_CLOUD_DELETED.to_string());
                updated.conflict_revision = Some(revision);
                updated.updated_at = now_millis();
                store.save_note(&updated)?;
                context.count_conflict();
            }
        }
        NOTE_STATE_PENDING_DELETE => {
            store.delete_note_row(note_id)?;
        }
        _ => {
            store.delete_note_row(note_id)?;
            context.pulled_notes += 1;
        }
    }

    Ok(())
}

fn apply_tag_upsert(store: &impl NotesStore, dto: &TagDto, context: &mut SyncContext) -> AppResult<()> {
    let Some(local) = store.load_note_tag(&dto.tag_id)? else {
        let record = NoteTagRecord {
            id: dto.tag_id.clone(),
            name: dto.name.clone(),
            revision: dto.revision,
            base_revision: dto.revision,
            sync_state: NOTE_STATE_SYNCED.to_string(),
            deleted: false,
            created_at: parse_timestamp_millis(&dto.created_at, now_millis()),
            updated_at: parse_timestamp_millis(&dto.updated_at, now_millis()),
        };
        store.save_note_tag(&record)?;
        context.pulled_tags += 1;
        return Ok(());
    };

    if local.sync_state == NOTE_STATE_SYNCED {
        if local.revision >= dto.revision {
            return Ok(());
        }
        let mut updated = local;
        updated.name = dto.name.clone();
        updated.revision = dto.revision;
        updated.base_revision = dto.revision;
        updated.updated_at = parse_timestamp_millis(&dto.updated_at, now_millis());
        store.save_note_tag(&updated)?;
        context.pulled_tags += 1;
        return Ok(());
    }

    if local.sync_state == NOTE_STATE_PENDING && dto.revision > local.base_revision {
        let mut updated = local;
        if updated.name == dto.name {
            updated.revision = dto.revision;
            updated.base_revision = dto.revision;
            updated.sync_state = NOTE_STATE_SYNCED.to_string();
            context.pushed_tags = context.pushed_tags.saturating_sub(0);
        } else {
            // Two devices renamed the same tag differently.
            updated.revision = dto.revision;
            updated.base_revision = dto.revision;
            updated.sync_state = NOTE_STATE_CONFLICT.to_string();
            context.count_conflict();
        }
        updated.created_at = parse_timestamp_millis(&dto.created_at, updated.created_at);
        updated.updated_at = parse_timestamp_millis(&dto.updated_at, now_millis());
        store.save_note_tag(&updated)?;
    }

    Ok(())
}

fn apply_tag_delete(store: &impl NotesStore, tag_id: &str, context: &mut SyncContext) -> AppResult<()> {
    if let Some(record) = store.load_note_tag(tag_id)? {
        if record.sync_state == NOTE_STATE_PENDING_DELETE || record.sync_state == NOTE_STATE_SYNCED
        {
            store.delete_note_tag_row(tag_id)?;
        }
    }

    // Remove the tag from synced notes on both sides so they stay
    // consistent with the new server state; pending notes keep their
    // references and recover through the 6008 flow on push.
    remove_tag_from_synced_notes(store, tag_id)?;
    context.pulled_tags += 1;

    Ok(())
}

fn reconcile_snapshot(
    store: &impl NotesStore,
    seen_notes: &HashSet<String>,
    seen_tags: &HashSet<String>,
) -> AppResult<()> {
    for record in store.load_all_note_rows()? {
        if record.deleted {
            continue;
        }
        if !record.has_local_edits() && !seen_notes.contains(&record.id) {
            store.delete_note_row(&record.id)?;
        }
    }
    for tag in store.load_all_note_tag_rows()? {
        if tag.deleted {
            continue;
        }
        let is_pending = tag.sync_state == NOTE_STATE_PENDING
            || tag.sync_state == NOTE_STATE_PENDING_DELETE
            || tag.sync_state == NOTE_STATE_CONFLICT;
        if !is_pending && !seen_tags.contains(&tag.id) {
            store.delete_note_tag_row(&tag.id)?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Conflict resolution
// ---------------------------------------------------------------------------

pub async fn resolve_conflict(
    state: &AppState,
    payload: ConflictResolutionPayload,
) -> AppResult<NoteRecord> {
    let record = state
        .database
        .load_note(&payload.note_id)?
        .ok_or_else(|| AppError::message("note not found"))?;

    if record.sync_state != NOTE_STATE_CONFLICT && record.sync_state != NOTE_STATE_CONFLICT_DELETE
    {
        return Err(AppError::message("note has no conflict"));
    }

    let cloud_revision = record.conflict_revision;
    let cloud_deleted = record
        .conflict_kind
        .as_deref()
        .map(|kind| kind == CONFLICT_KIND_CLOUD_DELETED)
        .unwrap_or(false);

    let mut record = record;
    let result = match payload.resolution.as_str() {
        "local" => {
            if cloud_deleted {
                // Keep the local content as a brand-new note; the deleted
                // note id is never restored.
                state.database.delete_note_row(&record.id)?;
                let mut fresh = NoteRecord::new(Uuid::new_v4().to_string(), now_millis());
                fresh.title = record.title.clone();
                fresh.markdown = record.markdown.clone();
                fresh.tag_ids = record.tag_ids.clone();
                fresh.attachment_ids = record.attachment_ids.clone();
                fresh.sync_state = NOTE_STATE_PENDING.to_string();
                state.database.save_note(&fresh)?;
                fresh
            } else {
                record.base_revision = cloud_revision.unwrap_or(record.revision);
                record.sync_state = NOTE_STATE_PENDING.to_string();
                clear_conflict(&mut record);
                record.updated_at = now_millis();
                state.database.save_note(&record)?;
                record
            }
        }
        "cloud" => {
            if cloud_deleted {
                state.database.delete_note_row(&record.id)?;
                record
            } else {
                let title = record.conflict_title.clone().unwrap_or_default();
                let markdown = record.conflict_markdown.clone().unwrap_or_default();
                let tag_ids = record.conflict_tag_ids.clone().unwrap_or_default();
                let attachment_ids =
                    record.conflict_attachment_ids.clone().unwrap_or_default();
                record.title = title;
                record.markdown = markdown;
                record.tag_ids = tag_ids;
                record.attachment_ids = attachment_ids;
                record.revision = cloud_revision.unwrap_or(record.revision);
                record.base_revision = record.revision;
                record.ancestor_title = record.title.clone();
                record.ancestor_markdown = record.markdown.clone();
                record.ancestor_tag_ids = record.tag_ids.clone();
                record.ancestor_attachment_ids = record.attachment_ids.clone();
                record.ancestor_revision = record.revision;
                record.sync_state = NOTE_STATE_SYNCED.to_string();
                clear_conflict(&mut record);
                record.updated_at = now_millis();
                state.database.save_note(&record)?;
                record
            }
        }
        "merged" => {
            let title = payload.title.clone().unwrap_or_else(|| record.title.clone());
            let markdown = payload
                .markdown
                .clone()
                .unwrap_or_else(|| record.markdown.clone());
            let tag_ids = payload
                .tag_ids
                .clone()
                .unwrap_or_else(|| record.tag_ids.clone());
            let attachment_ids = payload
                .attachment_ids
                .clone()
                .unwrap_or_else(|| record.attachment_ids.clone());

            if cloud_deleted {
                state.database.delete_note_row(&record.id)?;
                let mut fresh = NoteRecord::new(Uuid::new_v4().to_string(), now_millis());
                fresh.title = title;
                fresh.markdown = markdown;
                fresh.tag_ids = tag_ids;
                fresh.attachment_ids = attachment_ids;
                fresh.sync_state = NOTE_STATE_PENDING.to_string();
                state.database.save_note(&fresh)?;
                fresh
            } else {
                record.title = title;
                record.markdown = markdown;
                record.tag_ids = tag_ids;
                record.attachment_ids = attachment_ids;
                record.base_revision = cloud_revision.unwrap_or(record.revision);
                record.sync_state = NOTE_STATE_PENDING.to_string();
                clear_conflict(&mut record);
                record.updated_at = now_millis();
                state.database.save_note(&record)?;
                record
            }
        }
        "confirm_delete" => {
            // User confirmed deleting the newer cloud revision.
            record.base_revision = cloud_revision.unwrap_or(record.revision);
            record.sync_state = NOTE_STATE_PENDING_DELETE.to_string();
            clear_conflict(&mut record);
            record.updated_at = now_millis();
            state.database.save_note(&record)?;
            record
        }
        "cancel_delete" => {
            // User kept the cloud version after a delete conflict.
            if let Some(title) = record.conflict_title.clone() {
                record.title = title;
            }
            if let Some(markdown) = record.conflict_markdown.clone() {
                record.markdown = markdown;
            }
            if let Some(tag_ids) = record.conflict_tag_ids.clone() {
                record.tag_ids = tag_ids;
            }
            if let Some(attachment_ids) = record.conflict_attachment_ids.clone() {
                record.attachment_ids = attachment_ids;
            }
            record.revision = cloud_revision.unwrap_or(record.revision);
            record.base_revision = record.revision;
            record.ancestor_title = record.title.clone();
            record.ancestor_markdown = record.markdown.clone();
            record.ancestor_tag_ids = record.tag_ids.clone();
            record.ancestor_attachment_ids = record.attachment_ids.clone();
            record.ancestor_revision = record.revision;
            record.sync_state = NOTE_STATE_SYNCED.to_string();
            clear_conflict(&mut record);
            record.updated_at = now_millis();
            state.database.save_note(&record)?;
            record
        }
        other => return Err(AppError::message(format!("unknown resolution: {other}"))),
    };

    notify_notes_changed(&state.app);
    Ok(result)
}

fn clear_conflict(record: &mut NoteRecord) {
    record.conflict_kind = None;
    record.conflict_title = None;
    record.conflict_markdown = None;
    record.conflict_tag_ids = None;
    record.conflict_attachment_ids = None;
    record.conflict_revision = None;
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

struct ComputedMerge {
    resolved_markdown: Option<String>,
    title: FieldMerge,
    tag_ids: Vec<String>,
    attachment_ids: Vec<String>,
}

fn compute_merge(record: &NoteRecord, cloud: &NoteDto) -> ComputedMerge {
    let cloud_attachment_ids = attachment_ids_of(cloud);
    ComputedMerge {
        resolved_markdown: merge_markdown(
            &record.ancestor_markdown,
            &record.markdown,
            &cloud.markdown,
        ),
        title: merge_field(&record.ancestor_title, &record.title, &cloud.title),
        tag_ids: merge_set(&record.ancestor_tag_ids, &record.tag_ids, &cloud.tag_ids),
        attachment_ids: merge_set(
            &record.ancestor_attachment_ids,
            &record.attachment_ids,
            &cloud_attachment_ids,
        ),
    }
}

fn cloud_content_matches(record: &NoteRecord, cloud: &NoteDto) -> bool {
    let cloud_tag_ids = normalize_id_list(cloud.tag_ids.clone());
    record.content_matches(
        &cloud.title,
        &cloud.markdown,
        &cloud_tag_ids,
        &attachment_ids_of(cloud),
    )
}

fn attachment_ids_of(dto: &NoteDto) -> Vec<String> {
    let mut ids: Vec<String> = dto
        .attachments
        .iter()
        .map(|attachment| attachment.attachment_id.clone())
        .collect();
    ids.sort();
    ids
}

fn normalize_id_list(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids.dedup();
    ids
}

fn cloud_to_record(dto: &NoteDto, created_at: i64) -> NoteRecord {
    let mut record = NoteRecord::new(
        dto.note_id.clone(),
        parse_timestamp_millis(&dto.created_at, created_at),
    );
    record.title = dto.title.clone();
    record.markdown = dto.markdown.clone();
    record.tag_ids = normalize_id_list(dto.tag_ids.clone());
    record.attachment_ids = attachment_ids_of(dto);
    record.revision = dto.revision;
    record.base_revision = dto.revision;
    record.ancestor_title = dto.title.clone();
    record.ancestor_markdown = dto.markdown.clone();
    record.ancestor_tag_ids = normalize_id_list(dto.tag_ids.clone());
    record.ancestor_attachment_ids = attachment_ids_of(dto);
    record.ancestor_revision = dto.revision;
    record.sync_state = NOTE_STATE_SYNCED.to_string();
    record.updated_at = parse_timestamp_millis(&dto.updated_at, now_millis());
    record
}

fn mark_note_synced(state: &AppState, mut record: NoteRecord, dto: &NoteDto) -> AppResult<()> {
    persist_cloud_attachments(&state.database, &dto.attachments)?;
    record.title = dto.title.clone();
    record.markdown = dto.markdown.clone();
    record.tag_ids = normalize_id_list(dto.tag_ids.clone());
    record.attachment_ids = attachment_ids_of(dto);
    record.revision = dto.revision;
    record.base_revision = dto.revision;
    record.ancestor_title = dto.title.clone();
    record.ancestor_markdown = dto.markdown.clone();
    record.ancestor_tag_ids = normalize_id_list(dto.tag_ids.clone());
    record.ancestor_attachment_ids = attachment_ids_of(dto);
    record.ancestor_revision = dto.revision;
    record.sync_state = NOTE_STATE_SYNCED.to_string();
    clear_conflict(&mut record);
    record.created_at = parse_timestamp_millis(&dto.created_at, record.created_at);
    record.updated_at = parse_timestamp_millis(&dto.updated_at, now_millis());
    state.database.save_note(&record)
}

fn enter_conflict(
    store: &impl NotesStore,
    mut record: NoteRecord,
    cloud: &NoteDto,
    kind: &str,
) -> AppResult<()> {
    persist_cloud_attachments(store, &cloud.attachments)?;
    record.sync_state = NOTE_STATE_CONFLICT.to_string();
    record.conflict_kind = Some(kind.to_string());
    record.conflict_title = Some(cloud.title.clone());
    record.conflict_markdown = Some(cloud.markdown.clone());
    record.conflict_tag_ids = Some(normalize_id_list(cloud.tag_ids.clone()));
    record.conflict_attachment_ids = Some(attachment_ids_of(cloud));
    record.conflict_revision = Some(cloud.revision);
    record.updated_at = now_millis();
    store.save_note(&record)
}

fn enter_cloud_deleted_conflict(state: &AppState, mut record: NoteRecord) -> AppResult<()> {
    record.sync_state = NOTE_STATE_CONFLICT.to_string();
    record.conflict_kind = Some(CONFLICT_KIND_CLOUD_DELETED.to_string());
    record.updated_at = now_millis();
    state.database.save_note(&record)
}

fn retarget_tag_references(store: &impl NotesStore, from: &str, to: &str) -> AppResult<()> {
    let rows = store.load_all_note_rows()?;
    for mut row in rows {
        let mut changed = false;
        if let Some(position) = row.tag_ids.iter().position(|id| id == from) {
            row.tag_ids[position] = to.to_string();
            changed = true;
        }
        if let Some(position) = row.ancestor_tag_ids.iter().position(|id| id == from) {
            row.ancestor_tag_ids[position] = to.to_string();
            changed = true;
        }
        if changed {
            store.save_note(&row)?;
        }
    }
    Ok(())
}

fn remove_tag_from_synced_notes(store: &impl NotesStore, tag_id: &str) -> AppResult<()> {
    let rows = store.load_all_note_rows()?;
    for mut row in rows {
        if row.sync_state != NOTE_STATE_SYNCED {
            continue;
        }
        let before = row.tag_ids.len();
        row.tag_ids.retain(|id| id != tag_id);
        row.ancestor_tag_ids.retain(|id| id != tag_id);
        if row.tag_ids.len() != before {
            store.save_note(&row)?;
        }
    }
    Ok(())
}

fn remove_tag_everywhere(store: &impl NotesStore, tag_id: &str) -> AppResult<()> {
    let rows = store.load_all_note_rows()?;
    for mut row in rows {
        let before = row.tag_ids.len();
        row.tag_ids.retain(|id| id != tag_id);
        row.ancestor_tag_ids.retain(|id| id != tag_id);
        if row.tag_ids.len() != before {
            store.save_note(&row)?;
        }
    }
    Ok(())
}

fn rekey_attachment(
    state: &AppState,
    record: NoteAttachmentRecord,
) -> AppResult<NoteAttachmentRecord> {
    let replacement_id = Uuid::new_v4().to_string();
    let source = attachment_cache_path(state, &record.id)?;
    let target = attachment_cache_path(state, &replacement_id)?;
    std::fs::copy(&source, &target)?;

    let mut replacement = record.clone();
    replacement.id = replacement_id.clone();
    replacement.sync_state = NOTE_STATE_PENDING.to_string();
    replacement.deleted = false;
    if let Err(error) = state.database.with_notes_transaction(|store| {
        store.save_note_attachment(&replacement)?;
        replace_current_attachment_references(store, &record.id, &replacement_id)?;
        store.delete_note_attachment_row(&record.id)
    }) {
        let _ = std::fs::remove_file(&target);
        return Err(error);
    }
    let _ = std::fs::remove_file(source);
    Ok(replacement)
}

fn replace_current_attachment_references(
    store: &impl NotesStore,
    from: &str,
    to: &str,
) -> AppResult<()> {
    let from_uri = format!("colink-attachment://{from}");
    let to_uri = format!("colink-attachment://{to}");
    for mut note in store.load_all_note_rows()? {
        if note.attachment_ids.iter().any(|id| id == from) || note.markdown.contains(&from_uri) {
            note.markdown = note.markdown.replace(&from_uri, &to_uri);
            for id in &mut note.attachment_ids {
                if id == from {
                    *id = to.to_string();
                }
            }
            note.attachment_ids = normalize_id_list(note.attachment_ids);
            if note.sync_state == NOTE_STATE_SYNCED {
                note.sync_state = NOTE_STATE_PENDING.to_string();
            }
            note.updated_at = now_millis();
            store.save_note(&note)?;
        }
    }
    Ok(())
}

fn remove_current_attachment_references(state: &AppState, attachment_id: &str) -> AppResult<()> {
    let pattern = regex::Regex::new(&format!(
        r"!?\[[^\]]*\]\(colink-attachment://{}\)",
        regex::escape(attachment_id)
    ))
    .map_err(|error| AppError::message(error.to_string()))?;
    for mut note in state.database.load_all_note_rows()? {
        if note.attachment_ids.iter().any(|id| id == attachment_id)
            || pattern.is_match(&note.markdown)
        {
            note.attachment_ids.retain(|id| id != attachment_id);
            note.markdown = pattern.replace_all(&note.markdown, "").to_string();
            while note.markdown.contains("\n\n\n") {
                note.markdown = note.markdown.replace("\n\n\n", "\n\n");
            }
            if note.sync_state == NOTE_STATE_SYNCED {
                note.sync_state = NOTE_STATE_PENDING.to_string();
            }
            note.updated_at = now_millis();
            state.database.save_note(&note)?;
        }
    }
    Ok(())
}

fn persist_cloud_attachments(store: &impl NotesStore, attachments: &[AttachmentDto]) -> AppResult<()> {
    for dto in attachments {
        let existing = store.load_note_attachment(&dto.attachment_id)?;
        if let Some(existing) = &existing {
            if existing.kind != dto.kind || existing.size != dto.size || existing.sha256 != dto.sha256 {
                return Err(AppError::message(format!(
                    "attachment id collision: {}",
                    dto.attachment_id
                )));
            }
        }
        let record = NoteAttachmentRecord {
            id: dto.attachment_id.clone(),
            kind: dto.kind.clone(),
            file_name: dto.file_name.clone(),
            media_type: dto.media_type.clone(),
            size: dto.size,
            sha256: dto.sha256.clone(),
            sync_state: NOTE_STATE_SYNCED.to_string(),
            deleted: false,
            created_at: parse_timestamp_millis(
                &dto.created_at,
                existing.map(|record| record.created_at).unwrap_or_else(now_millis),
            ),
        };
        store.save_note_attachment(&record)?;
    }
    Ok(())
}

fn parse_timestamp_millis(raw: &str, fallback: i64) -> i64 {
    chrono::DateTime::parse_from_rfc3339(raw)
        .map(|timestamp| timestamp.timestamp_millis())
        .unwrap_or(fallback)
}

fn sha256_file(path: &std::path::Path) -> AppResult<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn attachment_cache_path(state: &AppState, id: &str) -> AppResult<PathBuf> {
    let app_dir = crate::state::app_data_dir(&state.app)?;
    let scope = state.database.current_notes_scope()?;
    let scoped = attachment_cache_path_for_scope(&app_dir, &scope, id);
    let legacy = app_dir.join("notes-attachments").join(id);
    let local = attachment_cache_path_for_scope(&app_dir, LOCAL_ACCOUNT_SCOPE, id);
    let fallback = if legacy.is_file() {
        Some(legacy)
    } else if scope != LOCAL_ACCOUNT_SCOPE && local.is_file() {
        Some(local)
    } else {
        None
    };
    if !scoped.exists() {
        if let Some(fallback) = fallback {
            if let Some(parent) = scoped.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if std::fs::rename(&fallback, &scoped).is_err() {
                std::fs::copy(&fallback, &scoped)?;
                std::fs::remove_file(fallback)?;
            }
        }
    }
    Ok(scoped)
}

fn attachment_cache_path_for_scope(
    app_dir: &std::path::Path,
    scope: &str,
    id: &str,
) -> PathBuf {
    app_dir
        .join("notes-attachments")
        .join(sha256_hex(scope.as_bytes()))
        .join(id)
}

fn load_settings(state: &AppState) -> AppResult<crate::models::AppSettings> {
    state
        .database
        .load_settings()?
        .ok_or_else(|| AppError::message("application settings are missing"))
}

async fn current_session(
    state: &AppState,
    settings: &crate::models::AppSettings,
) -> AppResult<crate::models::SessionRecord> {
    current_session_opt(state, settings)
        .await
        .ok_or_else(|| AppError::message("not logged in"))
}

async fn current_session_opt(
    state: &AppState,
    settings: &crate::models::AppSettings,
) -> Option<crate::models::SessionRecord> {
    let session = state.database.load_session().ok()??;
    match crate::auth::refresh_session_if_needed(&state.database, &state.http, settings, session)
        .await
    {
        Ok(session) => Some(session),
        Err(error) => {
            tracing::warn!(%error, "notes: session refresh failed");
            None
        }
    }
}

fn notify_notes_changed(app: &AppHandle) {
    let _ = app.emit(NOTES_UPDATED_EVENT, ());
}

struct NotesUpdateOnDrop<'a>(&'a AppHandle);

impl Drop for NotesUpdateOnDrop<'_> {
    fn drop(&mut self) {
        notify_notes_changed(self.0);
    }
}
