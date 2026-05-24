use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter};
use tokio::{
    sync::watch,
    time::{interval, sleep, MissedTickBehavior},
};
use tokio_tungstenite::{connect_async, tungstenite::Message};
use url::Url;
use uuid::Uuid;

use crate::{
    error::AppResult,
    models::{unix_now, AppSettings, CloudStatus, DeviceIdentity, DeviceInfo, SessionRecord},
    network::http::HttpClient,
    store::db::Database,
};

const AUTH_REFRESH_PATH: &str = "/api/v1/auth/refresh";
const DEVICES_PATH: &str = "/api/v1/devices";
const WS_TICKET_PATH: &str = "/api/v1/ws/ticket";
const WS_CONNECT_PATH: &str = "/ws/v1";
const ACCESS_TOKEN_TTL_SECONDS: i64 = 15 * 60;

pub const AUTH_INVALIDATED_EVENT: &str = "auth-invalidated";
pub const CLOUD_STATUS_EVENT: &str = "cloud-status";
pub const DEVICES_UPDATED_EVENT: &str = "devices-updated";

#[derive(Clone)]
pub struct CloudConnectionManager {
    app: AppHandle,
    database: Database,
    http: HttpClient,
    inner: Arc<Mutex<ManagerState>>,
}

struct ManagerState {
    generation: u64,
    cancel: Option<watch::Sender<bool>>,
    status: CloudStatus,
}

#[derive(Clone)]
struct ConnectionContext {
    settings: AppSettings,
    session: SessionRecord,
    device: DeviceIdentity,
}

