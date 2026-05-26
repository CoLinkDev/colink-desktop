mod api;
mod commands;
mod crypto;
mod device_cache;
mod error;
mod models;
mod network;
mod protocol;
mod runtime;
mod runtime_events;
mod shell;
mod service;
mod state;
mod store;
mod sync;

use commands::{
    bootstrap_app, cancel_transfer, delete_device, get_settings, list_devices, login, logout,
    pick_download_directory, pick_files, register_account, rotate_device_key, send_files,
    send_text, update_device_name, update_settings,
};
use state::AppState;
use tauri::{Manager, WindowEvent};

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            let state = AppState::initialize(&app.handle())?;
            let settings = state.database.load_settings()?.unwrap_or_else(|| {
                panic!("application settings should exist after initialization")
            });
            app.manage(state);
            shell::apply_auto_start(settings.auto_start)?;
            let shell_state = shell::initialize(&app.handle(), &settings)?;
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
            rotate_device_key,
            get_settings,
            update_settings,
            pick_download_directory,
            send_text,
            pick_files,
            send_files,
            cancel_transfer
        ])
        .run(tauri::generate_context!())
        .expect("failed to run CoLink desktop")
}
