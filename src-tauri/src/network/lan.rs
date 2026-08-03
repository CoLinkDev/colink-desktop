use std::{
    collections::{HashMap, HashSet, VecDeque},
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

#[cfg(target_os = "windows")]
use std::ffi::c_void;

use base64::{engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD}, Engine};
use futures_util::{stream::FuturesUnordered, SinkExt, StreamExt};
use mdns_sd::{DaemonEvent, DaemonStatus, IfKind, ServiceDaemon, ServiceEvent, ServiceInfo};
use rand::{rngs::OsRng, seq::SliceRandom, RngCore};
use serde::{Deserialize, Serialize};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch, Mutex as AsyncMutex},
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
#[cfg(target_os = "windows")]
use windows::Win32::{
    Foundation::HANDLE,
    System::Power::{
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, HPOWERNOTIFY, PowerRegisterSuspendResumeNotification,
        PowerUnregisterSuspendResumeNotification,
    },
    UI::WindowsAndMessaging::{DEVICE_NOTIFY_CALLBACK, PBT_APMRESUMEAUTOMATIC},
};

use crate::{
    crypto::{
        keys::{sign_payload, verify_signature},
        lan::{
            choose_suite, pairing_code, supported_suites, LanEphemeralKeyPair, LanSessionCrypto,
            AES_256_GCM_SUITE,
        },
    },
    error::{AppError, AppResult},
    i18n::{self, TextKey},
    models::{
        unix_now_millis, DeviceIdentity, LanPairingCandidate, LanPairingCompleted,
        LanPairingFailed, LanPairingRequest, TrustedPeerKeyRecord, LAN_PORT,
    },
    protocol::{
        check_business_protocol_version, check_lan_protocol_version, negotiated_lan_protocol_version,
        supports_lan_key_exchange, supports_lan_key_exchange_nonce, supports_lan_pair_string,
        supports_lan_pair_string_v2,
        AuthChallengePayload,
        AuthResponsePayload, BusinessEnvelope, BusinessKeyExchangeNoncePayload, CameraDataFrame,
        BusinessKeyExchangePayload, BusinessNegotiatePayload, BusinessVersionAckPayload,
        BusinessVersionPayload, EmptyPayload, EncryptedBusinessPayload, FileDataFrame, LanEnvelope,
        LanRejectPayload, PairingIdentityPayload, ProtocolHelloAckEnvelope, ProtocolHelloEnvelope,
        ProtocolHelloPayload, SwimEnvelope, SwimGossip, SwimPayload, VersionAckPayload,
        BUSINESS_PROTOCOL_VERSION, LAN_PROTOCOL_VERSION,
    },
    runtime_events::{CorrelatedBusinessMessage, RuntimeEvent},
    store::db::Database,
    sync::MutexExt,
};

const SERVICE_TYPE: &str = "_colink._tcp.local.";
const MDNS_PORT: u16 = 5_353;
const MDNS_SD_VERSION: &str = "0.20.3";
const MDNS_REBUILD_DEBOUNCE: Duration = Duration::from_secs(2);
const SYSTEM_RESUME_REBUILD_DELAY: Duration = Duration::from_secs(5);
const MIN_LAN_PORT: u16 = 1_024;
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);
const PAIRING_TIMEOUT: Duration = Duration::from_secs(240);
const TRANSFER_IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const HEARTBEAT_INTERVAL_SECS: u64 = 15;
const KEEPALIVE_TIMEOUT_SECS: u64 = 45;
const SWIM_PERIOD: Duration = Duration::from_millis(5_000);
const SWIM_DIRECT_TIMEOUT: Duration = Duration::from_millis(1_000);
const SWIM_INDIRECT_TIMEOUT: Duration = Duration::from_millis(2_000);
const SWIM_PROBE_BATCH_SIZE: usize = 2;
const SWIM_SUSPECT_MISSES: u8 = 2;
const SWIM_SUSPECT_TIMEOUT_MILLIS: i64 = 3_000;
const SWIM_MAX_GOSSIP: usize = 10;
const SWIM_MAX_BODY_BYTES: usize = 16 * 1024;
const CAMERA_SEND_BUFFER_CAPACITY: usize = 3;
const CAMERA_RECEIVE_BUFFER_CAPACITY: usize = 4;
const REASON_AUTH_UNKNOWN_DEVICE: &str = "colink:auth.unknown_device.v1";
const REASON_AUTH_KEY_CHANGED: &str = "colink:auth.key_changed.v1";
const REASON_PAIRING_CANCELLED: &str = "colink:pairing.cancelled.v1";
const REASON_PAIRING_USER_REJECTED: &str = "colink:pairing.user_rejected.v1";
const REASON_PAIRING_TIMEOUT: &str = "colink:pairing.timeout.v1";
const REASON_PAIRING_CONNECTION_CLOSED: &str = "colink:pairing.connection_closed.v1";
const REASON_PAIRING_PAIR_STRING_INVALID: &str = "colink:pairing.pair_string_invalid.v1";
const REASON_PAIRING_PAIR_STRING_EXPIRED: &str = "colink:pairing.pair_string_expired.v1";
const REASON_PAIRING_PAIR_STRING_UNAVAILABLE: &str = "colink:pairing.pair_string_unavailable.v1";
const REASON_KEY_EXCHANGE_SIGNATURE_INVALID: &str =
    "colink:key_exchange.signature_invalid.v1";
const REASON_KEY_EXCHANGE_TIMESTAMP_EXPIRED: &str =
    "colink:key_exchange.timestamp_expired.v1";
const REASON_KEY_EXCHANGE_GENERIC: &str = "colink:key_exchange.generic.v1";
const MESSAGE_AUTH_UNKNOWN_DEVICE: &str = "No trust record for this device";
const MESSAGE_AUTH_KEY_CHANGED: &str = "Peer public key differs from stored trust record";
const MESSAGE_PAIRING_CANCELLED: &str = "LAN pairing was cancelled";
const MESSAGE_PAIRING_USER_REJECTED: &str = "User declined the pairing request";
const MESSAGE_PAIRING_TIMEOUT: &str = "LAN pairing timed out";
const MESSAGE_PAIRING_PAIR_STRING_INVALID: &str = "Pair string is invalid";
const MESSAGE_PAIRING_PAIR_STRING_EXPIRED: &str = "Pair string has expired";
const MESSAGE_PAIRING_PAIR_STRING_UNAVAILABLE: &str = "Pair string is unavailable";
const PAIR_STRING_RECOMMENDED_TTL_MILLIS: i64 = 60 * 60 * 1_000;
const MESSAGE_KEY_EXCHANGE_SIGNATURE_INVALID: &str =
    "Ephemeral key signature verification failed";
const MESSAGE_KEY_EXCHANGE_TIMESTAMP_EXPIRED: &str = "Ephemeral key timestamp expired";
const MESSAGE_KEY_EXCHANGE_GENERIC: &str = "Ephemeral key exchange failed";

fn pairing_rejection(payload: serde_json::Value) -> (String, String) {
    serde_json::from_value::<LanRejectPayload>(payload)
        .map(|rejection| (rejection.reason, rejection.message))
        .unwrap_or_else(|_| {
            (
                REASON_PAIRING_USER_REJECTED.to_string(),
                "pairing rejected".to_string(),
            )
        })
}

enum TransferStreamEvent {
    Activity,
    Closed,
}

#[derive(Clone)]
pub struct LanManager {
    database: Database,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    inner: Arc<Mutex<LanState>>,
    swim_probe_lock: Arc<AsyncMutex<()>>,
}

struct LanState {
    generation: u64,
    active_device: Option<DeviceIdentity>,
    cancel: Option<watch::Sender<bool>>,
    discovery_refresh_tx: Option<mpsc::UnboundedSender<DiscoveryRefreshRequest>>,
    discovery_refresh_at: HashMap<String, i64>,
    high_priority_probe: bool,
    peers: HashMap<String, PeerEntry>,
    peer_endpoints: HashMap<String, (String, u16)>,
    peer_names: HashMap<String, String>,
    peer_types: HashMap<String, String>,
    members: HashMap<String, MemberRecord>,
    gossip: VecDeque<SwimGossip>,
    local_incarnation: i64,
    probe_queue: VecDeque<String>,
    probe_round_candidates: Vec<String>,
    probe_in_flight: HashSet<String>,
    seq: u64,
    transfer_tokens: HashMap<String, String>,
    transfer_senders: HashMap<String, mpsc::UnboundedSender<FileDataFrame>>,
    camera_tokens: HashMap<String, String>,
    camera_senders: HashMap<String, mpsc::Sender<CameraDataFrame>>,
    camera_receive_buffers: HashMap<String, CameraReceiveBuffer>,
    pending_pairings: HashMap<String, oneshot::Sender<bool>>,
    pairing_candidates: HashMap<String, LanPairingCandidate>,
    pair_strings: HashMap<String, PairStringRecord>,
}

#[derive(Debug, Clone)]
struct MemberRecord {
    state: MemberState,
    incarnation: i64,
    updated_at: i64,
    missed_probes: u8,
}

struct PeerConnection {
    connection_id: Uuid,
    sender: mpsc::UnboundedSender<PendingBusinessMessage>,
    initiated_by_local: bool,
    business_version: String,
}

struct PendingLanSend {
    message: BusinessEnvelope,
    envelope_id: Option<String>,
    correlation_id: Option<String>,
    result_tx: oneshot::Sender<AppResult<()>>,
}

struct PendingBusinessMessage {
    message: BusinessEnvelope,
    envelope_id: Option<String>,
    correlation_id: Option<String>,
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
    Camera { session_id: String },
}

struct HandshakeResult<S> {
    stream: WebSocketStream<S>,
    peer_device_id: String,
    crypto: LanSessionCrypto,
    business_version: String,
    outbound_seq: u64,
}

struct PeerProof {
    device_id: String,
    public_key: String,
    name: String,
}

struct PairingDecision {
    request_id: String,
    accepted: bool,
    reason: Option<String>,
    message: Option<String>,
}

struct PairingPrompt {
    request_id: String,
    response: oneshot::Receiver<bool>,
}

#[derive(Debug, Clone)]
struct PairStringRecord {
    device_id: String,
    public_key: String,
    expires_at: i64,
    state: PairStringState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairStringState {
    Active,
    Reserved,
    Consumed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairStringPayload {
    device_id: String,
    public_key: String,
    token: String,
    expires_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    platform: Option<String>,
}

struct DiscoveryRefreshRequest {
    label: String,
    completion: Option<oneshot::Sender<()>>,
}

struct PendingMdnsRebuild {
    reason: &'static str,
    deadline: Instant,
}

fn schedule_mdns_rebuild(
    pending_rebuild: &mut Option<PendingMdnsRebuild>,
    reason: &'static str,
) {
    schedule_mdns_rebuild_after(pending_rebuild, reason, MDNS_REBUILD_DEBOUNCE);
}

fn schedule_mdns_rebuild_after(
    pending_rebuild: &mut Option<PendingMdnsRebuild>,
    reason: &'static str,
    delay: Duration,
) {
    let deadline = Instant::now() + delay;
    match pending_rebuild {
        Some(pending) => {
            pending.reason = reason;
            pending.deadline = pending.deadline.max(deadline);
        }
        None => {
            *pending_rebuild = Some(PendingMdnsRebuild { reason, deadline });
        }
    }
}

#[cfg(target_os = "windows")]
struct SystemResumeNotification {
    registration: HPOWERNOTIFY,
    _recipient: Box<DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS>,
    _sender: Box<mpsc::UnboundedSender<()>>,
}

// The callback state is immutable after registration. Windows guarantees that
// PowerUnregisterSuspendResumeNotification stops callbacks before this owner drops it.
#[cfg(target_os = "windows")]
unsafe impl Send for SystemResumeNotification {}

#[cfg(target_os = "windows")]
impl SystemResumeNotification {
    fn register(sender: mpsc::UnboundedSender<()>) -> Option<Self> {
        let mut sender = Box::new(sender);
        let mut recipient = Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS {
            Callback: Some(system_resume_callback),
            Context: sender.as_mut() as *mut mpsc::UnboundedSender<()> as *mut c_void,
        });
        let mut registration = std::ptr::null_mut();
        let status = unsafe {
            PowerRegisterSuspendResumeNotification(
                DEVICE_NOTIFY_CALLBACK,
                HANDLE(recipient.as_mut() as *mut DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS as *mut c_void),
                &mut registration,
            )
        };
        let registration = HPOWERNOTIFY(registration as isize);
        if !status.is_ok() || registration.is_invalid() {
            warn!(error_code = status.0, "windows system resume notification registration failed");
            return None;
        }

        info!("windows system resume notification registered");
        Some(Self {
            registration,
            _recipient: recipient,
            _sender: sender,
        })
    }
}

#[cfg(target_os = "windows")]
impl Drop for SystemResumeNotification {
    fn drop(&mut self) {
        let status = unsafe { PowerUnregisterSuspendResumeNotification(self.registration) };
        if !status.is_ok() {
            warn!(error_code = status.0, "windows system resume notification unregistration failed");
        }
    }
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn system_resume_callback(
    context: *const c_void,
    event_type: u32,
    _setting: *const c_void,
) -> u32 {
    if event_type == PBT_APMRESUMEAUTOMATIC && !context.is_null() {
        let sender = unsafe { &*(context as *const mpsc::UnboundedSender<()>) };
        let _ = sender.send(());
    }
    0
}

#[derive(Debug, Clone)]
struct ParsedPairString {
    version: PairStringVersion,
    device_id: String,
    public_key: String,
    token: String,
    expires_at: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PairStringVersion {
    V1,
    V2,
}

#[derive(Debug, Clone, Copy)]
enum PairStringFailure {
    Invalid,
    Expired,
    Unavailable,
}

impl PairStringFailure {
    fn reason(self) -> &'static str {
        match self {
            Self::Invalid => REASON_PAIRING_PAIR_STRING_INVALID,
            Self::Expired => REASON_PAIRING_PAIR_STRING_EXPIRED,
            Self::Unavailable => REASON_PAIRING_PAIR_STRING_UNAVAILABLE,
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::Invalid => MESSAGE_PAIRING_PAIR_STRING_INVALID,
            Self::Expired => MESSAGE_PAIRING_PAIR_STRING_EXPIRED,
            Self::Unavailable => MESSAGE_PAIRING_PAIR_STRING_UNAVAILABLE,
        }
    }
}

#[derive(Default)]
struct CameraReceiveBuffer {
    frames: VecDeque<CameraDataFrame>,
    event_queued: bool,
    waiting_for_keyframe: bool,
}

impl CameraReceiveBuffer {
    fn push(&mut self, frame: CameraDataFrame) -> bool {
        if frame.codec == "h264" && self.waiting_for_keyframe {
            if !frame.keyframe {
                return false;
            }
            self.frames.clear();
            self.waiting_for_keyframe = false;
        }

        if self.frames.len() >= CAMERA_RECEIVE_BUFFER_CAPACITY {
            self.frames.clear();
            if frame.codec == "h264" && !frame.keyframe {
                self.waiting_for_keyframe = true;
                return false;
            }
        }

        self.frames.push_back(frame);
        true
    }

