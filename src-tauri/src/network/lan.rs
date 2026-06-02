use std::{
    collections::{HashMap, HashSet, VecDeque},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use futures_util::{SinkExt, StreamExt};
use mdns_sd::{DaemonEvent, IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
    time::{interval, timeout, Instant, MissedTickBehavior},
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
use tracing::{debug, info, warn};
use url::{form_urlencoded, Url};
use uuid::Uuid;

use crate::{
    crypto::{
        keys::{sign_payload, verify_signature},
        lan::{choose_suite, pairing_code, supported_suites, LanSessionCrypto, AES_256_GCM_SUITE},
    },
    error::{AppError, AppResult},
    i18n::{self, TextKey},
    models::{
        unix_now_millis, DeviceIdentity, LanPairingCandidate, LanPairingRequest,
        TrustedPeerKeyRecord, LAN_PORT,
    },
    protocol::{
        BusinessEnvelope, BusinessNegotiatePayload, EncryptedBusinessPayload, FileDataFrame,
        HandshakeAcceptPayload, HandshakeProofPayload, HandshakeRejectPayload, PeerEnvelope,
        SwimEnvelope, SwimGossip, SwimPayload,
    },
    runtime_events::RuntimeEvent,
    store::db::Database,
    sync::MutexExt,
};

const SERVICE_TYPE: &str = "_colink._tcp.local.";
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PAIRING_TIMEOUT: Duration = Duration::from_secs(60);
const TRANSFER_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const REPLAY_DRIFT_MILLIS: i64 = 30_000;
const BUSINESS_IDLE_TIMEOUT_SECS: u64 = 180;
const PING_INTERVAL_SECS: u64 = 15;
const KEEPALIVE_TIMEOUT_SECS: u64 = 45;
const SWIM_PERIOD: Duration = Duration::from_millis(1_000);
const SWIM_DIRECT_TIMEOUT: Duration = Duration::from_millis(500);
const SWIM_INDIRECT_TIMEOUT: Duration = Duration::from_millis(500);
const SWIM_SUSPECT_TIMEOUT_MILLIS: i64 = 5_000;
const SWIM_MAX_GOSSIP: usize = 10;
const SWIM_MAX_BODY_BYTES: usize = 16 * 1024;

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
    active_device: Option<DeviceIdentity>,
    cancel: Option<watch::Sender<bool>>,
    peers: HashMap<String, PeerEntry>,
    peer_endpoints: HashMap<String, (String, u16)>,
    members: HashMap<String, MemberRecord>,
    gossip: VecDeque<SwimGossip>,
    probe_cursor: usize,
    seq: u64,
    transfer_tokens: HashMap<String, String>,
    transfer_senders: HashMap<String, mpsc::UnboundedSender<FileDataFrame>>,
    pending_pairings: HashMap<String, oneshot::Sender<bool>>,
    pairing_candidates: HashMap<String, LanPairingCandidate>,
}

#[derive(Debug, Clone)]
struct MemberRecord {
    state: MemberState,
    incarnation: i64,
    updated_at: i64,
}

struct PeerConnection {
    connection_id: Uuid,
    sender: mpsc::UnboundedSender<BusinessEnvelope>,
    initiated_by_local: bool,
}

struct PendingLanSend {
    message: BusinessEnvelope,
    result_tx: oneshot::Sender<AppResult<()>>,
}

enum PeerEntry {
    Connected(PeerConnection),
    Connecting(VecDeque<PendingLanSend>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MemberState {
    Alive,
    Suspect,
    Dead,
    Left,
}

impl MemberState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Alive => "alive",
            Self::Suspect => "suspect",
            Self::Dead => "dead",
            Self::Left => "left",
        }
    }

    fn from_str(value: &str) -> Option<Self> {
        match value {
            "alive" => Some(Self::Alive),
            "suspect" => Some(Self::Suspect),
            "dead" => Some(Self::Dead),
            "left" => Some(Self::Left),
            _ => None,
        }
    }

    fn priority(self) -> u8 {
        match self {
            Self::Alive => 0,
            Self::Suspect => 1,
            Self::Dead => 2,
            Self::Left => 3,
        }
    }
}

#[derive(Clone)]
struct LanContext {
    device: DeviceIdentity,
    incarnation: i64,
}

enum InboundRoute {
    Peer,
    Transfer { session_id: String },
}

enum TrustState {
    Trusted,
    Unknown,
    KeyChanged,
}

struct HandshakeResult<S> {
    stream: WebSocketStream<S>,
    peer_device_id: String,
    crypto: LanSessionCrypto,
}

struct PeerProof {
    device_id: String,
    public_key: String,
    name: String,
    nonce: String,
}

impl LanManager {
    pub fn new(database: Database, event_tx: mpsc::UnboundedSender<RuntimeEvent>) -> Self {
        Self {
            database,
            event_tx,
            inner: Arc::new(Mutex::new(LanState {
                generation: 0,
                active_device: None,
                cancel: None,
                peers: HashMap::new(),
                peer_endpoints: HashMap::new(),
                members: HashMap::new(),
                gossip: VecDeque::new(),
                probe_cursor: 0,
                seq: 0,
                transfer_tokens: HashMap::new(),
                transfer_senders: HashMap::new(),
                pending_pairings: HashMap::new(),
                pairing_candidates: HashMap::new(),
            })),
        }
    }

