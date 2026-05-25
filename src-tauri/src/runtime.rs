use std::{
    collections::{HashMap, HashSet},
    fs,
    io::{Read, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use clipboard_rs::{
    common::RustImage, Clipboard, ClipboardContext, ClipboardHandler, ClipboardWatcher,
    ClipboardWatcherContext, RustImageData, WatcherShutdown,
};
use rfd::{FileDialog, MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use sanitize_filename::sanitize;
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt},
    sync::{mpsc, Mutex as AsyncMutex},
    time::sleep,
};
use uuid::Uuid;

use crate::{
    device_cache::reconcile_devices,
    error::{AppError, AppResult},
    models::{
        unix_now_millis, AppLogEntry, DeviceInfo, FileTransferRecord, SendFilePayload,
        SendTextPayload, TextMessageRecord, CLIPBOARD_MAX_BYTES, FILE_CHUNK_SIZE, MAX_TEXT_LENGTH,
    },
    network::{
        cloud::{CloudConnectionManager, DEVICES_UPDATED_EVENT},
        http::HttpClient,
        lan::LanManager,
    },
    protocol::{
        AnnouncePayload, BusinessEnvelope, ClipboardSyncPayload, FileAcceptPayload, FileAckPayload,
        FileCancelPayload, FileChunkPayload, FileDataFrame, FileDataFrameKind, FileDonePayload,
        FileOfferPayload, FileReadyPayload, FileRejectPayload, FileRetransmitPayload,
        TextMessagePayload, CLIPBOARD_SYNC_TYPE, FILE_ACCEPT_TYPE, FILE_ACK_TYPE, FILE_CANCEL_TYPE,
        FILE_CHUNK_TYPE, FILE_DONE_TYPE, FILE_OFFER_TYPE, FILE_READY_TYPE, FILE_REJECT_TYPE,
        FILE_RETRANSMIT_TYPE, TEXT_MESSAGE_TYPE,
    },
    runtime_events::RuntimeEvent,
    shell,
    store::db::Database,
};

pub const MESSAGES_UPDATED_EVENT: &str = "messages-updated";
pub const TRANSFERS_UPDATED_EVENT: &str = "transfers-updated";
pub const TRANSFER_PROGRESS_EVENT: &str = "transfer-progress";
pub const TRANSFER_PREPARING_EVENT: &str = "transfer-preparing";
pub const LOGS_UPDATED_EVENT: &str = "logs-updated";
const FILE_CHECKSUM_ALGORITHM: &str = "blake3";
const FILE_HASH_BUFFER_SIZE: usize = 1_048_576;
const TRANSFER_PROGRESS_INTERVAL_MS: i64 = 500;
const LAN_SEND_WINDOW_CHUNKS: i64 = 8;
const RELAY_SEND_WINDOW_CHUNKS: i64 = 4;

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
    source_path: PathBuf,
    record: FileTransferRecord,
    acknowledged_chunks: i64,
    last_reported_bytes: i64,
    last_progress_at: i64,
}

