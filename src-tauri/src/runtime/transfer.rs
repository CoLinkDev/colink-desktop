use std::{fs, io::SeekFrom, path::PathBuf, sync::Arc};

use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use rand::{rngs::OsRng, RngCore};
use sanitize_filename::sanitize;
use tauri::Emitter;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Mutex as AsyncMutex, Notify},
    time::{sleep, Duration},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    i18n::TextKey,
    models::{
        unix_now_millis, FileOfferDecisionPayload, FileOfferRequest, FileTransferRecord,
        SendFilePayload, FILE_CHUNK_SIZE,
    },
    protocol::{
        supports_business_protocol_at_least, BusinessEnvelope, FileAcceptPayload, FileAckPayload,
        FileCancelPayload, FileChunkPayload, FileDataFrame, FileDataFrameKind, FileDonePayload,
        FileFinishPayload, FileOfferPayload, FileReadyPayload, FileRejectPayload,
        FileRetransmitPayload, FileV3AcceptPayload, FileV3OfferPayload, FileV3ReadyPayload,
        FILE_ACCEPT_TYPE, FILE_ACK_TYPE, FILE_CANCEL_TYPE, FILE_CHUNK_TYPE, FILE_DONE_TYPE,
        FILE_OFFER_TYPE, FILE_READY_TYPE, FILE_REJECT_TYPE, FILE_RETRANSMIT_TYPE,
        FILE_V3_ACCEPT_TYPE, FILE_V3_ACK_TYPE, FILE_V3_CANCEL_TYPE, FILE_V3_CHUNK_TYPE,
        FILE_V3_DONE_TYPE, FILE_V3_FINISH_TYPE, FILE_V3_OFFER_TYPE, FILE_V3_READY_TYPE,
        FILE_V3_REJECT_TYPE, FILE_V3_RETRANSMIT_TYPE,
    },
    sync::MutexExt,
};

use super::{
    progress::{
        acknowledged_file_bytes, calculate_bytes_per_second, should_send_file_ack,
        TransferPreparingPayload, TransferProgressPayload,
    },
    route::TransferRoute,
    filesystem::{commit_filesystem_upload, create_filesystem_upload_temp},
    utils::{
        build_file_checksum_with_algorithm, unique_download_path, verify_file_checksum,
        FileChecksumAlgorithm, FileChecksumVerifier,
    },
    AppRuntime, FileTransferProtocol, IncomingFileState, OutgoingFileState, PendingFileOfferState,
    FILE_OFFER_ENDED_EVENT, FILE_OFFER_REQUESTED_EVENT, FILE_V3_RELAY_ACK_INTERVAL_CHUNKS,
    FILE_V3_RELAY_SEND_WINDOW_CHUNKS, LAN_SEND_WINDOW_CHUNKS, RELAY_SEND_WINDOW_CHUNKS,
    TRANSFER_PREPARING_EVENT, TRANSFER_PROGRESS_EVENT, TRANSFER_PROGRESS_INTERVAL_MS,
};

#[derive(Debug, Clone, Copy)]
enum ChunkTransport {
    Relay,
    Lan,
}

#[cfg(test)]
mod tests {
    use super::{
        transfer_error_text_key, REASON_TRANSFER_CHECKSUM_MISMATCH, REASON_TRANSFER_GENERIC,
        REASON_TRANSFER_STORAGE_FULL, REASON_TRANSFER_USER_CANCELLED, REASON_TRANSFER_USER_REJECTED,
    };
    use crate::i18n;

    #[test]
    fn maps_all_defined_transfer_reason_codes() {
        let cases = [
            (REASON_TRANSFER_USER_CANCELLED, "Transfer cancelled"),
            (REASON_TRANSFER_USER_REJECTED, "Recipient rejected the transfer"),
            (REASON_TRANSFER_CHECKSUM_MISMATCH, "File checksum verification failed"),
            (REASON_TRANSFER_STORAGE_FULL, "Storage full at destination"),
            (REASON_TRANSFER_GENERIC, "Transfer failed"),
        ];

        for (reason, expected) in cases {
            let key = transfer_error_text_key(reason).expect("defined transfer reason");
            assert_eq!(i18n::text("en", key), expected);
        }
    }

    #[test]
    fn localizes_protocol_reason_codes_in_each_supported_language() {
        let key = transfer_error_text_key(REASON_TRANSFER_USER_CANCELLED)
            .expect("defined transfer reason");
        for language in ["zh-CN", "zh-TW", "ja", "ko", "de", "es", "ru"] {
            assert_ne!(i18n::text(language, key), "Transfer cancelled");
        }
    }

    #[test]
    fn preserves_unknown_reason_codes_for_forward_compatibility() {
        assert!(transfer_error_text_key("colink:transfer.future_reason.v1").is_none());
    }
}

impl FileTransferProtocol {
    fn offer_type(self) -> &'static str {
        match self {
            Self::V2 => FILE_OFFER_TYPE,
            Self::V3 => FILE_V3_OFFER_TYPE,
        }
    }

    fn accept_type(self) -> &'static str {
        match self {
            Self::V2 => FILE_ACCEPT_TYPE,
            Self::V3 => FILE_V3_ACCEPT_TYPE,
        }
    }

    fn reject_type(self) -> &'static str {
        match self {
            Self::V2 => FILE_REJECT_TYPE,
            Self::V3 => FILE_V3_REJECT_TYPE,
        }
    }

    fn cancel_type(self) -> &'static str {
        match self {
            Self::V2 => FILE_CANCEL_TYPE,
            Self::V3 => FILE_V3_CANCEL_TYPE,
        }
    }

    fn done_type(self) -> &'static str {
        match self {
            Self::V2 => FILE_DONE_TYPE,
            Self::V3 => FILE_V3_DONE_TYPE,
        }
    }
}

const REASON_TRANSFER_USER_CANCELLED: &str = "colink:transfer.user_cancelled.v1";
const REASON_TRANSFER_USER_REJECTED: &str = "colink:transfer.user_rejected.v1";
const REASON_TRANSFER_CHECKSUM_MISMATCH: &str = "colink:transfer.checksum_mismatch.v1";
const REASON_TRANSFER_STORAGE_FULL: &str = "colink:transfer.storage_full.v1";
const REASON_TRANSFER_GENERIC: &str = "colink:transfer.generic.v1";
const FILE_OFFER_TIMEOUT: Duration = Duration::from_secs(60);
const FILE_V3_READY_TIMEOUT: Duration = Duration::from_secs(60);
const FILE_V3_RELAY_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const FILE_V3_RELAY_ACK_INTERVAL: Duration = Duration::from_millis(500);
const FILE_V3_RELAY_RETRANSMIT_TIMEOUT: Duration = Duration::from_secs(2);

fn transfer_error_text_key(reason: &str) -> Option<TextKey> {
    match reason {
        REASON_TRANSFER_USER_CANCELLED => Some(TextKey::TransferUserCancelled),
        REASON_TRANSFER_USER_REJECTED => Some(TextKey::TransferUserRejected),
        REASON_TRANSFER_CHECKSUM_MISMATCH => Some(TextKey::TransferChecksumMismatch),
        REASON_TRANSFER_STORAGE_FULL => Some(TextKey::TransferStorageFull),
        REASON_TRANSFER_GENERIC => Some(TextKey::TransferGeneric),
        _ => None,
    }
}

fn transfer_error_message(runtime: &AppRuntime, reason: &str) -> String {
    transfer_error_text_key(reason)
        .map(|key| runtime.user_text(key))
        .unwrap_or_else(|| reason.to_string())
}