    pub fn start(&self) -> AppResult<()> {
        let settings = self
            .database
            .load_settings()?
            .ok_or_else(|| AppError::message(self.user_text(TextKey::SettingsNotInitialized)))?;
        if !settings.lan_discovery {
            info!("lan discovery disabled");
            self.stop();
            return Ok(());
        }

        let device = self.database.load_device_identity()?;
        let Some(device) = device else {
            debug!("lan manager skipped because device identity is missing");
            self.stop();
            return Ok(());
        };

        let context = LanContext {
            device,
            incarnation: unix_now_millis(),
        };
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let generation = {
            let mut inner = self.inner.lock_unpoisoned();
            if inner.cancel.is_some()
                && inner
                    .active_device
                    .as_ref()
                    .is_some_and(|active| same_lan_identity(active, &context.device))
            {
                debug!(device_id = %context.device.device_id, "lan manager already running");
                return Ok(());
            }
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.active_device = Some(context.device.clone());
            inner.cancel = Some(cancel_tx);
            inner.peers.clear();
            inner.peer_endpoints.clear();
            inner.members.clear();
            inner.gossip.clear();
            inner.transfer_tokens.clear();
            inner.transfer_senders.clear();
            inner.pending_pairings.clear();
            inner.pairing_candidates.clear();
            inner.seq = 0;
            inner.probe_cursor = 0;
            inner.generation
        };

        self.push_gossip(SwimGossip {
            device_id: context.device.device_id.clone(),
            state: MemberState::Alive.as_str().to_string(),
            incarnation: context.incarnation,
        });
        self.emit_pairing_candidates();
        info!(generation = generation, device_id = %context.device.device_id, "lan manager starting");

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            manager.run(generation, context, cancel_rx).await;
        });
        Ok(())
    }

    pub fn stop(&self) {
        let (peers, transfer_senders, pending) = {
            let mut inner = self.inner.lock_unpoisoned();
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.active_device = None;
            inner.peer_endpoints.clear();
            inner.members.clear();
            inner.gossip.clear();
            inner.pairing_candidates.clear();
            inner.transfer_tokens.clear();
            (
                std::mem::take(&mut inner.peers),
                std::mem::take(&mut inner.transfer_senders),
                std::mem::take(&mut inner.pending_pairings),
            )
        };
        drop((peers, transfer_senders, pending));
        self.emit_pairing_candidates();
        info!("lan manager stopped");
    }

    pub fn trusted_member_states(&self) -> HashMap<String, String> {
        let trusted = self
            .database
            .load_trusted_peer_keys()
            .map(|records| {
                records
                    .into_iter()
                    .filter(|record| record.lan_paired)
                    .map(|record| record.device_id)
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();

        let inner = self.inner.lock_unpoisoned();
        inner
            .members
            .iter()
            .filter(|(_, record)| matches!(record.state, MemberState::Alive | MemberState::Suspect))
            .filter_map(|(device_id, record)| {
                trusted
                    .contains(device_id)
                    .then(|| (device_id.clone(), record.state.as_str().to_string()))
            })
            .collect()
    }

    pub fn is_swim_alive(&self, device_id: &str) -> bool {
        self.inner
            .lock_unpoisoned()
            .members
            .get(device_id)
            .is_some_and(|member| matches!(member.state, MemberState::Alive | MemberState::Suspect))
    }

    pub fn is_available(&self, device_id: &str) -> bool {
        self.is_swim_alive(device_id)
            && self.is_lan_trusted(device_id)
            && self.peer_endpoint(device_id).is_some()
    }

    pub async fn send(&self, device_id: &str, message: BusinessEnvelope) -> AppResult<()> {
        if !self.is_available(device_id) {
            return Err(AppError::message(
                self.user_text(TextKey::LanPeerNotConnected),
            ));
        }

        let context = self.load_context()?;
        let generation = self.current_generation();
        let (ip, port) = self
            .peer_endpoint(device_id)
            .ok_or_else(|| AppError::message(self.user_text(TextKey::LanDeviceNotFound)))?;
        let ip = ip
            .parse::<IpAddr>()
            .map_err(|_| AppError::message(self.user_text(TextKey::LanDeviceAddressInvalid)))?;

        let mut message = message;
        loop {
            let receiver = {
                let mut inner = self.inner.lock_unpoisoned();
                if inner.generation != generation {
                    return Err(AppError::message(
                        self.user_text(TextKey::LanPeerUnavailable),
                    ));
                }

                match inner.peers.get_mut(device_id) {
                    Some(PeerEntry::Connected(peer)) => {
                        let sender = peer.sender.clone();
                        drop(inner);
                        match sender.send(message) {
                            Ok(()) => return Ok(()),
                            Err(error) => {
                                message = error.0;
                                self.remove_stale_peer_sender(device_id, &sender);
                                continue;
                            }
                        }
                    }
                    Some(PeerEntry::Connecting(queue)) => {
                        let (tx, rx) = oneshot::channel();
                        queue.push_back(PendingLanSend {
                            message,
                            result_tx: tx,
                        });
                        rx
                    }
                    None => {
                        let (tx, rx) = oneshot::channel();
                        let mut queue = VecDeque::new();
                        queue.push_back(PendingLanSend {
                            message,
                            result_tx: tx,
                        });
                        inner
                            .peers
                            .insert(device_id.to_string(), PeerEntry::Connecting(queue));
                        let manager = self.clone();
                        let device_id = device_id.to_string();
                        tauri::async_runtime::spawn(async move {
                            manager
                                .connect_pending_peer(generation, context, device_id, ip, port)
                                .await;
                        });
                        rx
                    }
                }
            };

            return receiver
                .await
                .map_err(|_| AppError::message(self.user_text(TextKey::LanPeerUnavailable)))?;
        }
    }

    pub fn peer_endpoint(&self, device_id: &str) -> Option<(String, u16)> {
        self.inner
            .lock_unpoisoned()
            .peer_endpoints
            .get(device_id)
            .cloned()
    }

    pub fn list_pairing_candidates(&self) -> Vec<LanPairingCandidate> {
        let mut candidates = self
            .inner
            .lock_unpoisoned()
            .pairing_candidates
            .values()
            .cloned()
            .collect::<Vec<_>>();
        candidates.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        candidates
    }

    pub fn respond_pairing(&self, request_id: &str, accepted: bool) -> AppResult<()> {
        let sender = self
            .inner
            .lock_unpoisoned()
            .pending_pairings
            .remove(request_id)
            .ok_or_else(|| AppError::message(self.user_text(TextKey::PairingRequestMissing)))?;
        sender
            .send(accepted)
            .map_err(|_| AppError::message(self.user_text(TextKey::PairingRequestEnded)))
    }

    pub fn forget_trust(&self, device_id: &str) -> AppResult<()> {
        self.database.remove_trusted_peer_key(device_id)?;
        self.detach_peer(self.current_generation(), device_id);
        Ok(())
    }

    pub fn start_pairing(&self, device_id: &str) -> AppResult<()> {
        let generation = self.current_generation();
        let context = self.load_context()?;
        let (ip, port) = self
            .peer_endpoint(device_id)
            .ok_or_else(|| AppError::message(self.user_text(TextKey::LanDeviceNotFound)))?;
        let ip = ip
            .parse::<IpAddr>()
            .map_err(|_| AppError::message(self.user_text(TextKey::LanDeviceAddressInvalid)))?;
        let manager = self.clone();
        let device_id = device_id.to_string();
        tauri::async_runtime::spawn(async move {
            let _ = manager
                .connect_outbound(generation, context, device_id, ip, port, true)
                .await;
        });
        Ok(())
    }

    pub fn register_transfer_token(&self, session_id: &str, token: &str) {
        self.inner
            .lock_unpoisoned()
            .transfer_tokens
            .insert(session_id.to_string(), token.to_string());
    }

    pub fn unregister_transfer(&self, session_id: &str) {
        let sender = {
            let mut inner = self.inner.lock_unpoisoned();
            inner.transfer_tokens.remove(session_id);
            inner.transfer_senders.remove(session_id)
        };
        drop(sender);
    }

    pub fn send_transfer_frame(&self, session_id: &str, frame: FileDataFrame) -> AppResult<()> {
        let sender = self
            .inner
            .lock_unpoisoned()
            .transfer_senders
            .get(session_id)
            .cloned()
            .ok_or_else(|| AppError::message("LAN data connection does not exist"))?;
        sender
            .send(frame)
            .map_err(|_| AppError::message("LAN data connection is unavailable"))
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
            warn!(port = LAN_PORT, "lan listener bind failed");
            let _ = self.event_tx.send(RuntimeEvent::Log {
                level: "warn".to_string(),
                source: "lan".to_string(),
                message: "local LAN listener port bind failed".to_string(),
            });
            self.finalize_generation(generation);
            return;
        };

        let Ok(mdns) = ServiceDaemon::new() else {
            warn!("mdns daemon initialization failed");
            let _ = self.event_tx.send(RuntimeEvent::Log {
                level: "warn".to_string(),
                source: "lan".to_string(),
                message: "mDNS service initialization failed".to_string(),
            });
            self.finalize_generation(generation);
            return;
        };

        let _ = mdns.set_ip_check_interval(5);
        let browse_rx = match mdns.browse(SERVICE_TYPE) {
            Ok(receiver) => receiver,
            Err(error) => {
                warn!(%error, "mdns browse failed");
                let _ = self.event_tx.send(RuntimeEvent::Log {
                    level: "warn".to_string(),
                    source: "lan".to_string(),
                    message: format!("mDNS browse failed to start: {error}"),
                });
                self.finalize_generation(generation);
                return;
            }
        };
        let monitor_rx = mdns.monitor().ok();
        let mut swim_interval = interval(SWIM_PERIOD);
        swim_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut suspect_interval = interval(Duration::from_millis(500));
        suspect_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);

        let _ = self.register_service(&mdns, &context);
        info!(generation = generation, "lan discovery loop started");

        loop {
            tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && *cancel_rx.borrow() {
                        self.broadcast_left(&context).await;
                        break;
                    }
                }
                accepted = listener.accept() => {
                    let Ok((stream, addr)) = accepted else {
                        continue;
                    };
                    let manager = self.clone();
                    let context = context.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = manager.handle_inbound_tcp(generation, context, stream, addr).await;
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
                _ = swim_interval.tick() => {
                    self.probe_next_member(generation, context.clone()).await;
                }
                _ = suspect_interval.tick() => {
                    self.promote_expired_suspects(generation);
                }
                event = recv_monitor_event(&monitor_rx), if monitor_rx.is_some() => {
                    if let Some(event) = event {
                        match event {
                            DaemonEvent::IpAdd(ip) if ip.is_ipv4() => {
                                debug!(%ip, "mdns address added");
                                let _ = self.register_service(&mdns, &context);
                            }
                            DaemonEvent::IpDel(_) => {
                                debug!("mdns address removed");
                                let _ = self.register_service(&mdns, &context);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        let _ = mdns.shutdown();
        self.finalize_generation(generation);
        self.clear_peers_for_generation(generation);
        info!(generation = generation, "lan discovery loop stopped");
    }

    fn register_service(&self, mdns: &ServiceDaemon, context: &LanContext) -> AppResult<()> {
        let hostname = hostname::get()
            .ok()
            .and_then(|value| value.into_string().ok())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "colink-desktop".to_string());
        let instance_name = format!("colink-{}", &context.device.device_id[..8]);
        let properties = [
            ("deviceId", context.device.device_id.as_str()),
            ("version", "1"),
        ];
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &format!("{hostname}.local."),
            "",
            LAN_PORT,
            &properties[..],
        )
        .map_err(|error| AppError::message(error.to_string()))?;
        let mut info = info.enable_addr_auto();
        info.set_interfaces(vec![IfKind::IPv4]);
        info!(
            port = LAN_PORT,
            "registering mdns service on ipv4 interfaces"
        );
        mdns.register(info)
            .map_err(|error| AppError::message(error.to_string()))
    }

    fn handle_service_resolved(
        &self,
        generation: u64,
        context: LanContext,
        service: mdns_sd::ResolvedService,
    ) {
        let Some(version) = service.get_property_val_str("version") else {
            return;
        };
        if version != "1" {
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
            .filter_map(|item| match item.to_ip_addr() {
                IpAddr::V4(ipv4) if is_usable_lan_ipv4(ipv4) => Some(ipv4),
                _ => None,
            })
            .max_by_key(|ip| lan_ipv4_score(*ip))
            .map(IpAddr::V4)
        else {
            return;
        };

        let port = service.get_port();
        debug!(device_id = %device_id, %ip, port = port, "resolved mdns peer");
        self.remember_peer_endpoint(&device_id, ip, port);
        let _ = self.event_tx.send(RuntimeEvent::LanDiscovered {
            device_id: device_id.clone(),
            ip: ip.to_string(),
            port,
            source: "mdns".to_string(),
        });

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            match manager.send_swim_ping(&context, &device_id).await {
                Ok(ack) => {
                    debug!(
                        device_id = %device_id,
                        from = %ack.payload.from,
                        seq = ack.payload.seq,
                        gossip_count = ack.payload.gossip.len(),
                        "mdns-triggered swim ping succeeded"
                    );
                    manager.process_swim_message(generation, &context, ack, None);
                }
                Err(error) => {
                    debug!(device_id = %device_id, %error, "mdns-triggered swim ping failed");
                }
            }
        });
    }

    async fn handle_inbound_tcp(
        &self,
        generation: u64,
        context: LanContext,
        stream: TcpStream,
        remote_addr: SocketAddr,
    ) -> AppResult<()> {
        debug!(%remote_addr, "accepted lan tcp connection");
        let mut peek = [0_u8; 32];
        let read = stream.peek(&mut peek).await?;
        if read > 0 && peek[..read].starts_with(b"POST /peer/swim/v1") {
            return self
                .handle_swim_http(generation, context, stream, remote_addr)
                .await;
        }

        self.handle_inbound_ws(generation, context, stream).await
    }

    async fn handle_swim_http(
        &self,
        generation: u64,
        context: LanContext,
        mut stream: TcpStream,
        remote_addr: SocketAddr,
    ) -> AppResult<()> {
        let request = match read_http_body(&mut stream).await {
            Ok(body) => body,
            Err(error) => {
                let _ =
                    write_http_response(&mut stream, StatusCode::BAD_REQUEST, &error.to_string())
                        .await;
                return Err(error);
            }
        };
        let message = serde_json::from_slice::<SwimEnvelope>(&request)?;
        let response = self
            .handle_swim_message(generation, &context, message, remote_addr)
            .await?;
        let payload = serde_json::to_vec(&response)?;
        write_http_response(
            &mut stream,
            StatusCode::OK,
            &String::from_utf8_lossy(&payload),
        )
        .await?;
        Ok(())
    }

    async fn handle_inbound_ws(
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
                        *route_for_callback.lock_unpoisoned() = Some(next_route);
                        Ok(response)
                    }
                    Err(response) => Err(response),
                },
            )
            .await
            .map_err(|error| AppError::message(error.to_string()))?;
        let route = route.lock_unpoisoned().take().unwrap_or(InboundRoute::Peer);
        match route {
            InboundRoute::Peer => {
                debug!("handling inbound lan peer websocket");
                let context = self.load_context().unwrap_or(context);
                let session =
                    perform_inbound_handshake(self, stream, &context, &self.database).await?;
                self.attach_peer_stream(generation, session, false).await
            }
            InboundRoute::Transfer { session_id } => {
                debug!(%session_id, "handling inbound lan transfer websocket");
                self.attach_transfer_stream(session_id, stream).await
            }
        }
    }

    async fn connect_outbound(
        &self,
        generation: u64,
        context: LanContext,
        expected_device_id: String,
        ip: IpAddr,
        port: u16,
        allow_pairing: bool,
    ) -> AppResult<()> {
        let context = self.load_context().unwrap_or(context);
        let url = Url::parse(&format!("ws://{ip}:{port}/peer"))?;
        debug!(expected_device_id = %expected_device_id, %ip, port = port, "connecting outbound lan peer");
        let (stream, _) = connect_async(url.as_str())
            .await
            .map_err(|error| AppError::message(error.to_string()))?;
        let session = perform_outbound_handshake(
            self,
            stream,
            &context,
            &self.database,
            &expected_device_id,
            allow_pairing,
        )
        .await?;
        self.attach_peer_stream(generation, session, true).await
    }

    async fn connect_pending_peer(
        &self,
        generation: u64,
        context: LanContext,
        device_id: String,
        ip: IpAddr,
        port: u16,
    ) {
        let result = self
            .connect_outbound(generation, context, device_id.clone(), ip, port, false)
            .await;
        if let Err(error) = result {
            warn!(%device_id, %error, "on-demand lan peer connection failed");
            self.fail_connecting_peer(generation, &device_id, error.to_string());
        }
    }

    fn fail_connecting_peer(&self, generation: u64, device_id: &str, reason: String) {
        let pending = {
            let mut inner = self.inner.lock_unpoisoned();
            if inner.generation != generation {
                return;
            }
            match inner.peers.remove(device_id) {
                Some(PeerEntry::Connecting(queue)) => queue,
                other => {
                    if let Some(entry) = other {
                        inner.peers.insert(device_id.to_string(), entry);
                    }
                    VecDeque::new()
                }
            }
        };

        for pending in pending {
            let _ = pending
                .result_tx
                .send(Err(AppError::message(reason.clone())));
        }
    }

    fn remove_stale_peer_sender(
        &self,
        device_id: &str,
        sender: &mpsc::UnboundedSender<BusinessEnvelope>,
    ) {
        let mut inner = self.inner.lock_unpoisoned();
        let should_remove = inner.peers.get(device_id).is_some_and(|entry| match entry {
            PeerEntry::Connected(peer) => peer.sender.same_channel(sender),
            PeerEntry::Connecting(_) => false,
        });
        if should_remove {
            inner.peers.remove(device_id);
        }
    }

    async fn attach_peer_stream<S>(
        &self,
        generation: u64,
        session: HandshakeResult<S>,
        initiated_by_local: bool,
    ) -> AppResult<()>
    where
        WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
            + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin
            + Send
            + 'static,
    {
        let peer_device_id = session.peer_device_id;
        let connection_id = Uuid::new_v4();
        let (tx, mut rx) = mpsc::unbounded_channel::<BusinessEnvelope>();
        let (pending, was_connected) = {
            let mut inner = self.inner.lock_unpoisoned();
            if inner.generation != generation {
                return Ok(());
            }

            if let Some(PeerEntry::Connected(existing)) = inner.peers.get(&peer_device_id) {
                let local_device_id = inner
                    .active_device
                    .as_ref()
                    .map(|device| device.device_id.as_str())
                    .unwrap_or_default();
                let local_is_smaller = local_device_id < peer_device_id.as_str();
                let keep_existing = if local_is_smaller {
                    existing.initiated_by_local
                } else {
                    !existing.initiated_by_local
                };
                if keep_existing {
                    debug!(device_id = %peer_device_id, "dropping duplicate lan peer stream");
                    return Ok(());
                }
            }

            let previous = inner.peers.remove(&peer_device_id);
            let was_connected = matches!(previous, Some(PeerEntry::Connected(_)));
            let pending = match previous {
                Some(PeerEntry::Connecting(queue)) => queue,
                _ => VecDeque::new(),
            };
            inner.peers.insert(
                peer_device_id.clone(),
                PeerEntry::Connected(PeerConnection {
                    connection_id,
                    sender: tx.clone(),
                    initiated_by_local,
                }),
            );
            inner.pairing_candidates.remove(&peer_device_id);
            (pending, was_connected)
        };

        for pending in pending {
            let result = tx
                .send(pending.message)
                .map_err(|_| AppError::message(self.user_text(TextKey::LanPeerUnavailable)));
            let _ = pending.result_tx.send(result);
        }
        self.emit_pairing_candidates();
        if !was_connected {
            info!(device_id = %peer_device_id, "lan peer connected");
            let _ = self.event_tx.send(RuntimeEvent::LanConnected {
                device_id: peer_device_id.clone(),
            });
        }

        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let (mut writer, mut reader) = session.stream.split();
            let mut crypto = session.crypto;
            let mut last_business_activity = Instant::now();
            let mut last_keepalive_activity = Instant::now();
            let mut ping_interval = tokio::time::interval(Duration::from_secs(PING_INTERVAL_SECS));
            ping_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            ping_interval.tick().await;
            let mut failed_outbound = None;
            loop {
                if last_keepalive_activity.elapsed() >= Duration::from_secs(KEEPALIVE_TIMEOUT_SECS)
                    || last_business_activity.elapsed()
                        >= Duration::from_secs(BUSINESS_IDLE_TIMEOUT_SECS)
                {
                    break;
                }
                tokio::select! {
                    outbound = rx.recv() => {
                        let Some(outbound) = outbound else {
                            break;
                        };
                        let encrypted = match crypto.encrypt(&outbound) {
                            Ok(payload) => payload,
                            Err(_) => {
                                failed_outbound = Some(outbound);
                                break;
                            }
                        };
                        let envelope = PeerEnvelope {
                            message_type: "business.v1.message".to_string(),
                            payload: match serde_json::to_value(encrypted) {
                                Ok(value) => value,
                                Err(_) => {
                                    failed_outbound = Some(outbound);
                                    break;
                                }
                            },
                        };
                        let text = match serde_json::to_string(&envelope) {
                            Ok(text) => text,
                            Err(_) => {
                                failed_outbound = Some(outbound);
                                break;
                            }
                        };
                        if writer.send(Message::Text(text.into())).await.is_err() {
                            failed_outbound = Some(outbound);
                            break;
                        }
                        last_business_activity = Instant::now();
                        last_keepalive_activity = Instant::now();
                    }
                    _ = ping_interval.tick() => {
                        if writer.send(Message::Ping(Vec::new().into())).await.is_err() {
                            break;
                        }
                    }
                    inbound = reader.next() => {
                        match inbound {
                            Some(Ok(Message::Text(text))) => {
                                last_keepalive_activity = Instant::now();
                                let Ok(envelope) = serde_json::from_str::<PeerEnvelope>(&text) else {
                                    continue;
                                };
                                if envelope.message_type != "business.v1.message" {
                                    continue;
                                }
                                let Ok(payload) = serde_json::from_value::<EncryptedBusinessPayload>(envelope.payload) else {
                                    break;
                                };
                                match crypto.decrypt(&payload) {
                                    Ok(message) => {
                                        last_business_activity = Instant::now();
                                        let _ = manager.event_tx.send(RuntimeEvent::LanMessage {
                                            from: peer_device_id.clone(),
                                            message,
                                        });
                                    }
                                    Err(_) => break,
                                }
                            }
                            Some(Ok(Message::Pong(_))) => {
                                last_keepalive_activity = Instant::now();
                            }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            Some(Ok(Message::Ping(payload))) => {
                                last_keepalive_activity = Instant::now();
                                if writer.send(Message::Pong(payload)).await.is_err() {
                                    break;
                                }
                            }
                            Some(Ok(_)) => {
                                last_keepalive_activity = Instant::now();
                            }
                        }
                    }
                }
            }
            let mut undelivered = Vec::new();
            if let Some(message) = failed_outbound {
                undelivered.push(message);
            }
            rx.close();
            while let Ok(message) = rx.try_recv() {
                undelivered.push(message);
            }
            if !undelivered.is_empty() {
                let _ = manager.event_tx.send(RuntimeEvent::LanSendFailed {
                    device_id: peer_device_id.clone(),
                    messages: undelivered,
                });
            }
            manager.detach_peer_connection(generation, &peer_device_id, connection_id);
            debug!(device_id = %peer_device_id, "lan peer stream ended");
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
            let mut inner = self.inner.lock_unpoisoned();
            inner.transfer_senders.insert(session_id.clone(), tx);
        }
        debug!(%session_id, "lan transfer stream attached");

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

    async fn handle_swim_message(
        &self,
        generation: u64,
        context: &LanContext,
        message: SwimEnvelope,
        remote_addr: SocketAddr,
    ) -> AppResult<SwimEnvelope> {
        if message.payload.from != context.device.device_id {
            self.remember_peer_endpoint(&message.payload.from, remote_addr.ip(), LAN_PORT);
        }

        match message.message_type.as_str() {
            "swim.ping" => {
                let seq = message.payload.seq;
                self.process_swim_message(generation, context, message, Some(remote_addr.ip()));
                Ok(self.swim_ack(context, seq))
            }
            "swim.ping-req" => {
                self.process_swim_message(
                    generation,
                    context,
                    message.clone(),
                    Some(remote_addr.ip()),
                );
                let target = message
                    .payload
                    .target
                    .ok_or_else(|| AppError::message("missing swim target"))?;
                if target == context.device.device_id {
                    return Ok(self.swim_ack(context, message.payload.seq));
                }
                let ack = self
                    .send_swim_ping(context, &target)
                    .await
                    .map_err(|error| AppError::message(error.to_string()))?;
                self.process_swim_message(generation, context, ack.clone(), None);
                Ok(ack)
            }
            _ => Err(AppError::message("unknown swim message")),
        }
    }

    fn process_swim_message(
        &self,
        generation: u64,
        context: &LanContext,
        message: SwimEnvelope,
        source_ip: Option<IpAddr>,
    ) {
        if self.current_generation() != generation {
            debug!(
                generation,
                from = %message.payload.from,
                message_type = %message.message_type,
                "ignored stale swim message"
            );
            return;
        }
        debug!(
            from = %message.payload.from,
            message_type = %message.message_type,
            seq = message.payload.seq,
            gossip_count = message.payload.gossip.len(),
            "processing swim message"
        );
        if let Some(ip) = source_ip {
            self.remember_peer_endpoint(&message.payload.from, ip, LAN_PORT);
        }
        self.observe_swim_alive(generation, context, &message.payload.from);
        for entry in message.payload.gossip {
            debug!(
                origin = %message.payload.from,
                device_id = %entry.device_id,
                state = %entry.state,
                incarnation = entry.incarnation,
                "processing swim gossip entry"
            );
            if entry.device_id == context.device.device_id
                && entry.state == MemberState::Suspect.as_str()
            {
                debug!(
                    origin = %message.payload.from,
                    incarnation = entry.incarnation,
                    "received swim suspicion for local device; gossiping self alive"
                );
                self.push_self_alive(context);
                continue;
            }
            self.merge_member(generation, context, &message.payload.from, entry);
        }
    }

    fn observe_swim_alive(&self, generation: u64, context: &LanContext, device_id: &str) {
        if device_id == context.device.device_id {
            return;
        }
        debug!(%device_id, "observed swim peer alive");
        self.mark_member(generation, context, device_id, MemberState::Alive, None);
    }

    async fn send_swim_ping(&self, context: &LanContext, target: &str) -> AppResult<SwimEnvelope> {
        let message = SwimEnvelope {
            message_type: "swim.ping".to_string(),
            payload: SwimPayload {
                seq: self.next_seq(),
                from: context.device.device_id.clone(),
                target: None,
                gossip: self.gossip_batch(),
            },
        };
        self.post_swim(target, message, SWIM_DIRECT_TIMEOUT).await
    }

    async fn send_swim_ping_req(
        &self,
        context: &LanContext,
        intermediary: &str,
        target: &str,
    ) -> AppResult<SwimEnvelope> {
        let message = SwimEnvelope {
            message_type: "swim.ping-req".to_string(),
            payload: SwimPayload {
                seq: self.next_seq(),
                from: context.device.device_id.clone(),
                target: Some(target.to_string()),
                gossip: self.gossip_batch(),
            },
        };
        self.post_swim(intermediary, message, SWIM_INDIRECT_TIMEOUT)
            .await
    }

    async fn post_swim(
        &self,
        target: &str,
        message: SwimEnvelope,
        timeout_duration: Duration,
    ) -> AppResult<SwimEnvelope> {
        let (ip, port) = self
            .peer_endpoint(target)
            .ok_or_else(|| AppError::message("SWIM target endpoint missing"))?;
        let url = format!("http://{ip}:{port}/peer/swim/v1");
        let client = reqwest::Client::new();
        let response = client
            .post(url)
            .timeout(timeout_duration)
            .json(&message)
            .send()
            .await?;
        if !response.status().is_success() {
            return Err(AppError::message("SWIM request failed"));
        }
        Ok(response.json::<SwimEnvelope>().await?)
    }

    async fn probe_next_member(&self, generation: u64, context: LanContext) {
        let Some(target) = self.next_probe_target(&context.device.device_id) else {
            return;
        };
        debug!(%target, "probing swim member");
        match self.send_swim_ping(&context, &target).await {
            Ok(ack) => {
                self.process_swim_message(generation, &context, ack, None);
                self.mark_member(generation, &context, &target, MemberState::Alive, None);
                return;
            }
            Err(error) => {
                debug!(%target, %error, "direct swim probe failed");
            }
        }

        let intermediaries = self.indirect_targets(&context.device.device_id, &target);
        for intermediary in intermediaries {
            if let Ok(ack) = self
                .send_swim_ping_req(&context, &intermediary, &target)
                .await
            {
                self.process_swim_message(generation, &context, ack, None);
                self.mark_member(generation, &context, &target, MemberState::Alive, None);
                return;
            }
        }

        self.mark_member(generation, &context, &target, MemberState::Suspect, None);
        warn!(%target, "swim member marked suspect");
    }

    fn next_probe_target(&self, local_device_id: &str) -> Option<String> {
        let mut inner = self.inner.lock_unpoisoned();
        let candidates = inner
            .members
            .iter()
            .filter(|(device_id, member)| {
                device_id.as_str() != local_device_id
                    && matches!(member.state, MemberState::Alive | MemberState::Suspect)
                    && inner.peer_endpoints.contains_key(*device_id)
            })
            .map(|(device_id, _)| device_id.clone())
            .collect::<Vec<_>>();
        if candidates.is_empty() {
            return None;
        }
        if inner.probe_cursor >= candidates.len() {
            inner.probe_cursor = 0;
        }
        let target = candidates[inner.probe_cursor].clone();
        inner.probe_cursor = (inner.probe_cursor + 1) % candidates.len();
        Some(target)
    }

    fn indirect_targets(&self, local_device_id: &str, target: &str) -> Vec<String> {
        self.inner
            .lock_unpoisoned()
            .members
            .iter()
            .filter(|(device_id, member)| {
                device_id.as_str() != local_device_id
                    && device_id.as_str() != target
                    && member.state == MemberState::Alive
            })
            .take(2)
            .map(|(device_id, _)| device_id.clone())
            .collect()
    }

    fn merge_member(&self, generation: u64, context: &LanContext, origin: &str, entry: SwimGossip) {
        if entry.incarnation > unix_now_millis() + 5 * 60 * 1000 {
            return;
        }
        let Some(state) = MemberState::from_str(&entry.state) else {
            return;
        };
        if state == MemberState::Left && origin != entry.device_id {
            return;
        }
        self.mark_member(
            generation,
            context,
            &entry.device_id,
            state,
            Some(entry.incarnation),
        );
    }

    fn mark_member(
        &self,
        generation: u64,
        context: &LanContext,
        device_id: &str,
        state: MemberState,
        incarnation: Option<i64>,
    ) {
        if device_id == context.device.device_id {
            return;
        }
        let now = unix_now_millis();
        let mut changed = false;
        let explicit_incarnation = incarnation.is_some();
        let next_incarnation = incarnation.unwrap_or_else(|| {
            self.inner
                .lock_unpoisoned()
                .members
                .get(device_id)
                .map(|member| member.incarnation)
                .unwrap_or(now)
        });

        {
            let mut inner = self.inner.lock_unpoisoned();
            if inner.generation != generation {
                debug!(
                    generation,
                    device_id,
                    state = state.as_str(),
                    incarnation = next_incarnation,
                    "ignored stale swim member update"
                );
                return;
            }
            let existing = inner.members.get(device_id).cloned();
            let accept = Self::should_accept_member_update(
                existing.as_ref(),
                state,
                next_incarnation,
                explicit_incarnation,
            );
            if accept {
                inner.members.insert(
                    device_id.to_string(),
                    MemberRecord {
                        state,
                        incarnation: next_incarnation,
                        updated_at: now,
                    },
                );
                changed = true;
                debug!(
                    device_id,
                    state = state.as_str(),
                    incarnation = next_incarnation,
                    previous_state = existing
                        .as_ref()
                        .map(|member| member.state.as_str())
                        .unwrap_or("none"),
                    previous_incarnation = existing.as_ref().map(|member| member.incarnation),
                    explicit_incarnation,
                    "accepted swim member update"
                );
            } else {
                debug!(
                    device_id,
                    state = state.as_str(),
                    incarnation = next_incarnation,
                    existing_state = existing
                        .as_ref()
                        .map(|member| member.state.as_str())
                        .unwrap_or("none"),
                    existing_incarnation = existing.as_ref().map(|member| member.incarnation),
                    explicit_incarnation,
                    "rejected swim member update"
                );
            }
        }

        if !changed {
            return;
        }

        self.push_gossip(SwimGossip {
            device_id: device_id.to_string(),
            state: state.as_str().to_string(),
            incarnation: next_incarnation,
        });

        match state {
            MemberState::Alive => {
                self.update_pairing_candidate(device_id, state);
                if self.is_lan_trusted(device_id) {
                    let _ = self.event_tx.send(RuntimeEvent::LanDeviceReachable {
                        device_id: device_id.to_string(),
                    });
                }
            }
            MemberState::Dead | MemberState::Left => {
                self.remove_pairing_candidate(device_id);
                let _ = self.event_tx.send(RuntimeEvent::LanDeviceUnreachable {
                    device_id: device_id.to_string(),
                });
                self.detach_peer(generation, device_id);
            }
            MemberState::Suspect => {
                self.update_pairing_candidate(device_id, state);
                if self.is_lan_trusted(device_id) {
                    let _ = self.event_tx.send(RuntimeEvent::LanDeviceStateChanged {
                        device_id: device_id.to_string(),
                    });
                }
            }
        }
    }

    fn should_accept_member_update(
        existing: Option<&MemberRecord>,
        state: MemberState,
        incarnation: i64,
        explicit_incarnation: bool,
    ) -> bool {
        match existing {
            Some(existing) if incarnation < existing.incarnation => false,
            Some(existing) if incarnation == existing.incarnation => {
                if explicit_incarnation {
                    Self::should_accept_same_incarnation_gossip(existing.state, state)
                } else {
                    state != existing.state
                }
            }
            _ => true,
        }
    }

    fn should_accept_same_incarnation_gossip(existing: MemberState, incoming: MemberState) -> bool {
        if matches!(existing, MemberState::Dead | MemberState::Left) {
            return false;
        }

        incoming.priority() > existing.priority()
    }

    fn promote_expired_suspects(&self, generation: u64) {
        let now = unix_now_millis();
        let expired = {
            let inner = self.inner.lock_unpoisoned();
            if inner.generation != generation {
                return;
            }
            inner
                .members
                .iter()
                .filter(|(_, member)| {
                    member.state == MemberState::Suspect
                        && now - member.updated_at >= SWIM_SUSPECT_TIMEOUT_MILLIS
                })
                .map(|(device_id, _)| device_id.clone())
                .collect::<Vec<_>>()
        };
        let Ok(context) = self.load_context() else {
            return;
        };
        for device_id in expired {
            self.mark_member(generation, &context, &device_id, MemberState::Dead, None);
        }
    }

    fn swim_ack(&self, context: &LanContext, seq: u64) -> SwimEnvelope {
        SwimEnvelope {
            message_type: "swim.ack".to_string(),
            payload: SwimPayload {
                seq,
                from: context.device.device_id.clone(),
                target: None,
                gossip: self.gossip_batch(),
            },
        }
    }

    async fn broadcast_left(&self, context: &LanContext) {
        let entry = SwimGossip {
            device_id: context.device.device_id.clone(),
            state: MemberState::Left.as_str().to_string(),
            incarnation: unix_now_millis(),
        };
        self.push_gossip(entry);
        let targets = self
            .inner
            .lock_unpoisoned()
            .members
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        for target in targets {
            let _ = self.send_swim_ping(context, &target).await;
        }
    }

    fn push_self_alive(&self, context: &LanContext) {
        self.push_gossip(SwimGossip {
            device_id: context.device.device_id.clone(),
            state: MemberState::Alive.as_str().to_string(),
            incarnation: unix_now_millis(),
        });
    }

    fn push_gossip(&self, entry: SwimGossip) {
        let mut inner = self.inner.lock_unpoisoned();
        inner.gossip.push_back(entry);
        while inner.gossip.len() > SWIM_MAX_GOSSIP * 4 {
            inner.gossip.pop_front();
        }
    }

    fn gossip_batch(&self) -> Vec<SwimGossip> {
        self.inner
            .lock_unpoisoned()
            .gossip
            .iter()
            .rev()
            .take(SWIM_MAX_GOSSIP)
            .cloned()
            .collect()
    }

    fn next_seq(&self) -> u64 {
        let mut inner = self.inner.lock_unpoisoned();
        inner.seq = inner.seq.saturating_add(1);
        inner.seq
    }

    fn is_lan_trusted(&self, device_id: &str) -> bool {
        self.database
            .load_trusted_peer_keys()
            .map(|records| {
                records
                    .iter()
                    .any(|record| record.device_id == device_id && record.lan_paired)
            })
            .unwrap_or(false)
    }

    async fn request_pairing(
        &self,
        device_id: &str,
        name: &str,
        public_key: &str,
        code: &str,
        reason: &str,
    ) -> AppResult<bool> {
        let request_id = Uuid::new_v4().to_string();
        let (tx, rx) = oneshot::channel();
        self.inner
            .lock_unpoisoned()
            .pending_pairings
            .insert(request_id.clone(), tx);
        let _ = self
            .event_tx
            .send(RuntimeEvent::LanPairingRequested(LanPairingRequest {
                request_id: request_id.clone(),
                device_id: device_id.to_string(),
                name: name.to_string(),
                code: code.to_string(),
                reason: reason.to_string(),
                public_key: public_key.to_string(),
            }));

        let result = timeout(PAIRING_TIMEOUT, rx).await;
        self.inner
            .lock_unpoisoned()
            .pending_pairings
            .remove(&request_id);
        result
            .map_err(|_| AppError::message("LAN pairing timed out"))?
            .map_err(|_| AppError::message("LAN pairing was cancelled"))
    }

    fn revoke_lan_pairing_for_key_change(&self, proof: &PeerProof) -> AppResult<()> {
        self.database
            .clear_lan_pairing(&proof.device_id, &proof.name, &proof.public_key)?;
        let _ = self.event_tx.send(RuntimeEvent::LanKeyChanged {
            device_id: proof.device_id.clone(),
            name: proof.name.clone(),
        });
        Ok(())
    }

    fn trust_peer(&self, proof: &PeerProof) -> AppResult<()> {
        let now = unix_now_millis();
        self.database.upsert_trusted_peer_key(TrustedPeerKeyRecord {
            device_id: proof.device_id.clone(),
            name: proof.name.clone(),
            public_key: proof.public_key.clone(),
            key_updated_at: now,
            trusted_at: Some(now),
            lan_paired: true,
        })
    }

    fn update_pairing_candidate(&self, device_id: &str, state: MemberState) {
        if state != MemberState::Alive {
            self.remove_pairing_candidate(device_id);
            return;
        }
        if self.is_lan_trusted(device_id) {
            self.remove_pairing_candidate(device_id);
            return;
        }
        let Some((ip, port)) = self.peer_endpoint(device_id) else {
            return;
        };
        self.inner.lock_unpoisoned().pairing_candidates.insert(
            device_id.to_string(),
            LanPairingCandidate {
                device_id: device_id.to_string(),
                ip,
                port,
                state: state.as_str().to_string(),
            },
        );
        self.emit_pairing_candidates();
    }

    fn remove_pairing_candidate(&self, device_id: &str) {
        let removed = self
            .inner
            .lock_unpoisoned()
            .pairing_candidates
            .remove(device_id)
            .is_some();
        if removed {
            self.emit_pairing_candidates();
        }
    }

    fn emit_pairing_candidates(&self) {
        let _ = self
            .event_tx
            .send(RuntimeEvent::LanPairingCandidatesUpdated(
                self.list_pairing_candidates(),
            ));
    }

    fn remember_peer_endpoint(&self, device_id: &str, ip: IpAddr, port: u16) {
        debug!(%device_id, %ip, port = port, "remembering lan peer endpoint");
        self.inner
            .lock_unpoisoned()
            .peer_endpoints
            .insert(device_id.to_string(), (ip.to_string(), port));
    }

    fn detach_peer(&self, generation: u64, device_id: &str) {
        let (should_emit, pending) = {
            let mut inner = self.inner.lock_unpoisoned();
            if inner.generation != generation {
                return;
            }
            match inner.peers.remove(device_id) {
                Some(PeerEntry::Connected(_)) => (true, VecDeque::new()),
                Some(PeerEntry::Connecting(queue)) => (false, queue),
                None => (false, VecDeque::new()),
            }
        };
        for pending in pending {
            let _ = pending.result_tx.send(Err(AppError::message(
                self.user_text(TextKey::LanPeerUnavailable),
            )));
        }
        if should_emit {
            warn!(%device_id, "lan peer disconnected");
            let _ = self.event_tx.send(RuntimeEvent::LanDisconnected {
                device_id: device_id.to_string(),
            });
        }
    }

    fn detach_peer_connection(&self, generation: u64, device_id: &str, connection_id: Uuid) {
        let should_emit = {
            let mut inner = self.inner.lock_unpoisoned();
            if inner.generation != generation {
                return;
            }
            let should_remove = inner.peers.get(device_id).is_some_and(|entry| match entry {
                PeerEntry::Connected(peer) => peer.connection_id == connection_id,
                PeerEntry::Connecting(_) => false,
            });
            if should_remove {
                inner.peers.remove(device_id);
                true
            } else {
                false
            }
        };
        if should_emit {
            warn!(%device_id, "lan peer disconnected");
            let _ = self.event_tx.send(RuntimeEvent::LanDisconnected {
                device_id: device_id.to_string(),
            });
        }
    }

    fn detach_transfer(&self, session_id: &str) {
        let should_emit = self
            .inner
            .lock_unpoisoned()
            .transfer_senders
            .remove(session_id)
            .is_some();
        if should_emit {
            debug!(%session_id, "lan transfer stream detached");
            let _ = self.event_tx.send(RuntimeEvent::LanTransferClosed {
                session_id: session_id.to_string(),
            });
        }
    }

    fn clear_peers_for_generation(&self, generation: u64) {
        let peer_ids = {
            let mut inner = self.inner.lock_unpoisoned();
            if inner.generation != generation {
                return;
            }
            inner
                .peers
                .drain()
                .filter_map(|(key, entry)| matches!(entry, PeerEntry::Connected(_)).then_some(key))
                .collect::<Vec<_>>()
        };
        for device_id in peer_ids {
            let _ = self
                .event_tx
                .send(RuntimeEvent::LanDisconnected { device_id });
        }
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
        let mut inner = self.inner.lock_unpoisoned();
        match inner.transfer_tokens.get(session_id) {
            Some(expected) if expected == token => {
                inner.transfer_tokens.remove(session_id);
                true
            }
            _ => false,
        }
    }

    fn load_context(&self) -> AppResult<LanContext> {
        let device = self
            .database
            .load_device_identity()?
            .ok_or_else(|| AppError::message("current device is not registered"))?;
        Ok(LanContext {
            device,
            incarnation: unix_now_millis(),
        })
    }

    fn current_generation(&self) -> u64 {
        self.inner.lock_unpoisoned().generation
    }

    fn finalize_generation(&self, generation: u64) {
        let mut inner = self.inner.lock_unpoisoned();
        if inner.generation != generation {
            return;
        }
        inner.active_device = None;
        inner.cancel = None;
    }

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

