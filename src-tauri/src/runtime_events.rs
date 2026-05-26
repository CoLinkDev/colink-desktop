use crate::{
    models::DeviceInfo,
    protocol::{BusinessEnvelope, ClipboardSyncPayload, DeviceOnlinePayload, FileDataFrame},
};

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    AuthInvalidated(String),
    CloudConnected,
    CloudDisconnected(Option<String>),
    CloudRelay {
        from: String,
        message: BusinessEnvelope,
    },
    DevicePresence {
        device_id: String,
        online: bool,
        payload: Option<DeviceOnlinePayload>,
    },
    DevicesSnapshot(Vec<DeviceInfo>),
    LanDiscovered {
        device_id: String,
        ip: String,
        port: u16,
        source: String,
    },
    LanConnected {
        device_id: String,
    },
    LanDisconnected {
        device_id: String,
    },
    LanMessage {
        from: String,
        message: BusinessEnvelope,
    },
    LanTransferFrame {
        session_id: String,
        frame: FileDataFrame,
    },
    LanTransferClosed {
        session_id: String,
    },
    LocalEndpoint {
        ip: String,
        port: u16,
    },
    ClipboardChanged(ClipboardSyncPayload),
    Log {
        level: String,
        source: String,
        message: String,
    },
}
