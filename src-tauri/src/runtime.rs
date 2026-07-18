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
use tokio::{
    sync::{mpsc, oneshot, Mutex as AsyncMutex, Notify},
    time::Duration,
};
use tracing::{debug, info, warn};
use uuid::Uuid;

mod clipboard;
mod filesystem;
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
    i18n::{self, TextKey},
    models::{
        unix_now_millis, AppLogEntry, DeviceInfo, FileTransferRecord, LanPairingCandidate,
        LanPairingDecisionPayload, SendTextPayload, StartLanPairingPayload, TextMessageRecord,
        RemoteFilesystemDownload, MAX_TEXT_LENGTH,
    },
    music::MusicService,
    network::{
        cloud::CloudConnectionManager, http::HttpClient, lan::LanManager,
        transport::TransportManager,
    },
    protocol::{
        BusinessEnvelope, ClipboardSyncPayload, FileAcceptPayload, FileAckPayload,
        FileCancelPayload, FileChunkPayload, FileDonePayload, FileOfferPayload, FileReadyPayload,
        FileRejectPayload, FileRetransmitPayload, TextMessagePayload, CLIPBOARD_SYNC_TYPE,
        FILE_ACCEPT_TYPE, FILE_ACK_TYPE, FILE_CANCEL_TYPE, FILE_CHUNK_TYPE, FILE_DONE_TYPE,
        FILE_OFFER_TYPE, FILE_READY_TYPE, FILE_REJECT_TYPE, FILE_RETRANSMIT_TYPE, MUSIC_ALIVE_TYPE,
        MUSIC_REQUEST_TYPE, SYSINFO_ALIVE_TYPE, TEXT_MESSAGE_TYPE, FS_DOWNLOAD_TYPE,
        FS_ERROR_TYPE, FS_LIST_RESULT_TYPE, FS_LIST_TYPE, FS_ROOTS_RESULT_TYPE, FS_ROOTS_TYPE,
        FS_STAT_RESULT_TYPE, FS_STAT_TYPE,
    },
    runtime_events::RuntimeEvent,
    store::db::Database,
    sysinfo::SysInfoService,
    sync::MutexExt,
};

pub const MESSAGES_UPDATED_EVENT: &str = "messages-updated";
pub const TRANSFERS_UPDATED_EVENT: &str = "transfers-updated";
pub const TRANSFER_PROGRESS_EVENT: &str = "transfer-progress";
pub const TRANSFER_PREPARING_EVENT: &str = "transfer-preparing";
pub const LOGS_UPDATED_EVENT: &str = "logs-updated";
pub const LAN_PAIRING_REQUESTED_EVENT: &str = "lan-pairing-requested";
pub const LAN_PAIRING_COMPLETED_EVENT: &str = "lan-pairing-completed";
pub const LAN_PAIRING_FAILED_EVENT: &str = "lan-pairing-failed";
pub const LAN_PAIRING_CANDIDATES_UPDATED_EVENT: &str = "lan-pairing-candidates-updated";
pub const FILE_OFFER_REQUESTED_EVENT: &str = "file-offer-requested";
pub const FILE_OFFER_ENDED_EVENT: &str = "file-offer-ended";
pub const REMOTE_FILESYSTEM_DOWNLOADS_UPDATED_EVENT: &str = "remote-filesystem-downloads-updated";
const TRANSFER_PROGRESS_INTERVAL_MS: i64 = 500;
const FILE_ACK_INTERVAL_CHUNKS: i64 = 7;
const LAN_SEND_WINDOW_CHUNKS: i64 = 8;
const RELAY_SEND_WINDOW_CHUNKS: i64 = FILE_ACK_INTERVAL_CHUNKS;
pub(super) const FILESYSTEM_REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
pub(super) const FILESYSTEM_DOWNLOAD_OFFER_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct AppRuntime {
    inner: Arc<RuntimeInner>,
}

struct RuntimeInner {
    app: AppHandle,
    database: Database,
    cloud: CloudConnectionManager,
    lan: LanManager,
    transport: TransportManager,
    music: MusicService,
    sysinfo: SysInfoService,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    state: Mutex<RuntimeState>,
}

