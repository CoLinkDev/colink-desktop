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
    pub(crate) devices: Vec<DeviceInfo>,
}
