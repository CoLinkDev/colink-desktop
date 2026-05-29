use std::{fs, io::SeekFrom, path::PathBuf, sync::Arc};

use base64::{engine::general_purpose::STANDARD, Engine};
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use sanitize_filename::sanitize;
use tauri::Emitter;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{Mutex as AsyncMutex, Notify},
};
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{unix_now_millis, FileTransferRecord, SendFilePayload, FILE_CHUNK_SIZE},
    protocol::{
        BusinessEnvelope, FileAcceptPayload, FileAckPayload, FileCancelPayload, FileChunkPayload,
        FileDataFrame, FileDataFrameKind, FileDonePayload, FileOfferPayload, FileReadyPayload,
        FileRejectPayload, FileRetransmitPayload, FILE_ACCEPT_TYPE, FILE_ACK_TYPE,
        FILE_CANCEL_TYPE, FILE_CHUNK_TYPE, FILE_DONE_TYPE, FILE_OFFER_TYPE, FILE_READY_TYPE,
        FILE_REJECT_TYPE, FILE_RETRANSMIT_TYPE,
    },
    sync::MutexExt,
};

use super::{
    progress::{
        acknowledged_file_bytes, calculate_bytes_per_second, should_send_file_ack,
        TransferPreparingPayload, TransferProgressPayload,
    },
    route::TransferRoute,
    utils::{build_file_checksum, unique_download_path, verify_file_checksum},
    AppRuntime, IncomingFileState, OutgoingFileState, LAN_SEND_WINDOW_CHUNKS,
    RELAY_SEND_WINDOW_CHUNKS, TRANSFER_PREPARING_EVENT, TRANSFER_PROGRESS_EVENT,
    TRANSFER_PROGRESS_INTERVAL_MS,
};

#[derive(Clone, Copy)]
enum ChunkTransport {
    Relay,
    Lan,
}

impl AppRuntime {
    pub fn send_files(&self, payload: SendFilePayload) -> AppResult<Vec<FileTransferRecord>> {
        if payload.paths.is_empty() {
            return Err(AppError::message("请选择文件"));
        }

        let mut records = Vec::new();
        let total = payload.paths.len();
        for (index, raw_path) in payload.paths.into_iter().enumerate() {
            let source_path = PathBuf::from(&raw_path);
            if !source_path.is_file() {
                return Err(AppError::message(format!("文件不存在: {raw_path}")));
            }
            self.emit_transfer_preparing(index + 1, total);

            let metadata = fs::metadata(&source_path)?;
            let file_size = metadata.len() as i64;
            let chunk_size = FILE_CHUNK_SIZE as i64;
            let total_chunks = if file_size == 0 {
                0
            } else {
                (file_size + chunk_size - 1) / chunk_size
            };
            let checksum = build_file_checksum(&source_path)?;
            let file_name = source_path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| AppError::message("文件名不合法"))?
                .to_string();
            let file_id = Uuid::new_v4().to_string();
            let created_at = unix_now_millis();
            let route = self.preferred_route(&payload.device_id);
            debug!(
                device_id = %payload.device_id,
                path = %source_path.display(),
                file_size = file_size,
                route = %route.as_str(),
                "preparing outgoing file transfer"
            );

            let record = FileTransferRecord {
                file_id: file_id.clone(),
                device_id: payload.device_id.clone(),
                direction: "outbound".to_string(),
                file_name: file_name.clone(),
                file_size,
                transferred_bytes: 0,
                total_chunks,
                status: "offered".to_string(),
                checksum: checksum.clone(),
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
                    source_path: source_path.clone(),
                    record: record.clone(),
                    ack_notify: Arc::new(Notify::new()),
                    acknowledged_chunks: 0,
                    last_reported_bytes: 0,
                    last_progress_at: created_at,
                },
            );

