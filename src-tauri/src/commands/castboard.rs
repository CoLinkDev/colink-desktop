use std::{
    sync::{Mutex, OnceLock},
    time::Duration,
};

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder, WindowEvent,
};
use tracing::{info, warn};
use url::Url;

use crate::state::AppState;

const CASTBOARD_WINDOW_LABEL: &str = "castboard";
const CASTBOARD_STATUS_EVENT: &str = "castboard-status";
const DEFAULT_CASTBOARD_DEV_URL: &str = "http://127.0.0.1:5173/index.html?debug=1";

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CastBoardMonitor {
    id: String,
    name: String,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
    scale_factor: f64,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CastBoardStatus {
    state: String,
    monitor: Option<CastBoardMonitor>,
    message: Option<String>,
}

static CASTBOARD_STATUS: OnceLock<Mutex<CastBoardStatus>> = OnceLock::new();

#[tauri::command]
pub fn list_castboard_monitors(app: AppHandle) -> Result<Vec<CastBoardMonitor>, String> {
    let monitors = app.available_monitors().map_err(|error| error.to_string())?;
    Ok(monitors
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| {
            let position = monitor.position();
            let size = monitor.size();
            CastBoardMonitor {
                id: index.to_string(),
                name: monitor
                    .name()
                    .cloned()
                    .unwrap_or_else(|| format!("Display {}", index + 1)),
                x: position.x,
                y: position.y,
                width: size.width,
                height: size.height,
                scale_factor: monitor.scale_factor(),
            }
        })
        .collect())
}

#[tauri::command]
pub fn get_castboard_status(app: AppHandle) -> CastBoardStatus {
    let status = castboard_status().lock().expect("castboard status poisoned").clone();
    if app.get_webview_window(CASTBOARD_WINDOW_LABEL).is_none()
        && matches!(status.state.as_str(), "opening" | "open" | "closing")
    {
        return set_castboard_status(&app, "closed", None, None);
    }
    status
}

#[tauri::command]
pub fn stop_castboard(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    info!("castboard stop requested");
    let Some(window) = app.get_webview_window(CASTBOARD_WINDOW_LABEL) else {
        state.runtime.end_local_castboard(CASTBOARD_WINDOW_LABEL);
        set_castboard_status(&app, "closed", None, None);
        return Ok(());
    };

    let current = castboard_status().lock().expect("castboard status poisoned").clone();
    set_castboard_status(&app, "closing", current.monitor, None);
    state.runtime.end_local_castboard(CASTBOARD_WINDOW_LABEL);
    window.close().map_err(|error| {
        let message = error.to_string();
        set_castboard_status(&app, "failed", None, Some(message.clone()));
        message
    })
}

#[tauri::command]
pub async fn open_castboard_on_monitor(
    app: AppHandle,
    state: State<'_, AppState>,
    monitor_id: String,
) -> Result<(), String> {
    info!(%monitor_id, "castboard open requested");
    let (monitor, position, size, index) = resolve_monitor(&app, &monitor_id).map_err(|error| {
        set_castboard_status(&app, "failed", None, Some(error.clone()));
        error
    })?;
    set_castboard_status(&app, "opening", Some(monitor.clone()), None);
    info!(
        index,
        name = monitor.name.as_str(),
        x = position.x,
        y = position.y,
        width = size.width,
        height = size.height,
        scale_factor = monitor.scale_factor,
        "castboard monitor selected"
    );

    if let Some(window) = app.get_webview_window(CASTBOARD_WINDOW_LABEL) {
        info!("castboard window already exists; moving existing window");
        place_castboard_window(&window, position, size).map_err(|error| {
            set_castboard_status(&app, "failed", Some(monitor.clone()), Some(error.clone()));
            error
        })?;
        let _ = window.set_focus();
        if let Err(error) = window.eval("window.location.reload();") {
            warn!(%error, "castboard window reload failed");
        }
        start_local_castboard_session(&state);
        set_castboard_status(&app, "open", Some(monitor), None);
        return Ok(());
    }

    let (url, url_label) = castboard_url().map_err(|error| {
        set_castboard_status(&app, "failed", Some(monitor.clone()), Some(error.clone()));
        error
    })?;
    info!(%url_label, "castboard window build starting");
    let window = WebviewWindowBuilder::new(&app, CASTBOARD_WINDOW_LABEL, url)
        .title("CastBoard")
        .decorations(false)
        .devtools(cfg!(debug_assertions))
        .resizable(false)
        .inner_size(size.width as f64, size.height as f64)
        .position(position.x as f64, position.y as f64)
        .build()
        .map_err(|error| {
            warn!(%error, "castboard window build failed");
            let message = error.to_string();
            set_castboard_status(&app, "failed", Some(monitor.clone()), Some(message.clone()));
            message
        })?;
    info!("castboard window built");
    place_castboard_window(&window, position, size).map_err(|error| {
        set_castboard_status(&app, "failed", Some(monitor.clone()), Some(error.clone()));
        error
    })?;
    #[cfg(debug_assertions)]
    window.open_devtools();
    let _ = window.set_focus();
    info!("castboard window focused");

    start_local_castboard_session(&state);
    set_castboard_status(&app, "open", Some(monitor), None);
    Ok(())
}

