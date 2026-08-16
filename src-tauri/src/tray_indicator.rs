use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::AppHandle;

use crate::protocol::*;

const ACTIVITY_DURATION: Duration = Duration::from_millis(100);

/// Messages that do NOT trigger the activity indicator.
/// These are keepalives, high-frequency intermediate frames managed by session lifecycle,
/// or infrastructure messages.
fn is_silent_message(message_type: &str) -> bool {
    matches!(
        message_type,
        MUSIC_ALIVE_TYPE
            | SYSINFO_ALIVE_TYPE
            | CAMERA_ALIVE_TYPE
            | FILE_CHUNK_TYPE
            | FILE_ACK_TYPE
            | FILE_RETRANSMIT_TYPE
            | FILE_READY_TYPE
            | CAMERA_FRAME_TYPE
            | CAMERA_READY_TYPE
            | CAMERA_CONFIG_ACK_TYPE
    )
}

#[derive(Clone)]
pub struct TrayIndicator {
    inner: Arc<Inner>,
}

struct Inner {
    app: AppHandle,
    active_sessions: AtomicUsize,
    timer_generation: AtomicU64,
    timer_active: AtomicBool,
}

impl TrayIndicator {
    pub fn new(app: AppHandle) -> Self {
        Self {
            inner: Arc::new(Inner {
                app,
                active_sessions: AtomicUsize::new(0),
                timer_generation: AtomicU64::new(0),
                timer_active: AtomicBool::new(false),
            }),
        }
    }

    /// Returns true if the activity indicator (yellow) should be displayed.
    pub fn is_active(&self) -> bool {
        self.inner.active_sessions.load(Ordering::Relaxed) > 0
            || self.inner.timer_active.load(Ordering::Relaxed)
    }

    /// Trigger a short activity blink for a business message.
    /// Call this with the message_type; silent messages are filtered out.
    pub fn trigger(&self, message_type: &str) {
        if is_silent_message(message_type) {
            return;
        }
        self.trigger_activity();
    }

    fn trigger_activity(&self) {
        if self.inner.active_sessions.load(Ordering::Relaxed) > 0 {
            return;
        }

        let gen = self.inner.timer_generation.fetch_add(1, Ordering::SeqCst) + 1;

        if self.inner.timer_active.swap(true, Ordering::SeqCst) {
            // Timer already running; bumping generation is enough to extend it.
            return;
        }

        // First activation — refresh tray to show yellow immediately.
        let _ = crate::shell::refresh_tray(&self.inner.app);

        let inner = self.inner.clone();
        tauri::async_runtime::spawn(async move {
            let mut seen_gen = gen;
            loop {
                tokio::time::sleep(ACTIVITY_DURATION).await;
                let current_gen = inner.timer_generation.load(Ordering::SeqCst);
                if current_gen == seen_gen {
                    break;
                }
                seen_gen = current_gen;
            }
            inner.timer_active.store(false, Ordering::SeqCst);
            let _ = crate::shell::refresh_tray(&inner.app);
        });
    }

    /// Register a long-running session (file transfer or camera).
    pub fn add_session(&self) {
        let prev = self.inner.active_sessions.fetch_add(1, Ordering::SeqCst);
        if prev == 0 {
            let _ = crate::shell::refresh_tray(&self.inner.app);
        }
    }

    /// Unregister a long-running session.
    pub fn remove_session(&self) {
        let prev = self.inner.active_sessions.fetch_sub(1, Ordering::SeqCst);
        if prev == 1 {
            let _ = crate::shell::refresh_tray(&self.inner.app);
        }
    }
}
