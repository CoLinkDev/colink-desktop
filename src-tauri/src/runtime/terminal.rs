use std::{collections::HashMap, io::{Read, Write}, sync::{Arc, Mutex}, thread};

use base64::{engine::general_purpose::STANDARD, Engine};
use portable_pty::{native_pty_system, ChildKiller, CommandBuilder, MasterPty, PtySize};
use tauri::Emitter;

use crate::{error::{AppError, AppResult}, protocol::{BusinessEnvelope, TerminalClosePayload, TerminalDataPayload, TERMINAL_CLOSE_TYPE, TERMINAL_DATA_TYPE}};

use super::AppRuntime;

#[derive(serde::Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct TerminalUiEvent { pub session_id: String, pub kind: String, pub data: Option<String>, pub exit_code: Option<i32>, pub message: Option<String> }

#[derive(serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum RemoteTerminalSupport {
    Unknown,
    Supported,
    Unsupported,
}

#[derive(Clone)]
pub(super) struct TerminalManager {
    sessions: Arc<Mutex<HashMap<String, HostSession>>>,
    remote_sessions: Arc<Mutex<HashMap<String, RemoteSession>>>,
}

struct HostSession {
    device_id: String,
    master: Box<dyn MasterPty + Send>,
    writer: Box<dyn Write + Send>,
    child: Box<dyn ChildKiller + Send + Sync>,
}

struct RemoteSession {
    device_id: String,
    open_request_id: Option<String>,
}

