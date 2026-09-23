use std::{
    fs,
    path::{Path, PathBuf},
    sync::Mutex,
};

use tauri::{AppHandle, Manager};

use crate::{
    error::AppResult,
    models::AppSettings,
    network::{cloud::CloudConnectionManager, http::HttpClient},
    runtime::AppRuntime,
    store::db::Database,
};

pub struct AppState {
    pub app: AppHandle,
    pub database: Database,
    pub http: HttpClient,
    pub cloud: CloudConnectionManager,
    pub runtime: AppRuntime,
    pending_share_files: Mutex<Vec<String>>,
}

impl AppState {
    pub fn initialize(app: &AppHandle) -> AppResult<Self> {
        let app_dir = app_data_dir(app)?;
        fs::create_dir_all(&app_dir)?;

        let database = Database::new(app_dir.join("colink.db"));
        database.initialize()?;

        let default_download_path = resolve_download_path(&app_dir)?;
        database.ensure_settings(AppSettings::new(default_download_path).normalize())?;
        let http = HttpClient::new()?;
        let (runtime, cloud) = AppRuntime::build(app.clone(), database.clone(), http.clone());

        Ok(Self {
            app: app.clone(),
            database,
            http,
            cloud,
            runtime,
            pending_share_files: Mutex::new(Vec::new()),
        })
    }

    pub fn queue_share_files<I>(&self, paths: I)
    where
        I: IntoIterator<Item = String>,
    {
        let mut pending = self
            .pending_share_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        pending.extend(paths.into_iter().filter(|path| !path.trim().is_empty()));
    }

    pub fn take_share_files(&self) -> Vec<String> {
        let mut pending = self
            .pending_share_files
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        std::mem::take(&mut *pending)
    }
}

fn resolve_download_path(app_dir: &Path) -> AppResult<String> {
    let path = dirs::download_dir().unwrap_or_else(|| app_dir.join("downloads"));

    if !path.exists() {
        fs::create_dir_all(&path)?;
    }

    Ok(path.to_string_lossy().to_string())
}

pub fn app_data_dir(app: &AppHandle) -> AppResult<PathBuf> {
    let mut app_dir = app.path().app_data_dir()?;

    if cfg!(debug_assertions) {
        let file_name = app_dir
            .file_name()
            .ok_or_else(|| crate::error::AppError::message("invalid app data directory"))?;
        let file_name = file_name.to_string_lossy();
        if !file_name.ends_with(".debug") {
            app_dir.set_file_name(format!("{file_name}.debug"));
        }
    }

    Ok(app_dir)
}
