use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use reqwest::Client;
use serde_json::{json, Value};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::Message, MaybeTlsStream, WebSocketStream};
use tracing::{debug, warn};
use url::Url;

use crate::error::{AppError, AppResult};

pub const DEFAULT_HOST: &str = "127.0.0.1";
pub const DEFAULT_PORT: u16 = 9223;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(3);

const RECONNECT_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackStatus {
    Active,
    None,
}

#[derive(Debug, Clone)]
pub struct CdpPlayingState {
    pub status: PlaybackStatus,
    pub track_id: Option<String>,
    pub title: String,
    pub artists: Vec<String>,
    pub album: Option<String>,
    pub cover_url: Option<String>,
    pub duration_ms: Option<i64>,
    pub progress_ms: Option<i64>,
    pub playing_state: i64,
}

pub struct CdpDetector {
    client: Client,
    host: String,
    port: u16,
    timeout: Duration,
    websocket: Option<DevToolsClient>,
    next_reconnect_at: Instant,
    last_connect_error: Option<String>,
    last_poll_error: Option<String>,
}

struct DevToolsClient {
    websocket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    next_id: u64,
    timeout: Duration,
}

#[derive(Debug, Clone)]
struct DevToolsTarget {
    title: String,
    url: String,
    target_type: String,
    websocket_url: String,
}

