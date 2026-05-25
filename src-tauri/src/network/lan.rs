use std::{
    collections::{HashMap, HashSet},
    net::{IpAddr, Ipv4Addr},
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use if_addrs::get_if_addrs;
use mdns_sd::{DaemonEvent, ServiceDaemon, ServiceEvent, ServiceInfo};
use serde_json::json;
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, watch},
    time::timeout,
};
use tokio_tungstenite::{
    accept_hdr_async, connect_async,
    tungstenite::{
        handshake::server::{ErrorResponse, Request, Response},
        http::StatusCode,
        Message,
    },
    WebSocketStream,
};
use url::{form_urlencoded, Url};
use uuid::Uuid;

use crate::{
    crypto::keys::{account_hash, sign_payload, verify_signature},
    error::{AppError, AppResult},
    models::{unix_now_millis, DeviceIdentity, LAN_PORT},
    protocol::{
        AuthFailPayload, AuthRequestPayload, AuthResponsePayload, BusinessEnvelope, FileDataFrame,
        PeerEnvelope,
    },
    runtime_events::RuntimeEvent,
    store::db::Database,
};

const SERVICE_TYPE: &str = "_colink._tcp.local.";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const TRANSFER_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const REPLAY_DRIFT_MILLIS: i64 = 30_000;
const FAILURE_COOLDOWN_MILLIS: i64 = 60_000;

enum TransferStreamEvent {
    Activity,
    Closed,
}

#[derive(Clone)]
pub struct LanManager {
    database: Database,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    inner: Arc<Mutex<LanState>>,
}

struct LanState {
    generation: u64,
    cancel: Option<watch::Sender<bool>>,
    peers: HashMap<String, mpsc::UnboundedSender<BusinessEnvelope>>,
    peer_endpoints: HashMap<String, (String, u16)>,
    transfer_tokens: HashMap<String, String>,
    transfer_senders: HashMap<String, mpsc::UnboundedSender<FileDataFrame>>,
    blocked_until: HashMap<String, i64>,
}

#[derive(Clone)]
struct LanContext {
    device: DeviceIdentity,
    account_hash: String,
}

enum InboundRoute {
    Peer,
    Transfer { session_id: String },
}

impl LanManager {
    pub fn new(database: Database, event_tx: mpsc::UnboundedSender<RuntimeEvent>) -> Self {
        Self {
            database,
            event_tx,
            inner: Arc::new(Mutex::new(LanState {
                generation: 0,
                cancel: None,
                peers: HashMap::new(),
                peer_endpoints: HashMap::new(),
                transfer_tokens: HashMap::new(),
                transfer_senders: HashMap::new(),
                blocked_until: HashMap::new(),
            })),
        }
    }

