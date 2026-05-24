use serde::{Deserialize, Serialize};

const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:8080";

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapPayload {
    pub settings: AppSettings,
    pub session: Option<SessionSummary>,
    pub devices: Vec<DeviceInfo>,
    pub device: Option<LocalDeviceSummary>,
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

pub fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_secs() as i64)
        .unwrap_or_default()
}
