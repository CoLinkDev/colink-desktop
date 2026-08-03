use std::{
    fs,
    path::Path,
    sync::Mutex,
    time::{Duration, SystemTime},
};

use tauri::AppHandle;
use tracing_appender::{
    non_blocking::WorkerGuard,
    rolling::{RollingFileAppender, Rotation},
};
use tracing_subscriber::EnvFilter;

use crate::{
    error::{AppError, AppResult},
    state,
};

const LOG_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

pub struct TracingGuard {
    _guard: Mutex<Option<WorkerGuard>>,
}

pub fn initialize(app: &AppHandle) -> AppResult<TracingGuard> {
    let log_dir = state::app_data_dir(app)?.join("logs");
    fs::create_dir_all(&log_dir)?;

    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("colink")
        .filename_suffix("log")
        .build(&log_dir)
        .map_err(|error| AppError::message(format!("failed to initialize log file: {error}")))?;
    let (writer, guard) = tracing_appender::non_blocking(file_appender);
    let filter = std::env::var("COLINK_LOG")
        .ok()
        .and_then(|value| EnvFilter::try_new(value).ok())
        .unwrap_or_else(default_log_filter);

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(true)
        .try_init()
        .map_err(|error| AppError::message(format!("failed to initialize tracing: {error}")))?;

    if let Err(error) = prune_old_logs(&log_dir) {
        tracing::warn!(%error, "failed to prune old log files");
    }
    tracing::info!(log_dir = %log_dir.display(), "developer logging initialized");

    Ok(TracingGuard {
        _guard: Mutex::new(Some(guard)),
    })
}

fn default_log_filter() -> EnvFilter {
    if cfg!(debug_assertions) {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info,colink_desktop::network::lan=debug,mdns_sd=debug")
    }
}

fn prune_old_logs(log_dir: &Path) -> AppResult<()> {
    let cutoff = SystemTime::now()
        .checked_sub(LOG_RETENTION)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    for entry in fs::read_dir(log_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }

        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if !file_name.starts_with("colink.") || !file_name.ends_with(".log") {
            continue;
        }

        let metadata = entry.metadata()?;
        let timestamp = metadata.modified().or_else(|_| metadata.created())?;
        if timestamp < cutoff {
            fs::remove_file(entry.path())?;
        }
    }

    Ok(())
}
