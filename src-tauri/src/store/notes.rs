use std::collections::{HashMap, HashSet};

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;
use uuid::Uuid;

use crate::{
    error::AppResult,
    models::{unix_now_millis, AppSettings, SessionRecord},
};

use super::db::Database;

pub const NOTES_SYNC_CURSOR_KEY: &str = "notes_sync_cursor";
pub const LOCAL_ACCOUNT_SCOPE: &str = "__local__";

pub const NOTE_STATE_SYNCED: &str = "synced";
pub const NOTE_STATE_PENDING: &str = "pending";
pub const NOTE_STATE_PENDING_DELETE: &str = "pendingDelete";
pub const NOTE_STATE_CONFLICT: &str = "conflict";
pub const NOTE_STATE_CONFLICT_DELETE: &str = "conflictDelete";

pub const CONFLICT_KIND_EDIT: &str = "edit";
pub const CONFLICT_KIND_CLOUD_DELETED: &str = "cloudDeleted";
pub const CONFLICT_KIND_DELETE: &str = "delete";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NoteRecord {
    pub id: String,
    pub title: String,
    pub markdown: String,
    pub tag_ids: Vec<String>,
    pub attachment_ids: Vec<String>,
    pub revision: i64,
    pub base_revision: i64,
    pub sync_state: String,
    pub conflict_kind: Option<String>,
    pub conflict_title: Option<String>,
    pub conflict_markdown: Option<String>,
    pub conflict_tag_ids: Option<Vec<String>>,
    pub conflict_attachment_ids: Option<Vec<String>>,
    pub conflict_revision: Option<i64>,
    pub ancestor_title: String,
    pub ancestor_markdown: String,
    pub ancestor_tag_ids: Vec<String>,
    pub ancestor_attachment_ids: Vec<String>,
    pub ancestor_revision: i64,
    pub deleted: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

impl NoteRecord {
    pub fn new(id: String, created_at: i64) -> Self {
        Self {
            id,
            title: String::new(),
            markdown: String::new(),
            tag_ids: Vec::new(),
            attachment_ids: Vec::new(),
            revision: 0,
            base_revision: 0,
            sync_state: NOTE_STATE_PENDING.to_string(),
            conflict_kind: None,
            conflict_title: None,
            conflict_markdown: None,
            conflict_tag_ids: None,
            conflict_attachment_ids: None,
            conflict_revision: None,
            ancestor_title: String::new(),
            ancestor_markdown: String::new(),
            ancestor_tag_ids: Vec::new(),
            ancestor_attachment_ids: Vec::new(),
            ancestor_revision: 0,
            deleted: false,
            created_at,
            updated_at: created_at,
        }
    }

    pub fn content_matches(
        &self,
        title: &str,
        markdown: &str,
        tag_ids: &[String],
        attachment_ids: &[String],
    ) -> bool {
        self.title == title
            && self.markdown == markdown
            && self.tag_ids == tag_ids
            && self.attachment_ids == attachment_ids
    }

    pub fn has_local_edits(&self) -> bool {
        self.sync_state == NOTE_STATE_PENDING
            || self.sync_state == NOTE_STATE_PENDING_DELETE
            || self.sync_state == NOTE_STATE_CONFLICT
            || self.sync_state == NOTE_STATE_CONFLICT_DELETE
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NoteTagRecord {
    pub id: String,
    pub name: String,
    pub revision: i64,
    pub base_revision: i64,
    pub sync_state: String,
    pub deleted: bool,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct NoteAttachmentRecord {
    pub id: String,
    pub kind: String,
    pub file_name: String,
    pub media_type: String,
    pub size: i64,
    pub sha256: String,
    pub sync_state: String,
    pub deleted: bool,
    pub created_at: i64,
}

fn parse_ids(raw: &str) -> rusqlite::Result<Vec<String>> {
    serde_json::from_str(raw).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            raw.len(),
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn encode_ids(ids: &[String]) -> String {
    let mut normalized = ids.to_vec();
    normalized.sort();
    normalized.dedup();
    serde_json::to_string(&normalized).expect("serializing string ids cannot fail")
}

pub(crate) fn normalize_tag_name(name: &str) -> String {
    let normalized = name.trim().nfc().collect::<String>();
    normalized.case_fold().nfc().collect()
}

pub(crate) fn account_notes_scope(settings: &AppSettings, session: &SessionRecord) -> String {
    format!(
        "{}\n{}",
        settings.server_url.trim().trim_end_matches('/'),
        session.user_id.trim()
    )
}

fn active_notes_scope(connection: &rusqlite::Connection) -> AppResult<String> {
    let settings = connection
        .query_row("SELECT value FROM kv_store WHERE key = 'settings'", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .map(|raw| serde_json::from_str::<AppSettings>(&raw))
        .transpose()?;
    let session = connection
        .query_row("SELECT value FROM kv_store WHERE key = 'session'", [], |row| {
            row.get::<_, String>(0)
        })
        .optional()?
        .map(|raw| serde_json::from_str::<SessionRecord>(&raw))
        .transpose()?;
    Ok(settings
        .zip(session)
        .map(|(settings, session)| account_notes_scope(&settings, &session))
        .unwrap_or_else(|| LOCAL_ACCOUNT_SCOPE.to_string()))
}

fn required_notes_scope(connection: &rusqlite::Connection) -> AppResult<String> {
    active_notes_scope(connection)
}

const NOTE_TAG_COLUMNS: &str =
    "id, name, revision, base_revision, sync_state, deleted, created_at, updated_at";

fn row_to_tag(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteTagRecord> {
    Ok(NoteTagRecord {
        id: row.get("id")?,
        name: row.get("name")?,
        revision: row.get("revision")?,
        base_revision: row.get("base_revision")?,
        sync_state: row.get("sync_state")?,
        deleted: row.get::<_, i64>("deleted")? != 0,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

const NOTE_ATTACHMENT_COLUMNS: &str =
    "id, kind, file_name, media_type, size, sha256, sync_state, deleted, created_at";

fn row_to_attachment(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteAttachmentRecord> {
    Ok(NoteAttachmentRecord {
        id: row.get("id")?,
        kind: row.get("kind")?,
        file_name: row.get("file_name")?,
        media_type: row.get("media_type")?,
        size: row.get("size")?,
        sha256: row.get("sha256")?,
        sync_state: row.get("sync_state")?,
        deleted: row.get::<_, i64>("deleted")? != 0,
        created_at: row.get("created_at")?,
    })
}

fn row_to_note(row: &rusqlite::Row<'_>) -> rusqlite::Result<NoteRecord> {
    Ok(NoteRecord {
        id: row.get("id")?,
        title: row.get("title")?,
        markdown: row.get("markdown")?,
        tag_ids: parse_ids(&row.get::<_, String>("tag_ids")?)?,
        attachment_ids: parse_ids(&row.get::<_, String>("attachment_ids")?)?,
        revision: row.get("revision")?,
        base_revision: row.get("base_revision")?,
        sync_state: row.get("sync_state")?,
        conflict_kind: row.get("conflict_kind")?,
        conflict_title: row.get("conflict_title")?,
        conflict_markdown: row.get("conflict_markdown")?,
        conflict_tag_ids: row
            .get::<_, Option<String>>("conflict_tag_ids")?
            .map(|raw| parse_ids(&raw))
            .transpose()?,
        conflict_attachment_ids: row
            .get::<_, Option<String>>("conflict_attachment_ids")?
            .map(|raw| parse_ids(&raw))
            .transpose()?,
        conflict_revision: row.get("conflict_revision")?,
        ancestor_title: row.get("ancestor_title")?,
        ancestor_markdown: row.get("ancestor_markdown")?,
        ancestor_tag_ids: parse_ids(&row.get::<_, String>("ancestor_tag_ids")?)?,
        ancestor_attachment_ids: parse_ids(&row.get::<_, String>("ancestor_attachment_ids")?)?,
        ancestor_revision: row.get("ancestor_revision")?,
        deleted: row.get::<_, i64>("deleted")? != 0,
        created_at: row.get("created_at")?,
        updated_at: row.get("updated_at")?,
    })
}

const NOTE_COLUMNS: &str = "id, title, markdown, tag_ids, attachment_ids, revision, base_revision, sync_state, conflict_kind, conflict_title, conflict_markdown, conflict_tag_ids, conflict_attachment_ids, conflict_revision, ancestor_title, ancestor_markdown, ancestor_tag_ids, ancestor_attachment_ids, ancestor_revision, deleted, created_at, updated_at";

pub(crate) struct NotesTransaction<'a> {
    transaction: &'a Transaction<'a>,
    scope: String,
}

impl NotesTransaction<'_> {
    pub(crate) fn load_note(&self, id: &str) -> AppResult<Option<NoteRecord>> {
        Ok(self
            .transaction
            .query_row(
                &format!("SELECT {NOTE_COLUMNS} FROM notes WHERE account_scope = ?1 AND id = ?2"),
                params![self.scope, id],
                row_to_note,
            )
            .optional()?)
    }

    pub(crate) fn load_all_note_rows(&self) -> AppResult<Vec<NoteRecord>> {
        let mut statement = self.transaction.prepare(&format!(
            "SELECT {NOTE_COLUMNS} FROM notes WHERE account_scope = ?1"
        ))?;
        let records = statement
            .query_map(params![self.scope], row_to_note)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub(crate) fn save_note(&self, record: &NoteRecord) -> AppResult<()> {
        save_note_on(self.transaction, &self.scope, record)
    }

    pub(crate) fn delete_note_row(&self, id: &str) -> AppResult<()> {
        self.transaction.execute(
            "DELETE FROM notes WHERE account_scope = ?1 AND id = ?2",
            params![self.scope, id],
        )?;
        Ok(())
    }

    pub(crate) fn load_note_tag(&self, id: &str) -> AppResult<Option<NoteTagRecord>> {
        Ok(self
            .transaction
            .query_row(
                &format!("SELECT {NOTE_TAG_COLUMNS} FROM note_tags WHERE account_scope = ?1 AND id = ?2"),
                params![self.scope, id],
                row_to_tag,
            )
            .optional()?)
    }

    pub(crate) fn load_all_note_tag_rows(&self) -> AppResult<Vec<NoteTagRecord>> {
        let mut statement = self.transaction.prepare(&format!(
            "SELECT {NOTE_TAG_COLUMNS} FROM note_tags WHERE account_scope = ?1"
        ))?;
        let records = statement
            .query_map(params![self.scope], row_to_tag)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub(crate) fn save_note_tag(&self, record: &NoteTagRecord) -> AppResult<()> {
        save_note_tag_on(self.transaction, &self.scope, record)
    }

    pub(crate) fn delete_note_tag_row(&self, id: &str) -> AppResult<()> {
        self.transaction.execute(
            "DELETE FROM note_tags WHERE account_scope = ?1 AND id = ?2",
            params![self.scope, id],
        )?;
        Ok(())
    }

    pub(crate) fn load_note_attachment(&self, id: &str) -> AppResult<Option<NoteAttachmentRecord>> {
        Ok(self
            .transaction
            .query_row(
                &format!("SELECT {NOTE_ATTACHMENT_COLUMNS} FROM note_attachments WHERE account_scope = ?1 AND id = ?2"),
                params![self.scope, id],
                row_to_attachment,
            )
            .optional()?)
    }

    pub(crate) fn save_note_attachment(&self, record: &NoteAttachmentRecord) -> AppResult<()> {
        save_note_attachment_on(self.transaction, &self.scope, record)
    }

    pub(crate) fn delete_note_attachment_row(&self, id: &str) -> AppResult<()> {
        self.transaction.execute(
            "DELETE FROM note_attachments WHERE account_scope = ?1 AND id = ?2",
            params![self.scope, id],
        )?;
        Ok(())
    }

    pub(crate) fn save_cursor(&self, cursor: &str) -> AppResult<()> {
        self.transaction.execute(
            "INSERT INTO kv_store (key, value, updated_at) VALUES (?1, ?2, ?3) ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
            params![format!("{NOTES_SYNC_CURSOR_KEY}:{}", self.scope), cursor, unix_now_millis()],
        )?;
        Ok(())
    }
}

fn save_note_on(
    connection: &rusqlite::Connection,
    scope: &str,
    record: &NoteRecord,
) -> AppResult<()> {
    connection.execute(
        "INSERT INTO notes (account_scope, id, title, markdown, tag_ids, attachment_ids, revision, base_revision, sync_state, conflict_kind, conflict_title, conflict_markdown, conflict_tag_ids, conflict_attachment_ids, conflict_revision, ancestor_title, ancestor_markdown, ancestor_tag_ids, ancestor_attachment_ids, ancestor_revision, deleted, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23)
         ON CONFLICT(account_scope, id) DO UPDATE SET title=excluded.title, markdown=excluded.markdown, tag_ids=excluded.tag_ids, attachment_ids=excluded.attachment_ids, revision=excluded.revision, base_revision=excluded.base_revision, sync_state=excluded.sync_state, conflict_kind=excluded.conflict_kind, conflict_title=excluded.conflict_title, conflict_markdown=excluded.conflict_markdown, conflict_tag_ids=excluded.conflict_tag_ids, conflict_attachment_ids=excluded.conflict_attachment_ids, conflict_revision=excluded.conflict_revision, ancestor_title=excluded.ancestor_title, ancestor_markdown=excluded.ancestor_markdown, ancestor_tag_ids=excluded.ancestor_tag_ids, ancestor_attachment_ids=excluded.ancestor_attachment_ids, ancestor_revision=excluded.ancestor_revision, deleted=excluded.deleted, updated_at=excluded.updated_at",
        params![scope, record.id, record.title, record.markdown, encode_ids(&record.tag_ids), encode_ids(&record.attachment_ids), record.revision, record.base_revision, record.sync_state, record.conflict_kind, record.conflict_title, record.conflict_markdown, record.conflict_tag_ids.as_ref().map(|ids| encode_ids(ids)), record.conflict_attachment_ids.as_ref().map(|ids| encode_ids(ids)), record.conflict_revision, record.ancestor_title, record.ancestor_markdown, encode_ids(&record.ancestor_tag_ids), encode_ids(&record.ancestor_attachment_ids), record.ancestor_revision, record.deleted as i64, record.created_at, record.updated_at],
    )?;
    Ok(())
}

fn save_note_tag_on(connection: &rusqlite::Connection, scope: &str, record: &NoteTagRecord) -> AppResult<()> {
    connection.execute(
        "INSERT INTO note_tags (account_scope, id, name, name_normalized, revision, base_revision, sync_state, deleted, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(account_scope, id) DO UPDATE SET name=excluded.name, name_normalized=excluded.name_normalized, revision=excluded.revision, base_revision=excluded.base_revision, sync_state=excluded.sync_state, deleted=excluded.deleted, updated_at=excluded.updated_at",
        params![scope, record.id, record.name, normalize_tag_name(&record.name), record.revision, record.base_revision, record.sync_state, record.deleted as i64, record.created_at, record.updated_at],
    )?;
    Ok(())
}

fn save_note_attachment_on(connection: &rusqlite::Connection, scope: &str, record: &NoteAttachmentRecord) -> AppResult<()> {
    connection.execute(
        "INSERT INTO note_attachments (account_scope, id, kind, file_name, media_type, size, sha256, sync_state, deleted, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10) ON CONFLICT(account_scope, id) DO UPDATE SET kind=excluded.kind, file_name=excluded.file_name, media_type=excluded.media_type, size=excluded.size, sha256=excluded.sha256, sync_state=excluded.sync_state, deleted=excluded.deleted",
        params![scope, record.id, record.kind, record.file_name, record.media_type, record.size, record.sha256, record.sync_state, record.deleted as i64, record.created_at],
    )?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClaimedAttachment {
    pub source_id: String,
    pub target_id: String,
}

fn remap_ids(ids: Vec<String>, mapping: &HashMap<String, String>) -> Vec<String> {
    let mut result = ids
        .into_iter()
        .map(|id| mapping.get(&id).cloned().unwrap_or(id))
        .collect::<Vec<_>>();
    result.sort();
    result.dedup();
    result
}

pub(crate) fn transfer_notes_scope(
    transaction: &Transaction<'_>,
    source_scope: &str,
    target_scope: &str,
) -> AppResult<Vec<ClaimedAttachment>> {
    if source_scope == target_scope {
        return Ok(Vec::new());
    }

    let mut target_tag_names = {
        let mut statement = transaction.prepare(
            "SELECT name_normalized, id FROM note_tags WHERE account_scope = ?1 AND deleted = 0 AND sync_state != 'pendingDelete'",
        )?;
        let names = statement
            .query_map(params![target_scope], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<Result<HashMap<_, _>, _>>()?;
        names
    };
    let mut target_tag_ids = load_scope_ids(transaction, "note_tags", "id", target_scope)?;
    let mut target_attachment_ids =
        load_scope_ids(transaction, "note_attachments", "id", target_scope)?;
    let mut target_note_ids = load_scope_ids(transaction, "notes", "id", target_scope)?;

    let source_tags = load_tags_in_scope(transaction, source_scope)?;
    let mut tag_mapping = HashMap::new();
    for mut tag in source_tags {
        let source_id = tag.id.clone();
        if tag.deleted || tag.sync_state == NOTE_STATE_PENDING_DELETE {
            continue;
        }
        let normalized = normalize_tag_name(&tag.name);
        if let Some(target_id) = target_tag_names.get(&normalized) {
            tag_mapping.insert(source_id, target_id.clone());
            continue;
        }
        if target_tag_ids.contains(&tag.id) {
            tag.id = Uuid::new_v4().to_string();
        }
        tag.revision = 0;
        tag.base_revision = 0;
        tag.sync_state = NOTE_STATE_PENDING.to_string();
        tag.deleted = false;
        tag_mapping.insert(source_id, tag.id.clone());
        target_tag_names.insert(normalized, tag.id.clone());
        target_tag_ids.insert(tag.id.clone());
        save_note_tag_on(transaction, target_scope, &tag)?;
    }

    let source_attachments = load_attachments_in_scope(transaction, source_scope)?;
    let mut attachment_mapping = HashMap::new();
    let mut claimed_attachments = Vec::new();
    for mut attachment in source_attachments {
        let source_id = attachment.id.clone();
        if attachment.deleted || attachment.sync_state == NOTE_STATE_PENDING_DELETE {
            continue;
        }
        if target_attachment_ids.contains(&attachment.id) {
            let existing = load_attachment_in_scope(transaction, target_scope, &attachment.id)?;
            if existing.as_ref().is_some_and(|target| {
                target.kind == attachment.kind
                    && target.size == attachment.size
                    && target.sha256 == attachment.sha256
            }) {
                attachment_mapping.insert(source_id.clone(), attachment.id.clone());
                claimed_attachments.push(ClaimedAttachment {
                    source_id,
                    target_id: attachment.id,
                });
                continue;
            }
            attachment.id = Uuid::new_v4().to_string();
        }
        attachment.sync_state = NOTE_STATE_PENDING.to_string();
        attachment.deleted = false;
        attachment_mapping.insert(source_id.clone(), attachment.id.clone());
        target_attachment_ids.insert(attachment.id.clone());
        save_note_attachment_on(transaction, target_scope, &attachment)?;
        claimed_attachments.push(ClaimedAttachment {
            source_id,
            target_id: attachment.id,
        });
    }

    for mut note in load_notes_in_scope(transaction, source_scope)? {
        if note.deleted || note.sync_state == NOTE_STATE_PENDING_DELETE {
            continue;
        }
        if target_note_ids.contains(&note.id) {
            note.id = Uuid::new_v4().to_string();
        }
        note.tag_ids = remap_ids(note.tag_ids, &tag_mapping);
        note.attachment_ids = remap_ids(note.attachment_ids, &attachment_mapping);
        note.revision = 0;
        note.base_revision = 0;
        note.sync_state = NOTE_STATE_PENDING.to_string();
        note.conflict_kind = None;
        note.conflict_title = None;
        note.conflict_markdown = None;
        note.conflict_tag_ids = None;
        note.conflict_attachment_ids = None;
        note.conflict_revision = None;
        note.ancestor_title.clear();
        note.ancestor_markdown.clear();
        note.ancestor_tag_ids.clear();
        note.ancestor_attachment_ids.clear();
        note.ancestor_revision = 0;
        note.deleted = false;
        target_note_ids.insert(note.id.clone());
        save_note_on(transaction, target_scope, &note)?;
    }

    transaction.execute(
        "DELETE FROM notes WHERE account_scope = ?1",
        params![source_scope],
    )?;
    transaction.execute(
        "DELETE FROM note_tags WHERE account_scope = ?1",
        params![source_scope],
    )?;
    transaction.execute(
        "DELETE FROM note_attachments WHERE account_scope = ?1",
        params![source_scope],
    )?;
    transaction.execute(
        "DELETE FROM kv_store WHERE key = ?1",
        params![format!("{NOTES_SYNC_CURSOR_KEY}:{source_scope}")],
    )?;
    if target_scope == LOCAL_ACCOUNT_SCOPE {
        transaction.execute(
            "DELETE FROM kv_store WHERE key = ?1",
            params![format!("{NOTES_SYNC_CURSOR_KEY}:{LOCAL_ACCOUNT_SCOPE}")],
        )?;
    }

    Ok(claimed_attachments)
}

fn load_scope_ids(
    transaction: &Transaction<'_>,
    table: &str,
    id_column: &str,
    scope: &str,
) -> AppResult<HashSet<String>> {
    let mut statement = transaction.prepare(&format!(
        "SELECT {id_column} FROM {table} WHERE account_scope = ?1"
    ))?;
    let ids = statement
        .query_map(params![scope], |row| row.get(0))?
        .collect::<Result<HashSet<_>, _>>()?;
    Ok(ids)
}

fn load_notes_in_scope(
    transaction: &Transaction<'_>,
    scope: &str,
) -> AppResult<Vec<NoteRecord>> {
    let mut statement = transaction.prepare(&format!(
        "SELECT {NOTE_COLUMNS} FROM notes WHERE account_scope = ?1"
    ))?;
    let notes = statement
        .query_map(params![scope], row_to_note)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(notes)
}

fn load_tags_in_scope(
    transaction: &Transaction<'_>,
    scope: &str,
) -> AppResult<Vec<NoteTagRecord>> {
    let mut statement = transaction.prepare(&format!(
        "SELECT {NOTE_TAG_COLUMNS} FROM note_tags WHERE account_scope = ?1"
    ))?;
    let tags = statement
        .query_map(params![scope], row_to_tag)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(tags)
}

fn load_attachments_in_scope(
    transaction: &Transaction<'_>,
    scope: &str,
) -> AppResult<Vec<NoteAttachmentRecord>> {
    let mut statement = transaction.prepare(&format!(
        "SELECT {NOTE_ATTACHMENT_COLUMNS} FROM note_attachments WHERE account_scope = ?1"
    ))?;
    let attachments = statement
        .query_map(params![scope], row_to_attachment)?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(attachments)
}

fn load_attachment_in_scope(
    transaction: &Transaction<'_>,
    scope: &str,
    id: &str,
) -> AppResult<Option<NoteAttachmentRecord>> {
    Ok(transaction
        .query_row(
            &format!(
                "SELECT {NOTE_ATTACHMENT_COLUMNS} FROM note_attachments WHERE account_scope = ?1 AND id = ?2"
            ),
            params![scope, id],
            row_to_attachment,
        )
        .optional()?)
}

impl Database {
    // ---------- notes ----------

    pub(crate) fn with_notes_transaction<T>(
        &self,
        operation: impl FnOnce(&NotesTransaction<'_>) -> AppResult<T>,
    ) -> AppResult<T> {
        let mut connection = self.open()?;
        let scope = required_notes_scope(&connection)?;
        let transaction = connection.transaction()?;
        let result = {
            let store = NotesTransaction {
                transaction: &transaction,
                scope,
            };
            operation(&store)?
        };
        transaction.commit()?;
        Ok(result)
    }

    pub(crate) fn current_notes_scope(&self) -> AppResult<String> {
        let connection = self.open()?;
        required_notes_scope(&connection)
    }

    #[cfg(test)]
    pub(crate) fn claim_local_notes(
        &self,
        target_scope: &str,
    ) -> AppResult<Vec<ClaimedAttachment>> {
        self.transfer_notes_scope(LOCAL_ACCOUNT_SCOPE, target_scope)
    }

    #[cfg(test)]
    pub(crate) fn transfer_notes_scope(
        &self,
        source_scope: &str,
        target_scope: &str,
    ) -> AppResult<Vec<ClaimedAttachment>> {
        self.transfer_notes_scope_with(source_scope, target_scope, |_| Ok(()))
    }

    pub(crate) fn transfer_notes_scope_with<F>(
        &self,
        source_scope: &str,
        target_scope: &str,
        before_commit: F,
    ) -> AppResult<Vec<ClaimedAttachment>>
    where
        F: FnOnce(&[ClaimedAttachment]) -> AppResult<()>,
    {
        let mut connection = self.open()?;
        let transaction = connection.transaction()?;
        let claimed = transfer_notes_scope(&transaction, source_scope, target_scope)?;
        before_commit(&claimed)?;
        transaction.commit()?;
        Ok(claimed)
    }

    pub fn load_notes(&self) -> AppResult<Vec<NoteRecord>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        let mut statement = connection.prepare(&format!(
            "SELECT {NOTE_COLUMNS} FROM notes WHERE account_scope = ?1 AND deleted = 0 AND sync_state != 'pendingDelete' ORDER BY updated_at DESC, id ASC"
        ))?;
        let records = statement
            .query_map(params![scope], row_to_note)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn load_all_note_rows(&self) -> AppResult<Vec<NoteRecord>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        let mut statement =
            connection.prepare(&format!("SELECT {NOTE_COLUMNS} FROM notes WHERE account_scope = ?1"))?;
        let records = statement
            .query_map(params![scope], row_to_note)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn load_note(&self, id: &str) -> AppResult<Option<NoteRecord>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        let record = connection
            .query_row(
                &format!("SELECT {NOTE_COLUMNS} FROM notes WHERE account_scope = ?1 AND id = ?2"),
                params![scope, id],
                row_to_note,
            )
            .optional()?;
        Ok(record)
    }

    pub fn save_note(&self, record: &NoteRecord) -> AppResult<()> {
        let connection = self.open()?;
        let scope = required_notes_scope(&connection)?;
        save_note_on(&connection, &scope, record)
    }

    pub fn delete_note_row(&self, id: &str) -> AppResult<()> {
        let connection = self.open()?;
        let scope = required_notes_scope(&connection)?;
        connection.execute("DELETE FROM notes WHERE account_scope = ?1 AND id = ?2", params![scope, id])?;
        Ok(())
    }

    // ---------- tags ----------

    pub fn load_note_tags(&self) -> AppResult<Vec<NoteTagRecord>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        let mut statement = connection.prepare(&format!(
            "SELECT {NOTE_TAG_COLUMNS} FROM note_tags WHERE account_scope = ?1 AND deleted = 0 AND sync_state != 'pendingDelete' ORDER BY created_at ASC, id ASC"
        ))?;
        let records = statement
            .query_map(params![scope], row_to_tag)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn load_all_note_tag_rows(&self) -> AppResult<Vec<NoteTagRecord>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        let mut statement =
            connection.prepare(&format!("SELECT {NOTE_TAG_COLUMNS} FROM note_tags WHERE account_scope = ?1"))?;
        let records = statement
            .query_map(params![scope], row_to_tag)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn load_note_tag(&self, id: &str) -> AppResult<Option<NoteTagRecord>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        let record = connection
            .query_row(
                &format!("SELECT {NOTE_TAG_COLUMNS} FROM note_tags WHERE account_scope = ?1 AND id = ?2"),
                params![scope, id],
                row_to_tag,
            )
            .optional()?;
        Ok(record)
    }

    pub fn find_note_tag_by_name(&self, name: &str) -> AppResult<Option<NoteTagRecord>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        let record = connection
            .query_row(
                &format!(
                    "SELECT {NOTE_TAG_COLUMNS} FROM note_tags WHERE account_scope = ?1 AND name_normalized = ?2 AND deleted = 0"
                ),
                params![scope, normalize_tag_name(name)],
                row_to_tag,
            )
            .optional()?;
        Ok(record)
    }

    pub fn save_note_tag(&self, record: &NoteTagRecord) -> AppResult<()> {
        let connection = self.open()?;
        let scope = required_notes_scope(&connection)?;
        save_note_tag_on(&connection, &scope, record)
    }

    pub fn delete_note_tag_row(&self, id: &str) -> AppResult<()> {
        let connection = self.open()?;
        let scope = required_notes_scope(&connection)?;
        connection.execute("DELETE FROM note_tags WHERE account_scope = ?1 AND id = ?2", params![scope, id])?;
        Ok(())
    }

    // ---------- attachments ----------

    pub fn load_note_attachments(&self) -> AppResult<Vec<NoteAttachmentRecord>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        let mut statement = connection.prepare(&format!(
            "SELECT {NOTE_ATTACHMENT_COLUMNS} FROM note_attachments WHERE account_scope = ?1 AND deleted = 0 ORDER BY created_at ASC, id ASC"
        ))?;
        let records = statement
            .query_map(params![scope], row_to_attachment)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(records)
    }

    pub fn load_note_attachment(&self, id: &str) -> AppResult<Option<NoteAttachmentRecord>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        let record = connection
            .query_row(
                &format!("SELECT {NOTE_ATTACHMENT_COLUMNS} FROM note_attachments WHERE account_scope = ?1 AND id = ?2"),
                params![scope, id],
                row_to_attachment,
            )
            .optional()?;
        Ok(record)
    }

    pub fn save_note_attachment(&self, record: &NoteAttachmentRecord) -> AppResult<()> {
        let connection = self.open()?;
        let scope = required_notes_scope(&connection)?;
        save_note_attachment_on(&connection, &scope, record)
    }

    pub fn delete_note_attachment_row(&self, id: &str) -> AppResult<()> {
        let connection = self.open()?;
        let scope = required_notes_scope(&connection)?;
        connection.execute("DELETE FROM note_attachments WHERE account_scope = ?1 AND id = ?2", params![scope, id])?;
        Ok(())
    }

    // ---------- sync cursor ----------

    pub fn load_notes_sync_cursor(&self) -> AppResult<Option<String>> {
        let connection = self.open()?;
        let scope = active_notes_scope(&connection)?;
        drop(connection);
        self.load_plain_kv(&format!("{NOTES_SYNC_CURSOR_KEY}:{scope}"))
    }

    #[cfg(test)]
    pub fn save_notes_sync_cursor(&self, cursor: &str) -> AppResult<()> {
        let connection = self.open()?;
        let scope = required_notes_scope(&connection)?;
        drop(connection);
        self.save_plain_kv(&format!("{NOTES_SYNC_CURSOR_KEY}:{scope}"), cursor)
    }

    pub fn clear_notes_sync_cursor(&self) -> AppResult<()> {
        let connection = self.open()?;
        let scope = required_notes_scope(&connection)?;
        drop(connection);
        self.delete_plain_kv(&format!("{NOTES_SYNC_CURSOR_KEY}:{scope}"))
    }
}

pub(crate) fn now_millis() -> i64 {
    unix_now_millis()
}
