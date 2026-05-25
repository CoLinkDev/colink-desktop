use serde::{Deserialize, Serialize};

const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8080";
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
        }
    }

    pub fn normalize(mut self) -> Self {
        self.server_url = self.server_url.trim().trim_end_matches('/').to_string();
        self.download_path = self.download_path.trim().to_string();
        self
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionRecord {
    pub user_id: String,
    pub access_token: String,
    pub refresh_token: String,
    pub access_token_expires_at: i64,
}

impl SessionRecord {
    pub fn is_expiring_soon(&self) -> bool {
        self.access_token_expires_at <= unix_now() + 60
    }

    pub fn summary(&self) -> SessionSummary {
        SessionSummary {
            user_id: self.user_id.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub user_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceIdentity {
    pub user_id: String,
    pub device_id: String,
    pub device_secret: String,
    pub name: String,
    pub device_type: String,
    pub public_key: String,
    pub private_key: String,
}

impl DeviceIdentity {
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
    pub last_seen: Option<String>,
    pub public_key: String,
    #[serde(default)]
    pub local_ip: Option<String>,
    #[serde(default)]
    pub local_port: Option<u16>,
    #[serde(default)]
    pub lan_available: bool,
    #[serde(default)]
    pub active_route: Option<String>,
    #[serde(default = "default_security_state")]
    pub security_state: String,
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
pub struct LoginPayload {
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

fn default_security_state() -> String {
    "unverified".to_string()
}
