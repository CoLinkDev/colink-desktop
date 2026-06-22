use base64::{engine::general_purpose::STANDARD, Engine as _};
use reqwest::Client;
use serde_json::Value;

use crate::protocol::{MusicLyricLinePayload, MusicLyricPayload};

use super::search::KugouTrack;

const LYRIC_SEARCH_URL: &str = "http://lyrics.kugou.com/search";
const LYRIC_DOWNLOAD_URL: &str = "http://lyrics.kugou.com/download";

pub async fn fetch_lyric(client: &Client, track: &KugouTrack) -> Option<MusicLyricPayload> {
    let response = client
        .get(LYRIC_SEARCH_URL)
        .header("User-Agent", "Mozilla/5.0")
        .header("Referer", "https://www.kugou.com/")
        .query(&[
            ("ver", "1"),
            ("man", "yes"),
            ("client", "pc"),
            ("keyword", lyric_query(track).as_str()),
            ("duration", &track.duration_ms.unwrap_or_default().to_string()),
            ("hash", track.hash.as_str()),
        ])
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()?;

    if response.get("status").and_then(Value::as_i64) != Some(200) {
        return None;
    }

    let candidate = response
        .get("candidates")?
        .as_array()?
        .iter()
        .filter_map(|value| parse_candidate(track, value))
        .max_by_key(|candidate| candidate.score)?;

    let response = client
        .get(LYRIC_DOWNLOAD_URL)
        .header("User-Agent", "Mozilla/5.0")
        .header("Referer", "https://www.kugou.com/")
        .query(&[
            ("ver", "1"),
            ("client", "pc"),
            ("id", candidate.id.as_str()),
            ("accesskey", candidate.access_key.as_str()),
            ("fmt", "lrc"),
            ("charset", "utf8"),
        ])
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()?;

    if response.get("status").and_then(Value::as_i64) != Some(200) {
        return None;
    }

    let lrc = decode_lyric_content(response.get("content").and_then(Value::as_str)?)?;
    let lines = parse_lrc(&lrc);
    if lines.is_empty() {
        return None;
    }

    Some(MusicLyricPayload {
        track_id: track.hash.clone(),
        lines: Some(lines),
        translated_lines: None,
    })
}

struct LyricCandidate {
    id: String,
    access_key: String,
    score: i64,
}

fn parse_candidate(track: &KugouTrack, value: &Value) -> Option<LyricCandidate> {
    let id = text(value.get("id"))?;
    let access_key = text(value.get("accesskey"))?;
    let mut score = value.get("score").and_then(Value::as_i64).unwrap_or_default();

    let expected_title = normalize(&track.title);
    let candidate_title = normalize(value.get("song").and_then(Value::as_str).unwrap_or_default());
    if !expected_title.is_empty() && expected_title == candidate_title {
        score += 100;
    } else if !expected_title.is_empty()
        && (expected_title.contains(&candidate_title) || candidate_title.contains(&expected_title))
    {
        score += 60;
    }

    let candidate_artist = normalize(
        value
            .get("singer")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    if track
        .artists
        .iter()
        .map(|item| normalize(item))
        .any(|artist| !artist.is_empty() && artist == candidate_artist)
    {
        score += 40;
    }

    if let (Some(expected_duration), Some(candidate_duration)) =
        (track.duration_ms, value.get("duration").and_then(Value::as_i64))
    {
        let candidate_duration = if candidate_duration > 10_000 {
            candidate_duration
        } else {
            candidate_duration * 1000
        };
        let diff = (expected_duration - candidate_duration).abs();
        if diff <= 2_000 {
            score += 20;
        } else if diff <= 5_000 {
            score += 10;
        }
    }

    Some(LyricCandidate {
        id,
        access_key,
        score,
    })
}

fn lyric_query(track: &KugouTrack) -> String {
    if let Some(filename) = track.filename.as_deref().map(str::trim) {
        if !filename.is_empty() {
            return filename.to_string();
        }
    }
    let artist = track.artists.first().map(String::as_str).unwrap_or_default();
    if artist.is_empty() {
        track.title.clone()
    } else {
        format!("{artist} - {}", track.title)
    }
}

fn decode_lyric_content(value: &str) -> Option<String> {
    let bytes = STANDARD.decode(value).ok()?;
    String::from_utf8(bytes).ok()
}

fn parse_lrc(value: &str) -> Vec<MusicLyricLinePayload> {
    let mut lines = Vec::new();
    for raw in value.lines() {
        let text = raw.trim();
        if text.is_empty() || !text.starts_with('[') || text.starts_with("[{") {
            continue;
        }

        let Some(close_index) = text.find(']') else {
            continue;
        };
        let Some(time) = parse_lrc_timestamp(&text[1..close_index]) else {
            continue;
        };
        let lyric_text = text[close_index + 1..].trim();
        if lyric_text.is_empty() {
            continue;
        }

        lines.push(MusicLyricLinePayload {
            time,
            text: lyric_text.to_string(),
        });
    }

    lines.sort_by_key(|line| line.time);
    lines
}

fn parse_lrc_timestamp(value: &str) -> Option<i64> {
    let (minutes, rest) = value.split_once(':')?;
    let minutes = minutes.trim().parse::<i64>().ok()?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    let (seconds, millis) = match rest.find(['.', ':']) {
        Some(index) => {
            let seconds = rest[..index].trim().parse::<i64>().ok()?;
            let mut fraction = rest[index + 1..].trim().to_string();
            if fraction.is_empty() {
                fraction = "0".to_string();
            }
            if fraction.len() > 3 {
                fraction.truncate(3);
            } else {
                while fraction.len() < 3 {
                    fraction.push('0');
                }
            }
            (seconds, fraction.parse::<i64>().ok()?)
        }
        None => (rest.parse::<i64>().ok()?, 0),
    };

    Some((minutes * 60 + seconds) * 1000 + millis)
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase().replace(char::is_whitespace, "")
}

#[cfg(test)]
mod tests {
    use super::{parse_lrc, parse_lrc_timestamp};

    #[test]
    fn parses_lrc_timestamp() {
        assert_eq!(parse_lrc_timestamp("01:23.45"), Some(83_450));
        assert_eq!(parse_lrc_timestamp("1:23"), Some(83_000));
        assert_eq!(parse_lrc_timestamp("bad"), None);
    }

    #[test]
    fn parses_lrc_lines() {
        let lines = parse_lrc("[00:01.00]first\n[00:02.50]second");
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].time, 1_000);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[1].time, 2_500);
    }
}