impl TerminalManager {
    pub(super) fn new() -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            remote_sessions: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub(super) fn open(&self, runtime: AppRuntime, device_id: String, session_id: String, cols: u16, rows: u16, env: Option<HashMap<String, String>>) -> AppResult<()> {
        if cols == 0 || rows == 0 { return Err(AppError::message("terminal dimensions must be positive")); }
        if self.is_active(&session_id) { return Err(AppError::message("terminal session already exists")); }
        let pty = native_pty_system();
        let pair = pty.openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|error| AppError::message(error.to_string()))?;
        let mut command = CommandBuilder::new(default_shell());
        if let Some(env) = env {
            for (key, value) in env {
                if matches!(key.as_str(), "TERM" | "LANG" | "LC_ALL") { command.env(key, value); }
            }
        }
        let mut child = pair.slave.spawn_command(command).map_err(|error| AppError::message(error.to_string()))?;
        let child_killer = child.clone_killer();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().map_err(|error| AppError::message(error.to_string()))?;
        let writer = pair.master.take_writer().map_err(|error| AppError::message(error.to_string()))?;
        self.sessions.lock().map_err(|_| AppError::message("terminal state unavailable"))?.insert(session_id.clone(), HostSession { device_id: device_id.clone(), master: pair.master, writer, child: child_killer });
        let manager = self.clone();
        let reader_runtime = runtime.clone();
        let reader_device_id = device_id.clone();
        let reader_session_id = session_id.clone();
        thread::spawn(move || {
            let mut buffer = [0_u8; 8192];
            loop {
                let read = match reader.read(&mut buffer) { Ok(0) | Err(_) => break, Ok(read) => read };
                if !manager.is_active(&reader_session_id) { break; }
                let payload = TerminalDataPayload { session_id: reader_session_id.clone(), stream: "output".to_string(), data: STANDARD.encode(&buffer[..read]) };
                let message = match BusinessEnvelope::from_payload(TERMINAL_DATA_TYPE, payload) { Ok(message) => message, Err(_) => break };
                if let Err(error) = tauri::async_runtime::block_on(reader_runtime.send_business_message(&reader_device_id, message)) {
                    tracing::warn!(device_id = %reader_device_id, session_id = %reader_session_id, %error, "terminal output forwarding failed");
                    break;
                }
            }
            manager.finish_host_session(reader_runtime, reader_device_id, reader_session_id, None);
        });
        let manager = self.clone();
        thread::spawn(move || {
            let exit_code = child.wait().ok().map(|status| status.exit_code() as i32);
            manager.finish_host_session(runtime, device_id, session_id, exit_code);
        });
        Ok(())
    }

    pub(super) fn write(&self, device_id: &str, session_id: &str, data: &[u8]) -> AppResult<()> {
        let mut sessions = self.sessions.lock().map_err(|_| AppError::message("terminal state unavailable"))?;
        let session = sessions.get_mut(session_id).ok_or_else(|| AppError::message("unknown terminal session"))?;
        if session.device_id != device_id { return Err(AppError::message("terminal session owner mismatch")); }
        session.writer.write_all(data)?;
        session.writer.flush()?;
        Ok(())
    }

    pub(super) fn resize(&self, device_id: &str, session_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        let sessions = self.sessions.lock().map_err(|_| AppError::message("terminal state unavailable"))?;
        let session = sessions.get(session_id).ok_or_else(|| AppError::message("unknown terminal session"))?;
        if session.device_id != device_id { return Err(AppError::message("terminal session owner mismatch")); }
        session.master.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|error| AppError::message(error.to_string()))?;
        Ok(())
    }

    pub(super) fn close(&self, session_id: &str) {
        if let Some(mut session) = self.remove(session_id) { let _ = session.child.kill(); }
    }

    pub(super) fn close_for_session(&self, device_id: &str, session_id: &str) {
        let owned = self.sessions.lock().ok().is_some_and(|sessions| sessions.get(session_id).is_some_and(|session| session.device_id == device_id));
        if owned { self.close(session_id); }
    }

    pub(super) fn close_for_device(&self, device_id: &str) {
        let ids = self.sessions.lock().ok().map(|sessions| sessions.iter().filter_map(|(id, session)| (session.device_id == device_id).then(|| id.clone())).collect::<Vec<_>>()).unwrap_or_default();
        for id in ids { self.close(&id); }
        if let Ok(mut sessions) = self.remote_sessions.lock() {
            sessions.retain(|_, session| session.device_id != device_id);
        }
    }

    pub(super) fn register_remote_session(&self, device_id: &str, session_id: &str, request_id: String) {
        if let Ok(mut sessions) = self.remote_sessions.lock() {
            sessions.insert(session_id.to_string(), RemoteSession { device_id: device_id.to_string(), open_request_id: Some(request_id) });
        }
    }

    pub(super) fn discard_remote_session(&self, session_id: &str) {
        if let Ok(mut sessions) = self.remote_sessions.lock() { sessions.remove(session_id); }
    }

    pub(super) fn accept_remote_session(&self, device_id: &str, session_id: &str, correlation_id: Option<&str>) -> bool {
        let Ok(mut sessions) = self.remote_sessions.lock() else { return false; };
        let Some(session) = sessions.get_mut(session_id) else { return false; };
        if session.device_id != device_id || session.open_request_id.as_deref() != correlation_id { return false; }
        session.open_request_id = None;
        true
    }

    pub(super) fn is_remote_session(&self, device_id: &str, session_id: &str) -> bool {
        self.remote_sessions.lock().ok().and_then(|sessions| sessions.get(session_id).map(|session| session.device_id == device_id && session.open_request_id.is_none())).unwrap_or(false)
    }

    pub(super) fn close_remote_session(&self, device_id: &str, session_id: &str) -> bool {
        let Ok(mut sessions) = self.remote_sessions.lock() else { return false; };
        let Some(session) = sessions.get(session_id) else { return false; };
        if session.device_id != device_id { return false; }
        sessions.remove(session_id);
        true
    }

    fn is_active(&self, session_id: &str) -> bool { self.sessions.lock().map(|sessions| sessions.contains_key(session_id)).unwrap_or(false) }
    fn remove(&self, session_id: &str) -> Option<HostSession> { self.sessions.lock().ok()?.remove(session_id) }

    fn finish_host_session(&self, runtime: AppRuntime, device_id: String, session_id: String, exit_code: Option<i32>) {
        if self.remove(&session_id).is_none() { return; }
        let payload = TerminalClosePayload { session_id: session_id.clone(), exit_code };
        if let Ok(message) = BusinessEnvelope::from_payload(TERMINAL_CLOSE_TYPE, payload) {
            if let Err(error) = tauri::async_runtime::block_on(runtime.send_business_message(&device_id, message)) {
                tracing::warn!(%device_id, %session_id, %error, "terminal close forwarding failed");
            }
        }
    }
}

