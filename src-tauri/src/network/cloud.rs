use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use reqwest::StatusCode;
use serde::Deserialize;
use tauri::{AppHandle, Emitter};
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
    time::{interval, sleep, timeout, MissedTickBehavior},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{handshake::client::Response, Message},
    MaybeTlsStream, WebSocketStream,
};
use tracing::{debug, error, info, warn};
use url::Url;
use uuid::Uuid;

use crate::{
    api::{DeviceListResponse, DEVICES_PATH},
    auth,
    error::{AppError, AppResult},
    i18n::{self, TextKey},
    models::{AppSettings, CloudStatus, DeviceIdentity, DeviceInfo, SessionRecord},
    network::http::HttpClient,
    protocol::{
        check_business_protocol_version, BusinessEnvelope, CloudClientEnvelope,
        CloudServerEnvelope, DeviceOnlinePayload, BUSINESS_PROTOCOL_VERSION,
    },
    runtime_events::RuntimeEvent,
    shell,
    store::db::Database,
    sync::MutexExt,
};

const WS_TICKET_PATH: &str = "/api/v1/ws/ticket";
const WS_CONNECT_PATH: &str = "/ws/v1";
const WS_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);

pub const AUTH_INVALIDATED_EVENT: &str = "auth-invalidated";
pub const CLOUD_STATUS_EVENT: &str = "cloud-status";
pub const DEVICES_UPDATED_EVENT: &str = "devices-updated";

#[derive(Clone)]
pub struct CloudConnectionManager {
    app: AppHandle,
    database: Database,
    http: HttpClient,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    inner: Arc<Mutex<ManagerState>>,
}

struct ManagerState {
    generation: u64,
    cancel: Option<watch::Sender<bool>>,
    command_tx: Option<mpsc::UnboundedSender<CloudCommand>>,
    status: CloudStatus,
    business_versions: HashMap<String, String>,
}

#[derive(Clone)]
struct ConnectionContext {
    settings: AppSettings,
    session: SessionRecord,
    device: DeviceIdentity,
}

enum ContextLoad {
    Ready(Box<ConnectionContext>),
    NoSession,
    Invalidated(String),
    Retryable(String),
}

enum ConnectionExit {
    Cancelled,
    Disconnected {
        connected_for: Duration,
        reason: Option<String>,
    },
}

enum CloudCommand {
    Relay {
        to: String,
        correlation_id: Option<String>,
        message: BusinessEnvelope,
    },
    Broadcast {
        correlation_id: Option<String>,
        message: BusinessEnvelope,
    },
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TicketRequest<'a> {
    device_id: &'a str,
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegisterRequest<'a> {
    device_id: &'a str,
    name: &'a str,
    #[serde(rename = "type")]
    device_type: &'a str,
    public_key: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceRegisterResponse {
    device_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TicketResponse {
    ticket: String,
}

impl CloudConnectionManager {
    pub fn new(
        app: AppHandle,
        database: Database,
        http: HttpClient,
        event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    ) -> Self {
        Self {
            app,
            database,
            http,
            event_tx,
            inner: Arc::new(Mutex::new(ManagerState {
                generation: 0,
                cancel: None,
                command_tx: None,
                status: CloudStatus::disconnected(),
                business_versions: HashMap::new(),
            })),
        }
    }

    pub fn snapshot(&self) -> CloudStatus {
        self.inner.lock_unpoisoned().status.clone()
    }

    pub fn is_connected(&self) -> bool {
        self.snapshot().connected
    }

    pub fn ensure_business_compatible(&self, device_id: &str) -> AppResult<()> {
        let version = {
            self.inner
                .lock_unpoisoned()
                .business_versions
                .get(device_id)
                .cloned()
        };
        let Some(version) = version else {
            return Ok(());
        };
        let compatibility = check_business_protocol_version(&version);
        if compatibility.compatible {
            Ok(())
        } else {
            Err(AppError::message(
                compatibility
                    .message
                    .or(compatibility.reason)
                    .unwrap_or_else(|| "business protocol version incompatible".to_string()),
            ))
        }
    }

    pub fn ensure_known_business_versions_compatible(&self) -> AppResult<()> {
        let versions = self
            .inner
            .lock_unpoisoned()
            .business_versions
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for version in versions {
            let compatibility = check_business_protocol_version(&version);
            if !compatibility.compatible {
                return Err(AppError::message(
                    compatibility
                        .message
                        .or(compatibility.reason)
                        .unwrap_or_else(|| "business protocol version incompatible".to_string()),
                ));
            }
        }
        Ok(())
    }

    pub fn start(&self) {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let generation = {
            let mut inner = self.inner.lock_unpoisoned();
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.cancel = Some(cancel_tx);
            inner.command_tx = None;
            inner.business_versions.clear();
            inner.status = CloudStatus::connecting();
            inner.generation
        };

        self.emit_status(CloudStatus::connecting());
        info!(generation = generation, "cloud connection starting");

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager.run(generation, cancel_rx).await;
        });
    }

