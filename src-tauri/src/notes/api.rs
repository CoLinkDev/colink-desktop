use serde::{de::DeserializeOwned, Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use std::time::Duration;
use tokio_util::io::ReaderStream;
use url::Url;

use crate::error::{AppError, AppResult};

pub(crate) const NOTES_PATH: &str = "/api/v1/notes";
pub(crate) const NOTE_TAGS_PATH: &str = "/api/v1/note-tags";
pub(crate) const NOTE_ATTACHMENTS_PATH: &str = "/api/v1/note-attachments";
pub(crate) const NOTES_SNAPSHOT_PATH: &str = "/api/v1/notes/sync/snapshot";
pub(crate) const NOTES_CHANGES_PATH: &str = "/api/v1/notes/sync/changes";
pub(crate) const NOTES_STORAGE_PATH: &str = "/api/v1/notes/storage";

pub(crate) const CODE_NOTE_NOT_FOUND: i32 = 6001;
pub(crate) const CODE_REVISION_CONFLICT: i32 = 6002;
pub(crate) const CODE_TAG_NOT_FOUND: i32 = 6003;
pub(crate) const CODE_ATTACHMENT_NOT_FOUND: i32 = 6005;
pub(crate) const CODE_INVALID_NOTE_REFERENCE: i32 = 6008;
pub(crate) const CODE_SYNC_CURSOR_EXPIRED: i32 = 6009;
pub(crate) const CODE_ATTACHMENT_ID_UNAVAILABLE: i32 = 6011;

const CLIENT_TIMEOUT: Duration = Duration::from_secs(120);

/// Dedicated HTTP client for notes: attachment uploads and downloads can
/// exceed the short default timeout used for the interactive APIs.
#[derive(Clone)]
pub(crate) struct NotesHttpClient {
    client: reqwest::Client,
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    code: i32,
    data: Option<T>,
    message: String,
}

impl NotesHttpClient {
    pub fn new() -> AppResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(CLIENT_TIMEOUT)
            .build()?;
        Ok(Self { client })
    }

    pub async fn get<T>(&self, base_url: &str, path: &str, token: &str) -> AppResult<T>
    where
        T: DeserializeOwned,
    {
        let request = self
            .client
            .get(endpoint(base_url, path)?)
            .bearer_auth(token);
        self.send_data(request).await
    }

    /// Performs a GET against an already-built absolute URL (used for
    /// requests with query strings such as sync pagination).
    pub async fn get_raw<T>(&self, url: &str, token: &str) -> AppResult<T>
    where
        T: DeserializeOwned,
    {
        let request = self.client.get(url.to_string()).bearer_auth(token);
        self.send_data(request).await
    }

    pub async fn post<Req, Res>(
        &self,
        base_url: &str,
        path: &str,
        body: &Req,
        token: &str,
    ) -> AppResult<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let request = self
            .client
            .post(endpoint(base_url, path)?)
            .bearer_auth(token)
            .json(body);
        self.send_data(request).await
    }

    pub async fn put<Req, Res>(
        &self,
        base_url: &str,
        path: &str,
        body: &Req,
        token: &str,
    ) -> AppResult<Res>
    where
        Req: Serialize + ?Sized,
        Res: DeserializeOwned,
    {
        let request = self
            .client
            .put(endpoint(base_url, path)?)
            .bearer_auth(token)
            .json(body);
        self.send_data(request).await
    }

    pub async fn delete<Res>(&self, base_url: &str, path: &str, token: &str) -> AppResult<Res>
    where
        Res: DeserializeOwned,
    {
        let request = self
            .client
            .delete(endpoint(base_url, path)?)
            .bearer_auth(token);
        self.send_data(request).await
    }

    pub async fn delete_ok(&self, base_url: &str, path: &str, token: &str) -> AppResult<()> {
        let request = self
            .client
            .delete(endpoint(base_url, path)?)
            .bearer_auth(token);
        self.send_ok(request).await
    }

    pub async fn upload_attachment(
        &self,
        base_url: &str,
        path: &str,
        token: &str,
        attachment_id: &str,
        kind: &str,
        sha256: &str,
        file_name: &str,
        file_path: &Path,
        size: u64,
    ) -> AppResult<AttachmentDto> {
        let media_type = infer_media_type(file_name, kind);
        let file = tokio::fs::File::open(file_path).await?;
        let body = reqwest::Body::wrap_stream(ReaderStream::new(file));
        let file_part = reqwest::multipart::Part::stream_with_length(body, size)
            .file_name(file_name.to_string())
            .mime_str(&media_type)?;
        let form = reqwest::multipart::Form::new()
            .text("attachmentId", attachment_id.to_string())
            .text("kind", kind.to_string())
            .text("sha256", sha256.to_string())
            .part("file", file_part);

        let request = self
            .client
            .post(endpoint(base_url, path)?)
            .bearer_auth(token)
            .multipart(form);
        self.send_data(request).await
    }

    /// Downloads raw attachment content. Returns 304 when the provided
    /// ETag (if any) still matches.
    pub async fn download_attachment(
        &self,
        base_url: &str,
        path: &str,
        token: &str,
        if_none_match: Option<&str>,
    ) -> AppResult<Option<reqwest::Response>> {
        let mut request = self
            .client
            .get(endpoint(base_url, path)?)
            .bearer_auth(token);
        if let Some(etag) = if_none_match {
            request = request.header("If-None-Match", format!("\"{etag}\""));
        }

        let response = request.send().await?;
        let status = response.status();

        if status == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        if !status.is_success() {
            let payload = response.text().await?;
            return Err(parse_error_envelope(status, &payload));
        }

        Ok(Some(response))
    }

    async fn send_data<T>(&self, request: reqwest::RequestBuilder) -> AppResult<T>
    where
        T: DeserializeOwned,
    {
        let response = request.send().await?;
        let status = response.status();
        let payload = response.text().await?;

        if !status.is_success() {
            return Err(parse_error_envelope(status, &payload));
        }

        let envelope: ApiEnvelope<T> = serde_json::from_str(&payload)?;
        if envelope.code != 0 {
            return Err(AppError::protocol(envelope.code, envelope.message));
        }

        envelope
            .data
            .ok_or_else(|| AppError::message("response data is missing"))
    }

    async fn send_ok(&self, request: reqwest::RequestBuilder) -> AppResult<()> {
        let response = request.send().await?;
        let status = response.status();
        let payload = response.text().await?;

        if !status.is_success() {
            return Err(parse_error_envelope(status, &payload));
        }

        let envelope: ApiEnvelope<serde_json::Value> = serde_json::from_str(&payload)?;
        if envelope.code != 0 {
            return Err(AppError::protocol(envelope.code, envelope.message));
        }
        Ok(())
    }
}