            let envelope = BusinessEnvelope::from_payload(
                FILE_OFFER_TYPE,
                FileOfferPayload {
                    session_id: file_id,
                    file_name,
                    file_size,
                    total_chunks,
                    chunk_size,
                    checksum,
                },
            )?;
            let _ = self.send_business_message(&payload.device_id, envelope)?;
            records.push(record);
        }

        self.emit_transfers()?;
        self.append_log(
            "info",
            "file",
            format!("已发送 {} 个文件邀请", records.len()),
        )?;
        Ok(records)
    }

    pub fn cancel_transfer(&self, file_id: &str) -> AppResult<()> {
        let mut outgoing_target = None;
        let mut active_record = None;
        let mut ack_notify = None;
        {
            let mut state = self.inner.state.lock_unpoisoned();
            state.cancelled_files.insert(file_id.to_string());
            if let Some(outgoing) = state.outgoing_files.get(file_id) {
                outgoing_target = Some(outgoing.record.device_id.clone());
                active_record = Some(outgoing.record.clone());
                ack_notify = Some(outgoing.ack_notify.clone());
            }
            if active_record
                .as_ref()
                .map(|record| record.status == "offered")
                .unwrap_or(false)
            {
                state.outgoing_files.remove(file_id);
            }
            if let Some(incoming) = state.incoming_files.remove(file_id) {
                if let Some(temp_path) = incoming.record.temp_path.as_ref() {
                    let _ = fs::remove_file(temp_path);
                }
                outgoing_target = Some(incoming.record.device_id.clone());
                active_record = Some(incoming.record);
            }
        }
        if let Some(notify) = ack_notify {
            notify.notify_one();
        }

        if let Some(device_id) = outgoing_target {
            let envelope = BusinessEnvelope::from_payload(
                FILE_CANCEL_TYPE,
                FileCancelPayload {
                    session_id: file_id.to_string(),
                    reason: "user cancelled".to_string(),
                },
            )?;
            let _ = self.send_business_message(&device_id, envelope);
        }
        let _ = self
            .inner
            .lan
            .send_transfer_frame(file_id, FileDataFrame::cancel("user cancelled"));
        self.inner.lan.unregister_transfer(file_id);

        let mut record = match active_record {
            Some(record) => Some(record),
            None => self.inner.database.load_transfer(file_id)?,
        };

        if let Some(record) = record.as_mut() {
            record.status = "cancelled".to_string();
            record.error = Some("user cancelled".to_string());
            record.updated_at = unix_now_millis();
            self.inner.database.save_transfer(record)?;
            self.emit_transfers()?;
        }

        self.append_log("info", "file", format!("已取消传输 {file_id}"))?;
        Ok(())
    }

    pub(super) async fn handle_file_offer(
        &self,
        from: &str,
        route: &str,
        payload: FileOfferPayload,
    ) -> AppResult<()> {
        let settings = self
            .inner
            .database
            .load_settings()?
            .ok_or_else(|| AppError::message("本地设置未初始化"))?;
        info!(
            from = %from,
            session_id = %payload.session_id,
            file_name = %payload.file_name,
            file_size = payload.file_size,
            route = %route,
            "received file offer"
        );
        let download_path = PathBuf::from(&settings.download_path);
        fs::create_dir_all(&download_path)?;
        let temp_name = format!("{}.part", sanitize(&payload.file_name));
        let temp_path = download_path.join(temp_name);

        let sender_name = self.lookup_device_name(from);
        let prompt = format!(
            "{} 想发送文件 {}\n大小: {} 字节",
            sender_name, payload.file_name, payload.file_size
        );
        let accepted = tauri::async_runtime::spawn_blocking(move || {
            MessageDialog::new()
                .set_level(MessageLevel::Info)
                .set_title("接收文件")
                .set_description(&prompt)
                .set_buttons(MessageButtons::YesNo)
                .show()
        })
        .await
        .unwrap_or(MessageDialogResult::No);

        if accepted != MessageDialogResult::Yes {
            info!(from = %from, session_id = %payload.session_id, "file offer rejected by user");
            let envelope = BusinessEnvelope::from_payload(
                FILE_REJECT_TYPE,
                FileRejectPayload {
                    session_id: payload.session_id,
                    reason: "user rejected".to_string(),
                },
            )?;
            let _ = self.send_business_message(from, envelope)?;
            return Ok(());
        }

        let created_at = unix_now_millis();
        let record = FileTransferRecord {
            file_id: payload.session_id.clone(),
            device_id: from.to_string(),
            direction: "inbound".to_string(),
            file_name: payload.file_name.clone(),
            file_size: payload.file_size,
            transferred_bytes: 0,
            total_chunks: payload.total_chunks,
            status: "receiving".to_string(),
            checksum: payload.checksum.clone(),
            route: route.to_string(),
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
                writer,
                record: record.clone(),
                received_chunks: 0,
                last_reported_bytes: 0,
                last_progress_at: created_at,
            },
        );

        let transfer_token = Uuid::new_v4().simple().to_string();
        self.inner
            .lan
            .register_transfer_token(&payload.session_id, &transfer_token);
        let envelope = BusinessEnvelope::from_payload(
            FILE_ACCEPT_TYPE,
            FileAcceptPayload {
                session_id: payload.session_id,
                transfer_token,
            },
        )?;
        let _ = self.send_business_message(from, envelope)?;
        info!(
            from,
            session_id = %record.file_id,
            route = %record.route,
            "file offer accepted"
        );
        self.emit_transfers()?;
        self.notify("文件接收", &format!("开始接收 {}", payload.file_name))?;
        if record.total_chunks == 0
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
                .ok_or_else(|| AppError::message("文件发送状态不存在"))?;
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
                    let _ = self.send_business_message(&record.device_id, ready)?;
                    return self.send_file_data_lan(file_id, source_path, record).await;
                }
                Err(error) => {
                    let reason = format!("LAN data connection failed: {error}");
                    warn!(file_id = %file_id, %error, "lan data connection failed");
                    let cancel = BusinessEnvelope::from_payload(
                        FILE_CANCEL_TYPE,
                        FileCancelPayload {
                            session_id: file_id.clone(),
                            reason: reason.clone(),
                        },
                    )?;
                    let _ = self.send_business_message(&record.device_id, cancel);
                    self.finish_outgoing_transfer(&file_id, "failed", Some(reason), None)?;
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

            self.send_transport_chunk(&file_id, &record, index, &buffer[..read], transport)?;
            index += 1;
        }

        self.send_transport_finish(&file_id, &record, transport)?;
        self.append_log(
            "info",
            "file",
            format!("文件 {} 已发送完成，等待确认", record.file_name),
        )?;
        Ok(())
    }

    fn send_transport_chunk(
        &self,
        file_id: &str,
        record: &FileTransferRecord,
        index: i64,
        bytes: &[u8],
        transport: ChunkTransport,
    ) -> AppResult<()> {
        match transport {
            ChunkTransport::Relay => {
                let chunk = BusinessEnvelope::from_payload(
                    FILE_CHUNK_TYPE,
                    FileChunkPayload {
                        session_id: file_id.to_string(),
                        chunk_index: index,
                        data: STANDARD.encode(bytes),
                    },
                )?;
                let _ = self.send_business_message(&record.device_id, chunk)?;
            }
            ChunkTransport::Lan => {
                let index =
                    u32::try_from(index).map_err(|_| AppError::message("文件分块索引过大"))?;
                self.inner
                    .lan
                    .send_transfer_frame(file_id, FileDataFrame::chunk(index, bytes.to_vec()))?;
            }
        }
        Ok(())
    }

    fn send_transport_finish(
        &self,
        file_id: &str,
        record: &FileTransferRecord,
        transport: ChunkTransport,
    ) -> AppResult<()> {
        if let ChunkTransport::Lan = transport {
            self.inner.lan.send_transfer_frame(
                file_id,
                FileDataFrame::finish(
                    u32::try_from(record.total_chunks)
                        .map_err(|_| AppError::message("文件分块数量超过协议限制"))?,
                ),
            )?;
        }
        Ok(())
    }

    fn cleanup_transport_after_cancel(&self, file_id: &str, transport: ChunkTransport) {
        if let ChunkTransport::Lan = transport {
            let _ = self
                .inner
                .lan
                .send_transfer_frame(file_id, FileDataFrame::cancel("user cancelled"));
            self.inner.lan.unregister_transfer(file_id);
        }
    }

    fn take_cancelled_outgoing(&self, file_id: &str) -> bool {
        let mut state = self.inner.state.lock_unpoisoned();
        if !state.cancelled_files.remove(file_id) {
            return false;
        }
        state.outgoing_files.remove(file_id);
        true
    }

    fn clear_outgoing_transfer_state(&self, file_id: &str) {
        let mut state = self.inner.state.lock_unpoisoned();
        state.cancelled_files.remove(file_id);
        state.outgoing_files.remove(file_id);
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
                self.handle_file_cancel(session_id, reason)
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
        let (writer, received_chunks, device_id) = {
            let state = self.inner.state.lock_unpoisoned();
            state.incoming_files.get(session_id).map(|item| {
                (
                    item.writer.clone(),
                    item.received_chunks,
                    item.record.device_id.clone(),
                )
            })
        }
        .ok_or_else(|| AppError::message("接收中的文件不存在"))?;

        if chunk_index < received_chunks {
            debug!(
                %session_id,
                chunk_index = chunk_index,
                received_chunks = received_chunks,
                "ignoring duplicate file chunk"
            );
            self.send_file_ack(&device_id, session_id, received_chunks)?;
            return Ok(());
        }
        if chunk_index > received_chunks {
            warn!(
                %session_id,
                chunk_index = chunk_index,
                received_chunks = received_chunks,
                "missing file chunk detected"
            );
            self.send_file_retransmit(&device_id, session_id, received_chunks)?;
            return Ok(());
        }

        let mut file = writer.lock().await;
        file.write_all(bytes).await?;
        drop(file);

        let updated_at = unix_now_millis();
        let (record, bytes_per_second, finished) =
            self.update_incoming_progress(session_id, bytes.len() as i64, updated_at)?;
        if let Some(bytes_per_second) = bytes_per_second {
            self.inner.database.save_transfer(&record)?;
            self.emit_transfer_progress(record.clone(), bytes_per_second);
        }
        let next_expected_index = chunk_index + 1;
        if should_send_file_ack(next_expected_index, record.total_chunks) {
            self.send_file_ack(&record.device_id, session_id, next_expected_index)?;
        }
        if finish_when_complete && finished {
            self.finish_incoming_transfer(session_id).await?;
        }
        Ok(())
    }

    async fn handle_lan_file_finish(&self, session_id: &str) -> AppResult<()> {
        let (received_chunks, total_chunks, device_id) = {
            let state = self.inner.state.lock_unpoisoned();
            state.incoming_files.get(session_id).map(|item| {
                (
                    item.received_chunks,
                    item.record.total_chunks,
                    item.record.device_id.clone(),
                )
            })
        }
        .ok_or_else(|| AppError::message("接收状态不存在"))?;

        if received_chunks < total_chunks {
            self.send_file_retransmit(&device_id, session_id, received_chunks)?;
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

    pub(super) async fn retransmit_file_chunk(
        &self,
        session_id: &str,
        chunk_index: i64,
        lan: bool,
    ) -> AppResult<()> {
        if chunk_index < 0 {
            return Ok(());
        }

        let (source_path, record) = {
            let state = self.inner.state.lock_unpoisoned();
            state
                .outgoing_files
                .get(session_id)
                .map(|item| (item.source_path.clone(), item.record.clone()))
        }
        .ok_or_else(|| AppError::message("文件发送状态不存在"))?;

        let offset = chunk_index
            .checked_mul(FILE_CHUNK_SIZE as i64)
            .ok_or_else(|| AppError::message("文件分块偏移溢出"))? as u64;
        let mut file = tokio::fs::File::open(&source_path).await?;
        file.seek(SeekFrom::Start(offset)).await?;
        let mut buffer = vec![0_u8; FILE_CHUNK_SIZE];
        let read = file.read(&mut buffer).await?;
        if read == 0 {
            return Ok(());
        }

        if lan {
            let chunk_index =
                u32::try_from(chunk_index).map_err(|_| AppError::message("文件分块索引过大"))?;
            self.inner.lan.send_transfer_frame(
                session_id,
                FileDataFrame::chunk(chunk_index, buffer[..read].to_vec()),
            )?;
        } else {
            let chunk = BusinessEnvelope::from_payload(
                FILE_CHUNK_TYPE,
                FileChunkPayload {
                    session_id: session_id.to_string(),
                    chunk_index,
                    data: STANDARD.encode(&buffer[..read]),
                },
            )?;
            let _ = self.send_business_message(&record.device_id, chunk)?;
        }

        Ok(())
    }

    async fn finish_incoming_transfer(&self, file_id: &str) -> AppResult<()> {
        let incoming = self
            .inner
            .state
            .lock_unpoisoned()
            .incoming_files
            .remove(file_id)
            .ok_or_else(|| AppError::message("接收状态不存在"))?;
        {
            let mut writer = incoming.writer.lock().await;
            writer.flush().await?;
        }

        let settings = self
            .inner
            .database
            .load_settings()?
            .ok_or_else(|| AppError::message("本地设置未初始化"))?;
        let download_dir = PathBuf::from(settings.download_path);
        fs::create_dir_all(&download_dir)?;

        let temp_path = incoming
            .record
            .temp_path
            .as_deref()
            .map(PathBuf::from)
            .ok_or_else(|| AppError::message("临时文件路径不存在"))?;
        let success = verify_file_checksum(&temp_path, &incoming.record.checksum)?;

        let final_path = if success {
            let path = unique_download_path(&download_dir, &incoming.record.file_name);
            tokio::fs::rename(&temp_path, &path).await?;
            Some(path.to_string_lossy().to_string())
        } else {
            let _ = tokio::fs::remove_file(&temp_path).await;
            None
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
            Some("checksum mismatch".to_string())
        };
        record.updated_at = unix_now_millis();
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()?;

        let done = BusinessEnvelope::from_payload(
            FILE_DONE_TYPE,
            FileDonePayload {
                session_id: file_id.to_string(),
                success,
                reason: if success {
                    None
                } else {
                    Some("checksum mismatch".to_string())
                },
            },
        )?;
        let _ = self.send_business_message(&record.device_id, done)?;
        self.inner.lan.unregister_transfer(file_id);

        if success {
            self.notify("文件接收完成", &format!("已保存 {}", record.file_name))?;
        } else {
            self.notify("文件接收失败", &format!("{} 校验失败", record.file_name))?;
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
        if let Some(notify) = removed.as_ref().map(|item| item.ack_notify.clone()) {
            notify.notify_one();
        }
        let record = removed.map(|item| item.record);
        let mut record = record
            .or(self.inner.database.load_transfer(file_id)?)
            .ok_or_else(|| AppError::message("传输记录不存在"))?;
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

    pub(super) fn handle_file_cancel(&self, file_id: &str, reason: String) -> AppResult<()> {
        warn!(%file_id, %reason, "file transfer cancelled by peer");
        self.inner.lan.unregister_transfer(file_id);
        if let Some(incoming) = self
            .inner
            .state
            .lock_unpoisoned()
            .incoming_files
            .remove(file_id)
        {
            if let Some(temp_path) = incoming.record.temp_path.as_ref() {
                let _ = fs::remove_file(temp_path);
            }
        }
        self.finish_outgoing_transfer(file_id, "cancelled", Some(reason), None)
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
            warn!(%file_id, "incoming lan transfer closed before completion");
            if let Some(temp_path) = incoming.record.temp_path.as_ref() {
                let _ = fs::remove_file(temp_path);
            }
            let mut record = incoming.record;
            record.status = "failed".to_string();
            record.error = Some("LAN data connection closed".to_string());
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
                Some("LAN data connection closed".to_string()),
                None,
            )?;
        }
        Ok(())
    }

    fn lan_endpoint_for_device(&self, device_id: &str) -> Option<(String, u16)> {
        if !self.inner.lan.has_peer(device_id) {
            debug!(%device_id, "lan endpoint unavailable because peer is not connected");
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
                .ok_or_else(|| AppError::message("文件发送状态不存在"))?;
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

    fn send_file_ack(
        &self,
        device_id: &str,
        file_id: &str,
        next_expected_index: i64,
    ) -> AppResult<()> {
        if self.transfer_route(file_id) == Some(TransferRoute::Lan) {
            let next_expected_index = u32::try_from(next_expected_index)
                .map_err(|_| AppError::message("文件分块索引过大"))?;
            self.inner
                .lan
                .send_transfer_frame(file_id, FileDataFrame::ack(next_expected_index))?;
            return Ok(());
        }

        let ack = BusinessEnvelope::from_payload(
            FILE_ACK_TYPE,
            FileAckPayload {
                session_id: file_id.to_string(),
                next_expected_index,
            },
        )?;
        let _ = self.send_business_message(device_id, ack)?;
        Ok(())
    }

    fn send_file_retransmit(
        &self,
        device_id: &str,
        file_id: &str,
        chunk_index: i64,
    ) -> AppResult<()> {
        if self.transfer_route(file_id) == Some(TransferRoute::Lan) {
            let chunk_index =
                u32::try_from(chunk_index).map_err(|_| AppError::message("文件分块索引过大"))?;
            self.inner
                .lan
                .send_transfer_frame(file_id, FileDataFrame::retransmit(chunk_index))?;
            return Ok(());
        }

        let retransmit = BusinessEnvelope::from_payload(
            FILE_RETRANSMIT_TYPE,
            FileRetransmitPayload {
                session_id: file_id.to_string(),
                chunk_index,
            },
        )?;
        let _ = self.send_business_message(device_id, retransmit)?;
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

    fn update_incoming_progress(
        &self,
        file_id: &str,
        delta_bytes: i64,
        updated_at: i64,
    ) -> AppResult<(FileTransferRecord, Option<f64>, bool)> {
        let mut state = self.inner.state.lock_unpoisoned();
        let incoming = state
            .incoming_files
            .get_mut(file_id)
            .ok_or_else(|| AppError::message("接收状态不存在"))?;
        incoming.record.transferred_bytes += delta_bytes;
        incoming.record.updated_at = updated_at;
        incoming.received_chunks += 1;
        let finished = incoming.received_chunks >= incoming.record.total_chunks;
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
        Ok((incoming.record.clone(), bytes_per_second, finished))
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
            item.error = Some("app restarted".to_string());
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