fn reject_ws(status: StatusCode, body: &str) -> ErrorResponse {
    Response::builder()
        .status(status)
        .body(Some(body.to_string()))
        .expect("valid websocket error response")
}

async fn perform_outbound_handshake(
    manager: &LanManager,
    mut stream: WebSocketStream<tokio_tungstenite::MaybeTlsStream<TcpStream>>,
    context: &LanContext,
    database: &Database,
    expected_device_id: &str,
    allow_pairing: bool,
) -> AppResult<HandshakeResult<tokio_tungstenite::MaybeTlsStream<TcpStream>>> {
    let request_nonce = Uuid::new_v4().simple().to_string();
    let request = build_handshake_proof(&context.device, &request_nonce)?;
    write_peer_message(&mut stream, "handshake.v1.request", &request).await?;

    let exchange = timeout(HANDSHAKE_TIMEOUT, read_peer_message(&mut stream))
        .await
        .map_err(|_| AppError::message("LAN handshake timed out"))??;
    if exchange.message_type == "handshake.v1.reject" {
        let payload: HandshakeRejectPayload = serde_json::from_value(exchange.payload)?;
        return Err(AppError::message(payload.reason));
    }
    if exchange.message_type != "handshake.v1.exchange" {
        return Err(AppError::message("invalid LAN handshake response type"));
    }
    let peer_payload: HandshakeProofPayload = serde_json::from_value(exchange.payload)?;
    if peer_payload.device_id != expected_device_id {
        return Err(AppError::message("LAN handshake device mismatch"));
    }
    verify_handshake_proof(&peer_payload)?;
    let proof = PeerProof {
        device_id: peer_payload.device_id,
        public_key: peer_payload.public_key,
        name: peer_payload.name,
        nonce: peer_payload.nonce,
    };
    let trust = trust_state(database, &proof)?;
    if matches!(trust, TrustState::KeyChanged) {
        manager.revoke_lan_pairing_for_key_change(&proof)?;
        let _ = write_peer_message(
            &mut stream,
            "handshake.v1.reject",
            &HandshakeRejectPayload {
                reason: "key_changed".to_string(),
            },
        )
        .await;
        return Err(AppError::message("LAN device key changed"));
    }
    if !matches!(trust, TrustState::Trusted) {
        if !allow_pairing {
            return Err(AppError::message("LAN device key is not trusted"));
        }
        let reason = match trust {
            TrustState::Unknown => "unknown_device",
            TrustState::Trusted => "trusted",
            TrustState::KeyChanged => "key_changed",
        };
        let code = pairing_code(
            &context.device.public_key,
            &proof.public_key,
            &request_nonce,
            &proof.nonce,
        );
        if !manager
            .request_pairing(
                &proof.device_id,
                &proof.name,
                &proof.public_key,
                &code,
                reason,
            )
            .await?
        {
            return Err(AppError::message("user cancelled LAN pairing"));
        }
    }

    let final_message = timeout(HANDSHAKE_TIMEOUT, read_peer_message(&mut stream))
        .await
        .map_err(|_| AppError::message("LAN handshake timed out"))??;
    if final_message.message_type == "handshake.v1.reject" {
        let payload: HandshakeRejectPayload = serde_json::from_value(final_message.payload)?;
        return Err(AppError::message(payload.reason));
    }
    if final_message.message_type != "handshake.v1.accept" {
        return Err(AppError::message("invalid LAN handshake confirmation type"));
    }
    let accept: HandshakeAcceptPayload = serde_json::from_value(final_message.payload)?;
    if accept.device_id != proof.device_id {
        return Err(AppError::message(
            "LAN handshake confirmation device mismatch",
        ));
    }
    if !matches!(trust, TrustState::Trusted) {
        manager.trust_peer(&proof)?;
    }

    let crypto = negotiate_business_crypto(&mut stream, context, &proof, true).await?;
    Ok(HandshakeResult {
        stream,
        peer_device_id: proof.device_id,
        crypto,
    })
}

