use std::sync::atomic::{AtomicBool, Ordering};

use image::{Rgba, RgbaImage};
use tauri::{
    image::Image,
    menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WebviewWindow,
};

use crate::{
    error::AppResult,
    models::{AppSettings, CloudStatus, DeviceInfo},
    state::AppState,
};

pub const SHELL_NAVIGATE_EVENT: &str = "shell-navigate";

const TRAY_ID: &str = "main-tray";
const MENU_OPEN: &str = "tray-open";
const MENU_SETTINGS: &str = "tray-settings";
const MENU_QUIT: &str = "tray-quit";
const MENU_DEVICE_PREFIX: &str = "tray-device:";

pub struct ShellState {
    allow_exit: AtomicBool,
}

impl ShellState {
    pub fn new() -> Self {
        Self {
            allow_exit: AtomicBool::new(false),
        }
    }

    pub fn allow_exit(&self) {
        self.allow_exit.store(true, Ordering::SeqCst);
    }

    pub fn should_allow_exit(&self) -> bool {
        self.allow_exit.load(Ordering::SeqCst)
    }
}

pub fn initialize(app: &AppHandle, settings: &AppSettings) -> AppResult<ShellState> {
    let shell = ShellState::new();
    let menu = build_tray_menu(app)?;
    let icon = build_tray_icon("disconnected");

    let _tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .icon(icon)
        .tooltip("CoLink Desktop")
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = show_main_window(&tray.app_handle(), None);
            }
        })
        .build(app)?;

    if settings.start_minimized {
        if let Some(window) = app.get_webview_window("main") {
            let _ = window.hide();
        }
    }

    refresh_tray(app)?;
    Ok(shell)
}

pub fn refresh_tray(app: &AppHandle) -> AppResult<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };

    let state = app.state::<AppState>();
    let devices = state.database.load_cached_devices().unwrap_or_default();
    let transfers = state.database.load_transfers(50).unwrap_or_default();
    let cloud = state.cloud.snapshot();
    let active_transfer = transfers.iter().any(|item| {
        matches!(item.status.as_str(), "offered" | "sending" | "receiving")
    });
    let icon_state = if active_transfer {
        "syncing"
    } else if cloud.connected || devices.iter().any(|item| item.lan_available) {
        "connected"
    } else {
        "disconnected"
    };

    let menu = build_tray_menu(app)?;
    let _ = tray.set_menu(Some(menu));
    let _ = tray.set_icon(Some(build_tray_icon(icon_state)));
    let _ = tray.set_tooltip(Some(tray_tooltip(&cloud, &devices)));
    Ok(())
}

pub fn handle_menu_event(app: &AppHandle, id: &str) -> AppResult<()> {
    if id == MENU_OPEN {
        return show_main_window(app, None);
    }
    if id == MENU_SETTINGS {
        return show_main_window(app, Some("/settings"));
    }
    if id == MENU_QUIT {
        return quit_app(app);
    }
    if let Some(device_id) = id.strip_prefix(MENU_DEVICE_PREFIX) {
        return show_main_window(app, Some(&format!("/messages?deviceId={device_id}")));
    }
    Ok(())
}

pub fn show_main_window(app: &AppHandle, route: Option<&str>) -> AppResult<()> {
    let window = main_window(app)?;
    window.show()?;
    window.unminimize()?;
    window.set_focus()?;
    if let Some(route) = route {
        let _ = app.emit(SHELL_NAVIGATE_EVENT, route.to_string());
    }
    Ok(())
}

pub fn apply_auto_start(enabled: bool) -> AppResult<()> {
    #[cfg(windows)]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
        use winreg::RegKey;

        let run = RegKey::predef(HKEY_CURRENT_USER).open_subkey_with_flags(
            "Software\\Microsoft\\Windows\\CurrentVersion\\Run",
            KEY_READ | KEY_WRITE,
        )?;
        let key_name = "CoLink Desktop";
        if enabled {
            let exe = std::env::current_exe()?;
            run.set_value(key_name, &format!("\"{}\"", exe.display()))?;
        } else {
            let _ = run.delete_value(key_name);
        }
    }

    #[cfg(not(windows))]
    let _ = enabled;

    Ok(())
}

pub fn quit_app(app: &AppHandle) -> AppResult<()> {
    app.state::<ShellState>().allow_exit();
    {
        let state = app.state::<AppState>();
        state.cloud.stop();
        let _ = state.runtime.deactivate();
    }
    app.exit(0);
    Ok(())
}

fn build_tray_menu(app: &AppHandle) -> AppResult<Menu<tauri::Wry>> {
    let open = MenuItem::with_id(app, MENU_OPEN, "打开", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, MENU_SETTINGS, "设置", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, MENU_QUIT, "退出", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;

    let online_devices = app
        .state::<AppState>()
        .database
        .load_cached_devices()
        .unwrap_or_default()
        .into_iter()
        .filter(|item| item.online || item.lan_available)
        .collect::<Vec<_>>();
    let online_submenu = build_online_devices_submenu(app, &online_devices)?;

    Menu::with_items(app, &[&open, &online_submenu, &settings, &separator, &quit]).map_err(Into::into)
}

fn build_online_devices_submenu(
    app: &AppHandle,
    devices: &[DeviceInfo],
) -> AppResult<Submenu<tauri::Wry>> {
    if devices.is_empty() {
        let empty = MenuItem::with_id(app, "tray-device-empty", "暂无在线设备", false, None::<&str>)?;
        return Submenu::with_items(app, "在线设备", true, &[&empty]).map_err(Into::into);
    }

    let items = devices
        .iter()
        .map(|item| {
            let route = item.active_route.clone().unwrap_or_else(|| "cloud".to_string());
            MenuItem::with_id(
                app,
                format!("{MENU_DEVICE_PREFIX}{}", item.device_id),
                format!("{} ({route})", item.name),
                true,
                None::<&str>,
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let refs = items.iter().map(|item| item as &dyn IsMenuItem<_>).collect::<Vec<_>>();
    Submenu::with_items(app, "在线设备", true, &refs).map_err(Into::into)
}

fn main_window(app: &AppHandle) -> AppResult<WebviewWindow<tauri::Wry>> {
    app.get_webview_window("main")
        .ok_or_else(|| crate::error::AppError::message("主窗口不存在"))
}

fn tray_tooltip(cloud: &CloudStatus, devices: &[DeviceInfo]) -> String {
    let online = devices.iter().filter(|item| item.online).count();
    format!("CoLink Desktop\n云端: {}\n在线设备: {online}", cloud.state)
}

fn build_tray_icon(state: &str) -> Image<'static> {
    let color = match state {
        "connected" => [52, 211, 153, 255],
        "syncing" => [251, 191, 36, 255],
        _ => [239, 68, 68, 255],
    };
    let mut image = RgbaImage::from_pixel(32, 32, Rgba([0, 0, 0, 0]));
    for y in 0..32 {
        for x in 0..32 {
            let dx = x as i32 - 16;
            let dy = y as i32 - 16;
            let distance = dx * dx + dy * dy;
            if distance <= 100 {
                image.put_pixel(x, y, Rgba(color));
            } else if distance <= 132 {
                image.put_pixel(x, y, Rgba([255, 255, 255, 180]));
            }
        }
    }
    Image::new_owned(image.into_raw(), 32, 32)
}
