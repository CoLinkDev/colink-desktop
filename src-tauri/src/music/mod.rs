mod ncm;
pub mod provider;
mod kugou;
mod qqmusic;
mod spotify;

use std::{
    collections::{HashMap, HashSet},
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
    store::db::Database,
    sync::MutexExt,
};

use self::provider::{create_provider, ActiveTrack, LyricFuture, MusicProvider, TrackState};

const ACTIVE_POLL_INTERVAL: Duration = Duration::from_millis(200);
const IDLE_POLL_INTERVAL: Duration = Duration::from_secs(1);
const ALIVE_TIMEOUT: Duration = Duration::from_secs(15);
const GRACE_PERIOD: Duration = Duration::from_secs(5);
const FAR_PROGRESS_PUBLISH_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Clone)]
pub struct MusicService {
    database: Database,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    send_tx: mpsc::UnboundedSender<MusicSendJob>,
    config_tx: watch::Sender<()>,
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
    source: String,
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

struct LyricFetchRequest {
    request_id: u64,
    track_id: String,
    provider_id: String,
}

struct MusicSendJob {
    device_id: String,
    label: &'static str,
    correlation_id: Option<String>,
    envelope: BusinessEnvelope,
}

enum ChainState {
    Scanning { index: usize },
    Active { index: usize },
    Grace { index: usize, since: Instant },
}

struct ProviderChain {
    provider_ids: Vec<String>,
    instances: HashMap<String, Box<dyn MusicProvider>>,
}

struct ScannedTrack {
    index: usize,
    provider_id: String,
    track: ActiveTrack,
}

impl ProviderChain {
    fn new(provider_ids: Vec<String>) -> Self {
        Self {
            provider_ids,
            instances: HashMap::new(),
        }
    }

    fn reload(&mut self, provider_ids: Vec<String>) {
        let active_ids = provider_ids.iter().cloned().collect::<HashSet<_>>();
        self.instances.retain(|id, _| active_ids.contains(id));
        self.provider_ids = provider_ids;
    }

    fn len(&self) -> usize {
        self.provider_ids.len()
    }

    fn provider_id(&self, index: usize) -> Option<&str> {
        self.provider_ids.get(index).map(String::as_str)
    }

    fn get_or_create(&mut self, index: usize) -> Option<&mut Box<dyn MusicProvider>> {
        let id = self.provider_ids.get(index)?.clone();
        if !self.instances.contains_key(&id) {
            let provider = create_provider(&id)?;
            self.instances.insert(id.clone(), provider);
        }
        self.instances.get_mut(&id)
    }
}

impl MusicService {
    pub fn new(
        database: Database,
        transport: TransportManager,
        event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    ) -> Self {
        let (send_tx, send_rx) = mpsc::unbounded_channel();
        let (config_tx, _) = watch::channel(());
        spawn_music_sender(transport.clone(), event_tx.clone(), send_rx);

        Self {
            database,
            event_tx,
            send_tx,
            config_tx,
            state: Arc::new(Mutex::new(MusicState {
                running: false,
                cancel: None,
                active_receivers: HashMap::new(),
                snapshot: MusicSnapshot::default(),
            })),
        }
    }

    pub fn notify_config_change(&self) {
        let _ = self.config_tx.send(());
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

    pub async fn handle_request(&self, from_device_id: &str, correlation_id: Option<String>) {
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

        self.send_snapshot_to_device(device_id, &snapshot, correlation_id)
            .await;
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
        let config_rx = self.config_tx.subscribe();
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
            service.run(cancel_rx, config_rx).await;
        });
    }

