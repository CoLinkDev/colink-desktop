use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tauri::{AppHandle, Emitter};
use tokio::{
    sync::{mpsc, watch},
    time::{interval, sleep, MissedTickBehavior},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, error, info, warn};
use url::Url;
use uuid::Uuid;

use crate::{
    api::{DeviceListResponse, DEVICES_PATH},
    auth,
    error::{AppError, AppResult},
    models::{AppSettings, CloudStatus, DeviceIdentity, DeviceInfo, SessionRecord},
    network::http::HttpClient,
    protocol::{BusinessEnvelope, CloudClientEnvelope, CloudServerEnvelope, DeviceOnlinePayload},
    runtime_events::RuntimeEvent,
    shell,
    store::db::Database,
    sync::MutexExt,
};

const WS_TICKET_PATH: &str = "/api/v1/ws/ticket";
const WS_CONNECT_PATH: &str = "/ws/v1";

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
        message: BusinessEnvelope,
    },
}

#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct TicketRequest<'a> {
    device_id: &'a str,
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
            })),
        }
    }

    pub fn snapshot(&self) -> CloudStatus {
        self.inner.lock_unpoisoned().status.clone()
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
            inner.status = CloudStatus::disconnected();
        }

        self.emit_status(CloudStatus::disconnected());
        info!("cloud connection stopped");
        let _ = self.event_tx.send(RuntimeEvent::CloudDisconnected(Some(
            "云端连接已停止".to_string(),
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
            inner.status = CloudStatus::disconnected();
        }

        self.emit_status(CloudStatus::disconnected());
        info!("cloud connection stopped");
    }

    pub fn send_relay(&self, to: &str, message: BusinessEnvelope) -> AppResult<()> {
        debug!(to, message_type = %message.message_type, "queueing cloud relay");
        self.send_command(CloudCommand::Relay {
            to: to.to_string(),
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
                    let _ = self.update_status_if_current(generation, status.clone());
                    self.emit_status(status);
                    let _ = self.event_tx.send(RuntimeEvent::Log {
                        level: "warn".to_string(),
                        source: "cloud".to_string(),
                        message,
                    });
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
            let _ = self.update_status_if_current(generation, phase.clone());
            self.emit_status(phase);
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
                    let _ = self.update_status_if_current(generation, status.clone());
                    self.emit_status(status);
                    let _ = self.event_tx.send(RuntimeEvent::CloudDisconnected(reason));
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
                    let _ = self.update_status_if_current(generation, status.clone());
                    self.emit_status(status);
                    let _ = self
                        .event_tx
                        .send(RuntimeEvent::CloudDisconnected(Some(message)));
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
            Ok(None) => return ContextLoad::Retryable("本地设置未初始化".to_string()),
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
                Err(error) => return ContextLoad::Invalidated(error.to_string()),
            };

        let device = match self.database.load_device_identity() {
            Ok(Some(device)) => device,
            Ok(None) => return ContextLoad::Retryable("当前设备尚未注册".to_string()),
            Err(error) => return ContextLoad::Retryable(error.to_string()),
        };

        if device.user_id.as_deref() != Some(session.user_id.as_str()) {
            return ContextLoad::Retryable("当前设备和账户状态不一致".to_string());
        }

        ContextLoad::Ready(Box::new(ConnectionContext {
            settings,
            session,
            device,
        }))
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

        let ws_url = build_ws_url(&context.settings.server_url, &ticket.ticket)
            .map_err(|error| ConnectionFailure::Retryable(error.to_string()))?;
        debug!(url = %ws_url, "connecting cloud websocket");
        let (stream, _) = connect_async(ws_url.as_str())
            .await
            .map_err(|error| ConnectionFailure::Retryable(error.to_string()))?;
        let connected_at = Instant::now();

        let (command_tx, mut command_rx) = mpsc::unbounded_channel();
        self.install_command_sender(generation, command_tx);

        let connected = CloudStatus::connected();
        let _ = self.update_status_if_current(generation, connected.clone());
        self.emit_status(connected);
        info!("cloud websocket connected");

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
                        payload: None,
                    };
                    if write_client_message(&mut writer, ping).await.is_err() {
                        warn!("cloud ping failed");
                        return Ok(ConnectionExit::Disconnected {
                            connected_for: connected_at.elapsed(),
                            reason: Some("云端连接已断开".to_string()),
                        });
                    }
                }
                command = command_rx.recv() => {
                    let Some(command) = command else {
                        continue;
                    };

                    let outbound = match command {
                        CloudCommand::Relay { to, message } => {
                            debug!(to, message_type = %message.message_type, "sending cloud relay");
                            CloudClientEnvelope {
                                id: Uuid::new_v4().to_string(),
                                message_type: "relay".to_string(),
                                to: Some(to),
                                payload: Some(serde_json::to_value(message).map_err(|error| ConnectionFailure::Retryable(error.to_string()))?),
                            }
                        }
                    };

                    if write_client_message(&mut writer, outbound).await.is_err() {
                        warn!("cloud message send failed");
                        return Ok(ConnectionExit::Disconnected {
                            connected_for: connected_at.elapsed(),
                            reason: Some("云端发送失败".to_string()),
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
                                reason: Some("服务端关闭了连接".to_string()),
                            });
                        }
                        Some(Ok(Message::Ping(payload))) => {
                            if writer.send(Message::Pong(payload)).await.is_err() {
                                return Ok(ConnectionExit::Disconnected {
                                    connected_for: connected_at.elapsed(),
                                    reason: Some("云端连接已断开".to_string()),
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
                                reason: Some("云端连接已结束".to_string()),
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
                    let _ = self.event_tx.send(RuntimeEvent::DevicePresence {
                        device_id,
                        online: false,
                        payload: None,
                    });
                }
            }
            "relay" => {
                debug!(
                    from = message.from.as_deref().unwrap_or("unknown"),
                    "cloud relay received"
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
            .send(RuntimeEvent::DevicesSnapshot(response.devices));
        debug!("synced devices from cloud server");
        Ok(())
    }

    fn send_command(&self, command: CloudCommand) -> AppResult<()> {
        let sender = self
            .inner
            .lock_unpoisoned()
            .command_tx
            .clone()
            .ok_or_else(|| AppError::message("云端连接尚未建立"))?;

        sender
            .send(command)
            .map_err(|_| AppError::message("云端连接不可用"))
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
        }
    }

    fn invalidate_auth(&self, message: String, generation: u64) {
        if let Err(error) = self.database.clear_session() {
            error!(%error, "failed to clear session during auth invalidation");
        }
        if let Err(error) = self.database.clear_cached_devices() {
            error!(%error, "failed to clear device cache during auth invalidation");
        }
        warn!(%message, "invalidating cloud auth");
        let _ = self.update_status_if_current(generation, CloudStatus::disconnected());
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

fn build_ws_url(base_url: &str, ticket: &str) -> Result<Url, url::ParseError> {
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
    url.set_query(Some(&format!("ticket={ticket}")));
    Ok(url)
}

fn backoff_delay(attempt: u32) -> Duration {
    match attempt {
        0 | 1 => Duration::from_secs(1),
        2 => Duration::from_secs(2),
        3 => Duration::from_secs(4),
        4 => Duration::from_secs(8),
        _ => Duration::from_secs(30),
    }
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
