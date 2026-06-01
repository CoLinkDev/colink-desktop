use std::sync::atomic::{AtomicBool, Ordering};

use image::ImageReader;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WebviewWindow,
};

use crate::{
    error::AppResult,
    i18n::{self, TextKey},
    models::{AppSettings, CloudStatus, DeviceInfo},
    state::AppState,
};

pub const SHELL_NAVIGATE_EVENT: &str = "shell-navigate";

const TRAY_ID: &str = "main-tray";
const MENU_OPEN: &str = "tray-open";
const MENU_SETTINGS: &str = "tray-settings";
const MENU_QUIT: &str = "tray-quit";

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
                let _ = show_main_window(tray.app_handle(), None);
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
    let active_transfer = transfers
        .iter()
        .any(|item| matches!(item.status.as_str(), "offered" | "sending" | "receiving"));
    let icon_state = if active_transfer {
        "syncing"
    } else if cloud.connected {
        "connected"
    } else if devices.iter().any(|item| item.lan_available) {
        "idle"
    } else {
        "disconnected"
    };

    let menu = build_tray_menu(app)?;
    let _ = tray.set_menu(Some(menu));
    let _ = tray.set_icon(Some(build_tray_icon(icon_state)));
    let _ = tray.set_tooltip(Some(tray_tooltip(app, &cloud, &devices)));
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
    let language = app
        .try_state::<AppState>()
        .and_then(|state| state.database.load_settings().ok().flatten())
        .map(|settings| settings.language)
        .unwrap_or_else(i18n::default_language_code);
    let open = MenuItem::with_id(
        app,
        MENU_OPEN,
        i18n::text(&language, TextKey::TrayOpen),
        true,
        None::<&str>,
    )?;
    let settings = MenuItem::with_id(
        app,
        MENU_SETTINGS,
        i18n::text(&language, TextKey::TraySettings),
        true,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(
        app,
        MENU_QUIT,
        i18n::text(&language, TextKey::TrayQuit),
        true,
        None::<&str>,
    )?;
    let separator = PredefinedMenuItem::separator(app)?;

    Menu::with_items(app, &[&open, &settings, &separator, &quit]).map_err(Into::into)
}

fn main_window(app: &AppHandle) -> AppResult<WebviewWindow<tauri::Wry>> {
    app.get_webview_window("main")
        .ok_or_else(|| crate::error::AppError::message("main window does not exist"))
}

fn tray_tooltip(app: &AppHandle, cloud: &CloudStatus, devices: &[DeviceInfo]) -> String {
    let language = app
        .try_state::<AppState>()
        .and_then(|state| state.database.load_settings().ok().flatten())
        .map(|settings| settings.language)
        .unwrap_or_else(i18n::default_language_code);
    let online = devices.iter().filter(|item| item.online).count();
    let lan = devices.iter().filter(|item| item.lan_available).count();
    format!(
        "CoLink Desktop\n{}: {}\n{}: {online}\n{}: {lan}",
        i18n::text(&language, TextKey::TrayCloud),
        i18n::cloud_state(&language, &cloud.state),
        i18n::text(&language, TextKey::TrayReachableDevices),
        i18n::text(&language, TextKey::TrayLan),
    )
}

fn build_tray_icon(state: &str) -> Image<'static> {
    let bytes = match state {
        "connected" => include_bytes!("../icons/tray/connected.png").as_slice(),
        "syncing" => include_bytes!("../icons/tray/syncing.png").as_slice(),
        "idle" => include_bytes!("../icons/tray/idle.png").as_slice(),
        _ => include_bytes!("../icons/tray/disconnected.png").as_slice(),
    };
    let image = ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .expect("tray icon format should be readable")
        .decode()
        .expect("tray icon should decode")
        .into_rgba8();
    let (width, height) = image.dimensions();
    Image::new_owned(image.into_raw(), width, height)
}
