use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const LAN_PROTOCOL_VERSION: &str = "1.2.0";
pub const BUSINESS_PROTOCOL_VERSION: &str = "1.5.0";
pub const TEXT_MESSAGE_TYPE: &str = "message.v1.text";
pub const CLIPBOARD_SYNC_TYPE: &str = "clipboard.v1.sync";
pub const FILE_OFFER_TYPE: &str = "file.v2.offer";
pub const FILE_ACCEPT_TYPE: &str = "file.v2.accept";
pub const FILE_REJECT_TYPE: &str = "file.v2.reject";
pub const FILE_CANCEL_TYPE: &str = "file.v2.cancel";
pub const FILE_READY_TYPE: &str = "file.v2.ready";
pub const FILE_CHUNK_TYPE: &str = "file.v2.chunk";
pub const FILE_ACK_TYPE: &str = "file.v2.ack";
pub const FILE_RETRANSMIT_TYPE: &str = "file.v2.retransmit";
pub const FILE_DONE_TYPE: &str = "file.v2.done";
pub const MUSIC_TRACK_TYPE: &str = "music.v1.track";
pub const MUSIC_LYRIC_TYPE: &str = "music.v1.lyric";
pub const MUSIC_PROGRESS_TYPE: &str = "music.v1.progress";
pub const MUSIC_ALIVE_TYPE: &str = "music.v1.alive";
pub const MUSIC_REQUEST_TYPE: &str = "music.v1.request";
pub const SYSINFO_STATS_TYPE: &str = "sysinfo.v1.stats";
pub const SYSINFO_ALIVE_TYPE: &str = "sysinfo.v1.alive";
pub const FS_ROOTS_TYPE: &str = "fs.v1.roots";
pub const FS_ROOTS_RESULT_TYPE: &str = "fs.v1.roots-result";
pub const FS_LIST_TYPE: &str = "fs.v1.list";
pub const FS_LIST_RESULT_TYPE: &str = "fs.v1.list-result";
pub const FS_STAT_TYPE: &str = "fs.v1.stat";
pub const FS_STAT_RESULT_TYPE: &str = "fs.v1.stat-result";
pub const FS_DOWNLOAD_TYPE: &str = "fs.v1.download";
pub const FS_ERROR_TYPE: &str = "fs.v1.error";
pub const SYSTEM_CONTROL_COMMAND_TYPE: &str = "system-control.v1.command";

const FILE_DATA_FRAME_VERSION: u8 = 0x01;
const FILE_DATA_FRAME_HEADER_LEN: usize = 8;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessEnvelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub payload: Value,
}

