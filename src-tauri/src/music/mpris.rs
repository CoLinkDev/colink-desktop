use std::time::Duration;

use mpris::{PlaybackStatus, PlayerFinder};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone)]
pub(super) struct MprisTrack {
    pub fallback_track_id: Option<String>,
    pub title: String,
    pub artist: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub progress_ms: Option<i64>,
    pub paused: bool,
}

pub(super) fn fetch_spotify_track() -> Option<MprisTrack> {
    let finder = PlayerFinder::new().ok()?;
    let players = finder.find_all().ok()?;

    players
        .into_iter()
        .find(|player| player_is_spotify(player.bus_name(), player.identity()))
        .and_then(read_track)
}

fn player_is_spotify(bus_name: &str, identity: &str) -> bool {
    let bus_name = bus_name.to_ascii_lowercase();
    let identity = identity.to_ascii_lowercase();
    bus_name.contains("spotify") || identity.contains("spotify")
}

fn read_track(player: mpris::Player) -> Option<MprisTrack> {
    let status = player.get_playback_status().ok()?;
    if status == PlaybackStatus::Stopped {
        return None;
    }

    let metadata = player.get_metadata().ok()?;
    let title = metadata.title().and_then(text)?;
    let artists = metadata
        .artists()
        .unwrap_or_default()
        .into_iter()
        .filter_map(text)
        .collect::<Vec<_>>();
    let artist = (!artists.is_empty()).then(|| artists.join(" / "));
    let album = metadata.album_name().and_then(text);
    let duration_ms = metadata.length().and_then(to_millis);
    let progress_ms = player.get_position().ok().and_then(to_millis);

    Some(MprisTrack {
        fallback_track_id: stable_track_id(
            Some(title.as_str()),
            artist.as_deref(),
            album.as_deref(),
            duration_ms,
        ),
        title,
        artist,
        artists,
        album,
        duration_ms,
        progress_ms,
        paused: status != PlaybackStatus::Playing,
    })
}

fn text(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn to_millis(value: Duration) -> Option<i64> {
    i64::try_from(value.as_millis()).ok()
}

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

#[cfg(test)]
mod tests {
    use super::player_is_spotify;

    #[test]
    fn matches_spotify_bus_name_or_identity() {
        assert!(player_is_spotify(
            "org.mpris.MediaPlayer2.spotify",
            "Spotify"
        ));
        assert!(!player_is_spotify(
            "org.mpris.MediaPlayer2.vlc",
            "VLC media player"
        ));
    }
}