    pub fn restart(&self) {
        self.start();
    }

    pub fn stop(&self) {
        {
            let mut inner = self.inner.lock_unpoisoned();
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.command_tx = None;
            inner.business_versions.clear();
            inner.status = CloudStatus::disconnected();
        }

        self.emit_status(CloudStatus::disconnected());
        info!("cloud connection stopped");
        let _ = self.event_tx.send(RuntimeEvent::CloudUnavailable);
        let _ = self.event_tx.send(RuntimeEvent::CloudDisconnected(Some(
            "cloud connection stopped".to_string(),
        )));
    }

    pub fn stop_quiet(&self) {
        {
            let mut inner = self.inner.lock_unpoisoned();
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.command_tx = None;
            inner.business_versions.clear();
            inner.status = CloudStatus::disconnected();
        }

        self.emit_status(CloudStatus::disconnected());
        info!("cloud connection stopped");
        let _ = self.event_tx.send(RuntimeEvent::CloudUnavailable);
    }

    pub fn send_relay(
        &self,
        to: &str,
        message: BusinessEnvelope,
        correlation_id: Option<String>,
    ) -> AppResult<()> {
        debug!(to, message_type = %message.message_type, "queueing cloud relay");
        self.send_command(CloudCommand::Relay {
            to: to.to_string(),
            correlation_id,
            message,
        })
    }

    pub fn send_broadcast(
        &self,
        message: BusinessEnvelope,
        correlation_id: Option<String>,
    ) -> AppResult<()> {
        debug!(message_type = %message.message_type, "queueing cloud broadcast");
        self.send_command(CloudCommand::Broadcast {
            correlation_id,
            message,
        })
    }

    async fn run(&self, generation: u64, mut cancel_rx: watch::Receiver<bool>) {
        let mut attempt = 0_u32;

        loop {
            if is_cancelled(&cancel_rx) {
                return;
            }

            let context = match self.load_context().await {
                ContextLoad::Ready(context) => *context,
                ContextLoad::NoSession => {
                    debug!("cloud connection skipped because no session exists");
                    let _ = self.update_status_if_current(generation, CloudStatus::disconnected());
                    self.clear_command_sender(generation);
                    let _ = self.event_tx.send(RuntimeEvent::CloudUnavailable);
                    return;
                }
                ContextLoad::Invalidated(message) => {
                    warn!(%message, "cloud auth invalidated while loading context");
                    self.invalidate_auth(message, generation);
                    return;
                }
                ContextLoad::Retryable(message) => {
                    warn!(attempt = attempt + 1, %message, "cloud context load failed");
                    attempt += 1;
                    let status = CloudStatus::reconnecting(attempt, Some(message.clone()));
                    if self.update_status_if_current(generation, status.clone()) {
                        self.emit_status(status);
                    }
                    let _ = self.event_tx.send(RuntimeEvent::Log {
                        level: "warn".to_string(),
                        source: "cloud".to_string(),
                        message,
                    });
                    let _ = self.event_tx.send(RuntimeEvent::CloudUnavailable);
                    if wait_or_cancel(backoff_delay(attempt), &mut cancel_rx).await {
                        return;
                    }
                    continue;
                }
            };

            let phase = if attempt == 0 {
                CloudStatus::connecting()
            } else {
                CloudStatus::reconnecting(attempt, None)
            };
            if self.update_status_if_current(generation, phase.clone()) {
                self.emit_status(phase);
            }
            info!(attempt = attempt, "cloud connect attempt starting");

            match self.connect_once(generation, context, &mut cancel_rx).await {
                Ok(ConnectionExit::Cancelled) => {
                    debug!("cloud connection cancelled");
                    return;
                }
                Ok(ConnectionExit::Disconnected {
                    connected_for,
                    reason,
                }) => {
                    warn!(
                        connected_for_ms = connected_for.as_millis() as u64,
                        reason = reason.as_deref().unwrap_or("unknown"),
                        "cloud connection disconnected"
                    );
                    self.clear_command_sender(generation);
                    attempt = if connected_for.as_secs() >= 60 {
                        1
                    } else {
                        attempt.saturating_add(1).max(1)
                    };
                    let status = CloudStatus::reconnecting(attempt, reason.clone());
                    if self.update_status_if_current(generation, status.clone()) {
                        self.emit_status(status);
                    }
                    let _ = self.event_tx.send(RuntimeEvent::CloudDisconnected(reason));
                    let _ = self.event_tx.send(RuntimeEvent::CloudUnavailable);
                    if wait_or_cancel(backoff_delay(attempt), &mut cancel_rx).await {
                        return;
                    }
                }
                Err(ConnectionFailure::Invalidated(message)) => {
                    warn!(%message, "cloud auth invalidated");
                    self.clear_command_sender(generation);
                    self.invalidate_auth(message, generation);
                    return;
                }
                Err(ConnectionFailure::Retryable(message)) => {
                    warn!(%message, "cloud connect attempt failed");
                    self.clear_command_sender(generation);
                    attempt += 1;
                    let status = CloudStatus::reconnecting(attempt, Some(message.clone()));
                    if self.update_status_if_current(generation, status.clone()) {
                        self.emit_status(status);
                    }
                    let _ = self
                        .event_tx
                        .send(RuntimeEvent::CloudDisconnected(Some(message)));
                    let _ = self.event_tx.send(RuntimeEvent::CloudUnavailable);
                    if wait_or_cancel(backoff_delay(attempt), &mut cancel_rx).await {
                        return;
                    }
                }
            }
        }
    }

