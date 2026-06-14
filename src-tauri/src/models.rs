use serde::{Deserialize, Serialize};

const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8080";
const TOKEN_EXPIRY_BUFFER_SECONDS: i64 = 60;
pub const LAN_PORT: u16 = 27_777;
pub const MAX_TEXT_LENGTH: usize = 10_000;
pub const FILE_CHUNK_SIZE: usize = 1_048_576;
pub const CLIPBOARD_MAX_BYTES: usize = 1_048_576;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub server_url: String,
    pub auto_start: bool,
    pub start_minimized: bool,
    pub lan_discovery: bool,
    pub download_path: String,
    pub notifications: bool,
    pub clipboard_sync: bool,
    pub language: String,
}

impl AppSettings {
    pub fn new(download_path: String) -> Self {
        Self {
            server_url: DEFAULT_SERVER_URL.to_string(),
            auto_start: true,
            start_minimized: true,
            lan_discovery: true,
            download_path,
            notifications: true,
            clipboard_sync: true,
            language: crate::i18n::default_language_code(),
        }
    }

    pub fn normalize(mut self) -> Self {
        self.server_url = self.server_url.trim().trim_end_matches('/').to_string();
        self.download_path = self.download_path.trim().to_string();
        self.language = crate::i18n::resolve_language(Some(&self.language)).to_string();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub user_id: String,
    pub username: String,
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at: i64,
}

impl SessionRecord {
    pub fn is_expiring_soon(&self) -> bool {
        self.access_token_expires_at <= unix_now() + TOKEN_EXPIRY_BUFFER_SECONDS
    }

    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            user_id: self.user_id.clone(),
            username: self.username.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub user_id: String,
    pub username: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub user_id: Option<String>,
    pub device_id: String,
    pub device_secret: Option<String>,
    pub name: String,
    pub device_type: String,
    pub public_key: String,
    pub private_key: String,
    pub cloud_key_sync_pending: bool,
}

impl DeviceIdentity {
    pub fn normalize(mut self) -> Self {
        self.user_id = normalize_optional_string(self.user_id);
        self.device_secret = normalize_optional_string(self.device_secret);
        self.name = self.name.trim().to_string();
        self.device_type = self.device_type.trim().to_string();
        self.public_key = self.public_key.trim().to_string();
        self.private_key = self.private_key.trim().to_string();
        self
    }

    pub fn summary(&self) -> LocalDeviceSummary {
        LocalDeviceSummary {
            device_id: self.device_id.clone(),
            name: self.name.clone(),
            device_type: self.device_type.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalDeviceSummary {
    pub device_id: String,
    pub name: String,
    pub device_type: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceInfo {
    pub device_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub device_type: String,
    pub online: bool,
    pub cloud_available: bool,
    pub last_seen: Option<String>,
    pub public_key: String,
    pub public_key_updated_at: Option<i64>,
    pub local_ip: Option<String>,
    pub local_port: Option<u16>,
    pub lan_available: bool,
    pub lan_state: String,
    pub active_route: Option<String>,
    pub device_sources: Vec<String>,
    pub trusted_by_lan: bool,
    pub trusted_by_cloud: bool,
    pub security_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedPeerKeyRecord {
    pub device_id: String,
    pub name: String,
    pub public_key: String,
    pub key_updated_at: i64,
    pub trusted_by_lan: bool,
    pub trusted_by_cloud: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicProviderConfig {
    pub id: String,
    pub enabled: bool,
    pub priority: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MusicProviderMeta {
    pub id: String,
    pub name: String,
    pub implemented: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanPairingCandidate {
    pub device_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub device_type: String,
    pub ip: String,
    pub port: u16,
    pub state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanPairingRequest {
    pub request_id: String,
    pub device_id: String,
    pub name: String,
    pub code: String,
    pub reason: String,
    pub public_key: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanPairingCompleted {
    pub request_id: String,
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanPairingFailed {
    pub request_id: String,
    pub device_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanPairingDecisionPayload {
    pub request_id: String,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StartLanPairingPayload {
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudStatus {
    pub state: String,
    pub connected: bool,
    pub attempt: u32,
    pub last_error: Option<String>,
}

impl CloudStatus {
    pub fn disconnected() -> Self {
        Self {
            state: "disconnected".to_string(),
            connected: false,
            attempt: 0,
            last_error: None,
        }
    }

    pub fn connecting() -> Self {
        Self {
            state: "connecting".to_string(),
            connected: false,
            attempt: 0,
            last_error: None,
        }
    }

    pub fn connected() -> Self {
        Self {
            state: "connected".to_string(),
            connected: true,
            attempt: 0,
            last_error: None,
        }
    }

    pub fn reconnecting(attempt: u32, last_error: Option<String>) -> Self {
        Self {
            state: "reconnecting".to_string(),
            connected: false,
            attempt,
            last_error,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextMessageRecord {
    pub message_id: String,
    pub device_id: String,
    pub direction: String,
    pub text: String,
    pub route: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransferRecord {
    pub file_id: String,
    pub device_id: String,
    pub direction: String,
    pub file_name: String,
    pub file_size: i64,
    pub transferred_bytes: i64,
    pub total_chunks: i64,
    pub status: String,
    pub checksum: String,
    pub route: String,
    pub temp_path: Option<String>,
    pub final_path: Option<String>,
    pub error: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOfferRequest {
    pub session_id: String,
    pub device_id: String,
    pub device_name: String,
    pub file_name: String,
    pub file_size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileOfferDecisionPayload {
    pub session_id: String,
    pub accepted: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppLogEntry {
    pub id: String,
    pub level: String,
    pub source: String,
    pub message: String,
    pub created_at: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapPayload {
    pub settings: AppSettings,
    pub session: Option<SessionSummary>,
    pub devices: Vec<DeviceInfo>,
    pub device: Option<LocalDeviceSummary>,
    pub cloud: CloudStatus,
    pub messages: Vec<TextMessageRecord>,
    pub transfers: Vec<FileTransferRecord>,
    pub logs: Vec<AppLogEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateRelease {
    pub version: String,
    pub release_notes: String,
    pub published_at: String,
    pub assets: Vec<AppUpdateAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppUpdateAsset {
    pub name: String,
    pub size: i64,
    pub download_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoginPayload {
    pub identifier: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLoginCredentials {
    pub identifier: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegisterPayload {
    pub email: String,
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendTextPayload {
    pub device_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceNameUpdatePayload {
    pub device_id: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceDeletePayload {
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RotateDeviceKeyPayload {
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SendFilePayload {
    pub device_id: String,
    pub paths: Vec<String>,
}

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}

pub fn unix_now_millis() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis() as i64)
        .unwrap_or_default()
}

fn normalize_optional_string(value: Option<String>) -> Option<String> {
    value
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}