const STATE_EXPRESSION: &str = r#"
(() => {
  const compactArtist = (artist) => artist ? ({
    id: artist.id ?? null,
    name: artist.name ?? null
  }) : null;

  const firstText = (...values) => {
    for (const value of values) {
      if (typeof value === "string" && value.trim()) return value.trim();
    }
    return null;
  };

  const compactAlbum = (album) => album ? ({
    id: album.id ?? null,
    name: album.name ?? null,
    coverUrl: firstText(album.picUrl, album.cover, album.blurPicUrl, album.override && album.override.imageUrl)
  }) : null;

  const compactTrack = (track) => {
    if (!track) return null;
    return {
      id: track.id ?? null,
      name: track.name ?? null,
      artists: Array.isArray(track.artists)
        ? track.artists.map(compactArtist).filter(Boolean)
        : [],
      album: compactAlbum(track.album)
    };
  };

  const getWebpackRequire = () => {
    if (window.__NCM_WEBPACK_REQUIRE) return window.__NCM_WEBPACK_REQUIRE;
    let webpackRequire;
    const moduleId = 900000 + Math.floor(Math.random() * 100000);
    window.webpackJsonp.push([[moduleId], {
      [moduleId]: function(module, exports, require) {
        webpackRequire = require;
      }
    }, [[moduleId]]]);
    window.__NCM_WEBPACK_REQUIRE = webpackRequire;
    return webpackRequire;
  };

  const isProgressSnapshot = (value) => value
    && typeof value === "object"
    && typeof value.current === "number"
    && ("cacheProgress" in value || "playId" in value);

  const isDvaInstance = (value) => {
    if (!value
      || typeof value !== "object"
      || typeof value.getStore !== "function"
      || typeof value.getDispatch !== "function") {
      return false;
    }
    try {
      const store = value.getStore();
      return store && typeof store === "object";
    } catch (_) {
      return false;
    }
  };

  const resolveDva = (webpackRequire) => {
    const cached = window.__NCM_DVA;
    if (isDvaInstance(cached)) return cached;
    window.__NCM_DVA = null;

    // The application store is initialized before this detector runs, so scan
    // only loaded modules. Requiring arbitrary modules during discovery can
    // execute unrelated application code and cause side effects.
    for (const module of Object.values(webpackRequire.c || {})) {
      let moduleExports;
      try {
        moduleExports = module && module.exports;
      } catch (_) {
        continue;
      }

      const candidates = [moduleExports];
      if (moduleExports && (typeof moduleExports === "object" || typeof moduleExports === "function")) {
        try {
          candidates.push(...Object.values(moduleExports));
        } catch (_) {
          // Some module export objects contain getters that may throw.
        }
      }

      for (const candidate of candidates) {
        if (isDvaInstance(candidate)) {
          window.__NCM_DVA = candidate;
          return candidate;
        }
      }
    }
    return null;
  };

  const resolveProgressAccessor = (webpackRequire) => {
    const cached = window.__NCM_PROGRESS_ACCESSOR;
    if (typeof cached === "function") {
      try {
        if (isProgressSnapshot(cached())) return cached;
      } catch (_) {
        window.__NCM_PROGRESS_ACCESSOR = null;
      }
    }

    // Webpack module IDs change between NetEase Cloud Music releases. Locate
    // the module by the stable player subscription and snapshot fields instead.
    const candidateIds = Object.entries(webpackRequire.m || {})
      .filter(([, factory]) => {
        const source = String(factory);
        return source.includes("playprogress")
          && source.includes("cacheProgress")
          && source.includes("subscribePlayStatus");
      })
      .map(([id]) => id);

    for (const id of candidateIds) {
      let moduleExports;
      try {
        moduleExports = webpackRequire(id);
      } catch (_) {
        continue;
      }
      for (const value of Object.values(moduleExports || {})) {
        if (typeof value !== "function" || value.length !== 0) continue;
        try {
          if (isProgressSnapshot(value())) {
            window.__NCM_PROGRESS_ACCESSOR = value;
            return value;
          }
        } catch (_) {
          // This export is not the progress snapshot accessor.
        }
      }
    }
    return null;
  };

  const stateNames = {
    [-1]: "End",
    0: "Stop",
    1: "Pause",
    2: "Playing"
  };

  let dva = window.__NCM_DVA;
  let progressAccessor = window.__NCM_PROGRESS_ACCESSOR;
  let injectError = null;
  try {
    const webpackRequire = getWebpackRequire();
    dva = resolveDva(webpackRequire);
    progressAccessor = resolveProgressAccessor(webpackRequire);
  } catch (error) {
    injectError = String(error && (error.stack || error.message) || error);
  }

  const store = dva && dva.getStore ? dva.getStore() : null;
  const playing = store && store.playing;
  const playingList = store && store.playingList;
  const currentList = playingList && playingList.curPlayingList;
  const currentTrackId = playing && playing.curTrack && playing.curTrack.id;
  const currentItem = Array.isArray(currentList)
    ? currentList.find((item) => String(item.resourceId ?? item.id ?? (item.track && item.track.id)) === String(currentTrackId))
    : null;

  let progress = null;
  try {
    progress = progressAccessor ? progressAccessor() : null;
  } catch (error) {
    progress = { error: String(error && (error.stack || error.message) || error) };
  }

  const currentSeconds = progress && typeof progress.current === "number" ? progress.current : null;
  const durationSeconds = playing && typeof playing.resourceDuration === "number"
    ? playing.resourceDuration
    : null;

  return {
    source: "debugger",
    href: location.href,
    documentTitle: document.title,
    injected: Boolean(dva && progressAccessor),
    injectError,
    playing: playing ? {
      state: playing.playingState,
      stateText: stateNames[playing.playingState] || String(playing.playingState),
      mode: playing.playingMode,
      playId: playing.playId,
      resourceType: playing.resourceType,
      resourceTrackId: playing.resourceTrackId,
      onlineResourceId: playing.onlineResourceId,
      title: playing.resourceName,
      artists: Array.isArray(playing.resourceArtists)
        ? playing.resourceArtists.map(compactArtist).filter(Boolean)
        : [],
      durationSeconds,
      durationMs: durationSeconds == null ? null : Math.round(durationSeconds * 1000),
      progressSeconds: currentSeconds,
      progressMs: currentSeconds == null ? null : Math.round(currentSeconds * 1000),
      coverUrl: firstText(
        playing.resourceCoverUrl,
        playing.curTrack && playing.curTrack.coverUrl,
        currentItem && currentItem.track && currentItem.track.coverUrl,
      ),
      curTrack: compactTrack(playing.curTrack),
    } : null,
    playlist: currentItem && currentItem.fromInfo && currentItem.fromInfo.sourceData
      ? currentItem.fromInfo.sourceData
      : null,
    playingList: {
      count: Array.isArray(currentList) ? currentList.length : null,
      currentItem: currentItem ? {
        id: currentItem.id ?? null,
        resourceId: currentItem.resourceId ?? null,
        resourceType: currentItem.resourceType ?? null,
        scene: currentItem.scene ?? null,
        href: currentItem.href ?? null,
        text: currentItem.text ?? null,
        fromInfo: currentItem.fromInfo ?? null,
        track: compactTrack(currentItem.track),
      } : null
    }
  };
})()
"#;

