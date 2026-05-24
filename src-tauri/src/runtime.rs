use std::{
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use clipboard_rs::{
    common::RustImage,
    Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher, ClipboardWatcherContext,
    RustImageData, WatcherShutdown,
};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use sanitize_filename::sanitize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::mpsc,
};
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    models::{
        unix_now_millis, AppLogEntry, FileTransferRecord, SendFilePayload, SendTextPayload,
        TextMessageRecord, CLIPBOARD_MAX_BYTES, FILE_CHUNK_SIZE, MAX_TEXT_LENGTH,
    },
    network::{
        cloud::{CloudConnectionManager, DEVICES_UPDATED_EVENT},
        http::HttpClient,
        lan::LanManager,
    },
    protocol::{
        AnnouncePayload, BusinessEnvelope, ClipboardSyncPayload, FileAcceptPayload,
        FileCancelPayload, FileChunkPayload, FileDonePayload, FileOfferPayload,
        FileRejectPayload, TextMessagePayload,
        CLIPBOARD_SYNC_TYPE, FILE_ACCEPT_TYPE, FILE_CANCEL_TYPE, FILE_CHUNK_TYPE, FILE_DONE_TYPE,
        FILE_OFFER_TYPE, FILE_REJECT_TYPE, TEXT_MESSAGE_TYPE,
    },
    runtime_events::RuntimeEvent,
    shell,
    store::db::Database,
};

pub const MESSAGES_UPDATED_EVENT: &str = "messages-updated";
pub const TRANSFERS_UPDATED_EVENT: &str = "transfers-updated";
pub const LOGS_UPDATED_EVENT: &str = "logs-updated";

#[derive(Clone)]
pub struct AppRuntime {
    inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    app: AppHandle,
    database: Database,
    cloud: CloudConnectionManager,
    lan: LanManager,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    state: Mutex<RuntimeState>,
}

struct RuntimeState {
    watcher_shutdown: Option<WatcherShutdown>,
    outgoing_files: HashMap<String, OutgoingFileState>,
    incoming_files: HashMap<String, IncomingFileState>,
    cancelled_files: HashSet<String>,
    clipboard_suppressed_hash: Option<String>,
    clipboard_last_sent_hash: Option<String>,
    cleanup_done: bool,
}

struct OutgoingFileState {
    device_id: String,
    source_path: PathBuf,
    route: String,
}

struct IncomingFileState {
    device_id: String,
    file_name: String,
    total_chunks: i64,
    received_chunks: i64,
    checksum: String,
    route: String,
    temp_path: PathBuf,
}

struct ClipboardWatcherHandler {
    ctx: ClipboardContext,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
}

impl ClipboardHandler for ClipboardWatcherHandler {
    fn on_clipboard_change(&mut self) {
        if let Ok(payload) = read_clipboard_payload(&self.ctx) {
            let _ = self.event_tx.send(RuntimeEvent::ClipboardChanged(payload));
        }
    }
}

