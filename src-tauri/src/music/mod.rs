mod cdp;
mod lyrics;

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::Duration,
};

use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};
use tracing::{debug, info};

use crate::{
    network::transport::TransportManager,
    protocol::{
        BusinessEnvelope, MusicLyricPayload, MusicProgressPayload, MusicTrackPayload,
        MUSIC_LYRIC_TYPE, MUSIC_PROGRESS_TYPE, MUSIC_TRACK_TYPE,
    },
    runtime_events::RuntimeEvent,
    sync::MutexExt,
};

use self::{
    cdp::{CdpDetector, CdpPlayingState, PlaybackStatus},
    lyrics::fetch_lyric,
};

const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(200);
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const ALIVE_TIMEOUT: Duration = Duration::from_secs(15);
const NONE_APP_CLOSE_DELAY: Duration = Duration::from_secs(1);
const FAR_PROGRESS_PUBLISH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct MusicService {
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    send_tx: mpsc::UnboundedSender<MusicSendJob>,
    lyric_cache: Arc<Mutex<HashMap<String, MusicLyricPayload>>>,
    state: Arc<Mutex<MusicState>>,
}

struct MusicState {
    running: bool,
    cancel: Option<watch::Sender<bool>>,
    active_receivers: HashMap<String, Instant>,
    snapshot: MusicSnapshot,
}

#[derive(Clone, Default)]
struct MusicSnapshot {
    track: Option<MusicTrackPayload>,
    lyric: Option<MusicLyricPayload>,
    progress: Option<MusicProgressPayload>,
}

#[derive(Clone, PartialEq, Eq)]
struct TrackKey {
    track_id: Option<String>,
    title: String,
    artists: Vec<String>,
    album: Option<String>,
    cover_url: Option<String>,
    duration_ms: Option<i64>,
}

#[derive(Debug, Clone)]
struct LyricFetchResult {
    request_id: u64,
    track_id: String,
    payload: MusicLyricPayload,
}

#[derive(Clone)]
struct LastProgressPush {
    payload: MusicProgressPayload,
    sent_at: Instant,
}

struct MusicSendJob {
    device_id: String,
    label: &'static str,
    envelope: BusinessEnvelope,
}

impl MusicService {
    pub fn new(transport: TransportManager, event_tx: mpsc::UnboundedSender<RuntimeEvent>) -> Self {
        let (send_tx, send_rx) = mpsc::unbounded_channel();
        spawn_music_sender(transport.clone(), event_tx.clone(), send_rx);

        Self {
            event_tx,
            send_tx,
            lyric_cache: Arc::new(Mutex::new(HashMap::new())),
            state: Arc::new(Mutex::new(MusicState {
                running: false,
                cancel: None,
                active_receivers: HashMap::new(),
                snapshot: MusicSnapshot::default(),
            })),
        }
    }

    pub async fn handle_alive(&self, from_device_id: &str) {
        let device_id = from_device_id.trim();
        if device_id.is_empty() {
            return;
        }

        let (was_new, should_start) = {
            let mut state = self.state.lock_unpoisoned();
            prune_expired_receivers_locked(&mut state.active_receivers);
            let was_new = state
                .active_receivers
                .insert(device_id.to_string(), Instant::now())
                .is_none();
            let should_start = if state.running {
                false
            } else {
                state.running = true;
                true
            };
            (was_new, should_start)
        };

        if should_start {
            self.start_loop();
            self.log_info(format!("music sync activated by {device_id}"));
            return;
        }

        if was_new {
            self.log_info(format!("music receiver registered: {device_id}"));
        }
    }

    pub async fn handle_request(&self, from_device_id: &str) {
        let device_id = from_device_id.trim();
        if device_id.is_empty() {
            return;
        }

        let (should_start, snapshot) = {
            let mut state = self.state.lock_unpoisoned();
            prune_expired_receivers_locked(&mut state.active_receivers);
            state
                .active_receivers
                .insert(device_id.to_string(), Instant::now());
            let should_start = if state.running {
                false
            } else {
                state.running = true;
                true
            };
            (should_start, state.snapshot.clone())
        };

        if should_start {
            self.start_loop();
            self.log_info(format!("music sync activated by request from {device_id}"));
        }

        self.send_snapshot_to_device(device_id, &snapshot).await;
    }

    pub fn stop(&self) {
        let cancel = {
            let mut state = self.state.lock_unpoisoned();
            state.running = false;
            state.active_receivers.clear();
            state.snapshot = MusicSnapshot::default();
            state.cancel.take()
        };

        if let Some(cancel) = cancel {
            let _ = cancel.send(true);
        }
        self.log_info("music sync stopped".to_string());
    }

