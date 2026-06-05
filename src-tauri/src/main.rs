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
mod service;
mod shell;
mod state;
mod store;
mod sync;

use commands::{
    bootstrap_app, cancel_transfer, clear_transfers, delete_device, forget_lan_trust, get_settings,
    list_devices, list_lan_pairing_candidates, login, logout, pick_download_directory, pick_files,
    register_account, respond_lan_pairing, rotate_device_key, send_files, send_text,
    start_lan_pairing, update_device_name, update_settings,
};
use state::AppState;
use tauri::{Manager, WindowEvent};

fn main() {
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
            pick_download_directory,
            send_text,
            pick_files,
            send_files,
            cancel_transfer,
            clear_transfers
        ])
        .run(tauri::generate_context!())
        .expect("failed to run CoLink desktop")
}
