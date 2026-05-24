use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

use crate::{
    error::AppResult,
    models::{unix_now, AppSettings, DeviceIdentity, DeviceInfo, SessionRecord},
};

const SETTINGS_KEY: &str = "settings";
const SESSION_KEY: &str = "session";
const DEVICE_IDENTITY_KEY: &str = "device_identity";
const DEVICE_CACHE_KEY: &str = "device_cache";

#[derive(Clone)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn initialize(&self) -> AppResult<()> {
        let connection = self.open()?;
        connection.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS kv_store (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at INTEGER NOT NULL
            );
            ",
        )?;
        Ok(())
    }

    pub fn ensure_settings(&self, default_settings: AppSettings) -> AppResult<AppSettings> {
        if let Some(settings) = self.load_settings()? {
            return Ok(settings);
        }

        self.save_settings(&default_settings)?;
        Ok(default_settings)
    }

    pub fn load_settings(&self) -> AppResult<Option<AppSettings>> {
        self.load_record(SETTINGS_KEY)
    }

    pub fn save_settings(&self, settings: &AppSettings) -> AppResult<()> {
        self.save_record(SETTINGS_KEY, settings)
    }

    pub fn load_session(&self) -> AppResult<Option<SessionRecord>> {
        self.load_record(SESSION_KEY)
    }

    pub fn save_session(&self, session: &SessionRecord) -> AppResult<()> {
        self.save_record(SESSION_KEY, session)
    }

    pub fn clear_session(&self) -> AppResult<()> {
        self.delete_record(SESSION_KEY)
    }

    pub fn load_device_identity(&self) -> AppResult<Option<DeviceIdentity>> {
        self.load_record(DEVICE_IDENTITY_KEY)
    }

    pub fn save_device_identity(&self, identity: &DeviceIdentity) -> AppResult<()> {
        self.save_record(DEVICE_IDENTITY_KEY, identity)
    }

    pub fn load_cached_devices(&self) -> AppResult<Vec<DeviceInfo>> {
        Ok(self.load_record(DEVICE_CACHE_KEY)?.unwrap_or_default())
    }

    pub fn save_cached_devices(&self, devices: &[DeviceInfo]) -> AppResult<()> {
        self.save_record(DEVICE_CACHE_KEY, devices)
    }

    pub fn clear_cached_devices(&self) -> AppResult<()> {
        self.delete_record(DEVICE_CACHE_KEY)
    }

    fn open(&self) -> AppResult<Connection> {
        Ok(Connection::open(&self.path)?)
    }

    fn load_record<T>(&self, key: &str) -> AppResult<Option<T>>
    where
        T: DeserializeOwned,
    {
        let connection = self.open()?;
        let value: Option<String> = connection
            .query_row(
                "SELECT value FROM kv_store WHERE key = ?1",
                params![key],
                |row| row.get(0),
            )
            .optional()?;

        value
            .map(|json| serde_json::from_str(&json))
            .transpose()
            .map_err(Into::into)
    }

    fn save_record<T>(&self, key: &str, value: &T) -> AppResult<()>
    where
        T: Serialize + ?Sized,
    {
        let connection = self.open()?;
        let json = serde_json::to_string(value)?;
        connection.execute(
            "
            INSERT INTO kv_store (key, value, updated_at)
            VALUES (?1, ?2, ?3)
            ON CONFLICT(key)
            DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at
            ",
            params![key, json, unix_now()],
        )?;
        Ok(())
    }

    fn delete_record(&self, key: &str) -> AppResult<()> {
        let connection = self.open()?;
        connection.execute("DELETE FROM kv_store WHERE key = ?1", params![key])?;
        Ok(())
    }
}
