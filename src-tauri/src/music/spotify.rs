mod gsmtc;
mod lyrics;

use std::{
    fs,
    num::NonZeroUsize,
    path::PathBuf,
    sync::{Arc, Mutex},
};

use async_trait::async_trait;
use lru::LruCache;
use musixmatch_inofficial::Musixmatch;

use crate::{
    music::provider::{ActiveTrack, LyricFuture, MusicProvider, TrackState},
    protocol::MusicLyricPayload,
    sync::MutexExt,
};

use self::gsmtc::GsmTrack;

const PROVIDER_ID: &str = "spotify";
const TRACK_CACHE_CAPACITY: usize = 32;
const LYRIC_CACHE_CAPACITY: usize = 64;

pub struct SpotifyProvider {
    client: Musixmatch,
    track_cache: Arc<Mutex<LruCache<String, SpotifyTrack>>>,
    lyric_cache: Arc<Mutex<LruCache<String, MusicLyricPayload>>>,
}

#[derive(Clone)]
struct SpotifyTrack {
    track_id: String,
    mxm_track_id: u64,
    title: String,
    artists: Vec<String>,
    album: Option<String>,
    duration_ms: Option<i64>,
}

impl SpotifyProvider {
    pub fn new() -> Self {
        Self {
            client: musixmatch_client(),
            track_cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(TRACK_CACHE_CAPACITY).expect("cache capacity must be non-zero"),
            ))),
            lyric_cache: Arc::new(Mutex::new(LruCache::new(
                NonZeroUsize::new(LYRIC_CACHE_CAPACITY).expect("cache capacity must be non-zero"),
            ))),
        }
    }
}

fn musixmatch_client() -> Musixmatch {
    let Some(path) = musixmatch_session_path() else {
        return Musixmatch::builder()
            .no_storage()
            .build()
            .unwrap_or_else(|_| Musixmatch::default());
    };

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }

    Musixmatch::builder()
        .storage_file(path)
        .build()
        .unwrap_or_else(|_| Musixmatch::default())
}

fn musixmatch_session_path() -> Option<PathBuf> {
    let mut dir = dirs::data_dir()?;
    dir.push(if cfg!(debug_assertions) {
        "dev.colink.desktop.debug"
    } else {
        "dev.colink.desktop"
    });
    Some(dir.join("musixmatch_session.json"))
}

#[async_trait]
impl MusicProvider for SpotifyProvider {
    async fn fetch_track(&mut self) -> TrackState {
        let Some(gsm_track) = gsmtc::fetch_current_track() else {
            return TrackState::None;
        };

        let matched = self.match_track(&gsm_track).await;
        TrackState::Active(ActiveTrack {
            track_id: matched
                .as_ref()
                .map(|track| track.track_id.clone())
                .or_else(|| gsm_track.fallback_track_id.clone()),
            title: matched
                .as_ref()
                .map(|track| track.title.clone())
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
            cover_url: None,
            cover_data: gsm_track.cover_data,
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
        let track_cache = self.track_cache.clone();
        Box::pin(async move {
            if track_id.is_empty() {
                return None;
            }

            if let Some(cached) = cache.lock_unpoisoned().get(&track_id).cloned() {
                return Some(cached);
            }

            let meta = track_cache
                .lock_unpoisoned()
                .iter()
                .find_map(|(_, track)| (track.track_id == track_id).then_some(track.clone()));

            let Some(meta) = meta else {
                return None;
            };

            let lyrics = lyrics::fetch_lyrics(&client, &meta).await?;
            cache
                .lock_unpoisoned()
                .put(track_id.clone(), lyrics.clone());
            Some(lyrics)
        })
    }
}

impl SpotifyProvider {
    async fn match_track(&self, track: &GsmTrack) -> Option<SpotifyTrack> {
        let cache_key = track.cache_key();
        if let Some(cached) = self.track_cache.lock_unpoisoned().get(&cache_key).cloned() {
            return Some(cached);
        }

        let matched = lyrics::resolve_track(&self.client, track).await?;
        self.track_cache
            .lock_unpoisoned()
            .put(cache_key, matched.clone());
        Some(matched)
    }
}