enum ContextLoad {
    Ready(ConnectionContext),
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RefreshResponse {
    token: String,
    refresh_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct RefreshRequest<'a> {
    refresh_token: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TicketRequest<'a> {
    device_id: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TicketResponse {
    ticket: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceListResponse {
    devices: Vec<DeviceInfo>,
}

#[derive(Debug, Serialize)]
struct ClientWsMessage {
    id: String,
    #[serde(rename = "type")]
    message_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ServerWsMessage {
    #[serde(rename = "type")]
    message_type: String,
    from: Option<String>,
    payload: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeviceOnlinePayload {
    name: String,
    #[serde(rename = "type")]
    device_type: String,
}

impl CloudConnectionManager {
    pub fn new(app: AppHandle, database: Database, http: HttpClient) -> Self {
        Self {
            app,
            database,
            http,
            inner: Arc::new(Mutex::new(ManagerState {
                generation: 0,
                cancel: None,
                status: CloudStatus::disconnected(),
            })),
        }
    }

    pub fn snapshot(&self) -> CloudStatus {
        self.inner
            .lock()
            .expect("cloud manager poisoned")
            .status
            .clone()
    }

    pub fn start(&self) {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let generation = {
            let mut inner = self.inner.lock().expect("cloud manager poisoned");
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.cancel = Some(cancel_tx);
            inner.status = CloudStatus::connecting();
            inner.generation
        };

        self.emit_status(CloudStatus::connecting());

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
            let mut inner = self.inner.lock().expect("cloud manager poisoned");
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.status = CloudStatus::disconnected();
        }

        self.emit_status(CloudStatus::disconnected());
    }

    async fn run(&self, generation: u64, mut cancel_rx: watch::Receiver<bool>) {
        let mut attempt = 0_u32;

        loop {
            if is_cancelled(&cancel_rx) {
                return;
            }

            let context = match self.load_context().await {
                ContextLoad::Ready(context) => context,
                ContextLoad::NoSession => {
                    let _ = self.update_status_if_current(generation, CloudStatus::disconnected());
                    return;
                }
                ContextLoad::Invalidated(message) => {
                    self.invalidate_auth(message, generation);
                    return;
                }
                ContextLoad::Retryable(message) => {
                    attempt += 1;
                    let status = CloudStatus::reconnecting(attempt, Some(message));
                    let _ = self.update_status_if_current(generation, status.clone());
                    self.emit_status(status);
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

            match self.connect_once(generation, context, &mut cancel_rx).await {
                Ok(ConnectionExit::Cancelled) => return,
                Ok(ConnectionExit::Disconnected {
                    connected_for,
                    reason,
                }) => {
                    attempt = if connected_for.as_secs() >= 60 {
                        1
                    } else {
                        attempt.saturating_add(1).max(1)
                    };
                    let status = CloudStatus::reconnecting(attempt, reason);
                    let _ = self.update_status_if_current(generation, status.clone());
                    self.emit_status(status);
                    if wait_or_cancel(backoff_delay(attempt), &mut cancel_rx).await {
                        return;
                    }
                }
                Err(ConnectionFailure::Invalidated(message)) => {
                    self.invalidate_auth(message, generation);
                    return;
                }
                Err(ConnectionFailure::Retryable(message)) => {
                    attempt += 1;
                    let status = CloudStatus::reconnecting(attempt, Some(message));
                    let _ = self.update_status_if_current(generation, status.clone());
                    self.emit_status(status);
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

        let session = match self.refresh_session_if_needed(&settings, session).await {
            Ok(session) => session,
            Err(error) => return ContextLoad::Invalidated(error),
        };

        let device = match self.database.load_device_identity() {
            Ok(Some(device)) => device,
            Ok(None) => return ContextLoad::Retryable("当前设备尚未注册".to_string()),
            Err(error) => return ContextLoad::Retryable(error.to_string()),
        };

        if device.user_id != session.user_id {
            return ContextLoad::Retryable("当前设备和账户状态不一致".to_string());
        }

        ContextLoad::Ready(ConnectionContext {
            settings,
            session,
            device,
        })
    }

    async fn refresh_session_if_needed(
        &self,
        settings: &AppSettings,
        session: SessionRecord,
    ) -> Result<SessionRecord, String> {
        if !session.is_expiring_soon() {
            return Ok(session);
        }

        let request = RefreshRequest {
            refresh_token: &session.refresh_token,
        };

        let response: RefreshResponse = self
            .http
            .post(&settings.server_url, AUTH_REFRESH_PATH, &request, None)
            .await
            .map_err(|error| error.to_string())?;

        let refreshed = SessionRecord {
            user_id: session.user_id,
            access_token: response.token,
            refresh_token: response.refresh_token,
            access_token_expires_at: unix_now() + ACCESS_TOKEN_TTL_SECONDS,
        };

        self.database
            .save_session(&refreshed)
            .map_err(|error| error.to_string())?;

        Ok(refreshed)
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
        let (stream, _) = connect_async(ws_url.as_str())
            .await
            .map_err(|error| ConnectionFailure::Retryable(error.to_string()))?;
        let connected_at = Instant::now();

        let connected = CloudStatus::connected();
        let _ = self.update_status_if_current(generation, connected.clone());
        self.emit_status(connected);

        let _ = self.sync_devices_from_server(&context).await;

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
                    let ping = ClientWsMessage {
                        id: Uuid::new_v4().to_string(),
                        message_type: "ping".to_string(),
                    };
                    let payload = serde_json::to_string(&ping)
                        .map_err(|error| ConnectionFailure::Retryable(error.to_string()))?;
                    if writer.send(Message::Text(payload.into())).await.is_err() {
                        return Ok(ConnectionExit::Disconnected {
                            connected_for: connected_at.elapsed(),
                            reason: Some("云端连接已断开".to_string()),
                        });
                    }
                }
                message = reader.next() => {
                    match message {
                        Some(Ok(Message::Text(text))) => {
                            self.handle_server_message(text.as_str()).await;
                        }
                        Some(Ok(Message::Close(_))) => {
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
        let Ok(message) = serde_json::from_str::<ServerWsMessage>(raw) else {
            return;
        };

        match message.message_type.as_str() {
            "device.online" => {
                let payload = message
                    .payload
                    .and_then(|value| serde_json::from_value::<DeviceOnlinePayload>(value).ok());
                if let Some(device_id) = message.from {
                    self.update_device_presence(&device_id, true, payload).await;
                }
            }
            "device.offline" => {
                if let Some(device_id) = message.from {
                    self.update_device_presence(&device_id, false, None).await;
                }
            }
            _ => {}
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

        self.database.save_cached_devices(&response.devices)?;
        self.emit_devices(response.devices);
        Ok(())
    }

    async fn update_device_presence(
        &self,
        device_id: &str,
        online: bool,
        payload: Option<DeviceOnlinePayload>,
    ) {
        let Ok(mut devices) = self.database.load_cached_devices() else {
            return;
        };

        let Some(device) = devices.iter_mut().find(|item| item.device_id == device_id) else {
            return;
        };

        device.online = online;

        if let Some(payload) = payload {
            device.name = payload.name;
            device.device_type = payload.device_type;
        }

        if self.database.save_cached_devices(&devices).is_ok() {
            self.emit_devices(devices);
        }
    }

    fn invalidate_auth(&self, message: String, generation: u64) {
        let _ = self.database.clear_session();
        let _ = self.database.clear_cached_devices();
        let _ = self.update_status_if_current(generation, CloudStatus::disconnected());
        self.emit_devices(Vec::new());
        self.emit_status(CloudStatus::disconnected());
        let _ = self.app.emit(AUTH_INVALIDATED_EVENT, message);
    }

    fn update_status_if_current(&self, generation: u64, status: CloudStatus) -> bool {
        let mut inner = self.inner.lock().expect("cloud manager poisoned");
        if inner.generation != generation {
            return false;
        }

        inner.status = status;
        true
    }

    fn emit_status(&self, status: CloudStatus) {
        let _ = self.app.emit(CLOUD_STATUS_EVENT, status);
    }

    fn emit_devices(&self, devices: Vec<DeviceInfo>) {
        let _ = self.app.emit(DEVICES_UPDATED_EVENT, devices);
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