    pub fn start(&self) -> AppResult<()> {
        let settings = self
            .database
            .load_settings()?
            .ok_or_else(|| AppError::message("本地设置未初始化"))?;
        if !settings.lan_discovery {
            self.stop();
            return Ok(());
        }
        let session = self.database.load_session()?;
        let device = self.database.load_device_identity()?;
        let (Some(session), Some(device)) = (session, device) else {
            self.stop();
            return Ok(());
        };
        if session.user_id != device.user_id {
            self.stop();
            return Ok(());
        }

        let context = LanContext {
            account_hash: account_hash(&session.user_id),
            device,
        };
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let generation = {
            let mut inner = self.inner.lock().expect("lan manager poisoned");
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.cancel = Some(cancel_tx);
            inner.peers.clear();
            inner.transfer_tokens.clear();
            inner.transfer_senders.clear();
            inner.generation
        };
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager.run(generation, context, cancel_rx).await;
        });
        Ok(())
    }

    pub fn stop(&self) {
        let (peers, transfer_senders) = {
            let mut inner = self.inner.lock().expect("lan manager poisoned");
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.peer_endpoints.clear();
            inner.transfer_tokens.clear();
            (
                std::mem::take(&mut inner.peers),
                std::mem::take(&mut inner.transfer_senders),
            )
        };
        drop((peers, transfer_senders));
    }

    pub fn has_peer(&self, device_id: &str) -> bool {
        self.inner
            .lock()
            .expect("lan manager poisoned")
            .peers
            .contains_key(device_id)
    }

    pub fn peer_ids(&self) -> HashSet<String> {
        self.inner
            .lock()
            .expect("lan manager poisoned")
            .peers
            .keys()
            .cloned()
            .collect()
    }

    pub fn send(&self, device_id: &str, message: BusinessEnvelope) -> AppResult<()> {
        let sender = self
            .inner
            .lock()
            .expect("lan manager poisoned")
            .peers
            .get(device_id)
            .cloned()
            .ok_or_else(|| AppError::message("LAN 对端未连接"))?;
        sender
            .send(message)
            .map_err(|_| AppError::message("LAN 对端不可用"))
    }

    pub fn peer_endpoint(&self, device_id: &str) -> Option<(String, u16)> {
        self.inner
            .lock()
            .expect("lan manager poisoned")
            .peer_endpoints
            .get(device_id)
            .cloned()
    }

    pub fn register_transfer_token(&self, session_id: &str, token: &str) {
        self.inner
            .lock()
            .expect("lan manager poisoned")
            .transfer_tokens
            .insert(session_id.to_string(), token.to_string());
    }

    pub fn unregister_transfer(&self, session_id: &str) {
        let sender = {
            let mut inner = self.inner.lock().expect("lan manager poisoned");
            inner.transfer_tokens.remove(session_id);
            inner.transfer_senders.remove(session_id)
        };
        drop(sender);
    }

    pub fn send_transfer_frame(&self, session_id: &str, frame: FileDataFrame) -> AppResult<()> {
        let sender = self
            .inner
            .lock()
            .expect("lan manager poisoned")
            .transfer_senders
            .get(session_id)
            .cloned()
            .ok_or_else(|| AppError::message("LAN 数据连接不存在"))?;
        sender
            .send(frame)
            .map_err(|_| AppError::message("LAN 数据连接不可用"))
    }

    pub async fn connect_transfer(
        &self,
        session_id: &str,
        token: &str,
        ip: &str,
        port: u16,
    ) -> AppResult<()> {
        let url = Url::parse(&format!(
            "ws://{ip}:{port}/transfer/{session_id}?token={token}"
        ))?;
        let (stream, _) = connect_async(url.as_str())
            .await
            .map_err(|error| AppError::message(error.to_string()))?;
        self.attach_transfer_stream(session_id.to_string(), stream)
            .await
    }

    async fn run(
        &self,
        generation: u64,
        context: LanContext,
        mut cancel_rx: watch::Receiver<bool>,
    ) {
        let Ok(listener) = TcpListener::bind(("0.0.0.0", LAN_PORT)).await else {
            let _ = self.event_tx.send(RuntimeEvent::Log {
                level: "warn".to_string(),
                source: "lan".to_string(),
                message: "本地 LAN 监听端口绑定失败".to_string(),
            });
            return;
        };

        let Ok(mdns) = ServiceDaemon::new() else {
            let _ = self.event_tx.send(RuntimeEvent::Log {
                level: "warn".to_string(),
                source: "lan".to_string(),
                message: "mDNS 服务初始化失败".to_string(),
            });
            return;
        };

        let _ = mdns.set_ip_check_interval(5);
        let browse_rx = match mdns.browse(SERVICE_TYPE) {
            Ok(receiver) => receiver,
            Err(error) => {
                let _ = self.event_tx.send(RuntimeEvent::Log {
                    level: "warn".to_string(),
                    source: "lan".to_string(),
                    message: format!("mDNS 浏览启动失败: {error}"),
                });
                return;
            }
        };
        let monitor_rx = mdns.monitor().ok();

        if let Some(ip) = pick_local_ipv4() {
            let _ = self.register_service(&mdns, &context, ip);
            let _ = self.event_tx.send(RuntimeEvent::LocalEndpoint {
                ip: ip.to_string(),
                port: LAN_PORT,
            });
        }

        loop {
            tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        break;
                    }
                }
                accepted = listener.accept() => {
                    let Ok((stream, _addr)) = accepted else {
                        continue;
                    };
                    let manager = self.clone();
                    let context = context.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = manager.handle_inbound(generation, context, stream).await;
                    });
                }
                event = browse_rx.recv_async() => {
                    let Ok(event) = event else {
                        continue;
                    };
                    if let ServiceEvent::ServiceResolved(service) = event {
                        self.handle_service_resolved(generation, context.clone(), *service);
                    }
                }
                event = recv_monitor_event(&monitor_rx), if monitor_rx.is_some() => {
                    if let Some(event) = event {
                        match event {
                            DaemonEvent::IpAdd(ip) if ip.is_ipv4() => {
                                let _ = self.register_service(&mdns, &context, ip);
                                let _ = self.event_tx.send(RuntimeEvent::LocalEndpoint {
                                    ip: ip.to_string(),
                                    port: LAN_PORT,
                                });
                            }
                            DaemonEvent::IpDel(_) => {
                                if let Some(ip) = pick_local_ipv4() {
                                    let _ = self.register_service(&mdns, &context, ip);
                                    let _ = self.event_tx.send(RuntimeEvent::LocalEndpoint {
                                        ip: ip.to_string(),
                                        port: LAN_PORT,
                                    });
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let _ = mdns.shutdown();
        self.clear_peers_for_generation(generation);
    }

    fn register_service(
        &self,
        mdns: &ServiceDaemon,
        context: &LanContext,
        ip: IpAddr,
    ) -> AppResult<()> {
        let hostname = hostname::get()
            .ok()
            .and_then(|value| value.into_string().ok())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "colink-desktop".to_string());
        let instance_name = format!("colink-{}", &context.device.device_id[..8]);
        let properties = [
            ("accountHash", context.account_hash.as_str()),
            ("deviceId", context.device.device_id.as_str()),
            ("version", "1"),
        ];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &format!("{hostname}.local."),
            ip.to_string(),
            LAN_PORT,
            &properties[..],
        )
        .map_err(|error| AppError::message(error.to_string()))?
        .enable_addr_auto();
        mdns.register(info)
            .map_err(|error| AppError::message(error.to_string()))
    }

    fn handle_service_resolved(
        &self,
        generation: u64,
        context: LanContext,
        service: mdns_sd::ResolvedService,
    ) {
        let Some(found_account_hash) = service.get_property_val_str("accountHash") else {
            return;
        };
        if found_account_hash != context.account_hash {
            return;
        }
        let Some(device_id) = service.get_property_val_str("deviceId").map(str::to_string) else {
            return;
        };
        if device_id == context.device.device_id {
            return;
        }

        let Some(ip) = service
            .get_addresses()
            .iter()
            .map(|item| item.to_ip_addr())
            .find(|addr| matches!(addr, IpAddr::V4(ipv4) if !ipv4.is_loopback()))
        else {
            return;
        };

        let port = service.get_port();
        let _ = self.event_tx.send(RuntimeEvent::LanDiscovered {
            device_id: device_id.clone(),
            ip: ip.to_string(),
            port,
            source: "mdns".to_string(),
        });
        self.remember_peer_endpoint(&device_id, ip, port);

        if !should_initiate(&context.device.device_id, &device_id) {
            return;
        }
        if self.has_peer(&device_id) || self.is_blocked(&device_id) {
            return;
        }

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let _ = manager
                .connect_outbound(generation, context, device_id, ip, port)
                .await;
        });
    }

    async fn connect_outbound(
        &self,
        generation: u64,
        context: LanContext,
        device_id: String,
        ip: IpAddr,
        port: u16,
    ) -> AppResult<()> {
        let url = Url::parse(&format!("ws://{ip}:{port}/peer"))?;
        let (stream, _) = connect_async(url.as_str())
            .await
            .map_err(|error| AppError::message(error.to_string()))?;
        match perform_outbound_handshake(stream, &context, &self.database).await {
            Ok((stream, peer_device_id)) => {
                self.attach_peer_stream(generation, peer_device_id, stream)
                    .await
            }
            Err(error) => {
                self.block_device(&device_id);
                Err(error)
            }
        }
    }

    async fn handle_inbound(
        &self,
        generation: u64,
        context: LanContext,
        stream: TcpStream,
    ) -> AppResult<()> {
        let route = Arc::new(Mutex::new(None));
        let route_for_callback = route.clone();
        let manager = self.clone();
        let stream =
            accept_hdr_async(
                stream,
                move |request: &Request, response: Response| match manager
                    .resolve_inbound_route(request)
                {
                    Ok(next_route) => {
                        *route_for_callback.lock().expect("inbound route poisoned") =
                            Some(next_route);
                        Ok(response)
                    }
                    Err(response) => Err(response),
                },
            )
            .await
            .map_err(|error| AppError::message(error.to_string()))?;
        let route = route
            .lock()
            .expect("inbound route poisoned")
            .take()
            .unwrap_or(InboundRoute::Peer);
        match route {
            InboundRoute::Peer => {
                let (stream, peer_device_id) =
                    perform_inbound_handshake(stream, &context, &self.database).await?;
                self.attach_peer_stream(generation, peer_device_id, stream)
                    .await
            }
            InboundRoute::Transfer { session_id } => {
                self.attach_transfer_stream(session_id, stream).await
            }
        }
    }

    async fn attach_peer_stream<S>(
        &self,
        generation: u64,
        peer_device_id: String,
        stream: WebSocketStream<S>,
    ) -> AppResult<()>
    where
        WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
            + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin
            + Send
            + 'static,
    {
        let now = unix_now_millis();
        {
            let inner = self.inner.lock().expect("lan manager poisoned");
            if inner.generation != generation {
                return Ok(());
            }
            if let Some(until) = inner.blocked_until.get(&peer_device_id) {
                if *until > now {
                    return Ok(());
                }
            }
        }

        let (tx, mut rx) = mpsc::unbounded_channel::<BusinessEnvelope>();
        {
            let mut inner = self.inner.lock().expect("lan manager poisoned");
            inner.peers.insert(peer_device_id.clone(), tx);
        }
        let _ = self.event_tx.send(RuntimeEvent::LanConnected {
            device_id: peer_device_id.clone(),
        });

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let (mut writer, mut reader) = stream.split();
            loop {
                tokio::select! {
                    outbound = rx.recv() => {
                        let Some(outbound) = outbound else {
                            break;
                        };
                        if let Ok(text) = serde_json::to_string(&outbound) {
                            if writer.send(Message::Text(text.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    inbound = reader.next() => {
                        match inbound {
                            Some(Ok(Message::Text(text))) => {
                                if let Ok(message) = serde_json::from_str::<BusinessEnvelope>(&text) {
                                    let _ = manager.event_tx.send(RuntimeEvent::LanMessage {
                                        from: peer_device_id.clone(),
                                        message,
                                    });
                                }
                            }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            Some(Ok(Message::Ping(payload))) => {
                                if writer.send(Message::Pong(payload)).await.is_err() {
                                    break;
                                }
                            }
                            Some(Ok(_)) => {}
                        }
                    }
                }
            }
            manager.detach_peer(generation, &peer_device_id);
        });
        Ok(())
    }

    async fn attach_transfer_stream<S>(
        &self,
        session_id: String,
        stream: WebSocketStream<S>,
    ) -> AppResult<()>
    where
        WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
            + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin
            + Send
            + 'static,
    {
        let (tx, mut rx) = mpsc::unbounded_channel::<FileDataFrame>();
        {
            let mut inner = self.inner.lock().expect("lan manager poisoned");
            inner.transfer_senders.insert(session_id.clone(), tx);
        }

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let (mut writer, mut reader) = stream.split();
            loop {
                let event = timeout(TRANSFER_IDLE_TIMEOUT, async {
                    tokio::select! {
                        outbound = rx.recv() => {
                            let Some(outbound) = outbound else {
                                return TransferStreamEvent::Closed;
                            };
                            if writer.send(Message::Binary(outbound.encode().into())).await.is_err() {
                                return TransferStreamEvent::Closed;
                            }
                            TransferStreamEvent::Activity
                        }
                        inbound = reader.next() => {
                            match inbound {
                                Some(Ok(Message::Binary(bytes))) => {
                                    if let Some(frame) = FileDataFrame::decode(bytes.as_ref()) {
                                        let _ = manager.event_tx.send(RuntimeEvent::LanTransferFrame {
                                            session_id: session_id.clone(),
                                            frame,
                                        });
                                    }
                                    TransferStreamEvent::Activity
                                }
                                Some(Ok(Message::Close(_))) | None | Some(Err(_)) => TransferStreamEvent::Closed,
                                Some(Ok(Message::Ping(payload))) => {
                                    if writer.send(Message::Pong(payload)).await.is_err() {
                                        return TransferStreamEvent::Closed;
                                    }
                                    TransferStreamEvent::Activity
                                }
                                Some(Ok(_)) => TransferStreamEvent::Activity,
                            }
                        }
                    }
                })
                .await;

                match event {
                    Ok(TransferStreamEvent::Activity) => {}
                    Ok(TransferStreamEvent::Closed) | Err(_) => break,
                }
            }
            manager.detach_transfer(&session_id);
        });
        Ok(())
    }

    fn detach_peer(&self, generation: u64, device_id: &str) {
        let should_emit = {
            let mut inner = self.inner.lock().expect("lan manager poisoned");
            if inner.generation != generation {
                return;
            }
            inner.peers.remove(device_id).is_some()
        };
        if should_emit {
            let _ = self.event_tx.send(RuntimeEvent::LanDisconnected {
                device_id: device_id.to_string(),
            });
        }
    }

    fn detach_transfer(&self, session_id: &str) {
        let should_emit = self
            .inner
            .lock()
            .expect("lan manager poisoned")
            .transfer_senders
            .remove(session_id)
            .is_some();
        if should_emit {
            let _ = self.event_tx.send(RuntimeEvent::LanTransferClosed {
                session_id: session_id.to_string(),
            });
        }
    }

    fn clear_peers_for_generation(&self, generation: u64) {
        let peer_ids = {
            let mut inner = self.inner.lock().expect("lan manager poisoned");
            if inner.generation != generation {
                return;
            }
            inner.peers.drain().map(|(key, _)| key).collect::<Vec<_>>()
        };
        for device_id in peer_ids {
            let _ = self
                .event_tx
                .send(RuntimeEvent::LanDisconnected { device_id });
        }
    }

    fn is_blocked(&self, device_id: &str) -> bool {
        let now = unix_now_millis();
        self.inner
            .lock()
            .expect("lan manager poisoned")
            .blocked_until
            .get(device_id)
            .map(|until| *until > now)
            .unwrap_or(false)
    }

    fn block_device(&self, device_id: &str) {
        self.inner
            .lock()
            .expect("lan manager poisoned")
            .blocked_until
            .insert(
                device_id.to_string(),
                unix_now_millis() + FAILURE_COOLDOWN_MILLIS,
            );
    }

    fn remember_peer_endpoint(&self, device_id: &str, ip: IpAddr, port: u16) {
        self.inner
            .lock()
            .expect("lan manager poisoned")
            .peer_endpoints
            .insert(device_id.to_string(), (ip.to_string(), port));
    }

    fn resolve_inbound_route(&self, request: &Request) -> Result<InboundRoute, ErrorResponse> {
        let path = request.uri().path();
        if path == "/peer" || path == "/" {
            return Ok(InboundRoute::Peer);
        }

        let Some(session_id) = path.strip_prefix("/transfer/") else {
            return Err(reject_ws(StatusCode::NOT_FOUND, "unknown websocket path"));
        };
        if session_id.is_empty() {
            return Err(reject_ws(StatusCode::BAD_REQUEST, "missing session id"));
        }

        let token = request
            .uri()
            .query()
            .and_then(|query| {
                form_urlencoded::parse(query.as_bytes())
                    .find_map(|(key, value)| (key == "token").then(|| value.into_owned()))
            })
            .unwrap_or_default();
        if token.is_empty() {
            return Err(reject_ws(
                StatusCode::UNAUTHORIZED,
                "missing transfer token",
            ));
        }
        if !self.consume_transfer_token(session_id, &token) {
            return Err(reject_ws(StatusCode::FORBIDDEN, "invalid transfer token"));
        }

        Ok(InboundRoute::Transfer {
            session_id: session_id.to_string(),
        })
    }

    fn consume_transfer_token(&self, session_id: &str, token: &str) -> bool {
        let mut inner = self.inner.lock().expect("lan manager poisoned");
        match inner.transfer_tokens.get(session_id) {
            Some(expected) if expected == token => {
                inner.transfer_tokens.remove(session_id);
                true
            }
            _ => false,
        }
    }
}

