use base64::{engine::general_purpose::STANDARD, Engine};
use clipboard_rs::{
    common::RustImage, Clipboard, ClipboardContext, ClipboardHandler, RustImageData,
};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::{
    error::{AppError, AppResult},
    i18n::{self, TextKey},
    models::CLIPBOARD_MAX_BYTES,
    protocol::ClipboardSyncPayload,
    runtime_events::RuntimeEvent,
};

pub(super) struct ClipboardWatcherHandler {
    pub(super) ctx: ClipboardContext,
    pub(super) event_tx: mpsc::UnboundedSender<RuntimeEvent>,
}

impl ClipboardHandler for ClipboardWatcherHandler {
    fn on_clipboard_change(&mut self) {
        if let Ok(payload) = read_clipboard_payload(&self.ctx) {
            let _ = self.event_tx.send(RuntimeEvent::ClipboardChanged(payload));
        }
    }
}

pub(super) fn clipboard_image_from_bytes(bytes: &[u8]) -> AppResult<RustImageData> {
    RustImageData::from_bytes(bytes).map_err(|error| AppError::message(error.to_string()))
}

pub(super) fn hash_clipboard_payload(payload: &ClipboardSyncPayload) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload.content_type.as_bytes());
    if let Some(content) = payload.content.as_ref() {
        hasher.update(content.as_bytes());
    }
    if let Some(data) = payload.data.as_ref() {
        hasher.update(data.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

fn read_clipboard_payload(ctx: &ClipboardContext) -> AppResult<ClipboardSyncPayload> {
    if let Ok(html) = ctx.get_html() {
        let trimmed = html.trim().to_string();
        if !trimmed.is_empty() && trimmed.len() <= CLIPBOARD_MAX_BYTES {
            return Ok(ClipboardSyncPayload {
                content_type: "text/html".to_string(),
                content: Some(trimmed),
                data: None,
            });
        }
    }

    if let Ok(text) = ctx.get_text() {
        let trimmed = text.trim().to_string();
        if !trimmed.is_empty() && trimmed.len() <= CLIPBOARD_MAX_BYTES {
            return Ok(ClipboardSyncPayload {
                content_type: "text/plain".to_string(),
                content: Some(trimmed),
                data: None,
            });
        }
    }

    if let Ok(image) = ctx.get_image() {
        let png = image
            .to_png()
            .map_err(|error| AppError::message(error.to_string()))?;
        if png.get_bytes().len() <= CLIPBOARD_MAX_BYTES {
            return Ok(ClipboardSyncPayload {
                content_type: "image/png".to_string(),
                content: None,
                data: Some(STANDARD.encode(png.get_bytes())),
            });
        }
    }

    Err(AppError::message(i18n::text(
        &i18n::default_language_code(),
        TextKey::ClipboardUnsupported,
    )))
}