    fn take(&mut self) -> Vec<CameraDataFrame> {
        self.event_queued = false;
        self.frames.drain(..).collect()
    }
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
                discovery_refresh_tx: None,
                discovery_refresh_at: HashMap::new(),
                high_priority_probe: false,
                peers: HashMap::new(),
                peer_endpoints: HashMap::new(),
                peer_names: HashMap::new(),
                peer_types: HashMap::new(),
                members: HashMap::new(),
                gossip: VecDeque::new(),
                local_incarnation: 0,
                probe_queue: VecDeque::new(),
                probe_round_candidates: Vec::new(),
                probe_in_flight: HashSet::new(),
                seq: 0,
                transfer_tokens: HashMap::new(),
                transfer_senders: HashMap::new(),
                camera_tokens: HashMap::new(),
                camera_senders: HashMap::new(),
                camera_receive_buffers: HashMap::new(),
                pending_pairings: HashMap::new(),
                pairing_candidates: HashMap::new(),
                pair_strings: HashMap::new(),
            })),
            swim_probe_lock: Arc::new(AsyncMutex::new(())),
        }
    }

    pub fn start(&self) -> AppResult<()> {
        self.database
            .load_settings()?
            .ok_or_else(|| AppError::message(self.user_text(TextKey::SettingsNotInitialized)))?;
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
            inner.discovery_refresh_tx = None;
            inner.discovery_refresh_at.clear();
            inner.high_priority_probe = false;
            inner.peers.clear();
            inner.peer_endpoints.clear();
            inner.peer_names.clear();
            inner.peer_types.clear();
            inner.members.clear();
            inner.gossip.clear();
            inner.local_incarnation = context.incarnation;
            inner.probe_in_flight.clear();
            inner.transfer_tokens.clear();
            inner.transfer_senders.clear();
            inner.camera_tokens.clear();
            inner.camera_senders.clear();
            inner.camera_receive_buffers.clear();
            inner.pending_pairings.clear();
            inner.pairing_candidates.clear();
            inner.seq = 0;
            inner.probe_queue.clear();
            inner.probe_round_candidates.clear();
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
        let (peers, transfer_senders, camera_senders, pending) = {
            let mut inner = self.inner.lock_unpoisoned();
            if let Some(cancel) = inner.cancel.take() {
                let _ = cancel.send(true);
            }
            inner.generation += 1;
            inner.active_device = None;
            inner.discovery_refresh_tx = None;
            inner.discovery_refresh_at.clear();
            inner.high_priority_probe = false;
            inner.peer_endpoints.clear();
            inner.peer_names.clear();
            inner.peer_types.clear();
            inner.members.clear();
            inner.gossip.clear();
            inner.probe_in_flight.clear();
            inner.pairing_candidates.clear();
            inner.transfer_tokens.clear();
            inner.camera_tokens.clear();
            inner.camera_receive_buffers.clear();
            inner.pair_strings.clear();
            (
                std::mem::take(&mut inner.peers),
                std::mem::take(&mut inner.transfer_senders),
                std::mem::take(&mut inner.camera_senders),
                std::mem::take(&mut inner.pending_pairings),
            )
        };
        drop((peers, transfer_senders, camera_senders, pending));
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
                    .filter(|record| Self::is_trusted(record))
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

    pub fn trusted_member_types(&self) -> HashMap<String, String> {
        let trusted = self
            .database
            .load_trusted_peer_keys()
            .map(|records| {
                records
                    .into_iter()
                    .filter(|record| Self::is_trusted(record))
                    .map(|record| record.device_id)
                    .collect::<HashSet<_>>()
            })
            .unwrap_or_default();

        let inner = self.inner.lock_unpoisoned();
        inner
            .members
            .iter()
            .filter(|(_, record)| matches!(record.state, MemberState::Alive | MemberState::Suspect))
            .filter_map(|(device_id, _)| {
                if !trusted.contains(device_id) {
                    return None;
                }
                inner
                    .peer_types
                    .get(device_id)
                    .and_then(|value| normalized_peer_type(value))
                    .map(|device_type| (device_id.clone(), device_type))
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
            && self.is_lan_authorized(device_id)
            && self.peer_endpoint(device_id).is_some()
    }

    async fn refresh_discovery(&self) {
        let (completion_tx, completion_rx) = oneshot::channel();
        let refresh_tx = self.inner.lock_unpoisoned().discovery_refresh_tx.clone();
        let Some(refresh_tx) = refresh_tx else {
            return;
        };
        if refresh_tx
            .send(DiscoveryRefreshRequest {
                label: "manual".to_string(),
                completion: Some(completion_tx),
            })
            .is_err()
        {
            return;
        }
        let _ = completion_rx.await;
    }

    pub async fn refresh_for_device_list(&self) -> AppResult<()> {
        let ((), swim_result) = tokio::join!(
            self.refresh_discovery(),
            self.refresh_swim_for_device_list(),
        );
        swim_result
    }

    async fn refresh_swim_for_device_list(&self) -> AppResult<()> {
        let _probe_guard = self.swim_probe_lock.lock().await;
        let context = match self.load_context() {
            Ok(context) => context,
            Err(error) => {
                debug!(%error, "lan refresh skipped because the lan context is unavailable");
                return Ok(());
            }
        };
        let generation = self.current_generation();
        let targets = {
            let mut inner = self.inner.lock_unpoisoned();
            inner.high_priority_probe = true;
            inner
                .members
                .iter()
                .filter(|(device_id, member)| {
                    device_id.as_str() != context.device.device_id
                        && matches!(member.state, MemberState::Alive | MemberState::Suspect)
                        && inner.peer_endpoints.contains_key(*device_id)
                })
                .map(|(device_id, _)| device_id.clone())
                .collect::<Vec<_>>()
        };

        let mut probes = FuturesUnordered::new();
        for target in targets {
            let manager = self.clone();
            let context = context.clone();
            probes.push(async move {
                manager
                    .probe_member(generation, context, target)
                    .await;
            });
        }
        while probes.next().await.is_some() {}

        let mut inner = self.inner.lock_unpoisoned();
        if inner.generation == generation {
            inner.high_priority_probe = false;
        }
        Ok(())
    }

    pub fn peer_business_version(&self, device_id: &str) -> Option<String> {
        self.inner
            .lock_unpoisoned()
            .peers
            .get(device_id)
            .and_then(|entry| match entry {
                PeerEntry::Connected(peer) => Some(peer.business_version.clone()),
                PeerEntry::Connecting(_) => None,
            })
    }

    pub async fn send(
        &self,
        device_id: &str,
        message: BusinessEnvelope,
        envelope_id: Option<String>,
        correlation_id: Option<String>,
    ) -> AppResult<()> {
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

        let mut outbound = PendingBusinessMessage {
            message,
            envelope_id,
            correlation_id,
        };
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
                        match sender.send(outbound) {
                            Ok(()) => return Ok(()),
                            Err(error) => {
                                outbound = error.0;
                                self.remove_stale_peer_sender(device_id, &sender);
                                continue;
                            }
                        }
                    }
                    Some(PeerEntry::Connecting(queue)) => {
                        let (tx, rx) = oneshot::channel();
                        queue.push_back(PendingLanSend {
                            message: outbound.message,
                            envelope_id: outbound.envelope_id.clone(),
                            correlation_id: outbound.correlation_id.clone(),
                            result_tx: tx,
                        });
                        rx
                    }
                    None => {
                        let (tx, rx) = oneshot::channel();
                        let mut queue = VecDeque::new();
                        queue.push_back(PendingLanSend {
                            message: outbound.message,
                            envelope_id: outbound.envelope_id.clone(),
                            correlation_id: outbound.correlation_id.clone(),
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

    pub fn peer_endpoints(&self) -> HashMap<String, (String, u16)> {
        self.inner.lock_unpoisoned().peer_endpoints.clone()
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
        self.database.clear_lan_pairing(device_id)?;
        self.detach_peer(self.current_generation(), device_id);
        self.refresh_pairing_candidate(device_id);
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
        self.detach_peer(generation, device_id);
        let manager = self.clone();
        let device_id = device_id.to_string();
        tauri::async_runtime::spawn(async move {
            let _ = manager
                .connect_outbound(generation, context, device_id, ip, port, true)
                .await;
        });
        Ok(())
    }

    pub fn create_pair_string(&self, legacy: bool) -> AppResult<String> {
        let context = self.load_context()?;
        let now = unix_now_millis();
        let expires_at = now + PAIR_STRING_RECOMMENDED_TTL_MILLIS;
        let mut token_bytes = [0_u8; 32];
        OsRng.fill_bytes(&mut token_bytes);
        let token = URL_SAFE_NO_PAD.encode(token_bytes);
        let pair_string = if legacy {
            let payload = PairStringPayload {
                device_id: context.device.device_id.clone(),
                public_key: context.device.public_key.clone(),
                token: token.clone(),
                expires_at,
                name: Some(context.device.name.clone()),
                platform: Some(context.device.device_type.clone()),
            };
            let encoded = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&payload)?);
            format!("colink://pair/v1?data={encoded}")
        } else {
            let device_id = Uuid::parse_str(&context.device.device_id)
                .map_err(|_| AppError::message("Device identity is invalid"))?;
            let public_key = STANDARD
                .decode(&context.device.public_key)
                .map_err(|_| AppError::message("Device identity is invalid"))?;
            if public_key.len() != 32 {
                return Err(AppError::message("Device identity is invalid"));
            }
            let mut payload = Vec::with_capacity(80);
            payload.extend_from_slice(device_id.as_bytes());
            payload.extend_from_slice(&public_key);
            payload.extend_from_slice(&token_bytes);
            format!("colink://pair/v2?d={}", URL_SAFE_NO_PAD.encode(payload))
        };
        let mut inner = self.inner.lock_unpoisoned();
        inner.pair_strings.retain(|_, record| {
            record.expires_at > now
                && matches!(record.state, PairStringState::Active | PairStringState::Reserved)
        });
        inner.pair_strings.insert(
            token,
            PairStringRecord {
                device_id: context.device.device_id,
                public_key: context.device.public_key,
                expires_at,
                state: PairStringState::Active,
            },
        );
        Ok(pair_string)
    }

    fn reserve_pair_string(
        &self,
        value: &str,
        context: &LanContext,
    ) -> Result<String, PairStringFailure> {
        let payload = parse_pair_string(value)?;
        let now = unix_now_millis();
        let mut inner = self.inner.lock_unpoisoned();
        let Some(record) = inner.pair_strings.get_mut(&payload.token) else {
            return Err(PairStringFailure::Invalid);
        };
        if record.expires_at <= now || payload.expires_at.is_some_and(|expires_at| expires_at <= now) {
            record.state = PairStringState::Cancelled;
            return Err(PairStringFailure::Expired);
        }
        if record.device_id != context.device.device_id
            || payload.device_id != context.device.device_id
            || !same_public_key(&record.public_key, &payload.public_key)
            || !same_public_key(&context.device.public_key, &payload.public_key)
            || payload.expires_at.is_some_and(|expires_at| record.expires_at != expires_at)
        {
            return Err(PairStringFailure::Invalid);
        }
        if record.state != PairStringState::Active {
            return Err(PairStringFailure::Unavailable);
        }
        record.state = PairStringState::Reserved;
        Ok(payload.token)
    }

    fn consume_pair_string(&self, token: &str) {
        if let Some(record) = self.inner.lock_unpoisoned().pair_strings.get_mut(token) {
            if record.state == PairStringState::Reserved {
                record.state = PairStringState::Consumed;
            }
        }
    }

    fn cancel_pair_string(&self, token: &str) {
        if let Some(record) = self.inner.lock_unpoisoned().pair_strings.get_mut(token) {
            if record.state == PairStringState::Reserved {
                record.state = PairStringState::Cancelled;
            }
        }
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

    pub fn register_camera_token(&self, session_id: &str, token: &str) {
        self.inner
            .lock_unpoisoned()
            .camera_tokens
            .insert(session_id.to_string(), token.to_string());
    }

    pub fn unregister_camera(&self, session_id: &str) {
        let sender = {
            let mut inner = self.inner.lock_unpoisoned();
            inner.camera_tokens.remove(session_id);
            inner.camera_receive_buffers.remove(session_id);
            inner.camera_senders.remove(session_id)
        };
        drop(sender);
    }

    pub fn take_camera_frames(&self, session_id: &str) -> Vec<CameraDataFrame> {
        self.inner
            .lock_unpoisoned()
            .camera_receive_buffers
            .get_mut(session_id)
            .map(CameraReceiveBuffer::take)
            .unwrap_or_default()
    }

    pub fn has_camera_connection(&self, session_id: &str) -> bool {
        self.inner
            .lock_unpoisoned()
            .camera_senders
            .contains_key(session_id)
    }

    pub fn send_camera_frame(&self, session_id: &str, frame: CameraDataFrame) -> AppResult<()> {
        let sender = self
            .inner
            .lock_unpoisoned()
            .camera_senders
            .get(session_id)
            .cloned()
            .ok_or_else(|| AppError::message("LAN camera connection does not exist"))?;
        // Bounded non-blocking send keeps capture off the WebSocket write path.
        // On congestion the host forces a keyframe so the controller can resync cleanly.
        sender
            .try_send(frame)
            .map_err(|error| match error {
                tokio::sync::mpsc::error::TrySendError::Full(_) => {
                    AppError::message("LAN camera connection is congested")
                }
                tokio::sync::mpsc::error::TrySendError::Closed(_) => {
                    AppError::message("LAN camera connection does not exist")
                }
            })
    }

    pub async fn connect_camera(
        &self,
        session_id: &str,
        token: &str,
        ip: &str,
        port: u16,
    ) -> AppResult<()> {
        let url = Url::parse(&format!(
            "ws://{ip}:{port}/camera-stream/{session_id}?token={token}"
        ))?;
        let (stream, _) = connect_async(url.as_str())
            .await
            .map_err(|error| AppError::message(error.to_string()))?;
        self.attach_camera_stream(session_id.to_string(), stream).await
    }

    async fn run(
        &self,
        generation: u64,
        context: LanContext,
        mut cancel_rx: watch::Receiver<bool>,
    ) {
        let (listener, port) = match bind_lan_listener().await {
            Ok(value) => value,
            Err(error) => {
                warn!(preferred_port = LAN_PORT, %error, "lan listener bind failed");
                self.finalize_generation(generation);
                return;
            }
        };
        info!(port, preferred_port = LAN_PORT, "lan listener bound");

        let (mut mdns, mut browse_rx, mut monitor_rx) =
            match self.start_mdns(generation, &context, port) {
                Ok(runtime) => runtime,
                Err(error) => {
                    warn!(%error, "mdns daemon initialization failed");
                    self.finalize_generation(generation);
                    return;
                }
            };
        let (discovery_refresh_tx, mut discovery_refresh_rx) = mpsc::unbounded_channel();
        {
            let mut inner = self.inner.lock_unpoisoned();
            if inner.generation != generation {
                return;
            }
            inner.discovery_refresh_tx = Some(discovery_refresh_tx);
        }
        let mut swim_interval = interval(SWIM_PERIOD);
        swim_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut suspect_interval = interval(Duration::from_millis(500));
        suspect_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut mdns_rebuild_interval = interval(Duration::from_millis(250));
        mdns_rebuild_interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let (system_resume_tx, mut system_resume_rx) = mpsc::unbounded_channel();
        #[cfg(target_os = "windows")]
        let _system_resume_notification = SystemResumeNotification::register(system_resume_tx);
        #[cfg(not(target_os = "windows"))]
        let _system_resume_tx = system_resume_tx;
        let mut pending_mdns_rebuild = None;
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
                request = discovery_refresh_rx.recv() => {
                    let Some(request) = request else {
                        continue;
                    };
                    debug!(label = %request.label, "refreshing mdns browse");
                    let completion = request.completion;
                    if let Err(error) = mdns.stop_browse(SERVICE_TYPE) {
                        warn!(%error, "mdns browse refresh stop failed");
                        if let Some(completion) = completion {
                            let _ = completion.send(());
                        }
                        continue;
                    }
                    match mdns.browse(SERVICE_TYPE) {
                        Ok(next_browse_rx) => browse_rx = next_browse_rx,
                        Err(error) => warn!(%error, "mdns browse refresh start failed"),
                    }
                    if let Some(completion) = completion {
                        let _ = completion.send(());
                    }
                }
                _ = swim_interval.tick() => {
                    self.schedule_probe_next_member(generation, context.clone());
                }
                _ = suspect_interval.tick() => {
                    self.promote_expired_suspects(generation);
                }
                Some(()) = system_resume_rx.recv() => {
                    info!(delay_secs = SYSTEM_RESUME_REBUILD_DELAY.as_secs(), "windows system resume detected; scheduling mdns rebuild");
                    schedule_mdns_rebuild_after(
                        &mut pending_mdns_rebuild,
                        "system_resume",
                        SYSTEM_RESUME_REBUILD_DELAY,
                    );
                }
                _ = mdns_rebuild_interval.tick(), if pending_mdns_rebuild.is_some() => {
                    let Some(pending) = pending_mdns_rebuild.take() else {
                        continue;
                    };
                    if Instant::now() < pending.deadline {
                        pending_mdns_rebuild = Some(pending);
                        continue;
                    }

                    self.shutdown_mdns(&mdns, generation, pending.reason).await;
                    match self.start_mdns(generation, &context, port) {
                        Ok((next_mdns, next_browse_rx, next_monitor_rx)) => {
                            mdns = next_mdns;
                            browse_rx = next_browse_rx;
                            monitor_rx = next_monitor_rx;
                            info!(reason = pending.reason, generation, "mdns daemon rebuilt");
                        }
                        Err(error) => {
                            warn!(reason = pending.reason, generation, %error, "mdns daemon rebuild failed");
                            self.finalize_generation(generation);
                            return;
                        }
                    }
                }
                event = recv_monitor_event(&monitor_rx), if monitor_rx.is_some() => {
                    if let Some(event) = event {
                        match event {
                            DaemonEvent::IpAdd(ip) => {
                                info!(%ip, "mdns network address added; scheduling daemon rebuild");
                                schedule_mdns_rebuild(&mut pending_mdns_rebuild, "network_change");
                            }
                            DaemonEvent::IpDel(ip) => {
                                info!(%ip, "mdns network address removed; scheduling daemon rebuild");
                                schedule_mdns_rebuild(&mut pending_mdns_rebuild, "network_change");
                            }
                            DaemonEvent::Announce(service, interface) => {
                                info!(%service, %interface, "mdns service announced");
                            }
                            DaemonEvent::Error(error) => {
                                warn!(%error, "mdns daemon error");
                            }
                            DaemonEvent::NameChange(change) => {
                                warn!(
                                    original = %change.original,
                                    new_name = %change.new_name,
                                    interface = %change.intf_name,
                                    "mdns name conflict resolved"
                                );
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        self.shutdown_mdns(&mdns, generation, "lan_manager_stop").await;
        self.finalize_generation(generation);
        self.clear_peers_for_generation(generation);
        info!(generation = generation, "lan discovery loop stopped");
    }

    fn start_mdns(
        &self,
        generation: u64,
        context: &LanContext,
        port: u16,
    ) -> AppResult<(
        ServiceDaemon,
        mdns_sd::Receiver<ServiceEvent>,
        Option<mdns_sd::Receiver<DaemonEvent>>,
    )> {
        let mdns = ServiceDaemon::new().map_err(|error| AppError::message(error.to_string()))?;
        mdns.set_ip_check_interval(5)
            .map_err(|error| AppError::message(error.to_string()))?;
        let browse_rx = mdns
            .browse(SERVICE_TYPE)
            .map_err(|error| AppError::message(error.to_string()))?;
        let monitor_rx = match mdns.monitor() {
            Ok(receiver) => Some(receiver),
            Err(error) => {
                warn!(%error, "mdns monitor failed to start");
                None
            }
        };

        info!(
            generation,
            mdns_version = MDNS_SD_VERSION,
            mdns_port = MDNS_PORT,
            service_port = port,
            "mdns daemon created"
        );
        self.register_mdns_service(&mdns, context, port);
        Ok((mdns, browse_rx, monitor_rx))
    }

    async fn shutdown_mdns(&self, mdns: &ServiceDaemon, generation: u64, reason: &str) {
        info!(generation, reason, "mdns daemon closing");
        match mdns.shutdown() {
            Ok(status_rx) => match status_rx.recv_async().await {
                Ok(DaemonStatus::Shutdown) => info!(generation, reason, "mdns daemon closed"),
                Ok(status) => warn!(generation, reason, ?status, "unexpected mdns daemon shutdown status"),
                Err(error) => warn!(generation, reason, %error, "mdns daemon shutdown status unavailable"),
            },
            Err(error) => warn!(generation, reason, %error, "mdns daemon shutdown failed"),
        }
    }

    fn register_service(
        &self,
        mdns: &ServiceDaemon,
        context: &LanContext,
        port: u16,
    ) -> AppResult<()> {
        let hostname = hostname::get()
            .ok()
            .and_then(|value| value.into_string().ok())
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| "colink-desktop".to_string());
        let instance_name = format!("colink-{}", &context.device.device_id[..8]);
        let name = context.device.name.trim();
        let mut properties = vec![
            ("deviceId", context.device.device_id.as_str()),
            ("version", "1"),
        ];
        let device_type = context.device.device_type.trim();
        if !device_type.is_empty() {
            properties.push(("type", device_type));
        }
        if !name.is_empty() && name.len() <= 200 {
            properties.push(("name", name));
        }
        let info = ServiceInfo::new(
            SERVICE_TYPE,
            &instance_name,
            &format!("{hostname}.local."),
            "",
            port,
            &properties[..],
        )
        .map_err(|error| AppError::message(error.to_string()))?;
        let mut info = info.enable_addr_auto();
        info.set_interfaces(vec![IfKind::IPv4]);
        info!(
            port,
            "registering mdns service on ipv4 interfaces"
        );
        mdns.register(info)
            .map_err(|error| AppError::message(error.to_string()))
    }

    fn register_mdns_service(&self, mdns: &ServiceDaemon, context: &LanContext, port: u16) {
        match self.register_service(mdns, context, port) {
            Ok(()) => info!(port, device_id = %context.device.device_id, "mdns service registration requested"),
            Err(error) => {
                warn!(port, device_id = %context.device.device_id, %error, "mdns service registration failed");
            }
        }
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
        let name = service
            .get_property_val_str("name")
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let device_type = service
            .get_property_val_str("type")
            .and_then(normalized_peer_type);

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
        let endpoint_changed = self.peer_endpoint(&device_id)
            != Some((ip.to_string(), port));
        let should_probe = endpoint_changed || !self.is_swim_alive(&device_id);
        self.remember_peer_endpoint(&device_id, ip, port);
        if let Some(name) = name {
            self.remember_peer_name(&device_id, name);
        }
        if let Some(device_type) = device_type {
            self.remember_peer_type(&device_id, device_type);
        }
        let _ = self.event_tx.send(RuntimeEvent::LanDiscovered {
            device_id: device_id.clone(),
            ip: ip.to_string(),
            port,
            source: "mdns".to_string(),
        });

        if !should_probe {
            return;
        }

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
                    if ack.is_target_ack(&device_id) {
                        manager.process_swim_message(generation, &context, ack);
                    } else {
                        warn!(
                            device_id = %device_id,
                            from = %ack.payload.from,
                            "ignored mdns-triggered swim ack from different device"
                        );
                    }
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
            InboundRoute::Camera { session_id } => {
                debug!(%session_id, "handling inbound lan camera websocket");
                self.attach_camera_stream(session_id, stream).await
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
        sender: &mpsc::UnboundedSender<PendingBusinessMessage>,
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
        let (tx, mut rx) = mpsc::unbounded_channel::<PendingBusinessMessage>();
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
                    business_version: session.business_version.clone(),
                }),
            );
            inner.pairing_candidates.remove(&peer_device_id);
            (pending, was_connected)
        };

        for pending in pending {
            let result = tx
                .send(PendingBusinessMessage {
                    message: pending.message,
                    envelope_id: pending.envelope_id,
                    correlation_id: pending.correlation_id,
                })
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
        let local_device_id = self
            .load_context()
            .map(|context| context.device.device_id)
            .unwrap_or_default();
        tauri::async_runtime::spawn(async move {
            let (mut writer, mut reader) = session.stream.split();
            let mut crypto = session.crypto;
            let mut last_application_activity = Instant::now();
            let mut heartbeat_interval =
                tokio::time::interval(Duration::from_secs(HEARTBEAT_INTERVAL_SECS));
            heartbeat_interval.set_missed_tick_behavior(MissedTickBehavior::Delay);
            heartbeat_interval.tick().await;
            let mut pending_heartbeats = HashSet::<String>::new();
            let mut outbound_seq = session.outbound_seq;
            let mut failed_outbound = None;
            loop {
                if last_application_activity.elapsed()
                    >= Duration::from_secs(KEEPALIVE_TIMEOUT_SECS)
                {
                    break;
                }
                tokio::select! {
                    outbound = rx.recv() => {
                        let Some(outbound) = outbound else {
                            break;
                        };
                        let message = outbound.message;
                        let envelope_id = outbound.envelope_id;
                        let correlation_id = outbound.correlation_id;
                        let encrypted = match crypto.encrypt(&message) {
                            Ok(payload) => payload,
                            Err(_) => {
                                failed_outbound = Some(CorrelatedBusinessMessage {
                                    message,
                                    envelope_id,
                                    correlation_id,
                                });
                                break;
                            }
                        };
                        let envelope = LanEnvelope {
                            id: envelope_id.clone().unwrap_or_else(|| Uuid::new_v4().to_string()),
                            message_type: "business.v1.message".to_string(),
                            from: local_device_id.clone(),
                            to: peer_device_id.clone(),
                            seq: next_lan_seq(&mut outbound_seq),
                            timestamp: unix_now_millis(),
                            correlation_id: correlation_id.clone(),
                            payload: match serde_json::to_value(encrypted) {
                                Ok(value) => value,
                                Err(_) => {
                                    failed_outbound = Some(CorrelatedBusinessMessage {
                                        message,
                                        envelope_id,
                                        correlation_id,
                                    });
                                    break;
                                }
                            },
                        };
                        let text = match serde_json::to_string(&envelope) {
                            Ok(text) => text,
                            Err(_) => {
                                failed_outbound = Some(CorrelatedBusinessMessage {
                                    message,
                                    envelope_id,
                                    correlation_id,
                                });
                                break;
                            }
                        };
                        if writer.send(Message::Text(text.into())).await.is_err() {
                            failed_outbound = Some(CorrelatedBusinessMessage {
                                message,
                                envelope_id,
                                correlation_id,
                            });
                            break;
                        }
                    }
                    _ = heartbeat_interval.tick() => {
                        let heartbeat_id = Uuid::new_v4().to_string();
                        let envelope = LanEnvelope {
                            id: heartbeat_id.clone(),
                            message_type: "heartbeat.v1.ping".to_string(),
                            from: local_device_id.clone(),
                            to: peer_device_id.clone(),
                            seq: next_lan_seq(&mut outbound_seq),
                            timestamp: unix_now_millis(),
                            correlation_id: None,
                            payload: serde_json::json!({}),
                        };
                        let Ok(text) = serde_json::to_string(&envelope) else {
                            break;
                        };
                        pending_heartbeats.insert(heartbeat_id);
                        if writer.send(Message::Text(text.into())).await.is_err() {
                            break;
                        }
                    }
                    inbound = reader.next() => {
                        match inbound {
                            Some(Ok(Message::Text(text))) => {
                                let Ok(envelope) = serde_json::from_str::<LanEnvelope>(&text) else {
                                    continue;
                                };
                                if envelope.from != peer_device_id || envelope.to != local_device_id {
                                    continue;
                                }
                                match envelope.message_type.as_str() {
                                    "heartbeat.v1.ping" => {
                                        last_application_activity = Instant::now();
                                        let pong = LanEnvelope {
                                            id: Uuid::new_v4().to_string(),
                                            message_type: "heartbeat.v1.pong".to_string(),
                                            from: local_device_id.clone(),
                                            to: peer_device_id.clone(),
                                            seq: next_lan_seq(&mut outbound_seq),
                                            timestamp: unix_now_millis(),
                                            correlation_id: Some(envelope.id),
                                            payload: serde_json::json!({}),
                                        };
                                        let Ok(text) = serde_json::to_string(&pong) else {
                                            break;
                                        };
                                        if writer.send(Message::Text(text.into())).await.is_err() {
                                            break;
                                        }
                                        continue;
                                    }
                                    "heartbeat.v1.pong" => {
                                        if envelope
                                            .correlation_id
                                            .as_ref()
                                            .is_some_and(|id| pending_heartbeats.remove(id))
                                        {
                                            last_application_activity = Instant::now();
                                        }
                                        continue;
                                    }
                                    "business.v1.message" => {
                                        last_application_activity = Instant::now();
                                    }
                                    _ => {
                                        last_application_activity = Instant::now();
                                        continue;
                                    }
                                }
                                let Ok(payload) = serde_json::from_value::<EncryptedBusinessPayload>(envelope.payload) else {
                                    continue;
                                };
                                match crypto.decrypt(&payload) {
                                    Ok(message) => {
                                        last_application_activity = Instant::now();
                                        let _ = manager.event_tx.send(RuntimeEvent::LanMessage {
                                            from: peer_device_id.clone(),
                                            envelope_id: envelope.id,
                                            correlation_id: envelope.correlation_id,
                                            message,
                                        });
                                    }
                                    Err(_) => continue,
                                }
                            }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            Some(Ok(_)) => {}
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
                undelivered.push(CorrelatedBusinessMessage {
                    message: message.message,
                    envelope_id: message.envelope_id,
                    correlation_id: message.correlation_id,
                });
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

    async fn attach_camera_stream<S>(
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
        let (tx, mut rx) = mpsc::channel::<CameraDataFrame>(CAMERA_SEND_BUFFER_CAPACITY);
        self.inner
            .lock_unpoisoned()
            .camera_senders
            .insert(session_id.clone(), tx);
        info!(
            %session_id,
            send_capacity = CAMERA_SEND_BUFFER_CAPACITY,
            receive_capacity = CAMERA_RECEIVE_BUFFER_CAPACITY,
            "LAN camera data stream attached"
        );
        let _ = self.event_tx.send(RuntimeEvent::LanCameraConnected {
            session_id: session_id.clone(),
        });
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let (mut writer, mut reader) = stream.split();
            loop {
                tokio::select! {
                    outbound = rx.recv() => {
                        let Some(outbound) = outbound else { break; };
                        if writer.send(Message::Binary(outbound.encode().into())).await.is_err() {
                            break;
                        }
                    }
                    inbound = reader.next() => {
                        match inbound {
                            Some(Ok(Message::Binary(bytes))) => {
                                if let Some(frame) = CameraDataFrame::decode(bytes.as_ref()) {
                                    manager.queue_camera_frame(&session_id, frame);
                                }
                            }
                            Some(Ok(Message::Ping(payload))) => {
                                if writer.send(Message::Pong(payload)).await.is_err() {
                                    break;
                                }
                            }
                            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break,
                            Some(Ok(_)) => {}
                        }
                    }
                }
            }
            manager.unregister_camera(&session_id);
            info!(%session_id, "LAN camera data stream detached");
            let _ = manager.event_tx.send(RuntimeEvent::LanCameraClosed { session_id });
        });
        Ok(())
    }

    fn queue_camera_frame(&self, session_id: &str, frame: CameraDataFrame) {
        let should_notify = {
            let mut inner = self.inner.lock_unpoisoned();
            let buffer = inner
                .camera_receive_buffers
                .entry(session_id.to_string())
                .or_default();
            if !buffer.push(frame) || buffer.event_queued {
                false
            } else {
                buffer.event_queued = true;
                true
            }
        };
        if should_notify {
            let _ = self.event_tx.send(RuntimeEvent::LanCameraFramesReady {
                session_id: session_id.to_string(),
            });
        }
    }

    async fn handle_swim_message(
        &self,
        generation: u64,
        context: &LanContext,
        message: SwimEnvelope,
        remote_addr: SocketAddr,
    ) -> AppResult<SwimEnvelope> {
        if message.payload.from != context.device.device_id {
            self.request_mdns_refresh(&message.payload.from, remote_addr.ip());
        }

        match message.message_type.as_str() {
            "swim.ping" => {
                let seq = message.payload.seq;
                self.process_swim_message(generation, context, message);
                Ok(self.swim_ack(context, seq))
            }
            "swim.ping-req" => {
                self.process_swim_message(generation, context, message.clone());
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
                if !ack.is_target_ack(&target) {
                    warn!(
                        target = %target,
                        from = %ack.payload.from,
                        "ping-req target identity mismatch"
                    );
                    return Err(AppError::message("swim target identity mismatch"));
                }
                self.process_swim_message(generation, context, ack.clone());
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
        self.observe_swim_alive(
            generation,
            context,
            &message.payload.from,
            message.payload.incarnation,
        );
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
                self.push_self_alive(context, entry.incarnation);
                continue;
            }
            self.merge_member(generation, context, &message.payload.from, entry);
        }
    }

    fn observe_swim_alive(
        &self,
        generation: u64,
        context: &LanContext,
        device_id: &str,
        incarnation: Option<i64>,
    ) {
        if device_id == context.device.device_id {
            return;
        }
        if incarnation.is_some_and(|value| value > unix_now_millis() + 5 * 60 * 1000) {
            return;
        }
        debug!(%device_id, "observed swim peer alive");
        self.clear_probe_misses(generation, device_id);
        self.mark_member(
            generation,
            context,
            device_id,
            MemberState::Alive,
            incarnation,
        );
    }

    fn record_probe_miss(&self, generation: u64, device_id: &str) -> u8 {
        let mut inner = self.inner.lock_unpoisoned();
        if inner.generation != generation {
            return 0;
        }
        let Some(member) = inner.members.get_mut(device_id) else {
            return 0;
        };
        member.missed_probes = member.missed_probes.saturating_add(1);
        member.missed_probes
    }

    fn clear_probe_misses(&self, generation: u64, device_id: &str) {
        let mut inner = self.inner.lock_unpoisoned();
        if inner.generation != generation {
            return;
        }
        if let Some(member) = inner.members.get_mut(device_id) {
            member.missed_probes = 0;
        }
    }

    async fn send_swim_ping(&self, context: &LanContext, target: &str) -> AppResult<SwimEnvelope> {
        let message = SwimEnvelope {
            message_type: "swim.ping".to_string(),
            payload: SwimPayload {
                seq: self.next_seq(),
                from: context.device.device_id.clone(),
                incarnation: Some(self.local_incarnation(context)),
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
                incarnation: Some(self.local_incarnation(context)),
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
            return Err(AppError::message(format!(
                "SWIM request failed: {}",
                response.status()
            )));
        }
        Ok(response.json::<SwimEnvelope>().await?)
    }

    fn schedule_probe_next_member(&self, generation: u64, context: LanContext) {
        let manager = self.clone();
        tauri::async_runtime::spawn(async move {
            let _probe_guard = manager.swim_probe_lock.lock().await;
            let targets = manager.next_probe_targets(&context.device.device_id);
            if targets.is_empty() {
                return;
            }
            for target in targets {
                manager
                    .probe_member(generation, context.clone(), target.clone())
                    .await;
                manager.finish_probe(generation, &target);
            }
        });
    }

    async fn probe_member(&self, generation: u64, context: LanContext, target: String) {
        debug!(%target, "probing swim member");
        match self.send_swim_ping(&context, &target).await {
            Ok(ack) => {
                let from = ack.payload.from.clone();
                self.process_swim_message(generation, &context, ack.clone());
                if ack.is_target_ack(&target) {
                    return;
                }
                warn!(%target, %from, "direct swim probe identity mismatch");
            }
            Err(error) => {
                debug!(%target, %error, "direct swim probe failed");
            }
        }

        let mut ping_reqs = FuturesUnordered::new();
        for intermediary in self.indirect_targets(&context.device.device_id, &target) {
            ping_reqs.push(async {
                let result = self
                    .send_swim_ping_req(&context, &intermediary, &target)
                    .await;
                (intermediary, result)
            });
        }
        while let Some((intermediary, result)) = ping_reqs.next().await {
            match result {
                Ok(ack) => {
                    let from = ack.payload.from.clone();
                    if ack.is_target_ack(&target) {
                        self.process_swim_message(generation, &context, ack);
                        return;
                    }
                    warn!(%target, %intermediary, %from, "indirect swim probe identity mismatch");
                }
                Err(error) => {
                    debug!(%target, %intermediary, %error, "indirect swim probe failed");
                }
            }
        }

        let missed_probes = self.record_probe_miss(generation, &target);
        if missed_probes < SWIM_SUSPECT_MISSES {
            warn!(
                %target,
                missed_probes,
                threshold = SWIM_SUSPECT_MISSES,
                "swim probe missed; keeping member alive"
            );
            return;
        }
        self.mark_member(generation, &context, &target, MemberState::Suspect, None);
        warn!(%target, missed_probes, "swim member marked suspect");
    }

    fn next_probe_targets(&self, local_device_id: &str) -> Vec<String> {
        let mut inner = self.inner.lock_unpoisoned();
        if inner.high_priority_probe {
            return Vec::new();
        }
        if !inner.probe_in_flight.is_empty() {
            return Vec::new();
        }

        let mut candidates = inner
            .members
            .iter()
            .filter(|(device_id, member)| {
                device_id.as_str() != local_device_id
                    && matches!(member.state, MemberState::Alive | MemberState::Suspect)
                    && inner.peer_endpoints.contains_key(*device_id)
            })
            .map(|(device_id, _)| device_id.clone())
            .collect::<Vec<_>>();
        candidates.sort();
        if candidates.is_empty() {
            inner.probe_queue.clear();
            inner.probe_round_candidates.clear();
            return Vec::new();
        }
        let target_set = candidates.iter().cloned().collect::<HashSet<_>>();
        if inner.probe_queue.is_empty() || inner.probe_round_candidates != candidates {
            inner.probe_round_candidates = candidates.clone();
            inner.probe_queue = shuffled_probe_queue(candidates);
        }
        let mut targets = Vec::new();
        while targets.len() < SWIM_PROBE_BATCH_SIZE {
            let Some(target) = inner.probe_queue.pop_front() else {
                break;
            };
            if target_set.contains(&target) {
                inner.probe_in_flight.insert(target.clone());
                targets.push(target);
            }
        }
        if targets.is_empty() {
            inner.probe_round_candidates.clear();
        }
        targets
    }

    fn finish_probe(&self, generation: u64, target: &str) {
        let mut inner = self.inner.lock_unpoisoned();
        if inner.generation == generation {
            inner.probe_in_flight.remove(target);
        }
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
                let missed_probes = if state == MemberState::Alive {
                    0
                } else {
                    existing
                        .as_ref()
                        .map(|member| member.missed_probes)
                        .unwrap_or(0)
                };
                inner.members.insert(
                    device_id.to_string(),
                    MemberRecord {
                        state,
                        incarnation: next_incarnation,
                        updated_at: now,
                        missed_probes,
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
                if self.is_lan_authorized(device_id) {
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
                if self.is_lan_authorized(device_id) {
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
                incarnation: Some(self.local_incarnation(context)),
                target: None,
                gossip: self.gossip_batch(),
            },
        }
    }

    async fn broadcast_left(&self, context: &LanContext) {
        let incarnation = [unix_now_millis(), self.local_incarnation(context) + 1]
            .into_iter()
            .max()
            .unwrap_or_else(unix_now_millis);
        self.set_local_incarnation(incarnation);
        let entry = SwimGossip {
            device_id: context.device.device_id.clone(),
            state: MemberState::Left.as_str().to_string(),
            incarnation,
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

    fn push_self_alive(&self, context: &LanContext, observed_suspicion_incarnation: i64) {
        let incarnation = [
            unix_now_millis(),
            self.local_incarnation(context) + 1,
            observed_suspicion_incarnation + 1,
        ]
        .into_iter()
        .max()
        .unwrap_or_else(unix_now_millis);
        self.set_local_incarnation(incarnation);
        self.push_gossip(SwimGossip {
            device_id: context.device.device_id.clone(),
            state: MemberState::Alive.as_str().to_string(),
            incarnation,
        });
    }

    fn local_incarnation(&self, context: &LanContext) -> i64 {
        self.inner
            .lock_unpoisoned()
            .local_incarnation
            .max(context.incarnation)
    }

    fn set_local_incarnation(&self, incarnation: i64) {
        let mut inner = self.inner.lock_unpoisoned();
        inner.local_incarnation = inner.local_incarnation.max(incarnation);
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

    fn is_lan_authorized(&self, device_id: &str) -> bool {
        self.database
            .load_trusted_peer_keys()
            .map(|records| {
                records
                    .iter()
                    .any(|record| record.device_id == device_id && Self::is_trusted(record))
            })
            .unwrap_or(false)
    }

    fn is_lan_trusted(&self, device_id: &str) -> bool {
        self.database
            .load_trusted_peer_keys()
            .map(|records| {
                records
                    .iter()
                    .any(|record| record.device_id == device_id && record.trusted_by_lan)
            })
            .unwrap_or(false)
    }

    fn is_trusted(record: &TrustedPeerKeyRecord) -> bool {
        record.trusted_by_lan || record.trusted_by_cloud
    }

    fn open_pairing_prompt(
        &self,
        device_id: &str,
        name: &str,
        public_key: &str,
        code: &str,
        reason: &str,
        initiated_locally: bool,
    ) -> PairingPrompt {
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
                initiated_locally,
            }));
        PairingPrompt {
            request_id,
            response: rx,
        }
    }

    fn finish_pairing_prompt(&self, request_id: &str) {
        self.inner.lock_unpoisoned().pending_pairings.remove(request_id);
    }

    async fn request_pairing(
        &self,
        device_id: &str,
        name: &str,
        public_key: &str,
        code: &str,
        reason: &str,
    ) -> PairingDecision {
        let PairingPrompt {
            request_id,
            response,
        } = self.open_pairing_prompt(device_id, name, public_key, code, reason, false);

        let result = timeout(PAIRING_TIMEOUT, response).await;
        self.finish_pairing_prompt(&request_id);
        match result {
            Ok(Ok(true)) => PairingDecision {
                request_id,
                accepted: true,
                reason: None,
                message: None,
            },
            Ok(Ok(false)) => {
                self.emit_pairing_failed(
                    &request_id,
                    device_id,
                    REASON_PAIRING_USER_REJECTED,
                    MESSAGE_PAIRING_USER_REJECTED,
                );
                PairingDecision {
                    request_id,
                    accepted: false,
                    reason: Some(REASON_PAIRING_USER_REJECTED.to_string()),
                    message: Some(MESSAGE_PAIRING_USER_REJECTED.to_string()),
                }
            }
            Ok(Err(_)) => {
                self.emit_pairing_failed(
                    &request_id,
                    device_id,
                    REASON_PAIRING_CANCELLED,
                    MESSAGE_PAIRING_CANCELLED,
                );
                PairingDecision {
                    request_id,
                    accepted: false,
                    reason: Some(REASON_PAIRING_CANCELLED.to_string()),
                    message: Some(MESSAGE_PAIRING_CANCELLED.to_string()),
                }
            }
            Err(_) => {
                self.emit_pairing_failed(
                    &request_id,
                    device_id,
                    REASON_PAIRING_TIMEOUT,
                    MESSAGE_PAIRING_TIMEOUT,
                );
                PairingDecision {
                    request_id,
                    accepted: false,
                    reason: Some(REASON_PAIRING_TIMEOUT.to_string()),
                    message: Some(MESSAGE_PAIRING_TIMEOUT.to_string()),
                }
            }
        }
    }

    fn revoke_lan_pairing_for_key_change(&self, proof: &PeerProof) -> AppResult<()> {
        self.database.clear_lan_pairing(&proof.device_id)?;
        let _ = self.event_tx.send(RuntimeEvent::LanKeyChanged {
            device_id: proof.device_id.clone(),
            name: proof.name.clone(),
        });
        Ok(())
    }

    fn trust_peer(&self, proof: &PeerProof) -> AppResult<()> {
        let now = unix_now_millis();
        let existing = self
            .database
            .load_trusted_peer_keys()?
            .into_iter()
            .find(|record| record.device_id == proof.device_id);
        let key_changed = existing
            .as_ref()
            .is_some_and(|record| record.public_key != proof.public_key);
        self.database.upsert_trusted_peer_key(TrustedPeerKeyRecord {
            device_id: proof.device_id.clone(),
            name: proof.name.clone(),
            public_key: proof.public_key.clone(),
            key_updated_at: now,
            trusted_by_lan: true,
            trusted_by_cloud: existing
                .as_ref()
                .is_some_and(|record| record.trusted_by_cloud && !key_changed),
        })
    }

    fn emit_pairing_completed(&self, request_id: &str, device_id: &str) {
        let _ = self
            .event_tx
            .send(RuntimeEvent::LanPairingCompleted(LanPairingCompleted {
                request_id: request_id.to_string(),
                device_id: device_id.to_string(),
            }));
    }

    fn emit_pairing_failed(
        &self,
        request_id: &str,
        device_id: &str,
        reason: impl Into<String>,
        message: impl Into<String>,
    ) {
        let _ = self
            .event_tx
            .send(RuntimeEvent::LanPairingFailed(LanPairingFailed {
                request_id: request_id.to_string(),
                device_id: device_id.to_string(),
                reason: reason.into(),
                message: message.into(),
            }));
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
        let (ip, port, name, device_type) = {
            let inner = self.inner.lock_unpoisoned();
            let Some((ip, port)) = inner.peer_endpoints.get(device_id).cloned() else {
                return;
            };
            let name = inner
                .peer_names
                .get(device_id)
                .cloned()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| device_id.to_string());
            let device_type = inner
                .peer_types
                .get(device_id)
                .and_then(|value| normalized_peer_type(value))
                .unwrap_or_else(|| "unknown".to_string());
            (ip, port, name, device_type)
        };
        self.inner.lock_unpoisoned().pairing_candidates.insert(
            device_id.to_string(),
            LanPairingCandidate {
                device_id: device_id.to_string(),
                name,
                device_type,
                ip,
                port,
                state: state.as_str().to_string(),
            },
        );
        self.emit_pairing_candidates();
    }

    fn refresh_pairing_candidate(&self, device_id: &str) {
        let state = self
            .inner
            .lock_unpoisoned()
            .members
            .get(device_id)
            .map(|member| member.state);
        let Some(state) = state else {
            return;
        };
        self.update_pairing_candidate(device_id, state);
    }

    fn remember_peer_name(&self, device_id: &str, name: String) {
        self.inner
            .lock_unpoisoned()
            .peer_names
            .insert(device_id.to_string(), name);
        self.refresh_pairing_candidate(device_id);
    }

    fn remember_peer_type(&self, device_id: &str, device_type: String) {
        self.inner
            .lock_unpoisoned()
            .peer_types
            .insert(device_id.to_string(), device_type);
        self.refresh_pairing_candidate(device_id);
        let reachable = self
            .inner
            .lock_unpoisoned()
            .members
            .get(device_id)
            .is_some_and(|member| {
                matches!(member.state, MemberState::Alive | MemberState::Suspect)
            });
        if reachable && self.is_lan_authorized(device_id) {
            let _ = self.event_tx.send(RuntimeEvent::LanDeviceStateChanged {
                device_id: device_id.to_string(),
            });
        }
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
        self.refresh_pairing_candidate(device_id);
    }

    fn request_mdns_refresh(&self, device_id: &str, source_ip: IpAddr) {
        let refresh_tx = {
            let mut inner = self.inner.lock_unpoisoned();
            if inner
                .peer_endpoints
                .get(device_id)
                .is_some_and(|(ip, _)| ip == &source_ip.to_string())
            {
                return;
            }
            let now = unix_now_millis();
            if inner
                .discovery_refresh_at
                .get(device_id)
                .is_some_and(|last| now - *last < 15_000)
            {
                return;
            }
            inner
                .discovery_refresh_at
                .insert(device_id.to_string(), now);
            inner.discovery_refresh_tx.clone()
        };
        if let Some(refresh_tx) = refresh_tx {
            let _ = refresh_tx.send(DiscoveryRefreshRequest {
                label: device_id.to_string(),
                completion: None,
            });
        }
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

        let (session_id, camera) = if let Some(session_id) = path.strip_prefix("/transfer/") {
            (session_id, false)
        } else if let Some(session_id) = path.strip_prefix("/camera-stream/") {
            (session_id, true)
        } else {
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
                "missing stream token",
            ));
        }
        let valid = if camera {
            self.consume_camera_token(session_id, &token)
        } else {
            self.consume_transfer_token(session_id, &token)
        };
        if !valid {
            return Err(reject_ws(StatusCode::FORBIDDEN, "invalid stream token"));
        }

        Ok(if camera {
            InboundRoute::Camera { session_id: session_id.to_string() }
        } else {
            InboundRoute::Transfer { session_id: session_id.to_string() }
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

    fn consume_camera_token(&self, session_id: &str, token: &str) -> bool {
        let mut inner = self.inner.lock_unpoisoned();
        match inner.camera_tokens.get(session_id) {
            Some(expected) if expected == token => {
                inner.camera_tokens.remove(session_id);
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
        inner.discovery_refresh_tx = None;
        inner.high_priority_probe = false;
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

fn parse_pair_string(value: &str) -> Result<ParsedPairString, PairStringFailure> {
    let url = Url::parse(value).map_err(|_| PairStringFailure::Invalid)?;
    if url.scheme() != "colink" || url.host_str() != Some("pair") {
        return Err(PairStringFailure::Invalid);
    }
    let expected_key = match url.path() {
        "/v1" => "data",
        "/v2" => "d",
        _ => return Err(PairStringFailure::Invalid),
    };
    let mut encoded = None;
    for (key, value) in url.query_pairs() {
        if key != expected_key || encoded.replace(value.into_owned()).is_some() {
            return Err(PairStringFailure::Invalid);
        }
    }
    let encoded = encoded.ok_or(PairStringFailure::Invalid)?;
    match url.path() {
        "/v1" => {
            let payload = URL_SAFE_NO_PAD
                .decode(encoded)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<PairStringPayload>(&bytes).ok())
                .ok_or(PairStringFailure::Invalid)?;
            if payload.device_id.trim().is_empty()
                || payload.token.is_empty()
                || STANDARD.decode(&payload.public_key).map(|value| value.len() != 32).unwrap_or(true)
                || URL_SAFE_NO_PAD.decode(&payload.token).map(|value| value.len() != 32).unwrap_or(true)
            {
                return Err(PairStringFailure::Invalid);
            }
            Ok(ParsedPairString {
                version: PairStringVersion::V1,
                device_id: payload.device_id,
                public_key: payload.public_key,
                token: payload.token,
                expires_at: Some(payload.expires_at),
            })
        }
        "/v2" => {
            let payload = URL_SAFE_NO_PAD.decode(encoded).map_err(|_| PairStringFailure::Invalid)?;
            if payload.len() != 80 {
                return Err(PairStringFailure::Invalid);
            }
            let device_id = Uuid::from_slice(&payload[..16]).map_err(|_| PairStringFailure::Invalid)?;
            Ok(ParsedPairString {
                version: PairStringVersion::V2,
                device_id: device_id.to_string(),
                public_key: STANDARD.encode(&payload[16..48]),
                token: URL_SAFE_NO_PAD.encode(&payload[48..]),
                expires_at: None,
            })
        }
        _ => Err(PairStringFailure::Invalid),
    }
}

fn same_public_key(left: &str, right: &str) -> bool {
    match (STANDARD.decode(left), STANDARD.decode(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
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
        let peer_hello = exchange_hello(&mut stream, context, allow_pairing).await?;
    let mut outbound_seq = 1_u64;
    let trust_record = database
        .load_trusted_peer_keys()?
        .into_iter()
        .find(|record| record.device_id == expected_device_id && LanManager::is_trusted(record));

    let session = if !allow_pairing {
        let record = trust_record.ok_or_else(|| AppError::message("LAN device key is not trusted"))?;
        authenticate_outbound(manager, &mut stream, context, &record, &mut outbound_seq).await?
    } else {
        pair_outbound(
            manager,
            &mut stream,
            context,
            expected_device_id,
            &mut outbound_seq,
        )
        .await?
    };

    let (crypto, business_version) = negotiate_business_crypto(
        &mut stream,
        context,
        &session.peer_public_key,
        &session.peer_device_id,
        &peer_hello.payload.protocol_version,
        true,
        &mut outbound_seq,
    )
    .await?;

    Ok(HandshakeResult {
        stream,
        peer_device_id: session.peer_device_id,
        crypto,
        business_version,
        outbound_seq,
    })
}

async fn perform_inbound_handshake(
    manager: &LanManager,
    mut stream: WebSocketStream<TcpStream>,
    context: &LanContext,
    database: &Database,
) -> AppResult<HandshakeResult<TcpStream>> {
    let peer_hello = exchange_hello(&mut stream, context, false).await?;
    let mut outbound_seq = 1_u64;
    let peer_device_id = peer_hello.payload.device_id;
    let trust_record = database
        .load_trusted_peer_keys()?
        .into_iter()
        .find(|record| record.device_id == peer_device_id && LanManager::is_trusted(record));

    let force_pairing = peer_hello
        .payload
        .extensions
        .get("forcePairing")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let session = if !force_pairing {
        let record = trust_record.ok_or_else(|| AppError::message("LAN device key is not trusted"))?;
        authenticate_inbound(manager, &mut stream, context, &record, &mut outbound_seq).await?
    } else {
        pair_inbound(
            manager,
            &mut stream,
            context,
            &peer_device_id,
            &peer_hello.payload.protocol_version,
            &mut outbound_seq,
        )
        .await?
    };

    let (crypto, business_version) = negotiate_business_crypto(
        &mut stream,
        context,
        &session.peer_public_key,
        &session.peer_device_id,
        &peer_hello.payload.protocol_version,
        false,
        &mut outbound_seq,
    )
    .await?;

    Ok(HandshakeResult {
        stream,
        peer_device_id: session.peer_device_id,
        crypto,
        business_version,
        outbound_seq,
    })
}

struct LanPeerSession {
    peer_device_id: String,
    peer_public_key: String,
}

async fn exchange_hello<S>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    force_pairing: bool,
) -> AppResult<ProtocolHelloEnvelope>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let hello = ProtocolHelloEnvelope {
        message_type: "protocol.hello".to_string(),
        payload: ProtocolHelloPayload {
            device_id: context.device.device_id.clone(),
            protocol_version: LAN_PROTOCOL_VERSION.to_string(),
            extensions: serde_json::json!({ "forcePairing": force_pairing }),
        },
    };
    stream
        .send(Message::Text(serde_json::to_string(&hello)?.into()))
        .await
        .map_err(|error| AppError::message(error.to_string()))?;

    let mut peer_hello = None;
    let mut peer_ack = None;
    while peer_hello.is_none() || peer_ack.is_none() {
        let message = timeout(HANDSHAKE_TIMEOUT, read_text_frame(stream))
            .await
            .map_err(|_| AppError::message("LAN hello timed out"))??;

        if peer_hello.is_none() {
            if let Ok(next_hello) = serde_json::from_str::<ProtocolHelloEnvelope>(&message) {
                if next_hello.message_type == "protocol.hello" {
                    let compatibility =
                        check_lan_protocol_version(&next_hello.payload.protocol_version);
                    write_hello_ack(stream, &compatibility).await?;
                    if !compatibility.compatible {
                        return Err(AppError::message(
                            compatibility
                                .message
                                .or(compatibility.reason)
                                .unwrap_or_else(|| "LAN protocol version incompatible".to_string()),
                        ));
                    }
                    peer_hello = Some(next_hello);
                    continue;
                }
            }
        }

        if peer_ack.is_none() {
            let Ok(ack) = serde_json::from_str::<ProtocolHelloAckEnvelope>(&message) else {
                continue;
            };
            if ack.message_type != "protocol.hello-ack" {
                continue;
            }
            if !ack.payload.compatible {
                return Err(AppError::message(
                    ack.payload
                        .message
                        .or(ack.payload.reason)
                        .unwrap_or_else(|| "LAN protocol version incompatible".to_string()),
                ));
            }
            peer_ack = Some(ack);
            continue;
        }
    }

    peer_hello.ok_or_else(|| AppError::message("LAN hello timed out"))
}

async fn write_hello_ack<S>(
    stream: &mut WebSocketStream<S>,
    compatibility: &crate::protocol::VersionCompatibility,
) -> AppResult<()>
where
    WebSocketStream<S>:
        futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let ack = ProtocolHelloAckEnvelope {
        message_type: "protocol.hello-ack".to_string(),
        payload: VersionAckPayload {
            compatible: compatibility.compatible,
            reason: compatibility.reason.clone(),
            message: compatibility.message.clone(),
        },
    };
    stream
        .send(Message::Text(serde_json::to_string(&ack)?.into()))
        .await
        .map_err(|error| AppError::message(error.to_string()))
}

async fn authenticate_outbound<S>(
    manager: &LanManager,
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    record: &TrustedPeerKeyRecord,
    outbound_seq: &mut u64,
) -> AppResult<LanPeerSession>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let local_nonce = Uuid::new_v4().simple().to_string();
    write_lan_message(
        stream,
        context,
        &record.device_id,
        "auth.v1.challenge",
        None,
        outbound_seq,
        &AuthChallengePayload {
            nonce: local_nonce.clone(),
        },
    )
    .await?;

    let mut peer_nonce = None;
    let mut sent_response = false;
    let mut local_verified = false;
    let mut peer_verified = false;
    let mut auth_aborted = false;
    loop {
        let envelope = timeout(HANDSHAKE_TIMEOUT, read_lan_message(stream))
            .await
            .map_err(|_| AppError::message("LAN auth timed out"))??;
        if envelope.from != record.device_id || envelope.to != context.device.device_id {
            continue;
        }
        match envelope.message_type.as_str() {
            "auth.v1.challenge" => {
                let Ok(payload) = serde_json::from_value::<AuthChallengePayload>(envelope.payload)
                else {
                    continue;
                };
                peer_nonce = Some(payload.nonce.clone());
                if !sent_response {
                    send_auth_response(
                        stream,
                        context,
                        &record.device_id,
                        &payload.nonce,
                        Some(envelope.id.clone()),
                        outbound_seq,
                    )
                    .await?;
                    sent_response = true;
                }
            }
            "auth.v1.response" => {
                let Ok(payload) =
                    serde_json::from_value::<AuthResponsePayload>(envelope.payload.clone())
                else {
                    continue;
                };
                if verify_auth_response(record, &envelope, &local_nonce, &payload.signature)? {
                    write_lan_message(
                        stream,
                        context,
                        &record.device_id,
                        "auth.v1.verified",
                        Some(envelope.id.clone()),
                        outbound_seq,
                        &EmptyPayload {},
                    )
                    .await?;
                    local_verified = true;
                } else {
                    if !auth_aborted {
                        manager.revoke_lan_pairing_for_key_change(&PeerProof {
                            device_id: record.device_id.clone(),
                            public_key: record.public_key.clone(),
                            name: record.name.clone(),
                        })?;
                        auth_aborted = true;
                    }
                    write_lan_message(
                        stream,
                        context,
                        &record.device_id,
                        "auth.v1.reject",
                        Some(envelope.id.clone()),
                        outbound_seq,
                        &LanRejectPayload {
                            reason: REASON_AUTH_KEY_CHANGED.to_string(),
                            message: MESSAGE_AUTH_KEY_CHANGED.to_string(),
                            details: None,
                        },
                    )
                    .await?;
                }
            }
            "auth.v1.verified" => {
                if !auth_aborted {
                    peer_verified = true;
                }
            }
            "auth.v1.reject" => {
                let Ok(payload) = serde_json::from_value::<LanRejectPayload>(envelope.payload)
                else {
                    continue;
                };
                if payload.reason == REASON_AUTH_KEY_CHANGED && !auth_aborted {
                    manager.revoke_lan_pairing_for_key_change(&PeerProof {
                        device_id: record.device_id.clone(),
                        public_key: record.public_key.clone(),
                        name: record.name.clone(),
                    })?;
                    auth_aborted = true;
                }
                continue;
            }
            _ => {}
        }
        if !auth_aborted && peer_nonce.is_some() && local_verified && peer_verified {
            return Ok(LanPeerSession {
                peer_device_id: record.device_id.clone(),
                peer_public_key: record.public_key.clone(),
            });
        }
    }
}

async fn authenticate_inbound<S>(
    manager: &LanManager,
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    record: &TrustedPeerKeyRecord,
    outbound_seq: &mut u64,
) -> AppResult<LanPeerSession>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let mut local_nonce = None;
    let mut sent_response = false;
    let mut sent_challenge = false;
    let mut local_verified = false;
    let mut peer_verified = false;
    let mut auth_aborted = false;
    loop {
        let envelope = timeout(HANDSHAKE_TIMEOUT, read_lan_message(stream))
            .await
            .map_err(|_| AppError::message("LAN auth timed out"))??;
        if envelope.to != context.device.device_id {
            continue;
        }
        match envelope.message_type.as_str() {
            "pairing.v1.request" => {
                if envelope.from != record.device_id {
                    continue;
                }
                let Ok(request) =
                    serde_json::from_value::<PairingIdentityPayload>(envelope.payload)
                else {
                    continue;
                };
                let local_nonce = Uuid::new_v4().simple().to_string();
                write_lan_message(
                    stream,
                    context,
                    &record.device_id,
                    "pairing.v1.exchange",
                    Some(envelope.id.clone()),
                    outbound_seq,
                    &PairingIdentityPayload {
                        public_key: context.device.public_key.clone(),
                        name: context.device.name.clone(),
                        nonce: local_nonce.clone(),
                        pair_string: None,
                    },
                )
                .await?;
                let code = pairing_code(
                    &request.public_key,
                    &context.device.public_key,
                    &request.nonce,
                    &local_nonce,
                );
                let decision = manager
                    .request_pairing(
                        &record.device_id,
                        &request.name,
                        &request.public_key,
                        &code,
                        "unknown_device",
                    )
                    .await;
                if !decision.accepted {
                    write_lan_message(
                        stream,
                        context,
                        &record.device_id,
                        "pairing.v1.reject",
                        Some(envelope.id.clone()),
                        outbound_seq,
                        &LanRejectPayload {
                            reason: decision
                                .reason
                                .unwrap_or_else(|| REASON_PAIRING_USER_REJECTED.to_string()),
                            message: decision
                                .message
                                .unwrap_or_else(|| MESSAGE_PAIRING_USER_REJECTED.to_string()),
                            details: None,
                        },
                    )
                    .await?;
                    return Err(AppError::message(MESSAGE_PAIRING_USER_REJECTED));
                }
                write_lan_message(
                    stream,
                    context,
                    &record.device_id,
                    "pairing.v1.confirm",
                    Some(envelope.id.clone()),
                    outbound_seq,
                    &EmptyPayload {},
                )
                .await?;
                loop {
                    let complete = timeout(PAIRING_TIMEOUT, read_lan_message(stream))
                        .await
                        .map_err(|_| AppError::message("LAN pairing timed out"))??;
                    if complete.to != context.device.device_id || complete.from != record.device_id
                    {
                        continue;
                    }
                    match complete.message_type.as_str() {
                        "pairing.v1.complete" => {
                            manager.trust_peer(&PeerProof {
                                device_id: record.device_id.clone(),
                                public_key: request.public_key.clone(),
                                name: request.name.clone(),
                            })?;
                            manager.emit_pairing_completed(&decision.request_id, &record.device_id);
                            return Ok(LanPeerSession {
                                peer_device_id: record.device_id.clone(),
                                peer_public_key: request.public_key,
                            });
                        }
                        "pairing.v1.reject" => {
                            let (reason, message) = pairing_rejection(complete.payload);
                            manager.emit_pairing_failed(
                                &decision.request_id,
                                &record.device_id,
                                reason,
                                message,
                            );
                            break;
                        }
                        _ => {}
                    }
                }
            }
            "auth.v1.challenge" => {
                if envelope.from != record.device_id {
                    write_lan_message(
                        stream,
                        context,
                        &envelope.from,
                        "auth.v1.reject",
                        Some(envelope.id.clone()),
                        outbound_seq,
                        &LanRejectPayload {
                            reason: REASON_AUTH_UNKNOWN_DEVICE.to_string(),
                            message: MESSAGE_AUTH_UNKNOWN_DEVICE.to_string(),
                            details: None,
                        },
                    )
                    .await?;
                    continue;
                }
                let Ok(payload) = serde_json::from_value::<AuthChallengePayload>(envelope.payload)
                else {
                    continue;
                };
                if !sent_challenge {
                    let nonce = Uuid::new_v4().simple().to_string();
                    write_lan_message(
                        stream,
                        context,
                        &record.device_id,
                        "auth.v1.challenge",
                        None,
                        outbound_seq,
                        &AuthChallengePayload {
                            nonce: nonce.clone(),
                        },
                    )
                    .await?;
                    local_nonce = Some(nonce);
                    sent_challenge = true;
                }
                if !sent_response {
                    send_auth_response(
                        stream,
                        context,
                        &record.device_id,
                        &payload.nonce,
                        Some(envelope.id.clone()),
                        outbound_seq,
                    )
                    .await?;
                    sent_response = true;
                }
            }
            "auth.v1.response" => {
                if envelope.from != record.device_id {
                    continue;
                }
                let Some(nonce) = local_nonce.as_deref() else {
                    continue;
                };
                let Ok(payload) =
                    serde_json::from_value::<AuthResponsePayload>(envelope.payload.clone())
                else {
                    continue;
                };
                if verify_auth_response(record, &envelope, nonce, &payload.signature)? {
                    write_lan_message(
                        stream,
                        context,
                        &record.device_id,
                        "auth.v1.verified",
                        Some(envelope.id.clone()),
                        outbound_seq,
                        &EmptyPayload {},
                    )
                    .await?;
                    local_verified = true;
                } else {
                    if !auth_aborted {
                        manager.revoke_lan_pairing_for_key_change(&PeerProof {
                            device_id: record.device_id.clone(),
                            public_key: record.public_key.clone(),
                            name: record.name.clone(),
                        })?;
                        auth_aborted = true;
                    }
                    write_lan_message(
                        stream,
                        context,
                        &record.device_id,
                        "auth.v1.reject",
                        Some(envelope.id.clone()),
                        outbound_seq,
                        &LanRejectPayload {
                            reason: REASON_AUTH_KEY_CHANGED.to_string(),
                            message: MESSAGE_AUTH_KEY_CHANGED.to_string(),
                            details: None,
                        },
                    )
                    .await?;
                }
            }
            "auth.v1.verified" => {
                if !auth_aborted && envelope.from == record.device_id {
                    peer_verified = true;
                }
            }
            "auth.v1.reject" => {
                let Ok(payload) = serde_json::from_value::<LanRejectPayload>(envelope.payload)
                else {
                    continue;
                };
                if payload.reason == REASON_AUTH_KEY_CHANGED && !auth_aborted {
                    manager.revoke_lan_pairing_for_key_change(&PeerProof {
                        device_id: record.device_id.clone(),
                        public_key: record.public_key.clone(),
                        name: record.name.clone(),
                    })?;
                    auth_aborted = true;
                }
                continue;
            }
            _ => {}
        }
        if !auth_aborted && sent_challenge && sent_response && local_verified && peer_verified {
            return Ok(LanPeerSession {
                peer_device_id: record.device_id.clone(),
                peer_public_key: record.public_key.clone(),
            });
        }
    }
}

async fn pair_outbound<S>(
    manager: &LanManager,
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    expected_device_id: &str,
    outbound_seq: &mut u64,
) -> AppResult<LanPeerSession>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let local_nonce = Uuid::new_v4().simple().to_string();
    write_lan_message(
        stream,
        context,
        expected_device_id,
        "pairing.v1.request",
        None,
        outbound_seq,
        &PairingIdentityPayload {
            public_key: context.device.public_key.clone(),
            name: context.device.name.clone(),
            nonce: local_nonce.clone(),
            pair_string: None,
        },
    )
    .await?;

    let mut pairing_prompt: Option<PairingPrompt> = None;
    let mut peer_identity: Option<PairingIdentityPayload> = None;
    loop {
        let envelope = if let Some(prompt) = pairing_prompt.as_mut() {
            tokio::select! {
                response = &mut prompt.response => {
                    let request_id = prompt.request_id.clone();
                    let _ = response;
                    manager.finish_pairing_prompt(&request_id);
                    manager.emit_pairing_failed(
                        &request_id,
                        expected_device_id,
                        REASON_PAIRING_CANCELLED,
                        MESSAGE_PAIRING_CANCELLED,
                    );
                    write_lan_message(
                        stream,
                        context,
                        expected_device_id,
                        "pairing.v1.reject",
                        None,
                        outbound_seq,
                        &LanRejectPayload {
                            reason: REASON_PAIRING_CANCELLED.to_string(),
                            message: MESSAGE_PAIRING_CANCELLED.to_string(),
                            details: None,
                        },
                    )
                    .await?;
                    return Err(AppError::message(MESSAGE_PAIRING_CANCELLED));
                }
                result = timeout(PAIRING_TIMEOUT, read_lan_message(stream)) => {
                    match result {
                        Ok(Ok(envelope)) => envelope,
                        Ok(Err(error)) => {
                            let request_id = prompt.request_id.clone();
                            manager.finish_pairing_prompt(&request_id);
                            manager.emit_pairing_failed(
                                &request_id,
                                expected_device_id,
                                REASON_PAIRING_CONNECTION_CLOSED,
                                error.to_string(),
                            );
                            return Err(error);
                        }
                        Err(_) => {
                            let request_id = prompt.request_id.clone();
                            manager.finish_pairing_prompt(&request_id);
                            manager.emit_pairing_failed(
                                &request_id,
                                expected_device_id,
                                REASON_PAIRING_TIMEOUT,
                                MESSAGE_PAIRING_TIMEOUT,
                            );
                            write_lan_message(
                                stream,
                                context,
                                expected_device_id,
                                "pairing.v1.reject",
                                None,
                                outbound_seq,
                                &LanRejectPayload {
                                    reason: REASON_PAIRING_TIMEOUT.to_string(),
                                    message: MESSAGE_PAIRING_TIMEOUT.to_string(),
                                    details: None,
                                },
                            )
                            .await?;
                            return Err(AppError::message(MESSAGE_PAIRING_TIMEOUT));
                        }
                    }
                }
            }
        } else {
            timeout(PAIRING_TIMEOUT, read_lan_message(stream))
                .await
                .map_err(|_| AppError::message(MESSAGE_PAIRING_TIMEOUT))??
        };
        if envelope.to != context.device.device_id || envelope.from != expected_device_id {
            continue;
        }
        match envelope.message_type.as_str() {
            "pairing.v1.exchange" => {
                if pairing_prompt.is_some() {
                    continue;
                }
                let Ok(payload) =
                    serde_json::from_value::<PairingIdentityPayload>(envelope.payload)
                else {
                    continue;
                };
                let code = pairing_code(
                    &context.device.public_key,
                    &payload.public_key,
                    &local_nonce,
                    &payload.nonce,
                );
                pairing_prompt = Some(manager.open_pairing_prompt(
                    expected_device_id,
                    &payload.name,
                    &payload.public_key,
                    &code,
                    "unknown_device",
                    true,
                ));
                peer_identity = Some(payload);
            }
            "pairing.v1.confirm" => {
                let Some(prompt) = pairing_prompt.take() else {
                    continue;
                };
                let Some(peer_identity) = peer_identity else {
                    continue;
                };
                manager.trust_peer(&PeerProof {
                    device_id: expected_device_id.to_string(),
                    public_key: peer_identity.public_key.clone(),
                    name: peer_identity.name.clone(),
                })?;
                manager.finish_pairing_prompt(&prompt.request_id);
                manager.emit_pairing_completed(&prompt.request_id, expected_device_id);
                write_lan_message(
                    stream,
                    context,
                    expected_device_id,
                    "pairing.v1.complete",
                    Some(envelope.id.clone()),
                    outbound_seq,
                    &EmptyPayload {},
                )
                .await?;
                return Ok(LanPeerSession {
                    peer_device_id: expected_device_id.to_string(),
                    peer_public_key: peer_identity.public_key,
                });
            }
            "pairing.v1.reject" => {
                if let Some(prompt) = pairing_prompt.take() {
                    let (reason, message) = pairing_rejection(envelope.payload);
                    manager.finish_pairing_prompt(&prompt.request_id);
                    manager.emit_pairing_failed(
                        &prompt.request_id,
                        expected_device_id,
                        reason,
                        message.clone(),
                    );
                    return Err(AppError::message(message));
                }
            }
            _ => {}
        }
    }
}

async fn pair_inbound<S>(
    manager: &LanManager,
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    expected_device_id: &str,
    peer_protocol_version: &str,
    outbound_seq: &mut u64,
) -> AppResult<LanPeerSession>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    loop {
        let envelope = timeout(PAIRING_TIMEOUT, read_lan_message(stream))
            .await
            .map_err(|_| AppError::message("LAN pairing timed out"))??;
        if envelope.to != context.device.device_id || envelope.from != expected_device_id {
            continue;
        }
        if envelope.message_type != "pairing.v1.request" {
            continue;
        }
        let Ok(request) = serde_json::from_value::<PairingIdentityPayload>(envelope.payload) else {
            continue;
        };
        if let Some(pair_string) = request.pair_string.as_deref() {
            let supported = parse_pair_string(pair_string)
                .map(|pair_string| match pair_string.version {
                    PairStringVersion::V1 => supports_lan_pair_string(peer_protocol_version),
                    PairStringVersion::V2 => supports_lan_pair_string_v2(peer_protocol_version),
                })
                .unwrap_or(false);
            if !supported {
                write_lan_message(
                    stream,
                    context,
                    expected_device_id,
                    "pairing.v1.reject",
                    Some(envelope.id.clone()),
                    outbound_seq,
                    &LanRejectPayload {
                        reason: REASON_PAIRING_PAIR_STRING_INVALID.to_string(),
                        message: MESSAGE_PAIRING_PAIR_STRING_INVALID.to_string(),
                        details: None,
                    },
                )
                .await?;
                return Err(AppError::message(MESSAGE_PAIRING_PAIR_STRING_INVALID));
            }
            let token = match manager.reserve_pair_string(pair_string, context) {
                Ok(token) => token,
                Err(failure) => {
                    write_lan_message(
                        stream,
                        context,
                        expected_device_id,
                        "pairing.v1.reject",
                        Some(envelope.id.clone()),
                        outbound_seq,
                        &LanRejectPayload {
                            reason: failure.reason().to_string(),
                            message: failure.message().to_string(),
                            details: None,
                        },
                    )
                    .await?;
                    return Err(AppError::message(failure.message()));
                }
            };
            return pair_inbound_with_pair_string(
                manager,
                stream,
                context,
                expected_device_id,
                outbound_seq,
                request,
                envelope.id,
                token,
            )
            .await;
        }
        let local_nonce = Uuid::new_v4().simple().to_string();
        write_lan_message(
            stream,
            context,
            expected_device_id,
            "pairing.v1.exchange",
            Some(envelope.id.clone()),
            outbound_seq,
            &PairingIdentityPayload {
                public_key: context.device.public_key.clone(),
                name: context.device.name.clone(),
                nonce: local_nonce.clone(),
                pair_string: None,
            },
        )
        .await?;
        let code = pairing_code(
            &request.public_key,
            &context.device.public_key,
            &request.nonce,
            &local_nonce,
        );
        let PairingPrompt {
            request_id,
            mut response,
        } = manager.open_pairing_prompt(
            expected_device_id,
            &request.name,
            &request.public_key,
            &code,
            "unknown_device",
            false,
        );
        loop {
            tokio::select! {
                decision = &mut response => {
                    match decision {
                        Ok(true) => break,
                        Ok(false) | Err(_) => {
                            manager.finish_pairing_prompt(&request_id);
                            manager.emit_pairing_failed(
                                &request_id,
                                expected_device_id,
                                REASON_PAIRING_USER_REJECTED,
                                MESSAGE_PAIRING_USER_REJECTED,
                            );
                            write_lan_message(
                                stream,
                                context,
                                expected_device_id,
                                "pairing.v1.reject",
                                Some(envelope.id.clone()),
                                outbound_seq,
                                &LanRejectPayload {
                                    reason: REASON_PAIRING_USER_REJECTED.to_string(),
                                    message: MESSAGE_PAIRING_USER_REJECTED.to_string(),
                                    details: None,
                                },
                            )
                            .await?;
                            let _ = timeout(Duration::from_secs(1), read_lan_message(stream)).await;
                            return Err(AppError::message(MESSAGE_PAIRING_USER_REJECTED));
                        }
                    }
                }
                result = timeout(PAIRING_TIMEOUT, read_lan_message(stream)) => {
                    let pairing_message = match result {
                        Ok(Ok(pairing_message)) => pairing_message,
                        Ok(Err(error)) => {
                            manager.finish_pairing_prompt(&request_id);
                            manager.emit_pairing_failed(
                                &request_id,
                                expected_device_id,
                                REASON_PAIRING_CONNECTION_CLOSED,
                                error.to_string(),
                            );
                            return Err(error);
                        }
                        Err(_) => {
                            manager.finish_pairing_prompt(&request_id);
                            manager.emit_pairing_failed(
                                &request_id,
                                expected_device_id,
                                REASON_PAIRING_TIMEOUT,
                                MESSAGE_PAIRING_TIMEOUT,
                            );
                            write_lan_message(
                                stream,
                                context,
                                expected_device_id,
                                "pairing.v1.reject",
                                Some(envelope.id.clone()),
                                outbound_seq,
                                &LanRejectPayload {
                                    reason: REASON_PAIRING_TIMEOUT.to_string(),
                                    message: MESSAGE_PAIRING_TIMEOUT.to_string(),
                                    details: None,
                                },
                            )
                            .await?;
                            return Err(AppError::message(MESSAGE_PAIRING_TIMEOUT));
                        }
                    };
                    if pairing_message.to != context.device.device_id || pairing_message.from != expected_device_id {
                        continue;
                    }
                    if pairing_message.message_type == "pairing.v1.reject" {
                        let (reason, message) = pairing_rejection(pairing_message.payload);
                        manager.finish_pairing_prompt(&request_id);
                        manager.emit_pairing_failed(
                            &request_id,
                            expected_device_id,
                            reason,
                            message.clone(),
                        );
                        return Err(AppError::message(message));
                    }
                }
            }
        }
        write_lan_message(
            stream,
            context,
            expected_device_id,
            "pairing.v1.confirm",
            Some(envelope.id.clone()),
            outbound_seq,
            &EmptyPayload {},
        )
        .await?;
        loop {
            let complete = match timeout(PAIRING_TIMEOUT, read_lan_message(stream)).await {
                Ok(Ok(complete)) => complete,
                Ok(Err(error)) => {
                    manager.emit_pairing_failed(
                        &request_id,
                        expected_device_id,
                        REASON_PAIRING_CONNECTION_CLOSED,
                        error.to_string(),
                    );
                    return Err(error);
                }
                Err(_) => {
                    manager.emit_pairing_failed(
                        &request_id,
                        expected_device_id,
                        REASON_PAIRING_TIMEOUT,
                        MESSAGE_PAIRING_TIMEOUT,
                    );
                    write_lan_message(
                        stream,
                        context,
                        expected_device_id,
                        "pairing.v1.reject",
                        Some(envelope.id.clone()),
                        outbound_seq,
                        &LanRejectPayload {
                            reason: REASON_PAIRING_TIMEOUT.to_string(),
                            message: MESSAGE_PAIRING_TIMEOUT.to_string(),
                            details: None,
                        },
                    )
                    .await?;
                    return Err(AppError::message(MESSAGE_PAIRING_TIMEOUT));
                }
            };
            if complete.to != context.device.device_id || complete.from != expected_device_id {
                continue;
            }
            match complete.message_type.as_str() {
                "pairing.v1.complete" => {
                    manager.trust_peer(&PeerProof {
                        device_id: expected_device_id.to_string(),
                        public_key: request.public_key.clone(),
                        name: request.name.clone(),
                    })?;
                    manager.emit_pairing_completed(&request_id, expected_device_id);
                    return Ok(LanPeerSession {
                        peer_device_id: expected_device_id.to_string(),
                        peer_public_key: request.public_key,
                    });
                }
                "pairing.v1.reject" => {
                    let (reason, message) = pairing_rejection(complete.payload);
                    manager.emit_pairing_failed(
                        &request_id,
                        expected_device_id,
                        reason,
                        message.clone(),
                    );
                    return Err(AppError::message(message));
                }
                _ => {}
            }
        }
    }
}

async fn pair_inbound_with_pair_string<S>(
    manager: &LanManager,
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    expected_device_id: &str,
    outbound_seq: &mut u64,
    request: PairingIdentityPayload,
    request_id: String,
    token: String,
) -> AppResult<LanPeerSession>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let result = async {
        let local_nonce = Uuid::new_v4().simple().to_string();
        write_lan_message(
            stream,
            context,
            expected_device_id,
            "pairing.v1.exchange",
            Some(request_id.clone()),
            outbound_seq,
            &PairingIdentityPayload {
                public_key: context.device.public_key.clone(),
                name: context.device.name.clone(),
                nonce: local_nonce,
                pair_string: None,
            },
        )
        .await?;
        write_lan_message(
            stream,
            context,
            expected_device_id,
            "pairing.v1.confirm",
            Some(request_id),
            outbound_seq,
            &EmptyPayload {},
        )
        .await?;
        loop {
            let complete = timeout(PAIRING_TIMEOUT, read_lan_message(stream))
                .await
                .map_err(|_| AppError::message(MESSAGE_PAIRING_TIMEOUT))??;
            if complete.to != context.device.device_id || complete.from != expected_device_id {
                continue;
            }
            match complete.message_type.as_str() {
                "pairing.v1.complete" => {
                    manager.trust_peer(&PeerProof {
                        device_id: expected_device_id.to_string(),
                        public_key: request.public_key.clone(),
                        name: request.name.clone(),
                    })?;
                    manager.consume_pair_string(&token);
                    return Ok(LanPeerSession {
                        peer_device_id: expected_device_id.to_string(),
                        peer_public_key: request.public_key,
                    });
                }
                "pairing.v1.reject" => {
                    let message = serde_json::from_value::<LanRejectPayload>(complete.payload)
                        .map(|payload| payload.message)
                        .unwrap_or_else(|_| "pairing rejected".to_string());
                    return Err(AppError::message(message));
                }
                _ => {}
            }
        }
    }
    .await;
    if result.is_err() {
        manager.cancel_pair_string(&token);
    }
    result
}

async fn negotiate_business_crypto<S>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    peer_public_key: &str,
    peer_device_id: &str,
    peer_protocol_version: &str,
    local_is_initiator: bool,
    outbound_seq: &mut u64,
) -> AppResult<(LanSessionCrypto, String)>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let peer_business_version =
        exchange_business_version(stream, context, peer_device_id, outbound_seq).await?;
    let key_exchange = if supports_lan_key_exchange(peer_protocol_version) {
        Some(
            exchange_ephemeral_keys(
                stream,
                context,
                peer_public_key,
                peer_device_id,
                peer_protocol_version,
                outbound_seq,
            )
            .await?,
        )
    } else {
        None
    };

    let local_supported = supported_suites();
    write_lan_message(
        stream,
        context,
        peer_device_id,
        "business.v1.negotiate",
        None,
        outbound_seq,
        &BusinessNegotiatePayload {
            supported: local_supported.clone(),
            preferred: AES_256_GCM_SUITE.to_string(),
        },
    )
    .await?;
    loop {
        let message = timeout(HANDSHAKE_TIMEOUT, read_lan_message(stream))
            .await
            .map_err(|_| AppError::message("LAN encryption negotiation timed out"))??;
        if message.to != context.device.device_id || message.from != peer_device_id {
            continue;
        }
        if message.message_type != "business.v1.negotiate" {
            continue;
        }
        let Ok(peer) = serde_json::from_value::<BusinessNegotiatePayload>(message.payload) else {
            continue;
        };
        let suite = choose_suite(&local_supported, &peer.supported, local_is_initiator)
            .ok_or_else(|| AppError::message("no compatible LAN encryption suite is available"))?;
        let crypto = if let Some(key_exchange) = key_exchange.as_ref() {
            LanSessionCrypto::new_with_ephemeral_keys(
                suite,
                &key_exchange.local,
                &key_exchange.peer_public_key,
                &context.device.device_id,
                peer_device_id,
                &negotiated_lan_protocol_version(peer_protocol_version),
                local_is_initiator,
            )
        } else {
            LanSessionCrypto::new(
                suite,
                &context.device.private_key,
                peer_public_key,
                local_is_initiator,
            )
        }?;
        return Ok((crypto, peer_business_version));
    }

}

struct EphemeralKeyExchange {
    local: LanEphemeralKeyPair,
    peer_public_key: String,
}

async fn exchange_ephemeral_keys<S>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    peer_identity_public_key: &str,
    peer_device_id: &str,
    peer_protocol_version: &str,
    outbound_seq: &mut u64,
) -> AppResult<EphemeralKeyExchange>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let nonce_exchange = if supports_lan_key_exchange_nonce(peer_protocol_version) {
        Some(exchange_key_exchange_nonces(stream, context, peer_device_id, outbound_seq).await?)
    } else {
        None
    };
    let local = LanEphemeralKeyPair::generate();
    let timestamp = unix_now_millis();
    let signature_input = if let Some(nonces) = nonce_exchange.as_ref() {
        key_exchange_signature_input_v2(
            &context.device.device_id,
            peer_device_id,
            &local.public_key,
            &nonces.local,
            &nonces.peer,
        )
    } else {
        key_exchange_signature_input(
            &context.device.device_id,
            peer_device_id,
            &local.public_key,
            timestamp,
        )
    };
    let signature = sign_payload(&context.device.private_key, signature_input.as_bytes())?;
    write_lan_message_with_timestamp(
        stream,
        context,
        peer_device_id,
        "business.v1.key-exchange",
        None,
        timestamp,
        outbound_seq,
        &BusinessKeyExchangePayload {
            ephemeral_public_key: local.public_key.clone(),
            signature,
        },
    )
    .await?;

    loop {
        let message = timeout(HANDSHAKE_TIMEOUT, read_lan_message(stream))
            .await
            .map_err(|_| AppError::message("LAN key exchange timed out"))??;
        if message.to != context.device.device_id || message.from != peer_device_id {
            continue;
        }
        match message.message_type.as_str() {
            "business.v1.key-exchange" => {
                let Ok(payload) =
                    serde_json::from_value::<BusinessKeyExchangePayload>(message.payload)
                else {
                    reject_key_exchange(
                        stream,
                        context,
                        peer_device_id,
                        Some(message.id.clone()),
                        outbound_seq,
                        REASON_KEY_EXCHANGE_GENERIC,
                        MESSAGE_KEY_EXCHANGE_GENERIC,
                    )
                    .await?;
                    return Err(AppError::message(MESSAGE_KEY_EXCHANGE_GENERIC));
                };
                if nonce_exchange.is_none() && (unix_now_millis() - message.timestamp).abs() > 30_000 {
                    reject_key_exchange(
                        stream,
                        context,
                        peer_device_id,
                        Some(message.id.clone()),
                        outbound_seq,
                        REASON_KEY_EXCHANGE_TIMESTAMP_EXPIRED,
                        MESSAGE_KEY_EXCHANGE_TIMESTAMP_EXPIRED,
                    )
                    .await?;
                    return Err(AppError::message(MESSAGE_KEY_EXCHANGE_TIMESTAMP_EXPIRED));
                }
                let signature_input = if let Some(nonces) = nonce_exchange.as_ref() {
                    key_exchange_signature_input_v2(
                        &message.from,
                        &message.to,
                        &payload.ephemeral_public_key,
                        &nonces.peer,
                        &nonces.local,
                    )
                } else {
                    key_exchange_signature_input(
                        &message.from,
                        &message.to,
                        &payload.ephemeral_public_key,
                        message.timestamp,
                    )
                };
                let valid = verify_signature(peer_identity_public_key, signature_input.as_bytes(), &payload.signature)?;
                if !valid {
                    reject_key_exchange(
                        stream,
                        context,
                        peer_device_id,
                        Some(message.id.clone()),
                        outbound_seq,
                        REASON_KEY_EXCHANGE_SIGNATURE_INVALID,
                        MESSAGE_KEY_EXCHANGE_SIGNATURE_INVALID,
                    )
                    .await?;
                    return Err(AppError::message(MESSAGE_KEY_EXCHANGE_SIGNATURE_INVALID));
                }
                return Ok(EphemeralKeyExchange {
                    local,
                    peer_public_key: payload.ephemeral_public_key,
                });
            }
            "business.v1.key-exchange-reject" => {
                let message = serde_json::from_value::<LanRejectPayload>(message.payload)
                    .map(|payload| payload.message)
                    .unwrap_or_else(|_| MESSAGE_KEY_EXCHANGE_GENERIC.to_string());
                return Err(AppError::message(message));
            }
            _ => {}
        }
    }
}

struct KeyExchangeNonces {
    local: String,
    peer: String,
}

async fn exchange_key_exchange_nonces<S>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    peer_device_id: &str,
    outbound_seq: &mut u64,
) -> AppResult<KeyExchangeNonces>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    let local = random_key_exchange_nonce();
    write_lan_message(
        stream,
        context,
        peer_device_id,
        "business.v1.key-exchange-nonce",
        None,
        outbound_seq,
        &BusinessKeyExchangeNoncePayload {
            nonce: local.clone(),
        },
    )
    .await?;

    loop {
        let message = timeout(HANDSHAKE_TIMEOUT, read_lan_message(stream))
            .await
            .map_err(|_| AppError::message("LAN key exchange nonce timed out"))??;
        if message.to != context.device.device_id || message.from != peer_device_id {
            continue;
        }
        if message.message_type != "business.v1.key-exchange-nonce" {
            continue;
        }
        let Ok(payload) = serde_json::from_value::<BusinessKeyExchangeNoncePayload>(message.payload) else {
            continue;
        };
        return Ok(KeyExchangeNonces {
            local,
            peer: payload.nonce,
        });
    }
}

fn random_key_exchange_nonce() -> String {
    let mut nonce = [0_u8; 32];
    OsRng.fill_bytes(&mut nonce);
    STANDARD.encode(nonce)
}

async fn reject_key_exchange<S>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    peer_device_id: &str,
    correlation_id: Option<String>,
    outbound_seq: &mut u64,
    reason: &str,
    message: &str,
) -> AppResult<()>
where
    WebSocketStream<S>:
        futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    write_lan_message(
        stream,
        context,
        peer_device_id,
        "business.v1.key-exchange-reject",
        correlation_id,
        outbound_seq,
        &LanRejectPayload {
            reason: reason.to_string(),
            message: message.to_string(),
            details: None,
        },
    )
    .await
}

async fn exchange_business_version<S>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    peer_device_id: &str,
    outbound_seq: &mut u64,
) -> AppResult<String>
where
    WebSocketStream<S>: futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error>
        + futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
        + Unpin,
{
    write_lan_message(
        stream,
        context,
        peer_device_id,
        "business.v1.version",
        None,
        outbound_seq,
        &BusinessVersionPayload {
            business_version: BUSINESS_PROTOCOL_VERSION.to_string(),
        },
    )
    .await?;

    let mut peer_version_received = false;
    let mut ack_received = false;
    let mut peer_business_version = None;
    while !peer_version_received || !ack_received {
        let message = timeout(HANDSHAKE_TIMEOUT, read_lan_message(stream))
            .await
            .map_err(|_| AppError::message("LAN business version exchange timed out"))??;
        if message.to != context.device.device_id || message.from != peer_device_id {
            continue;
        }
        match message.message_type.as_str() {
            "business.v1.version" => {
                let payload = serde_json::from_value::<BusinessVersionPayload>(message.payload);
                let compatibility = payload
                    .as_ref()
                    .map(|payload| check_business_protocol_version(&payload.business_version))
                    .unwrap_or_else(|_| check_business_protocol_version(""));
                write_lan_message(
                    stream,
                    context,
                    peer_device_id,
                    "business.v1.version-ack",
                    Some(message.id.clone()),
                    outbound_seq,
                    &BusinessVersionAckPayload {
                        compatible: compatibility.compatible,
                        reason: compatibility.reason.clone(),
                        message: compatibility.message.clone(),
                    },
                )
                .await?;
                if !compatibility.compatible {
                    return Err(AppError::message(
                        compatibility
                            .message
                            .or(compatibility.reason)
                            .unwrap_or_else(|| {
                                "business protocol version incompatible".to_string()
                        }),
                    ));
                }
                peer_business_version = payload.ok().map(|payload| payload.business_version);
                peer_version_received = true;
            }
            "business.v1.version-ack" => {
                let Ok(ack) = serde_json::from_value::<BusinessVersionAckPayload>(message.payload)
                else {
                    continue;
                };
                if !ack.compatible {
                    return Err(AppError::message(
                        ack.message.or(ack.reason).unwrap_or_else(|| {
                            "business protocol version incompatible".to_string()
                        }),
                    ));
                }
                ack_received = true;
            }
            _ => {}
        }
    }
    peer_business_version.ok_or_else(|| AppError::message("business protocol version missing"))
}

async fn send_auth_response<S>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    peer_device_id: &str,
    peer_nonce: &str,
    correlation_id: Option<String>,
    outbound_seq: &mut u64,
) -> AppResult<()>
where
    WebSocketStream<S>:
        futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let timestamp = unix_now_millis();
    let input = auth_signature_input(&context.device.device_id, timestamp, peer_nonce);
    let signature = sign_payload(&context.device.private_key, input.as_bytes())?;
    write_lan_message_with_timestamp(
        stream,
        context,
        peer_device_id,
        "auth.v1.response",
        correlation_id,
        timestamp,
        outbound_seq,
        &AuthResponsePayload { signature },
    )
    .await
}