fn reject_ws(status: StatusCode, body: &str) -> ErrorResponse {
    Response::builder()
        .status(status)
        .body(Some(body.to_string()))
        .expect("valid websocket error response")
}

async fn perform_outbound_handshake(
    mut stream: WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    context: &LanContext,
    database: &Database,
) -> AppResult<(
    WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    String,
)> {
    let nonce = Uuid::new_v4().simple().to_string();
    let timestamp = unix_now_millis();
    let proof = format!("{}{}{}", context.device.device_id, timestamp, nonce);
    let signature = sign_payload(&context.device.private_key, proof.as_bytes())?;
    let request = PeerEnvelope {
        message_type: "auth.request".to_string(),
        payload: serde_json::to_value(AuthRequestPayload {
            device_id: context.device.device_id.clone(),
            timestamp,
            nonce: nonce.clone(),
            signature,
        })?,
    };
    write_peer_message(&mut stream, &request).await?;

    let response = timeout(HANDSHAKE_TIMEOUT, read_peer_message(&mut stream))
        .await
        .map_err(|_| AppError::message("LAN 握手超时"))??;
    if response.message_type == "auth.fail" {
        let payload: AuthFailPayload = serde_json::from_value(response.payload)?;
        return Err(AppError::message(payload.reason));
    }
    if response.message_type != "auth.response" {
        return Err(AppError::message("LAN 握手响应类型错误"));
    }
    let payload: AuthResponsePayload = serde_json::from_value(response.payload)?;
    if payload.peer_nonce != nonce {
        return Err(AppError::message("LAN 握手 nonce 不匹配"));
    }
    verify_known_peer(database, &payload.device_id, |public_key| {
        let proof = format!(
            "{}{}{}{}",
            payload.device_id, payload.timestamp, payload.nonce, payload.peer_nonce
        );
        verify_signature(public_key, proof.as_bytes(), &payload.signature)
    })?;
    ensure_timestamp(payload.timestamp)?;

    let confirm = PeerEnvelope {
        message_type: "auth.confirm".to_string(),
        payload: json!({}),
    };
    write_peer_message(&mut stream, &confirm).await?;
    Ok((stream, payload.device_id))
}

