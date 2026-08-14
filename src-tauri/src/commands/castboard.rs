use std::{
    collections::HashMap,
    sync::{Mutex, OnceLock},
    time::Duration,
};

use serde::Serialize;
use tauri::{
    webview::PageLoadEvent, AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, State,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};
use tracing::{info, warn};
use url::Url;

use crate::state::AppState;

#[cfg(windows)]
use windows::{
    core::PCWSTR,
    Win32::{
        Graphics::Gdi::{EnumDisplayDevicesW, DISPLAY_DEVICEW},
        UI::WindowsAndMessaging::EDD_GET_DEVICE_INTERFACE_NAME,
    },
};

#[cfg(windows)]
use winreg::{enums::HKEY_LOCAL_MACHINE, RegKey};

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
    let friendly_names = friendly_monitor_names();
    let monitors = app.available_monitors().map_err(|error| error.to_string())?;
    Ok(monitors
        .into_iter()
        .enumerate()
        .map(|(index, monitor)| {
            let position = monitor.position();
            let size = monitor.size();
            let raw_name = monitor.name().map(String::as_str);
            CastBoardMonitor {
                id: index.to_string(),
                name: castboard_monitor_name(index, raw_name, &friendly_names),
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
    language: String,
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
        let mut url = window.url().map_err(|error| error.to_string())?;
        set_castboard_language(&mut url, &language);
        window.navigate(url).map_err(|error| error.to_string())?;
        start_local_castboard_session(&state);
        set_castboard_status(&app, "open", Some(monitor), None);
        return Ok(());
    }

    let (url, url_label) = castboard_url(&language).map_err(|error| {
        set_castboard_status(&app, "failed", Some(monitor.clone()), Some(error.clone()));
        error
    })?;
    let runtime = state.runtime.clone();
    info!(%url_label, "castboard window build starting");
    let window = WebviewWindowBuilder::new(&app, CASTBOARD_WINDOW_LABEL, url)
        .title("CastBoard")
        .decorations(false)
        .devtools(cfg!(debug_assertions))
        .resizable(false)
        .inner_size(size.width as f64, size.height as f64)
        .position(position.x as f64, position.y as f64)
        .on_page_load(move |_window, payload| {
            if matches!(payload.event(), PageLoadEvent::Finished) {
                runtime.begin_local_castboard(CASTBOARD_WINDOW_LABEL);
            }
        })
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
    let friendly_names = friendly_monitor_names();
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
            name: castboard_monitor_name(index, monitor.name().map(String::as_str), &friendly_names),
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

fn castboard_monitor_name(
    index: usize,
    raw_name: Option<&str>,
    friendly_names: &HashMap<String, String>,
) -> String {
    raw_name
        .and_then(|name| friendly_names.get(&name.to_ascii_uppercase()).cloned())
        .or_else(|| raw_name.map(ToOwned::to_owned))
        .unwrap_or_else(|| format!("Display {}", index + 1))
}

#[cfg(not(windows))]
fn friendly_monitor_names() -> HashMap<String, String> {
    HashMap::new()
}

#[cfg(windows)]
fn friendly_monitor_names() -> HashMap<String, String> {
    let mut names = HashMap::new();
    let mut adapter_index = 0;

    loop {
        let mut adapter = display_device();
        let adapter_found = unsafe {
            EnumDisplayDevicesW(PCWSTR::null(), adapter_index, &mut adapter, 0).as_bool()
        };
        if !adapter_found {
            break;
        }

        let display_name = wide_array_to_string(&adapter.DeviceName);
        if !display_name.is_empty() {
            if let Some(name) = friendly_name_for_display(&display_name) {
                names.insert(display_name.to_ascii_uppercase(), name);
            }
        }

        adapter_index += 1;
    }

    names
}

#[cfg(windows)]
fn friendly_name_for_display(display_name: &str) -> Option<String> {
    let display_name_wide = to_wide(display_name);
    let mut monitor = display_device();
    let found = unsafe {
        EnumDisplayDevicesW(
            PCWSTR(display_name_wide.as_ptr()),
            0,
            &mut monitor,
            EDD_GET_DEVICE_INTERFACE_NAME,
        )
        .as_bool()
    };
    if !found {
        return None;
    }

    let device_id = wide_array_to_string(&monitor.DeviceID);
    let registry_path = monitor_registry_path(&device_id)?;
    let hklm = RegKey::predef(HKEY_LOCAL_MACHINE);
    let key = hklm.open_subkey(format!(r"SYSTEM\CurrentControlSet\Enum\DISPLAY\{registry_path}\Device Parameters")).ok()?;
    let edid = key.get_raw_value("EDID").ok()?;
    parse_edid_name(&edid.bytes)
}

#[cfg(windows)]
fn monitor_registry_path(device_id: &str) -> Option<String> {
    let parts = device_id.split('#').collect::<Vec<_>>();
    if parts.len() < 3 || !parts.get(0)?.ends_with(r"DISPLAY") {
        return None;
    }
    Some(format!(r"{}\{}", parts[1], parts[2]))
}

#[cfg(windows)]
fn parse_edid_name(edid: &[u8]) -> Option<String> {
    for offset in [54, 72, 90, 108] {
        if edid.len() < offset + 18 {
            continue;
        }
        let descriptor = &edid[offset..offset + 18];
        if descriptor[0..5] == [0, 0, 0, 0xfc, 0] {
            let name = descriptor[5..18]
                .iter()
                .copied()
                .take_while(|byte| *byte != b'\n' && *byte != b'\r' && *byte != 0)
                .collect::<Vec<_>>();
            let name = String::from_utf8_lossy(&name).trim().to_string();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

#[cfg(windows)]
fn display_device() -> DISPLAY_DEVICEW {
    DISPLAY_DEVICEW {
        cb: std::mem::size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    }
}

#[cfg(windows)]
fn to_wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn wide_array_to_string(value: &[u16]) -> String {
    let len = value.iter().position(|item| *item == 0).unwrap_or(value.len());
    String::from_utf16_lossy(&value[..len]).trim().to_string()
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

fn castboard_url(language: &str) -> Result<(WebviewUrl, String), String> {
    let dev_url = std::env::var("COLINK_CASTBOARD_DEV_URL")
        .unwrap_or_default()
        .trim()
        .to_string();
    if !dev_url.is_empty() {
        let mut url = Url::parse(&dev_url).map_err(|error| error.to_string())?;
        set_castboard_language(&mut url, language);
        return Ok((WebviewUrl::External(url.clone()), url.to_string()));
    }

    if cfg!(debug_assertions) {
        let mut url = Url::parse(DEFAULT_CASTBOARD_DEV_URL).map_err(|error| error.to_string())?;
        set_castboard_language(&mut url, language);
        return Ok((WebviewUrl::External(url.clone()), url.to_string()));
    }

    let mut query = url::form_urlencoded::Serializer::new(String::new());
    query.append_pair("lang", language);
    let path = format!("castboard/index.html?{}", query.finish());
    Ok((
        WebviewUrl::App(path.clone().into()),
        format!("app://{path}"),
    ))
}

fn set_castboard_language(url: &mut Url, language: &str) {
    let existing_parameters = url
        .query_pairs()
        .filter(|(key, _)| key != "lang")
        .map(|(key, value)| (key.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    let mut query = url.query_pairs_mut();
    query.clear();
    for (key, value) in existing_parameters {
        query.append_pair(&key, &value);
    }
    query.append_pair("lang", language);
}