async fn perform_inbound_handshake(
    manager: &LanManager,
    mut stream: WebSocketStream<TcpStream>,
    context: &LanContext,
    database: &Database,
) -> AppResult<HandshakeResult<TcpStream>> {
    let request = timeout(HANDSHAKE_TIMEOUT, read_peer_message(&mut stream))
        .await
        .map_err(|_| AppError::message("LAN handshake timed out"))??;
    if request.message_type != "handshake.v1.request" {
        let _ = write_peer_message(
            &mut stream,
            "handshake.v1.reject",
            &HandshakeRejectPayload {
                reason: "invalid_handshake".to_string(),
            },
        )
        .await;
        return Err(AppError::message("invalid LAN handshake request type"));
    }
    let peer_payload: HandshakeProofPayload = serde_json::from_value(request.payload)?;
    verify_handshake_proof(&peer_payload)?;
    let proof = PeerProof {
        device_id: peer_payload.device_id,
        public_key: peer_payload.public_key,
        name: peer_payload.name,
        nonce: peer_payload.nonce,
    };
    let trust = trust_state(database, &proof)?;
    if matches!(trust, TrustState::KeyChanged) {
        manager.revoke_lan_pairing_for_key_change(&proof)?;
        write_peer_message(
            &mut stream,
            "handshake.v1.reject",
            &HandshakeRejectPayload {
                reason: "key_changed".to_string(),
            },
        )
        .await?;
        return Err(AppError::message("LAN device key changed"));
    }

    let exchange_nonce = Uuid::new_v4().simple().to_string();
    let exchange = build_handshake_proof(&context.device, &exchange_nonce)?;
    write_peer_message(&mut stream, "handshake.v1.exchange", &exchange).await?;

    if !matches!(trust, TrustState::Trusted) {
        let reason = match trust {
            TrustState::Unknown => "unknown_device",
            TrustState::Trusted => "trusted",
            TrustState::KeyChanged => "key_changed",
        };
        let code = pairing_code(
            &proof.public_key,
            &context.device.public_key,
            &proof.nonce,
            &exchange_nonce,
        );
        let accepted = manager
            .request_pairing(
                &proof.device_id,
                &proof.name,
                &proof.public_key,
                &code,
                reason,
            )
            .await?;
        if !accepted {
            write_peer_message(
                &mut stream,
                "handshake.v1.reject",
                &HandshakeRejectPayload {
                    reason: "user_rejected".to_string(),
                },
            )
            .await?;
            return Err(AppError::message("user rejected LAN pairing"));
        }
        manager.trust_peer(&proof)?;
    }

    write_peer_message(
        &mut stream,
        "handshake.v1.accept",
        &HandshakeAcceptPayload {
            device_id: context.device.device_id.clone(),
        },
    )
    .await?;

    let crypto = negotiate_business_crypto(&mut stream, context, &proof, false).await?;
    Ok(HandshakeResult {
        stream,
        peer_device_id: proof.device_id,
        crypto,
    })
}

