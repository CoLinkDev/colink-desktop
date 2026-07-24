use crate::{
    models::{
        DeviceInfo, LanPairingCandidate, LanPairingCompleted, LanPairingFailed, LanPairingRequest,
    },
    protocol::{BusinessEnvelope, ClipboardSyncPayload, DeviceOnlinePayload, FileDataFrame},
};

#[derive(Debug, Clone)]
pub struct CorrelatedBusinessMessage {
    pub message: BusinessEnvelope,
    pub envelope_id: Option<String>,
    pub correlation_id: Option<String>,
}

#[derive(Debug, Clone)]
pub enum RuntimeEvent {
    AuthInvalidated(String),
    CloudConnected,
    CloudDisconnected(Option<String>),
    CloudUnavailable,
    CloudRelay {
        from: String,
        envelope_id: Option<String>,
        correlation_id: Option<String>,
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
    LanDeviceReachable {
        device_id: String,
    },
    LanDeviceUnreachable {
        device_id: String,
    },
    LanDeviceStateChanged {
        device_id: String,
    },
    LanKeyChanged {
        device_id: String,
        name: String,
    },
    LanSendFailed {
        device_id: String,
        messages: Vec<CorrelatedBusinessMessage>,
    },
    LanMessage {
        from: String,
        envelope_id: String,
        correlation_id: Option<String>,
        message: BusinessEnvelope,
    },
    LanTransferFrame {
        session_id: String,
        frame: FileDataFrame,
    },
    LanTransferClosed {
        session_id: String,
    },
    LanCameraFramesReady {
        session_id: String,
    },
    LanCameraConnected {
        session_id: String,
    },
    LanCameraClosed {
        session_id: String,
    },
    NativeCameraFramesReady {
        session_id: String,
    },
    NativeCameraStopped {
        session_id: String,
        generation: u64,
    },
    NativeCameraFailed {
        session_id: String,
        generation: u64,
        message: String,
    },
    LanPairingRequested(LanPairingRequest),
    LanPairingCompleted(LanPairingCompleted),
    LanPairingFailed(LanPairingFailed),
    LanPairingCandidatesUpdated(Vec<LanPairingCandidate>),
    ClipboardChanged(ClipboardSyncPayload),
    Log {
        level: String,
        source: String,
        message: String,
    },
}
