mod gsmtc;
mod lyrics;
mod search;

use std::{
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lru::LruCache;
use reqwest::Client;

use crate::{
    music::provider::{ActiveTrack, LyricFuture, MusicProvider, TrackState},
    protocol::MusicLyricPayload,
    sync::MutexExt,
};

use self::{gsmtc::GsmTrack, search::QqMusicTrack};

const PROVIDER_ID: &str = "qqmusic";
const LYRIC_CACHE_CAPACITY: usize = 64;
const TRACK_CACHE_CAPACITY: usize = 32;

pub struct QqMusicProvider {
    client: Client,
    track_cache: Arc<Mutex<LruCache<String, QqMusicTrack>>>,
    lyric_cache: Arc<Mutex<LruCache<String, MusicLyricPayload>>>,
}

impl QqMusicProvider {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            track_cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(TRACK_CACHE_CAPACITY).expect("cache capacity must be non-zero"),
            ))),
            lyric_cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(LYRIC_CACHE_CAPACITY).expect("cache capacity must be non-zero"),
            ))),
        }
    }
}

#[async_trait]
impl MusicProvider for QqMusicProvider {
    async fn fetch_track(&mut self) -> TrackState {
        let Some(gsm_track) = gsmtc::fetch_current_track() else {
            return TrackState::None;
        };

        let matched = self.match_track(&gsm_track).await;
        TrackState::Active(ActiveTrack {
            track_id: matched
                .as_ref()
                .map(|track| track.song_mid.clone())
                .or_else(|| gsm_track.fallback_track_id.clone()),
            title: matched
                .as_ref()
                .and_then(|track| track.title.clone())
                .unwrap_or_else(|| gsm_track.title.clone()),
            artists: matched
                .as_ref()
                .map(|track| track.artists.clone())
                .filter(|artists| !artists.is_empty())
                .unwrap_or_else(|| gsm_track.artists.clone()),
            album: matched
                .as_ref()
                .and_then(|track| track.album.clone())
                .or(gsm_track.album),
            source: PROVIDER_ID,
            cover_url: matched.as_ref().and_then(|track| track.cover_url.clone()),
            cover_data: None,
            duration_ms: matched
                .as_ref()
                .and_then(|track| track.duration_ms)
                .or(gsm_track.duration_ms),
            progress_ms: gsm_track.progress_ms.unwrap_or(0),
            paused: gsm_track.paused,
        })
    }

    fn fetch_lyrics(&self, track_id: &str) -> LyricFuture {
        let track_id = track_id.trim().to_string();
        let client = self.client.clone();
        let cache = self.lyric_cache.clone();
        Box::pin(async move {
            if track_id.is_empty() {
                return None;
            }

            if let Some(cached) = cache.lock_unpoisoned().get(&track_id).cloned() {
                return Some(cached);
            }

            let payload = lyrics::fetch_lyric(&client, &track_id).await?;
            cache
                .lock_unpoisoned()
                .put(track_id.clone(), payload.clone());
            Some(payload)
        })
    }
}

impl QqMusicProvider {
    async fn match_track(&self, track: &GsmTrack) -> Option<QqMusicTrack> {
        let cache_key = track.cache_key();
        if let Some(cached) = self.track_cache.lock_unpoisoned().get(&cache_key).cloned() {
            return Some(cached);
        }

        let matched = search::search_track(&self.client, track).await?;
        self.track_cache
            .lock_unpoisoned()
            .put(cache_key, matched.clone());
        Some(matched)
    }
}
