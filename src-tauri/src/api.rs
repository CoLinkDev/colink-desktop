use serde::{Deserialize, Serialize};

use crate::models::DeviceInfo;

pub(crate) const AUTH_REFRESH_PATH: &str = "/api/v1/auth/refresh";
pub(crate) const DEVICES_PATH: &str = "/api/v1/devices";
pub(crate) const ACCESS_TOKEN_TTL_SECONDS: i64 = 15 * 60;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RefreshResponse {
    pub(crate) token: String,
    pub(crate) refresh_token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RefreshRequest<'a> {
    pub(crate) refresh_token: &'a str,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DeviceListResponse {
    pub(crate) devices: Vec<CloudDeviceRecord>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CloudDeviceRecord {
    pub(crate) device_id: String,
    pub(crate) name: String,
    #[serde(rename = "type")]
    pub(crate) device_type: String,
    pub(crate) online: bool,
    pub(crate) last_seen: Option<String>,
    pub(crate) public_key: String,
    pub(crate) public_key_updated_at: Option<String>,
}

impl From<CloudDeviceRecord> for DeviceInfo {
    fn from(record: CloudDeviceRecord) -> Self {
        Self {
            device_id: record.device_id,
            name: record.name,
            device_type: record.device_type,
            online: record.online,
            cloud_available: false,
            last_seen: record.last_seen,
            public_key: record.public_key,
            public_key_updated_at: parse_timestamp_millis(record.public_key_updated_at),
            lan_available: false,
            lan_state: "unavailable".to_string(),
            active_route: None,
            device_sources: Vec::new(),
            security_state: "unverified".to_string(),
        }
    }
}

fn parse_timestamp_millis(value: Option<String>) -> Option<i64> {
    value.and_then(|raw| {
        chrono::DateTime::parse_from_rfc3339(raw.trim())
            .ok()
            .map(|timestamp| timestamp.timestamp_millis())
    })
}

impl DeviceListResponse {
    pub(crate) fn into_devices(self) -> Vec<DeviceInfo> {
        self.devices.into_iter().map(Into::into).collect()
    }
}