    async fn load_context(&self) -> ContextLoad {
        let settings = match self.database.load_settings() {
            Ok(Some(settings)) => settings,
            Ok(None) => {
                return ContextLoad::Retryable("local settings are not initialized".to_string())
            }
            Err(error) => return ContextLoad::Retryable(error.to_string()),
        };

        let session = match self.database.load_session() {
            Ok(Some(session)) => session,
            Ok(None) => return ContextLoad::NoSession,
            Err(error) => return ContextLoad::Retryable(error.to_string()),
        };

        let session =
            match auth::refresh_session_if_needed(&self.database, &self.http, &settings, session)
                .await
            {
                Ok(session) => session,
                Err(error) if is_auth_error(&error) => {
                    return ContextLoad::Invalidated(error.to_string());
                }
                Err(error) => return ContextLoad::Retryable(error.to_string()),
            };

        let mut device = match self.database.load_device_identity() {
            Ok(Some(device)) => device,
            Ok(None) => {
                return ContextLoad::Retryable("current device is not registered".to_string())
            }
            Err(error) => return ContextLoad::Retryable(error.to_string()),
        };

        match self
            .sync_current_device_identity(&settings, &session, &device)
            .await
        {
            Ok(updated) => device = updated,
            Err(error) if is_auth_error(&error) => {
                return ContextLoad::Invalidated(error.to_string());
            }
            Err(error) => return ContextLoad::Retryable(error.to_string()),
        }

        ContextLoad::Ready(Box::new(ConnectionContext {
            settings,
            session,
            device,
        }))
    }

    async fn sync_current_device_identity(
        &self,
        settings: &AppSettings,
        session: &SessionRecord,
        identity: &DeviceIdentity,
    ) -> AppResult<DeviceIdentity> {
        let request = DeviceRegisterRequest {
            device_id: &identity.device_id,
            name: &identity.name,
            device_type: &identity.device_type,
            public_key: &identity.public_key,
        };
        let response: DeviceRegisterResponse = self
            .http
            .post(
                &settings.server_url,
                DEVICES_PATH,
                &request,
                Some(&session.access_token),
            )
            .await?;

        let mut updated = identity.clone();
        updated.user_id = Some(session.user_id.clone());
        updated.device_id = response.device_id;
        updated.cloud_key_sync_pending = false;
        self.database.save_device_identity(&updated)?;
        Ok(updated)
    }