fn generate_file_v3_transfer_token() -> String {
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

fn is_valid_file_v3_transfer_token(token: &str) -> bool {
    token.len() == 43
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        && URL_SAFE_NO_PAD
            .decode(token)
            .is_ok_and(|bytes| bytes.len() == 32)
}

fn select_file_checksum_algorithm(
    peer_business_version: Option<&str>,
) -> AppResult<FileChecksumAlgorithm> {
    match peer_business_version {
        Some(version) if supports_business_protocol_at_least(version, 1, 3, 0) => {
            Ok(FileChecksumAlgorithm::None)
        }
        Some(version) if supports_business_protocol_at_least(version, 1, 2, 0) => {
            Ok(FileChecksumAlgorithm::Blake3)
        }
        Some(_) => Err(AppError::message(
            "peer does not support any file checksum algorithm",
        )),
        None => Ok(FileChecksumAlgorithm::Blake3),
    }
}

impl AppRuntime {
    pub(crate) fn format_transfer_error(&self, reason_or_message: &str) -> String {
        transfer_error_text_key(reason_or_message)
            .map(|key| self.user_text(key))
            .unwrap_or_else(|| reason_or_message.to_string())
    }

    pub(crate) fn format_transfer_records(&self, records: &mut [FileTransferRecord]) {
        for record in records {
            if let Some(error) = record.error.as_deref() {
                record.error = Some(self.format_transfer_error(error));
            }
        }
    }

    fn format_generic_transfer_error(&self, message: &str) -> String {
        if message.trim().is_empty() {
            self.user_text(TextKey::TransferGeneric)
        } else {
            format!("{}: {message}", self.user_text(TextKey::TransferGeneric))
        }
    }

    pub(crate) fn transfer_error_from_peer(
        &self,
        reason: Option<&str>,
        message: Option<&str>,
    ) -> Option<String> {
        let message = message.filter(|value| !value.trim().is_empty());
        match reason {
            Some(REASON_TRANSFER_GENERIC) => Some(
                message
                    .map(|value| self.format_generic_transfer_error(value))
                    .unwrap_or_else(|| self.user_text(TextKey::TransferGeneric)),
            ),
            Some(reason) if reason.starts_with("colink:") => Some(reason.to_string()),
            Some(reason) => Some(message.unwrap_or(reason).to_string()),
            None => message.map(str::to_string),
        }
    }

    pub async fn send_files(&self, payload: SendFilePayload) -> AppResult<Vec<FileTransferRecord>> {
        if payload.paths.is_empty() {
            return Err(AppError::message(self.user_text(TextKey::SelectFiles)));
        }

        let mut records = Vec::new();
        let total = payload.paths.len();
        for (index, raw_path) in payload.paths.into_iter().enumerate() {
            let source_path = PathBuf::from(&raw_path);
            if !source_path.is_file() {
                return Err(AppError::message(
                    self.user_message(TextKey::FileNotFound, &[("path", raw_path)]),
                ));
            }
            self.emit_transfer_preparing(index + 1, total);
            records.push(
                self.send_file_offer_from_path(&payload.device_id, source_path, None)
                    .await?,
            );
        }

        self.emit_transfers()?;
        info!(device_id = %payload.device_id, count = records.len(), "file offers sent");
        Ok(records)
    }

    pub(super) async fn send_file_offer_from_path(
        &self,
        device_id: &str,
        source_path: PathBuf,
        correlation_id: Option<String>,
    ) -> AppResult<FileTransferRecord> {
        if !source_path.is_file() {
            return Err(AppError::message(
                self.user_message(
                    TextKey::FileNotFound,
                    &[("path", source_path.to_string_lossy().to_string())],
                ),
            ));
        }

        let metadata = fs::metadata(&source_path)?;
        let file_size = i64::try_from(metadata.len())
            .map_err(|_| AppError::message("file is too large to transfer"))?;
        let chunk_size = FILE_CHUNK_SIZE as i64;
        let total_chunks = if file_size == 0 {
            0
        } else {
            (file_size + chunk_size - 1) / chunk_size
        };
        if self.inner.lan.is_available(device_id) {
            if let Err(error) = self.inner.lan.ensure_peer_connected(device_id).await {
                warn!(%device_id, %error, "lan peer connection failed before checksum selection");
            }
        }
        let algorithm =
            select_file_checksum_algorithm(self.peer_business_version(device_id).as_deref())?;
        let protocol = self
            .peer_business_version(device_id)
            .as_deref()
            .is_some_and(|version| supports_business_protocol_at_least(version, 1, 15, 0))
            .then_some(FileTransferProtocol::V3)
            .unwrap_or(FileTransferProtocol::V2);
        let checksum = build_file_checksum_with_algorithm(&source_path, algorithm)?;
        let file_name = source_path
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| AppError::message(self.user_text(TextKey::InvalidFileName)))?
            .to_string();
        let file_id = Uuid::new_v4().to_string();
        let created_at = unix_now_millis();
        debug!(
            %device_id,
            path = %source_path.display(),
            file_size,
            "preparing outgoing file transfer"
        );

        let route = if protocol == FileTransferProtocol::V3 {
            if self.inner.lan.peer_business_version(device_id).is_some() {
                TransferRoute::Lan.as_str().to_string()
            } else if self.inner.cloud.is_connected() {
                TransferRoute::Cloud.as_str().to_string()
            } else {
                return Err(AppError::message(self.user_text(TextKey::DeviceNotConnected)));
            }
        } else if self.inner.lan.is_available(device_id) {
            TransferRoute::Lan.as_str().to_string()
        } else {
            TransferRoute::Cloud.as_str().to_string()
        };
        let envelope = match protocol {
            FileTransferProtocol::V2 => BusinessEnvelope::from_payload(
                protocol.offer_type(),
                FileOfferPayload {
                    session_id: file_id.clone(),
                    file_name: file_name.clone(),
                    file_size,
                    total_chunks,
                    chunk_size,
                    checksum: checksum.clone(),
                },
            )?,
            FileTransferProtocol::V3 => BusinessEnvelope::from_payload(
                protocol.offer_type(),
                FileV3OfferPayload {
                    session_id: file_id.clone(),
                    file_name: file_name.clone(),
                    file_size,
                    checksum: checksum.clone(),
                },
            )?,
        };
        let mut record = FileTransferRecord {
            file_id: file_id.clone(),
            device_id: device_id.to_string(),
            direction: "outbound".to_string(),
            file_name,
            file_size,
            transferred_bytes: 0,
            total_chunks,
            status: "offered".to_string(),
            checksum,
            route: route.clone(),
            temp_path: None,
            final_path: Some(source_path.to_string_lossy().to_string()),
            error: None,
            created_at,
            updated_at: created_at,
        };
        self.inner.database.save_transfer(&record)?;
        self.inner.state.lock_unpoisoned().outgoing_files.insert(
            file_id.clone(),
            OutgoingFileState {
                source_path,
                record: record.clone(),
                protocol,
                ack_notify: Arc::new(Notify::new()),
                acknowledged_chunks: 0,
            last_reported_bytes: 0,
            last_progress_at: created_at,
            last_activity_at: created_at,
        },
        );
        self.inner.indicator.add_session();
        self.expire_outgoing_file_offer(file_id.clone());

        let sent_route = match protocol {
            FileTransferProtocol::V3 => {
                self.send_file_v3_control(device_id, &route, envelope, correlation_id)
                    .await
                    .map(|_| route.clone())
            }
            FileTransferProtocol::V2 => {
                self.send_business_message_with_correlation(device_id, envelope, correlation_id)
                    .await
            }
        };
        let sent_route = match sent_route {
            Ok(sent_route) => sent_route,
            Err(error) => {
                let _ = self.finish_outgoing_transfer(
                    &file_id,
                    "failed",
                    Some(self.format_generic_transfer_error(&error.to_string())),
                    None,
                );
                return Err(error);
            }
        };
        if record.route != sent_route {
            record.route = sent_route;
            record.updated_at = unix_now_millis();
            self.inner.database.save_transfer(&record)?;
            if let Some(outgoing) = self
                .inner
                .state
                .lock_unpoisoned()
                .outgoing_files
                .get_mut(&file_id)
            {
                outgoing.record = record.clone();
            }
        }
        Ok(record)
    }

    pub fn cancel_transfer(&self, file_id: &str) -> AppResult<()> {
        let mut outgoing_target = None;
        let mut active_record = None;
        let mut ack_notify = None;
        let mut protocol = FileTransferProtocol::V2;
        let mut control_route = None;
        let mut sessions_removed: usize = 0;
        {
            let mut state = self.inner.state.lock_unpoisoned();
            state.cancelled_files.insert(file_id.to_string());
            if let Some(outgoing) = state.outgoing_files.get(file_id) {
                outgoing_target = Some(outgoing.record.device_id.clone());
                active_record = Some(outgoing.record.clone());
                ack_notify = Some(outgoing.ack_notify.clone());
                protocol = outgoing.protocol;
                control_route = Some(outgoing.record.route.clone());
            }
            if state.outgoing_files.remove(file_id).is_some() {
                sessions_removed += 1;
            }
            if let Some(incoming) = state.incoming_files.remove(file_id) {
                if let Some(temp_path) = incoming.record.temp_path.as_ref() {
                    let _ = fs::remove_file(temp_path);
                }
                outgoing_target = Some(incoming.record.device_id.clone());
                active_record = Some(incoming.record);
                protocol = incoming.protocol;
                control_route = active_record.as_ref().map(|record| record.route.clone());
                sessions_removed += 1;
            }
        }
        for _ in 0..sessions_removed {
            self.inner.indicator.remove_session();
        }
        if let Some(notify) = ack_notify {
            notify.notify_one();
        }

        if let Some(device_id) = outgoing_target {
            let envelope = BusinessEnvelope::from_payload(
                protocol.cancel_type(),
                FileCancelPayload {
                    session_id: file_id.to_string(),
                    reason: REASON_TRANSFER_USER_CANCELLED.to_string(),
                    message: transfer_error_message(self, REASON_TRANSFER_USER_CANCELLED),
                    details: None,
                },
            )?;
            let runtime = self.clone();
            let control_route = control_route.clone();
            tauri::async_runtime::spawn(async move {
                if protocol == FileTransferProtocol::V3 {
                    let Some(route) = control_route else {
                        return;
                    };
                    let _ = runtime
                        .send_file_v3_control(&device_id, &route, envelope, None)
                        .await;
                } else {
                    let _ = runtime.send_business_message(&device_id, envelope).await;
                }
            });
        }
        let _ = self.inner.lan.send_transfer_frame(
            file_id,
            FileDataFrame::cancel(REASON_TRANSFER_USER_CANCELLED),
        );
        self.inner.lan.unregister_transfer(file_id);

        let mut record = match active_record {
            Some(record) => Some(record),
            None => self.inner.database.load_transfer(file_id)?,
        };

        if let Some(record) = record.as_mut() {
            record.status = "cancelled".to_string();
            record.error = Some(self.user_text(TextKey::TransferUserCancelled));
            record.updated_at = unix_now_millis();
            self.inner.database.save_transfer(record)?;
            self.emit_transfers()?;
        }

        info!(%file_id, "file transfer cancelled");
        Ok(())
    }

    pub(super) async fn handle_file_offer(
        &self,
        from: &str,
        route: &str,
        envelope_id: Option<String>,
        correlation_id: Option<String>,
        payload: FileOfferPayload,
        protocol: FileTransferProtocol,
    ) -> AppResult<()> {
        info!(
            from = %from,
            session_id = %payload.session_id,
            file_name = %payload.file_name,
            file_size = payload.file_size,
            route = %route,
            "received file offer"
        );
        let filesystem_download_id = self.associate_remote_filesystem_file_offer(
            from,
            correlation_id.as_deref().or(envelope_id.as_deref()),
            &payload.session_id,
        );
        let filesystem_upload = self.consume_filesystem_upload(
            from,
            correlation_id.as_deref().or(envelope_id.as_deref()),
        );
        if !self.file_checksum_allowed_for_peer(&payload.checksum, from) {
            if let Some(request_id) = filesystem_download_id.as_deref() {
                self.fail_remote_filesystem_download(
                    request_id,
                    "Unsupported file checksum algorithm".to_string(),
                );
            }
            let envelope = BusinessEnvelope::from_payload(
                protocol.reject_type(),
                FileRejectPayload {
                    session_id: payload.session_id,
                    reason: REASON_TRANSFER_GENERIC.to_string(),
                    message: "Unsupported file checksum algorithm".to_string(),
                    details: None,
                },
            )?;
            if protocol == FileTransferProtocol::V3 {
                self.send_file_v3_control(from, route, envelope, envelope_id)
                    .await?;
            } else {
                let _ = self
                    .send_business_message_with_correlation(from, envelope, envelope_id)
                    .await?;
            }
            return Ok(());
        }
        let request = FileOfferRequest {
            session_id: payload.session_id.clone(),
            device_id: from.to_string(),
            device_name: self.lookup_device_name(from),
            file_name: payload.file_name.clone(),
            file_size: payload.file_size,
        };
        let auto_accept_file_offers = self
            .inner
            .database
            .load_settings()?
            .ok_or_else(|| AppError::message(self.user_text(TextKey::SettingsNotInitialized)))?
            .auto_accept_file_offers;
        let session_id = payload.session_id.clone();
        self.inner
            .state
            .lock_unpoisoned()
            .pending_file_offers
            .insert(
                session_id.clone(),
                PendingFileOfferState {
                    from: from.to_string(),
                    route: route.to_string(),
                    envelope_id: envelope_id.clone(),
                    filesystem_download_id: filesystem_download_id.clone(),
                    filesystem_upload: filesystem_upload.clone(),
                    protocol,
                    payload,
                },
            );
        self.expire_pending_file_offer(session_id);
        if filesystem_download_id.is_some() || filesystem_upload.is_some() || auto_accept_file_offers {
            let accepted = self
                .respond_file_offer(FileOfferDecisionPayload {
                    session_id: request.session_id.clone(),
                    accepted: true,
                    destination_path: None,
                })
                .await;
            if let Err(error) = accepted {
                let envelope = BusinessEnvelope::from_payload(
                    protocol.reject_type(),
                    FileRejectPayload {
                        session_id: request.session_id,
                        reason: REASON_TRANSFER_GENERIC.to_string(),
                        message: error.to_string(),
                        details: None,
                    },
                )?;
                if protocol == FileTransferProtocol::V3 {
                    let _ = self
                        .send_file_v3_control(from, route, envelope, envelope_id)
                        .await;
                } else {
                    let _ = self
                        .send_business_message_with_correlation(from, envelope, envelope_id)
                        .await;
                }
                return Err(error);
            }
            return Ok(());
        }
        let destination = if filesystem_download_id.is_some() {
            self.device_route("/files", from)
        } else {
            self.device_route("/transfers", from)
        };
        let _ = crate::shell::show_main_window(&self.inner.app, Some(&destination));
        let _ = self.inner.app.emit(FILE_OFFER_REQUESTED_EVENT, request);
        Ok(())
    }

    fn file_checksum_allowed_for_peer(&self, checksum: &str, device_id: &str) -> bool {
        if FileChecksumVerifier::new(checksum).is_err() {
            return false;
        }
        let algorithm = checksum
            .split_once(':')
            .map(|(algorithm, _)| algorithm.to_ascii_lowercase())
            .unwrap_or_default();
        algorithm != "none"
            || self
                .peer_business_version(device_id)
                .is_some_and(|version| supports_business_protocol_at_least(&version, 1, 3, 0))
    }

    pub(super) fn peer_business_version(&self, device_id: &str) -> Option<String> {
        self.inner
            .lan
            .peer_business_version(device_id)
            .or_else(|| self.inner.cloud.business_version(device_id))
    }

    fn expire_pending_file_offer(&self, session_id: String) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            sleep(FILE_OFFER_TIMEOUT).await;
            let expired = runtime
                .inner
                .state
                .lock_unpoisoned()
                .pending_file_offers
                .remove(&session_id);
            if let Some(pending) = expired {
                let _ = runtime.inner.app.emit(FILE_OFFER_ENDED_EVENT, &session_id);
                if let Some(request_id) = pending.filesystem_download_id.as_deref() {
                    runtime.fail_remote_filesystem_download(
                        request_id,
                        "The file download request expired before it was accepted".to_string(),
                    );
                }
                let _ = runtime
                    .reject_file_offer(
                        &pending.from,
                        &pending.route,
                        pending.payload.session_id,
                        pending.envelope_id,
                        pending.protocol,
                    )
                    .await;
            }
        });
    }

    pub async fn respond_file_offer(&self, decision: FileOfferDecisionPayload) -> AppResult<()> {
        let FileOfferDecisionPayload {
            session_id,
            accepted,
            destination_path,
        } = decision;
        let Some(pending) = self
            .inner
            .state
            .lock_unpoisoned()
            .pending_file_offers
            .remove(&session_id)
        else {
            return Ok(());
        };
        let _ = self
            .inner
            .app
            .emit(FILE_OFFER_ENDED_EVENT, &session_id);

        if accepted {
            let filesystem_download_id = pending.filesystem_download_id.clone();
            let result = self.accept_file_offer(pending, destination_path.as_deref()).await;
            if let Err(error) = result {
                if let Some(request_id) = filesystem_download_id.as_deref() {
                    self.fail_remote_filesystem_download(request_id, error.to_string());
                }
                return Err(error);
            }
            Ok(())
        } else {
            if let Some(request_id) = pending.filesystem_download_id.as_deref() {
                self.fail_remote_filesystem_download(
                    request_id,
                    "The file download was declined".to_string(),
                );
            }
            self.reject_file_offer(
                &pending.from,
                &pending.route,
                pending.payload.session_id,
                pending.envelope_id,
                pending.protocol,
            )
                .await
        }
    }

    pub fn pending_file_offers(&self) -> Vec<FileOfferRequest> {
        let pending = self
            .inner
            .state
            .lock_unpoisoned()
            .pending_file_offers
            .values()
            .cloned()
            .collect::<Vec<_>>();

        pending
            .into_iter()
            .map(|item| FileOfferRequest {
                session_id: item.payload.session_id,
                device_id: item.from.clone(),
                device_name: self.lookup_device_name(&item.from),
                file_name: item.payload.file_name,
                file_size: item.payload.file_size,
            })
            .collect()
    }

    async fn reject_file_offer(
        &self,
        from: &str,
        route: &str,
        session_id: String,
        correlation_id: Option<String>,
        protocol: FileTransferProtocol,
    ) -> AppResult<()> {
        info!(from = %from, session_id = %session_id, "file offer rejected by user");
        let envelope = BusinessEnvelope::from_payload(
            protocol.reject_type(),
            FileRejectPayload {
                session_id,
                reason: REASON_TRANSFER_USER_REJECTED.to_string(),
                    message: transfer_error_message(self, REASON_TRANSFER_USER_REJECTED),
                details: None,
            },
        )?;
        if protocol == FileTransferProtocol::V3 {
            self.send_file_v3_control(from, route, envelope, correlation_id)
                .await?;
        } else {
            let _ = self
                .send_business_message_with_correlation(from, envelope, correlation_id)
                .await?;
        }
        Ok(())
    }

    async fn accept_file_offer(
        &self,
        pending: PendingFileOfferState,
        destination_path: Option<&str>,
    ) -> AppResult<()> {
        let PendingFileOfferState {
            from,
            route,
            envelope_id,
            filesystem_download_id,
            filesystem_upload,
            protocol,
            payload,
        } = pending;
        let settings =
            self.inner.database.load_settings()?.ok_or_else(|| {
                AppError::message(self.user_text(TextKey::SettingsNotInitialized))
            })?;
        let download_path = if let Some(destination) = filesystem_upload.as_ref() {
            destination.parent.clone()
        } else {
            crate::service::validate_receive_directory(destination_path.unwrap_or(&settings.download_path))?
        };
        let verifier = Arc::new(AsyncMutex::new(FileChecksumVerifier::new(
            &payload.checksum,
        )?));
        let temp_name = format!("{}.part", sanitize(&payload.file_name));
        let temp_path = match filesystem_upload.as_ref() {
            Some(destination) => create_filesystem_upload_temp(destination, payload.file_size)
                .map_err(|error| AppError::message(error.message))?,
            None => download_path.join(temp_name),
        };

        let created_at = unix_now_millis();
        let record = FileTransferRecord {
            file_id: payload.session_id.clone(),
            device_id: from.clone(),
            direction: "inbound".to_string(),
            file_name: payload.file_name.clone(),
            file_size: payload.file_size,
            transferred_bytes: 0,
            total_chunks: payload.total_chunks,
            status: "receiving".to_string(),
            checksum: payload.checksum.clone(),
            route: route.clone(),
            temp_path: Some(temp_path.to_string_lossy().to_string()),
            final_path: None,
            error: None,
            created_at,
            updated_at: created_at,
        };
        let writer = Arc::new(AsyncMutex::new(tokio::fs::File::create(&temp_path).await?));
        self.inner.database.save_transfer(&record)?;
        self.inner.state.lock_unpoisoned().incoming_files.insert(
            payload.session_id.clone(),
            IncomingFileState {
                process_lock: Arc::new(AsyncMutex::new(())),
                writer,
                verifier,
                record: record.clone(),
                protocol,
                transfer_token: None,
                v3_ready_received: false,
                received_chunks: 0,
                lan_finish_received: false,
                reorder_buffer: Default::default(),
                gap_detected_at: None,
                last_ack_at: created_at,
                last_acknowledged_chunks: 0,
                last_activity_at: created_at,
                last_reported_bytes: 0,
                last_progress_at: created_at,
                filesystem_upload,
            },
        );

        let transfer_token = (protocol == FileTransferProtocol::V3 && route == "lan")
            .then(generate_file_v3_transfer_token);
        if protocol == FileTransferProtocol::V2 {
            let token = Uuid::new_v4().simple().to_string();
            self.inner.lan.register_transfer_token(&payload.session_id, &token);
            self.inner
                .state
                .lock_unpoisoned()
                .incoming_files
                .get_mut(&payload.session_id)
                .expect("incoming transfer was just created")
                .transfer_token = Some(token);
        } else {
            self.inner
                .state
                .lock_unpoisoned()
                .incoming_files
                .get_mut(&payload.session_id)
                .expect("incoming transfer was just created")
                .transfer_token = transfer_token.clone();
        }
        let envelope = match protocol {
            FileTransferProtocol::V2 => BusinessEnvelope::from_payload(
                protocol.accept_type(),
                FileAcceptPayload {
                    session_id: payload.session_id,
                    transfer_token: self
                        .inner
                        .state
                        .lock_unpoisoned()
                        .incoming_files
                        .get(&record.file_id)
                        .and_then(|state| state.transfer_token.clone())
                        .expect("v2 transfer token is set"),
                },
            )?,
            FileTransferProtocol::V3 => BusinessEnvelope::from_payload(
                protocol.accept_type(),
                FileV3AcceptPayload {
                    session_id: payload.session_id,
                    transfer_token,
                },
            )?,
        };
        let send_result = if protocol == FileTransferProtocol::V3 && route == "lan" {
            self.inner
                .lan
                .send(&from, envelope, None, envelope_id)
                .await
        } else if protocol == FileTransferProtocol::V3 {
            self.inner
                .cloud
                .send_relay(&from, envelope, None, envelope_id)
        } else {
            self.send_business_message_with_correlation(&from, envelope, envelope_id)
                .await
                .map(|_| ())
        };
        if let Err(error) = send_result {
            self.inner
                .state
                .lock_unpoisoned()
                .incoming_files
                .remove(&record.file_id);
            self.inner.lan.unregister_transfer(&record.file_id);
            let _ = tokio::fs::remove_file(&temp_path).await;
            let mut failed = record.clone();
            failed.status = "failed".to_string();
            failed.error = Some(self.format_generic_transfer_error(&error.to_string()));
            failed.updated_at = unix_now_millis();
            let _ = self.inner.database.save_transfer(&failed);
            let _ = self.emit_transfers();
            return Err(error);
        }
        info!(
            from = %from,
            session_id = %record.file_id,
            route = %record.route,
            "file offer accepted"
        );
        self.inner.indicator.add_session();
        if protocol == FileTransferProtocol::V3 && route == TransferRoute::Cloud.as_str() {
            self.watch_file_v3_relay_incoming(record.file_id.clone());
        }
        self.emit_transfers()?;
        let destination = if filesystem_download_id.is_some() {
            self.device_route("/files", &from)
        } else {
            self.device_route("/transfers", &from)
        };
        let _ = crate::shell::show_main_window(&self.inner.app, Some(&destination));
        self.notify(
            TextKey::FileReceiveTitle,
            &[],
            &self.user_message(
                TextKey::FileReceiveStarted,
                &[("file", payload.file_name.clone())],
            ),
        )?;
        if protocol == FileTransferProtocol::V2
            && record.total_chunks == 0
            && TransferRoute::from_str(&record.route) != Some(TransferRoute::Lan)
        {
            self.finish_incoming_transfer(&record.file_id).await?;
        }
        Ok(())
    }

    pub(super) async fn start_file_send(&self, payload: FileAcceptPayload) -> AppResult<()> {
        let file_id = payload.session_id;
        let (source_path, mut record) = {
            let mut state = self.inner.state.lock_unpoisoned();
            let outgoing = state
                .outgoing_files
                .get_mut(&file_id)
                .ok_or_else(|| AppError::message("file send state does not exist"))?;
            let now = unix_now_millis();
            outgoing.record.status = "sending".to_string();
            outgoing.record.updated_at = now;
            outgoing.last_reported_bytes = outgoing.record.transferred_bytes;
            outgoing.last_progress_at = now;
            (outgoing.source_path.clone(), outgoing.record.clone())
        };
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()?;
        info!(file_id = %file_id, device_id = %record.device_id, "starting outgoing file transfer");

        if let Some((ip, port)) = self.lan_endpoint_for_device(&record.device_id) {
            debug!(file_id = %file_id, device_id = %record.device_id, %ip, port = port, "trying lan data connection");
            match self
                .inner
                .lan
                .connect_transfer(&file_id, &payload.transfer_token, &ip, port)
                .await
            {
                Ok(()) => {
                    record.route = TransferRoute::Lan.as_str().to_string();
                    self.update_outgoing_route(&file_id, TransferRoute::Lan)?;
                    info!(file_id = %file_id, "using lan data route for file transfer");
                    let ready = BusinessEnvelope::from_payload(
                        FILE_READY_TYPE,
                        FileReadyPayload {
                            session_id: file_id.clone(),
                        },
                    )?;
                    let _ = self.send_business_message(&record.device_id, ready).await?;
                    return self.send_file_data_lan(file_id, source_path, record).await;
                }
                Err(error) => {
                    let message = format!("{}: {error}", self.user_text(TextKey::TransferLanFailed));
                    warn!(file_id = %file_id, %error, "lan data connection failed");
                    let cancel = BusinessEnvelope::from_payload(
                        FILE_CANCEL_TYPE,
                        FileCancelPayload {
                            session_id: file_id.clone(),
                            reason: REASON_TRANSFER_GENERIC.to_string(),
                            message: message.clone(),
                            details: None,
                        },
                    )?;
                    let _ = self.send_business_message(&record.device_id, cancel).await;
                    self.finish_outgoing_transfer(
                        &file_id,
                        "failed",
                        Some(self.format_generic_transfer_error(&message)),
                        None,
                    )?;
                    return Ok(());
                }
            }
        }

        record.route = TransferRoute::Cloud.as_str().to_string();
        self.update_outgoing_route(&file_id, TransferRoute::Cloud)?;
        info!(file_id = %file_id, "using cloud relay route for file transfer");
        self.send_file_data_relay(file_id, source_path, record)
            .await
    }

    fn expire_outgoing_file_offer(&self, file_id: String) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            sleep(FILE_OFFER_TIMEOUT).await;
            let outgoing = runtime
                .inner
                .state
                .lock_unpoisoned()
                .outgoing_files
                .get(&file_id)
                .filter(|state| state.record.status == "offered")
                .map(|state| state.record.clone());
            if outgoing.is_some() {
                let _ = runtime.finish_outgoing_transfer(
                    &file_id,
                    "failed",
                    Some(runtime.user_text(TextKey::TransferOfferExpired)),
                    None,
                );
            }
        });
    }

    fn watch_file_v3_relay_outgoing(&self, file_id: String) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                sleep(FILE_V3_RELAY_ACK_INTERVAL).await;
                let timed_out = {
                    let state = runtime.inner.state.lock_unpoisoned();
                    let Some(outgoing) = state.outgoing_files.get(&file_id) else {
                        break;
                    };
                    if outgoing.protocol != FileTransferProtocol::V3
                        || outgoing.record.route != TransferRoute::Cloud.as_str()
                    {
                        break;
                    }
                    unix_now_millis() - outgoing.last_activity_at
                        >= FILE_V3_RELAY_IDLE_TIMEOUT.as_millis() as i64
                };
                if timed_out {
                    let device_id = {
                        let state = runtime.inner.state.lock_unpoisoned();
                        state
                            .outgoing_files
                            .get(&file_id)
                            .map(|outgoing| outgoing.record.device_id.clone())
                    };
                    if let Some(device_id) = device_id {
                        let _ = runtime.cancel_file_v3_after_lan_failure(
                            &file_id,
                            &device_id,
                            TransferRoute::Cloud.as_str(),
                            runtime.user_text(TextKey::TransferRelayInactive),
                        );
                    }
                    break;
                }
            }
        });
    }

    fn watch_file_v3_relay_incoming(&self, file_id: String) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            loop {
                sleep(FILE_V3_RELAY_ACK_INTERVAL).await;
                let (device_id, retransmit_index, timed_out) = {
                    let mut state = runtime.inner.state.lock_unpoisoned();
                    let Some(incoming) = state.incoming_files.get_mut(&file_id) else {
                        break;
                    };
                    if incoming.protocol != FileTransferProtocol::V3
                        || incoming.record.route != TransferRoute::Cloud.as_str()
                    {
                        break;
                    }
                    let now = unix_now_millis();
                    if now - incoming.last_activity_at
                        >= FILE_V3_RELAY_IDLE_TIMEOUT.as_millis() as i64
                    {
                        (incoming.record.device_id.clone(), None, true)
                    } else {
                        let retransmit_index = incoming
                            .gap_detected_at
                            .filter(|detected_at| {
                                now - *detected_at
                                    >= FILE_V3_RELAY_RETRANSMIT_TIMEOUT.as_millis() as i64
                            })
                            .map(|_| {
                                incoming.gap_detected_at = Some(now);
                                incoming.received_chunks
                            });
                        (incoming.record.device_id.clone(), retransmit_index, false)
                    }
                };
                if timed_out {
                    let _ = runtime
                        .fail_file_v3_incoming(
                            &file_id,
                            &device_id,
                            runtime.user_text(TextKey::TransferRelayInactive),
                        )
                        .await;
                    break;
                }
                let _ = runtime.maybe_send_file_v3_relay_ack(&file_id).await;
                if let Some(chunk_index) = retransmit_index {
                    let _ = runtime
                        .send_file_retransmit(&device_id, &file_id, chunk_index)
                        .await;
                }
            }
        });
    }

    pub(super) async fn start_file_v3_send(
        &self,
        payload: FileV3AcceptPayload,
        control_route: &str,
    ) -> AppResult<()> {
        let file_id = payload.session_id;
        let (source_path, mut record, protocol) = {
            let mut state = self.inner.state.lock_unpoisoned();
            let outgoing = state
                .outgoing_files
                .get_mut(&file_id)
                .ok_or_else(|| AppError::message("file send state does not exist"))?;
            if outgoing.protocol != FileTransferProtocol::V3 {
                return Err(AppError::message("file.v3.accept does not match the active transfer"));
            }
            if outgoing.record.status != "offered" {
                return Ok(());
            }
            let now = unix_now_millis();
            outgoing.record.status = "sending".to_string();
            outgoing.record.updated_at = now;
            outgoing.last_reported_bytes = outgoing.record.transferred_bytes;
            outgoing.last_progress_at = now;
            outgoing.last_activity_at = now;
            (
                outgoing.source_path.clone(),
                outgoing.record.clone(),
                outgoing.protocol,
            )
        };
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()?;

        if record.route != control_route {
            self.cancel_file_v3_after_lan_failure(
                &file_id,
                &record.device_id,
                &record.route,
                self.user_text(TextKey::TransferRouteMismatch),
            )?;
            return Ok(());
        }

        if control_route == TransferRoute::Lan.as_str()
            && !payload
                .transfer_token
                .as_deref()
                .is_some_and(is_valid_file_v3_transfer_token)
        {
            self.cancel_file_v3_after_lan_failure(
                &file_id,
                &record.device_id,
                TransferRoute::Lan.as_str(),
                self.user_text(TextKey::TransferGeneric),
            )?;
            return Ok(());
        }
        if control_route != TransferRoute::Lan.as_str() && payload.transfer_token.is_some() {
            self.cancel_file_v3_after_lan_failure(
                &file_id,
                &record.device_id,
                control_route,
                self.user_text(TextKey::TransferGeneric),
            )?;
            return Ok(());
        }

        if let Some(token) = payload.transfer_token {
            if !self.inner.lan.is_available(&record.device_id) {
                let message = self.user_text(TextKey::TransferLanFailed);
                self.cancel_file_v3_after_lan_failure(
                    &file_id,
                    &record.device_id,
                    TransferRoute::Lan.as_str(),
                    message,
                )?;
                return Ok(());
            }
            let progress_runtime = self.clone();
            let progress_file_id = file_id.clone();
            let fingerprint = match self
                .inner
                .lan
                .register_file_v3_transfer(
                    &file_id,
                    &token,
                    source_path,
                    move |transferred_bytes| {
                        progress_runtime.report_file_v3_lan_upload_progress(
                            &progress_file_id,
                            transferred_bytes,
                        )
                    },
                )
            {
                Ok(value) => value,
                Err(error) => {
                    self.cancel_file_v3_after_lan_failure(
                        &file_id,
                        &record.device_id,
                        TransferRoute::Lan.as_str(),
                        format!("{}: {error}", self.user_text(TextKey::TransferLanFailed)),
                    )?;
                    return Ok(());
                }
            };
            record.route = TransferRoute::Lan.as_str().to_string();
            self.update_outgoing_route(&file_id, TransferRoute::Lan)?;
            let ready = BusinessEnvelope::from_payload(
                FILE_V3_READY_TYPE,
                FileV3ReadyPayload {
                    session_id: file_id.clone(),
                    cert_fingerprint: fingerprint,
                },
            )?;
            if let Err(error) = self
                .inner
                .transport
                .send_lan_only(&record.device_id, ready)
                .await
            {
                self.cancel_file_v3_after_lan_failure(
                    &file_id,
                    &record.device_id,
                    TransferRoute::Lan.as_str(),
                    format!("{}: {error}", self.user_text(TextKey::TransferLanFailed)),
                )?;
            } else {
                self.expire_file_v3_endpoint(file_id.clone(), record.device_id.clone());
            }
            return Ok(());
        }

        record.route = TransferRoute::Cloud.as_str().to_string();
        self.update_outgoing_route(&file_id, TransferRoute::Cloud)?;
        if let Err(_error) = self
            .send_file_data_relay_v3(file_id.clone(), source_path, record.clone(), protocol)
            .await
        {
            self.cancel_file_v3_after_lan_failure(
                &file_id,
                &record.device_id,
                TransferRoute::Cloud.as_str(),
                self.user_text(TextKey::TransferGeneric),
            )?;
        }
        Ok(())
    }

    fn cancel_file_v3_after_lan_failure(
        &self,
        file_id: &str,
        device_id: &str,
        control_route: &str,
        message: String,
    ) -> AppResult<()> {
        let cancel = BusinessEnvelope::from_payload(
            FILE_V3_CANCEL_TYPE,
            FileCancelPayload {
                session_id: file_id.to_string(),
                reason: REASON_TRANSFER_GENERIC.to_string(),
                message: message.clone(),
                details: None,
            },
        )?;
        let runtime = self.clone();
        let device_id = device_id.to_string();
        let control_route = control_route.to_string();
        tauri::async_runtime::spawn(async move {
            if control_route == TransferRoute::Lan.as_str() {
                let _ = runtime.inner.lan.send(&device_id, cancel, None, None).await;
            } else if control_route == TransferRoute::Cloud.as_str() {
                let _ = runtime.inner.cloud.send_relay(&device_id, cancel, None, None);
            }
        });
        self.finish_outgoing_transfer(
            file_id,
            "failed",
            Some(self.format_generic_transfer_error(&message)),
            None,
        )
    }

    fn expire_file_v3_endpoint(&self, file_id: String, device_id: String) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            sleep(FILE_V3_READY_TIMEOUT).await;
            let should_cancel = {
                let state = runtime.inner.state.lock_unpoisoned();
                state.outgoing_files.get(&file_id).is_some_and(|outgoing| {
                    outgoing.protocol == FileTransferProtocol::V3
                        && outgoing.record.device_id == device_id
                        && outgoing.record.route == TransferRoute::Lan.as_str()
                        && outgoing.record.status == "sending"
                })
            };
            if should_cancel && runtime.inner.lan.expire_file_v3_transfer(&file_id) {
                let _ = runtime.cancel_file_v3_after_lan_failure(
                    &file_id,
                    &device_id,
                    TransferRoute::Lan.as_str(),
                    runtime.user_text(TextKey::TransferHttpsIncomplete),
                );
            }
        });
    }

    async fn send_file_data_relay(
        &self,
        file_id: String,
        source_path: PathBuf,
        record: FileTransferRecord,
    ) -> AppResult<()> {
        self.send_file_chunks(
            file_id,
            source_path,
            record,
            RELAY_SEND_WINDOW_CHUNKS,
            ChunkTransport::Relay,
            FileTransferProtocol::V2,
        )
        .await
    }

    async fn send_file_data_relay_v3(
        &self,
        file_id: String,
        source_path: PathBuf,
        record: FileTransferRecord,
        protocol: FileTransferProtocol,
    ) -> AppResult<()> {
        self.watch_file_v3_relay_outgoing(file_id.clone());
        self.send_file_chunks(
            file_id,
            source_path,
            record,
            FILE_V3_RELAY_SEND_WINDOW_CHUNKS,
            ChunkTransport::Relay,
            protocol,
        )
        .await
    }

    async fn send_file_data_lan(
        &self,
        file_id: String,
        source_path: PathBuf,
        record: FileTransferRecord,
    ) -> AppResult<()> {
        self.send_file_chunks(
            file_id,
            source_path,
            record,
            LAN_SEND_WINDOW_CHUNKS,
            ChunkTransport::Lan,
            FileTransferProtocol::V2,
        )
        .await
    }

    async fn send_file_chunks(
        &self,
        file_id: String,
        source_path: PathBuf,
        record: FileTransferRecord,
        window_size: i64,
        transport: ChunkTransport,
        protocol: FileTransferProtocol,
    ) -> AppResult<()> {
        let mut file = tokio::fs::File::open(&source_path).await?;
        let mut index = 0_i64;
        let mut buffer = vec![0_u8; FILE_CHUNK_SIZE];
        loop {
            if self.take_cancelled_outgoing(&file_id) {
                self.cleanup_transport_after_cancel(&file_id, transport);
                return Ok(());
            }

            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }

            if !self
                .wait_for_send_window(&file_id, index, window_size)
                .await?
            {
                self.clear_outgoing_transfer_state(&file_id);
                self.cleanup_transport_after_cancel(&file_id, transport);
                return Ok(());
            }

            self.send_transport_chunk(&file_id, &record, index, &buffer[..read], transport, protocol)
                .await?;
            index += 1;
        }

        self.send_transport_finish(&file_id, &record, transport, protocol).await?;
        info!(file_id = %record.file_id, transport = ?transport, "file transfer sent; waiting for confirmation");
        Ok(())
    }

    async fn send_transport_chunk(
        &self,
        file_id: &str,
        record: &FileTransferRecord,
        index: i64,
        bytes: &[u8],
        transport: ChunkTransport,
        protocol: FileTransferProtocol,
    ) -> AppResult<()> {
        match transport {
            ChunkTransport::Relay => {
                let chunk = BusinessEnvelope::from_payload(
                    match protocol {
                        FileTransferProtocol::V2 => FILE_CHUNK_TYPE,
                        FileTransferProtocol::V3 => FILE_V3_CHUNK_TYPE,
                    },
                    FileChunkPayload {
                        session_id: file_id.to_string(),
                        chunk_index: index,
                        data: STANDARD.encode(bytes),
                    },
                )?;
                if protocol == FileTransferProtocol::V3 {
                    self.inner
                        .cloud
                        .send_relay(&record.device_id, chunk, None, None)?;
                    self.touch_file_v3_relay_outgoing(file_id);
                } else {
                    let _ = self.send_business_message(&record.device_id, chunk).await?;
                }
            }
            ChunkTransport::Lan => {
                let index = u32::try_from(index)
                    .map_err(|_| AppError::message("file chunk index is too large"))?;
                self.inner
                    .lan
                    .send_transfer_frame(file_id, FileDataFrame::chunk(index, bytes.to_vec()))?;
            }
        }
        Ok(())
    }

    async fn send_transport_finish(
        &self,
        file_id: &str,
        record: &FileTransferRecord,
        transport: ChunkTransport,
        protocol: FileTransferProtocol,
    ) -> AppResult<()> {
        if let ChunkTransport::Lan = transport {
            self.inner.lan.send_transfer_frame(
                file_id,
                FileDataFrame::finish(
                    u32::try_from(record.total_chunks).map_err(|_| {
                        AppError::message("file chunk count exceeds protocol limit")
                    })?,
                ),
            )?;
        }
        if matches!(transport, ChunkTransport::Relay) && protocol == FileTransferProtocol::V3 {
            let finish = BusinessEnvelope::from_payload(
                FILE_V3_FINISH_TYPE,
                FileFinishPayload {
                    session_id: file_id.to_string(),
                    total_chunks: record.total_chunks,
                },
            )?;
            self.inner
                .cloud
                .send_relay(&record.device_id, finish, None, None)?;
            self.touch_file_v3_relay_outgoing(file_id);
        }
        Ok(())
    }

    fn cleanup_transport_after_cancel(&self, file_id: &str, transport: ChunkTransport) {
        if let ChunkTransport::Lan = transport {
            let _ = self.inner.lan.send_transfer_frame(
                file_id,
                FileDataFrame::cancel(REASON_TRANSFER_USER_CANCELLED),
            );
            self.inner.lan.unregister_transfer(file_id);
        }
    }

    fn take_cancelled_outgoing(&self, file_id: &str) -> bool {
        let removed = {
            let mut state = self.inner.state.lock_unpoisoned();
            if !state.cancelled_files.remove(file_id) {
                return false;
            }
            state.outgoing_files.remove(file_id)
        };
        if removed.is_some() {
            self.inner.indicator.remove_session();
        }
        true
    }

    fn clear_outgoing_transfer_state(&self, file_id: &str) {
        let removed = {
            let mut state = self.inner.state.lock_unpoisoned();
            state.cancelled_files.remove(file_id);
            state.outgoing_files.remove(file_id)
        };
        if removed.is_some() {
            self.inner.indicator.remove_session();
        }
    }

    pub(super) async fn handle_file_chunk(&self, payload: FileChunkPayload) -> AppResult<()> {
        let session_id = payload.session_id;
        let bytes = STANDARD.decode(payload.data)?;
        debug!(
            session_id = %session_id,
            chunk_index = payload.chunk_index,
            bytes = bytes.len(),
            "received relay file chunk"
        );
        self.process_incoming_chunk(&session_id, payload.chunk_index, &bytes, true)
            .await
    }

    pub(super) async fn handle_file_v3_ready(
        &self,
        from: &str,
        route: &str,
        payload: FileV3ReadyPayload,
    ) -> AppResult<()> {
        if route != TransferRoute::Lan.as_str() {
            return Ok(());
        }
        let (token, temp_path, record) = {
            let mut state = self.inner.state.lock_unpoisoned();
            let incoming = state
                .incoming_files
                .get_mut(&payload.session_id)
                .ok_or_else(|| AppError::message("file.v3 receive state does not exist"))?;
            if incoming.protocol != FileTransferProtocol::V3 || incoming.record.device_id != from {
                return Ok(());
            }
            if incoming.record.route != TransferRoute::Lan.as_str() {
                return Ok(());
            }
            if incoming.v3_ready_received {
                return Ok(());
            }
            incoming.v3_ready_received = true;
            (
                incoming.transfer_token.clone(),
                incoming.record.temp_path.clone(),
                incoming.record.clone(),
            )
        };
        let Some(token) = token else {
            self.fail_file_v3_incoming(
                &payload.session_id,
                from,
                self.user_text(TextKey::TransferLanFailed),
            )
            .await?;
            return Ok(());
        };
        let Some(temp_path) = temp_path else {
            self.fail_file_v3_incoming(
                &payload.session_id,
                from,
                self.user_text(TextKey::TransferGeneric),
            )
            .await?;
            return Ok(());
        };
        let Some((ip, port)) = self.lan_endpoint_for_device(from) else {
            self.fail_file_v3_incoming(
                &payload.session_id,
                from,
                self.user_text(TextKey::TransferLanFailed),
            )
            .await?;
            return Ok(());
        };
        let progress_runtime = self.clone();
        let progress_session_id = payload.session_id.clone();
        let result = self
            .inner
            .lan
            .download_file_v3(
                &payload.session_id,
                &token,
                &ip,
                port,
                &payload.cert_fingerprint,
                PathBuf::from(&temp_path).as_path(),
                record.file_size,
                move |transferred_bytes| {
                    progress_runtime.report_file_v3_lan_download_progress(
                        &progress_session_id,
                        transferred_bytes,
                    )
                },
            )
            .await;
        match result {
            Ok(transferred_bytes) => {
                let updated = {
                    let mut state = self.inner.state.lock_unpoisoned();
                    let incoming = state
                        .incoming_files
                        .get_mut(&payload.session_id)
                        .ok_or_else(|| AppError::message("file.v3 receive state disappeared"))?;
                    incoming.record.transferred_bytes = transferred_bytes;
                    incoming.record.route = TransferRoute::Lan.as_str().to_string();
                    incoming.record.updated_at = unix_now_millis();
                    incoming.record.clone()
                };
                self.inner.database.save_transfer(&updated)?;
                self.emit_transfers()?;
                self.finish_incoming_transfer(&payload.session_id).await?;
            }
            Err(error) => {
                self.fail_file_v3_incoming(
                    &payload.session_id,
                    from,
                format!("{}: {error}", self.user_text(TextKey::TransferLanFailed)),
                )
                .await?;
            }
        }
        Ok(())
    }

    pub(super) async fn handle_file_v3_chunk(
        &self,
        from: &str,
        route: &str,
        payload: FileChunkPayload,
    ) -> AppResult<()> {
        if route != TransferRoute::Cloud.as_str() {
            return Ok(());
        }
        let belongs_to_sender = self
            .inner
            .state
            .lock_unpoisoned()
            .incoming_files
            .get(&payload.session_id)
            .is_some_and(|incoming| {
                incoming.protocol == FileTransferProtocol::V3
                    && incoming.record.device_id == from
                    && incoming.record.route == TransferRoute::Cloud.as_str()
            });
        if !belongs_to_sender {
            return Ok(());
        }
        let bytes = STANDARD.decode(payload.data)?;
        self.process_incoming_chunk(&payload.session_id, payload.chunk_index, &bytes, false)
            .await
    }

    pub(super) async fn handle_file_v3_finish(
        &self,
        from: &str,
        route: &str,
        payload: FileFinishPayload,
    ) -> AppResult<()> {
        if route != TransferRoute::Cloud.as_str() {
            return Ok(());
        }
        if payload.total_chunks < 0 {
            return Err(AppError::message("file.v3.finish has a negative chunk count"));
        }
        let (record, received_chunks) = {
            let mut state = self.inner.state.lock_unpoisoned();
            let incoming = state
                .incoming_files
                .get_mut(&payload.session_id)
                .ok_or_else(|| AppError::message("file.v3 receive state does not exist"))?;
            if incoming.protocol != FileTransferProtocol::V3
                || incoming.record.device_id != from
                || incoming.record.route != TransferRoute::Cloud.as_str()
            {
                return Ok(());
            }
            if incoming.record.total_chunks != 0 && incoming.record.total_chunks != payload.total_chunks {
                return Err(AppError::message("file.v3.finish total chunk count changed"));
            }
            incoming.record.total_chunks = payload.total_chunks;
            incoming.lan_finish_received = true;
            let now = unix_now_millis();
            incoming.record.updated_at = now;
            incoming.last_activity_at = now;
            if incoming.received_chunks < payload.total_chunks {
                incoming.gap_detected_at.get_or_insert(now);
            }
            (incoming.record.clone(), incoming.received_chunks)
        };
        self.inner.database.save_transfer(&record)?;
        if received_chunks < payload.total_chunks {
            return Ok(());
        }
        if received_chunks > payload.total_chunks {
            return Err(AppError::message("file.v3.finish excludes already received chunks"));
        }
        self.finish_incoming_transfer(&payload.session_id).await
    }

    async fn fail_file_v3_incoming(
        &self,
        file_id: &str,
        device_id: &str,
        message: String,
    ) -> AppResult<()> {
        let incoming = self
            .inner
            .state
            .lock_unpoisoned()
            .incoming_files
            .remove(file_id);
        let control_route = incoming.as_ref().map(|state| state.record.route.clone());
        if let Some(incoming) = incoming {
            self.inner.indicator.remove_session();
            if let Some(temp_path) = incoming.record.temp_path.as_ref() {
                let _ = tokio::fs::remove_file(temp_path).await;
            }
            let mut record = incoming.record;
            record.status = "failed".to_string();
            record.error = Some(self.format_generic_transfer_error(&message));
            record.temp_path = None;
            record.updated_at = unix_now_millis();
            self.inner.database.save_transfer(&record)?;
            self.emit_transfers()?;
        }
        let cancel = BusinessEnvelope::from_payload(
            FILE_V3_CANCEL_TYPE,
            FileCancelPayload {
                session_id: file_id.to_string(),
                reason: REASON_TRANSFER_GENERIC.to_string(),
                message,
                details: None,
            },
        )?;
        match control_route.as_deref() {
            Some(route) if route == TransferRoute::Lan.as_str() => {
                let _ = self.inner.lan.send(device_id, cancel, None, None).await;
            }
            Some(_) => {
                let _ = self.inner.cloud.send_relay(device_id, cancel, None, None);
            }
            None => {
                warn!(%file_id, %device_id, "skipped file.v3 cancel without an established control-plane route");
            }
        }
        Ok(())
    }

    pub(super) async fn handle_lan_transfer_frame(
        &self,
        session_id: &str,
        frame: FileDataFrame,
    ) -> AppResult<()> {
        debug!(%session_id, kind = ?frame.kind, index = frame.index, "received lan transfer frame");
        match frame.kind {
            FileDataFrameKind::Chunk => {
                self.handle_lan_file_chunk(session_id, frame.index as i64, frame.payload)
                    .await
            }
            FileDataFrameKind::Ack => self.handle_file_ack(FileAckPayload {
                session_id: session_id.to_string(),
                next_expected_index: frame.index as i64,
            }),
            FileDataFrameKind::Finish => self.handle_lan_file_finish(session_id).await,
            FileDataFrameKind::Retransmit => {
                self.retransmit_file_chunk(session_id, frame.index as i64, true)
                    .await
            }
            FileDataFrameKind::Cancel => {
                let reason = String::from_utf8_lossy(&frame.payload).to_string();
                let reason = if reason.trim().is_empty() {
                    REASON_TRANSFER_GENERIC.to_string()
                } else {
                    reason
                };
                self.handle_file_cancel(session_id, reason, None)
            }
        }
    }

    async fn handle_lan_file_chunk(
        &self,
        session_id: &str,
        chunk_index: i64,
        bytes: Vec<u8>,
    ) -> AppResult<()> {
        self.process_incoming_chunk(session_id, chunk_index, &bytes, false)
            .await
    }

    async fn process_incoming_chunk(
        &self,
        session_id: &str,
        chunk_index: i64,
        bytes: &[u8],
        finish_when_complete: bool,
    ) -> AppResult<()> {
        let process_lock = {
            let state = self.inner.state.lock_unpoisoned();
            state
                .incoming_files
                .get(session_id)
                .map(|item| item.process_lock.clone())
        }
        .ok_or_else(|| AppError::message("incoming file does not exist"))?;
        let _processing_guard = process_lock.lock().await;

        let (writer, verifier, received_chunks, device_id, protocol, route, total_chunks, finish_received) = {
            let state = self.inner.state.lock_unpoisoned();
            state.incoming_files.get(session_id).map(|item| {
                (
                    item.writer.clone(),
                    item.verifier.clone(),
                    item.received_chunks,
                    item.record.device_id.clone(),
                    item.protocol,
                    item.record.route.clone(),
                    item.record.total_chunks,
                    item.lan_finish_received,
                )
            })
        }
        .ok_or_else(|| AppError::message("incoming file does not exist"))?;

        let is_v3_relay = protocol == FileTransferProtocol::V3
            && route == TransferRoute::Cloud.as_str();

        if chunk_index < 0 {
            return Err(AppError::message("file chunk index is negative"));
        }
        if finish_received && total_chunks >= 0 && chunk_index >= total_chunks {
            return Err(AppError::message("file chunk exceeds announced total"));
        }

        if chunk_index < received_chunks {
            debug!(
                %session_id,
                chunk_index = chunk_index,
                received_chunks = received_chunks,
                "ignoring duplicate file chunk"
            );
            if is_v3_relay {
                {
                    let mut state = self.inner.state.lock_unpoisoned();
                    if let Some(incoming) = state.incoming_files.get_mut(session_id) {
                        incoming.last_activity_at = unix_now_millis();
                    }
                }
                self.maybe_send_file_v3_relay_ack(session_id).await?;
            } else {
                self.send_file_ack(&device_id, session_id, received_chunks)
                    .await?;
            }
            return Ok(());
        }
        if chunk_index > received_chunks {
            warn!(
                %session_id,
                chunk_index = chunk_index,
                received_chunks = received_chunks,
                "missing file chunk detected"
            );
            if is_v3_relay {
                let now = unix_now_millis();
                let mut state = self.inner.state.lock_unpoisoned();
                let incoming = state
                    .incoming_files
                    .get_mut(session_id)
                    .ok_or_else(|| AppError::message("incoming file does not exist"))?;
                let within_window = chunk_index - incoming.received_chunks
                    <= FILE_V3_RELAY_SEND_WINDOW_CHUNKS;
                if within_window
                    && (incoming.reorder_buffer.contains_key(&chunk_index)
                        || incoming.reorder_buffer.len()
                            < FILE_V3_RELAY_SEND_WINDOW_CHUNKS as usize)
                {
                    incoming.reorder_buffer.insert(chunk_index, bytes.to_vec());
                }
                incoming.gap_detected_at.get_or_insert(now);
                incoming.last_activity_at = now;
            } else {
                self.send_file_retransmit(&device_id, session_id, received_chunks)
                    .await?;
            }
            return Ok(());
        }

        let (mut record, mut bytes_per_second, mut finished, mut lan_finish_received) = self
            .append_incoming_chunk(session_id, &writer, &verifier, bytes)
            .await?;

        if is_v3_relay {
            while let Some(buffered) = {
                let mut state = self.inner.state.lock_unpoisoned();
                let incoming = state
                    .incoming_files
                    .get_mut(session_id)
                    .ok_or_else(|| AppError::message("incoming file does not exist"))?;
                incoming.reorder_buffer.remove(&incoming.received_chunks)
            } {
                let (next_record, next_bytes_per_second, next_finished, next_finish_received) = self
                    .append_incoming_chunk(session_id, &writer, &verifier, &buffered)
                    .await?;
                record = next_record;
                if next_bytes_per_second.is_some() {
                    bytes_per_second = next_bytes_per_second;
                }
                finished = next_finished;
                lan_finish_received = next_finish_received;
            }

            let mut state = self.inner.state.lock_unpoisoned();
            let incoming = state
                .incoming_files
                .get_mut(session_id)
                .ok_or_else(|| AppError::message("incoming file does not exist"))?;
            incoming.gap_detected_at = (!incoming.reorder_buffer.is_empty())
                .then(unix_now_millis);
        }

        if let Some(bytes_per_second) = bytes_per_second {
            self.inner.database.save_transfer(&record)?;
            self.emit_transfer_progress(record.clone(), bytes_per_second);
        }
        let next_expected_index = self
            .inner
            .state
            .lock_unpoisoned()
            .incoming_files
            .get(session_id)
            .map(|incoming| incoming.received_chunks)
            .ok_or_else(|| AppError::message("incoming file does not exist"))?;
        if is_v3_relay {
            self.maybe_send_file_v3_relay_ack(session_id).await?;
        } else if should_send_file_ack(next_expected_index, record.total_chunks) {
            self.send_file_ack(&record.device_id, session_id, next_expected_index)
                .await?;
        }
        if finish_when_complete && finished {
            self.finish_incoming_transfer(session_id).await?;
        } else if !finish_when_complete && lan_finish_received && !is_v3_relay {
            if finished {
                self.finish_incoming_transfer(session_id).await?;
            } else {
                self.send_file_retransmit(&record.device_id, session_id, next_expected_index)
                    .await?;
            }
        } else if is_v3_relay && lan_finish_received && finished {
            self.finish_incoming_transfer(session_id).await?;
        }
        Ok(())
    }

    async fn append_incoming_chunk(
        &self,
        session_id: &str,
        writer: &Arc<AsyncMutex<tokio::fs::File>>,
        verifier: &Arc<AsyncMutex<FileChecksumVerifier>>,
        bytes: &[u8],
    ) -> AppResult<(FileTransferRecord, Option<f64>, bool, bool)> {
        let mut file = writer.lock().await;
        file.write_all(bytes).await?;
        drop(file);
        {
            let mut verifier = verifier.lock().await;
            verifier.update(bytes);
        }
        self.update_incoming_progress(session_id, bytes.len() as i64, unix_now_millis())
    }

    async fn handle_lan_file_finish(&self, session_id: &str) -> AppResult<()> {
        let (received_chunks, total_chunks, device_id) = {
            let mut state = self.inner.state.lock_unpoisoned();
            state.incoming_files.get_mut(session_id).map(|item| {
                item.lan_finish_received = true;
                (
                    item.received_chunks,
                    item.record.total_chunks,
                    item.record.device_id.clone(),
                )
            })
        }
        .ok_or_else(|| AppError::message("receive state does not exist"))?;

        if received_chunks < total_chunks {
            self.send_file_retransmit(&device_id, session_id, received_chunks)
                .await?;
            return Ok(());
        }

        self.finish_incoming_transfer(session_id).await
    }

    pub(super) fn handle_file_ack(&self, payload: FileAckPayload) -> AppResult<()> {
        let Some((record, bytes_per_second)) = self.update_outgoing_ack_progress(
            &payload.session_id,
            payload.next_expected_index,
            unix_now_millis(),
        )?
        else {
            return Ok(());
        };

        self.inner.database.save_transfer(&record)?;
        debug!(
            session_id = %payload.session_id,
            next_expected_index = payload.next_expected_index,
            "processed file ack"
        );
        if let Some(bytes_per_second) = bytes_per_second {
            self.emit_transfer_progress(record, bytes_per_second);
        }
        Ok(())
    }

    pub(super) fn handle_file_v3_ack(
        &self,
        from: &str,
        route: &str,
        payload: FileAckPayload,
    ) -> AppResult<()> {
        if route != TransferRoute::Cloud.as_str()
            || !self.is_file_v3_outgoing_control(&payload.session_id, from, route)
        {
            return Ok(());
        }
        self.handle_file_ack(payload)
    }

    pub(super) async fn retransmit_file_chunk(
        &self,
        session_id: &str,
        chunk_index: i64,
        lan: bool,
    ) -> AppResult<()> {
        if chunk_index < 0 {
            return Ok(());
        }

        let (source_path, record, protocol) = {
            let state = self.inner.state.lock_unpoisoned();
            state
                .outgoing_files
                .get(session_id)
                .map(|item| (item.source_path.clone(), item.record.clone(), item.protocol))
        }
        .ok_or_else(|| AppError::message("file send state does not exist"))?;
        if chunk_index >= record.total_chunks {
            return Ok(());
        }

        let offset = chunk_index
            .checked_mul(FILE_CHUNK_SIZE as i64)
            .ok_or_else(|| AppError::message("file chunk offset overflow"))?
            as u64;
        let mut file = tokio::fs::File::open(&source_path).await?;
        file.seek(SeekFrom::Start(offset)).await?;
        let mut buffer = vec![0_u8; FILE_CHUNK_SIZE];
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }

        if lan {
            let chunk_index = u32::try_from(chunk_index)
                .map_err(|_| AppError::message("file chunk index is too large"))?;
            self.inner.lan.send_transfer_frame(
                session_id,
                FileDataFrame::chunk(chunk_index, buffer[..read].to_vec()),
            )?;
        } else {
            let chunk = BusinessEnvelope::from_payload(
                match protocol {
                    FileTransferProtocol::V2 => FILE_CHUNK_TYPE,
                    FileTransferProtocol::V3 => FILE_V3_CHUNK_TYPE,
                },
                FileChunkPayload {
                    session_id: session_id.to_string(),
                    chunk_index,
                    data: STANDARD.encode(&buffer[..read]),
                },
            )?;
            if protocol == FileTransferProtocol::V3 {
                self.inner
                    .cloud
                    .send_relay(&record.device_id, chunk, None, None)?;
                self.touch_file_v3_relay_outgoing(session_id);
            } else {
                let _ = self.send_business_message(&record.device_id, chunk).await?;
            }
        }

        Ok(())
    }

    pub(super) async fn retransmit_file_v3_chunk(
        &self,
        from: &str,
        route: &str,
        session_id: &str,
        chunk_index: i64,
    ) -> AppResult<()> {
        if route != TransferRoute::Cloud.as_str()
            || !self.is_file_v3_outgoing_control(session_id, from, route)
        {
            return Ok(());
        }
        self.touch_file_v3_relay_outgoing(session_id);
        self.retransmit_file_chunk(session_id, chunk_index, false).await
    }

    pub(super) fn handle_file_v3_reject(
        &self,
        from: &str,
        route: &str,
        payload: FileRejectPayload,
    ) -> AppResult<()> {
        if !self.is_file_v3_outgoing_control(&payload.session_id, from, route) {
            return Ok(());
        }
        self.finish_outgoing_transfer(
            &payload.session_id,
            "rejected",
            self.transfer_error_from_peer(Some(&payload.reason), Some(&payload.message)),
            None,
        )
    }

    pub(super) fn handle_file_v3_done(
        &self,
        from: &str,
        route: &str,
        payload: FileDonePayload,
    ) -> AppResult<()> {
        if !self.is_file_v3_outgoing_control(&payload.session_id, from, route) {
            return Ok(());
        }
        self.finish_outgoing_transfer(
            &payload.session_id,
            if payload.success { "completed" } else { "failed" },
            self.transfer_error_from_peer(payload.reason.as_deref(), payload.message.as_deref()),
            None,
        )
    }

    pub(super) fn handle_file_v3_cancel(
        &self,
        from: &str,
        route: &str,
        payload: FileCancelPayload,
    ) -> AppResult<()> {
        if !self.is_file_v3_control(&payload.session_id, from, route) {
            return Ok(());
        }
        self.handle_file_cancel(&payload.session_id, payload.reason, Some(payload.message))
    }

    async fn finish_incoming_transfer(&self, file_id: &str) -> AppResult<()> {
        let incoming = self
            .inner
            .state
            .lock_unpoisoned()
            .incoming_files
            .remove(file_id)
            .ok_or_else(|| AppError::message("receive state does not exist"))?;
        self.inner.indicator.remove_session();
        {
            let mut writer = incoming.writer.lock().await;
            writer.flush().await?;
        }

        let temp_path = incoming
            .record
            .temp_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| AppError::message("temporary file path does not exist"))?;
        let download_dir = temp_path
            .parent()
            .map(PathBuf::from)
            .ok_or_else(|| AppError::message("temporary file directory does not exist"))?;

        let checksum_matches = match incoming.protocol {
            FileTransferProtocol::V2 => {
                let verifier = incoming.verifier.lock().await;
                verifier.verify()
            }
            FileTransferProtocol::V3 => verify_file_checksum(&temp_path, &incoming.record.checksum)?,
        };
        let size_matches = match incoming.protocol {
            FileTransferProtocol::V2 => true,
            FileTransferProtocol::V3 => tokio::fs::metadata(&temp_path)
                .await
                .map(|metadata| metadata.len() == incoming.record.file_size as u64)
                .unwrap_or(false),
        };
        let integrity_matches = checksum_matches && size_matches;

        let (final_path, success, failure_message) = if integrity_matches {
            if let Some(destination) = incoming.filesystem_upload.as_ref() {
                match commit_filesystem_upload(destination, &temp_path) {
                    Ok(path) => (Some(path.to_string_lossy().to_string()), true, None),
                    Err(error) => {
                        let _ = tokio::fs::remove_file(&temp_path).await;
                        (None, false, Some(error.message))
                    }
                }
            } else {
                let path = unique_download_path(&download_dir, &incoming.record.file_name);
                match tokio::fs::rename(&temp_path, &path).await {
                    Ok(()) => (Some(path.to_string_lossy().to_string()), true, None),
                    Err(error) => {
                        let _ = tokio::fs::remove_file(&temp_path).await;
                        (None, false, Some(error.to_string()))
                    }
                }
            }
        } else {
            let _ = tokio::fs::remove_file(&temp_path).await;
            (None, false, None)
        };

        let mut record = incoming.record;
        record.status = if success {
            "completed".to_string()
        } else {
            "failed".to_string()
        };
        record.final_path = final_path.clone();
        record.temp_path = None;
        record.error = if success {
            None
        } else {
            Some(
                failure_message
                    .as_deref()
                    .map(|message| self.format_generic_transfer_error(message))
                    .unwrap_or_else(|| {
                        if size_matches {
                            REASON_TRANSFER_CHECKSUM_MISMATCH.to_string()
                        } else {
                            self.user_text(TextKey::TransferSizeMismatch)
                        }
                    }),
            )
        };
        record.updated_at = unix_now_millis();
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()?;

        let done = BusinessEnvelope::from_payload(
            incoming.protocol.done_type(),
            FileDonePayload {
                session_id: file_id.to_string(),
                success,
                reason: if success {
                    None
                } else {
                    Some(if checksum_matches && size_matches {
                        REASON_TRANSFER_GENERIC.to_string()
                    } else if size_matches {
                        REASON_TRANSFER_CHECKSUM_MISMATCH.to_string()
                    } else {
                        REASON_TRANSFER_GENERIC.to_string()
                    })
                },
                message: if success {
                    None
                } else {
                    Some(failure_message.unwrap_or_else(|| {
                        if size_matches {
                            transfer_error_message(self, REASON_TRANSFER_CHECKSUM_MISMATCH)
                        } else {
                            self.user_text(TextKey::TransferSizeMismatch)
                        }
                    }))
                },
                details: None,
            },
        )?;
        if incoming.protocol == FileTransferProtocol::V3 {
            if record.route == TransferRoute::Lan.as_str() {
                self.inner
                    .lan
                    .send(&record.device_id, done, None, None)
                    .await?;
            } else {
                self.inner
                    .cloud
                    .send_relay(&record.device_id, done, None, None)?;
            }
        } else {
            let _ = self.send_business_message(&record.device_id, done).await?;
        }
        self.inner.lan.unregister_transfer(file_id);
        let _ = crate::shell::show_main_window(
            &self.inner.app,
            Some(&self.device_route("/transfers", &record.device_id)),
        );

        if success {
            self.notify(
                TextKey::FileReceiveCompleteTitle,
                &[],
                &self.user_message(TextKey::FileSaved, &[("file", record.file_name.clone())]),
            )?;
        } else {
            self.notify(
                TextKey::FileReceiveFailedTitle,
                &[],
                &self.user_message(
                    TextKey::FileChecksumFailed,
                    &[("file", record.file_name.clone())],
                ),
            )?;
        }
        self.inner
            .state
            .lock_unpoisoned()
            .cancelled_files
            .remove(file_id);

        Ok(())
    }

    pub(super) fn finish_outgoing_transfer(
        &self,
        file_id: &str,
        status: &str,
        error: Option<String>,
        final_path: Option<String>,
    ) -> AppResult<()> {
        let removed = self
            .inner
            .state
            .lock_unpoisoned()
            .outgoing_files
            .remove(file_id);
        if removed.is_some() {
            self.inner.indicator.remove_session();
        }
        if let Some(notify) = removed.as_ref().map(|item| item.ack_notify.clone()) {
            notify.notify_one();
        }
        let record = removed.map(|item| item.record);
        let mut record = record
            .or(self.inner.database.load_transfer(file_id)?)
            .ok_or_else(|| AppError::message("transfer record does not exist"))?;
        record.status = status.to_string();
        if status == "completed" {
            record.transferred_bytes = record.file_size;
        }
        if let Some(error) = error {
            record.error = Some(error);
        }
        if let Some(path) = final_path {
            record.final_path = Some(path);
        }
        record.updated_at = unix_now_millis();
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()?;
        self.inner.lan.unregister_transfer(file_id);
        info!(%file_id, %status, "outgoing file transfer finished");
        self.inner
            .state
            .lock_unpoisoned()
            .cancelled_files
            .remove(file_id);
        Ok(())
    }

    pub(super) fn handle_file_cancel(
        &self,
        file_id: &str,
        reason: String,
        message: Option<String>,
    ) -> AppResult<()> {
        warn!(%file_id, %reason, "file transfer cancelled by peer");
        let error = self.transfer_error_from_peer(Some(&reason), message.as_deref());
        self.inner.lan.unregister_transfer(file_id);
        if matches!(
            self.inner
                .database
                .load_transfer(file_id)?
                .as_ref()
                .map(|record| record.status.as_str()),
            Some("completed")
        ) {
            debug!(%file_id, "ignored file cancel after transfer completion");
            return Ok(());
        }
        if let Some(incoming) = self
            .inner
            .state
            .lock_unpoisoned()
            .incoming_files
            .remove(file_id)
        {
            self.inner.indicator.remove_session();
            if let Some(temp_path) = incoming.record.temp_path.as_ref() {
                let _ = fs::remove_file(temp_path);
            }
            let mut record = incoming.record;
            record.status = "cancelled".to_string();
            record.error = error.clone();
            record.temp_path = None;
            record.updated_at = unix_now_millis();
            self.inner.database.save_transfer(&record)?;
            self.emit_transfers()?;
            return Ok(());
        }
        if let Some(mut record) = self.inner.database.load_transfer(file_id)? {
            if record.direction == "inbound" {
                record.status = "cancelled".to_string();
                record.error = error.clone();
                record.updated_at = unix_now_millis();
                self.inner.database.save_transfer(&record)?;
                self.emit_transfers()?;
                return Ok(());
            }
        }
        self.finish_outgoing_transfer(file_id, "cancelled", error, None)
    }

    pub(super) fn handle_lan_transfer_closed(&self, file_id: &str) -> AppResult<()> {
        let (incoming, outgoing_active, cancelled) = {
            let mut state = self.inner.state.lock_unpoisoned();
            let cancelled = state.cancelled_files.contains(file_id);
            let incoming = state.incoming_files.remove(file_id);
            let outgoing_active = state.outgoing_files.contains_key(file_id);
            (incoming, outgoing_active, cancelled)
        };

        if cancelled {
            debug!(%file_id, "ignored lan transfer close for cancelled transfer");
            return Ok(());
        }

        if let Some(incoming) = incoming {
            let record = self.inner.database.load_transfer(file_id)?;
            if matches!(record.as_ref().map(|item| item.status.as_str()), Some("completed")) {
                debug!(%file_id, "ignored lan transfer close after incoming completion");
                return Ok(());
            }
            self.inner.indicator.remove_session();
            warn!(%file_id, "incoming lan transfer closed before completion");
            if let Some(temp_path) = incoming.record.temp_path.as_ref() {
                let _ = fs::remove_file(temp_path);
            }
            let mut record = incoming.record;
            record.status = "failed".to_string();
            record.error = Some(self.user_text(TextKey::TransferConnectionClosed));
            record.temp_path = None;
            record.updated_at = unix_now_millis();
            self.inner.database.save_transfer(&record)?;
            self.emit_transfers()?;
            return Ok(());
        }

        if outgoing_active {
            warn!(%file_id, "outgoing lan transfer closed before completion");
            self.finish_outgoing_transfer(
                file_id,
                "failed",
                Some(self.user_text(TextKey::TransferConnectionClosed)),
                None,
            )?;
        }
        Ok(())
    }

    fn lan_endpoint_for_device(&self, device_id: &str) -> Option<(String, u16)> {
        if !self.inner.lan.is_available(device_id) {
            debug!(%device_id, "lan endpoint unavailable because peer is not lan available");
            return None;
        }
        let endpoint = self.inner.lan.peer_endpoint(device_id);
        if endpoint.is_none() {
            debug!(%device_id, "lan endpoint unavailable in peer endpoint table");
        }
        endpoint
    }

    fn update_outgoing_route(&self, file_id: &str, route: TransferRoute) -> AppResult<()> {
        let record = {
            let mut state = self.inner.state.lock_unpoisoned();
            let outgoing = state
                .outgoing_files
                .get_mut(file_id)
                .ok_or_else(|| AppError::message("file send state does not exist"))?;
            outgoing.record.route = route.as_str().to_string();
            outgoing.record.updated_at = unix_now_millis();
            outgoing.record.clone()
        };
        self.inner.database.save_transfer(&record)?;
        debug!(%file_id, route = %route.as_str(), "updated outgoing transfer route");
        self.emit_transfers()
    }

    pub(super) fn mark_incoming_route(&self, file_id: &str, route: TransferRoute) -> AppResult<()> {
        let record = {
            let mut state = self.inner.state.lock_unpoisoned();
            let Some(incoming) = state.incoming_files.get_mut(file_id) else {
                return Ok(());
            };
            incoming.record.route = route.as_str().to_string();
            incoming.record.updated_at = unix_now_millis();
            incoming.record.clone()
        };
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()
    }

    fn transfer_route(&self, file_id: &str) -> Option<TransferRoute> {
        let state = self.inner.state.lock_unpoisoned();
        state
            .incoming_files
            .get(file_id)
            .and_then(|item| TransferRoute::from_str(&item.record.route))
            .or_else(|| {
                state
                    .outgoing_files
                    .get(file_id)
                    .and_then(|item| TransferRoute::from_str(&item.record.route))
            })
    }

    fn transfer_protocol(&self, file_id: &str) -> Option<FileTransferProtocol> {
        let state = self.inner.state.lock_unpoisoned();
        state
            .incoming_files
            .get(file_id)
            .map(|item| item.protocol)
            .or_else(|| state.outgoing_files.get(file_id).map(|item| item.protocol))
    }

    fn touch_file_v3_relay_outgoing(&self, file_id: &str) {
        let mut state = self.inner.state.lock_unpoisoned();
        if let Some(outgoing) = state.outgoing_files.get_mut(file_id) {
            if outgoing.protocol == FileTransferProtocol::V3
                && outgoing.record.route == TransferRoute::Cloud.as_str()
            {
                outgoing.last_activity_at = unix_now_millis();
            }
        }
    }

    fn is_file_v3_outgoing_control(&self, file_id: &str, from: &str, route: &str) -> bool {
        self.inner
            .state
            .lock_unpoisoned()
            .outgoing_files
            .get(file_id)
            .is_some_and(|outgoing| {
                outgoing.protocol == FileTransferProtocol::V3
                    && outgoing.record.device_id == from
                    && outgoing.record.route == route
            })
    }

    fn is_file_v3_control(&self, file_id: &str, from: &str, route: &str) -> bool {
        let state = self.inner.state.lock_unpoisoned();
        state.outgoing_files.get(file_id).is_some_and(|outgoing| {
            outgoing.protocol == FileTransferProtocol::V3
                && outgoing.record.device_id == from
                && outgoing.record.route == route
        }) || state.incoming_files.get(file_id).is_some_and(|incoming| {
            incoming.protocol == FileTransferProtocol::V3
                && incoming.record.device_id == from
                && incoming.record.route == route
        })
    }

    async fn send_file_v3_control(
        &self,
        device_id: &str,
        route: &str,
        message: BusinessEnvelope,
        correlation_id: Option<String>,
    ) -> AppResult<()> {
        match route {
            route if route == TransferRoute::Lan.as_str() => {
                self.inner
                    .lan
                    .send(device_id, message, None, correlation_id)
                    .await
            }
            route if route == TransferRoute::Cloud.as_str() => self
                .inner
                .cloud
                .send_relay(device_id, message, None, correlation_id),
            _ => Err(AppError::message("invalid file.v3 control-plane route")),
        }
    }

    async fn send_file_ack(
        &self,
        device_id: &str,
        file_id: &str,
        next_expected_index: i64,
    ) -> AppResult<()> {
        let protocol = self.transfer_protocol(file_id).unwrap_or(FileTransferProtocol::V2);
        if protocol == FileTransferProtocol::V2
            && self.transfer_route(file_id) == Some(TransferRoute::Lan)
        {
            let next_expected_index = u32::try_from(next_expected_index)
                .map_err(|_| AppError::message("file chunk index is too large"))?;
            self.inner
                .lan
                .send_transfer_frame(file_id, FileDataFrame::ack(next_expected_index))?;
            return Ok(());
        }

        let ack = BusinessEnvelope::from_payload(
            match protocol {
                FileTransferProtocol::V2 => FILE_ACK_TYPE,
                FileTransferProtocol::V3 => FILE_V3_ACK_TYPE,
            },
            FileAckPayload {
                session_id: file_id.to_string(),
                next_expected_index,
            },
        )?;
        if protocol == FileTransferProtocol::V3 {
            self.inner.cloud.send_relay(device_id, ack, None, None)?;
        } else {
            let _ = self.send_business_message(device_id, ack).await?;
        }
        Ok(())
    }

    async fn maybe_send_file_v3_relay_ack(&self, file_id: &str) -> AppResult<()> {
        let now = unix_now_millis();
        let candidate = {
            let state = self.inner.state.lock_unpoisoned();
            state.incoming_files.get(file_id).and_then(|incoming| {
                (incoming.protocol == FileTransferProtocol::V3
                    && incoming.record.route == TransferRoute::Cloud.as_str()
                    && incoming.received_chunks > incoming.last_acknowledged_chunks
                    && (incoming.received_chunks - incoming.last_acknowledged_chunks
                        >= FILE_V3_RELAY_ACK_INTERVAL_CHUNKS
                        || now - incoming.last_ack_at
                            >= FILE_V3_RELAY_ACK_INTERVAL.as_millis() as i64))
                .then(|| (incoming.record.device_id.clone(), incoming.received_chunks))
            })
        };
        let Some((device_id, next_expected_index)) = candidate else {
            return Ok(());
        };
        self.send_file_ack(&device_id, file_id, next_expected_index)
            .await?;
        let mut state = self.inner.state.lock_unpoisoned();
        if let Some(incoming) = state.incoming_files.get_mut(file_id) {
            if incoming.protocol == FileTransferProtocol::V3
                && incoming.record.route == TransferRoute::Cloud.as_str()
            {
                incoming.last_acknowledged_chunks = incoming
                    .last_acknowledged_chunks
                    .max(next_expected_index);
                incoming.last_ack_at = now;
            }
        }
        Ok(())
    }

    async fn send_file_retransmit(
        &self,
        device_id: &str,
        file_id: &str,
        chunk_index: i64,
    ) -> AppResult<()> {
        let protocol = self.transfer_protocol(file_id).unwrap_or(FileTransferProtocol::V2);
        if protocol == FileTransferProtocol::V2
            && self.transfer_route(file_id) == Some(TransferRoute::Lan)
        {
            let chunk_index = u32::try_from(chunk_index)
                .map_err(|_| AppError::message("file chunk index is too large"))?;
            self.inner
                .lan
                .send_transfer_frame(file_id, FileDataFrame::retransmit(chunk_index))?;
            return Ok(());
        }

        let retransmit = BusinessEnvelope::from_payload(
            match protocol {
                FileTransferProtocol::V2 => FILE_RETRANSMIT_TYPE,
                FileTransferProtocol::V3 => FILE_V3_RETRANSMIT_TYPE,
            },
            FileRetransmitPayload {
                session_id: file_id.to_string(),
                chunk_index,
            },
        )?;
        if protocol == FileTransferProtocol::V3 {
            self.inner.cloud.send_relay(device_id, retransmit, None, None)?;
        } else {
            let _ = self.send_business_message(device_id, retransmit).await?;
        }
        Ok(())
    }

    async fn wait_for_send_window(
        &self,
        file_id: &str,
        next_chunk_index: i64,
        window_size: i64,
    ) -> AppResult<bool> {
        loop {
            let notify = {
                let state = self.inner.state.lock_unpoisoned();
                if state.cancelled_files.contains(file_id) {
                    return Ok(false);
                }
                let Some(outgoing) = state.outgoing_files.get(file_id) else {
                    return Ok(false);
                };
                if next_chunk_index - outgoing.acknowledged_chunks < window_size {
                    return Ok(true);
                }
                outgoing.ack_notify.clone()
            };

            notify.notified().await;
        }
    }

    fn update_outgoing_ack_progress(
        &self,
        file_id: &str,
        next_expected_index: i64,
        updated_at: i64,
    ) -> AppResult<Option<(FileTransferRecord, Option<f64>)>> {
        let mut state = self.inner.state.lock_unpoisoned();
        let Some(outgoing) = state.outgoing_files.get_mut(file_id) else {
            return Ok(None);
        };

        if outgoing.protocol == FileTransferProtocol::V3
            && outgoing.record.route == TransferRoute::Cloud.as_str()
        {
            outgoing.last_activity_at = updated_at;
        }

        let next_expected_index = next_expected_index.clamp(0, outgoing.record.total_chunks);
        if next_expected_index <= outgoing.acknowledged_chunks {
            return Ok(None);
        }

        outgoing.acknowledged_chunks = next_expected_index;
        outgoing.ack_notify.notify_one();
        let acknowledged_bytes = acknowledged_file_bytes(
            outgoing.record.file_size,
            outgoing.record.total_chunks,
            next_expected_index,
        );
        if acknowledged_bytes <= outgoing.record.transferred_bytes {
            return Ok(None);
        }

        outgoing.record.transferred_bytes = acknowledged_bytes;
        outgoing.record.updated_at = updated_at;
        let finished = acknowledged_bytes >= outgoing.record.file_size;
        let should_report =
            finished || updated_at - outgoing.last_progress_at >= TRANSFER_PROGRESS_INTERVAL_MS;
        let bytes_per_second = if should_report {
            let delta = outgoing.record.transferred_bytes - outgoing.last_reported_bytes;
            let duration = updated_at - outgoing.last_progress_at;
            outgoing.last_reported_bytes = outgoing.record.transferred_bytes;
            outgoing.last_progress_at = updated_at;
            Some(calculate_bytes_per_second(delta, duration))
        } else {
            None
        };

        Ok(Some((outgoing.record.clone(), bytes_per_second)))
    }

    fn report_file_v3_lan_upload_progress(
        &self,
        file_id: &str,
        transferred_bytes: i64,
    ) -> AppResult<()> {
        let updated_at = unix_now_millis();
        let progress = {
            let mut state = self.inner.state.lock_unpoisoned();
            let Some(outgoing) = state.outgoing_files.get_mut(file_id) else {
                return Ok(());
            };
            if outgoing.protocol != FileTransferProtocol::V3
                || outgoing.record.route != TransferRoute::Lan.as_str()
                || outgoing.record.status != "sending"
            {
                return Ok(());
            }
            if transferred_bytes <= outgoing.record.transferred_bytes {
                return Ok(());
            }
            if transferred_bytes > outgoing.record.file_size {
                return Err(AppError::message(
                    "LAN HTTPS upload exceeds the offered file size",
                ));
            }

            outgoing.record.transferred_bytes = transferred_bytes;
            outgoing.record.updated_at = updated_at;
            outgoing.last_activity_at = updated_at;
            let should_report = transferred_bytes == outgoing.record.file_size
                || updated_at - outgoing.last_progress_at >= TRANSFER_PROGRESS_INTERVAL_MS;
            should_report.then(|| {
                let delta = outgoing.record.transferred_bytes - outgoing.last_reported_bytes;
                let duration = updated_at - outgoing.last_progress_at;
                outgoing.last_reported_bytes = outgoing.record.transferred_bytes;
                outgoing.last_progress_at = updated_at;
                (
                    outgoing.record.clone(),
                    calculate_bytes_per_second(delta, duration),
                )
            })
        };

        if let Some((record, bytes_per_second)) = progress {
            if self.inner.database.update_active_transfer_progress(
                &record.file_id,
                record.transferred_bytes,
                record.updated_at,
            )? {
                self.emit_transfer_progress(record, bytes_per_second);
            }
        }
        Ok(())
    }

    fn update_incoming_progress(
        &self,
        file_id: &str,
        delta_bytes: i64,
        updated_at: i64,
    ) -> AppResult<(FileTransferRecord, Option<f64>, bool, bool)> {
        let mut state = self.inner.state.lock_unpoisoned();
        let incoming = state
            .incoming_files
            .get_mut(file_id)
            .ok_or_else(|| AppError::message("receive state does not exist"))?;
        incoming.record.transferred_bytes += delta_bytes;
        incoming.record.updated_at = updated_at;
        incoming.received_chunks += 1;
        incoming.last_activity_at = updated_at;
        let finished = incoming.record.total_chunks > 0
            && incoming.received_chunks >= incoming.record.total_chunks;
        let should_report =
            finished || updated_at - incoming.last_progress_at >= TRANSFER_PROGRESS_INTERVAL_MS;
        let bytes_per_second = if should_report {
            let delta = incoming.record.transferred_bytes - incoming.last_reported_bytes;
            let duration = updated_at - incoming.last_progress_at;
            incoming.last_reported_bytes = incoming.record.transferred_bytes;
            incoming.last_progress_at = updated_at;
            Some(calculate_bytes_per_second(delta, duration))
        } else {
            None
        };
        Ok((
            incoming.record.clone(),
            bytes_per_second,
            finished,
            incoming.lan_finish_received,
        ))
    }

    fn report_file_v3_lan_download_progress(
        &self,
        file_id: &str,
        transferred_bytes: i64,
    ) -> AppResult<()> {
        let updated_at = unix_now_millis();
        let progress = {
            let mut state = self.inner.state.lock_unpoisoned();
            let Some(incoming) = state.incoming_files.get_mut(file_id) else {
                return Ok(());
            };
            if incoming.protocol != FileTransferProtocol::V3
                || incoming.record.route != TransferRoute::Lan.as_str()
            {
                return Ok(());
            }
            if transferred_bytes <= incoming.record.transferred_bytes {
                return Ok(());
            }
            if transferred_bytes > incoming.record.file_size {
                return Err(AppError::message(
                    "LAN HTTPS download exceeds the offered file size",
                ));
            }

            incoming.record.transferred_bytes = transferred_bytes;
            incoming.record.updated_at = updated_at;
            incoming.last_activity_at = updated_at;
            let should_report = transferred_bytes == incoming.record.file_size
                || updated_at - incoming.last_progress_at >= TRANSFER_PROGRESS_INTERVAL_MS;
            should_report.then(|| {
                let delta = incoming.record.transferred_bytes - incoming.last_reported_bytes;
                let duration = updated_at - incoming.last_progress_at;
                incoming.last_reported_bytes = incoming.record.transferred_bytes;
                incoming.last_progress_at = updated_at;
                (
                    incoming.record.clone(),
                    calculate_bytes_per_second(delta, duration),
                )
            })
        };

        if let Some((record, bytes_per_second)) = progress {
            if self.inner.database.update_active_transfer_progress(
                &record.file_id,
                record.transferred_bytes,
                record.updated_at,
            )? {
                self.emit_transfer_progress(record, bytes_per_second);
            }
        }
        Ok(())
    }

    fn emit_transfer_progress(&self, record: FileTransferRecord, bytes_per_second: f64) {
        let _ = self.inner.app.emit(
            TRANSFER_PROGRESS_EVENT,
            TransferProgressPayload {
                record,
                bytes_per_second,
            },
        );
    }

    fn emit_transfer_preparing(&self, current: usize, total: usize) {
        let _ = self.inner.app.emit(
            TRANSFER_PREPARING_EVENT,
            TransferPreparingPayload { current, total },
        );
    }

    pub(super) fn cleanup_unfinished_transfers(&self) -> AppResult<()> {
        let mut state = self.inner.state.lock_unpoisoned();
        if state.cleanup_done {
            return Ok(());
        }
        state.cleanup_done = true;
        drop(state);

        let unfinished = self.inner.database.load_unfinished_transfers()?;
        let had_unfinished = !unfinished.is_empty();
        for mut item in unfinished {
            if let Some(path) = item.temp_path.as_ref() {
                let _ = fs::remove_file(path);
            }
            item.status = "failed".to_string();
            item.error = Some(self.user_text(TextKey::TransferGeneric));
            item.temp_path = None;
            item.updated_at = unix_now_millis();
            self.inner.database.save_transfer(&item)?;
        }
        if had_unfinished {
            self.emit_transfers()?;
        }
        Ok(())
    }
}