fn verify_auth_response(
    record: &TrustedPeerKeyRecord,
    envelope: &LanEnvelope,
    peer_nonce: &str,
    signature: &str,
) -> AppResult<bool> {
    let input = auth_signature_input(&envelope.from, envelope.timestamp, peer_nonce);
    verify_signature(&record.public_key, input.as_bytes(), signature)
}

fn auth_signature_input(from: &str, timestamp: i64, nonce: &str) -> String {
    format!("from={from}\ntimestamp={timestamp}\nnonce={nonce}")
}

fn key_exchange_signature_input(from: &str, to: &str, ephemeral_public_key: &str, timestamp: i64) -> String {
    format!(
        "domain=colink-lan-key-exchange\nfrom={from}\nto={to}\nephemeralPublicKey={ephemeral_public_key}\ntimestamp={timestamp}"
    )
}

fn key_exchange_signature_input_v2(
    from: &str,
    to: &str,
    ephemeral_public_key: &str,
    local_nonce: &str,
    peer_nonce: &str,
) -> String {
    format!(
        "domain=colink-lan-key-exchange-v2\nfrom={from}\nto={to}\nephemeralPublicKey={ephemeral_public_key}\nlocalNonce={local_nonce}\npeerNonce={peer_nonce}"
    )
}

async fn write_lan_message<S, T>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    to: &str,
    message_type: &str,
    correlation_id: Option<String>,
    outbound_seq: &mut u64,
    payload: &T,
) -> AppResult<()>
where
    T: serde::Serialize,
    WebSocketStream<S>:
        futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    write_lan_message_with_timestamp(
        stream,
        context,
        to,
        message_type,
        correlation_id,
        unix_now_millis(),
        outbound_seq,
        payload,
    )
    .await
}

