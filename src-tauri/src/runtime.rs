use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
};

use base64::{engine::general_purpose::STANDARD, Engine};
use clipboard_rs::{
    Clipboard, ClipboardContext, ClipboardWatcher, ClipboardWatcherContext, WatcherShutdown,
};
use rfd::FileDialog;
use tauri::{AppHandle, Emitter};
use tauri_plugin_notification::NotificationExt;
use tokio::sync::{mpsc, Mutex as AsyncMutex, Notify};
use uuid::Uuid;

mod clipboard;
mod progress;
mod route;
mod transfer;
mod utils;

use self::clipboard::{
    clipboard_image_from_bytes, hash_clipboard_payload, ClipboardWatcherHandler,
};
use self::route::TransferRoute;

use crate::{
    device_presence,
    error::{AppError, AppResult},
    models::{
        unix_now_millis, AppLogEntry, DeviceInfo, FileTransferRecord, SendTextPayload,
        TextMessageRecord, MAX_TEXT_LENGTH,
    },
    network::{
        cloud::CloudConnectionManager,
        http::HttpClient,
        lan::LanManager,
    },
    protocol::{
        AnnouncePayload, BusinessEnvelope, ClipboardSyncPayload, FileAcceptPayload, FileAckPayload,
        FileCancelPayload, FileChunkPayload, FileDonePayload, FileOfferPayload, FileReadyPayload,
        FileRejectPayload, FileRetransmitPayload, TextMessagePayload, CLIPBOARD_SYNC_TYPE,
        FILE_ACCEPT_TYPE, FILE_ACK_TYPE, FILE_CANCEL_TYPE, FILE_CHUNK_TYPE, FILE_DONE_TYPE,
        FILE_OFFER_TYPE, FILE_READY_TYPE, FILE_REJECT_TYPE, FILE_RETRANSMIT_TYPE,
        TEXT_MESSAGE_TYPE,
    },
    runtime_events::RuntimeEvent,
    store::db::Database,
    sync::MutexExt,
};

pub const MESSAGES_UPDATED_EVENT: &str = "messages-updated";
pub const TRANSFERS_UPDATED_EVENT: &str = "transfers-updated";
pub const TRANSFER_PROGRESS_EVENT: &str = "transfer-progress";
pub const TRANSFER_PREPARING_EVENT: &str = "transfer-preparing";
pub const LOGS_UPDATED_EVENT: &str = "logs-updated";
const TRANSFER_PROGRESS_INTERVAL_MS: i64 = 500;
const FILE_ACK_INTERVAL_CHUNKS: i64 = 7;
const LAN_SEND_WINDOW_CHUNKS: i64 = 8;
const RELAY_SEND_WINDOW_CHUNKS: i64 = FILE_ACK_INTERVAL_CHUNKS;

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
    ack_notify: Arc<Notify>,
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
        let mut state = self.inner.state.lock_unpoisoned();
        let notifiers = state
            .outgoing_files
            .values()
            .map(|outgoing| outgoing.ack_notify.clone())
            .collect::<Vec<_>>();
        state.cancelled_files.clear();
        state.outgoing_files.clear();
        state.incoming_files.clear();
        drop(state);
        for notify in notifiers {
            notify.notify_one();
        }
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
        device_presence::replace_all(
            &self.inner.database,
            &self.inner.app,
            &self.inner.lan.peer_ids(),
            devices,
        )
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
                let _ = device_presence::update_one(
                    &self.inner.database,
                    &self.inner.app,
                    &self.inner.lan.peer_ids(),
                    &device_id,
                    online,
                    payload.clone(),
                );
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
            RuntimeEvent::DevicesSnapshot(devices) => {
                let _ = self.replace_cached_devices(devices);
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
                let _ = self.reconcile_device_routes();
                let _ = self.append_log(
                    "info",
                    "lan",
                    format!("已建立 LAN 直连: {}", self.lookup_device_name(&device_id)),
                );
            }
            RuntimeEvent::LanDisconnected { device_id } => {
                let _ = self.reconcile_device_routes();
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
                    let _ = self.mark_incoming_route(&payload.session_id, TransferRoute::Lan);
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

    fn broadcast_clipboard(&self, payload: ClipboardSyncPayload) -> AppResult<()> {
        let hash = hash_clipboard_payload(&payload);
        {
            let mut state = self.inner.state.lock_unpoisoned();
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
                let image = clipboard_image_from_bytes(&bytes)?;
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

        self.inner.state.lock_unpoisoned().clipboard_suppressed_hash = Some(hash);
        self.append_log(
            "info",
            "clipboard",
            format!("已应用来自 {} 的剪贴板", self.lookup_device_name(from)),
        )?;
        Ok(())
    }

    fn start_clipboard_watcher(&self) -> AppResult<()> {
        let state = self.inner.state.lock_unpoisoned();
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

        self.inner.state.lock_unpoisoned().watcher_shutdown = Some(shutdown);
        Ok(())
    }

    fn stop_clipboard_watcher(&self) {
        if let Some(shutdown) = self.inner.state.lock_unpoisoned().watcher_shutdown.take() {
            shutdown.stop();
        }
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

    fn reconcile_device_routes(&self) -> AppResult<()> {
        device_presence::reconcile_routes(
            &self.inner.database,
            &self.inner.app,
            &self.inner.lan.peer_ids(),
        )?;
        Ok(())
    }
}
