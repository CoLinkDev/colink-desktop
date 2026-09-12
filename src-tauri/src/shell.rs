use std::{
    process::Command,
    sync::{
        atomic::{AtomicBool, Ordering},
        Mutex,
    },
};

#[cfg(all(unix, not(target_os = "macos")))]
use std::{
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use image::ImageReader;
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, WebviewWindow,
};
use url::form_urlencoded;

use crate::{
    error::AppResult,
    i18n::{self, TextKey},
    models::{AppSettings, CloudStatus, DeviceInfo},
    state::AppState,
    sync::MutexExt,
};

#[cfg(all(unix, not(target_os = "macos")))]
use crate::error::AppError;

pub const SHELL_NAVIGATE_EVENT: &str = "shell-navigate";

const TRAY_ID: &str = "main-tray";
const MENU_OPEN: &str = "tray-open";
const MENU_SETTINGS: &str = "tray-settings";
const MENU_QUIT: &str = "tray-quit";
const MENU_DEVICES: &str = "tray-devices";
const MENU_DEVICE_PREFIX: &str = "tray-device:";
const MAX_TRAY_DEVICES: usize = 8;

#[cfg(all(unix, not(target_os = "macos")))]
const LINUX_AUTOSTART_FILE: &str = "dev.colink.desktop.desktop";

pub struct ShellState {
    allow_exit: AtomicBool,
    tray_menu_snapshot: Mutex<TrayMenuSnapshot>,
    tray_icons: TrayIcons,
}

#[derive(Clone, PartialEq, Eq)]
struct TrayMenuSnapshot {
    language: String,
    cloud_state: String,
    devices: Vec<TrayMenuDevice>,
}

#[derive(Clone, PartialEq, Eq)]
struct TrayMenuDevice {
    device_id: String,
    name: String,
    route: String,
}

struct TrayIcons {
    connected: DecodedTrayIcon,
    activity: DecodedTrayIcon,
    idle: DecodedTrayIcon,
    disconnected: DecodedTrayIcon,
}

struct DecodedTrayIcon {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

impl TrayIcons {
    fn load() -> Self {
        Self {
            connected: DecodedTrayIcon::decode(include_bytes!("../icons/tray/connected.png")),
            activity: DecodedTrayIcon::decode(include_bytes!("../icons/tray/activity.png")),
            idle: DecodedTrayIcon::decode(include_bytes!("../icons/tray/idle.png")),
            disconnected: DecodedTrayIcon::decode(include_bytes!("../icons/tray/disconnected.png")),
        }
    }

    fn image(&self, state: &str) -> Image<'_> {
        match state {
            "connected" => self.connected.image(),
            "activity" => self.activity.image(),
            "idle" => self.idle.image(),
            _ => self.disconnected.image(),
        }
    }
}

impl DecodedTrayIcon {
    fn decode(bytes: &[u8]) -> Self {
        let image = ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .expect("tray icon format should be readable")
            .decode()
            .expect("tray icon should decode")
            .into_rgba8();
        let (width, height) = image.dimensions();
        Self {
            rgba: image.into_raw(),
            width,
            height,
        }
    }

    fn image(&self) -> Image<'_> {
        Image::new(&self.rgba, self.width, self.height)
    }
}