async fn perform_inbound_handshake(
    mut stream: WebSocketStream<TcpStream>,
    context: &LanContext,
    database: &Database,
) -> AppResult<(WebSocketStream<TcpStream>, String)> {
    let request = timeout(HANDSHAKE_TIMEOUT, read_peer_message(&mut stream))
        .await
        .map_err(|_| AppError::message("LAN 握手超时"))??;
    if request.message_type != "auth.request" {
        let _ = send_auth_fail(&mut stream, "invalid auth.request").await;
        return Err(AppError::message("LAN 握手请求类型错误"));
    }
    let payload: AuthRequestPayload = serde_json::from_value(request.payload)?;
    ensure_timestamp(payload.timestamp)?;
    verify_known_peer(database, &payload.device_id, |public_key| {
        let proof = format!(
            "{}{}{}",
            payload.device_id, payload.timestamp, payload.nonce
        );
        verify_signature(public_key, proof.as_bytes(), &payload.signature)
    })?;

    let nonce = Uuid::new_v4().simple().to_string();
    let timestamp = unix_now_millis();
    let proof = format!(
        "{}{}{}{}",
        context.device.device_id, timestamp, nonce, payload.nonce
    );
    let signature = sign_payload(&context.device.private_key, proof.as_bytes())?;
    let response = PeerEnvelope {
        message_type: "auth.response".to_string(),
        payload: serde_json::to_value(AuthResponsePayload {
            device_id: context.device.device_id.clone(),
            timestamp,
            nonce,
            peer_nonce: payload.nonce.clone(),
            signature,
        })?,
    };
    write_peer_message(&mut stream, &response).await?;

    let confirm = timeout(HANDSHAKE_TIMEOUT, read_peer_message(&mut stream))
        .await
        .map_err(|_| AppError::message("LAN 握手超时"))??;
    if confirm.message_type != "auth.confirm" {
        return Err(AppError::message("LAN 握手确认类型错误"));
    }
    Ok((stream, payload.device_id))
}

