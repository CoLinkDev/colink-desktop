use std::time::{SystemTime, UNIX_EPOCH};

use aes::cipher::{generic_array::GenericArray, BlockEncrypt, KeyInit};
use aes::Aes128;
use reqwest::{
    header::{HeaderMap, HeaderValue, CONTENT_TYPE, COOKIE, REFERER, USER_AGENT},
    Client,
};
use serde_json::Value;
use uuid::Uuid;

use crate::protocol::{MusicLyricLinePayload, MusicLyricPayload};

const EAPI_KEY: &[u8; 16] = b"e82ckenh8dichen8";
const LYRIC_URL: &str = "https://interface3.music.163.com/eapi/song/lyric/v1";

pub async fn fetch_lyric(track_id: &str) -> Option<MusicLyricPayload> {
    let track_id = track_id.trim();
    if track_id.is_empty() {
        return None;
    }

    let client = Client::builder().no_proxy().build().ok()?;
    let response = eapi_post(
        &client,
        LYRIC_URL,
        serde_json::json!({
            "id": track_id,
            "cp": "false",
            "lv": "0",
            "kv": "0",
            "tv": "0",
            "rv": "0",
            "yv": "0",
            "ytv": "0",
            "yrv": "0",
            "csrf_token": "",
        }),
    )
    .await
    .ok()?;

    if response.get("code").and_then(Value::as_i64) != Some(200) {
        return None;
    }

    let lrc = lyric_text(response.get("lrc"));
    let tlyric = lyric_text(response.get("tlyric"));
    let lines = parse_lrc(&lrc);
    let translated_lines = parse_lrc(&tlyric);

    if lines.is_empty() && translated_lines.is_empty() {
        return None;
    }

    Some(MusicLyricPayload {
        track_id: track_id.to_string(),
        lines: if lines.is_empty() {
            None
        } else {
            Some(
                lines
                    .into_iter()
                    .map(|line| MusicLyricLinePayload {
                        time: line.time_ms,
                        text: line.text,
                    })
                    .collect(),
            )
        },
        translated_lines: if translated_lines.is_empty() {
            None
        } else {
            Some(
                translated_lines
                    .into_iter()
                    .map(|line| MusicLyricLinePayload {
                        time: line.time_ms,
                        text: line.text,
                    })
                    .collect(),
            )
        },
    })
}

#[derive(Debug, Clone)]
struct LyricLine {
    time_ms: i64,
    text: String,
}

async fn eapi_post(client: &Client, url: &str, mut data: Value) -> Result<Value, reqwest::Error> {
    let header = serde_json::json!({
        "__csrf": "",
        "appver": "8.0.0",
        "buildver": unix_now_seconds().to_string(),
        "channel": "",
        "deviceId": "",
        "mobilename": "",
        "resolution": "1920x1080",
        "os": "android",
        "osver": "",
        "requestId": format!("{}_{}", unix_now_millis(), Uuid::new_v4().simple()),
        "versioncode": "140",
        "MUSIC_U": "",
    });

    let cookie = cookie_string(&header);
    data["header"] = header.clone();

    let path = url
        .trim_start_matches("https://interface3.music.163.com")
        .replacen("/eapi", "/api", 1);
    let text = serde_json::to_string(&data).unwrap_or_else(|_| "{}".to_string());
    let message = format!("nobody{path}use{text}md5forencrypt");
    let digest = format!("{:x}", md5::compute(message.as_bytes()));
    let raw = format!("{path}-36cd479b6b5-{text}-36cd479b6b5-{digest}");
    let encrypted = aes_ecb_encrypt(raw.as_bytes(), EAPI_KEY);
    let body = format!("params={}", hex_encode_upper(&encrypted));

    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static("Mozilla/5.0 (Linux; Android 9; PCT-AL10) AppleWebKit/537.36"),
    );
    headers.insert(REFERER, HeaderValue::from_static("https://music.163.com/"));
    if !cookie.is_empty() {
        headers.insert(
            COOKIE,
            HeaderValue::from_str(&cookie)
                .unwrap_or_else(|_| HeaderValue::from_static("")),
        );
    }
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-www-form-urlencoded"),
    );

    client
        .post(url)
        .headers(headers)
        .body(body)
        .send()
        .await?
        .json::<Value>()
        .await
}

fn cookie_string(header: &Value) -> String {
    let Some(map) = header.as_object() else {
        return String::new();
    };
    map.iter()
        .map(|(key, value)| format!("{key}={}", value.as_str().unwrap_or_default()))
        .collect::<Vec<_>>()
        .join("; ")
}

fn aes_ecb_encrypt(plaintext: &[u8], key: &[u8; 16]) -> Vec<u8> {
    let cipher = Aes128::new(GenericArray::from_slice(key));
    let block_size = 16;
    let pad_len = block_size - (plaintext.len() % block_size);
    let mut buffer = Vec::with_capacity(plaintext.len() + pad_len);
    buffer.extend_from_slice(plaintext);
    buffer.extend(std::iter::repeat(pad_len as u8).take(pad_len));

    for chunk in buffer.chunks_mut(block_size) {
        let block = GenericArray::from_mut_slice(chunk);
        cipher.encrypt_block(block);
    }

    buffer
}

fn hex_encode_upper(bytes: &[u8]) -> String {
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push_str(&format!("{:02X}", byte));
    }
    output
}

fn parse_lrc(lrc: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for raw in lrc.lines() {
        let text = raw.trim();
        if text.is_empty() || !text.starts_with('[') || text.starts_with("[{") {
            continue;
        }

        let Some(close_index) = text.find(']') else {
            continue;
        };
        let Some(time_ms) = parse_lrc_timestamp(&text[1..close_index]) else {
            continue;
        };
        let lyric_text = text[close_index + 1..].trim();
        if lyric_text.is_empty() {
            continue;
        }

        lines.push(LyricLine {
            time_ms,
            text: lyric_text.to_string(),
        });
    }

    lines.sort_by_key(|line| line.time_ms);
    lines
}

fn parse_lrc_timestamp(value: &str) -> Option<i64> {
    let (minutes, rest) = value.split_once(':')?;
    let minutes = minutes.trim().parse::<i64>().ok()?;
    let rest = rest.trim();
    if rest.is_empty() {
        return None;
    }

    let (seconds, millis) = match rest.find(|c| c == '.' || c == ':') {
        Some(index) => {
            let seconds = rest[..index].trim().parse::<i64>().ok()?;
            let fraction = &rest[index + 1..];
            let mut fraction = fraction.trim().to_string();
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
            let millis = fraction.parse::<i64>().ok()?;
            (seconds, millis)
        }
        None => (rest.parse::<i64>().ok()?, 0),
    };

    Some((minutes * 60 + seconds) * 1000 + millis)
}

fn lyric_text(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_object)
        .and_then(|item| item.get("lyric"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

fn unix_now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

fn unix_now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{parse_lrc_timestamp, parse_lrc};

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
        assert_eq!(lines[0].time_ms, 1_000);
        assert_eq!(lines[0].text, "first");
        assert_eq!(lines[1].time_ms, 2_500);
    }
}