impl BusinessEnvelope {
    pub fn from_payload<T>(message_type: &str, payload: T) -> serde_json::Result<Self>
    where
        T: Serialize,
    {
        Ok(Self {
            message_type: message_type.to_string(),
            payload: serde_json::to_value(payload)?,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextMessagePayload {
    pub message_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClipboardSyncPayload {
    pub content_type: String,
    pub content: Option<String>,
    pub data: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOfferPayload {
    pub session_id: String,
    pub file_name: String,
    pub file_size: i64,
    pub total_chunks: i64,
    pub chunk_size: i64,
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAcceptPayload {
    pub session_id: String,
    pub transfer_token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRejectPayload {
    pub session_id: String,
    pub reason: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCancelPayload {
    pub session_id: String,
    pub reason: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReadyPayload {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChunkPayload {
    pub session_id: String,
    pub chunk_index: i64,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAckPayload {
    pub session_id: String,
    pub next_expected_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRetransmitPayload {
    pub session_id: String,
    pub chunk_index: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDonePayload {
    pub session_id: String,
    pub success: bool,
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicTrackPayload {
    pub track_id: Option<String>,
    pub title: Option<String>,
    pub artists: Option<Vec<String>>,
    pub album: Option<String>,
    pub source: Option<String>,
    pub cover_url: Option<String>,
    pub cover_data: Option<String>,
    pub duration: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicLyricLinePayload {
    pub time: i64,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicLyricPayload {
    pub track_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines: Option<Vec<MusicLyricLinePayload>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub translated_lines: Option<Vec<MusicLyricLinePayload>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicProgressPayload {
    pub track_id: String,
    pub progress: i64,
    pub paused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SysInfoStatsPayload {
    pub cpu: f64,
    pub mem: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gpu: Option<f64>,
    #[serde(rename = "net_up", skip_serializing_if = "Option::is_none")]
    pub net_up: Option<f64>,
    #[serde(rename = "net_down", skip_serializing_if = "Option::is_none")]
    pub net_down: Option<f64>,
    #[serde(rename = "disk_read", skip_serializing_if = "Option::is_none")]
    pub disk_read: Option<f64>,
    #[serde(rename = "disk_write", skip_serializing_if = "Option::is_none")]
    pub disk_write: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileDataFrameKind {
    Chunk,
    Ack,
    Finish,
    Retransmit,
    Cancel,
}

impl FileDataFrameKind {
    fn from_wire(value: u8) -> Option<Self> {
        match value {
            0x01 => Some(Self::Chunk),
            0x02 => Some(Self::Ack),
            0x03 => Some(Self::Finish),
            0x04 => Some(Self::Retransmit),
            0x05 => Some(Self::Cancel),
            _ => None,
        }
    }

    fn as_wire(self) -> u8 {
        match self {
            Self::Chunk => 0x01,
            Self::Ack => 0x02,
            Self::Finish => 0x03,
            Self::Retransmit => 0x04,
            Self::Cancel => 0x05,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDataFrame {
    pub kind: FileDataFrameKind,
    pub index: u32,
    pub payload: Vec<u8>,
}

impl FileDataFrame {
    pub fn chunk(chunk_index: u32, payload: Vec<u8>) -> Self {
        Self {
            kind: FileDataFrameKind::Chunk,
            index: chunk_index,
            payload,
        }
    }

    pub fn ack(next_expected_index: u32) -> Self {
        Self {
            kind: FileDataFrameKind::Ack,
            index: next_expected_index,
            payload: Vec::new(),
        }
    }

    pub fn finish(total_chunks: u32) -> Self {
        Self {
            kind: FileDataFrameKind::Finish,
            index: total_chunks,
            payload: Vec::new(),
        }
    }

    pub fn retransmit(chunk_index: u32) -> Self {
        Self {
            kind: FileDataFrameKind::Retransmit,
            index: chunk_index,
            payload: Vec::new(),
        }
    }

    pub fn cancel(reason: &str) -> Self {
        Self {
            kind: FileDataFrameKind::Cancel,
            index: 0,
            payload: reason.as_bytes().to_vec(),
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(FILE_DATA_FRAME_HEADER_LEN + self.payload.len());
        bytes.push(FILE_DATA_FRAME_VERSION);
        bytes.push(self.kind.as_wire());
        bytes.extend_from_slice(&[0, 0]);
        bytes.extend_from_slice(&self.index.to_be_bytes());
        bytes.extend_from_slice(&self.payload);
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        if bytes.len() < FILE_DATA_FRAME_HEADER_LEN || bytes[0] != FILE_DATA_FRAME_VERSION {
            return None;
        }
        let kind = FileDataFrameKind::from_wire(bytes[1])?;
        let index = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
        Some(Self {
            kind,
            index,
            payload: bytes[FILE_DATA_FRAME_HEADER_LEN..].to_vec(),
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudClientEnvelope {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudServerEnvelope {
    pub id: Option<String>,
    #[serde(rename = "type")]
    pub message_type: String,
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(default)]
    pub correlation_id: Option<String>,
    pub payload: Option<Value>,
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOnlinePayload {
    pub name: String,
    #[serde(rename = "type")]
    pub device_type: String,
    pub business_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolHelloEnvelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub payload: ProtocolHelloPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolHelloPayload {
    pub device_id: String,
    pub protocol_version: String,
    pub extensions: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProtocolHelloAckEnvelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub payload: VersionAckPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionAckPayload {
    pub compatible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanEnvelope {
    pub id: String,
    #[serde(rename = "type")]
    pub message_type: String,
    pub from: String,
    pub to: String,
    pub seq: u64,
    pub timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub correlation_id: Option<String>,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthChallengePayload {
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponsePayload {
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingIdentityPayload {
    pub public_key: String,
    pub name: String,
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmptyPayload {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanRejectPayload {
    pub reason: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessNegotiatePayload {
    pub supported: Vec<String>,
    pub preferred: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessVersionPayload {
    pub business_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessVersionAckPayload {
    pub compatible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VersionCompatibility {
    pub compatible: bool,
    pub reason: Option<String>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessKeyExchangePayload {
    pub ephemeral_public_key: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemControlCommandPayload {
    pub action: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemControlAction {
    Sleep,
    Shutdown,
    Lock,
}

impl SystemControlAction {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "sleep" => Some(Self::Sleep),
            "shutdown" => Some(Self::Shutdown),
            "lock" => Some(Self::Lock),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sleep => "sleep",
            Self::Shutdown => "shutdown",
            Self::Lock => "lock",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsRootsPayload {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsRootsResultPayload {
    pub roots: Vec<FsRootEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsRootEntry {
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsListPayload {
    pub path: String,
    #[serde(default)]
    pub offset: Option<i64>,
    #[serde(default)]
    pub limit: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsListResultPayload {
    pub path: String,
    pub entries: Vec<FsEntry>,
    pub total: i64,
    pub offset: i64,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsEntry {
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
    pub readonly: bool,
    pub hidden: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsStatPayload {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsStatResultPayload {
    pub path: String,
    pub exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub modified: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub readonly: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsDownloadPayload {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FsErrorPayload {
    pub reason: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BusinessKeyExchangeNoncePayload {
    pub nonce: String,
}

pub fn check_lan_protocol_version(peer_version: &str) -> VersionCompatibility {
    check_semantic_major(
        LAN_PROTOCOL_VERSION,
        peer_version,
        "colink:protocol.invalid_version.v1",
        "colink:protocol.major_mismatch.v1",
        "LAN protocol",
    )
}

pub fn supports_lan_key_exchange(peer_version: &str) -> bool {
    match (semver(LAN_PROTOCOL_VERSION), semver(peer_version)) {
        (Some(local), Some(peer)) => {
            local.major == peer.major && local >= Semver::new(1, 1, 0) && peer >= Semver::new(1, 1, 0)
        }
        _ => false,
    }
}

pub fn supports_lan_key_exchange_nonce(peer_version: &str) -> bool {
    match (semver(LAN_PROTOCOL_VERSION), semver(peer_version)) {
        (Some(local), Some(peer)) => {
            local.major == peer.major && local >= Semver::new(1, 2, 0) && peer >= Semver::new(1, 2, 0)
        }
        _ => false,
    }
}

pub fn negotiated_lan_protocol_version(peer_version: &str) -> String {
    match (semver(LAN_PROTOCOL_VERSION), semver(peer_version)) {
        (Some(local), Some(peer)) => std::cmp::min(local, peer).to_wire(),
        _ => LAN_PROTOCOL_VERSION.to_string(),
    }
}

pub fn check_business_protocol_version(peer_version: &str) -> VersionCompatibility {
    check_semantic_major(
        BUSINESS_PROTOCOL_VERSION,
        peer_version,
        "colink:business.invalid_version.v1",
        "colink:business.major_mismatch.v1",
        "Business protocol",
    )
}

pub fn supports_business_protocol_at_least(
    peer_version: &str,
    major: u64,
    minor: u64,
    patch: u64,
) -> bool {
    match (semver(BUSINESS_PROTOCOL_VERSION), semver(peer_version)) {
        (Some(local), Some(peer)) => {
            let required = Semver::new(major, minor, patch);
            local.major == peer.major && local >= required && peer >= required
        }
        _ => false,
    }
}

fn check_semantic_major(
    local_version: &str,
    peer_version: &str,
    invalid_reason: &str,
    mismatch_reason: &str,
    label: &str,
) -> VersionCompatibility {
    let Some(local_major) = semver_major(local_version) else {
        return VersionCompatibility {
            compatible: false,
            reason: Some(invalid_reason.to_string()),
            message: Some(format!("{label} local version is invalid")),
        };
    };
    let Some(peer_major) = semver_major(peer_version) else {
        return VersionCompatibility {
            compatible: false,
            reason: Some(invalid_reason.to_string()),
            message: Some(format!("{label} peer version is invalid")),
        };
    };
    if local_major != peer_major {
        return VersionCompatibility {
            compatible: false,
            reason: Some(mismatch_reason.to_string()),
            message: Some(format!(
                "{label} major version {peer_major} is incompatible with local major version {local_major}"
            )),
        };
    }
    VersionCompatibility {
        compatible: true,
        reason: None,
        message: None,
    }
}

fn semver_major(value: &str) -> Option<u64> {
    semver(value).map(|version| version.major)
}

fn parse_semver_part(value: &str) -> Option<u64> {
    if value.is_empty() || (value.len() > 1 && value.starts_with('0')) {
        return None;
    }
    value.parse::<u64>().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Semver {
    major: u64,
    minor: u64,
    patch: u64,
}

impl Semver {
    const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self { major, minor, patch }
    }

    fn to_wire(self) -> String {
        format!("{}.{}.{}", self.major, self.minor, self.patch)
    }
}

fn semver(value: &str) -> Option<Semver> {
    let mut parts = value.trim().split('.');
    let version = Semver {
        major: parse_semver_part(parts.next()?)?,
        minor: parse_semver_part(parts.next()?)?,
        patch: parse_semver_part(parts.next()?)?,
    };
    parts.next().is_none().then_some(version)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EncryptedBusinessPayload {
    pub ciphertext: String,
    pub nonce: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwimEnvelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub payload: SwimPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SwimPayload {
    pub seq: u64,
    pub from: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub incarnation: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    pub gossip: Vec<SwimGossip>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SwimGossip {
    pub device_id: String,
    pub state: String,
    pub incarnation: i64,
}

#[cfg(test)]
mod tests {
    use super::{
        BusinessEnvelope, FileDataFrame, FileDataFrameKind, MusicLyricLinePayload,
        MusicLyricPayload, MusicProgressPayload, MusicTrackPayload, MUSIC_LYRIC_TYPE,
        MUSIC_PROGRESS_TYPE, MUSIC_TRACK_TYPE,
    };

    #[test]
    fn encodes_and_decodes_file_data_frame() {
        let frame = FileDataFrame::chunk(7, vec![1, 2, 3]);
        let encoded = frame.encode();

        assert_eq!(encoded[..8], [1, 1, 0, 0, 0, 0, 0, 7]);
        assert_eq!(FileDataFrame::decode(&encoded), Some(frame));
    }

    #[test]
    fn rejects_invalid_file_data_frame() {
        assert_eq!(FileDataFrame::decode(&[2, 1, 0, 0, 0, 0, 0, 1]), None);
        assert_eq!(FileDataFrame::decode(&[1, 9, 0, 0, 0, 0, 0, 1]), None);
        assert_eq!(FileDataFrame::decode(&[1, 1, 0]), None);
    }

    #[test]
    fn builds_control_frames() {
        assert_eq!(FileDataFrame::ack(3).kind, FileDataFrameKind::Ack);
        assert_eq!(FileDataFrame::finish(4).index, 4);
        assert_eq!(FileDataFrame::cancel("stop").payload, b"stop");
    }

    #[test]
    fn serializes_music_track_payload() {
        let payload = MusicTrackPayload {
            track_id: Some("abc123".into()),
            title: Some("Song Title".into()),
            artists: Some(vec!["Artist A".into(), "Artist B".into()]),
            album: Some("Album".into()),
            source: Some("ncm".into()),
            cover_url: Some("https://example.com/cover.jpg".into()),
            cover_data: Some("iVBORw0KGgoAAAANSUhEUgAA".into()),
            duration: Some(234500),
        };

        assert_eq!(
            serde_json::to_value(
                BusinessEnvelope::from_payload(MUSIC_TRACK_TYPE, payload).unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "type": "music.v1.track",
                "payload": {
                    "trackId": "abc123",
                    "title": "Song Title",
                    "artists": ["Artist A", "Artist B"],
                    "album": "Album",
                    "source": "ncm",
                    "coverUrl": "https://example.com/cover.jpg",
                    "coverData": "iVBORw0KGgoAAAANSUhEUgAA",
                    "duration": 234500,
                }
            }),
        );
    }

    #[test]
    fn serializes_music_lyric_payload() {
        let payload = MusicLyricPayload {
            track_id: "abc123".into(),
            lines: Some(vec![MusicLyricLinePayload {
                time: 12_500,
                text: "First line".into(),
            }]),
            translated_lines: Some(vec![MusicLyricLinePayload {
                time: 12_500,
                text: "第一行".into(),
            }]),
        };

        assert_eq!(
            serde_json::to_value(
                BusinessEnvelope::from_payload(MUSIC_LYRIC_TYPE, payload).unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "type": "music.v1.lyric",
                "payload": {
                    "trackId": "abc123",
                    "lines": [{"time": 12500, "text": "First line"}],
                    "translatedLines": [{"time": 12500, "text": "第一行"}],
                }
            }),
        );
    }

    #[test]
    fn serializes_music_progress_payload() {
        let payload = MusicProgressPayload {
            track_id: "abc123".into(),
            progress: 45_200,
            paused: false,
        };

        assert_eq!(
            serde_json::to_value(
                BusinessEnvelope::from_payload(MUSIC_PROGRESS_TYPE, payload).unwrap()
            )
            .unwrap(),
            serde_json::json!({
                "type": "music.v1.progress",
                "payload": {
                    "trackId": "abc123",
                    "progress": 45200,
                    "paused": false,
                }
            }),
        );
    }
}