struct RuntimeState {
    watcher_shutdown: Option<WatcherShutdown>,
    outgoing_files: HashMap<String, OutgoingFileState>,
    incoming_files: HashMap<String, IncomingFileState>,
    pending_file_offers: HashMap<String, PendingFileOfferState>,
    pending_filesystem_requests: HashMap<String, PendingFilesystemRequest>,
    remote_filesystem_downloads: HashMap<String, RemoteFilesystemDownload>,
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
    verifier: Arc<AsyncMutex<crate::runtime::utils::FileChecksumVerifier>>,
    record: FileTransferRecord,
    received_chunks: i64,
    lan_finish_received: bool,
    last_reported_bytes: i64,
    last_progress_at: i64,
}

#[derive(Clone)]
pub(super) struct PendingFileOfferState {
    from: String,
    route: String,
    envelope_id: Option<String>,
    filesystem_download_id: Option<String>,
    payload: FileOfferPayload,
}

pub(super) struct PendingFilesystemRequest {
    pub(super) device_id: String,
    pub(super) expected_response_type: &'static str,
    pub(super) sender: oneshot::Sender<AppResult<BusinessEnvelope>>,
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
        let transport = TransportManager::new(database.clone(), lan.clone(), cloud.clone());
        let music = MusicService::new(
            app.clone(),
            database.clone(),
            transport.clone(),
            event_tx.clone(),
        );
        let sysinfo = SysInfoService::new(app.clone(), transport.clone(), event_tx.clone());
        let runtime = Self {
            inner: Arc::new(RuntimeInner {
                app,
                database,
                cloud: cloud.clone(),
                lan,
                transport,
                music,
                sysinfo,
                event_tx: event_tx.clone(),
                state: Mutex::new(RuntimeState {
                    watcher_shutdown: None,
                    outgoing_files: HashMap::new(),
                    incoming_files: HashMap::new(),
                    pending_file_offers: HashMap::new(),
                    pending_filesystem_requests: HashMap::new(),
                    remote_filesystem_downloads: HashMap::new(),
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
        self.inner.music.stop();
        self.inner.sysinfo.stop();
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
        state.pending_file_offers.clear();
        state.pending_filesystem_requests.clear();
        state.remote_filesystem_downloads.clear();
        drop(state);
        for notify in notifiers {
            notify.notify_one();
        }
        Ok(())
    }

    pub fn reload_music_config(&self) {
        self.inner.music.notify_config_change();
    }

    pub fn begin_local_castboard(&self, window_label: &str) {
        self.inner.music.begin_local_session(window_label);
        self.inner.sysinfo.begin_local_session(window_label);
    }

    pub fn end_local_castboard(&self, window_label: &str) {
        self.inner.music.end_local_session(window_label);
        self.inner.sysinfo.end_local_session(window_label);
    }

    pub async fn send_text(&self, payload: SendTextPayload) -> AppResult<TextMessageRecord> {
        let text = payload.text.trim().to_string();
        if text.is_empty() {
            return Err(AppError::message(self.user_text(TextKey::MessageEmpty)));
        }
        if text.chars().count() > MAX_TEXT_LENGTH {
            return Err(AppError::message(self.user_text(TextKey::MessageTooLong)));
        }

        let message_id = Uuid::new_v4().to_string();

        let envelope = BusinessEnvelope::from_payload(
            TEXT_MESSAGE_TYPE,
            TextMessagePayload {
                message_id: message_id.clone(),
                text,
            },
        )?;
        let route = self
            .send_business_message(&payload.device_id, envelope)
            .await?;
        let record = TextMessageRecord {
            message_id,
            device_id: payload.device_id.clone(),
            direction: "outbound".to_string(),
            text: payload.text.trim().to_string(),
            route,
            created_at: unix_now_millis(),
        };
        self.inner.database.save_message(&record)?;
        self.emit_messages()?;
        self.append_log(
            "info",
            "message",
            format!("sent text message to {}", payload.device_id),
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

    pub fn clear_transfers(&self) -> AppResult<()> {
        self.inner.database.clear_transfers()?;
        self.emit_transfers()?;
        Ok(())
    }

    pub fn replace_cached_devices(
        &self,
        devices: Vec<DeviceInfo>,
        cloud_snapshot: bool,
    ) -> AppResult<Vec<DeviceInfo>> {
        device_presence::replace_all(
            &self.inner.database,
            &self.inner.app,
            &self.inner.lan.trusted_member_states(),
            &self.inner.lan.trusted_member_types(),
            &self.inner.lan.peer_endpoints(),
            devices,
            cloud_snapshot,
        )
    }

    pub fn reset_cached_device_presence(&self) -> AppResult<Vec<DeviceInfo>> {
        device_presence::reset_cached_presence(&self.inner.database, &self.inner.app)
    }

    pub fn list_lan_pairing_candidates(&self) -> Vec<LanPairingCandidate> {
        self.inner.lan.list_pairing_candidates()
    }

    pub fn start_lan_pairing(&self, payload: StartLanPairingPayload) -> AppResult<()> {
        self.inner.lan.start_pairing(&payload.device_id)
    }

    pub fn respond_lan_pairing(&self, payload: LanPairingDecisionPayload) -> AppResult<()> {
        self.inner
            .lan
            .respond_pairing(&payload.request_id, payload.accepted)
    }

    pub fn forget_lan_trust(&self, device_id: &str) -> AppResult<Vec<DeviceInfo>> {
        self.inner.lan.forget_trust(device_id)?;
        self.reconcile_device_routes()
    }

    fn spawn_event_loop(&self, mut event_rx: mpsc::UnboundedReceiver<RuntimeEvent>) {
        let runtime = self.clone();
        tauri::async_runtime::spawn(async move {
            info!("runtime event loop started");
            while let Some(event) = event_rx.recv().await {
                runtime.handle_event(event).await;
            }
            info!("runtime event loop stopped");
        });
    }

    async fn handle_event(&self, event: RuntimeEvent) {
        match event {
            RuntimeEvent::AuthInvalidated(message) => {
                warn!(%message, "runtime received auth invalidation");
                let _ = self.append_log("warn", "auth", message);
            }
            RuntimeEvent::CloudConnected => {
                info!("runtime received cloud connected");
                let _ = self.activate();
                let _ =
                    self.append_log("info", "cloud", "cloud connection established".to_string());
            }
            RuntimeEvent::CloudDisconnected(reason) => {
                warn!(
                    reason = reason.as_deref().unwrap_or("unknown"),
                    "runtime received cloud disconnected"
                );
                let _ = self.append_log(
                    "warn",
                    "cloud",
                    reason.unwrap_or_else(|| "cloud connection disconnected".to_string()),
                );
            }
            RuntimeEvent::CloudUnavailable => {
                debug!("runtime received cloud unavailable");
                let _ = device_presence::mark_cloud_unavailable(
                    &self.inner.database,
                    &self.inner.app,
                    &self.inner.lan.trusted_member_states(),
                    &self.inner.lan.trusted_member_types(),
                    &self.inner.lan.peer_endpoints(),
                );
            }
            RuntimeEvent::CloudRelay {
                from,
                envelope_id,
                correlation_id,
                message,
            } => {
                debug!(%from, message_type = %message.message_type, "runtime received cloud relay");
                self
                    .handle_business_message(&from, "cloud", envelope_id, correlation_id, message)
                    .await;
            }
            RuntimeEvent::DevicePresence {
                device_id,
                online,
                payload,
            } => {
                debug!(%device_id, online = online, "runtime received device presence");
                let _ = device_presence::update_one(
                    &self.inner.database,
                    &self.inner.app,
                    &self.inner.lan.trusted_member_states(),
                    &self.inner.lan.trusted_member_types(),
                    &self.inner.lan.peer_endpoints(),
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
                    format!("{device_name} is online")
                } else {
                    format!("{device_name} is offline")
                };
                let _ = self.append_log("info", "device", body);
            }
            RuntimeEvent::DevicesSnapshot(devices) => {
                debug!(count = devices.len(), "runtime received devices snapshot");
                let _ = self.replace_cached_devices(devices, true);
            }
            RuntimeEvent::ClipboardChanged(payload) => {
                debug!(content_type = %payload.content_type, "runtime received clipboard change");
                let _ = self.broadcast_clipboard(payload).await;
            }
            RuntimeEvent::LanDiscovered {
                device_id,
                ip,
                port,
                source,
            } => {
                debug!(%device_id, %ip, port = port, %source, "runtime received lan discovery");
                let _ = self.append_log(
                    "info",
                    "lan",
                    format!("discovered LAN device {device_id} @ {ip}:{port} ({source})"),
                );
            }
            RuntimeEvent::LanConnected { device_id } => {
                info!(%device_id, "runtime received lan connected");
                let _ = self.reconcile_device_routes();
                let _ = self.append_log(
                    "info",
                    "lan",
                    format!(
                        "LAN direct connection established: {}",
                        self.lookup_device_name(&device_id)
                    ),
                );
            }
            RuntimeEvent::LanDisconnected { device_id } => {
                warn!(%device_id, "runtime received lan disconnected");
                let _ = self.reconcile_device_routes();
                let _ = self.append_log(
                    "warn",
                    "lan",
                    format!(
                        "LAN connection disconnected: {}",
                        self.lookup_device_name(&device_id)
                    ),
                );
            }
            RuntimeEvent::LanDeviceReachable { device_id } => {
                debug!(%device_id, "runtime received lan device reachable");
                let _ = self.reconcile_device_routes();
            }
            RuntimeEvent::LanDeviceUnreachable { device_id } => {
                debug!(%device_id, "runtime received lan device unreachable");
                let _ = self.reconcile_device_routes();
            }
            RuntimeEvent::LanDeviceStateChanged { device_id } => {
                debug!(%device_id, "runtime received lan device state changed");
                let _ = self.reconcile_device_routes();
            }
            RuntimeEvent::LanKeyChanged { device_id, name } => {
                warn!(%device_id, "runtime received lan key changed");
                let _ = self.reconcile_device_routes();
                let _ = self.inner.app.emit(
                    "lan-key-changed",
                    serde_json::json!({
                        "deviceId": device_id,
                        "name": name,
                    }),
                );
                let _ = self.append_log(
                    "warn",
                    "lan",
                    "LAN device key changed; LAN trust was revoked".to_string(),
                );
            }
            RuntimeEvent::LanSendFailed {
                device_id,
                messages,
            } => {
                warn!(%device_id, count = messages.len(), "runtime received failed lan sends");
                if self.inner.cloud.is_connected() {
                    for message in messages {
                        let _ = self.inner.cloud.send_relay(
                            &device_id,
                            message.message,
                            message.envelope_id,
                            message.correlation_id,
                        );
                    }
                }
            }
            RuntimeEvent::LanMessage {
                from,
                envelope_id,
                correlation_id,
                message,
            } => {
                debug!(%from, message_type = %message.message_type, "runtime received lan message");
                self
                    .handle_business_message(
                        &from,
                        "lan",
                        Some(envelope_id),
                        correlation_id,
                        message,
                    )
                    .await;
            }
            RuntimeEvent::LanTransferFrame { session_id, frame } => {
                debug!(%session_id, "runtime received lan transfer frame");
                let _ = self.handle_lan_transfer_frame(&session_id, frame).await;
            }
            RuntimeEvent::LanTransferClosed { session_id } => {
                debug!(%session_id, "runtime received lan transfer closed");
                let _ = self.handle_lan_transfer_closed(&session_id);
            }
            RuntimeEvent::LanPairingRequested(request) => {
                debug!(device_id = %request.device_id, reason = %request.reason, "runtime received lan pairing request");
                let device_name = if request.name.trim().is_empty() {
                    request.device_id.clone()
                } else {
                    request.name.clone()
                };
                let body = self.user_message(
                    TextKey::PairingRequestBody,
                    &[("name", device_name), ("code", request.code.clone())],
                );
                let _ = self.inner.app.emit(LAN_PAIRING_REQUESTED_EVENT, request);
                let _ = self.notify(TextKey::PairingRequestTitle, &[], &body);
                let _ = crate::shell::show_main_window(&self.inner.app, None);
            }
            RuntimeEvent::LanPairingCompleted(payload) => {
                debug!(
                    device_id = %payload.device_id,
                    request_id = %payload.request_id,
                    "runtime received lan pairing completed"
                );
                let _ = self.inner.app.emit(LAN_PAIRING_COMPLETED_EVENT, payload);
            }
            RuntimeEvent::LanPairingFailed(payload) => {
                debug!(
                    device_id = %payload.device_id,
                    request_id = %payload.request_id,
                    reason = %payload.reason,
                    "runtime received lan pairing failed"
                );
                let _ = self.inner.app.emit(LAN_PAIRING_FAILED_EVENT, payload);
            }
            RuntimeEvent::LanPairingCandidatesUpdated(candidates) => {
                debug!(
                    count = candidates.len(),
                    "runtime received lan pairing candidates"
                );
                let _ = self
                    .inner
                    .app
                    .emit(LAN_PAIRING_CANDIDATES_UPDATED_EVENT, candidates);
            }
            RuntimeEvent::Log {
                level,
                source,
                message,
            } => {
                debug!(%level, %source, "runtime received app log event");
                let _ = self.append_log(&level, &source, message);
            }
        }
    }

    async fn handle_business_message(
        &self,
        from: &str,
        route: &str,
        envelope_id: Option<String>,
        correlation_id: Option<String>,
        message: BusinessEnvelope,
    ) {
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
                    let _ = self.notify(
                        TextKey::MessageFromTitle,
                        &[("name", sender_name.clone())],
                        &payload.text,
                    );
                    let _ = crate::shell::show_main_window(
                        &self.inner.app,
                        Some(&self.device_route("/messages", from)),
                    );
                    let _ = self.append_log(
                        "info",
                        "message",
                        format!("received text message from {sender_name}"),
                    );
                }
            }
            FILE_OFFER_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileOfferPayload>(message.payload) {
                    let _ = self
                        .handle_file_offer(from, route, envelope_id, correlation_id, payload)
                        .await;
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
                        Some(payload.message),
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
                        payload.message.or(payload.reason),
                        None,
                    );
                }
            }
            FILE_CANCEL_TYPE => {
                if let Ok(payload) = serde_json::from_value::<FileCancelPayload>(message.payload) {
                    let _ = self.handle_file_cancel(&payload.session_id, payload.message);
                }
            }
            CLIPBOARD_SYNC_TYPE => {
                if let Ok(payload) = serde_json::from_value::<ClipboardSyncPayload>(message.payload)
                {
                    let _ = self.apply_remote_clipboard(from, payload);
                }
            }
            MUSIC_ALIVE_TYPE => {
                let music = self.inner.music.clone();
                let from = from.to_string();
                tauri::async_runtime::spawn(async move {
                    music.handle_alive(&from).await;
                });
            }
            MUSIC_REQUEST_TYPE => {
                let music = self.inner.music.clone();
                let from = from.to_string();
                tauri::async_runtime::spawn(async move {
                    music.handle_request(&from, envelope_id).await;
                });
            }
            SYSINFO_ALIVE_TYPE => {
                let sysinfo = self.inner.sysinfo.clone();
                let from = from.to_string();
                tauri::async_runtime::spawn(async move {
                    sysinfo.handle_alive(&from).await;
                });
            }
            FS_ROOTS_TYPE | FS_LIST_TYPE | FS_STAT_TYPE | FS_DOWNLOAD_TYPE => {
                self
                    .handle_filesystem_message(
                        from,
                        envelope_id.or(correlation_id),
                        message,
                    )
                    .await;
            }
            FS_ROOTS_RESULT_TYPE | FS_LIST_RESULT_TYPE | FS_STAT_RESULT_TYPE => {
                self.complete_filesystem_request(
                    from,
                    correlation_id.as_deref().or(envelope_id.as_deref()),
                    &message,
                );
            }
            FS_ERROR_TYPE => {
                let request_id = correlation_id.as_deref().or(envelope_id.as_deref());
                self.complete_filesystem_request(from, request_id, &message);
                self.complete_remote_filesystem_download_error(from, request_id, &message);
            }
            _ => {}
        }
    }

    async fn broadcast_clipboard(&self, payload: ClipboardSyncPayload) -> AppResult<()> {
        let settings =
            self.inner.database.load_settings()?.ok_or_else(|| {
                AppError::message(self.user_text(TextKey::SettingsNotInitialized))
            })?;
        if !settings.clipboard_sync {
            return Ok(());
        }
        if !self.inner.cloud.is_connected() {
            return Ok(());
        }

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

        let envelope = BusinessEnvelope::from_payload(CLIPBOARD_SYNC_TYPE, payload.clone())?;
        self.inner.transport.broadcast_cloud(envelope, None)?;
        self.append_log("info", "clipboard", "synced local clipboard".to_string())?;
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
                    .ok_or_else(|| AppError::message("clipboard image data is missing"))?;
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
            format!("applied clipboard from {}", self.lookup_device_name(from)),
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

    fn notify(
        &self,
        title_key: TextKey,
        title_args: &[(&str, String)],
        body: &str,
    ) -> AppResult<()> {
        let settings =
            self.inner.database.load_settings()?.ok_or_else(|| {
                AppError::message(self.user_text(TextKey::SettingsNotInitialized))
            })?;
        let title = i18n::message(&settings.language, title_key, title_args);
        self.inner
            .app
            .notification()
            .builder()
            .title(&title)
            .body(body)
            .show()
            .map_err(|error| AppError::message(error.to_string()))
    }

    fn device_route(&self, path: &str, device_id: &str) -> String {
        let query = url::form_urlencoded::Serializer::new(String::new())
            .append_pair("deviceId", device_id)
            .finish();
        format!("{path}?{query}")
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

    pub(super) async fn send_business_message(
        &self,
        device_id: &str,
        message: BusinessEnvelope,
    ) -> AppResult<String> {
        self.send_business_message_with_ids(device_id, message, None, None)
            .await
    }

    pub(super) async fn send_business_message_with_correlation(
        &self,
        device_id: &str,
        message: BusinessEnvelope,
        correlation_id: Option<String>,
    ) -> AppResult<String> {
        self.send_business_message_with_ids(device_id, message, None, correlation_id)
            .await
    }

    pub(super) async fn send_business_message_with_envelope_id(
        &self,
        device_id: &str,
        message: BusinessEnvelope,
        envelope_id: String,
    ) -> AppResult<String> {
        self
            .send_business_message_with_ids(device_id, message, Some(envelope_id), None)
            .await
    }

    async fn send_business_message_with_ids(
        &self,
        device_id: &str,
        message: BusinessEnvelope,
        envelope_id: Option<String>,
        correlation_id: Option<String>,
    ) -> AppResult<String> {
        self
            .inner
            .transport
            .send(device_id, message, envelope_id, correlation_id)
            .await
    }

    pub fn reconcile_device_routes(&self) -> AppResult<Vec<DeviceInfo>> {
        let devices = device_presence::reconcile_routes(
            &self.inner.database,
            &self.inner.app,
            &self.inner.lan.trusted_member_states(),
            &self.inner.lan.trusted_member_types(),
            &self.inner.lan.peer_endpoints(),
        )?;
        Ok(devices)
    }

    pub(super) fn current_language(&self) -> String {
        self.inner
            .database
            .load_settings()
            .ok()
            .flatten()
            .map(|settings| settings.language)
            .unwrap_or_else(i18n::default_language_code)
    }

    pub(super) fn user_text(&self, key: TextKey) -> String {
        let language = self.current_language();
        i18n::text(&language, key).to_string()
    }

    pub(super) fn user_message(&self, key: TextKey, args: &[(&str, String)]) -> String {
        let language = self.current_language();
        i18n::message(&language, key, args)
    }
}
