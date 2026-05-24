use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const TEXT_MESSAGE_TYPE: &str = "message.v1.text";
pub const CLIPBOARD_SYNC_TYPE: &str = "clipboard.v1.sync";
pub const FILE_OFFER_TYPE: &str = "file.v1.offer";
pub const FILE_ACCEPT_TYPE: &str = "file.v1.accept";
pub const FILE_REJECT_TYPE: &str = "file.v1.reject";
pub const FILE_CHUNK_TYPE: &str = "file.v1.chunk";
pub const FILE_DONE_TYPE: &str = "file.v1.done";
pub const FILE_CANCEL_TYPE: &str = "file.v1.cancel";

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
    pub file_id: String,
    pub file_name: String,
    pub file_size: i64,
    pub total_chunks: i64,
    pub chunk_size: i64,
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileAcceptPayload {
    pub file_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileRejectPayload {
    pub file_id: String,
    pub reason: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileChunkPayload {
    pub file_id: String,
    pub index: i64,
    pub total_chunks: i64,
    pub data: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileDonePayload {
    pub file_id: String,
    pub success: bool,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCancelPayload {
    pub file_id: String,
    pub reason: String,
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
    pub payload: Option<Value>,
    pub timestamp: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnnouncePayload {
    pub local_ip: String,
    pub local_port: u16,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeviceOnlinePayload {
    pub name: String,
    #[serde(rename = "type")]
    pub device_type: String,
    pub local_ip: Option<String>,
    pub local_port: Option<u16>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerEnvelope {
    #[serde(rename = "type")]
    pub message_type: String,
    pub payload: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRequestPayload {
    pub device_id: String,
    pub timestamp: i64,
    pub nonce: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthResponsePayload {
    pub device_id: String,
    pub timestamp: i64,
    pub nonce: String,
    pub peer_nonce: String,
    pub signature: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthFailPayload {
    pub reason: String,
}
