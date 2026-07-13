#[cfg(windows)]
use sha2::{Digest, Sha256};

#[cfg(windows)]
use base64::{engine::general_purpose::STANDARD, Engine as _};

#[cfg(windows)]
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSession, GlobalSystemMediaTransportControlsSessionManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus,
};

#[cfg(windows)]
use windows::Storage::Streams::{DataReader, IRandomAccessStreamReference, InputStreamOptions};

#[cfg(windows)]
const MAX_THUMBNAIL_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct GsmTrack {
    pub fallback_track_id: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub cover_data: Option<String>,
    pub duration_ms: Option<i64>,
    pub progress_ms: Option<i64>,
    pub paused: bool,
}

impl GsmTrack {
    pub fn cache_key(&self) -> String {
        [
            self.title.trim(),
            self.artist.as_deref().unwrap_or_default().trim(),
            self.album.as_deref().unwrap_or_default().trim(),
            &self
                .duration_ms
                .map(|value| value.to_string())
                .unwrap_or_default(),
        ]
        .join("\u{1f}")
    }
}

#[cfg(windows)]
pub fn fetch_current_track() -> Option<GsmTrack> {
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .ok()?
        .get()
        .ok()?;
    let sessions = manager.GetSessions().ok()?;
    let mut selected: Option<GlobalSystemMediaTransportControlsSession> = None;

    for index in 0..sessions.Size().ok()? {
        let session = sessions.GetAt(index).ok()?;
        if looks_like_spotify_session(&session) {
            selected = Some(session);
            break;
        }
    }

    if selected.is_none() {
        if let Ok(current) = manager.GetCurrentSession() {
            if looks_like_spotify_session(&current) {
                selected = Some(current);
            }
        }
    }

    let session = selected?;
    read_session(&session)
}

#[cfg(target_os = "linux")]
pub fn fetch_current_track() -> Option<GsmTrack> {
    crate::music::mpris::fetch_spotify_track().map(|track| GsmTrack {
        fallback_track_id: track.fallback_track_id,
        title: track.title,
        artist: track.artist,
        artists: track.artists,
        album: track.album,
        cover_data: None,
        duration_ms: track.duration_ms,
        progress_ms: track.progress_ms,
        paused: track.paused,
    })
}

#[cfg(all(not(windows), not(target_os = "linux")))]
pub fn fetch_current_track() -> Option<GsmTrack> {
    None
}

#[cfg(windows)]
fn read_session(session: &GlobalSystemMediaTransportControlsSession) -> Option<GsmTrack> {
    let playback = session.GetPlaybackInfo().ok()?;
    let status = playback.PlaybackStatus().ok()?;
    if matches!(
        status,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus::Closed
            | GlobalSystemMediaTransportControlsSessionPlaybackStatus::Stopped
    ) {
        return None;
    }

    let props = session.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
    let timeline = session.GetTimelineProperties().ok()?;
    let title = hstring_text(props.Title().ok());
    let artist = hstring_text(props.Artist().ok());
    let album = hstring_text(props.AlbumTitle().ok());
    let cover_data = props
        .Thumbnail()
        .ok()
        .and_then(|thumbnail| read_thumbnail_base64(&thumbnail));
    let duration_ms = timespan_ms(timeline.EndTime().ok());
    let progress_ms = timespan_ms(timeline.Position().ok());
    let artists = split_artists(artist.as_deref());
    let fallback_track_id = stable_track_id(
        title.as_deref(),
        artist.as_deref(),
        album.as_deref(),
        duration_ms,
    );

    let title = title?;
    Some(GsmTrack {
        fallback_track_id,
        title,
        artist,
        artists,
        album,
        cover_data,
        duration_ms,
        progress_ms,
        paused: status != GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing,
    })
}

#[cfg(windows)]
fn looks_like_spotify_session(session: &GlobalSystemMediaTransportControlsSession) -> bool {
    let source = session
        .SourceAppUserModelId()
        .ok()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    source.contains("spotify")
}

#[cfg(windows)]
fn hstring_text(value: Option<windows::core::HSTRING>) -> Option<String> {
    value
        .map(|item| item.to_string_lossy())
        .map(|item| item.trim().to_string())
        .filter(|item| !item.is_empty())
}

#[cfg(windows)]
fn timespan_ms(value: Option<windows::Foundation::TimeSpan>) -> Option<i64> {
    value.map(|item| item.Duration / 10_000)
}

#[cfg(windows)]
fn read_thumbnail_base64(thumbnail: &IRandomAccessStreamReference) -> Option<String> {
    let stream = thumbnail.OpenReadAsync().ok()?.get().ok()?;
    let size = usize::try_from(stream.Size().ok()?).ok()?;
    if size == 0 || size > MAX_THUMBNAIL_BYTES {
        return None;
    }

    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader
        .SetInputStreamOptions(InputStreamOptions::Partial)
        .ok()?;
    let loaded = reader.LoadAsync(size as u32).ok()?.get().ok()? as usize;
    if loaded == 0 || loaded > MAX_THUMBNAIL_BYTES {
        return None;
    }

    let mut bytes = vec![0_u8; loaded];
    reader.ReadBytes(&mut bytes).ok()?;
    Some(STANDARD.encode(bytes))
}

#[cfg(windows)]
fn split_artists(value: Option<&str>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    for separator in [" / ", "、", ";", "；", ",", "，"] {
        if value.contains(separator) {
            return value
                .split(separator)
                .map(str::trim)
                .filter(|item| !item.is_empty())
                .map(ToString::to_string)
                .collect();
        }
    }
    let value = value.trim();
    if value.is_empty() {
        Vec::new()
    } else {
        vec![value.to_string()]
    }
}

#[cfg(windows)]
fn stable_track_id(
    title: Option<&str>,
    artist: Option<&str>,
    album: Option<&str>,
    duration_ms: Option<i64>,
) -> Option<String> {
    let title = title.unwrap_or_default().trim();
    let artist = artist.unwrap_or_default().trim();
    if title.is_empty() && artist.is_empty() {
        return None;
    }

    let raw = [
        title,
        artist,
        album.unwrap_or_default().trim(),
        &duration_ms
            .map(|value| value.to_string())
            .unwrap_or_default(),
    ]
    .join("\u{1f}");
    Some(format!("{:x}", Sha256::digest(raw.as_bytes())))
}