pub fn handle_castboard_window_event(
    app: &AppHandle,
    state: &State<'_, AppState>,
    event: &WindowEvent,
) {
    if let WindowEvent::Destroyed = event {
        info!("castboard window destroyed");
        state.runtime.end_local_castboard(CASTBOARD_WINDOW_LABEL);
        set_castboard_status(app, "closed", None, None);
    }
}

fn place_castboard_window(
    window: &WebviewWindow,
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
) -> Result<(), String> {
    window.set_fullscreen(false).map_err(|error| error.to_string())?;
    window
        .set_position(PhysicalPosition::new(position.x, position.y))
        .map_err(|error| error.to_string())?;
    window
        .set_size(PhysicalSize::new(size.width, size.height))
        .map_err(|error| error.to_string())?;
    info!(
        x = position.x,
        y = position.y,
        width = size.width,
        height = size.height,
        "castboard window placed"
    );
    window.set_fullscreen(true).map_err(|error| error.to_string())?;
    Ok(())
}

fn start_local_castboard_session(state: &State<'_, AppState>) {
    state.runtime.begin_local_castboard(CASTBOARD_WINDOW_LABEL);
    let runtime = state.runtime.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(700)).await;
        runtime.begin_local_castboard(CASTBOARD_WINDOW_LABEL);
    });
    info!("castboard local runtime session started");
}

fn resolve_monitor(
    app: &AppHandle,
    monitor_id: &str,
) -> Result<(CastBoardMonitor, PhysicalPosition<i32>, PhysicalSize<u32>, usize), String> {
    let index = monitor_id
        .trim()
        .parse::<usize>()
        .map_err(|_| "invalid monitor id".to_string())?;
    let monitors = app.available_monitors().map_err(|error| error.to_string())?;
    let monitor = monitors
        .get(index)
        .ok_or_else(|| "monitor not found".to_string())?;
    let position = *monitor.position();
    let size = *monitor.size();
    Ok((
        CastBoardMonitor {
            id: index.to_string(),
            name: monitor
                .name()
                .cloned()
                .unwrap_or_else(|| format!("Display {}", index + 1)),
            x: position.x,
            y: position.y,
            width: size.width,
            height: size.height,
            scale_factor: monitor.scale_factor(),
        },
        position,
        size,
        index,
    ))
}

fn castboard_status() -> &'static Mutex<CastBoardStatus> {
    CASTBOARD_STATUS.get_or_init(|| {
        Mutex::new(CastBoardStatus {
            state: "closed".to_string(),
            monitor: None,
            message: None,
        })
    })
}

fn set_castboard_status(
    app: &AppHandle,
    state: &str,
    monitor: Option<CastBoardMonitor>,
    message: Option<String>,
) -> CastBoardStatus {
    let next = CastBoardStatus {
        state: state.to_string(),
        monitor,
        message,
    };
    *castboard_status()
        .lock()
        .expect("castboard status poisoned") = next.clone();
    let _ = app.emit(CASTBOARD_STATUS_EVENT, next.clone());
    next
}

fn castboard_url() -> Result<(WebviewUrl, String), String> {
    let dev_url = std::env::var("COLINK_CASTBOARD_DEV_URL")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !dev_url.is_empty() {
        return Url::parse(&dev_url)
            .map(WebviewUrl::External)
            .map(|url| (url, dev_url))
            .map_err(|error| error.to_string());
    }

    if cfg!(debug_assertions) {
        return Url::parse(DEFAULT_CASTBOARD_DEV_URL)
            .map(WebviewUrl::External)
            .map(|url| (url, DEFAULT_CASTBOARD_DEV_URL.to_string()))
            .map_err(|error| error.to_string());
    }

    Ok((
        WebviewUrl::App("castboard/index.html".into()),
        "app://castboard/index.html".to_string(),
    ))
}
