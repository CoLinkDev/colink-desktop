use std::{future::Future, pin::Pin};

use async_trait::async_trait;

use crate::protocol::MusicLyricPayload;

use super::{
    kugou::KugouProvider, ncm::NcmProvider, qqmusic::QqMusicProvider, spotify::SpotifyProvider,
};

pub enum TrackState {
    Active(ActiveTrack),
    None,
}

pub struct ActiveTrack {
    pub track_id: Option<String>,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub source: &'static str,
    pub cover_url: Option<String>,
    pub cover_data: Option<String>,
    pub duration_ms: Option<i64>,
    pub progress_ms: i64,
    pub paused: bool,
}

pub type LyricFuture = Pin<Box<dyn Future<Output = Option<MusicLyricPayload>> + Send>>;

#[async_trait]
pub trait MusicProvider: Send {
    async fn fetch_track(&mut self) -> TrackState;

    fn fetch_lyrics(&self, track_id: &str) -> LyricFuture;
}

pub struct KnownProvider {
    pub id: &'static str,
    pub name: &'static str,
    pub implemented: bool,
}

pub const KNOWN_PROVIDERS: &[KnownProvider] = &[
    KnownProvider {
        id: "qqmusic",
        name: "QQ Music",
        implemented: !cfg!(target_os = "linux"),
    },
    KnownProvider {
        id: "kugou",
        name: "Kugou Music",
        implemented: !cfg!(target_os = "linux"),
    },
    KnownProvider {
        id: "ncm",
        name: "NetEase Cloud Music",
        implemented: !cfg!(target_os = "linux"),
    },
    KnownProvider {
        id: "spotify",
        name: "Spotify",
        implemented: true,
    },
];

pub fn create_provider(id: &str) -> Option<Box<dyn MusicProvider>> {
    match id {
        "qqmusic" => Some(Box::new(QqMusicProvider::new())),
        "kugou" => Some(Box::new(KugouProvider::new())),
        "ncm" => Some(Box::new(NcmProvider::new())),
        "spotify" => Some(Box::new(SpotifyProvider::new())),
        _ => None,
    }
}

pub fn known_provider(id: &str) -> Option<&'static KnownProvider> {
    KNOWN_PROVIDERS.iter().find(|provider| provider.id == id)
}
