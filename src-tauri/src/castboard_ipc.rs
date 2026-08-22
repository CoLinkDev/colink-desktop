use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::WebviewWindow;

use crate::runtime::AppRuntime;

pub const WINDOW_LABEL: &str = "castboard";

pub const INITIALIZATION_SCRIPT: &str = r#"
(() => {
  let nextId = 1;
  const listeners = new Set();

  function request({ type, payload }) {
    const request = {
      channel: "castboard",
      kind: "request",
      id: String(nextId++),
      type,
      payload: payload || {},
    };
    return window.__TAURI_INTERNALS__.invoke("castboard_request", { request });
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

  window.castboardIPC = { request, subscribe, _dispatch };
})();
"#;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CastBoardRequest {
    channel: String,
    kind: String,
    id: String,
    #[serde(rename = "type")]
    request_type: String,
    payload: Value,
}

#[derive(Debug, PartialEq, Eq)]
enum CastBoardAction {
    Close,
    OpenDevTools,
    Ready,
}

impl CastBoardRequest {
    fn validate(&self) -> Result<(), String> {
        if self.channel != "castboard" {
            return Err("invalid CastBoard request channel".to_string());
        }
        if self.kind != "request" {
            return Err("invalid CastBoard message kind".to_string());
        }
        if self.id.trim().is_empty() {
            return Err("CastBoard request id must not be empty".to_string());
        }
        if !self.payload.is_object() {
            return Err("CastBoard request payload must be an object".to_string());
        }
        Ok(())
    }

    fn action(&self) -> Result<CastBoardAction, String> {
        match self.request_type.as_str() {
            "castboard.close" => Ok(CastBoardAction::Close),
            "castboard.openDevTools" => Ok(CastBoardAction::OpenDevTools),
            "castboard.ready" => Ok(CastBoardAction::Ready),
            _ => Err(format!(
                "unknown CastBoard request type: {}",
                self.request_type
            )),
        }
    }
}

pub fn handle_request(
    window: &WebviewWindow,
    runtime: &AppRuntime,
    request: CastBoardRequest,
) -> Result<Value, String> {
    if window.label() != WINDOW_LABEL {
        return Err("CastBoard requests are only accepted from the CastBoard window".to_string());
    }

    request.validate()?;
    match request.action()? {
        CastBoardAction::Close => {
            window.close().map_err(|error| error.to_string())?;
        }
        CastBoardAction::OpenDevTools => {
            #[cfg(debug_assertions)]
            window.open_devtools();
        }
        CastBoardAction::Ready => {
            runtime.begin_local_castboard(WINDOW_LABEL);
            dispatch_host_ready(window)?;
        }
    }
    Ok(Value::Null)
}

pub fn dispatch_host_ready(window: &WebviewWindow) -> Result<(), String> {
    dispatch_event(window, host_ready_event())
}

pub fn dispatch_business_event<T>(
    window: &WebviewWindow,
    message_type: &str,
    payload: &T,
) -> Result<(), String>
where
    T: Serialize,
{
    let message = serde_json::json!({
        "channel": "castboard",
        "kind": "event",
        "type": "business",
        "payload": {
            "type": message_type,
            "payload": payload,
        },
    });
    dispatch_event(window, message)
}

fn dispatch_event(window: &WebviewWindow, message: Value) -> Result<(), String> {
    let message_json = serde_json::to_string(&message).map_err(|error| error.to_string())?;
    window
        .eval(format!("window.castboardIPC?._dispatch({message_json});"))
        .map_err(|error| error.to_string())
}

fn host_ready_event() -> Value {
    serde_json::json!({
        "channel": "castboard",
        "kind": "event",
        "type": "host.ready",
        "payload": {},
    })
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::{host_ready_event, CastBoardAction, CastBoardRequest};

    fn request(request_type: &str) -> CastBoardRequest {
        CastBoardRequest {
            channel: "castboard".to_string(),
            kind: "request".to_string(),
            id: "1".to_string(),
            request_type: request_type.to_string(),
            payload: json!({}),
        }
    }

    #[test]
    fn accepts_the_supported_actions() {
        assert_eq!(request("castboard.close").action(), Ok(CastBoardAction::Close));
        assert_eq!(
            request("castboard.openDevTools").action(),
            Ok(CastBoardAction::OpenDevTools)
        );
        assert_eq!(request("castboard.ready").action(), Ok(CastBoardAction::Ready));
    }

    #[test]
    fn serializes_the_host_ready_event() {
        assert_eq!(
            host_ready_event(),
            json!({
                "channel": "castboard",
                "kind": "event",
                "type": "host.ready",
                "payload": {},
            }),
        );
    }

    #[test]
    fn deserializes_the_protocol_envelope() {
        let request: CastBoardRequest = serde_json::from_value(json!({
            "channel": "castboard",
            "kind": "request",
            "id": "7",
            "type": "castboard.ready",
            "payload": {},
        }))
        .expect("valid CastBoard request");

        assert_eq!(request.validate(), Ok(()));
        assert_eq!(request.action(), Ok(CastBoardAction::Ready));
    }

    #[test]
    fn rejects_invalid_envelopes() {
        let mut invalid = request("castboard.ready");
        invalid.channel = "other".to_string();
        assert_eq!(
            invalid.validate(),
            Err("invalid CastBoard request channel".to_string())
        );

        let mut invalid = request("castboard.ready");
        invalid.payload = Value::Null;
        assert_eq!(
            invalid.validate(),
            Err("CastBoard request payload must be an object".to_string())
        );
    }

    #[test]
    fn rejects_unknown_actions() {
        assert_eq!(
            request("castboard.unknown").action(),
            Err("unknown CastBoard request type: castboard.unknown".to_string())
        );
    }
}