    async fn connect_once(
        &self,
        generation: u64,
        context: ConnectionContext,
        cancel_rx: &mut watch::Receiver<bool>,
    ) -> Result<ConnectionExit, ConnectionFailure> {
        let request = TicketRequest {
            device_id: &context.device.device_id,
        };

        debug!(device_id = %context.device.device_id, "requesting cloud websocket ticket");
        let ticket: TicketResponse = self
            .http
            .post(
                &context.settings.server_url,
                WS_TICKET_PATH,
                &request,
                Some(&context.session.access_token),
            )
            .await
            .map_err(|error| classify_connect_error(error.to_string()))?;

        let ws_url = build_ws_url(
            &context.settings.server_url,
            &ticket.ticket,
            BUSINESS_PROTOCOL_VERSION,
        )
        .map_err(|error| ConnectionFailure::Retryable(error.to_string()))?;
        debug!(url = %ws_url, "connecting cloud websocket");
        let (stream, _) = open_websocket(ws_url.as_str(), WS_CONNECT_TIMEOUT)
            .await
            .map_err(ConnectionFailure::Retryable)?;
        let connected_at = Instant::now();

        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        self.install_command_sender(generation, command_tx);

        let connected = CloudStatus::connected();
        if self.update_status_if_current(generation, connected.clone()) {
            self.emit_status(connected);
        }
        info!("cloud websocket connected");

        let _ = self.sync_pending_device_key(&context).await;
        let _ = self.sync_devices_from_server(&context).await;
        let _ = self.event_tx.send(RuntimeEvent::CloudConnected);

        let (mut writer, mut reader) = stream.split();
        let mut ping_interval = interval(Duration::from_secs(30));
        ping_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && is_cancelled(cancel_rx) {
                        let _ = writer.send(Message::Close(None)).await;
                    }
                    return Ok(ConnectionExit::Cancelled);
                }
                _ = ping_interval.tick() => {
                    let ping = CloudClientEnvelope {
                        id: Uuid::new_v4().to_string(),
                        message_type: "ping".to_string(),
                        to: None,
                        correlation_id: None,
                        payload: None,
                    };
                    if write_client_message(&mut writer, ping).await.is_err() {
                        warn!("cloud ping failed");
                        return Ok(ConnectionExit::Disconnected {
                            connected_for: connected_at.elapsed(),
                            reason: Some("cloud connection disconnected".to_string()),
                        });
                    }
                }
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        continue;
                    };

                    let outbound = match command {
                        CloudCommand::Relay { to, correlation_id, message } => {
                            debug!(to, message_type = %message.message_type, "sending cloud relay");
                            CloudClientEnvelope {
                                id: Uuid::new_v4().to_string(),
                                message_type: "relay".to_string(),
                                to: Some(to),
                                correlation_id,
                                payload: Some(serde_json::to_value(message).map_err(|error| ConnectionFailure::Retryable(error.to_string()))?),
                            }
                        }
                        CloudCommand::Broadcast { correlation_id, message } => {
                            debug!(message_type = %message.message_type, "sending cloud broadcast");
                            CloudClientEnvelope {
                                id: Uuid::new_v4().to_string(),
                                message_type: "broadcast".to_string(),
                                to: None,
                                correlation_id,
                                payload: Some(serde_json::to_value(message).map_err(|error| ConnectionFailure::Retryable(error.to_string()))?),
                            }
                        }
                    };

                    if write_client_message(&mut writer, outbound).await.is_err() {
                        warn!("cloud message send failed");
                        return Ok(ConnectionExit::Disconnected {
                            connected_for: connected_at.elapsed(),
                            reason: Some("cloud send failed".to_string()),
                        });
                    }
                }
                message = reader.next() => {
                    match message {
                        Some(Ok(Message::Text(text))) => {
                            debug!(bytes = text.len(), "received cloud text frame");
                            self.handle_server_message(text.as_str()).await;
                        }
                        Some(Ok(Message::Close(_))) => {
                            info!("cloud websocket closed by server");
                            return Ok(ConnectionExit::Disconnected {
                                connected_for: connected_at.elapsed(),
                                reason: Some("server closed the connection".to_string()),
                            });
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            if writer.send(Message::Pong(payload)).await.is_err() {
                                return Ok(ConnectionExit::Disconnected {
                                    connected_for: connected_at.elapsed(),
                                    reason: Some("cloud connection disconnected".to_string()),
                                });
                            }
                        }
                        Some(Ok(_)) => {}
                        Some(Err(error)) => {
                            warn!(%error, "cloud websocket read failed");
                            return Ok(ConnectionExit::Disconnected {
                                connected_for: connected_at.elapsed(),
                                reason: Some(error.to_string()),
                            });
                        }
                        None => {
                            return Ok(ConnectionExit::Disconnected {
                                connected_for: connected_at.elapsed(),
                                reason: Some("cloud connection ended".to_string()),
                            });
                        }
                    }
                }
            }
        }
    }

    async fn handle_server_message(&self, raw: &str) {
        let Ok(message) = serde_json::from_str::<CloudServerEnvelope>(raw) else {
            warn!("received invalid cloud message");
            return;
        };

        match message.message_type.as_str() {
            "device.online" => {
                debug!(
                    from = message.from.as_deref().unwrap_or("unknown"),
                    "cloud device online"
                );
                let payload = message
                    .payload
                    .and_then(|value| serde_json::from_value::<DeviceOnlinePayload>(value).ok());
                if let Some(device_id) = message.from {
                    if let Some(payload) = payload.as_ref() {
                        self.remember_business_version(&device_id, &payload.business_version);
                    }
                    let _ = self.event_tx.send(RuntimeEvent::DevicePresence {
                        device_id,
                        online: true,
                        payload,
                    });
                }
            }
            "device.offline" => {
                debug!(
                    from = message.from.as_deref().unwrap_or("unknown"),
                    "cloud device offline"
                );
                if let Some(device_id) = message.from {
                    self.forget_business_version(&device_id);
                    let _ = self.event_tx.send(RuntimeEvent::DevicePresence {
                        device_id,
                        online: false,
                        payload: None,
                    });
                }
            }
            "relay" | "broadcast" => {
                debug!(
                    from = message.from.as_deref().unwrap_or("unknown"),
                    message_type = %message.message_type,
                    "cloud business message received"
                );
                let Some(from) = message.from else {
                    return;
                };
                let Some(payload) = message.payload else {
                    return;
                };
                let Ok(business) = serde_json::from_value::<BusinessEnvelope>(payload) else {
                    return;
                };
                let _ = self.event_tx.send(RuntimeEvent::CloudRelay {
                    from,
                    envelope_id: message.id,
                    message: business,
                });
            }
            _ => {
                debug!(message_type = %message.message_type, "ignored cloud message");
            }
        }
    }

    async fn sync_devices_from_server(&self, context: &ConnectionContext) -> AppResult<()> {
        let response: DeviceListResponse = self
            .http
            .get(
                &context.settings.server_url,
                DEVICES_PATH,
                Some(&context.session.access_token),
            )
            .await?;

        let _ = self
            .event_tx
            .send(RuntimeEvent::DevicesSnapshot(response.into_devices()));
        debug!("synced devices from cloud server");
        Ok(())
    }

    async fn sync_pending_device_key(&self, context: &ConnectionContext) -> AppResult<()> {
        let Some(identity) = self.database.load_device_identity()? else {
            return Ok(());
        };
        if !identity.cloud_key_sync_pending
            || identity.user_id.as_deref() != Some(context.session.user_id.as_str())
        {
            return Ok(());
        }

        let path = format!("{DEVICES_PATH}/{}/key", identity.device_id);
        let request = serde_json::json!({ "publicKey": identity.public_key });
        self.http
            .put_empty(
                &context.settings.server_url,
                &path,
                &request,
                Some(&context.session.access_token),
            )
            .await?;

        if let Some(mut latest) = self.database.load_device_identity()? {
            if latest.device_id == identity.device_id && latest.public_key == identity.public_key {
                latest.cloud_key_sync_pending = false;
                self.database.save_device_identity(&latest)?;
            }
        }

        debug!("synced pending device key to cloud server");
        Ok(())
    }

    fn send_command(&self, command: CloudCommand) -> AppResult<()> {
        let sender = self
            .inner
            .lock_unpoisoned()
            .command_tx
            .clone()
            .ok_or_else(|| AppError::message(self.user_text(TextKey::CloudNotConnected)))?;

        sender
            .send(command)
            .map_err(|_| AppError::message(self.user_text(TextKey::CloudUnavailable)))
    }

    fn install_command_sender(&self, generation: u64, sender: mpsc::UnboundedSender<CloudCommand>) {
        let mut inner = self.inner.lock_unpoisoned();
        if inner.generation == generation {
            inner.command_tx = Some(sender);
        }
    }

    fn clear_command_sender(&self, generation: u64) {
        let mut inner = self.inner.lock_unpoisoned();
        if inner.generation == generation {
            inner.command_tx = None;
            inner.business_versions.clear();
        }
    }

    fn remember_business_version(&self, device_id: &str, business_version: &str) {
        self.inner
            .lock_unpoisoned()
            .business_versions
            .insert(device_id.to_string(), business_version.to_string());
    }

    fn forget_business_version(&self, device_id: &str) {
        self.inner
            .lock_unpoisoned()
            .business_versions
            .remove(device_id);
    }

    fn invalidate_auth(&self, message: String, generation: u64) {
        if !self.update_status_if_current(generation, CloudStatus::disconnected()) {
            return;
        }

        if let Err(error) = self.database.clear_session() {
            error!(%error, "failed to clear session during auth invalidation");
        }
        if let Err(error) = self.database.clear_cached_devices() {
            error!(%error, "failed to clear device cache during auth invalidation");
        }
        if let Err(error) = self.database.clear_cloud_trust() {
            error!(%error, "failed to clear cloud trust during auth invalidation");
        }
        warn!(%message, "invalidating cloud auth");
        self.emit_devices(Vec::new());
        self.emit_status(CloudStatus::disconnected());
        let _ = self
            .event_tx
            .send(RuntimeEvent::AuthInvalidated(message.clone()));
        let _ = self.app.emit(AUTH_INVALIDATED_EVENT, message);
    }

    fn update_status_if_current(&self, generation: u64, status: CloudStatus) -> bool {
        let mut inner = self.inner.lock_unpoisoned();
        if inner.generation != generation {
            return false;
        }

        inner.status = status;
        true
    }

    fn emit_status(&self, status: CloudStatus) {
        let _ = self.app.emit(CLOUD_STATUS_EVENT, status);
        let _ = shell::refresh_tray(&self.app);
    }

    fn emit_devices(&self, devices: Vec<DeviceInfo>) {
        let _ = self.app.emit(DEVICES_UPDATED_EVENT, devices);
        let _ = shell::refresh_tray(&self.app);
    }
}