impl AppRuntime {
    pub fn build(app: AppHandle, database: Database, http: HttpClient) -> (Self, CloudConnectionManager) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let lan = LanManager::new(database.clone(), event_tx.clone());
        let cloud = CloudConnectionManager::new(
            app.clone(),
            database.clone(),
            http.clone(),
            event_tx.clone(),
        );
        let runtime = Self {
            inner: Arc::new(RuntimeInner {
                app,
                database,
                cloud: cloud.clone(),
                lan,
                event_tx: event_tx.clone(),
                state: Mutex::new(RuntimeState {
                    watcher_shutdown: None,
                    outgoing_files: HashMap::new(),
                    incoming_files: HashMap::new(),
                    cancelled_files: HashSet::new(),
                    clipboard_suppressed_hash: None,
                    clipboard_last_sent_hash: None,
                    cleanup_done: false,
                }),
            }),
        };
        runtime.spawn_event_loop(event_rx);
        (runtime, cloud)
    }

    pub fn activate(&self) -> AppResult<()> {
        self.cleanup_unfinished_transfers()?;
        self.inner.lan.start()?;
        self.start_clipboard_watcher()?;
        Ok(())
    }

    pub fn deactivate(&self) -> AppResult<()> {
        self.inner.lan.stop();
        self.stop_clipboard_watcher();
        let mut state = self.inner.state.lock().expect("runtime state poisoned");
        state.cancelled_files.clear();
        state.outgoing_files.clear();
        state.incoming_files.clear();
        Ok(())
    }

    pub fn send_text(&self, payload: SendTextPayload) -> AppResult<TextMessageRecord> {
        let text = payload.text.trim().to_string();
        if text.is_empty() {
            return Err(AppError::message("消息不能为空"));
        }
        if text.chars().count() > MAX_TEXT_LENGTH {
            return Err(AppError::message("消息长度不能超过 10000"));
        }

        let record = TextMessageRecord {
            message_id: Uuid::new_v4().to_string(),
            device_id: payload.device_id.clone(),
            direction: "outbound".to_string(),
            text: text.clone(),
            route: self.preferred_route(&payload.device_id),
            created_at: unix_now_millis(),
        };
        self.inner.database.save_message(&record)?;
        self.emit_messages()?;

        let envelope = BusinessEnvelope::from_payload(
            TEXT_MESSAGE_TYPE,
            TextMessagePayload {
                message_id: record.message_id.clone(),
                text,
            },
        )?;
        let _ = self.send_business_message(&payload.device_id, envelope)?;
        self.append_log("info", "message", format!("已发送文本消息到 {}", payload.device_id))?;
        Ok(record)
    }

    pub fn send_files(&self, payload: SendFilePayload) -> AppResult<Vec<FileTransferRecord>> {
        if payload.paths.is_empty() {
            return Err(AppError::message("请选择文件"));
        }

        let mut records = Vec::new();
        for raw_path in payload.paths {
            let source_path = PathBuf::from(&raw_path);
            if !source_path.is_file() {
                return Err(AppError::message(format!("文件不存在: {raw_path}")));
            }

            let metadata = fs::metadata(&source_path)?;
            let file_size = metadata.len() as i64;
            let chunk_size = FILE_CHUNK_SIZE as i64;
            let total_chunks = ((file_size + chunk_size - 1) / chunk_size).max(1);
            let checksum = format!("sha256:{}", hash_file_sha256(&source_path)?);
            let file_name = source_path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| AppError::message("文件名不合法"))?
                .to_string();
            let file_id = Uuid::new_v4().to_string();
            let created_at = unix_now_millis();

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
                route: self.preferred_route(&payload.device_id),
                temp_path: None,
                final_path: Some(source_path.to_string_lossy().to_string()),
                error: None,
                created_at,
                updated_at: created_at,
            };
            self.inner.database.save_transfer(&record)?;
            self.inner.state.lock().expect("runtime state poisoned").outgoing_files.insert(
                file_id.clone(),
                OutgoingFileState {
                    device_id: payload.device_id.clone(),
                    source_path: source_path.clone(),
                    route: self.preferred_route(&payload.device_id),
                },
            );

            let envelope = BusinessEnvelope::from_payload(
                FILE_OFFER_TYPE,
                FileOfferPayload {
                    file_id,
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
        self.append_log("info", "file", format!("已发送 {} 个文件邀请", records.len()))?;
        Ok(records)
    }

    pub fn cancel_transfer(&self, file_id: &str) -> AppResult<()> {
        let mut outgoing_target = None;
        {
            let mut state = self.inner.state.lock().expect("runtime state poisoned");
            state.cancelled_files.insert(file_id.to_string());
            if let Some(outgoing) = state.outgoing_files.get(file_id) {
                outgoing_target = Some(outgoing.device_id.clone());
            }
            if let Some(incoming) = state.incoming_files.remove(file_id) {
                let _ = fs::remove_file(&incoming.temp_path);
                outgoing_target = Some(incoming.device_id);
            }
        }

        if let Some(device_id) = outgoing_target {
            let envelope = BusinessEnvelope::from_payload(
                FILE_CANCEL_TYPE,
                FileCancelPayload {
                    file_id: file_id.to_string(),
                    reason: "user cancelled".to_string(),
                },
            )?;
            let _ = self.send_business_message(&device_id, envelope);
        }

        if let Some(mut record) = self
            .inner
            .database
            .load_transfers(500)?
            .into_iter()
            .find(|item| item.file_id == file_id)
        {
            record.status = "cancelled".to_string();
            record.error = Some("user cancelled".to_string());
            record.updated_at = unix_now_millis();
            self.inner.database.save_transfer(&record)?;
            self.emit_transfers()?;
        }

        self.append_log("info", "file", format!("已取消传输 {file_id}"))?;
        Ok(())
    }

    pub fn pick_file_paths(&self, multiple: bool) -> AppResult<Vec<String>> {
        let dialog = FileDialog::new();
        let paths = if multiple {
            dialog.pick_files().unwrap_or_default()
        } else {
            dialog.pick_file().into_iter().collect()
        };
        Ok(paths
            .into_iter()
            .map(|item| item.to_string_lossy().to_string())
            .collect())
    }

    pub fn pick_folder_path(&self) -> AppResult<Option<String>> {
        Ok(FileDialog::new()
            .pick_folder()
            .map(|item| item.to_string_lossy().to_string()))
    }

    fn spawn_event_loop(&self, mut event_rx: mpsc::UnboundedReceiver<RuntimeEvent>) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            while let Some(event) = event_rx.recv().await {
                runtime.handle_event(event).await;
            }
        });
    }

    async fn handle_event(&self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::AuthInvalidated(message) => {
                let _ = self.deactivate();
                let _ = self.append_log("warn", "auth", message);
            }
            RuntimeEvent::CloudConnected => {
                let _ = self.activate();
                let _ = self.append_log("info", "cloud", "云端连接已建立".to_string());
            }
            RuntimeEvent::CloudDisconnected(reason) => {
                let _ = self.append_log(
                    "warn",
                    "cloud",
                    reason.unwrap_or_else(|| "云端连接已断开".to_string()),
                );
            }
            RuntimeEvent::CloudRelay { from, message } => {
                self.handle_business_message(&from, "cloud", message).await;
            }
            RuntimeEvent::DevicePresence {
                device_id,
                online,
                payload,
            } => {
                let _ = self.update_device_presence_cache(&device_id, online, payload.clone());
                let device_name = self
                    .inner
                    .database
                    .load_cached_devices()
                    .ok()
                    .and_then(|items| {
                        items.into_iter()
                            .find(|item| item.device_id == device_id)
                            .map(|item| item.name)
                    })
                    .or_else(|| payload.map(|item| item.name))
                    .unwrap_or(device_id);

                let body = if online {
                    format!("{device_name} 已上线")
                } else {
                    format!("{device_name} 已离线")
                };
                let _ = self.append_log("info", "device", body.clone());
                let _ = self.notify("设备状态变化", &body);
            }
            RuntimeEvent::ClipboardChanged(payload) => {
                let _ = self.broadcast_clipboard(payload);
            }
            RuntimeEvent::LanDiscovered {
                device_id,
                ip,
                port,
                source,
            } => {
                let _ = self.append_log(
                    "info",
                    "lan",
                    format!("发现局域网设备 {device_id} @ {ip}:{port} ({source})"),
                );
            }
            RuntimeEvent::LanConnected { device_id } => {
                let _ = self.update_device_route(&device_id, true);
                let _ = self.append_log(
                    "info",
                    "lan",
                    format!("已建立 LAN 直连: {}", self.lookup_device_name(&device_id)),
                );
            }
            RuntimeEvent::LanDisconnected { device_id } => {
                let _ = self.update_device_route(&device_id, false);
                let _ = self.append_log(
                    "warn",
                    "lan",
                    format!("LAN 连接已断开: {}", self.lookup_device_name(&device_id)),
                );
            }
            RuntimeEvent::LanMessage { from, message } => {
                self.handle_business_message(&from, "lan", message).await;
            }
            RuntimeEvent::LocalEndpoint { ip, port } => {
                let _ = self.inner.cloud.announce(AnnouncePayload {
                    local_ip: ip,
                    local_port: port,
                });
            }
            RuntimeEvent::Log {
                level,
                source,
                message,
            } => {
                let _ = self.append_log(&level, &source, message);
            }
        }
    }

    async fn handle_business_message(&self, from: &str, route: &str, message: BusinessEnvelope) {
        match message.message_type.as_str() {
            TEXT_MESSAGE_TYPE => {
                if let Ok(payload) = serde_json::from_value::<TextMessagePayload>(message.payload) {
                    let record = TextMessageRecord {
                        message_id: payload.message_id,
                        device_id: from.to_string(),
                        direction: "inbound".to_string(),
                        text: payload.text.clone(),
                        route: route.to_string(),
                        created_at: unix_now_millis(),
                    };
                    let _ = self.inner.database.save_message(&record);
                    let _ = self.emit_messages();
                    let sender_name = self.lookup_device_name(from);
                    let _ = self.notify(&format!("来自 {sender_name} 的消息"), &payload.text);
                    let _ = self.append_log("info", "message", format!("收到来自 {sender_name} 的文本消息"));
                }
            }
            FILE_OFFER_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileOfferPayload>(message.payload) {
                    let _ = self.handle_file_offer(from, route, payload).await;
                }
            }
            FILE_ACCEPT_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileAcceptPayload>(message.payload) {
                    let runtime = self.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = runtime.start_file_send(payload.file_id).await;
                    });
                }
            }
            FILE_REJECT_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileRejectPayload>(message.payload) {
                    let _ = self.finish_outgoing_transfer(
                        &payload.file_id,
                        "rejected",
                        Some(payload.reason),
                        None,
                    );
                }
            }
            FILE_CHUNK_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileChunkPayload>(message.payload) {
                    let _ = self.handle_file_chunk(payload).await;
                }
            }
            FILE_DONE_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileDonePayload>(message.payload) {
                    let status = if payload.success { "completed" } else { "failed" };
                    let _ = self.finish_outgoing_transfer(
                        &payload.file_id,
                        status,
                        payload.reason,
                        None,
                    );
                }
            }
            FILE_CANCEL_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileCancelPayload>(message.payload) {
                    let _ = self.handle_file_cancel(&payload.file_id, payload.reason);
                }
            }
            CLIPBOARD_SYNC_TYPE => {
                if let Ok(payload) = serde_json::from_value::<ClipboardSyncPayload>(message.payload) {
                    let _ = self.apply_remote_clipboard(from, payload);
                }
            }
            _ => {}
        }
    }

    async fn handle_file_offer(
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
            let envelope = BusinessEnvelope::from_payload(
                FILE_REJECT_TYPE,
                FileRejectPayload {
                    file_id: payload.file_id,
                    reason: "user rejected".to_string(),
                },
            )?;
            self.inner.cloud.send_relay(from, envelope)?;
            return Ok(());
        }

        let created_at = unix_now_millis();
        let record = FileTransferRecord {
            file_id: payload.file_id.clone(),
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
        self.inner.database.save_transfer(&record)?;
        self.inner.state.lock().expect("runtime state poisoned").incoming_files.insert(
            payload.file_id.clone(),
            IncomingFileState {
                device_id: from.to_string(),
                file_name: payload.file_name.clone(),
                total_chunks: payload.total_chunks,
                received_chunks: 0,
                checksum: payload.checksum.clone(),
                route: route.to_string(),
                temp_path: temp_path.clone(),
            },
        );
        tokio::fs::File::create(&temp_path).await?;

        let envelope = BusinessEnvelope::from_payload(
            FILE_ACCEPT_TYPE,
            FileAcceptPayload {
                file_id: payload.file_id,
            },
        )?;
        let _ = self.send_business_message(from, envelope)?;
        self.emit_transfers()?;
        self.notify("文件接收", &format!("开始接收 {}", payload.file_name))?;
        Ok(())
    }

    async fn start_file_send(&self, file_id: String) -> AppResult<()> {
        let outgoing = {
            let state = self.inner.state.lock().expect("runtime state poisoned");
            state
                .outgoing_files
                .get(&file_id)
                .map(|item| (item.device_id.clone(), item.source_path.clone(), item.route.clone()))
        }
        .ok_or_else(|| AppError::message("文件发送状态不存在"))?;

        let mut record = self
            .inner
            .database
            .load_transfers(500)?
            .into_iter()
            .find(|item| item.file_id == file_id)
            .ok_or_else(|| AppError::message("传输记录不存在"))?;
        record.status = "sending".to_string();
        record.updated_at = unix_now_millis();
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()?;

        let mut file = tokio::fs::File::open(&outgoing.1).await?;
        let mut index = 0_i64;
        let mut buffer = vec![0_u8; FILE_CHUNK_SIZE];
        loop {
            {
                let state = self.inner.state.lock().expect("runtime state poisoned");
                if state.cancelled_files.contains(&file_id) {
                    return Ok(());
                }
            }

            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }

            let chunk = BusinessEnvelope::from_payload(
                FILE_CHUNK_TYPE,
                FileChunkPayload {
                    file_id: file_id.clone(),
                    index,
                    total_chunks: record.total_chunks,
                    data: STANDARD.encode(&buffer[..read]),
                },
            )?;
            let _ = self.send_business_message(&outgoing.0, chunk)?;

            record.transferred_bytes += read as i64;
            record.updated_at = unix_now_millis();
            self.inner.database.save_transfer(&record)?;
            self.emit_transfers()?;
            index += 1;
        }

        self.append_log("info", "file", format!("文件 {} 已发送完成，等待确认", record.file_name))?;
        Ok(())
    }

    async fn handle_file_chunk(&self, payload: FileChunkPayload) -> AppResult<()> {
        let incoming = {
            let state = self.inner.state.lock().expect("runtime state poisoned");
            state
                .incoming_files
                .get(&payload.file_id)
                .map(|item| IncomingFileState {
                    device_id: item.device_id.clone(),
                    file_name: item.file_name.clone(),
                    total_chunks: item.total_chunks,
                    received_chunks: item.received_chunks,
                    checksum: item.checksum.clone(),
                    route: item.route.clone(),
                    temp_path: item.temp_path.clone(),
                })
        }
        .ok_or_else(|| AppError::message("接收中的文件不存在"))?;

        if payload.index != incoming.received_chunks {
            self.handle_file_cancel(&payload.file_id, "chunk order mismatch".to_string())?;
            return Err(AppError::message("文件分块顺序错误"));
        }

        let bytes = STANDARD.decode(payload.data)?;
        let mut file = tokio::fs::OpenOptions::new()
            .append(true)
            .open(&incoming.temp_path)
            .await?;
        file.write_all(&bytes).await?;

        let mut record = self
            .inner
            .database
            .load_transfers(500)?
            .into_iter()
            .find(|item| item.file_id == payload.file_id)
            .ok_or_else(|| AppError::message("传输记录不存在"))?;
        record.transferred_bytes += bytes.len() as i64;
        record.updated_at = unix_now_millis();
        self.inner.database.save_transfer(&record)?;

        let finished = {
            let mut state = self.inner.state.lock().expect("runtime state poisoned");
            let Some(item) = state.incoming_files.get_mut(&payload.file_id) else {
                return Err(AppError::message("接收状态不存在"));
            };
            item.received_chunks += 1;
            item.received_chunks >= item.total_chunks
        };

        self.emit_transfers()?;

        if finished {
            self.finish_incoming_transfer(&payload.file_id).await?;
        }

        Ok(())
    }

    async fn finish_incoming_transfer(&self, file_id: &str) -> AppResult<()> {
        let incoming = self
            .inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .incoming_files
            .remove(file_id)
            .ok_or_else(|| AppError::message("接收状态不存在"))?;

        let settings = self
            .inner
            .database
            .load_settings()?
            .ok_or_else(|| AppError::message("本地设置未初始化"))?;
        let download_dir = PathBuf::from(settings.download_path);
        fs::create_dir_all(&download_dir)?;

        let computed = hash_file_sha256(&incoming.temp_path)?;
        let expected = incoming
            .checksum
            .strip_prefix("sha256:")
            .unwrap_or(&incoming.checksum)
            .to_string();
        let success = computed.eq_ignore_ascii_case(&expected);

        let final_path = if success {
            let path = unique_download_path(&download_dir, &incoming.file_name);
            tokio::fs::rename(&incoming.temp_path, &path).await?;
            Some(path.to_string_lossy().to_string())
        } else {
            let _ = tokio::fs::remove_file(&incoming.temp_path).await;
            None
        };

        let mut record = self
            .inner
            .database
            .load_transfers(500)?
            .into_iter()
            .find(|item| item.file_id == file_id)
            .ok_or_else(|| AppError::message("传输记录不存在"))?;
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
                file_id: file_id.to_string(),
                success,
                reason: if success {
                    None
                } else {
                    Some("checksum mismatch".to_string())
                },
            },
        )?;
        let _ = self.send_business_message(&incoming.device_id, done)?;

        if success {
            self.notify("文件接收完成", &format!("已保存 {}", incoming.file_name))?;
        } else {
            self.notify("文件接收失败", &format!("{} 校验失败", incoming.file_name))?;
        }

        Ok(())
    }

    fn finish_outgoing_transfer(
        &self,
        file_id: &str,
        status: &str,
        error: Option<String>,
        final_path: Option<String>,
    ) -> AppResult<()> {
        self.inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .outgoing_files
            .remove(file_id);

        let mut record = self
            .inner
            .database
            .load_transfers(500)?
            .into_iter()
            .find(|item| item.file_id == file_id)
            .ok_or_else(|| AppError::message("传输记录不存在"))?;
        record.status = status.to_string();
        if let Some(error) = error {
            record.error = Some(error);
        }
        if let Some(path) = final_path {
            record.final_path = Some(path);
        }
        record.updated_at = unix_now_millis();
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()?;
        Ok(())
    }

    fn handle_file_cancel(&self, file_id: &str, reason: String) -> AppResult<()> {
        if let Some(incoming) = self
            .inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .incoming_files
            .remove(file_id)
        {
            let _ = fs::remove_file(&incoming.temp_path);
        }
        self.finish_outgoing_transfer(file_id, "cancelled", Some(reason), None)
    }

    fn broadcast_clipboard(&self, payload: ClipboardSyncPayload) -> AppResult<()> {
        let hash = hash_clipboard_payload(&payload);
        {
            let mut state = self.inner.state.lock().expect("runtime state poisoned");
            if state.clipboard_suppressed_hash.as_deref() == Some(hash.as_str()) {
                state.clipboard_suppressed_hash = None;
                return Ok(());
            }
            if state.clipboard_last_sent_hash.as_deref() == Some(hash.as_str()) {
                return Ok(());
            }
            state.clipboard_last_sent_hash = Some(hash);
        }

        let my_device_id = self
            .inner
            .database
            .load_device_identity()?
            .map(|item| item.device_id)
            .unwrap_or_default();
        let devices = self.inner.database.load_cached_devices()?;
        let envelope = BusinessEnvelope::from_payload(CLIPBOARD_SYNC_TYPE, payload.clone())?;
        for device in devices
            .into_iter()
            .filter(|item| item.online && item.device_id != my_device_id)
        {
            let _ = self.send_business_message(&device.device_id, envelope.clone());
        }
        self.append_log("info", "clipboard", "已同步本地剪贴板".to_string())?;
        Ok(())
    }

    fn apply_remote_clipboard(&self, from: &str, payload: ClipboardSyncPayload) -> AppResult<()> {
        let hash = hash_clipboard_payload(&payload);
        let ctx = ClipboardContext::new().map_err(|error| AppError::message(error.to_string()))?;
        match payload.content_type.as_str() {
            "text/html" => {
                if let Some(content) = payload.content {
                    ctx.set_html(content)
                        .map_err(|error| AppError::message(error.to_string()))?;
                }
            }
            "image/png" | "image/jpeg" => {
                let data = payload
                    .data
                    .ok_or_else(|| AppError::message("剪贴板图片数据缺失"))?;
                let bytes = STANDARD.decode(data)?;
                let image =
                    RustImageData::from_bytes(&bytes).map_err(|error| AppError::message(error.to_string()))?;
                ctx.set_image(image)
                    .map_err(|error| AppError::message(error.to_string()))?;
            }
            _ => {
                if let Some(content) = payload.content {
                    ctx.set_text(content)
                        .map_err(|error| AppError::message(error.to_string()))?;
                }
            }
        }

        self.inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .clipboard_suppressed_hash = Some(hash);
        self.append_log(
            "info",
            "clipboard",
            format!("已应用来自 {} 的剪贴板", self.lookup_device_name(from)),
        )?;
        Ok(())
    }

    fn start_clipboard_watcher(&self) -> AppResult<()> {
        let state = self.inner.state.lock().expect("runtime state poisoned");
        if state.watcher_shutdown.is_some() {
            return Ok(());
        }
        drop(state);

        let ctx = ClipboardContext::new().map_err(|error| AppError::message(error.to_string()))?;
        let mut watcher =
            ClipboardWatcherContext::new().map_err(|error| AppError::message(error.to_string()))?;
        let handler = ClipboardWatcherHandler {
            ctx,
            event_tx: self.inner.event_tx.clone(),
        };
        let shutdown = watcher.add_handler(handler).get_shutdown_channel();
        thread::spawn(move || {
            watcher.start_watch();
        });

        self.inner.state.lock().expect("runtime state poisoned").watcher_shutdown = Some(shutdown);
        Ok(())
    }

    fn stop_clipboard_watcher(&self) {
        if let Some(shutdown) = self
            .inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .watcher_shutdown
            .take()
        {
            shutdown.stop();
        }
    }

    fn cleanup_unfinished_transfers(&self) -> AppResult<()> {
        let mut state = self.inner.state.lock().expect("runtime state poisoned");
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

    fn lookup_device_name(&self, device_id: &str) -> String {
        self.inner
            .database
            .load_cached_devices()
            .ok()
            .and_then(|items| {
                items.into_iter()
                    .find(|item| item.device_id == device_id)
                    .map(|item| item.name)
            })
            .unwrap_or_else(|| device_id.to_string())
    }

    fn notify(&self, title: &str, body: &str) -> AppResult<()> {
        let settings = self
            .inner
            .database
            .load_settings()?
            .ok_or_else(|| AppError::message("本地设置未初始化"))?;
        if !settings.notifications {
            return Ok(());
        }

        self.inner
            .app
            .notification()
            .builder()
            .title(title)
            .body(body)
            .show()
            .map_err(|error| AppError::message(error.to_string()))
    }

    fn append_log(&self, level: &str, source: &str, message: String) -> AppResult<()> {
        let entry = AppLogEntry {
            id: Uuid::new_v4().to_string(),
            level: level.to_string(),
            source: source.to_string(),
            message,
            created_at: unix_now_millis(),
        };
        self.inner.database.append_log(&entry)?;
        self.emit_logs()
    }

    fn emit_messages(&self) -> AppResult<()> {
        let messages = self.inner.database.load_messages(200)?;
        let _ = self.inner.app.emit(MESSAGES_UPDATED_EVENT, messages);
        Ok(())
    }

    fn emit_transfers(&self) -> AppResult<()> {
        let transfers = self.inner.database.load_transfers(200)?;
        let _ = self.inner.app.emit(TRANSFERS_UPDATED_EVENT, transfers);
        Ok(())
    }

    fn emit_logs(&self) -> AppResult<()> {
        let logs = self.inner.database.load_logs(200)?;
        let _ = self.inner.app.emit(LOGS_UPDATED_EVENT, logs);
        Ok(())
    }

    fn send_business_message(&self, device_id: &str, message: BusinessEnvelope) -> AppResult<String> {
        if self.inner.lan.has_peer(device_id) {
            self.inner.lan.send(device_id, message)?;
            Ok("lan".to_string())
        } else {
            self.inner.cloud.send_relay(device_id, message)?;
            Ok("cloud".to_string())
        }
    }

    fn preferred_route(&self, device_id: &str) -> String {
        if self.inner.lan.has_peer(device_id) {
            "lan".to_string()
        } else {
            "cloud".to_string()
        }
    }

    fn update_device_presence_cache(
        &self,
        device_id: &str,
        online: bool,
        payload: Option<crate::protocol::DeviceOnlinePayload>,
    ) -> AppResult<()> {
        let mut devices = self.inner.database.load_cached_devices()?;
        let Some(device) = devices.iter_mut().find(|item| item.device_id == device_id) else {
            return Ok(());
        };
        device.online = online;
        if let Some(payload) = payload {
            device.local_ip = payload.local_ip;
            device.local_port = payload.local_port;
            device.name = payload.name;
            device.device_type = payload.device_type;
            if !device.lan_available {
                device.active_route = Some("cloud".to_string());
            }
        } else if !online {
            device.local_ip = None;
            device.local_port = None;
            device.lan_available = false;
            device.active_route = None;
        }
        self.inner.database.save_cached_devices(&devices)?;
        let _ = self.inner.app.emit(DEVICES_UPDATED_EVENT, devices);
        let _ = shell::refresh_tray(&self.inner.app);
        Ok(())
    }

    fn update_device_route(&self, device_id: &str, lan_available: bool) -> AppResult<()> {
        let mut devices = self.inner.database.load_cached_devices()?;
        let Some(device) = devices.iter_mut().find(|item| item.device_id == device_id) else {
            return Ok(());
        };
        device.lan_available = lan_available;
        device.security_state = if lan_available {
            "verified".to_string()
        } else {
            device.security_state.clone()
        };
        device.active_route = if lan_available {
            Some("lan".to_string())
        } else if device.online {
            Some("cloud".to_string())
        } else {
            None
        };
        self.inner.database.save_cached_devices(&devices)?;
        let _ = self.inner.app.emit(DEVICES_UPDATED_EVENT, devices);
        let _ = shell::refresh_tray(&self.inner.app);
        Ok(())
    }
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

    Err(AppError::message("剪贴板内容不支持或超过 1MB"))
}

fn hash_clipboard_payload(payload: &ClipboardSyncPayload) -> String {
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

fn hash_file_sha256(path: &Path) -> AppResult<String> {
    let bytes = fs::read(path)?;
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    Ok(format!("{:x}", hasher.finalize()))
}

fn unique_download_path(download_dir: &Path, file_name: &str) -> PathBuf {
    let safe_name = sanitize(file_name);
    let candidate = download_dir.join(&safe_name);
    if !candidate.exists() {
        return candidate;
    }

    let stem = Path::new(&safe_name)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("file");
    let extension = Path::new(&safe_name)
        .extension()
        .and_then(|value| value.to_str())
        .map(|value| format!(".{value}"))
        .unwrap_or_default();

    for index in 2..1000 {
        let name = format!("{stem} ({index}){extension}");
        let path = download_dir.join(name);
        if !path.exists() {
            return path;
        }
    }

    download_dir.join(format!("{}-{}", Uuid::new_v4(), safe_name))
}