impl CdpDetector {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            host: DEFAULT_HOST.to_string(),
            port: DEFAULT_PORT,
            timeout: DEFAULT_TIMEOUT,
            websocket: None,
            next_reconnect_at: Instant::now(),
            last_connect_error: None,
            last_poll_error: None,
        }
    }

    pub async fn poll(&mut self) -> CdpPlayingState {
        let timeout_duration = self.timeout;
        let Some(websocket) = self.ensure_connected().await else {
            return self.none_state();
        };

        match timeout(timeout_duration, websocket.evaluate_state()).await {
            Ok(Ok(state)) => {
                self.last_poll_error = None;
                self.map_state(state)
            }
            Ok(Err(error)) => {
                self.disconnect();
                self.next_reconnect_at = Instant::now() + RECONNECT_INTERVAL;
                self.log_poll_error(error.to_string());
                self.none_state()
            }
            Err(_) => {
                self.disconnect();
                self.next_reconnect_at = Instant::now() + RECONNECT_INTERVAL;
                self.log_poll_error("devtools evaluate timed out".to_string());
                self.none_state()
            }
        }
    }

    fn disconnect(&mut self) {
        if let Some(websocket) = self.websocket.as_mut() {
            websocket.close();
        }
        self.websocket = None;
    }

    async fn ensure_connected(&mut self) -> Option<&mut DevToolsClient> {
        if self.websocket.is_some() {
            return self.websocket.as_mut();
        }

        let now = Instant::now();
        if now < self.next_reconnect_at {
            return None;
        }

        let target = match self
            .list_targets()
            .await
            .and_then(|targets| choose_target(&targets))
        {
            Ok(target) => target,
            Err(error) => {
                self.next_reconnect_at = now + RECONNECT_INTERVAL;
                self.log_connect_error(error.to_string());
                return None;
            }
        };

        match DevToolsClient::connect(&target.websocket_url, self.timeout).await {
            Ok(client) => {
                debug!(
                    host = %self.host,
                    port = self.port,
                    url = %target.url,
                    title = %target.title,
                    target_type = %target.target_type,
                    "connected to devtools"
                );
                self.websocket = Some(client);
                self.last_connect_error = None;
                self.last_poll_error = None;
                self.websocket.as_mut()
            }
            Err(error) => {
                self.next_reconnect_at = now + RECONNECT_INTERVAL;
                self.log_connect_error(error.to_string());
                None
            }
        }
    }

    async fn list_targets(&self) -> AppResult<Vec<DevToolsTarget>> {
        let url = format!("http://{}:{}/json/list", self.host, self.port);
        let raw_targets = self
            .client
            .get(url)
            .send()
            .await?
            .json::<Vec<Value>>()
            .await?;

        let mut targets = Vec::new();
        for raw in raw_targets {
            let Some(websocket_url) = raw.get("webSocketDebuggerUrl").and_then(Value::as_str)
            else {
                continue;
            };
            targets.push(DevToolsTarget {
                title: raw
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                url: raw
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                target_type: raw
                    .get("type")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                websocket_url: websocket_url.to_string(),
            });
        }
        Ok(targets)
    }

    fn map_state(&self, raw: Value) -> CdpPlayingState {
        let Some(playing) = raw.get("playing").and_then(Value::as_object) else {
            return self.none_state();
        };

        let playing_state = int_value(playing.get("state")).unwrap_or_default();
        let cur_track = playing.get("curTrack").and_then(Value::as_object);
        let artists = cur_track
            .and_then(|item| item.get("artists"))
            .or_else(|| playing.get("artists"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_object())
                    .filter_map(|item| item.get("name").and_then(Value::as_str))
                    .map(|item| item.trim().to_string())
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();

        let album = cur_track
            .and_then(|item| item.get("album"))
            .and_then(Value::as_object)
            .and_then(|item| item.get("name"))
            .and_then(Value::as_str)
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty());

        let cover_url = first_text([
            playing.get("coverUrl"),
            playing.get("resourceCoverUrl"),
            cur_track.and_then(|item| item.get("coverUrl")),
            cur_track
                .and_then(|item| item.get("album"))
                .and_then(Value::as_object)
                .and_then(|item| item.get("coverUrl")),
        ]);

        let title = first_text([
            cur_track.and_then(|item| item.get("name")),
            playing.get("title"),
            playing.get("resourceName"),
        ])
        .unwrap_or_default();

        let track_id = cur_track
            .and_then(|item| item.get("id"))
            .and_then(Value::as_i64)
            .or_else(|| int_value(playing.get("resourceTrackId")));

        let duration_ms = int_value(playing.get("durationMs"));
        let progress_ms = int_value(playing.get("progressMs"));
        let status = if matches!(playing_state, 1 | 2) {
            PlaybackStatus::Active
        } else {
            PlaybackStatus::None
        };

        CdpPlayingState {
            status,
            track_id: track_id.map(|item| item.to_string()),
            title,
            artists,
            album,
            cover_url,
            duration_ms,
            progress_ms,
            playing_state,
        }
    }

    fn none_state(&self) -> CdpPlayingState {
        CdpPlayingState {
            status: PlaybackStatus::None,
            track_id: None,
            title: String::new(),
            artists: Vec::new(),
            album: None,
            cover_url: None,
            duration_ms: None,
            progress_ms: None,
            playing_state: 0,
        }
    }

    fn log_connect_error(&mut self, message: String) {
        if self.last_connect_error.as_deref() == Some(message.as_str()) {
            return;
        }
        self.last_connect_error = Some(message.clone());
        warn!(%message, "failed to connect music detector");
    }

    fn log_poll_error(&mut self, message: String) {
        if self.last_poll_error.as_deref() == Some(message.as_str()) {
            return;
        }
        self.last_poll_error = Some(message.clone());
        warn!(%message, "music detector poll failed");
    }
}