    fn start_loop(&self) {
        let (cancel_tx, cancel_rx) = watch::channel(false);
        {
            let mut state = self.state.lock_unpoisoned();
            if state.running && state.cancel.is_some() {
                return;
            }
            state.running = true;
            state.cancel = Some(cancel_tx);
        }

        let service = self.clone();
        tauri::async_runtime::spawn(async move {
            service.run(cancel_rx).await;
        });
    }

    async fn run(&self, mut cancel_rx: watch::Receiver<bool>) {
        let mut detector = CdpDetector::new();
        let (lyric_tx, mut lyric_rx) = mpsc::unbounded_channel::<LyricFetchResult>();
        let mut prev_status = PlaybackStatus::None;
        let mut prev_track_key: Option<TrackKey> = None;
        let mut prev_progress: Option<MusicProgressPayload> = None;
        let mut last_progress_push: Option<LastProgressPush> = None;
        let mut none_since: Option<Instant> = None;
        let mut request_id = 0_u64;

        info!("music sync loop started");

        loop {
            if is_cancelled(&cancel_rx) {
                break;
            }

            let active_ids = self.prune_active_receivers();
            if active_ids.is_empty() {
                break;
            }

            while let Ok(result) = lyric_rx.try_recv() {
                self.handle_lyric_result(result, request_id, prev_track_key.as_ref(), &active_ids)
                    .await;
            }

            let wait = if matches!(prev_status, PlaybackStatus::Active) {
                ACTIVE_POLL_INTERVAL
            } else {
                IDLE_POLL_INTERVAL
            };

            tokio::select! {
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && is_cancelled(&cancel_rx) {
                        break;
                    }
                }
                _ = tokio::time::sleep(wait) => {
                    let state = detector.poll().await;
                    self.handle_detector_state(
                        state,
                        &active_ids,
                        &mut prev_status,
                        &mut prev_track_key,
                        &mut prev_progress,
                        &mut last_progress_push,
                        &mut none_since,
                        &mut request_id,
                        &lyric_tx,
                    )
                    .await;
                }
                result = lyric_rx.recv() => {
                    if let Some(result) = result {
                self.handle_lyric_result(
                    result,
                    request_id,
                    prev_track_key.as_ref(),
                    &active_ids,
                        )
                        .await;
                    }
                }
            }
        }

