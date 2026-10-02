use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{Manager, WebviewWindow};
use tracing::warn;

use crate::{
    castboard_plugins,
    protocol::SystemControlAction,
    runtime::AppRuntime,
};

pub const WINDOW_LABEL: &str = "castboard";

pub const INITIALIZATION_SCRIPT: &str = r#"
(() => {
  const listeners = new Set();

  function send(event) {
    return window.__TAURI_INTERNALS__.invoke("castboard_event", { event });
  }

  function _dispatch(message) {
    for (const listener of listeners) {
      listener(message);
    }
  }

  function subscribe(listener) {
    listeners.add(listener);
    return () => listeners.delete(listener);
  }

  window.castboardIPC = { send, subscribe, _dispatch };
})();
"#;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CastBoardEvent {
    channel: String,
    id: Option<String>,
    #[serde(rename = "type")]
    event_type: String,
    payload: Value,
}

#[derive(Debug, PartialEq, Eq)]
enum CastBoardAction {
    Close,
    OpenDevTools,
    Ready,
    MusicAlive,
    SysInfoAlive,
    MediaControl(SystemControlAction),
}

impl CastBoardEvent {
    fn validate(&self) -> Result<(), String> {
        if self.channel != "castboard" {
            return Err("invalid CastBoard event channel".to_string());
        }
        if self.event_type.trim().is_empty() {
            return Err("CastBoard event type must not be empty".to_string());
        }
        Ok(())
    }

    fn action(&self) -> Result<CastBoardAction, String> {
        match self.event_type.as_str() {
            "app.close" => Ok(CastBoardAction::Close),
            "app.openDevTools" => Ok(CastBoardAction::OpenDevTools),
            "page.ready" => Ok(CastBoardAction::Ready),
            "music.alive" => Ok(CastBoardAction::MusicAlive),
            "sysinfo.alive" => Ok(CastBoardAction::SysInfoAlive),
            "media.control" => self.media_control_action(),
            _ => Err(format!(
                "unknown CastBoard event type: {}",
                self.event_type
            )),
        }
    }

    fn media_control_action(&self) -> Result<CastBoardAction, String> {
        let action = self
            .payload
            .get("action")
            .and_then(Value::as_str)
            .ok_or_else(|| "CastBoard media control action must be a string".to_string())?;
        let media_action = match action {
            "play" => SystemControlAction::Play,
            "pause" => SystemControlAction::Pause,
            "next" => SystemControlAction::Next,
            "previous" => SystemControlAction::Previous,
            _ => return Err(format!("unsupported CastBoard media control action: {action}")),
        };
        Ok(CastBoardAction::MediaControl(media_action))
    }
}

pub fn handle_event(
    window: &WebviewWindow,
    runtime: &AppRuntime,
    event: CastBoardEvent,
) -> Result<Value, String> {
    if window.label() != WINDOW_LABEL {
        return Err("CastBoard events are only accepted from the CastBoard window".to_string());
    }

    event.validate()?;
    match event.action()? {
        CastBoardAction::Close => {
            window.close().map_err(|error| error.to_string())?;
        }
        CastBoardAction::OpenDevTools => {
            #[cfg(debug_assertions)]
            window.open_devtools();
        }
        CastBoardAction::Ready => {
            let id = event
                .id
                .as_deref()
                .filter(|id| !id.trim().is_empty())
                .ok_or_else(|| "CastBoard page.ready event requires an id".to_string())?;
            runtime.handle_local_ready(window.label());
            dispatch_host_ready(window, id)?;
            match castboard_plugins::registrations(window.app_handle()) {
                Ok(plugins) if !plugins.is_empty() => dispatch_plugins(window, plugins)?,
                Ok(_) => {}
                Err(error) => warn!(%error, "failed to load CastBoard plugins"),
            }
        }
        CastBoardAction::MusicAlive => {
            runtime.handle_local_music_alive(WINDOW_LABEL);
        }
        CastBoardAction::SysInfoAlive => {
            runtime.handle_local_sysinfo_alive(WINDOW_LABEL);
        }
        CastBoardAction::MediaControl(action) => {
            runtime.execute_local_media_control(action)?;
        }
    }
    Ok(Value::Null)
}

pub fn dispatch_host_ready(window: &WebviewWindow, id: &str) -> Result<(), String> {
    dispatch_event(window, host_ready_event(id))
}

fn dispatch_plugins(window: &WebviewWindow, plugins: Vec<Value>) -> Result<(), String> {
    dispatch_event(
        window,
        serde_json::json!({
            "channel": "castboard",
            "type": "plugins.register",
            "payload": { "plugins": plugins },
        }),
    )
}

pub fn dispatch_protocol_event<T>(
    window: &WebviewWindow,
    message_type: &str,
    payload: &T,
) -> Result<(), String>
where
    T: Serialize,
{
    let event_type = castboard_event_type(message_type)?;
    let message = serde_json::json!({
        "channel": "castboard",
        "type": event_type,
        "payload": payload,
    });
    dispatch_event(window, message)
}