async fn negotiate_business_crypto<S>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    proof: &PeerProof,
    local_is_initiator: bool,
) -> AppResult<LanSessionCrypto>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let local_supported = supported_suites();
    write_peer_message(
        stream,
        "business.v1.negotiate",
        &BusinessNegotiatePayload {
            supported: local_supported.clone(),
            preferred: AES_256_GCM_SUITE.to_string(),
        },
    )
    .await?;
    let message = timeout(HANDSHAKE_TIMEOUT, read_peer_message(stream))
        .await
        .map_err(|_| AppError::message("LAN encryption negotiation timed out"))??;
    if message.message_type != "business.v1.negotiate" {
        return Err(AppError::message("invalid LAN encryption negotiation type"));
    }
    let peer: BusinessNegotiatePayload = serde_json::from_value(message.payload)?;
    let suite = choose_suite(&local_supported, &peer.supported, local_is_initiator)
        .ok_or_else(|| AppError::message("no compatible LAN encryption suite is available"))?;
    LanSessionCrypto::new(
        suite,
        &context.device.private_key,
        &proof.public_key,
        local_is_initiator,
    )
}

fn build_handshake_proof(device: &DeviceIdentity, nonce: &str) -> AppResult<HandshakeProofPayload> {
    let timestamp = unix_now_millis();
    let proof = format!("{}{}{}", device.device_id, timestamp, nonce);
    let signature = sign_payload(&device.private_key, proof.as_bytes())?;
    Ok(HandshakeProofPayload {
        device_id: device.device_id.clone(),
        public_key: device.public_key.clone(),
        name: device.name.clone(),
        timestamp,
        nonce: nonce.to_string(),
        signature,
    })
}

