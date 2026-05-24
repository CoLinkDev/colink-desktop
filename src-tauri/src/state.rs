use std::{fs, path::PathBuf};

use tauri::{AppHandle, Manager};

use crate::{
    error::AppResult,
    models::AppSettings,
    network::{cloud::CloudConnectionManager, http::HttpClient},
    store::db::Database,
};

pub struct AppState {
    pub database: Database,
    pub http: HttpClient,
    pub cloud: CloudConnectionManager,
}

impl AppState {
    pub fn initialize(app: &AppHandle) -> AppResult<Self> {
        let app_dir = app.path().app_data_dir()?;
        fs::create_dir_all(&app_dir)?;

        let database = Database::new(app_dir.join("colink.db"));
        database.initialize()?;

        let default_download_path = resolve_download_path(&app_dir)?;
        database.ensure_settings(AppSettings::new(default_download_path).normalize())?;
        let http = HttpClient::new()?;
        let cloud = CloudConnectionManager::new(app.clone(), database.clone(), http.clone());

        Ok(Self {
            database,
            http,
            cloud,
        })
    }
}

fn resolve_download_path(app_dir: &PathBuf) -> AppResult<String> {
    let path = dirs::download_dir().unwrap_or_else(|| app_dir.join("downloads"));

    if !path.exists() {
        fs::create_dir_all(&path)?;
    }

    Ok(path.to_string_lossy().to_string())
}