        self.finish_run();
        info!("music sync loop stopped");
    }

    async fn handle_detector_state(
        &self,
        state: CdpPlayingState,
        _active_ids: &[String],
        prev_status: &mut PlaybackStatus,
        prev_track_key: &mut Option<TrackKey>,
        prev_progress: &mut Option<MusicProgressPayload>,
        last_progress_push: &mut Option<LastProgressPush>,
        none_since: &mut Option<Instant>,
        request_id: &mut u64,
        lyric_tx: &mpsc::UnboundedSender<LyricFetchResult>,
    ) {
        match state.status {
            PlaybackStatus::None => {
                if !matches!(prev_status, PlaybackStatus::None) {
                    *none_since = Some(Instant::now());
                }

                if prev_track_key.is_some() {
                    let elapsed = none_since
                        .as_ref()
                        .map(Instant::elapsed)
                        .unwrap_or_default();
                    if elapsed >= NONE_APP_CLOSE_DELAY {
                        self.clear_playback_state().await;
                        *request_id += 1;
                        *prev_track_key = None;
                        *prev_progress = None;
                        *last_progress_push = None;
                        *prev_status = PlaybackStatus::None;
                        *none_since = None;
                        return;
                    }
                }

                *prev_status = PlaybackStatus::None;
            }
            PlaybackStatus::Active => {
                *none_since = None;
                let track_key = track_key(&state);
                let track_changed = prev_track_key.as_ref() != Some(&track_key);
                let progress_ms = state.progress_ms.unwrap_or(0);

                if track_changed {
                    let progress = build_progress_payload(&state, progress_ms);
                    *request_id += 1;
                    *prev_track_key = Some(track_key);
                    *prev_progress = progress.clone();
                    *prev_status = PlaybackStatus::Active;
                    self.publish_track_change(&state, progress_ms).await;
                    *last_progress_push = progress.map(|payload| LastProgressPush {
                        payload,
                        sent_at: Instant::now(),
                    });
                    self.spawn_lyric_fetch(*request_id, state.track_id.clone(), lyric_tx.clone());
                    return;
                }

                let progress = build_progress_payload(&state, progress_ms);
                if progress_payload_changed(prev_progress.as_ref(), progress.as_ref()) {
                    *prev_progress = progress.clone();
                    match progress {
                        Some(payload) => {
                            let now = Instant::now();
                            let lyric = self.current_lyric_for_track(&payload.track_id);
                            let should_push = should_push_progress(
                                &payload,
                                last_progress_push.as_ref(),
                                lyric.as_ref(),
                                now,
                            );
                            self.update_progress(payload.clone(), should_push).await;
                            if should_push {
                                *last_progress_push = Some(LastProgressPush {
                                    payload,
                                    sent_at: now,
                                });
                            }
                        }
                        None => {
                            self.clear_progress_snapshot();
                            *last_progress_push = None;
                        }
                    }
                }
                *prev_status = PlaybackStatus::Active;
            }
        }
    }

    async fn handle_lyric_result(
        &self,
        result: LyricFetchResult,
        current_request_id: u64,
        current_track_key: Option<&TrackKey>,
        active_ids: &[String],
    ) {
        if result.request_id != current_request_id {
            return;
        }

        let Some(track_key) = current_track_key else {
            return;
        };
        let Some(track_id) = track_key.track_id.as_deref() else {
            return;
        };
        if track_id != result.track_id {
            return;
        }

        {
            let mut state = self.state.lock_unpoisoned();
            state.snapshot.lyric = Some(result.payload.clone());
        }

        for device_id in active_ids {
            self.send_lyric_message(device_id, &result.payload).await;
        }
    }

    async fn publish_track_change(&self, state: &CdpPlayingState, progress_ms: i64) {
        let Some(track) = build_track_payload(state) else {
            self.clear_playback_state().await;
            return;
        };

        let progress = build_progress_payload(state, progress_ms);
        {
            let mut guard = self.state.lock_unpoisoned();
            guard.snapshot.track = Some(track.clone());
            guard.snapshot.lyric = None;
            guard.snapshot.progress = progress.clone();
        }

        let active_ids = self.prune_active_receivers();
        for device_id in active_ids {
            self.send_track_message(&device_id, &track).await;
            if let Some(progress) = &progress {
                self.send_progress_message(&device_id, progress).await;
            }
        }

        if let Some(track_id) = track.track_id.as_deref() {
            debug!(track_id = %track_id, "music track changed");
        } else {
            debug!("music track changed");
        }
    }

    async fn update_progress(&self, progress: MusicProgressPayload, send: bool) {
        {
            let mut guard = self.state.lock_unpoisoned();
            guard.snapshot.progress = Some(progress.clone());
        }

        if !send {
            return;
        }

        for device_id in self.prune_active_receivers() {
            self.send_progress_message(&device_id, &progress).await;
        }
    }

    fn clear_progress_snapshot(&self) {
        let mut guard = self.state.lock_unpoisoned();
        guard.snapshot.progress = None;
    }

    fn current_lyric_for_track(&self, track_id: &str) -> Option<MusicLyricPayload> {
        let state = self.state.lock_unpoisoned();
        state
            .snapshot
            .lyric
            .as_ref()
            .filter(|payload| payload.track_id == track_id)
            .cloned()
    }

    fn spawn_lyric_fetch(
        &self,
        request_id: u64,
        track_id: Option<String>,
        lyric_tx: mpsc::UnboundedSender<LyricFetchResult>,
    ) {
        let Some(track_id) = track_id.filter(|value| !value.trim().is_empty()) else {
            return;
        };
        let cache = self.lyric_cache.clone();
        tauri::async_runtime::spawn(async move {
            if let Some(cached) = cache.lock_unpoisoned().get(&track_id).cloned() {
                let _ = lyric_tx.send(LyricFetchResult {
                    request_id,
                    track_id,
                    payload: cached,
                });
                return;
            }

            let Some(payload) = fetch_lyric(&track_id).await else {
                return;
            };
            cache
                .lock_unpoisoned()
                .insert(track_id.clone(), payload.clone());
            let _ = lyric_tx.send(LyricFetchResult {
                request_id,
                track_id,
                payload,
            });
        });
    }

    async fn clear_playback_state(&self) {
        {
            let mut guard = self.state.lock_unpoisoned();
            guard.snapshot = MusicSnapshot::default();
        }

        let active_ids = self.prune_active_receivers();
        let empty = empty_track_payload();
        for device_id in active_ids {
            self.send_track_message(&device_id, &empty).await;
        }
    }

    fn prune_active_receivers(&self) -> Vec<String> {
        let mut state = self.state.lock_unpoisoned();
        prune_expired_receivers_locked(&mut state.active_receivers);
        state.active_receivers.keys().cloned().collect()
    }

    fn finish_run(&self) {
        let mut state = self.state.lock_unpoisoned();
        state.running = false;
        state.cancel = None;
        state.active_receivers.clear();
        state.snapshot = MusicSnapshot::default();
    }

    async fn send_snapshot_to_device(&self, device_id: &str, snapshot: &MusicSnapshot) {
        let Some(track) = &snapshot.track else {
            let empty = empty_track_payload();
            self.send_track_message(device_id, &empty).await;
            return;
        };

        self.send_track_message(device_id, track).await;
        if let Some(lyric) = &snapshot.lyric {
            self.send_lyric_message(device_id, lyric).await;
        }
        if let Some(progress) = &snapshot.progress {
            self.send_progress_message(device_id, progress).await;
        }
    }

    async fn send_track_message(&self, device_id: &str, payload: &MusicTrackPayload) {
        if let Err(error) = self.queue_music_message(device_id, "track", MUSIC_TRACK_TYPE, payload)
        {
            self.log_warn(format!(
                "failed to send music track to {device_id}: {error}"
            ));
        }
    }

    async fn send_lyric_message(&self, device_id: &str, payload: &MusicLyricPayload) {
        if let Err(error) = self.queue_music_message(device_id, "lyric", MUSIC_LYRIC_TYPE, payload)
        {
            self.log_warn(format!(
                "failed to send music lyric to {device_id}: {error}"
            ));
        }
    }

    async fn send_progress_message(&self, device_id: &str, payload: &MusicProgressPayload) {
        if let Err(error) =
            self.queue_music_message(device_id, "progress", MUSIC_PROGRESS_TYPE, payload)
        {
            self.log_warn(format!(
                "failed to send music progress to {device_id}: {error}"
            ));
        }
    }

    fn queue_music_message<T>(
        &self,
        device_id: &str,
        label: &'static str,
        message_type: &str,
        payload: &T,
    ) -> Result<(), String>
    where
        T: serde::Serialize,
    {
        let envelope = BusinessEnvelope::from_payload(message_type, payload)
            .map_err(|error| error.to_string())?;
        self.send_tx
            .send(MusicSendJob {
                device_id: device_id.to_string(),
                label,
                envelope,
            })
            .map_err(|_| "music send queue is closed".to_string())?;
        Ok(())
    }

    fn log_info(&self, message: impl Into<String>) {
        let _ = self.event_tx.send(RuntimeEvent::Log {
            level: "info".to_string(),
            source: "music".to_string(),
            message: message.into(),
        });
    }

    fn log_warn(&self, message: impl Into<String>) {
        let _ = self.event_tx.send(RuntimeEvent::Log {
            level: "warn".to_string(),
            source: "music".to_string(),
            message: message.into(),
        });
    }
}

