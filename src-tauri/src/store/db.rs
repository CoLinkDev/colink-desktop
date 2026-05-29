use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::{Map, Value};

use crate::{
    error::{AppError, AppResult},
    models::{
        unix_now, AppLogEntry, AppSettings, DeviceIdentity, DeviceInfo, FileTransferRecord,
        LanTrustRecord, SessionRecord, TextMessageRecord,
    },
};

const SETTINGS_KEY: &str = "settings";
const SESSION_KEY: &str = "session";
const DEVICE_IDENTITY_KEY: &str = "device_identity";
const DEVICE_CACHE_KEY: &str = "device_cache";
const LAN_TRUST_KEY: &str = "lan_trust";
const MAX_LOG_ENTRIES: i64 = 300;

type MigrationFn = fn(&Transaction<'_>) -> AppResult<()>;

struct Migration {
    version: i64,
    name: &'static str,
    run: MigrationFn,
}

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        name: "baseline",
        run: migrate_1_baseline,
    },
    Migration {
        version: 2,
        name: "normalize_kv_records",
        run: migrate_2_normalize_kv_records,
    },
];

const BASELINE_SCHEMA_SQL: &str = "
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
";

#[derive(Clone)]
pub struct Database {
    path: PathBuf,
}

impl Database {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn initialize(&self) -> AppResult<()> {
        let mut connection = self.open()?;
        run_migrations(&mut connection)
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
        self.save_record(SETTINGS_KEY, &settings.clone().normalize())
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
        self.save_record(DEVICE_IDENTITY_KEY, &identity.clone().normalize())
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

    pub fn load_lan_trusts(&self) -> AppResult<Vec<LanTrustRecord>> {
        Ok(self.load_record(LAN_TRUST_KEY)?.unwrap_or_default())
    }

    pub fn save_lan_trusts(&self, records: &[LanTrustRecord]) -> AppResult<()> {
        self.save_record(LAN_TRUST_KEY, records)
    }

    pub fn upsert_lan_trust(&self, record: LanTrustRecord) -> AppResult<()> {
        let mut records = self.load_lan_trusts()?;
        if let Some(existing) = records
            .iter_mut()
            .find(|item| item.device_id == record.device_id)
        {
            *existing = record;
        } else {
            records.push(record);
        }
        self.save_lan_trusts(&records)
    }

    pub fn remove_lan_trust(&self, device_id: &str) -> AppResult<()> {
        let records = self
            .load_lan_trusts()?
            .into_iter()
            .filter(|item| item.device_id != device_id)
            .collect::<Vec<_>>();
        self.save_lan_trusts(&records)
    }

