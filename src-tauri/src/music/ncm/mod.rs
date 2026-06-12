mod cdp;
mod lyrics;

use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lru::LruCache;

use crate::{
    music::provider::{ActiveTrack, LyricFuture, MusicProvider, TrackState},
    protocol::MusicLyricPayload,
    sync::MutexExt,
};

use self::cdp::{CdpDetector, PlaybackStatus};

const PROVIDER_ID: &str = "ncm";
const LYRIC_CACHE_CAPACITY: usize = 64;

pub struct NcmProvider {
    detector: CdpDetector,
    lyric_cache: Arc<Mutex<LruCache<String, MusicLyricPayload>>>,
}

impl NcmProvider {
    pub fn new() -> Self {
        Self {
            detector: CdpDetector::new(),
            lyric_cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(LYRIC_CACHE_CAPACITY).expect("cache capacity must be non-zero"),
            ))),
        }
    }
}

#[async_trait]
impl MusicProvider for NcmProvider {
    async fn fetch_track(&mut self) -> TrackState {
        let state = self.detector.poll().await;
        if !matches!(state.status, PlaybackStatus::Active) {
            return TrackState::None;
        }

        TrackState::Active(ActiveTrack {
            track_id: state.track_id,
            title: state.title,
            artists: state.artists,
            album: state.album,
            source: PROVIDER_ID,
            cover_url: state.cover_url,
            cover_data: None,
            duration_ms: state.duration_ms,
            progress_ms: state.progress_ms.unwrap_or(0),
            paused: state.playing_state != 2,
        })
    }

    fn fetch_lyrics(&self, track_id: &str) -> LyricFuture {
        let track_id = track_id.trim().to_string();
        let cache = self.lyric_cache.clone();
        Box::pin(async move {
            if track_id.is_empty() {
                return None;
            }

            if let Some(cached) = cache.lock_unpoisoned().get(&track_id).cloned() {
                return Some(cached);
            }

            let payload = lyrics::fetch_lyric(&track_id).await?;
            cache
                .lock_unpoisoned()
                .put(track_id.clone(), payload.clone());
            Some(payload)
        })
    }
}
