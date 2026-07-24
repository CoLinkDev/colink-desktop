use std::{collections::HashMap, sync::{Arc, Mutex}, time::{Duration, Instant}};

use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use tauri::Emitter;
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::{
    error::{AppError, AppResult},
    protocol::{
        BusinessEnvelope, CameraAlivePayload, CameraClosePayload, CameraConfigAckPayload,
        CameraConfigPayload, CameraEntry, CameraFramePayload, CameraListPayload,
        CameraListResultPayload, CameraOpenAckPayload, CameraOpenPayload, CameraReadyPayload,
        CAMERA_ALIVE_TYPE, CAMERA_CLOSE_TYPE, CAMERA_CONFIG_ACK_TYPE, CAMERA_FRAME_TYPE,
        CAMERA_LIST_TYPE, CAMERA_OPEN_ACK_TYPE, CAMERA_OPEN_TYPE, CAMERA_READY_TYPE,
    },
    runtime::camera_capture::{CameraCaptureProfile, CameraCaptureRequest},
};

use super::AppRuntime;

const CAMERA_MINIMUM_MINOR: u64 = 10;
const CAMERA_LIST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteCameraSupport {
    Unknown,
    Supported,
    Unsupported,
}

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CameraUiEvent {
    pub session_id: String,
    pub kind: String,
    pub data: Option<String>,
    pub codec: Option<String>,
    pub transport: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fps: Option<u32>,
    pub keyframe: Option<bool>,
    pub sequence: Option<u64>,
    pub timestamp_ms: Option<u64>,
    pub message: Option<String>,
}

#[derive(Clone)]
pub(super) struct CameraManager {
    remote_sessions: Arc<Mutex<HashMap<String, RemoteCameraSession>>>,
    pending_lists: Arc<Mutex<HashMap<String, PendingCameraList>>>,
    host_sessions: Arc<Mutex<HashMap<String, HostCameraSession>>>,
}

struct RemoteCameraSession {
    device_id: String,
    open_request_id: String,
    open: bool,
    codec: Option<String>,
}

struct PendingCameraList {
    device_id: String,
    sender: oneshot::Sender<AppResult<Vec<CameraEntry>>>,
}

struct HostCameraSession {
    device_id: String,
    camera_id: String,
    width: u32,
    height: u32,
    fps: u32,
    ready: bool,
    last_alive: Option<Instant>,
    sequence: u64,
    started_at: Instant,
    transport: Option<String>,
    pending_lan_ready: bool,
    capture_running: bool,
    capture_stopping: bool,
    restart_capture: bool,
    capture_generation: u64,
}

struct HostCameraSessionInfo {
    session_id: String,
    width: u32,
    height: u32,
    fps: u32,
}

struct HostCameraConfigUpdate {
    width: u32,
    height: u32,
    fps: u32,
    stop_capture: bool,
}

struct HostCameraPreferences {
    camera_id: String,
    width: u32,
    height: u32,
    fps: u32,
}

