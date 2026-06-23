use std::time::Duration;

use serde::Serialize;
use tauri::{
    AppHandle, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};
use tracing::{info, warn};
use url::Url;

use crate::state::AppState;

const CASTBOARD_WINDOW_LABEL: &str = "castboard";
const DEFAULT_CASTBOARD_DEV_URL: &str = "http://127.0.0.1:5173/index.html?debug=1";

#[derive(Serialize)]
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
pub async fn open_castboard_on_monitor(
    app: AppHandle,
    state: State<'_, AppState>,
    monitor_id: String,
) -> Result<(), String> {
    info!(%monitor_id, "castboard open requested");
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
    let monitor_name = monitor
        .name()
        .map(String::as_str)
        .unwrap_or("unknown");
    info!(
        index,
        name = monitor_name,
        x = position.x,
        y = position.y,
        width = size.width,
        height = size.height,
        scale_factor = monitor.scale_factor(),
        "castboard monitor selected"
    );

    if let Some(window) = app.get_webview_window(CASTBOARD_WINDOW_LABEL) {
        info!("castboard window already exists; moving existing window");
        place_castboard_window(&window, position, size)?;
        let _ = window.set_focus();
        if let Err(error) = window.eval("window.location.reload();") {
            warn!(%error, "castboard window reload failed");
        }
        start_local_castboard_session(&state);
        return Ok(());
    }

    let (url, url_label) = castboard_url()?;
    info!(%url_label, "castboard window build starting");
    let window = WebviewWindowBuilder::new(&app, CASTBOARD_WINDOW_LABEL, url)
        .title("CastBoard")
        .decorations(false)
        .resizable(false)
        .inner_size(size.width as f64, size.height as f64)
        .position(position.x as f64, position.y as f64)
        .build()
        .map_err(|error| {
            warn!(%error, "castboard window build failed");
            error.to_string()
        })?;
    info!("castboard window built");
    place_castboard_window(&window, position, size)?;
    let _ = window.set_focus();
    info!("castboard window focused");

    start_local_castboard_session(&state);
    Ok(())
}

fn place_castboard_window(
    window: &WebviewWindow,
    position: PhysicalPosition<i32>,
    size: PhysicalSize<u32>,
) -> Result<(), String> {
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