impl DevToolsClient {
    async fn connect(url: &str, timeout: Duration) -> AppResult<Self> {
        let parsed = Url::parse(url)?;
        if parsed.scheme() != "ws" {
            return Err(AppError::message(format!(
                "unsupported devtools websocket url: {url}"
            )));
        }

        let (websocket, _) = connect_async(url)
            .await
            .map_err(|error| AppError::message(error.to_string()))?;
        let _ = timeout;

        Ok(Self {
            websocket,
            next_id: 1,
            timeout,
        })
    }

    async fn evaluate_state(&mut self) -> AppResult<Value> {
        self.call(
            "Runtime.evaluate",
            json!({
                "expression": STATE_EXPRESSION,
                "returnByValue": true,
                "awaitPromise": true,
            }),
        )
        .await
    }

    async fn call(&mut self, method: &str, params: Value) -> AppResult<Value> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        self.websocket
            .send(Message::Text(serde_json::to_string(&request)?.into()))
            .await
            .map_err(|error| AppError::message(error.to_string()))?;

        loop {
            let message = timeout(self.timeout, self.websocket.next())
                .await
                .map_err(|_| AppError::message("devtools websocket timed out"))?;
            let Some(message) = message else {
                return Err(AppError::message("devtools websocket ended"));
            };
            match message.map_err(|error| AppError::message(error.to_string()))? {
                Message::Text(text) => {
                    let response: Value = serde_json::from_str(&text)?;
                    if response.get("id").and_then(Value::as_u64) != Some(id) {
                        continue;
                    }
                    if let Some(error) = response.get("error") {
                        return Err(AppError::message(error.to_string()));
                    }
                    let Some(result) = response
                        .get("result")
                        .and_then(Value::as_object)
                        .and_then(|item| item.get("result"))
                        .and_then(Value::as_object)
                        .and_then(|item| item.get("value"))
                        .cloned()
                    else {
                        return Err(AppError::message("devtools returned an empty response"));
                    };
                    return Ok(result);
                }
                Message::Close(_) => return Err(AppError::message("devtools websocket closed")),
                Message::Ping(payload) => {
                    self.websocket
                        .send(Message::Pong(payload))
                        .await
                        .map_err(|error| AppError::message(error.to_string()))?;
                }
                Message::Pong(_) | Message::Binary(_) | Message::Frame(_) => {}
            }
        }
    }

    fn close(&mut self) {
        let _ = self.websocket.close(None);
    }
}

fn choose_target(targets: &[DevToolsTarget]) -> AppResult<DevToolsTarget> {
    let preferred = targets
        .iter()
        .find(|target| target.target_type == "page" && target.url.contains("pub/app.html"))
        .or_else(|| targets.iter().find(|target| target.target_type == "page"))
        .or_else(|| targets.first())
        .ok_or_else(|| AppError::message("no available devtools target"))?;

    Ok(preferred.clone())
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

fn int_value(value: Option<&Value>) -> Option<i64> {
    let value = value?;
    if let Some(number) = value.as_i64() {
        return Some(number);
    }
    if let Some(number) = value.as_f64() {
        return Some(number.round() as i64);
    }
    if let Some(text) = value.as_str() {
        let text = text.trim();
        if text.is_empty() {
            return None;
        }
        return text.parse::<i64>().ok();
    }
    None
}