async fn open_websocket(
    url: &str,
    connect_timeout: Duration,
) -> Result<(WebSocketStream<MaybeTlsStream<TcpStream>>, Response), String> {
    timeout(connect_timeout, connect_async(url))
        .await
        .map_err(|_| {
            format!(
                "cloud websocket connect timed out after {} seconds",
                connect_timeout.as_secs()
            )
        })?
        .map_err(|error| error.to_string())
}

impl CloudConnectionManager {
    fn user_text(&self, key: TextKey) -> String {
        let language = self
            .database
            .load_settings()
            .ok()
            .flatten()
            .map(|settings| settings.language)
            .unwrap_or_else(i18n::default_language_code);
        i18n::text(&language, key).to_string()
    }
}

enum ConnectionFailure {
    Invalidated(String),
    Retryable(String),
}

fn classify_connect_error(message: String) -> ConnectionFailure {
    if message.to_ascii_lowercase().contains("unauthorized") {
        ConnectionFailure::Invalidated(message)
    } else {
        ConnectionFailure::Retryable(message)
    }
}

fn is_auth_error(error: &AppError) -> bool {
    match error {
        AppError::Network(network) => network.status() == Some(StatusCode::UNAUTHORIZED),
        AppError::Protocol { code, .. } => AppError::is_auth_protocol_code(*code),
        AppError::Message(message) => {
            message.eq_ignore_ascii_case("unauthorized")
                || message.eq_ignore_ascii_case("invalid refresh token")
                || message.eq_ignore_ascii_case("token revoked")
        }
        _ => false,
    }
}