fn spawn_music_sender(
    transport: TransportManager,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    mut send_rx: mpsc::UnboundedReceiver<MusicSendJob>,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(job) = send_rx.recv().await {
            if let Err(error) = transport.send(&job.device_id, job.envelope).await {
                emit_music_log(
                    &event_tx,
                    "warn",
                    format!(
                        "failed to send music {} to {}: {}",
                        job.label, job.device_id, error
                    ),
                );
            }
        }
    });
}

fn emit_music_log(
    event_tx: &mpsc::UnboundedSender<RuntimeEvent>,
    level: &str,
    message: impl Into<String>,
) {
    let _ = event_tx.send(RuntimeEvent::Log {
        level: level.to_string(),
        source: "music".to_string(),
        message: message.into(),
    });
}

fn build_track_payload(state: &CdpPlayingState) -> Option<MusicTrackPayload> {
    if !matches!(state.status, PlaybackStatus::Active) {
        return None;
    }

    Some(MusicTrackPayload {
        track_id: state.track_id.clone(),
        title: non_empty(state.title.as_str()),
        artists: if state.artists.is_empty() {
            None
        } else {
            Some(state.artists.clone())
        },
        album: state.album.clone(),
        source: Some("ncm".to_string()),
        cover_url: state.cover_url.clone(),
        cover_data: None,
        duration: state.duration_ms,
    })
}