    pub fn ensure_lan_trusts_for_devices(
        &self,
        devices: &[DeviceInfo],
        local_device_id: Option<&str>,
    ) -> AppResult<()> {
        let mut records = self.load_lan_trusts()?;
        let now = unix_now();
        let mut changed = false;

        for device in devices {
            if local_device_id == Some(device.device_id.as_str()) {
                continue;
            }
            if device.public_key.trim().is_empty() {
                continue;
            }

            if let Some(record) = records
                .iter_mut()
                .find(|record| record.device_id == device.device_id)
            {
                if record.name != device.name || record.public_key != device.public_key {
                    record.name = device.name.clone();
                    record.public_key = device.public_key.clone();
                    record.trusted_at = now;
                    changed = true;
                }
            } else {
                records.push(LanTrustRecord {
                    device_id: device.device_id.clone(),
                    name: device.name.clone(),
                    public_key: device.public_key.clone(),
                    trusted_at: now,
                });
                changed = true;
            }
        }

        if changed {
            self.save_lan_trusts(&records)?;
        }

        Ok(())
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

        let rows = statement.query_map(params![limit as i64], map_file_transfer_row)?;

        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn clear_transfers(&self) -> AppResult<()> {
        let connection = self.open()?;
        connection.execute(
            "DELETE FROM file_transfers WHERE status NOT IN ('pending', 'offered', 'sending', 'receiving')",
            [],
        )?;
        Ok(())
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
                map_file_transfer_row,
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
                LIMIT ?1
            )
            ",
            params![MAX_LOG_ENTRIES],
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

        let rows = statement.query_map([], map_file_transfer_row)?;

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

fn map_file_transfer_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<FileTransferRecord> {
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
}

fn run_migrations(connection: &mut Connection) -> AppResult<()> {
    connection.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS schema_migrations (
            version INTEGER PRIMARY KEY,
            name TEXT NOT NULL,
            applied_at INTEGER NOT NULL
        );
        ",
    )?;

    let latest_applied = latest_applied_migration(connection)?;
    let latest_known = MIGRATIONS
        .last()
        .map(|migration| migration.version)
        .unwrap_or(0);
    if latest_applied > latest_known {
        return Err(AppError::message(format!(
            "database schema version {latest_applied} is newer than this application supports ({latest_known})"
        )));
    }

    for migration in MIGRATIONS
        .iter()
        .filter(|migration| migration.version > latest_applied)
    {
        let transaction = connection.transaction()?;
        (migration.run)(&transaction)?;
        transaction.execute(
            "
            INSERT INTO schema_migrations (version, name, applied_at)
            VALUES (?1, ?2, ?3)
            ",
            params![migration.version, migration.name, unix_now()],
        )?;
        transaction.commit()?;
    }

    Ok(())
}

fn latest_applied_migration(connection: &Connection) -> AppResult<i64> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(Into::into)
}

fn migrate_1_baseline(transaction: &Transaction<'_>) -> AppResult<()> {
    transaction.execute_batch(BASELINE_SCHEMA_SQL)?;
    Ok(())
}

fn migrate_2_normalize_kv_records(transaction: &Transaction<'_>) -> AppResult<()> {
    migrate_json_record(transaction, SETTINGS_KEY, normalize_settings_json)?;
    migrate_json_record(transaction, SESSION_KEY, normalize_session_json)?;
    migrate_json_record(
        transaction,
        DEVICE_IDENTITY_KEY,
        normalize_device_identity_json,
    )?;
    migrate_json_record(transaction, DEVICE_CACHE_KEY, normalize_device_cache_json)
}

fn migrate_json_record(
    transaction: &Transaction<'_>,
    key: &str,
    normalize: fn(Value) -> AppResult<Value>,
) -> AppResult<()> {
    let Some(raw) = transaction
        .query_row(
            "SELECT value FROM kv_store WHERE key = ?1",
            params![key],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(());
    };

    let value = normalize(serde_json::from_str(&raw)?)?;
    let json = serde_json::to_string(&value)?;
    transaction.execute(
        "
        UPDATE kv_store
        SET value = ?2, updated_at = ?3
        WHERE key = ?1
        ",
        params![key, json, unix_now()],
    )?;
    Ok(())
}

fn normalize_settings_json(value: Value) -> AppResult<Value> {
    let mut object = into_object(value, SETTINGS_KEY)?;
    trim_string_field(&mut object, "serverUrl")?;
    if let Some(server_url) = object.get("serverUrl").and_then(Value::as_str) {
        object.insert(
            "serverUrl".to_string(),
            Value::String(server_url.trim_end_matches('/').to_string()),
        );
    }
    trim_string_field(&mut object, "downloadPath")?;
    let language = match object.get("language") {
        Some(Value::String(language)) => crate::i18n::resolve_language(Some(language)).to_string(),
        Some(_) => return Err(AppError::message("language must be a string")),
        None => crate::i18n::default_language_code(),
    };
    object.insert("language".to_string(), Value::String(language));
    serde_json::from_value::<AppSettings>(Value::Object(object.clone()))?;
    Ok(Value::Object(object))
}

fn normalize_session_json(value: Value) -> AppResult<Value> {
    let mut object = into_object(value, SESSION_KEY)?;
    object
        .entry("username".to_string())
        .or_insert_with(|| Value::String(String::new()));
    serde_json::from_value::<SessionRecord>(Value::Object(object.clone()))?;
    Ok(Value::Object(object))
}

fn normalize_device_identity_json(value: Value) -> AppResult<Value> {
    let mut object = into_object(value, DEVICE_IDENTITY_KEY)?;
    normalize_optional_string_field(&mut object, "userId")?;
    normalize_optional_string_field(&mut object, "deviceSecret")?;
    trim_string_field(&mut object, "name")?;
    trim_string_field(&mut object, "deviceType")?;
    trim_string_field(&mut object, "publicKey")?;
    trim_string_field(&mut object, "privateKey")?;
    object
        .entry("cloudKeySyncPending".to_string())
        .or_insert(Value::Bool(false));
    serde_json::from_value::<DeviceIdentity>(Value::Object(object.clone()))?;
    Ok(Value::Object(object))
}

fn normalize_device_cache_json(value: Value) -> AppResult<Value> {
    let Value::Array(devices) = value else {
        return Err(AppError::message("device_cache must be a JSON array"));
    };

    let mut normalized = Vec::with_capacity(devices.len());
    for device in devices {
        let mut object = into_object(device, DEVICE_CACHE_KEY)?;
        let online = object
            .get("online")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        object.entry("lastSeen".to_string()).or_insert(Value::Null);
        object
            .entry("cloudAvailable".to_string())
            .or_insert(Value::Bool(online));
        object
            .entry("lanAvailable".to_string())
            .or_insert(Value::Bool(false));
        object
            .entry("activeRoute".to_string())
            .or_insert(Value::Null);
        object
            .entry("deviceSources".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        object
            .entry("securityState".to_string())
            .or_insert_with(|| Value::String("unverified".to_string()));
        normalized.push(Value::Object(object));
    }

    serde_json::from_value::<Vec<DeviceInfo>>(Value::Array(normalized.clone()))?;
    Ok(Value::Array(normalized))
}

fn into_object(value: Value, key: &str) -> AppResult<Map<String, Value>> {
    match value {
        Value::Object(object) => Ok(object),
        _ => Err(AppError::message(format!("{key} must be a JSON object"))),
    }
}

fn trim_string_field(object: &mut Map<String, Value>, field: &str) -> AppResult<()> {
    let Some(value) = object.get_mut(field) else {
        return Ok(());
    };
    let Some(raw) = value.as_str() else {
        return Err(AppError::message(format!("{field} must be a string")));
    };
    *value = Value::String(raw.trim().to_string());
    Ok(())
}

fn normalize_optional_string_field(object: &mut Map<String, Value>, field: &str) -> AppResult<()> {
    let Some(value) = object.get_mut(field) else {
        object.insert(field.to_string(), Value::Null);
        return Ok(());
    };
    if value.is_null() {
        return Ok(());
    }
    let Some(raw) = value.as_str() else {
        return Err(AppError::message(format!(
            "{field} must be a string or null"
        )));
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        *value = Value::Null;
    } else {
        *value = Value::String(trimmed.to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use rusqlite::{params, Connection};
    use uuid::Uuid;

    use super::Database;
    use crate::models::{
        AppLogEntry, AppSettings, DeviceIdentity, FileTransferRecord, TextMessageRecord,
    };

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

    #[test]
    fn save_settings_normalizes_at_storage_boundary() {
        let path = temp_db_path();
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        database
            .save_settings(&AppSettings {
                server_url: " http://127.0.0.1:8080/ ".to_string(),
                auto_start: true,
                start_minimized: true,
                lan_discovery: true,
                download_path: " D:/downloads ".to_string(),
                notifications: true,
                language: "unknown".to_string(),
            })
            .expect("save settings");

        let settings = database
            .load_settings()
            .expect("load settings")
            .expect("settings");

        assert_eq!(settings.server_url, "http://127.0.0.1:8080");
        assert_eq!(settings.download_path, "D:/downloads");
        assert_eq!(settings.language, crate::i18n::default_language_code());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn persists_local_only_device_identity() {
        let path = std::env::temp_dir().join(format!("colink-db-{}.sqlite", Uuid::new_v4()));
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        database
            .save_device_identity(&DeviceIdentity {
                user_id: None,
                device_id: "device-1".to_string(),
                device_secret: None,
                name: "Desktop".to_string(),
                device_type: "windows".to_string(),
                public_key: "pk".to_string(),
                private_key: "sk".to_string(),
                cloud_key_sync_pending: false,
            })
            .expect("save identity");

        let identity = database
            .load_device_identity()
            .expect("load identity")
            .expect("identity");

        assert_eq!(identity.user_id, None);
        assert_eq!(identity.device_secret, None);
        assert_eq!(identity.device_id, "device-1");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn migrates_legacy_database_and_preserves_records() {
        let path = temp_db_path();
        create_legacy_database(&path);

        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let settings = database
            .load_settings()
            .expect("load settings")
            .expect("settings");
        assert_eq!(settings.server_url, "http://127.0.0.1:8080");
        assert_eq!(settings.download_path, "D:/downloads");
        assert_eq!(settings.language, crate::i18n::default_language_code());

        let session = database
            .load_session()
            .expect("load session")
            .expect("session");
        assert_eq!(session.username, "");

        let identity = database
            .load_device_identity()
            .expect("load identity")
            .expect("identity");
        assert_eq!(identity.user_id.as_deref(), Some("user-1"));
        assert_eq!(identity.device_secret.as_deref(), Some("secret-1"));
        assert_eq!(identity.name, "Desktop");
        assert!(!identity.cloud_key_sync_pending);

        let devices = database.load_cached_devices().expect("load devices");
        assert_eq!(devices.len(), 1);
        assert!(devices[0].cloud_available);
        assert!(!devices[0].lan_available);
        assert_eq!(devices[0].active_route, None);
        assert_eq!(devices[0].device_sources, Vec::<String>::new());
        assert_eq!(devices[0].security_state, "unverified");

        let messages = database.load_messages(10).expect("load messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_id, "m1");

        assert_eq!(migration_versions(&path), vec![1, 2]);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn initialize_is_idempotent_after_migrations() {
        let path = temp_db_path();
        let database = Database::new(path.clone());

        database.initialize().expect("first init");
        database.initialize().expect("second init");

        assert_eq!(migration_versions(&path), vec![1, 2]);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn applies_incremental_migration_after_baseline() {
        let path = temp_db_path();
        create_legacy_database(&path);
        let connection = Connection::open(&path).expect("open legacy db");
        connection
            .execute_batch(
                "
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at INTEGER NOT NULL
                );
                INSERT INTO schema_migrations (version, name, applied_at)
                VALUES (1, 'baseline', 1);
                ",
            )
            .expect("seed migration");
        drop(connection);

        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let identity = database
            .load_device_identity()
            .expect("load identity")
            .expect("identity");
        assert!(!identity.cloud_key_sync_pending);
        assert_eq!(migration_versions(&path), vec![1, 2]);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rejects_database_newer_than_application() {
        let path = temp_db_path();
        let connection = Connection::open(&path).expect("open db");
        connection
            .execute_batch(
                "
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at INTEGER NOT NULL
                );
                INSERT INTO schema_migrations (version, name, applied_at)
                VALUES (99, 'future', 1);
                ",
            )
            .expect("seed future migration");
        drop(connection);

        let database = Database::new(path.clone());
        let error = database.initialize().expect_err("future db rejected");
        assert!(error
            .to_string()
            .contains("newer than this application supports"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn rolls_back_failed_migration() {
        let path = temp_db_path();
        create_legacy_database(&path);
        let connection = Connection::open(&path).expect("open legacy db");
        connection
            .execute(
                "UPDATE kv_store SET value = ?2 WHERE key = ?1",
                params!["session", r#"{"userId":"user-1"}"#],
            )
            .expect("corrupt session");
        drop(connection);

        let database = Database::new(path.clone());
        assert!(database.initialize().is_err());

        let connection = Connection::open(&path).expect("open failed db");
        let versions = connection
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .expect("prepare versions")
            .query_map([], |row| row.get::<_, i64>(0))
            .expect("query versions")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect versions");
        assert_eq!(versions, vec![1]);

        let raw_identity: String = connection
            .query_row(
                "SELECT value FROM kv_store WHERE key = ?1",
                params!["device_identity"],
                |row| row.get(0),
            )
            .expect("identity json");
        assert!(!raw_identity.contains("cloudKeySyncPending"));

        let _ = fs::remove_file(path);
    }

    fn temp_db_path() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("colink-db-{}.sqlite", Uuid::new_v4()))
    }

    fn create_legacy_database(path: &std::path::Path) {
        let connection = Connection::open(path).expect("open legacy db");
        connection
            .execute_batch(super::BASELINE_SCHEMA_SQL)
            .expect("create baseline schema");
        connection
            .execute(
                "
                INSERT INTO kv_store (key, value, updated_at)
                VALUES (?1, ?2, 1)
                ",
                params![
                    "settings",
                    r#"{
                        "serverUrl": " http://127.0.0.1:8080/ ",
                        "autoStart": true,
                        "startMinimized": true,
                        "lanDiscovery": true,
                        "downloadPath": " D:/downloads ",
                        "notifications": true
                    }"#
                ],
            )
            .expect("insert settings");
        connection
            .execute(
                "
                INSERT INTO kv_store (key, value, updated_at)
                VALUES (?1, ?2, 1)
                ",
                params![
                    "session",
                    r#"{
                        "userId": "user-1",
                        "accessToken": "access",
                        "refreshToken": "refresh",
                        "accessTokenExpiresAt": 123
                    }"#
                ],
            )
            .expect("insert session");
        connection
            .execute(
                "
                INSERT INTO kv_store (key, value, updated_at)
                VALUES (?1, ?2, 1)
                ",
                params![
                    "device_identity",
                    r#"{
                        "userId": " user-1 ",
                        "deviceId": "device-1",
                        "deviceSecret": " secret-1 ",
                        "name": " Desktop ",
                        "deviceType": " windows ",
                        "publicKey": " pk ",
                        "privateKey": " sk "
                    }"#
                ],
            )
            .expect("insert identity");
        connection
            .execute(
                "
                INSERT INTO kv_store (key, value, updated_at)
                VALUES (?1, ?2, 1)
                ",
                params![
                    "device_cache",
                    r#"[{
                        "deviceId": "device-2",
                        "name": "Peer",
                        "type": "windows",
                        "online": true,
                        "lastSeen": null,
                        "publicKey": "peer-pk"
                    }]"#
                ],
            )
            .expect("insert device cache");
        connection
            .execute(
                "
                INSERT INTO messages (
                    message_id,
                    device_id,
                    direction,
                    text,
                    route,
                    created_at
                )
                VALUES ('m1', 'device-2', 'inbound', 'hello', 'cloud', 1)
                ",
                [],
            )
            .expect("insert message");
    }

    fn migration_versions(path: &std::path::Path) -> Vec<i64> {
        let connection = Connection::open(path).expect("open db");
        let mut statement = connection
            .prepare("SELECT version FROM schema_migrations ORDER BY version")
            .expect("prepare versions");
        statement
            .query_map([], |row| row.get::<_, i64>(0))
            .expect("query versions")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect versions")
    }
}