fn verify_handshake_proof(payload: &HandshakeProofPayload) -> AppResult<()> {
    ensure_timestamp(payload.timestamp)?;
    let proof = format!(
        "{}{}{}",
        payload.device_id, payload.timestamp, payload.nonce
    );
    if !verify_signature(&payload.public_key, proof.as_bytes(), &payload.signature)? {
        return Err(AppError::message("signature invalid"));
    }
    Ok(())
}

fn trust_state(database: &Database, proof: &PeerProof) -> AppResult<TrustState> {
    let trusts = database.load_trusted_peer_keys()?;
    let Some(record) = trusts
        .iter()
        .find(|record| record.device_id == proof.device_id)
    else {
        return Ok(TrustState::Unknown);
    };
    if !record.lan_paired {
        return Ok(TrustState::Unknown);
    }
    if record.public_key == proof.public_key {
        Ok(TrustState::Trusted)
    } else {
        Ok(TrustState::KeyChanged)
    }
}

async fn write_peer_message<S, T>(
    stream: &mut WebSocketStream<S>,
    message_type: &str,
    payload: &T,
) -> AppResult<()>
where
    T: serde::Serialize,
    WebSocketStream<S>:
        futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let envelope = PeerEnvelope {
        message_type: message_type.to_string(),
        payload: serde_json::to_value(payload)?,
    };
    let text = serde_json::to_string(&envelope)?;
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
            Message::Close(_) => return Err(AppError::message("LAN connection was closed")),
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }
    Err(AppError::message("LAN connection ended"))
}