fn empty_track_payload() -> MusicTrackPayload {
    MusicTrackPayload {
        track_id: None,
        title: None,
        artists: None,
        album: None,
        source: None,
        cover_url: None,
        cover_data: None,
        duration: None,
    }
}

fn build_progress_payload(
    state: &CdpPlayingState,
    progress_ms: i64,
) -> Option<MusicProgressPayload> {
    let track_id = state.track_id.clone()?;
    Some(MusicProgressPayload {
        track_id,
        progress: progress_ms,
        paused: state.playing_state != 2,
    })
}

fn progress_payload_changed(
    previous: Option<&MusicProgressPayload>,
    next: Option<&MusicProgressPayload>,
) -> bool {
    match (previous, next) {
        (Some(previous), Some(next)) => {
            previous.track_id != next.track_id
                || previous.progress != next.progress
                || previous.paused != next.paused
        }
        (None, None) => false,
        _ => true,
    }
}

fn should_push_progress(
    payload: &MusicProgressPayload,
    previous: Option<&LastProgressPush>,
    lyric: Option<&MusicLyricPayload>,
    now: Instant,
) -> bool {
    let Some(previous) = previous else {
        return true;
    };

    if previous.payload.track_id != payload.track_id || previous.payload.paused != payload.paused {
        return true;
    }

    if previous.payload.progress == payload.progress {
        return false;
    }

    if payload.progress < previous.payload.progress {
        return true;
    }

    if crossed_lyric_line(previous.payload.progress, payload.progress, lyric) {
        return true;
    }

    now.duration_since(previous.sent_at) >= FAR_PROGRESS_PUBLISH_INTERVAL
}

fn crossed_lyric_line(
    previous_progress_ms: i64,
    current_progress_ms: i64,
    lyric: Option<&MusicLyricPayload>,
) -> bool {
    let Some(lyric) = lyric else {
        return false;
    };
    let previous_progress_ms = previous_progress_ms.max(0);
    let current_progress_ms = current_progress_ms.max(0);
    if current_progress_ms <= previous_progress_ms {
        return false;
    }

    lyric
        .lines
        .iter()
        .flatten()
        .chain(lyric.translated_lines.iter().flatten())
        .any(|line| line.time > previous_progress_ms && line.time <= current_progress_ms)
}

fn track_key(state: &CdpPlayingState) -> TrackKey {
    TrackKey {
        track_id: state.track_id.clone(),
        title: state.title.clone(),
        artists: state.artists.clone(),
        album: state.album.clone(),
        cover_url: state.cover_url.clone(),
        duration_ms: state.duration_ms,
    }
}

fn prune_expired_receivers_locked(receivers: &mut HashMap<String, Instant>) {
    let now = Instant::now();
    receivers.retain(|_, last_seen| now.duration_since(*last_seen) < ALIVE_TIMEOUT);
}

fn non_empty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn is_cancelled(cancel_rx: &watch::Receiver<bool>) -> bool {
    *cancel_rx.borrow()
}

#[cfg(test)]
mod tests {
    use crate::protocol::MusicLyricLinePayload;
    use tokio::time::Instant;

    use super::{
        crossed_lyric_line, should_push_progress, LastProgressPush, MusicLyricPayload,
        MusicProgressPayload,
    };

    #[test]
    fn pushes_progress_when_crossing_lyric_line() {
        let lyric = MusicLyricPayload {
            track_id: "track".to_string(),
            lines: Some(vec![MusicLyricLinePayload {
                time: 1_000,
                text: "line".to_string(),
            }]),
            translated_lines: None,
        };
        let now = Instant::now();
        let previous = LastProgressPush {
            payload: MusicProgressPayload {
                track_id: "track".to_string(),
                progress: 900,
                paused: false,
            },
            sent_at: now,
        };
        let current = MusicProgressPayload {
            track_id: "track".to_string(),
            progress: 1_000,
            paused: false,
        };

        assert!(crossed_lyric_line(900, 1_000, Some(&lyric)));
        assert!(should_push_progress(
            &current,
            Some(&previous),
            Some(&lyric),
            now
        ));
    }

    #[test]
    fn pushes_progress_when_playback_seeks_backwards() {
        let now = Instant::now();
        let previous = LastProgressPush {
            payload: MusicProgressPayload {
                track_id: "track".to_string(),
                progress: 10_000,
                paused: false,
            },
            sent_at: now,
        };
        let current = MusicProgressPayload {
            track_id: "track".to_string(),
            progress: 5_000,
            paused: false,
        };

        assert!(should_push_progress(&current, Some(&previous), None, now));
    }
}
