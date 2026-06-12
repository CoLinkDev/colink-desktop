use musixmatch_inofficial::{
    models::{SortOrder, Subtitle, SubtitleFormat, SubtitleLines, Track, TrackId},
    Musixmatch,
};

use crate::{
    music::spotify::SpotifyTrack,
    protocol::{MusicLyricLinePayload, MusicLyricPayload},
};

use super::GsmTrack;

pub async fn resolve_track(client: &Musixmatch, track: &GsmTrack) -> Option<SpotifyTrack> {
    let query = client
        .track_search()
        .q_track(&track.title)
        .q_artist(track.artist.as_deref().unwrap_or_default())
        .f_has_lyrics()
        .s_track_rating(SortOrder::Desc)
        .send(5, 1)
        .await
        .ok()?;

    choose_track(track, query).map(|value| to_track(&value))
}

pub async fn fetch_lyrics(client: &Musixmatch, track: &SpotifyTrack) -> Option<MusicLyricPayload> {
    let subtitle = client
        .track_subtitle(
            TrackId::TrackId(track.mxm_track_id),
            SubtitleFormat::Json,
            track.duration_ms.map(|value| value as f32 / 1000.0),
            Some(2.0),
        )
        .await
        .ok()?;
    subtitle_to_payload(track.track_id.clone(), subtitle)
}

fn choose_track(source: &GsmTrack, tracks: Vec<Track>) -> Option<Track> {
    let mut candidates: Vec<(i64, usize, Track)> = tracks
        .into_iter()
        .enumerate()
        .map(|(index, track)| (score_track(source, &track), index, track))
        .collect();
    candidates.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    candidates
        .into_iter()
        .find(|(score, _, _)| *score > 0)
        .map(|(_, _, track)| track)
}

fn score_track(source: &GsmTrack, candidate: &Track) -> i64 {
    let mut score = 0_i64;
    let source_title = normalize(&source.title);
    let candidate_title = normalize(&candidate.track_name);

    if !source_title.is_empty() && source_title == candidate_title {
        score += 100;
    } else if !source_title.is_empty()
        && (source_title.contains(&candidate_title) || candidate_title.contains(&source_title))
    {
        score += 60;
    }

    let source_artists = source
        .artists
        .iter()
        .map(|item| normalize(item))
        .collect::<Vec<_>>();
    let candidate_artist = normalize(&candidate.artist_name);
    if source_artists.iter().any(|item| item == &candidate_artist) {
        score += 40;
    }

    if let Some(source_duration) = source.duration_ms {
        let candidate_duration = i64::from(candidate.track_length) * 1000;
        let diff = (source_duration - candidate_duration).abs();
        if diff <= 2_000 {
            score += 20;
        } else if diff <= 5_000 {
            score += 10;
        }
    }

    if candidate.has_subtitles {
        score += 10;
    }

    score
}

fn subtitle_to_payload(track_id: String, subtitle: Subtitle) -> Option<MusicLyricPayload> {
    let lines = subtitle.to_lines().ok()?;
    let lines = lines_to_payload(lines);
    if lines.is_empty() {
        return None;
    }
    Some(MusicLyricPayload {
        track_id,
        lines: Some(lines),
        translated_lines: None,
    })
}

fn lines_to_payload(lines: SubtitleLines) -> Vec<MusicLyricLinePayload> {
    lines
        .lines
        .into_iter()
        .map(|line| MusicLyricLinePayload {
            time: line.time.total_ms().into(),
            text: line.text,
        })
        .collect()
}

fn to_track(track: &Track) -> SpotifyTrack {
    SpotifyTrack {
        track_id: track
            .track_spotify_id
            .clone()
            .unwrap_or_else(|| track.track_id.to_string()),
        mxm_track_id: track.track_id,
        title: track.track_name.clone(),
        artists: vec![track.artist_name.clone()],
        album: Some(track.album_name.clone()),
        duration_ms: Some(i64::from(track.track_length) * 1000),
    }
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}