async fn write_lan_message_with_timestamp<S, T>(
    stream: &mut WebSocketStream<S>,
    context: &LanContext,
    to: &str,
    message_type: &str,
    correlation_id: Option<String>,
    timestamp: i64,
    outbound_seq: &mut u64,
    payload: &T,
) -> AppResult<()>
where
    T: serde::Serialize,
    WebSocketStream<S>:
        futures_util::Sink<Message, Error = tokio_tungstenite::tungstenite::Error> + Unpin,
{
    let envelope = LanEnvelope {
        id: Uuid::new_v4().to_string(),
        message_type: message_type.to_string(),
        from: context.device.device_id.clone(),
        to: to.to_string(),
        seq: next_lan_seq(outbound_seq),
        timestamp,
        correlation_id,
        payload: serde_json::to_value(payload)?,
    };
    stream
        .send(Message::Text(serde_json::to_string(&envelope)?.into()))
        .await
        .map_err(|error| AppError::message(error.to_string()))
}

fn next_lan_seq(next: &mut u64) -> u64 {
    let seq = *next;
    *next = next.saturating_add(1);
    seq
}

async fn read_lan_message<S>(stream: &mut WebSocketStream<S>) -> AppResult<LanEnvelope>
where
    WebSocketStream<S>:
        futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    loop {
        let text = read_text_frame(stream).await?;
        let Ok(envelope) = serde_json::from_str::<LanEnvelope>(&text) else {
            continue;
        };
        return Ok(envelope);
    }
}

