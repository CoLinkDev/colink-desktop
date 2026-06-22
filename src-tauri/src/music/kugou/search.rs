use reqwest::Client;
use serde_json::Value;

use super::gsmtc::GsmTrack;

const SEARCH_URL: &str = "https://songsearch.kugou.com/song_search_v2";

#[derive(Debug, Clone)]
pub struct KugouTrack {
    pub hash: String,
    pub title: String,
    pub filename: Option<String>,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub duration_ms: Option<i64>,
    pub cover_url: Option<String>,
}

pub async fn search_track(client: &Client, track: &GsmTrack) -> Option<KugouTrack> {
    let query = search_query(track);
    if query.is_empty() {
        return None;
    }

    let response = client
        .get(SEARCH_URL)
        .header("User-Agent", "Mozilla/5.0")
        .header("Referer", "https://www.kugou.com/")
        .query(&[
            ("keyword", query.as_str()),
            ("page", "1"),
            ("pagesize", "10"),
            ("platform", "WebFilter"),
            ("iscorrection", "1"),
            ("privilege_filter", "0"),
        ])
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()?;

    let songs = response.get("data")?.get("lists")?.as_array()?;
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

fn parse_track(value: &Value) -> Option<KugouTrack> {
    let hash = first_text([value.get("FileHash"), value.get("HQFileHash")])?;
    let title = first_text([value.get("SongName"), value.get("OriSongName")])?;
    let image = first_text([value.get("Image"), value.get("AlbumImage")])
        .or_else(|| first_text([value.get("trans_param")?.get("union_cover")]));

    Some(KugouTrack {
        hash,
        title,
        filename: first_text([value.get("FileName")]),
        artists: parse_artists(value),
        album: first_text([value.get("AlbumName")]),
        duration_ms: first_i64([
            value.get("Duration"),
            value.get("HQDuration"),
            value.get("ResDuration"),
            value.get("SQDuration"),
        ])
        .map(|value| value * 1000),
        cover_url: image.map(|value| {
            value
                .replace("{size}", "300")
                .replace("\\/", "/")
                .to_string()
        }),
    })
}

fn parse_artists(value: &Value) -> Vec<String> {
    if let Some(items) = value.get("Singers").and_then(Value::as_array) {
        let artists = items
            .iter()
            .filter_map(|item| first_text([item.get("name"), item.get("Name")]))
            .collect::<Vec<_>>();
        if !artists.is_empty() {
            return artists;
        }
    }
    split_artists(first_text([value.get("SingerName")]).as_deref())
}

fn score_track(source: &GsmTrack, candidate: &KugouTrack) -> i64 {
    let mut score = 0_i64;
    let source_title = normalize(&source.title);
    let candidate_title = normalize(&candidate.title);
    let candidate_filename = normalize(candidate.filename.as_deref().unwrap_or_default());

    if !source_title.is_empty() && source_title == candidate_title {
        score += 100;
    } else if !source_title.is_empty()
        && (source_title.contains(&candidate_title) || candidate_title.contains(&source_title))
    {
        score += 60;
    } else if !source_title.is_empty() && candidate_filename.contains(&source_title) {
        score += 50;
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
    if source_artists
        .iter()
        .any(|source| candidate_artists.iter().any(|candidate| source == candidate))
    {
        score += 40;
    } else if source_artists
        .iter()
        .any(|source| !source.is_empty() && candidate_filename.contains(source))
    {
        score += 25;
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

fn first_i64<'a, I>(values: I) -> Option<i64>
where
    I: IntoIterator<Item = Option<&'a Value>>,
{
    for value in values {
        if let Some(value) = value.and_then(Value::as_i64) {
            return Some(value);
        }
    }
    None
}

fn split_artists(value: Option<&str>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    for separator in [" / ", "、", ";", "；", ",", "，", "&"] {
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

fn normalize(value: &str) -> String {
    value.trim().to_lowercase().replace(char::is_whitespace, "")
}