struct IncomingFileState {
    writer: Arc<AsyncMutex<tokio::fs::File>>,
    record: FileTransferRecord,
    received_chunks: i64,
    last_reported_bytes: i64,
    last_progress_at: i64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferProgressPayload {
    record: FileTransferRecord,
    bytes_per_second: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TransferPreparingPayload {
    current: usize,
    total: usize,
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
    pub fn build(
        app: AppHandle,
        database: Database,
        http: HttpClient,
    ) -> (Self, CloudConnectionManager) {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let lan = LanManager::new(database.clone(), event_tx.clone());
        let cloud = CloudConnectionManager::new(
            app.clone(),
            database.clone(),
            http.clone(),
            lan.clone(),
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
        self.append_log(
            "info",
            "message",
            format!("已发送文本消息到 {}", payload.device_id),
        )?;
        Ok(record)
    }

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
            self.inner
                .state
                .lock()
                .expect("runtime state poisoned")
                .outgoing_files
                .insert(
                    file_id.clone(),
                    OutgoingFileState {
                        source_path: source_path.clone(),
                        record: record.clone(),
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
        {
            let mut state = self.inner.state.lock().expect("runtime state poisoned");
            state.cancelled_files.insert(file_id.to_string());
            if let Some(outgoing) = state.outgoing_files.get(file_id) {
                outgoing_target = Some(outgoing.record.device_id.clone());
                active_record = Some(outgoing.record.clone());
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

    pub fn replace_cached_devices(&self, devices: Vec<DeviceInfo>) -> AppResult<Vec<DeviceInfo>> {
        let previous = self.inner.database.load_cached_devices()?;
        let reconciled = reconcile_devices(devices, &previous, &self.inner.lan.peer_ids());
        self.inner.database.save_cached_devices(&reconciled)?;
        let _ = self
            .inner
            .app
            .emit(DEVICES_UPDATED_EVENT, reconciled.clone());
        let _ = shell::refresh_tray(&self.inner.app);
        Ok(reconciled)
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
                        items
                            .into_iter()
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
            RuntimeEvent::LanTransferFrame { session_id, frame } => {
                let _ = self.handle_lan_transfer_frame(&session_id, frame).await;
            }
            RuntimeEvent::LanTransferClosed { session_id } => {
                let _ = self.handle_lan_transfer_closed(&session_id);
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
                    let _ = self.append_log(
                        "info",
                        "message",
                        format!("收到来自 {sender_name} 的文本消息"),
                    );
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
                        let _ = runtime.start_file_send(payload).await;
                    });
                }
            }
            FILE_REJECT_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileRejectPayload>(message.payload) {
                    let _ = self.finish_outgoing_transfer(
                        &payload.session_id,
                        "rejected",
                        Some(payload.reason),
                        None,
                    );
                }
            }
            FILE_READY_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileReadyPayload>(message.payload) {
                    let _ = self.mark_incoming_route(&payload.session_id, "lan");
                }
            }
            FILE_CHUNK_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileChunkPayload>(message.payload) {
                    let _ = self.handle_file_chunk(payload).await;
                }
            }
            FILE_ACK_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileAckPayload>(message.payload) {
                    let _ = self.handle_file_ack(payload);
                }
            }
            FILE_RETRANSMIT_TYPE => {
                if let Ok(payload) =
                    serde_json::from_value::<FileRetransmitPayload>(message.payload)
                {
                    let runtime = self.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = runtime
                            .retransmit_file_chunk(&payload.session_id, payload.chunk_index, false)
                            .await;
                    });
                }
            }
            FILE_DONE_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileDonePayload>(message.payload) {
                    let status = if payload.success {
                        "completed"
                    } else {
                        "failed"
                    };
                    let _ = self.finish_outgoing_transfer(
                        &payload.session_id,
                        status,
                        payload.reason,
                        None,
                    );
                }
            }
            FILE_CANCEL_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileCancelPayload>(message.payload) {
                    let _ = self.handle_file_cancel(&payload.session_id, payload.reason);
                }
            }
            CLIPBOARD_SYNC_TYPE => {
                if let Ok(payload) = serde_json::from_value::<ClipboardSyncPayload>(message.payload)
                {
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
        self.inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .incoming_files
            .insert(
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
        self.emit_transfers()?;
        self.notify("文件接收", &format!("开始接收 {}", payload.file_name))?;
        if record.total_chunks == 0 && record.route != "lan" {
            self.finish_incoming_transfer(&record.file_id).await?;
        }
        Ok(())
    }

    async fn start_file_send(&self, payload: FileAcceptPayload) -> AppResult<()> {
        let file_id = payload.session_id;
        let (source_path, mut record) = {
            let mut state = self.inner.state.lock().expect("runtime state poisoned");
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

        if let Some((ip, port)) = self.lan_endpoint_for_device(&record.device_id) {
            match self
                .inner
                .lan
                .connect_transfer(&file_id, &payload.transfer_token, &ip, port)
                .await
            {
                Ok(()) => {
                    record.route = "lan".to_string();
                    self.update_outgoing_route(&file_id, "lan")?;
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

        record.route = "cloud".to_string();
        self.update_outgoing_route(&file_id, "cloud")?;
        self.send_file_data_relay(file_id, source_path, record)
            .await
    }

    async fn send_file_data_relay(
        &self,
        file_id: String,
        source_path: PathBuf,
        record: FileTransferRecord,
    ) -> AppResult<()> {
        let mut file = tokio::fs::File::open(&source_path).await?;
        let mut index = 0_i64;
        let mut buffer = vec![0_u8; FILE_CHUNK_SIZE];
        loop {
            {
                let state = self.inner.state.lock().expect("runtime state poisoned");
                if state.cancelled_files.contains(&file_id) {
                    drop(state);
                    let mut state = self.inner.state.lock().expect("runtime state poisoned");
                    state.cancelled_files.remove(&file_id);
                    state.outgoing_files.remove(&file_id);
                    return Ok(());
                }
            }

            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }

            if !self
                .wait_for_send_window(&file_id, index, RELAY_SEND_WINDOW_CHUNKS)
                .await?
            {
                let mut state = self.inner.state.lock().expect("runtime state poisoned");
                state.cancelled_files.remove(&file_id);
                state.outgoing_files.remove(&file_id);
                return Ok(());
            }

            let chunk = BusinessEnvelope::from_payload(
                FILE_CHUNK_TYPE,
                FileChunkPayload {
                    session_id: file_id.clone(),
                    chunk_index: index,
                    data: STANDARD.encode(&buffer[..read]),
                },
            )?;
            let _ = self.send_business_message(&record.device_id, chunk)?;
            index += 1;
        }

        self.append_log(
            "info",
            "file",
            format!("文件 {} 已发送完成，等待确认", record.file_name),
        )?;
        Ok(())
    }

    async fn send_file_data_lan(
        &self,
        file_id: String,
        source_path: PathBuf,
        record: FileTransferRecord,
    ) -> AppResult<()> {
        let mut file = tokio::fs::File::open(&source_path).await?;
        let mut index = 0_u32;
        let mut buffer = vec![0_u8; FILE_CHUNK_SIZE];
        loop {
            {
                let state = self.inner.state.lock().expect("runtime state poisoned");
                if state.cancelled_files.contains(&file_id) {
                    drop(state);
                    let _ = self
                        .inner
                        .lan
                        .send_transfer_frame(&file_id, FileDataFrame::cancel("user cancelled"));
                    let mut state = self.inner.state.lock().expect("runtime state poisoned");
                    state.cancelled_files.remove(&file_id);
                    state.outgoing_files.remove(&file_id);
                    self.inner.lan.unregister_transfer(&file_id);
                    return Ok(());
                }
            }

            let read = file.read(&mut buffer).await?;
            if read == 0 {
                break;
            }

            if !self
                .wait_for_send_window(&file_id, index as i64, LAN_SEND_WINDOW_CHUNKS)
                .await?
            {
                let _ = self
                    .inner
                    .lan
                    .send_transfer_frame(&file_id, FileDataFrame::cancel("user cancelled"));
                let mut state = self.inner.state.lock().expect("runtime state poisoned");
                state.cancelled_files.remove(&file_id);
                state.outgoing_files.remove(&file_id);
                self.inner.lan.unregister_transfer(&file_id);
                return Ok(());
            }

            self.inner.lan.send_transfer_frame(
                &file_id,
                FileDataFrame::chunk(index, buffer[..read].to_vec()),
            )?;
            index += 1;
        }

        self.inner.lan.send_transfer_frame(
            &file_id,
            FileDataFrame::finish(
                u32::try_from(record.total_chunks)
                    .map_err(|_| AppError::message("文件分块数量超过协议限制"))?,
            ),
        )?;

        self.append_log(
            "info",
            "file",
            format!("文件 {} 已发送完成，等待确认", record.file_name),
        )?;
        Ok(())
    }

    async fn handle_file_chunk(&self, payload: FileChunkPayload) -> AppResult<()> {
        let session_id = payload.session_id;
        let (writer, received_chunks, device_id) = {
            let state = self.inner.state.lock().expect("runtime state poisoned");
            state.incoming_files.get(&session_id).map(|item| {
                (
                    item.writer.clone(),
                    item.received_chunks,
                    item.record.device_id.clone(),
                )
            })
        }
        .ok_or_else(|| AppError::message("接收中的文件不存在"))?;

        if payload.chunk_index < received_chunks {
            self.send_file_ack(&device_id, &session_id, received_chunks)?;
            return Ok(());
        }
        if payload.chunk_index > received_chunks {
            self.send_file_retransmit(&device_id, &session_id, received_chunks)?;
            return Ok(());
        }

        let bytes = STANDARD.decode(payload.data)?;
        let mut file = writer.lock().await;
        file.write_all(&bytes).await?;
        drop(file);

        let updated_at = unix_now_millis();
        let (record, bytes_per_second, finished) =
            self.update_incoming_progress(&session_id, bytes.len() as i64, updated_at)?;
        if let Some(bytes_per_second) = bytes_per_second {
            self.inner.database.save_transfer(&record)?;
            self.emit_transfer_progress(record.clone(), bytes_per_second);
        }
        self.send_file_ack(&record.device_id, &session_id, payload.chunk_index + 1)?;

        if finished {
            self.finish_incoming_transfer(&session_id).await?;
        }

        Ok(())
    }

    async fn handle_lan_transfer_frame(
        &self,
        session_id: &str,
        frame: FileDataFrame,
    ) -> AppResult<()> {
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
        let (writer, received_chunks, device_id) = {
            let state = self.inner.state.lock().expect("runtime state poisoned");
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
            self.send_file_ack(&device_id, session_id, received_chunks)?;
            return Ok(());
        }
        if chunk_index > received_chunks {
            self.send_file_retransmit(&device_id, session_id, received_chunks)?;
            return Ok(());
        }

        let mut file = writer.lock().await;
        file.write_all(&bytes).await?;
        drop(file);

        let updated_at = unix_now_millis();
        let (record, bytes_per_second, _) =
            self.update_incoming_progress(session_id, bytes.len() as i64, updated_at)?;
        if let Some(bytes_per_second) = bytes_per_second {
            self.inner.database.save_transfer(&record)?;
            self.emit_transfer_progress(record.clone(), bytes_per_second);
        }
        self.send_file_ack(&record.device_id, session_id, chunk_index + 1)?;
        Ok(())
    }

    async fn handle_lan_file_finish(&self, session_id: &str) -> AppResult<()> {
        let (received_chunks, total_chunks, device_id) = {
            let state = self.inner.state.lock().expect("runtime state poisoned");
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

    fn handle_file_ack(&self, payload: FileAckPayload) -> AppResult<()> {
        let Some((record, bytes_per_second)) = self.update_outgoing_ack_progress(
            &payload.session_id,
            payload.next_expected_index,
            unix_now_millis(),
        )?
        else {
            return Ok(());
        };

        self.inner.database.save_transfer(&record)?;
        if let Some(bytes_per_second) = bytes_per_second {
            self.emit_transfer_progress(record, bytes_per_second);
        }
        Ok(())
    }

    async fn retransmit_file_chunk(
        &self,
        session_id: &str,
        chunk_index: i64,
        lan: bool,
    ) -> AppResult<()> {
        if chunk_index < 0 {
            return Ok(());
        }

        let (source_path, record) = {
            let state = self.inner.state.lock().expect("runtime state poisoned");
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
            .lock()
            .expect("runtime state poisoned")
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
            .lock()
            .expect("runtime state poisoned")
            .cancelled_files
            .remove(file_id);

        Ok(())
    }

    fn finish_outgoing_transfer(
        &self,
        file_id: &str,
        status: &str,
        error: Option<String>,
        final_path: Option<String>,
    ) -> AppResult<()> {
        let record = self
            .inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .outgoing_files
            .remove(file_id)
            .map(|item| item.record);
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
        self.inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .cancelled_files
            .remove(file_id);
        Ok(())
    }

    fn handle_file_cancel(&self, file_id: &str, reason: String) -> AppResult<()> {
        self.inner.lan.unregister_transfer(file_id);
        if let Some(incoming) = self
            .inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .incoming_files
            .remove(file_id)
        {
            if let Some(temp_path) = incoming.record.temp_path.as_ref() {
                let _ = fs::remove_file(temp_path);
            }
        }
        self.finish_outgoing_transfer(file_id, "cancelled", Some(reason), None)
    }

    fn handle_lan_transfer_closed(&self, file_id: &str) -> AppResult<()> {
        let (incoming, outgoing_active, cancelled) = {
            let mut state = self.inner.state.lock().expect("runtime state poisoned");
            let cancelled = state.cancelled_files.contains(file_id);
            let incoming = state.incoming_files.remove(file_id);
            let outgoing_active = state.outgoing_files.contains_key(file_id);
            (incoming, outgoing_active, cancelled)
        };

        if cancelled {
            return Ok(());
        }

        if let Some(incoming) = incoming {
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
            return None;
        }
        self.inner.lan.peer_endpoint(device_id).or_else(|| {
            self.inner
                .database
                .load_cached_devices()
                .ok()
                .and_then(|devices| {
                    devices.into_iter().find_map(|device| {
                        if device.device_id == device_id {
                            Some((device.local_ip?, device.local_port?))
                        } else {
                            None
                        }
                    })
                })
        })
    }

    fn update_outgoing_route(&self, file_id: &str, route: &str) -> AppResult<()> {
        let record = {
            let mut state = self.inner.state.lock().expect("runtime state poisoned");
            let outgoing = state
                .outgoing_files
                .get_mut(file_id)
                .ok_or_else(|| AppError::message("文件发送状态不存在"))?;
            outgoing.record.route = route.to_string();
            outgoing.record.updated_at = unix_now_millis();
            outgoing.record.clone()
        };
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()
    }

    fn mark_incoming_route(&self, file_id: &str, route: &str) -> AppResult<()> {
        let record = {
            let mut state = self.inner.state.lock().expect("runtime state poisoned");
            let Some(incoming) = state.incoming_files.get_mut(file_id) else {
                return Ok(());
            };
            incoming.record.route = route.to_string();
            incoming.record.updated_at = unix_now_millis();
            incoming.record.clone()
        };
        self.inner.database.save_transfer(&record)?;
        self.emit_transfers()
    }

    fn transfer_route(&self, file_id: &str) -> Option<String> {
        let state = self.inner.state.lock().expect("runtime state poisoned");
        state
            .incoming_files
            .get(file_id)
            .map(|item| item.record.route.clone())
            .or_else(|| {
                state
                    .outgoing_files
                    .get(file_id)
                    .map(|item| item.record.route.clone())
            })
    }

    fn send_file_ack(
        &self,
        device_id: &str,
        file_id: &str,
        next_expected_index: i64,
    ) -> AppResult<()> {
        if self.transfer_route(file_id).as_deref() == Some("lan") {
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
        if self.transfer_route(file_id).as_deref() == Some("lan") {
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
            {
                let state = self.inner.state.lock().expect("runtime state poisoned");
                if state.cancelled_files.contains(file_id) {
                    return Ok(false);
                }
                let Some(outgoing) = state.outgoing_files.get(file_id) else {
                    return Ok(false);
                };
                if next_chunk_index - outgoing.acknowledged_chunks < window_size {
                    return Ok(true);
                }
            }

            sleep(Duration::from_millis(20)).await;
        }
    }

    fn update_outgoing_ack_progress(
        &self,
        file_id: &str,
        next_expected_index: i64,
        updated_at: i64,
    ) -> AppResult<Option<(FileTransferRecord, Option<f64>)>> {
        let mut state = self.inner.state.lock().expect("runtime state poisoned");
        let Some(outgoing) = state.outgoing_files.get_mut(file_id) else {
            return Ok(None);
        };

        let next_expected_index = next_expected_index.clamp(0, outgoing.record.total_chunks);
        if next_expected_index <= outgoing.acknowledged_chunks {
            return Ok(None);
        }

        outgoing.acknowledged_chunks = next_expected_index;
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
        let mut state = self.inner.state.lock().expect("runtime state poisoned");
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
                let image = RustImageData::from_bytes(&bytes)
                    .map_err(|error| AppError::message(error.to_string()))?;
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

        self.inner
            .state
            .lock()
            .expect("runtime state poisoned")
            .watcher_shutdown = Some(shutdown);
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
                items
                    .into_iter()
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

    fn send_business_message(
        &self,
        device_id: &str,
        message: BusinessEnvelope,
    ) -> AppResult<String> {
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
        let previous = devices.clone();
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
        }

        let devices = reconcile_devices(devices, &previous, &self.inner.lan.peer_ids());
        self.inner.database.save_cached_devices(&devices)?;
        let _ = self.inner.app.emit(DEVICES_UPDATED_EVENT, devices);
        let _ = shell::refresh_tray(&self.inner.app);
        Ok(())
    }

    fn update_device_route(&self, _device_id: &str, _lan_available: bool) -> AppResult<()> {
        let devices = self.inner.database.load_cached_devices()?;
        let reconciled = reconcile_devices(devices.clone(), &devices, &self.inner.lan.peer_ids());
        self.inner.database.save_cached_devices(&reconciled)?;
        let _ = self.inner.app.emit(DEVICES_UPDATED_EVENT, reconciled);
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

fn build_file_checksum(path: &Path) -> AppResult<String> {
    let digest = hash_file_by_algorithm(path, FILE_CHECKSUM_ALGORITHM)?;
    Ok(format!("{FILE_CHECKSUM_ALGORITHM}:{digest}"))
}

fn verify_file_checksum(path: &Path, checksum: &str) -> AppResult<bool> {
    let (algorithm, expected) = split_checksum(checksum);
    let actual = hash_file_by_algorithm(path, algorithm)?;
    Ok(actual.eq_ignore_ascii_case(expected))
}

fn split_checksum(checksum: &str) -> (&str, &str) {
    if let Some((algorithm, digest)) = checksum.split_once(':') {
        return (algorithm, digest);
    }

    ("sha256", checksum)
}

fn hash_file_by_algorithm(path: &Path, algorithm: &str) -> AppResult<String> {
    let mut file = fs::File::open(path)?;
    let mut buffer = vec![0_u8; FILE_HASH_BUFFER_SIZE];

    match algorithm {
        "sha256" => {
            let mut hasher = Sha256::new();
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(format!("{:x}", hasher.finalize()))
        }
        "blake3" => {
            let mut hasher = blake3::Hasher::new();
            loop {
                let read = file.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                hasher.update(&buffer[..read]);
            }
            Ok(hasher.finalize().to_hex().to_string())
        }
        _ => Err(AppError::message(format!("不支持的校验算法: {algorithm}"))),
    }
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

fn calculate_bytes_per_second(delta_bytes: i64, duration_ms: i64) -> f64 {
    if delta_bytes <= 0 {
        return 0.0;
    }

    if duration_ms <= 0 {
        return delta_bytes as f64 * 1000.0;
    }

    delta_bytes as f64 * 1000.0 / duration_ms as f64
}

fn acknowledged_file_bytes(file_size: i64, total_chunks: i64, next_expected_index: i64) -> i64 {
    if file_size <= 0 || total_chunks <= 0 || next_expected_index <= 0 {
        return 0;
    }

    let acknowledged = next_expected_index
        .min(total_chunks)
        .saturating_mul(FILE_CHUNK_SIZE as i64);
    acknowledged.min(file_size)
}