async fn read_text_frame<S>(stream: &mut WebSocketStream<S>) -> AppResult<String>
where
    WebSocketStream<S>:
        futures_util::Stream<Item = Result<Message, tokio_tungstenite::tungstenite::Error>> + Unpin,
{
    while let Some(message) = stream.next().await {
        match message.map_err(|error| AppError::message(error.to_string()))? {
            Message::Text(text) => return Ok(text.to_string()),
            Message::Close(_) => return Err(AppError::message("LAN connection was closed")),
            Message::Ping(_) | Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
        }
    }
    Err(AppError::message("LAN connection ended"))
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

async fn bind_lan_listener() -> io::Result<(TcpListener, u16)> {
    let mut last_error = None;
    for port in lan_port_candidates() {
        match TcpListener::bind(("0.0.0.0", port)).await {
            Ok(listener) => return Ok((listener, port)),
            Err(error) if error.kind() == io::ErrorKind::AddrInUse => {
                last_error = Some(error);
            }
            Err(error) => return Err(error),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        io::Error::new(io::ErrorKind::AddrInUse, "no LAN port is available")
    }))
}

fn lan_port_candidates() -> impl Iterator<Item = u16> {
    let mut ports = Vec::with_capacity((u16::MAX - MIN_LAN_PORT + 1) as usize);
    ports.push(LAN_PORT);
    let max_distance = (LAN_PORT - MIN_LAN_PORT).max(u16::MAX - LAN_PORT);
    for distance in 1..=max_distance {
        if let Some(port) = LAN_PORT.checked_add(distance) {
            ports.push(port);
        }
        if let Some(port) = LAN_PORT.checked_sub(distance).filter(|port| *port >= MIN_LAN_PORT) {
            ports.push(port);
        }
    }
    ports.into_iter()
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

fn normalized_peer_type(value: &str) -> Option<String> {
    let value = value.trim().to_ascii_lowercase();
    match value.as_str() {
        "windows" | "macos" | "linux" | "android" | "ios" => Some(value),
        _ => None,
    }
}

fn shuffled_probe_queue(mut candidates: Vec<String>) -> VecDeque<String> {
    candidates.shuffle(&mut rand::thread_rng());
    candidates.into()
}

impl SwimEnvelope {
    fn is_target_ack(&self, target: &str) -> bool {
        self.message_type == "swim.ack" && self.payload.from == target
    }
}

fn same_lan_identity(left: &DeviceIdentity, right: &DeviceIdentity) -> bool {
    left.device_id == right.device_id
}

#[cfg(test)]
mod tests {
    use super::{lan_port_candidates, CameraReceiveBuffer, LanManager, MemberRecord, MemberState};
    use crate::protocol::CameraDataFrame;

    fn member(state: MemberState, incarnation: i64) -> MemberRecord {
        MemberRecord {
            state,
            incarnation,
            updated_at: 0,
            missed_probes: 0,
        }
    }

    fn camera_frame(sequence: u64, keyframe: bool) -> CameraDataFrame {
        CameraDataFrame::new("h264", keyframe, sequence, sequence, vec![sequence as u8])
            .expect("camera frame")
    }

    #[test]
    fn camera_receive_buffer_discards_deltas_until_keyframe_after_overflow() {
        let mut buffer = CameraReceiveBuffer::default();
        assert!(buffer.push(camera_frame(0, true)));
        assert!(buffer.push(camera_frame(1, false)));
        assert!(buffer.push(camera_frame(2, false)));
        assert!(buffer.push(camera_frame(3, false)));
        assert!(!buffer.push(camera_frame(4, false)));
        assert!(!buffer.push(camera_frame(5, false)));
        assert!(buffer.push(camera_frame(6, true)));
        assert!(buffer.push(camera_frame(7, false)));

        let frames = buffer.take();
        assert_eq!(
            frames
                .into_iter()
                .map(|frame| frame.sequence)
                .collect::<Vec<_>>(),
            vec![6, 7]
        );
    }

    #[test]
    fn port_candidates_choose_the_nearest_higher_port_on_ties() {
        assert_eq!(
            lan_port_candidates().take(5).collect::<Vec<_>>(),
            vec![27_777, 27_778, 27_776, 27_779, 27_775],
        );
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

    #[test]
    fn explicit_direct_observation_cannot_revive_same_incarnation_dead() {
        let existing = member(MemberState::Dead, 100);
        assert!(!LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Alive,
            100,
            true,
        ));
        assert!(LanManager::should_accept_member_update(
            Some(&existing),
            MemberState::Alive,
            101,
            true,
        ));
    }
}