impl ShellState {
    fn new(tray_menu_snapshot: TrayMenuSnapshot, tray_icons: TrayIcons) -> Self {
        Self {
            allow_exit: AtomicBool::new(false),
            tray_menu_snapshot: Mutex::new(tray_menu_snapshot),
            tray_icons,
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
    let state = app.state::<AppState>();
    let devices = state.database.load_cached_devices().unwrap_or_default();
    let cloud = state.cloud.snapshot();
    let tray_menu_snapshot = TrayMenuSnapshot::new(settings, &cloud, &devices);
    let menu = build_tray_menu(app, &tray_menu_snapshot)?;
    let tray_icons = TrayIcons::load();
    let icon = tray_icons.image("disconnected");

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

    Ok(ShellState::new(tray_menu_snapshot, tray_icons))
}

pub fn refresh_tray(app: &AppHandle) -> AppResult<()> {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return Ok(());
    };

    let state = app.state::<AppState>();
    let devices = state.database.load_cached_devices().unwrap_or_default();
    let cloud = state.cloud.snapshot();
    let settings = state.database.load_settings()?;
    let shell = app.state::<ShellState>();

    if let Some(settings) = settings {
        let next_snapshot = TrayMenuSnapshot::new(&settings, &cloud, &devices);
        let menu_changed = *shell.tray_menu_snapshot.lock_unpoisoned() != next_snapshot;
        if menu_changed {
            tray.set_menu(Some(build_tray_menu(app, &next_snapshot)?))?;
            *shell.tray_menu_snapshot.lock_unpoisoned() = next_snapshot;
        }
    }

    let icon_state = if state.runtime.indicator().is_active() {
        "activity"
    } else if cloud.connected {
        "connected"
    } else if state.runtime.lan_is_active() {
        "idle"
    } else {
        "disconnected"
    };

    let _ = tray.set_icon(Some(shell.tray_icons.image(icon_state)));
    let _ = tray.set_tooltip(Some(tray_tooltip(app, &cloud, &devices)));
    Ok(())
}

pub fn refresh_tray_menu_labels(app: &AppHandle, _language: &str) -> AppResult<()> {
    refresh_tray(app)
}

pub fn handle_menu_event(app: &AppHandle, id: &str) -> AppResult<()> {
    if id == MENU_OPEN {
        return show_main_window(app, None);
    }
    if id == MENU_SETTINGS {
        return show_main_window(app, Some("/settings"));
    }
    if id == MENU_DEVICES {
        return show_main_window(app, Some("/devices"));
    }
    if let Some(device_id) = id.strip_prefix(MENU_DEVICE_PREFIX) {
        let query = form_urlencoded::Serializer::new(String::new())
            .append_pair("deviceId", device_id)
            .finish();
        return show_main_window(app, Some(&format!("/messages?{query}")));
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
    if cfg!(debug_assertions) {
        return Ok(());
    }

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

    #[cfg(all(unix, not(target_os = "macos")))]
    apply_linux_auto_start(enabled)?;

    #[cfg(any(target_os = "macos", not(any(windows, unix))))]
    let _ = enabled;

    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn apply_linux_auto_start(enabled: bool) -> AppResult<()> {
    let path = linux_autostart_path()?;

    if !enabled {
        return match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error.into()),
        };
    }

    let executable = std::env::current_exe()?;
    let entry = linux_autostart_entry(&executable)?;
    let directory = path
        .parent()
        .ok_or_else(|| AppError::message("Linux autostart path has no parent directory"))?;
    fs::create_dir_all(directory)?;
    fs::write(path, entry)?;
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_autostart_path() -> AppResult<PathBuf> {
    let config_dir = dirs::config_dir()
        .ok_or_else(|| AppError::message("Unable to resolve the XDG config directory"))?;
    Ok(config_dir.join("autostart").join(LINUX_AUTOSTART_FILE))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn linux_autostart_entry(executable: &Path) -> AppResult<String> {
    let executable = executable
        .to_str()
        .ok_or_else(|| AppError::message("Autostart executable path is not valid UTF-8"))?;
    let executable = desktop_entry_exec_value(executable);
    Ok(format!(
        "[Desktop Entry]\nType=Application\nName=CoLink Desktop\nExec={executable}\nTerminal=false\nX-GNOME-Autostart-enabled=true\n"
    ))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn desktop_entry_exec_value(value: &str) -> String {
    if !value
        .chars()
        .any(|character| matches!(character, ' ' | '\t' | '\n' | '"' | '\\' | '%'))
    {
        return value.to_string();
    }

    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    format!("\"{escaped}\"")
}

#[cfg(all(test, unix, not(target_os = "macos")))]
mod linux_autostart_tests {
    use std::path::Path;

    use super::linux_autostart_entry;

    #[test]
    fn creates_a_desktop_entry_for_an_executable_path_with_spaces() {
        let entry = linux_autostart_entry(Path::new("/opt/CoLink Desktop/colink-desktop"))
            .expect("create autostart entry");

        assert!(entry.contains("Type=Application\n"));
        assert!(entry.contains("Exec=\"/opt/CoLink Desktop/colink-desktop\"\n"));
        assert!(entry.contains("X-GNOME-Autostart-enabled=true\n"));
    }
}

pub fn open_external_url(url: &str) -> AppResult<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("rundll32")
            .arg("url.dll,FileProtocolHandler")
            .arg(url)
            .spawn()?;
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url).spawn()?;
    }

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

fn build_tray_menu(app: &AppHandle, snapshot: &TrayMenuSnapshot) -> AppResult<Menu<tauri::Wry>> {
    let language = &snapshot.language;
    let open = MenuItem::with_id(
        app,
        MENU_OPEN,
        i18n::text(&language, TextKey::TrayOpen),
        true,
        None::<&str>,
    )?;
    let cloud_status = MenuItem::new(
        app,
        format!(
            "{}: {}",
            i18n::text(language, TextKey::TrayCloud),
            i18n::cloud_state(language, &snapshot.cloud_state)
        ),
        false,
        None::<&str>,
    )?;
    let lan_count = snapshot
        .devices
        .iter()
        .filter(|device| device.route == "lan")
        .count();
    let device_status = MenuItem::new(
        app,
        format!(
            "{}: {} · {}: {lan_count}",
            i18n::text(language, TextKey::TrayReachableDevices),
            snapshot.devices.len(),
            i18n::text(language, TextKey::TrayLan),
        ),
        false,
        None::<&str>,
    )?;
    let devices_menu = Submenu::new(
        app,
        format!(
            "{} ({})",
            i18n::text(language, TextKey::TrayReachableDevices),
            snapshot.devices.len()
        ),
        true,
    )?;
    if snapshot.devices.is_empty() {
        devices_menu.append(&MenuItem::new(
            app,
            i18n::text(language, TextKey::TrayNoReachableDevices),
            false,
            None::<&str>,
        )?)?;
    } else {
        for device in snapshot.devices.iter().take(MAX_TRAY_DEVICES) {
            let route = if device.route == "lan" {
                i18n::text(language, TextKey::TrayLan)
            } else {
                i18n::text(language, TextKey::TrayCloud)
            };
            devices_menu.append(&MenuItem::with_id(
                app,
                format!("{MENU_DEVICE_PREFIX}{}", device.device_id),
                escape_menu_text(&format!("{} · {route}", device.name)),
                true,
                None::<&str>,
            )?)?;
        }
    }
    devices_menu.append(&PredefinedMenuItem::separator(app)?)?;
    devices_menu.append(&MenuItem::with_id(
        app,
        MENU_DEVICES,
        i18n::text(language, TextKey::TrayViewAllDevices),
        true,
        None::<&str>,
    )?)?;
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
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &cloud_status,
            &device_status,
            &devices_menu,
            &PredefinedMenuItem::separator(app)?,
            &settings,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;
    Ok(menu)
}

impl TrayMenuSnapshot {
    fn new(settings: &AppSettings, cloud: &CloudStatus, devices: &[DeviceInfo]) -> Self {
        let mut devices = devices
            .iter()
            .filter(|device| {
                !device.device_sources.iter().any(|source| source == "local")
                    && (device.lan_available || device.cloud_available)
            })
            .map(|device| TrayMenuDevice {
                device_id: device.device_id.clone(),
                name: if device.name.trim().is_empty() {
                    device.device_id.clone()
                } else {
                    device.name.trim().to_string()
                },
                route: if device.active_route.as_deref() == Some("lan")
                    || (device.lan_available && !device.cloud_available)
                {
                    "lan".to_string()
                } else {
                    "cloud".to_string()
                },
            })
            .collect::<Vec<_>>();
        devices.sort_by(|left, right| {
            let left_route = usize::from(left.route != "lan");
            let right_route = usize::from(right.route != "lan");
            left_route
                .cmp(&right_route)
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.device_id.cmp(&right.device_id))
        });

        Self {
            language: settings.language.clone(),
            cloud_state: cloud.state.clone(),
            devices,
        }
    }
}

fn escape_menu_text(value: &str) -> String {
    value
        .replace('\r', " ")
        .replace('\n', " ")
        .replace('&', "&&")
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
