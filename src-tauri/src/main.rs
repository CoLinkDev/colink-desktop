#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod api;
mod auth;
mod commands;
mod crypto;
mod dev_log;
mod device_cache;
mod device_presence;
mod error;
mod i18n;
mod models;
mod music;
mod network;
mod protocol;
mod runtime;
mod runtime_events;
mod sysinfo;
mod service;
mod shell;
mod state;
mod store;
mod sync;

use commands::{
    bootstrap_app, cancel_transfer, check_update, clear_saved_login, clear_transfers,
    delete_device, forget_lan_trust, get_castboard_status, get_music_providers, get_saved_login,
    get_settings, handle_castboard_window_event, list_available_music_providers,
    list_castboard_monitors, list_devices, list_lan_pairing_candidates, list_logs, login, logout,
    open_castboard_on_monitor, open_update_download, pending_file_offers, pick_download_directory,
    pick_files, register_account, respond_file_offer, respond_lan_pairing, rotate_device_key,
    save_saved_login, send_files, send_text, start_lan_pairing, stop_castboard, update_device_name,
    update_music_providers, update_settings,
};
use state::AppState;
use tauri::{Manager, WindowEvent};

fn main() {
    let _ = rustls::crypto::ring::default_provider().install_default();

    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            let _ = shell::show_main_window(app, None);
        }))
        .setup(|app| {
            let tracing_guard = dev_log::initialize(app.handle())?;
            app.manage(tracing_guard);
            let state = AppState::initialize(app.handle())?;
            let settings = state.database.load_settings()?.unwrap_or_else(|| {
                panic!("application settings should exist after initialization")
            });
            app.manage(state);
            shell::apply_auto_start(settings.auto_start)?;
            let shell_state = shell::initialize(app.handle(), &settings)?;
            app.manage(shell_state);
            Ok(())
        })
        .on_menu_event(|app, event| {
            let _ = shell::handle_menu_event(app, event.id().0.as_ref());
        })
        .on_window_event(|window, event| {
            if window.label() == "castboard" {
                let app = window.app_handle();
                let state = app.state::<AppState>();
                handle_castboard_window_event(app, &state, event);
                return;
            }
            if window.label() != "main" {
                return;
            }
            if let WindowEvent::CloseRequested { api, .. } = event {
                let shell_state = window.app_handle().state::<shell::ShellState>();
                if shell_state.should_allow_exit() {
                    return;
                }
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap_app,
            login,
            register_account,
            logout,
            get_saved_login,
            save_saved_login,
            clear_saved_login,
            list_logs,
            list_devices,
            update_device_name,
            delete_device,
            forget_lan_trust,
            rotate_device_key,
            list_lan_pairing_candidates,
            start_lan_pairing,
            respond_lan_pairing,
            get_settings,
            update_settings,
            get_music_providers,
            update_music_providers,
            list_available_music_providers,
            list_castboard_monitors,
            get_castboard_status,
            open_castboard_on_monitor,
            stop_castboard,
            check_update,
            open_update_download,
            pick_download_directory,
            send_text,
            pick_files,
            send_files,
            cancel_transfer,
            pending_file_offers,
            respond_file_offer,
            clear_transfers
        ])
        .run(tauri::generate_context!())
        .expect("failed to run CoLink desktop")
}
