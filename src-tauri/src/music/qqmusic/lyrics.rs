use reqwest::Client;
use serde_json::Value;

use crate::protocol::{MusicLyricLinePayload, MusicLyricPayload};

const LYRIC_URL: &str = "https://i.y.qq.com/lyric/fcgi-bin/fcg_query_lyric_new.fcg";

pub async fn fetch_lyric(client: &Client, track_id: &str) -> Option<MusicLyricPayload> {
    let track_id = track_id.trim();
    if track_id.is_empty() {
        return None;
    }

    let response = client
        .get(LYRIC_URL)
        .header("User-Agent", "Mozilla/5.0")
        .header("Referer", "https://y.qq.com/")
        .query(&[
            ("songmid", track_id),
            ("g_tk", "5381"),
            ("format", "json"),
            ("inCharset", "utf8"),
            ("outCharset", "utf-8"),
            ("nobase64", "1"),
        ])
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()?;

    if !matches!(
        response.get("retcode").and_then(Value::as_i64),
        Some(0) | None
    ) || !matches!(response.get("code").and_then(Value::as_i64), Some(0) | None)
    {
        return None;
    }

    let lines = parse_lrc(lyric_text(response.get("lyric")).as_str());
    let translated_lines = parse_lrc(lyric_text(response.get("trans")).as_str());
    if lines.is_empty() && translated_lines.is_empty() {
        return None;
    }

    Some(MusicLyricPayload {
        track_id: track_id.to_string(),
        lines: if lines.is_empty() { None } else { Some(lines) },
        translated_lines: if translated_lines.is_empty() {
            None
        } else {
            Some(translated_lines)
        },
    })
}

fn lyric_text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
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