impl CameraManager {
    pub(super) fn new() -> Self {
        Self {
            remote_sessions: Arc::new(Mutex::new(HashMap::new())),
            pending_lists: Arc::new(Mutex::new(HashMap::new())),
            host_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn register_remote_session(&self, device_id: &str, session_id: &str, request_id: String) {
        if let Ok(mut sessions) = self.remote_sessions.lock() {
            sessions.insert(session_id.to_string(), RemoteCameraSession {
                device_id: device_id.to_string(),
                open_request_id: request_id,
                open: false,
                codec: None,
            });
        }
    }

    fn discard_remote_session(&self, session_id: &str) {
        if let Ok(mut sessions) = self.remote_sessions.lock() {
            sessions.remove(session_id);
        }
    }

    fn accept_remote_session(&self, device_id: &str, payload: &CameraOpenAckPayload, correlation_id: Option<&str>) -> bool {
        let Ok(mut sessions) = self.remote_sessions.lock() else { return false; };
        let Some(session) = sessions.get_mut(&payload.session_id) else { return false; };
        if session.device_id != device_id || session.open_request_id != correlation_id.unwrap_or_default() || !payload.accepted {
            return false;
        }
        session.open = true;
        session.codec = payload.negotiated_codec.clone();
        true
    }

    fn remote_session_codec(&self, device_id: &str, session_id: &str) -> Option<String> {
        self.remote_sessions.lock().ok().and_then(|sessions| {
            sessions.get(session_id)
                .filter(|session| session.device_id == device_id && session.open)
                .and_then(|session| session.codec.clone())
        })
    }

    fn remote_session_codec_by_id(&self, session_id: &str) -> Option<String> {
        self.remote_sessions.lock().ok().and_then(|sessions| {
            sessions.get(session_id)
                .filter(|session| session.open)
                .and_then(|session| session.codec.clone())
        })
    }

    fn close_remote_session_by_id(&self, session_id: &str) -> bool {
        self.remote_sessions
            .lock()
            .ok()
            .and_then(|mut sessions| sessions.remove(session_id))
            .is_some()
    }

    fn close_remote_session(&self, device_id: &str, session_id: &str) -> bool {
        let Ok(mut sessions) = self.remote_sessions.lock() else { return false; };
        let Some(session) = sessions.get(session_id) else { return false; };
        if session.device_id != device_id { return false; }
        sessions.remove(session_id);
        true
    }

    fn close_for_device(&self, device_id: &str) -> (Vec<String>, Vec<String>) {
        let mut remote_ids = Vec::new();
        if let Ok(mut sessions) = self.remote_sessions.lock() {
            sessions.retain(|session_id, session| {
                if session.device_id == device_id {
                    remote_ids.push(session_id.clone());
                    false
                } else {
                    true
                }
            });
        }
        if let Ok(mut lists) = self.pending_lists.lock() {
            lists.retain(|_, pending| pending.device_id != device_id);
        }
        let host_ids = self.host_sessions.lock().ok().map(|mut sessions| {
            let ids = sessions.iter().filter_map(|(session_id, session)| {
                (session.device_id == device_id).then(|| session_id.clone())
            }).collect::<Vec<_>>();
            for session_id in &ids {
                sessions.remove(session_id);
            }
            ids
        }).unwrap_or_default();
        (remote_ids, host_ids)
    }

    pub(super) fn close_all_host_sessions(&self) -> Vec<String> {
        if let Ok(mut sessions) = self.host_sessions.lock() {
            return sessions.drain().map(|(session_id, _)| session_id).collect();
        }
        Vec::new()
    }

    fn open_host_session(
        &self,
        device_id: &str,
        payload: &CameraOpenPayload,
        profile: CameraCaptureProfile,
    ) -> Result<HostCameraSessionInfo, (&'static str, &'static str)> {
        if !payload.preferred_codecs.iter().any(|codec| codec == "h264") {
            return Err(("colink:camera.no_common_codec.v1", "No H.264 camera codec was offered"));
        }
        let mut sessions = self.host_sessions.lock().map_err(|_| (
            "colink:camera.generic.v1",
            "camera state unavailable",
        ))?;
        if sessions.contains_key(&payload.session_id) {
            return Err(("colink:camera.session_conflict.v1", "camera session already exists"));
        }
        sessions.insert(payload.session_id.clone(), HostCameraSession {
            device_id: device_id.to_string(),
            camera_id: payload.camera_id.clone(),
            width: profile.width,
            height: profile.height,
            fps: profile.fps,
            ready: false,
            last_alive: None,
            sequence: 0,
            started_at: Instant::now(),
            transport: None,
            pending_lan_ready: false,
            capture_running: false,
            capture_stopping: false,
            restart_capture: false,
            capture_generation: 0,
        });
        Ok(HostCameraSessionInfo {
            session_id: payload.session_id.clone(),
            width: profile.width,
            height: profile.height,
            fps: profile.fps,
        })
    }

    fn mark_host_ready(
        &self,
        device_id: &str,
        session_id: &str,
        transport: &str,
        lan_connected: bool,
    ) -> bool {
        if !matches!(transport, "lan" | "relay") { return false; }
        let Ok(mut sessions) = self.host_sessions.lock() else { return false; };
        let Some(session) = sessions.get_mut(session_id) else { return false; };
        if session.device_id != device_id || session.ready || session.pending_lan_ready {
            return false;
        }
        if transport == "lan" && !lan_connected {
            session.pending_lan_ready = true;
            return false;
        }
        session.ready = true;
        session.transport = Some(transport.to_string());
        true
    }

    fn mark_host_lan_connected(&self, session_id: &str) -> bool {
        let Ok(mut sessions) = self.host_sessions.lock() else { return false; };
        let Some(session) = sessions.get_mut(session_id) else { return false; };
        if session.ready || !session.pending_lan_ready {
            return false;
        }
        session.pending_lan_ready = false;
        session.ready = true;
        session.transport = Some("lan".to_string());
        true
    }

    fn begin_host_capture(&self, session_id: &str) -> Option<CameraCaptureRequest> {
        let mut sessions = self.host_sessions.lock().ok()?;
        let session = sessions.get_mut(session_id)?;
        if !session.ready || session.capture_running || session.capture_stopping { return None; }
        session.capture_running = true;
        session.restart_capture = false;
        session.capture_generation = session.capture_generation.saturating_add(1);
        Some(CameraCaptureRequest {
            session_id: session_id.to_string(),
            generation: session.capture_generation,
            camera_id: session.camera_id.clone(),
            width: session.width,
            height: session.height,
            fps: session.fps,
        })
    }

    fn reconfigure_host_session(
        &self,
        device_id: &str,
        payload: &CameraConfigPayload,
        profile: CameraCaptureProfile,
    ) -> Result<HostCameraConfigUpdate, (&'static str, &'static str)> {
        let mut sessions = self.host_sessions.lock().map_err(|_| (
            "colink:camera.generic.v1",
            "camera state unavailable",
        ))?;
        let Some(session) = sessions.get_mut(&payload.session_id) else {
            return Err(("colink:camera.session_not_found.v1", "camera session does not exist"));
        };
        if session.device_id != device_id {
            return Err(("colink:camera.session_not_found.v1", "camera session does not exist"));
        }
        session.width = profile.width;
        session.height = profile.height;
        session.fps = profile.fps;
        let stop_capture = session.capture_running;
        if stop_capture {
            session.capture_running = false;
            session.capture_stopping = true;
            session.restart_capture = true;
        } else if session.capture_stopping {
            session.restart_capture = true;
        }
        Ok(HostCameraConfigUpdate {
            width: session.width,
            height: session.height,
            fps: session.fps,
            stop_capture,
        })
    }

    fn host_camera_preferences(
        &self,
        device_id: &str,
        payload: &CameraConfigPayload,
    ) -> Result<HostCameraPreferences, (&'static str, &'static str)> {
        let sessions = self.host_sessions.lock().map_err(|_| (
            "colink:camera.generic.v1",
            "camera state unavailable",
        ))?;
        let Some(session) = sessions.get(&payload.session_id) else {
            return Err(("colink:camera.session_not_found.v1", "camera session does not exist"));
        };
        if session.device_id != device_id {
            return Err(("colink:camera.session_not_found.v1", "camera session does not exist"));
        }
        Ok(HostCameraPreferences {
            camera_id: session.camera_id.clone(),
            width: payload.width.unwrap_or(session.width).clamp(160, 1280),
            height: payload.height.unwrap_or(session.height).clamp(120, 720),
            fps: payload.fps.unwrap_or(session.fps).clamp(1, 24),
        })
    }

    fn receive_host_alive(&self, device_id: &str, session_id: &str) -> bool {
        if let Ok(mut sessions) = self.host_sessions.lock() {
            if let Some(session) = sessions.get_mut(session_id).filter(|session| session.device_id == device_id) {
                session.last_alive = Some(Instant::now());
                return true;
            }
        }
        false
    }

    fn next_host_frame(&self, session_id: &str, generation: u64) -> Option<(String, String, u64, u64)> {
        let mut sessions = self.host_sessions.lock().ok()?;
        let session = sessions.get_mut(session_id)?;
        if !session.capture_running
            || session.capture_generation != generation
            || !session.ready
            || !session.last_alive.is_some_and(|alive| alive.elapsed() <= Duration::from_secs(15))
        {
            return None;
        }
        let sequence = session.sequence;
        session.sequence = session.sequence.saturating_add(1);
        Some((
            session.device_id.clone(),
            session.transport.clone()?,
            sequence,
            session.started_at.elapsed().as_millis() as u64,
        ))
    }

    fn close_host_session(&self, device_id: &str, session_id: &str) -> bool {
        let Ok(mut sessions) = self.host_sessions.lock() else { return false; };
        if sessions.get(session_id).is_none_or(|session| session.device_id != device_id) {
            return false;
        }
        sessions.remove(session_id);
        true
    }

    fn close_host_session_by_id(&self, session_id: &str) -> Option<String> {
        self.host_sessions.lock().ok()?.remove(session_id).map(|session| session.device_id)
    }

    fn fail_host_capture(&self, session_id: &str, generation: u64) -> Option<String> {
        let mut sessions = self.host_sessions.lock().ok()?;
        let session = sessions.get(session_id)?;
        if !session.capture_running || session.capture_generation != generation {
            return None;
        }
        sessions.remove(session_id).map(|session| session.device_id)
    }

    fn finish_host_capture(&self, session_id: &str, generation: u64) -> Option<CameraCaptureRequest> {
        let mut sessions = self.host_sessions.lock().ok()?;
        let session = sessions.get_mut(session_id)?;
        if session.capture_generation != generation {
            return None;
        }
        session.capture_running = false;
        session.capture_stopping = false;
        if !session.restart_capture
            || !session.ready
            || !session.last_alive.is_some_and(|alive| alive.elapsed() <= Duration::from_secs(15))
        {
            session.restart_capture = false;
            return None;
        }
        session.restart_capture = false;
        session.capture_running = true;
        session.capture_generation = session.capture_generation.saturating_add(1);
        Some(CameraCaptureRequest {
            session_id: session_id.to_string(),
            generation: session.capture_generation,
            camera_id: session.camera_id.clone(),
            width: session.width,
            height: session.height,
            fps: session.fps,
        })
    }

    fn host_session_exists(&self, session_id: &str) -> bool {
        self.host_sessions.lock().is_ok_and(|sessions| sessions.contains_key(session_id))
    }

    fn expire_host_session(&self, session_id: &str) -> Option<String> {
        let expired = self.host_sessions.lock().ok().and_then(|sessions| {
            sessions.get(session_id).map(|session| {
                session.last_alive.map_or_else(
                    || session.started_at.elapsed() > Duration::from_secs(15),
                    |alive| alive.elapsed() > Duration::from_secs(15),
                )
            })
        }).unwrap_or(false);
        expired.then(|| self.close_host_session_by_id(session_id)).flatten()
    }
}

impl AppRuntime {
    pub(super) fn handle_camera_device_disconnected(&self, device_id: &str) {
        let (remote_ids, host_ids) = self.inner.camera.close_for_device(device_id);
        for session_id in remote_ids {
            self.inner.lan.unregister_camera(&session_id);
            self.emit_camera_event(CameraUiEvent {
                session_id,
                kind: "closed".to_string(),
                data: None,
                codec: None,
                transport: None,
                width: None,
                height: None,
                fps: None,
                keyframe: None,
                sequence: None,
                timestamp_ms: None,
                message: Some("Camera device disconnected".to_string()),
            });
        }
        for session_id in host_ids {
            self.inner.camera_capture.stop(&session_id);
            self.inner.lan.unregister_camera(&session_id);
        }
    }

    pub fn remote_camera_support(&self, device_id: &str) -> RemoteCameraSupport {
        match self.peer_business_version(device_id) {
            None => RemoteCameraSupport::Unknown,
            Some(version) if crate::protocol::supports_business_protocol_at_least(&version, 1, CAMERA_MINIMUM_MINOR, 0) => RemoteCameraSupport::Supported,
            Some(_) => RemoteCameraSupport::Unsupported,
        }
    }

    pub async fn list_remote_cameras(&self, device_id: &str) -> AppResult<Vec<CameraEntry>> {
        if !matches!(self.remote_camera_support(device_id), RemoteCameraSupport::Supported) {
            return Err(AppError::message("remote device does not support camera streaming"));
        }
        let request_id = Uuid::new_v4().to_string();
        let (sender, receiver) = oneshot::channel();
        self.inner.camera.pending_lists.lock().map_err(|_| AppError::message("camera state unavailable"))?.insert(request_id.clone(), PendingCameraList {
            device_id: device_id.to_string(), sender,
        });
        let message = BusinessEnvelope::from_payload(CAMERA_LIST_TYPE, CameraListPayload {})?;
        if let Err(error) = self.send_business_message_with_envelope_id(device_id, message, request_id.clone()).await {
            self.inner.camera.pending_lists.lock().ok().and_then(|mut lists| lists.remove(&request_id));
            return Err(error);
        }
        match tokio::time::timeout(CAMERA_LIST_TIMEOUT, receiver).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(AppError::message("remote camera list request was cancelled")),
            Err(_) => {
                self.inner.camera.pending_lists.lock().ok().and_then(|mut lists| lists.remove(&request_id));
                Err(AppError::message("remote camera list request timed out"))
            }
        }
    }

    pub async fn open_remote_camera(&self, device_id: &str, camera_id: String, preferred_codecs: Vec<String>) -> AppResult<String> {
        if !matches!(self.remote_camera_support(device_id), RemoteCameraSupport::Supported) {
            return Err(AppError::message("remote device does not support camera streaming"));
        }
        let session_id = Uuid::new_v4().to_string();
        let request_id = Uuid::new_v4().to_string();
        let lan_available = self.inner.lan.is_available(device_id);
        let preferred_width = if lan_available { 960 } else { 640 };
        let preferred_height = if lan_available { 540 } else { 360 };
        let preferred_fps = if lan_available { 15 } else { 8 };
        let message = BusinessEnvelope::from_payload(CAMERA_OPEN_TYPE, CameraOpenPayload {
            session_id: session_id.clone(),
            camera_id,
            preferred_codecs,
            preferred_width: Some(preferred_width),
            preferred_height: Some(preferred_height),
            preferred_fps: Some(preferred_fps),
        })?;
        self.inner.camera.register_remote_session(device_id, &session_id, request_id.clone());
        if let Err(error) = self.send_business_message_with_envelope_id(device_id, message, request_id).await {
            self.inner.camera.discard_remote_session(&session_id);
            return Err(error);
        }
        Ok(session_id)
    }

    pub async fn send_camera_alive(&self, device_id: &str, session_id: &str) -> AppResult<()> {
        if self.inner.camera.remote_session_codec(device_id, session_id).is_none() {
            return Err(AppError::message("camera session is not active"));
        }
        self.send_business_message(device_id, BusinessEnvelope::from_payload(CAMERA_ALIVE_TYPE, CameraAlivePayload { session_id: session_id.to_string() })?).await?;
        Ok(())
    }

    pub async fn close_remote_camera(&self, device_id: &str, session_id: &str) -> AppResult<()> {
        if !self.inner.camera.close_remote_session(device_id, session_id) {
            return Ok(());
        }
        self.inner.lan.unregister_camera(session_id);
        self.send_business_message(device_id, BusinessEnvelope::from_payload(CAMERA_CLOSE_TYPE, CameraClosePayload {
            session_id: session_id.to_string(), reason: None, message: None,
        })?).await?;
        Ok(())
    }

    pub(super) async fn handle_camera_list_result(&self, from: &str, correlation_id: Option<&str>, payload: CameraListResultPayload) {
        let Some(request_id) = correlation_id else { return; };
        let pending = self.inner.camera.pending_lists.lock().ok().and_then(|mut lists| lists.remove(request_id));
        if let Some(pending) = pending.filter(|pending| pending.device_id == from) {
            let result = match payload.message.or(payload.reason) {
                Some(message) => Err(AppError::message(message)),
                None => Ok(payload.cameras),
            };
            let _ = pending.sender.send(result);
        }
    }

    pub(super) async fn handle_camera_open_ack(&self, from: &str, correlation_id: Option<&str>, payload: CameraOpenAckPayload) {
        if !payload.accepted {
            self.inner.camera.discard_remote_session(&payload.session_id);
            self.emit_camera_event(CameraUiEvent {
                session_id: payload.session_id,
                kind: "failed".to_string(),
                data: None,
                codec: None,
                transport: None,
                width: None,
                height: None,
                fps: None,
                keyframe: None,
                sequence: None,
                timestamp_ms: None,
                message: payload.message.or(payload.reason),
            });
            return;
        }
        if payload.negotiated_codec.as_deref() != Some("h264")
            || !self.inner.camera.accept_remote_session(from, &payload, correlation_id)
        {
            return;
        }
        let transport = if let (Some(token), Some((ip, port))) = (
            payload.stream_token.as_deref(),
            self.inner.lan.peer_endpoint(from),
        ) {
            match self.inner.lan.connect_camera(&payload.session_id, token, &ip, port).await {
                Ok(()) => "lan",
                Err(error) => {
                    tracing::warn!(session_id = %payload.session_id, %ip, port, %error, "camera LAN data plane connect failed; falling back to relay");
                    "relay"
                }
            }
        } else {
            "relay"
        };
        let ready = BusinessEnvelope::from_payload(CAMERA_READY_TYPE, CameraReadyPayload {
            session_id: payload.session_id.clone(), transport: transport.to_string(),
        });
        if let Ok(ready) = ready {
            if let Err(error) = self.send_business_message(from, ready).await {
                self.inner.camera.discard_remote_session(&payload.session_id);
                self.emit_camera_event(CameraUiEvent { session_id: payload.session_id, kind: "failed".to_string(), data: None, codec: None, transport: None, width: None, height: None, fps: None, keyframe: None, sequence: None, timestamp_ms: None, message: Some(error.to_string()) });
                return;
            }
        }
        self.emit_camera_event(CameraUiEvent {
            session_id: payload.session_id,
            kind: "opened".to_string(),
            data: None,
            codec: payload.negotiated_codec,
            transport: Some(transport.to_string()),
            width: payload.width,
            height: payload.height,
            fps: payload.fps,
            keyframe: None,
            sequence: None,
            timestamp_ms: None,
            message: None,
        });
    }

    pub(super) fn handle_camera_frame(&self, from: &str, payload: CameraFramePayload) {
        if self.inner.camera.remote_session_codec(from, &payload.session_id).as_deref() != Some(payload.codec.as_str()) {
            return;
        }
        self.emit_camera_event(CameraUiEvent {
            session_id: payload.session_id,
            kind: "frame".to_string(),
            data: Some(payload.data),
            codec: Some(payload.codec),
            transport: None,
            width: None,
            height: None,
            fps: None,
            keyframe: Some(payload.keyframe),
            sequence: Some(payload.sequence),
            timestamp_ms: Some(payload.timestamp_ms),
            message: None,
        });
    }

    pub(super) fn handle_lan_camera_frame(&self, session_id: &str, frame: crate::protocol::CameraDataFrame) {
        if self.inner.camera.remote_session_codec_by_id(session_id).as_deref() != Some(frame.codec.as_str()) {
            return;
        }
        self.emit_camera_event(CameraUiEvent {
            session_id: session_id.to_string(),
            kind: "frame".to_string(),
            data: Some(BASE64.encode(frame.payload)),
            codec: Some(frame.codec),
            transport: None,
            width: None,
            height: None,
            fps: None,
            keyframe: Some(frame.keyframe),
            sequence: Some(frame.sequence as u64),
            timestamp_ms: Some(frame.timestamp_ms as u64),
            message: None,
        });
    }

    pub(super) async fn handle_lan_camera_closed(&self, session_id: &str) {
        if self.inner.camera.close_remote_session_by_id(session_id) {
            self.emit_camera_event(CameraUiEvent {
                session_id: session_id.to_string(),
                kind: "closed".to_string(),
                data: None,
                codec: None,
                transport: None,
                width: None,
                height: None,
                fps: None,
                keyframe: None,
                sequence: None,
                timestamp_ms: None,
                message: Some("LAN camera connection closed".to_string()),
            });
            return;
        }
        let Some(device_id) = self.inner.camera.close_host_session_by_id(session_id) else { return; };
        self.inner.camera_capture.stop(session_id);
        if let Ok(close) = BusinessEnvelope::from_payload(CAMERA_CLOSE_TYPE, CameraClosePayload {
            session_id: session_id.to_string(),
            reason: Some("colink:camera.device_lost.v1".to_string()),
            message: Some("LAN camera connection closed".to_string()),
        }) {
            let _ = self.send_business_message(&device_id, close).await;
        }
    }

    pub(super) fn handle_camera_close(&self, from: &str, payload: CameraClosePayload) {
        self.inner.lan.unregister_camera(&payload.session_id);
        if self.inner.camera.close_remote_session(from, &payload.session_id) {
            self.emit_camera_event(CameraUiEvent { session_id: payload.session_id, kind: "closed".to_string(), data: None, codec: None, transport: None, width: None, height: None, fps: None, keyframe: None, sequence: None, timestamp_ms: None, message: payload.message.or(payload.reason) });
        } else if self.inner.camera.close_host_session(from, &payload.session_id) {
            self.inner.camera_capture.stop(&payload.session_id);
        }
    }

    pub(super) async fn handle_camera_open(&self, from: &str, envelope_id: Option<String>, payload: CameraOpenPayload) {
        let Some(correlation_id) = envelope_id else { return; };
        let requested_width = payload.preferred_width.unwrap_or(1280).clamp(160, 1280);
        let requested_height = payload.preferred_height.unwrap_or(720).clamp(120, 720);
        let requested_fps = payload.preferred_fps.unwrap_or(15).clamp(1, 24);
        let result = self
            .inner
            .camera_capture
            .negotiate(
                &payload.camera_id,
                requested_width,
                requested_height,
                requested_fps,
            )
            .and_then(|profile| {
                self.inner
                    .camera
                    .open_host_session(from, &payload, profile)
                    .map_err(|(_, message)| AppError::message(message))
            });
        match result {
            Ok(session) => {
                let stream_token = self.inner.lan.is_available(from).then(|| {
                    let token = Uuid::new_v4().simple().to_string();
                    self.inner.lan.register_camera_token(&session.session_id, &token);
                    token
                });
                if let Ok(response) = BusinessEnvelope::from_payload(CAMERA_OPEN_ACK_TYPE, CameraOpenAckPayload {
                    session_id: session.session_id.clone(),
                    accepted: true,
                    negotiated_codec: Some("h264".to_string()),
                    width: Some(session.width),
                    height: Some(session.height),
                    fps: Some(session.fps),
                    stream_token,
                    reason: None,
                    message: None,
                }) {
                    let _ = self.send_business_message_with_correlation(from, response, Some(correlation_id)).await;
                }
                let runtime = self.clone();
                let session_id = session.session_id;
                tauri::async_runtime::spawn(async move {
                    loop {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        if let Some(device_id) = runtime.inner.camera.expire_host_session(&session_id) {
                            runtime.inner.camera_capture.stop(&session_id);
                            runtime.inner.lan.unregister_camera(&session_id);
                            if let Ok(close) = BusinessEnvelope::from_payload(CAMERA_CLOSE_TYPE, CameraClosePayload {
                                session_id: session_id.clone(),
                                reason: Some("colink:camera.alive_timeout.v1".to_string()),
                                message: Some("Camera heartbeat timed out".to_string()),
                            }) {
                                let _ = runtime.send_business_message(&device_id, close).await;
                            }
                            break;
                        }
                        if !runtime.inner.camera.host_session_exists(&session_id) {
                            break;
                        }
                    }
                });
            }
            Err(error) => {
                if let Ok(response) = BusinessEnvelope::from_payload(CAMERA_OPEN_ACK_TYPE, CameraOpenAckPayload {
                    session_id: payload.session_id,
                    accepted: false,
                    negotiated_codec: None,
                    width: None,
                    height: None,
                    fps: None,
                    stream_token: None,
                    reason: Some("colink:camera.device_lost.v1".to_string()),
                    message: Some(error.to_string()),
                }) {
                    let _ = self.send_business_message_with_correlation(from, response, Some(correlation_id)).await;
                }
            }
        }
    }

    pub(super) async fn handle_camera_ready(&self, from: &str, payload: CameraReadyPayload) {
        let lan_connected = payload.transport != "lan" || self.inner.lan.has_camera_connection(&payload.session_id);
        let ready = self.inner.camera.mark_host_ready(from, &payload.session_id, &payload.transport, lan_connected);
        if ready && payload.transport == "relay" {
            self.inner.lan.unregister_camera(&payload.session_id);
        }
    }

    pub(super) async fn handle_lan_camera_connected(&self, session_id: &str) {
        self.inner.camera.mark_host_lan_connected(session_id);
    }

    pub(super) async fn handle_camera_alive(&self, from: &str, payload: CameraAlivePayload) {
        if self.inner.camera.receive_host_alive(from, &payload.session_id) {
            self.start_host_capture(&payload.session_id).await;
        }
    }

    pub(super) async fn handle_camera_config(&self, from: &str, correlation_id: Option<&str>, payload: CameraConfigPayload) {
        let Some(correlation_id) = correlation_id else { return; };
        let response = match self.inner.camera.host_camera_preferences(from, &payload) {
            Ok(request) => match self
                .inner
                .camera_capture
                .negotiate(&request.camera_id, request.width, request.height, request.fps)
            {
                Ok(profile) => match self.inner.camera.reconfigure_host_session(from, &payload, profile) {
                    Ok(update) => {
                        if update.stop_capture {
                            self.inner.camera_capture.stop(&payload.session_id);
                        }
                        CameraConfigAckPayload {
                            session_id: payload.session_id,
                            applied: true,
                            width: Some(update.width),
                            height: Some(update.height),
                            fps: Some(update.fps),
                            reason: None,
                            message: None,
                        }
                    }
                    Err((reason, message)) => CameraConfigAckPayload {
                        session_id: payload.session_id,
                        applied: false,
                        width: None,
                        height: None,
                        fps: None,
                        reason: Some(reason.to_string()),
                        message: Some(message.to_string()),
                    },
                },
                Err(error) => CameraConfigAckPayload {
                    session_id: payload.session_id,
                    applied: false,
                    width: None,
                    height: None,
                    fps: None,
                    reason: Some("colink:camera.device_lost.v1".to_string()),
                    message: Some(error.to_string()),
                },
            },
            Err((reason, message)) => CameraConfigAckPayload {
                session_id: payload.session_id,
                applied: false,
                width: None,
                height: None,
                fps: None,
                reason: Some(reason.to_string()),
                message: Some(message.to_string()),
            },
        };
        if let Ok(response) = BusinessEnvelope::from_payload(CAMERA_CONFIG_ACK_TYPE, response) {
            let _ = self.send_business_message_with_correlation(from, response, Some(correlation_id.to_string())).await;
        }
    }

    pub(super) async fn handle_native_camera_frame(
        &self,
        session_id: &str,
        generation: u64,
        keyframe: bool,
        payload: Vec<u8>,
    ) {
        let Some((device_id, transport, sequence, timestamp_ms)) = self.inner.camera.next_host_frame(session_id, generation) else { return; };
        if transport == "lan" {
            let Some(frame) = crate::protocol::CameraDataFrame::new("h264", keyframe, sequence, timestamp_ms, payload) else { return; };
            if let Err(error) = self.inner.lan.send_camera_frame(session_id, frame) {
                tracing::debug!(%session_id, %error, "camera LAN frame dropped");
            }
            return;
        }
        if let Ok(frame) = BusinessEnvelope::from_payload(CAMERA_FRAME_TYPE, CameraFramePayload {
            session_id: session_id.to_string(),
            codec: "h264".to_string(),
            keyframe,
            sequence,
            timestamp_ms,
            data: BASE64.encode(payload),
        }) {
            if let Err(error) = self.send_business_message(&device_id, frame).await {
                tracing::debug!(%session_id, %error, "camera relay frame dropped");
            }
        }
    }

    pub(super) async fn handle_native_camera_failed(&self, session_id: &str, generation: u64, message: String) {
        let Some(device_id) = self.inner.camera.fail_host_capture(session_id, generation) else { return; };
        tracing::warn!(%session_id, generation, %message, "native camera capture failed");
        if let Ok(close) = BusinessEnvelope::from_payload(CAMERA_CLOSE_TYPE, CameraClosePayload {
            session_id: session_id.to_string(),
            reason: Some("colink:camera.capture_failed.v1".to_string()),
            message: Some(message),
        }) {
            let _ = self.send_business_message(&device_id, close).await;
        }
        self.inner.camera_capture.stop(session_id);
        self.inner.lan.unregister_camera(session_id);
    }

    pub(super) async fn handle_native_camera_stopped(&self, session_id: &str, generation: u64) {
        let restart = self.inner.camera.finish_host_capture(session_id, generation);
        match restart {
            Some(request) => {
                if !self.start_host_capture_request(request).await {
                    self.inner.camera_capture.release_stopped_camera(session_id);
                }
            }
            None => self.inner.camera_capture.release_stopped_camera(session_id),
        }
    }

    async fn start_host_capture(&self, session_id: &str) {
        let Some(request) = self.inner.camera.begin_host_capture(session_id) else { return; };
        self.start_host_capture_request(request).await;
    }

    async fn start_host_capture_request(&self, request: CameraCaptureRequest) -> bool {
        let generation = request.generation;
        let session_id = request.session_id.clone();
        if let Err(error) = self.inner.camera_capture.start(request) {
            self.handle_native_camera_failed(&session_id, generation, error.to_string()).await;
            return false;
        }
        true
    }

    pub(super) fn emit_camera_event(&self, event: CameraUiEvent) {
        let _ = self.inner.app.emit("camera-event", event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconfiguration_waits_for_the_previous_capture_to_stop() {
        let manager = CameraManager::new();
        let session_id = "session";
        let device_id = "device";
        let opened = manager
            .open_host_session(
                device_id,
                &CameraOpenPayload {
                    session_id: session_id.to_string(),
                    camera_id: "camera".to_string(),
                    preferred_codecs: vec!["h264".to_string()],
                    preferred_width: Some(640),
                    preferred_height: Some(360),
                    preferred_fps: Some(8),
                },
                CameraCaptureProfile {
                    width: 640,
                    height: 360,
                    fps: 8,
                },
            )
            .expect("open host camera session");
        assert!(manager.mark_host_ready(device_id, &opened.session_id, "relay", false));
        assert!(manager.receive_host_alive(device_id, &opened.session_id));
        let first = manager
            .begin_host_capture(&opened.session_id)
            .expect("begin initial capture");

        let update = manager
            .reconfigure_host_session(
                device_id,
                &CameraConfigPayload {
                    session_id: opened.session_id.clone(),
                    width: Some(960),
                    height: Some(540),
                    fps: Some(15),
                },
                CameraCaptureProfile {
                    width: 960,
                    height: 540,
                    fps: 15,
                },
            )
            .expect("reconfigure host camera session");

        assert!(update.stop_capture);
        assert!(manager.begin_host_capture(&opened.session_id).is_none());

        let second = manager
            .finish_host_capture(&opened.session_id, first.generation)
            .expect("restart capture after stop confirmation");
        assert_eq!(second.generation, first.generation + 1);
        assert_eq!((second.width, second.height, second.fps), (960, 540, 15));
    }
}
