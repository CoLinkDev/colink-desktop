use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

use crate::{
    error::AppResult,
    models::{
        unix_now, AppLogEntry, AppSettings, DeviceIdentity, DeviceInfo, FileTransferRecord,
        SessionRecord, TextMessageRecord,
    },
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

            CREATE TABLE IF NOT EXISTS messages (
                message_id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL,
                direction TEXT NOT NULL,
                text TEXT NOT NULL,
                route TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_device_created_at
                ON messages (device_id, created_at);

            CREATE TABLE IF NOT EXISTS file_transfers (
                file_id TEXT PRIMARY KEY,
                device_id TEXT NOT NULL,
                direction TEXT NOT NULL,
                file_name TEXT NOT NULL,
                file_size INTEGER NOT NULL,
                transferred_bytes INTEGER NOT NULL,
                total_chunks INTEGER NOT NULL,
                status TEXT NOT NULL,
                checksum TEXT NOT NULL,
                route TEXT NOT NULL,
                temp_path TEXT,
                final_path TEXT,
                error TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_file_transfers_device_updated_at
                ON file_transfers (device_id, updated_at);

            CREATE TABLE IF NOT EXISTS app_logs (
                id TEXT PRIMARY KEY,
                level TEXT NOT NULL,
                source TEXT NOT NULL,
                message TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_app_logs_created_at
                ON app_logs (created_at DESC);
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

    pub fn clear_device_identity(&self) -> AppResult<()> {
        self.delete_record(DEVICE_IDENTITY_KEY)
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

    pub fn load_messages(&self, limit: usize) -> AppResult<Vec<TextMessageRecord>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "
            SELECT
                message_id,
                device_id,
                direction,
                text,
                route,
                created_at
            FROM messages
            ORDER BY created_at ASC
            LIMIT ?1
            ",
        )?;

        let rows = statement.query_map(params![limit as i64], |row| {
            Ok(TextMessageRecord {
                message_id: row.get(0)?,
                device_id: row.get(1)?,
                direction: row.get(2)?,
                text: row.get(3)?,
                route: row.get(4)?,
                created_at: row.get(5)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn save_message(&self, message: &TextMessageRecord) -> AppResult<()> {
        let connection = self.open()?;
        connection.execute(
            "
            INSERT INTO messages (
                message_id,
                device_id,
                direction,
                text,
                route,
                created_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6)
            ON CONFLICT(message_id) DO UPDATE SET
                device_id = excluded.device_id,
                direction = excluded.direction,
                text = excluded.text,
                route = excluded.route,
                created_at = excluded.created_at
            ",
            params![
                message.message_id,
                message.device_id,
                message.direction,
                message.text,
                message.route,
                message.created_at
            ],
        )?;
        Ok(())
    }

    pub fn load_transfers(&self, limit: usize) -> AppResult<Vec<FileTransferRecord>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "
            SELECT
                file_id,
                device_id,
                direction,
                file_name,
                file_size,
                transferred_bytes,
                total_chunks,
                status,
                checksum,
                route,
                temp_path,
                final_path,
                error,
                created_at,
                updated_at
            FROM file_transfers
            ORDER BY updated_at DESC
            LIMIT ?1
            ",
        )?;

        let rows = statement.query_map(params![limit as i64], |row| {
            Ok(FileTransferRecord {
                file_id: row.get(0)?,
                device_id: row.get(1)?,
                direction: row.get(2)?,
                file_name: row.get(3)?,
                file_size: row.get(4)?,
                transferred_bytes: row.get(5)?,
                total_chunks: row.get(6)?,
                status: row.get(7)?,
                checksum: row.get(8)?,
                route: row.get(9)?,
                temp_path: row.get(10)?,
                final_path: row.get(11)?,
                error: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn load_transfer(&self, file_id: &str) -> AppResult<Option<FileTransferRecord>> {
        let connection = self.open()?;
        connection
            .query_row(
                "
                SELECT
                    file_id,
                    device_id,
                    direction,
                    file_name,
                    file_size,
                    transferred_bytes,
                    total_chunks,
                    status,
                    checksum,
                    route,
                    temp_path,
                    final_path,
                    error,
                    created_at,
                    updated_at
                FROM file_transfers
                WHERE file_id = ?1
                ",
                params![file_id],
                |row| {
                    Ok(FileTransferRecord {
                        file_id: row.get(0)?,
                        device_id: row.get(1)?,
                        direction: row.get(2)?,
                        file_name: row.get(3)?,
                        file_size: row.get(4)?,
                        transferred_bytes: row.get(5)?,
                        total_chunks: row.get(6)?,
                        status: row.get(7)?,
                        checksum: row.get(8)?,
                        route: row.get(9)?,
                        temp_path: row.get(10)?,
                        final_path: row.get(11)?,
                        error: row.get(12)?,
                        created_at: row.get(13)?,
                        updated_at: row.get(14)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn save_transfer(&self, transfer: &FileTransferRecord) -> AppResult<()> {
        let connection = self.open()?;
        connection.execute(
            "
            INSERT INTO file_transfers (
                file_id,
                device_id,
                direction,
                file_name,
                file_size,
                transferred_bytes,
                total_chunks,
                status,
                checksum,
                route,
                temp_path,
                final_path,
                error,
                created_at,
                updated_at
            )
            VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15)
            ON CONFLICT(file_id) DO UPDATE SET
                device_id = excluded.device_id,
                direction = excluded.direction,
                file_name = excluded.file_name,
                file_size = excluded.file_size,
                transferred_bytes = excluded.transferred_bytes,
                total_chunks = excluded.total_chunks,
                status = excluded.status,
                checksum = excluded.checksum,
                route = excluded.route,
                temp_path = excluded.temp_path,
                final_path = excluded.final_path,
                error = excluded.error,
                created_at = excluded.created_at,
                updated_at = excluded.updated_at
            ",
            params![
                transfer.file_id,
                transfer.device_id,
                transfer.direction,
                transfer.file_name,
                transfer.file_size,
                transfer.transferred_bytes,
                transfer.total_chunks,
                transfer.status,
                transfer.checksum,
                transfer.route,
                transfer.temp_path,
                transfer.final_path,
                transfer.error,
                transfer.created_at,
                transfer.updated_at
            ],
        )?;
        Ok(())
    }

    pub fn load_logs(&self, limit: usize) -> AppResult<Vec<AppLogEntry>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "
            SELECT
                id,
                level,
                source,
                message,
                created_at
            FROM app_logs
            ORDER BY created_at DESC
            LIMIT ?1
            ",
        )?;

        let rows = statement.query_map(params![limit as i64], |row| {
            Ok(AppLogEntry {
                id: row.get(0)?,
                level: row.get(1)?,
                source: row.get(2)?,
                message: row.get(3)?,
                created_at: row.get(4)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn append_log(&self, entry: &AppLogEntry) -> AppResult<()> {
        let connection = self.open()?;
        connection.execute(
            "
            INSERT INTO app_logs (id, level, source, message, created_at)
            VALUES (?1, ?2, ?3, ?4, ?5)
            ",
            params![
                entry.id,
                entry.level,
                entry.source,
                entry.message,
                entry.created_at
            ],
        )?;
        connection.execute(
            "
            DELETE FROM app_logs
            WHERE id NOT IN (
                SELECT id FROM app_logs
                ORDER BY created_at DESC
                LIMIT 300
            )
            ",
            [],
        )?;
        Ok(())
    }

    pub fn load_unfinished_transfers(&self) -> AppResult<Vec<FileTransferRecord>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "
            SELECT
                file_id,
                device_id,
                direction,
                file_name,
                file_size,
                transferred_bytes,
                total_chunks,
                status,
                checksum,
                route,
                temp_path,
                final_path,
                error,
                created_at,
                updated_at
            FROM file_transfers
            WHERE status IN ('pending', 'offered', 'sending', 'receiving')
            ORDER BY updated_at DESC
            ",
        )?;

        let rows = statement.query_map([], |row| {
            Ok(FileTransferRecord {
                file_id: row.get(0)?,
                device_id: row.get(1)?,
                direction: row.get(2)?,
                file_name: row.get(3)?,
                file_size: row.get(4)?,
                transferred_bytes: row.get(5)?,
                total_chunks: row.get(6)?,
                status: row.get(7)?,
                checksum: row.get(8)?,
                route: row.get(9)?,
                temp_path: row.get(10)?,
                final_path: row.get(11)?,
                error: row.get(12)?,
                created_at: row.get(13)?,
                updated_at: row.get(14)?,
            })
        })?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
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

#[cfg(test)]
mod tests {
    use std::fs;

    use uuid::Uuid;

    use super::Database;
    use crate::models::{AppLogEntry, AppSettings, FileTransferRecord, TextMessageRecord};

    #[test]
    fn persists_structured_records() {
        let path = std::env::temp_dir().join(format!("colink-db-{}.sqlite", Uuid::new_v4()));
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let settings = AppSettings::new("D:/downloads".to_string()).normalize();
        database.save_settings(&settings).expect("save settings");
        assert_eq!(
            database
                .load_settings()
                .expect("load settings")
                .expect("settings")
                .download_path,
            "D:/downloads"
        );

        database
            .save_message(&TextMessageRecord {
                message_id: "m1".to_string(),
                device_id: "d1".to_string(),
                direction: "outbound".to_string(),
                text: "hello".to_string(),
                route: "cloud".to_string(),
                created_at: 1,
            })
            .expect("save message");
        assert_eq!(database.load_messages(10).expect("messages").len(), 1);

        database
            .save_transfer(&FileTransferRecord {
                file_id: "f1".to_string(),
                device_id: "d1".to_string(),
                direction: "outbound".to_string(),
                file_name: "a.txt".to_string(),
                file_size: 12,
                transferred_bytes: 12,
                total_chunks: 1,
                status: "completed".to_string(),
                checksum: "sha256:test".to_string(),
                route: "cloud".to_string(),
                temp_path: None,
                final_path: Some("D:/downloads/a.txt".to_string()),
                error: None,
                created_at: 1,
                updated_at: 2,
            })
            .expect("save transfer");
        assert_eq!(database.load_transfers(10).expect("transfers").len(), 1);

        database
            .append_log(&AppLogEntry {
                id: "l1".to_string(),
                level: "info".to_string(),
                source: "test".to_string(),
                message: "ok".to_string(),
                created_at: 1,
            })
            .expect("save log");
        assert_eq!(database.load_logs(10).expect("logs").len(), 1);

        let _ = fs::remove_file(path);
    }
}
