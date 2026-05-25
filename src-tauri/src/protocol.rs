use serde::{Deserialize, Serialize};
use serde_json::Value;

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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileCancelPayload {
    pub session_id: String,
    pub reason: String,
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

#[cfg(test)]
mod tests {
    use super::{FileDataFrame, FileDataFrameKind};

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
}