async fn send_auth_fail<S>(stream: &mut WebSocketStream<S>, reason: &str) -> AppResult<()>
where
    WebSocketStream<S>:
        futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let frame = PeerEnvelope {
        message_type: "auth.fail".to_string(),
        payload: serde_json::to_value(AuthFailPayload {
            reason: reason.to_string(),
        })?,
    };
    write_peer_message(stream, &frame).await
}

async fn write_peer_message<S>(
    stream: &mut WebSocketStream<S>,
    value: &PeerEnvelope,
) -> AppResult<()>
where
    WebSocketStream<S>:
        futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let text = serde_json::to_string(value)?;
    stream
        .send(Message::Text(text.into()))
        .await
        .map_err(|error| AppError::message(error.to_string()))
}

async fn read_peer_message<S>(stream: &mut WebSocketStream<S>) -> AppResult<PeerEnvelope>
where
    WebSocketStream<S>:
        futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(message) = stream.next().await {
        match message.map_err(|error| AppError::message(error.to_string()))? {
            Message::Text(text) => return Ok(serde_json::from_str(&text)?),
            Message::Close(_) => return Err(AppError::message("LAN 连接已关闭")),
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }
    Err(AppError::message("LAN 连接已结束"))
}

fn verify_known_peer<F>(database: &Database, device_id: &str, verify: F) -> AppResult<()>
where
    F: FnOnce(&str) -> AppResult<bool>,
{
    let devices = database.load_cached_devices()?;
    let public_key = devices
        .iter()
        .find(|item| item.device_id == device_id)
        .map(|item| item.public_key.as_str())
        .ok_or_else(|| AppError::message("unknown device"))?;
    if !verify(public_key)? {
        return Err(AppError::message("signature invalid"));
    }
    Ok(())
}

fn ensure_timestamp(timestamp: i64) -> AppResult<()> {
    let drift = (unix_now_millis() - timestamp).abs();
    if drift > REPLAY_DRIFT_MILLIS {
        return Err(AppError::message("timestamp drift too large"));
    }
    Ok(())
}

async fn recv_monitor_event(
    receiver: &Option<mdns_sd::Receiver<DaemonEvent>>,
) -> Option<DaemonEvent> {
    let receiver = receiver.as_ref()?;
    receiver.recv_async().await.ok()
}

fn pick_local_ipv4() -> Option<IpAddr> {
    get_if_addrs().ok().and_then(|interfaces| {
        interfaces
            .into_iter()
            .find_map(|interface| match interface.ip() {
                IpAddr::V4(ipv4) if !ipv4.is_loopback() && ipv4 != Ipv4Addr::UNSPECIFIED => {
                    Some(IpAddr::V4(ipv4))
                }
                _ => None,
            })
    })
}

fn should_initiate(local_device_id: &str, peer_device_id: &str) -> bool {
    local_device_id < peer_device_id
}