impl AppRuntime {
    pub fn remote_terminal_support(&self, device_id: &str) -> RemoteTerminalSupport {
        match self.peer_business_version(device_id) {
            None => RemoteTerminalSupport::Unknown,
            Some(version) if crate::protocol::supports_business_protocol_at_least(&version, 1, 9, 0) => {
                RemoteTerminalSupport::Supported
            }
            Some(_) => RemoteTerminalSupport::Unsupported,
        }
    }

    pub async fn open_remote_terminal(&self, device_id: &str, cols: u16, rows: u16) -> AppResult<String> {
        if cols == 0 || rows == 0 { return Err(AppError::message("terminal dimensions must be positive")); }
        if self
            .peer_business_version(device_id)
            .is_some_and(|version| !crate::protocol::supports_business_protocol_at_least(&version, 1, 9, 0))
        {
            return Err(AppError::message("remote device does not support terminal control"));
        }
        let session_id = uuid::Uuid::new_v4().to_string();
        let request_id = uuid::Uuid::new_v4().to_string();
        let payload = crate::protocol::TerminalOpenPayload { session_id: session_id.clone(), cols, rows, env: None };
        let message = BusinessEnvelope::from_payload(crate::protocol::TERMINAL_OPEN_TYPE, payload)?;
        self.inner.terminal.register_remote_session(device_id, &session_id, request_id.clone());
        if let Err(error) = self.send_business_message_with_envelope_id(device_id, message, request_id).await {
            self.inner.terminal.discard_remote_session(&session_id);
            return Err(error);
        }
        Ok(session_id)
    }

    pub async fn write_remote_terminal(&self, device_id: &str, session_id: &str, data: String) -> AppResult<()> {
        if !self.inner.terminal.is_remote_session(device_id, session_id) { return Err(AppError::message("terminal session is not active")); }
        let payload = TerminalDataPayload { session_id: session_id.to_string(), stream: "input".to_string(), data: STANDARD.encode(data.as_bytes()) };
        self.send_business_message(device_id, BusinessEnvelope::from_payload(TERMINAL_DATA_TYPE, payload)?).await?;
        Ok(())
    }

    pub async fn resize_remote_terminal(&self, device_id: &str, session_id: &str, cols: u16, rows: u16) -> AppResult<()> {
        if !self.inner.terminal.is_remote_session(device_id, session_id) { return Err(AppError::message("terminal session is not active")); }
        let payload = crate::protocol::TerminalResizePayload { session_id: session_id.to_string(), cols, rows };
        self.send_business_message(device_id, BusinessEnvelope::from_payload(crate::protocol::TERMINAL_RESIZE_TYPE, payload)?).await?;
        Ok(())
    }

    pub async fn close_remote_terminal(&self, device_id: &str, session_id: &str) -> AppResult<()> {
        if !self.inner.terminal.is_remote_session(device_id, session_id) { return Err(AppError::message("terminal session is not active")); }
        let payload = TerminalClosePayload { session_id: session_id.to_string(), exit_code: None };
        self.send_business_message(device_id, BusinessEnvelope::from_payload(TERMINAL_CLOSE_TYPE, payload)?).await?;
        self.inner.terminal.discard_remote_session(session_id);
        Ok(())
    }

    pub(super) fn emit_terminal_event(&self, event: TerminalUiEvent) { let _ = self.inner.app.emit("terminal-event", event); }
}

fn default_shell() -> String {
    #[cfg(windows)] { std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string()) }
    #[cfg(not(windows))] { std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string()) }
}