fn castboard_event_type(message_type: &str) -> Result<&'static str, String> {
    match message_type {
        "music.v1.track" => Ok("music.track"),
        "music.v1.lyric" => Ok("music.lyric"),
        "music.v1.progress" => Ok("music.progress"),
        "sysinfo.v1.stats" => Ok("sysinfo.stats"),
        _ => Err(format!("unsupported CastBoard protocol event type: {message_type}")),
    }
}

fn dispatch_event(window: &WebviewWindow, message: Value) -> Result<(), String> {
    let message_json = serde_json::to_string(&message).map_err(|error| error.to_string())?;
    window
        .eval(format!("window.castboardIPC?._dispatch({message_json});"))
        .map_err(|error| error.to_string())
}

fn host_ready_event(id: &str) -> Value {
    serde_json::json!({
        "channel": "castboard",
        "type": "host.ready",
        "id": id,
        "payload": { "ok": true },
    })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use crate::protocol::SystemControlAction;

    use super::{castboard_event_type, host_ready_event, CastBoardAction, CastBoardEvent};

    fn event(event_type: &str) -> CastBoardEvent {
        CastBoardEvent {
            channel: "castboard".to_string(),
            id: None,
            event_type: event_type.to_string(),
            payload: json!({}),
        }
    }

    fn media_control_event(action: &str) -> CastBoardEvent {
        let mut event = event("media.control");
        event.payload = json!({ "action": action });
        event
    }

    #[test]
    fn accepts_the_supported_actions() {
        assert_eq!(event("app.close").action(), Ok(CastBoardAction::Close));
        assert_eq!(
            event("app.openDevTools").action(),
            Ok(CastBoardAction::OpenDevTools)
        );
        assert_eq!(event("page.ready").action(), Ok(CastBoardAction::Ready));
        assert_eq!(event("music.alive").action(), Ok(CastBoardAction::MusicAlive));
        assert_eq!(
            event("sysinfo.alive").action(),
            Ok(CastBoardAction::SysInfoAlive)
        );
        assert_eq!(
            media_control_event("play").action(),
            Ok(CastBoardAction::MediaControl(SystemControlAction::Play))
        );
        assert_eq!(
            media_control_event("pause").action(),
            Ok(CastBoardAction::MediaControl(SystemControlAction::Pause))
        );
        assert_eq!(
            media_control_event("next").action(),
            Ok(CastBoardAction::MediaControl(SystemControlAction::Next))
        );
        assert_eq!(
            media_control_event("previous").action(),
            Ok(CastBoardAction::MediaControl(SystemControlAction::Previous))
        );
    }

    #[test]
    fn serializes_the_host_ready_event() {
        assert_eq!(
            host_ready_event("7"),
            json!({
                "channel": "castboard",
                "type": "host.ready",
                "id": "7",
                "payload": { "ok": true },
            }),
        );
    }

    #[test]
    fn deserializes_the_protocol_envelope() {
        let event: CastBoardEvent = serde_json::from_value(json!({
            "channel": "castboard",
            "id": "7",
            "type": "page.ready",
            "payload": {},
        }))
        .expect("valid CastBoard event");

        assert_eq!(event.validate(), Ok(()));
        assert_eq!(event.action(), Ok(CastBoardAction::Ready));
    }

    #[test]
    fn rejects_invalid_envelopes() {
        let mut invalid = event("page.ready");
        invalid.channel = "other".to_string();
        assert_eq!(
            invalid.validate(),
            Err("invalid CastBoard event channel".to_string())
        );

        let mut invalid = event("page.ready");
        invalid.event_type = " ".to_string();
        assert_eq!(
            invalid.validate(),
            Err("CastBoard event type must not be empty".to_string())
        );
    }

    #[test]
    fn rejects_unknown_actions() {
        assert_eq!(
            event("castboard.unknown").action(),
            Err("unknown CastBoard event type: castboard.unknown".to_string())
        );
    }

    #[test]
    fn rejects_non_media_control_actions() {
        assert_eq!(
            media_control_event("shutdown").action(),
            Err("unsupported CastBoard media control action: shutdown".to_string())
        );
        assert_eq!(
            event("media.control").action(),
            Err("CastBoard media control action must be a string".to_string())
        );
    }

    #[test]
    fn maps_protocol_types_to_flat_event_types() {
        assert_eq!(castboard_event_type("music.v1.track"), Ok("music.track"));
        assert_eq!(castboard_event_type("music.v1.lyric"), Ok("music.lyric"));
        assert_eq!(castboard_event_type("music.v1.progress"), Ok("music.progress"));
        assert_eq!(castboard_event_type("sysinfo.v1.stats"), Ok("sysinfo.stats"));
        assert!(castboard_event_type("unknown").is_err());
    }
}