    async fn run(&self, mut cancel_rx: watch::Receiver<bool>, mut config_rx: watch::Receiver<()>) {
        let mut chain = ProviderChain::new(self.load_enabled_music_provider_ids());
        let mut chain_state = ChainState::Scanning { index: 0 };
        let (lyric_tx, mut lyric_rx) = mpsc::unbounded_channel::<LyricFetchResult>();
        let mut prev_track_key: Option<TrackKey> = None;
        let mut prev_progress: Option<MusicProgressPayload> = None;
        let mut last_progress_push: Option<LastProgressPush> = None;
        let mut request_id = 0_u64;

        info!(
            providers = chain.provider_ids.join(","),
            "music sync loop started"
        );

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

            let wait = match chain_state {
                ChainState::Active { .. } | ChainState::Grace { .. } => ACTIVE_POLL_INTERVAL,
                ChainState::Scanning { .. } => IDLE_POLL_INTERVAL,
            };

            tokio::select! {
                changed = config_rx.changed() => {
                    if changed.is_err() {
                        continue;
                    }
                    chain.reload(self.load_enabled_music_provider_ids());
                    request_id += 1;
                    prev_track_key = None;
                    prev_progress = None;
                    last_progress_push = None;
                    self.clear_playback_state().await;
                    chain_state = ChainState::Scanning { index: 0 };
                }
                changed = cancel_rx.changed() => {
                    if changed.is_ok() && is_cancelled(&cancel_rx) {
                        break;
                    }
                }
                _ = tokio::time::sleep(wait) => {
                    match chain_state {
                        ChainState::Scanning { index } => {
                            if let Some(scanned) = scan_for_active_track(&mut chain, index).await {
                                chain_state = ChainState::Active { index: scanned.index };
                                if let Some(request) = handle_scanned_track(
                                    self,
                                    scanned,
                                    &mut prev_track_key,
                                    &mut prev_progress,
                                    &mut last_progress_push,
                                    &mut request_id,
                                )
                                .await
                                {
                                    spawn_lyric_request(&chain, request, &lyric_tx);
                                }
                            } else {
                                if prev_track_key.is_some() {
                                    self.clear_playback_state().await;
                                    request_id += 1;
                                    prev_track_key = None;
                                    prev_progress = None;
                                    last_progress_push = None;
                                }
                                chain_state = ChainState::Scanning { index: 0 };
                            }
                        }
                        ChainState::Active { index } => {
                            match fetch_provider_track(&mut chain, index).await {
                                Some(TrackState::Active(track)) => {
                                    let provider_id = chain
                                        .provider_id(index)
                                        .map(str::to_string)
                                        .unwrap_or_default();
                                    if let Some(request) = handle_scanned_track(
                                        self,
                                        ScannedTrack {
                                            index,
                                            provider_id,
                                            track,
                                        },
                                        &mut prev_track_key,
                                        &mut prev_progress,
                                        &mut last_progress_push,
                                        &mut request_id,
                                    )
                                    .await
                                    {
                                        spawn_lyric_request(&chain, request, &lyric_tx);
                                    }
                                }
                                _ => {
                                    chain_state = ChainState::Grace {
                                        index,
                                        since: Instant::now(),
                                    };
                                }
                            }
                        }
                        ChainState::Grace { index, since } => {
                            match fetch_provider_track(&mut chain, index).await {
                                Some(TrackState::Active(track)) => {
                                    chain_state = ChainState::Active { index };
                                    let provider_id = chain
                                        .provider_id(index)
                                        .map(str::to_string)
                                        .unwrap_or_default();
                                    if let Some(request) = handle_scanned_track(
                                        self,
                                        ScannedTrack {
                                            index,
                                            provider_id,
                                            track,
                                        },
                                        &mut prev_track_key,
                                        &mut prev_progress,
                                        &mut last_progress_push,
                                        &mut request_id,
                                    )
                                    .await
                                    {
                                        spawn_lyric_request(&chain, request, &lyric_tx);
                                    }
                                }
                                _ if since.elapsed() < GRACE_PERIOD => {}
                                _ => {
                                    chain_state = ChainState::Scanning { index: 0 };
                                }
                            }
                        }
                    }
                }
                result = lyric_rx.recv() => {
                    if let Some(result) = result {
                        let active_ids = self.prune_active_receivers();
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

    fn load_enabled_music_provider_ids(&self) -> Vec<String> {
        match self.database.load_music_providers() {
            Ok(providers) => providers
                .into_iter()
                .filter(|provider| provider.enabled)
                .map(|provider| provider.id)
                .collect(),
            Err(error) => {
                self.log_warn(format!("failed to load music providers: {error}"));
                Vec::new()
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

    async fn publish_track_change(&self, track: &ActiveTrack) {
        let track_payload = build_track_payload(track);
        let progress = build_progress_payload(track);
        {
            let mut guard = self.state.lock_unpoisoned();
            guard.snapshot.track = Some(track_payload.clone());
            guard.snapshot.lyric = None;
            guard.snapshot.progress = progress.clone();
        }

        let active_ids = self.prune_active_receivers();
        for device_id in active_ids {
            self.send_track_message(&device_id, &track_payload).await;
            if let Some(progress) = &progress {
                self.send_progress_message(&device_id, progress).await;
            }
        }

        if let Some(track_id) = track.track_id.as_deref() {
            debug!(track_id = %track_id, source = track.source, "music track changed");
        } else {
            debug!(source = track.source, "music track changed");
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

    async fn send_snapshot_to_device(
        &self,
        device_id: &str,
        snapshot: &MusicSnapshot,
        correlation_id: Option<String>,
    ) {
        let Some(track) = &snapshot.track else {
            let empty = empty_track_payload();
            self.send_track_message_with_correlation(device_id, &empty, correlation_id)
                .await;
            return;
        };

        self.send_track_message_with_correlation(device_id, track, correlation_id.clone())
            .await;
        if let Some(lyric) = &snapshot.lyric {
            self.send_lyric_message_with_correlation(device_id, lyric, correlation_id.clone())
                .await;
        }
        if let Some(progress) = &snapshot.progress {
            self.send_progress_message(device_id, progress).await;
        }
    }

    async fn send_track_message(&self, device_id: &str, payload: &MusicTrackPayload) {
        self.send_track_message_with_correlation(device_id, payload, None)
            .await;
    }

    async fn send_track_message_with_correlation(
        &self,
        device_id: &str,
        payload: &MusicTrackPayload,
        correlation_id: Option<String>,
    ) {
        if let Err(error) =
            self.queue_music_message(device_id, "track", MUSIC_TRACK_TYPE, payload, correlation_id)
        {
            self.log_warn(format!(
                "failed to send music track to {device_id}: {error}"
            ));
        }
    }

    async fn send_lyric_message(&self, device_id: &str, payload: &MusicLyricPayload) {
        self.send_lyric_message_with_correlation(device_id, payload, None)
            .await;
    }

    async fn send_lyric_message_with_correlation(
        &self,
        device_id: &str,
        payload: &MusicLyricPayload,
        correlation_id: Option<String>,
    ) {
        if let Err(error) =
            self.queue_music_message(device_id, "lyric", MUSIC_LYRIC_TYPE, payload, correlation_id)
        {
            self.log_warn(format!(
                "failed to send music lyric to {device_id}: {error}"
            ));
        }
    }

    async fn send_progress_message(&self, device_id: &str, payload: &MusicProgressPayload) {
        if let Err(error) =
            self.queue_music_message(device_id, "progress", MUSIC_PROGRESS_TYPE, payload, None)
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
        correlation_id: Option<String>,
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
                correlation_id,
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

async fn handle_active_track(
    service: &MusicService,
    track: ActiveTrack,
    provider_id: String,
    prev_track_key: &mut Option<TrackKey>,
    prev_progress: &mut Option<MusicProgressPayload>,
    last_progress_push: &mut Option<LastProgressPush>,
    request_id: &mut u64,
) -> Option<LyricFetchRequest> {
    let key = track_key(&track);
    let track_changed = prev_track_key.as_ref() != Some(&key);

    if track_changed {
        *request_id += 1;
        *prev_track_key = Some(key);
        let progress = build_progress_payload(&track);
        *prev_progress = progress.clone();
        *last_progress_push = progress.map(|payload| LastProgressPush {
            payload,
            sent_at: Instant::now(),
        });

        service.publish_track_change(&track).await;
        return track
            .track_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(|track_id| LyricFetchRequest {
                request_id: *request_id,
                track_id: track_id.to_string(),
                provider_id,
            });
    }

    let progress = build_progress_payload(&track);
    if progress_payload_changed(prev_progress.as_ref(), progress.as_ref()) {
        *prev_progress = progress.clone();
        match progress {
            Some(payload) => {
                let now = Instant::now();
                let lyric = service.current_lyric_for_track(&payload.track_id);
                let should_push = should_push_progress(
                    &payload,
                    last_progress_push.as_ref(),
                    lyric.as_ref(),
                    now,
                );
                service.update_progress(payload.clone(), should_push).await;
                if should_push {
                    *last_progress_push = Some(LastProgressPush {
                        payload,
                        sent_at: now,
                    });
                }
            }
            None => {
                service.clear_progress_snapshot();
                *last_progress_push = None;
            }
        }
    }

    None
}

async fn handle_scanned_track(
    service: &MusicService,
    scanned: ScannedTrack,
    prev_track_key: &mut Option<TrackKey>,
    prev_progress: &mut Option<MusicProgressPayload>,
    last_progress_push: &mut Option<LastProgressPush>,
    request_id: &mut u64,
) -> Option<LyricFetchRequest> {
    handle_active_track(
        service,
        scanned.track,
        scanned.provider_id,
        prev_track_key,
        prev_progress,
        last_progress_push,
        request_id,
    )
    .await
}

fn spawn_lyric_request(
    chain: &ProviderChain,
    request: LyricFetchRequest,
    lyric_tx: &mpsc::UnboundedSender<LyricFetchResult>,
) {
    if let Some(provider) = chain.instances.get(&request.provider_id) {
        let future = provider.fetch_lyrics(&request.track_id);
        spawn_lyric_fetch(
            request.request_id,
            request.track_id,
            future,
            lyric_tx.clone(),
        );
    }
}

async fn scan_for_active_track(
    chain: &mut ProviderChain,
    start_index: usize,
) -> Option<ScannedTrack> {
    for index in start_index..chain.len() {
        let provider_id = chain.provider_id(index)?.to_string();
        match fetch_provider_track(chain, index).await {
            Some(TrackState::Active(track)) => {
                return Some(ScannedTrack {
                    index,
                    provider_id,
                    track,
                });
            }
            Some(TrackState::None) | None => {}
        }
    }
    None
}

async fn fetch_provider_track(chain: &mut ProviderChain, index: usize) -> Option<TrackState> {
    let provider = chain.get_or_create(index)?;
    Some(provider.fetch_track().await)
}

fn spawn_lyric_fetch(
    request_id: u64,
    track_id: String,
    future: LyricFuture,
    lyric_tx: mpsc::UnboundedSender<LyricFetchResult>,
) {
    tauri::async_runtime::spawn(async move {
        if let Some(payload) = future.await {
            let _ = lyric_tx.send(LyricFetchResult {
                request_id,
                track_id,
                payload,
            });
        }
    });
}

fn spawn_music_sender(
    transport: TransportManager,
    event_tx: mpsc::UnboundedSender<RuntimeEvent>,
    mut send_rx: mpsc::UnboundedReceiver<MusicSendJob>,
) {
    tauri::async_runtime::spawn(async move {
        while let Some(job) = send_rx.recv().await {
            if let Err(error) = transport
                .send(&job.device_id, job.envelope, job.correlation_id)
                .await
            {
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

fn build_track_payload(track: &ActiveTrack) -> MusicTrackPayload {
    MusicTrackPayload {
        track_id: track.track_id.clone(),
        title: non_empty(track.title.as_str()),
        artists: if track.artists.is_empty() {
            None
        } else {
            Some(track.artists.clone())
        },
        album: track.album.clone(),
        source: Some(track.source.to_string()),
        cover_url: track.cover_url.clone(),
        cover_data: track.cover_data.clone(),
        duration: track.duration_ms,
    }
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

fn build_progress_payload(track: &ActiveTrack) -> Option<MusicProgressPayload> {
    let track_id = track.track_id.clone()?;
    Some(MusicProgressPayload {
        track_id,
        progress: track.progress_ms,
        paused: track.paused,
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

fn track_key(track: &ActiveTrack) -> TrackKey {
    TrackKey {
        track_id: track.track_id.clone(),
        title: track.title.clone(),
        artists: track.artists.clone(),
        album: track.album.clone(),
        source: track.source.to_string(),
        cover_url: track.cover_url.clone(),
        duration_ms: track.duration_ms,
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
