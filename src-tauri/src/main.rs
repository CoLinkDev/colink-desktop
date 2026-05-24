mod commands;
mod crypto;
mod error;
mod models;
mod network;
mod service;
mod state;
mod store;

use commands::{
    bootstrap_app, get_settings, list_devices, login, logout, register_account, update_settings,
};
use state::AppState;
use tauri::Manager;

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            let state = AppState::initialize(&app.handle())?;
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            bootstrap_app,
            login,
            register_account,
            logout,
            list_devices,
            get_settings,
            update_settings
        ])
        .run(tauri::generate_context!())
        .expect("failed to run CoLink desktop")
}