fn ensure_timestamp(timestamp: i64) -> AppResult<()> {
    let drift = (unix_now_millis() - timestamp).abs();
    if drift > REPLAY_DRIFT_MILLIS {
        return Err(AppError::message("timestamp drift too large"));
    }
    Ok(())
}

async fn read_http_body(stream: &mut TcpStream) -> AppResult<Vec<u8>> {
    let mut buffer = Vec::new();
    let header_end;
    loop {
        let mut chunk = [0_u8; 1024];
        let read = timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .map_err(|_| AppError::message("HTTP request timeout"))??;
        if read == 0 {
            return Err(AppError::message("HTTP request ended"));
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(index) = find_header_end(&buffer) {
            header_end = index;
            break;
        }
        if buffer.len() > SWIM_MAX_BODY_BYTES {
            return Err(AppError::message("HTTP request too large"));
        }
    }

    let headers = String::from_utf8_lossy(&buffer[..header_end]);
    let content_length = headers
        .lines()
        .find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse::<usize>().ok())
                .flatten()
        })
        .ok_or_else(|| AppError::message("missing content-length"))?;
    if content_length > SWIM_MAX_BODY_BYTES {
        return Err(AppError::message("SWIM request too large"));
    }

    let body_start = header_end + 4;
    while buffer.len() < body_start + content_length {
        let mut chunk = vec![0_u8; body_start + content_length - buffer.len()];
        let read = timeout(Duration::from_secs(5), stream.read(&mut chunk))
            .await
            .map_err(|_| AppError::message("HTTP body timeout"))??;
        if read == 0 {
            return Err(AppError::message("HTTP body ended"));
        }
        buffer.extend_from_slice(&chunk[..read]);
    }

    Ok(buffer[body_start..body_start + content_length].to_vec())
}