fn build_ws_url(
    base_url: &str,
    ticket: &str,
    business_version: &str,
) -> Result<Url, url::ParseError> {
    let mut url = Url::parse(base_url)?;

    match url.scheme() {
        "https" => {
            let _ = url.set_scheme("wss");
        }
        _ => {
            let _ = url.set_scheme("ws");
        }
    }

    url.set_path(WS_CONNECT_PATH);
    url.query_pairs_mut()
        .clear()
        .append_pair("ticket", ticket)
        .append_pair("businessVersion", business_version);
    Ok(url)
}

fn backoff_delay(attempt: u32) -> Duration {
    let exponent = attempt.saturating_sub(1).min(16);
    let seconds = 2_u64.pow(exponent).min(30);
    Duration::from_secs(seconds)
}

fn is_cancelled(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
}

async fn wait_or_cancel(duration: Duration, cancel_rx: &mut watch::Receiver<bool>) -> bool {
    tokio::select! {
        _ = sleep(duration) => false,
        changed = cancel_rx.changed() => changed.is_ok() && *cancel_rx.borrow(),
    }
}

async fn write_client_message<S>(
    writer: &mut S,
    message: CloudClientEnvelope,
) -> Result<(), tokio_tungstenite::tungstenite::Error>
where
    S: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let payload = serde_json::to_string(&message)
        .map_err(|error| tokio_tungstenite::tungstenite::Error::Io(std::io::Error::other(error)))?;
    writer.send(Message::Text(payload.into())).await
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures_util::SinkExt;
    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        accept_hdr_async,
        tungstenite::{
            handshake::server::{ErrorResponse, Request, Response},
            http::StatusCode,
            Message,
        },
    };

    use super::{backoff_delay, build_ws_url, open_websocket, WS_CONNECT_PATH};

    #[test]
    fn builds_websocket_url_from_http_base_url() {
        let url = build_ws_url(
            "http://example.com/base?ignored=true",
            "ticket value",
            "business.v1",
        )
        .expect("url");

        assert_eq!(url.scheme(), "ws");
        assert_eq!(url.host_str(), Some("example.com"));
        assert_eq!(url.path(), WS_CONNECT_PATH);
        assert_eq!(
            url.query(),
            Some("ticket=ticket+value&businessVersion=business.v1")
        );
    }

    #[test]
    fn builds_secure_websocket_url_from_https_base_url() {
        let url = build_ws_url("https://example.com", "ticket", "business.v1").expect("url");

        assert_eq!(url.scheme(), "wss");
        assert_eq!(url.path(), WS_CONNECT_PATH);
    }

    #[test]
    fn caps_cloud_reconnect_backoff_at_thirty_seconds() {
        assert_eq!(backoff_delay(1), Duration::from_secs(1));
        assert_eq!(backoff_delay(2), Duration::from_secs(2));
        assert_eq!(backoff_delay(6), Duration::from_secs(30));
        assert_eq!(backoff_delay(99), Duration::from_secs(30));
    }

    #[tokio::test]
    async fn websocket_handshake_sends_ticket_and_business_version() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("mock server address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept client");
            let mut websocket =
                accept_hdr_async(stream, |request: &Request, response: Response| {
                    assert_eq!(request.uri().path(), WS_CONNECT_PATH);
                    assert_eq!(
                        request.uri().query(),
                        Some("ticket=test-ticket&businessVersion=business.v1"),
                    );
                    Ok(response)
                })
                .await
                .expect("complete websocket handshake");
            websocket
                .send(Message::Close(None))
                .await
                .expect("close websocket");
        });

        let url = format!(
            "ws://{address}{WS_CONNECT_PATH}?ticket=test-ticket&businessVersion=business.v1"
        );
        let (_websocket, response) = open_websocket(&url, Duration::from_secs(2))
            .await
            .expect("client handshake");

        assert_eq!(response.status(), StatusCode::SWITCHING_PROTOCOLS);
        server.await.expect("mock server task");
    }

    #[tokio::test]
    async fn websocket_handshake_surfaces_server_rejection() {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock server");
        let address = listener.local_addr().expect("mock server address");
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.expect("accept client");
            let result = accept_hdr_async(stream, |_request: &Request, _response: Response| {
                let mut rejection = ErrorResponse::new(Some("invalid ticket".to_string()));
                *rejection.status_mut() = StatusCode::UNAUTHORIZED;
                Err(rejection)
            })
            .await;
            assert!(result.is_err());
        });

        let url = format!("ws://{address}{WS_CONNECT_PATH}?ticket=invalid");
        let error = open_websocket(&url, Duration::from_secs(2))
            .await
            .expect_err("server must reject handshake");

        assert!(error.contains("401"));
        server.await.expect("mock server task");
    }
}