fn parse_error_envelope(status: reqwest::StatusCode, payload: &str) -> AppError {
    if let Ok(envelope) = serde_json::from_str::<ApiEnvelope<serde_json::Value>>(payload) {
        if envelope.code != 0 {
            return AppError::protocol(envelope.code, envelope.message);
        }
    }

    AppError::message(format!("request failed with status {status}"))
}

fn endpoint(base_url: &str, path: &str) -> AppResult<String> {
    let base = Url::parse(base_url)?;
    Ok(base.join(path.trim_start_matches('/'))?.to_string())
}

fn infer_media_type(file_name: &str, kind: &str) -> String {
    let extension = file_name
        .rsplit('.')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();

    let guessed = match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "zip" => "application/zip",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        _ => match kind {
            "image" => "image/png",
            _ => "application/octet-stream",
        },
    };

    guessed.to_string()
}

pub(crate) fn sha256_hex(content: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content);
    let digest = hasher.finalize();
    let mut output = String::with_capacity(digest.len() * 2);
    for byte in digest {
        use std::fmt::Write;
        let _ = write!(output, "{byte:02x}");
    }
    output
}

// ---------- DTOs ----------

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoteDto {
    pub note_id: String,
    pub title: String,
    pub markdown: String,
    pub tag_ids: Vec<String>,
    pub attachments: Vec<AttachmentDto>,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AttachmentDto {
    pub attachment_id: String,
    pub kind: String,
    pub file_name: String,
    pub media_type: String,
    pub size: i64,
    pub sha256: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TagDto {
    pub tag_id: String,
    pub name: String,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TagListDto {
    pub tags: Vec<TagDto>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateNoteRequest<'a> {
    pub note_id: &'a str,
    pub title: &'a str,
    pub markdown: &'a str,
    pub tag_ids: &'a [String],
    pub attachment_ids: &'a [String],
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateNoteRequest<'a> {
    pub base_revision: i64,
    pub title: &'a str,
    pub markdown: &'a str,
    pub tag_ids: &'a [String],
    pub attachment_ids: &'a [String],
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct NoteDeleteDto {
    pub note_id: String,
    pub revision: i64,
    #[serde(default)]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TagDeleteDto {
    pub tag_id: String,
    pub revision: i64,
    #[serde(default)]
    pub deleted_at: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotDto {
    pub notes: Vec<NoteDto>,
    pub tags: Vec<TagDto>,
    pub next_page_token: Option<String>,
    pub cursor: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChangeEntryDto {
    #[serde(rename = "type")]
    pub resource_type: String,
    pub operation: String,
    #[serde(default)]
    pub note: Option<NoteDto>,
    #[serde(default)]
    pub tag: Option<TagDto>,
    #[serde(default)]
    pub note_id: Option<String>,
    #[serde(default)]
    pub tag_id: Option<String>,
    #[serde(default)]
    pub revision: Option<i64>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChangesDto {
    pub changes: Vec<ChangeEntryDto>,
    pub next_cursor: String,
    pub has_more: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageDto {
    pub used_bytes: i64,
    pub limit_bytes: i64,
    pub remaining_bytes: i64,
    pub attachment_bytes: i64,
    pub markdown_bytes: i64,
    pub max_attachment_bytes: i64,
    pub max_markdown_bytes: i64,
}
