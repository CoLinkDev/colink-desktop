use reqwest::Client;
use serde_json::{json, Value};

use super::gsmtc::GsmTrack;

const SEARCH_URL: &str = "https://u.y.qq.com/cgi-bin/musicu.fcg";

#[derive(Debug, Clone)]
pub struct QqMusicTrack {
    pub song_mid: String,
    pub title: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub cover_url: Option<String>,
}

pub async fn search_track(client: &Client, track: &GsmTrack) -> Option<QqMusicTrack> {
    let query = search_query(track);
    if query.is_empty() {
        return None;
    }

    let payload = json!({
        "comm": {
            "ct": "19",
            "cv": "1859",
            "uin": "0",
        },
        "req": {
            "method": "DoSearchForQQMusicDesktop",
            "module": "music.search.SearchCgiService",
            "param": {
                "grp": 1,
                "num_per_page": 10,
                "page_num": 1,
                "query": query,
                "search_type": 0,
            },
        },
    });

    let response = client
        .post(SEARCH_URL)
        .header("User-Agent", "Mozilla/5.0")
        .header("Referer", "https://y.qq.com/")
        .json(&payload)
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()?;

    let songs = response
        .get("req")?
        .get("data")?
        .get("body")?
        .get("song")?
        .get("list")?
        .as_array()?;

    songs
        .iter()
        .filter_map(parse_track)
        .map(|candidate| {
            let score = score_track(track, &candidate);
            (score, candidate)
        })
        .filter(|(score, _)| *score > 0)
        .max_by_key(|(score, _)| *score)
        .map(|(_, candidate)| candidate)
}

fn search_query(track: &GsmTrack) -> String {
    [Some(track.title.as_str()), track.artist.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_track(value: &Value) -> Option<QqMusicTrack> {
    let song_mid = first_text([value.get("mid"), value.get("songmid")])?;
    let album = value.get("album").and_then(Value::as_object);
    let album_mid = first_text([
        album.and_then(|item| item.get("mid")),
        album.and_then(|item| item.get("pmid")),
    ]);

    let artists = value
        .get("singer")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| first_text([item.get("title"), item.get("name")]))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(QqMusicTrack {
        song_mid,
        title: first_text([value.get("title"), value.get("name")]),
        artists,
        album: first_text([
            album.and_then(|item| item.get("title")),
            album.and_then(|item| item.get("name")),
        ]),
        duration_ms: value
            .get("interval")
            .and_then(Value::as_i64)
            .map(|value| value * 1000),
        cover_url: album_mid
            .map(|mid| format!("https://y.gtimg.cn/music/photo_new/T002R300x300M000{mid}.jpg")),
    })
}

fn score_track(source: &GsmTrack, candidate: &QqMusicTrack) -> i64 {
    let mut score = 0_i64;
    let source_title = normalize(&source.title);
    let candidate_title = normalize(candidate.title.as_deref().unwrap_or_default());

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
    let candidate_artists = candidate
        .artists
        .iter()
        .map(|item| normalize(item))
        .collect::<Vec<_>>();
    if source_artists.iter().any(|source| {
        candidate_artists
            .iter()
            .any(|candidate| source == candidate)
    }) {
        score += 40;
    }

    if let (Some(source_duration), Some(candidate_duration)) =
        (source.duration_ms, candidate.duration_ms)
    {
        let diff = (source_duration - candidate_duration).abs();
        if diff <= 2_000 {
            score += 20;
        } else if diff <= 5_000 {
            score += 10;
        }
    }

    score
}

fn first_text<'a, I>(values: I) -> Option<String>
where
    I: IntoIterator<Item = Option<&'a Value>>,
{
    for value in values {
        let Some(text) = value.and_then(Value::as_str) else {
            continue;
        };
        let text = text.trim();
        if !text.is_empty() {
            return Some(text.to_string());
        }
    }
    None
}

fn normalize(value: &str) -> String {
    value.trim().to_lowercase()
}