async fn write_http_response(
    stream: &mut TcpStream,
    status: StatusCode,
    body: &str,
) -> AppResult<()> {
    let response = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status.as_u16(),
        status.canonical_reason().unwrap_or(""),
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;
    Ok(())
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

async fn recv_monitor_event(
    receiver: &Option<mdns_sd::Receiver<DaemonEvent>>,
) -> Option<DaemonEvent> {
    let receiver = receiver.as_ref()?;
    receiver.recv_async().await.ok()
}

fn lan_ipv4_score(ip: Ipv4Addr) -> u8 {
    match ip.octets() {
        [192, 168, _, _] => 4,
        [10, _, _, _] => 3,
        [172, second, _, _] if (16..=31).contains(&second) => 2,
        _ => 1,
    }
}

fn is_usable_lan_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_loopback()
        && !ip.is_link_local()
        && !ip.is_multicast()
        && !ip.is_broadcast()
        && ip != Ipv4Addr::UNSPECIFIED
}

fn same_lan_identity(left: &DeviceIdentity, right: &DeviceIdentity) -> bool {
    left.device_id == right.device_id
}

#[cfg(test)]
mod tests {
    use super::{LanManager, MemberRecord, MemberState};

    fn member(state: MemberState, incarnation: i64) -> MemberRecord {
        MemberRecord {
            state,
            incarnation,
            updated_at: 0,
        }
    }

    #[test]
    fn swim_gossip_same_incarnation_only_accepts_higher_priority_state() {
        let existing = member(MemberState::Alive, 100);
        assert!(LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Suspect,
            100,
            true,
        ));
        assert!(LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Dead,
            100,
            true,
        ));

        let existing = member(MemberState::Suspect, 100);
        assert!(LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Dead,
            100,
            true,
        ));
        assert!(!LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Alive,
            100,
            true,
        ));

        let existing = member(MemberState::Dead, 100);
        assert!(!LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Alive,
            100,
            true,
        ));
        assert!(!LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Suspect,
            100,
            true,
        ));

        let existing = member(MemberState::Left, 100);
        assert!(!LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Alive,
            100,
            true,
        ));
    }

    #[test]
    fn swim_gossip_higher_incarnation_overrides_any_state() {
        let existing = member(MemberState::Dead, 100);
        assert!(LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Alive,
            101,
            true,
        ));

        let existing = member(MemberState::Alive, 100);
        assert!(!LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Dead,
            99,
            true,
        ));
    }

    #[test]
    fn local_observation_can_change_state_without_new_incarnation() {
        let existing = member(MemberState::Suspect, 100);
        assert!(LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Alive,
            100,
            false,
        ));
    }
}
