use std::path::PathBuf;

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::{
    error::{AppError, AppResult},
    models::{
        unix_now, unix_now_millis, AppLogEntry, AppSettings, DeviceIdentity, DeviceInfo,
        FileTransferRecord, MusicProviderConfig, SessionRecord, TextMessageRecord,
        TrustedPeerKeyRecord,
    },
    music::provider::known_provider,
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
    Migration {
        version: 3,
        name: "add_device_public_key_updated_at",
        run: migrate_3_add_device_public_key_updated_at,
    },
    Migration {
        version: 4,
        name: "move_trusted_peer_keys_to_table",
        run: migrate_4_move_trusted_peer_keys_to_table,
    },
    Migration {
        version: 5,
        name: "migrate_lan_trusts_table_to_trusted_peer_keys",
        run: migrate_5_migrate_lan_trusts_table_to_trusted_peer_keys,
    },
    Migration {
        version: 6,
        name: "rename_lan_trust_device_source",
        run: migrate_6_rename_lan_trust_device_source,
    },
    Migration {
        version: 7,
        name: "add_lan_paired_to_trusted_peer_keys",
        run: migrate_7_add_lan_paired_to_trusted_peer_keys,
    },
    Migration {
        version: 8,
        name: "add_clipboard_sync_setting",
        run: migrate_8_add_clipboard_sync_setting,
    },
    Migration {
        version: 9,
        name: "add_lan_state_to_device_cache",
        run: migrate_9_add_lan_state_to_device_cache,
    },
    Migration {
        version: 10,
        name: "split_trusted_peer_key_sources",
        run: migrate_10_split_trusted_peer_key_sources,
    },
    Migration {
        version: 11,
        name: "add_device_cache_trust_flags",
        run: migrate_11_add_device_cache_trust_flags,
    },
    Migration {
        version: 12,
        name: "add_device_cache_lan_endpoint",
        run: migrate_12_add_device_cache_lan_endpoint,
    },
    Migration {
        version: 13,
        name: "add_music_providers",
        run: migrate_13_add_music_providers,
    },
    Migration {
        version: 14,
        name: "drop_device_identity_secret",
        run: migrate_14_drop_device_identity_secret,
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

const TRUSTED_PEER_KEYS_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS trusted_peer_keys (
    device_id TEXT PRIMARY KEY,
    name TEXT NOT NULL,
    public_key TEXT NOT NULL,
    key_updated_at INTEGER NOT NULL,
    trusted_by_lan INTEGER NOT NULL DEFAULT 0,
    trusted_by_cloud INTEGER NOT NULL DEFAULT 0
);
";

const MUSIC_PROVIDERS_SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS music_providers (
    id TEXT PRIMARY KEY,
    enabled INTEGER NOT NULL DEFAULT 1,
    priority INTEGER NOT NULL DEFAULT 0
);
";

const TIMESTAMP_SECONDS_CUTOFF: i64 = 10_000_000_000;

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

    pub fn load_trusted_peer_keys(&self) -> AppResult<Vec<TrustedPeerKeyRecord>> {
        let connection = self.open()?;
        load_trusted_peer_keys_from_connection(&connection)
    }

    pub fn save_trusted_peer_keys(&self, records: &[TrustedPeerKeyRecord]) -> AppResult<()> {
        let mut connection = self.open()?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM trusted_peer_keys", [])?;
        for record in records {
            upsert_trusted_peer_key_row_in_transaction(&transaction, record)?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn load_music_providers(&self) -> AppResult<Vec<MusicProviderConfig>> {
        let connection = self.open()?;
        let mut statement = connection.prepare(
            "
            SELECT id, enabled, priority
            FROM music_providers
            ORDER BY priority ASC, id ASC
            ",
        )?;
        let rows = statement.query_map([], |row| {
            Ok(MusicProviderConfig {
                id: row.get(0)?,
                enabled: row.get::<_, bool>(1)?,
                priority: row.get(2)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
    }

    pub fn save_music_providers(&self, providers: &[MusicProviderConfig]) -> AppResult<()> {
        let providers = canonicalize_music_providers(providers);
        let mut connection = self.open()?;
        let transaction = connection.transaction()?;
        transaction.execute("DELETE FROM music_providers", [])?;
        for provider in providers {
            transaction.execute(
                "
                INSERT INTO music_providers (id, enabled, priority)
                VALUES (?1, ?2, ?3)
                ",
                params![provider.id, provider.enabled, provider.priority],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn upsert_trusted_peer_key(&self, record: TrustedPeerKeyRecord) -> AppResult<()> {
        let connection = self.open()?;
        upsert_trusted_peer_key_row(&connection, &record)
    }

    pub fn clear_lan_pairing(&self, device_id: &str) -> AppResult<()> {
        let connection = self.open()?;
        connection.execute(
            "UPDATE trusted_peer_keys SET trusted_by_lan = 0 WHERE device_id = ?1",
            params![device_id],
        )?;
        Ok(())
    }

    pub fn clear_cloud_trust(&self) -> AppResult<()> {
        let connection = self.open()?;
        connection.execute("UPDATE trusted_peer_keys SET trusted_by_cloud = 0", [])?;
        connection.execute(
            "DELETE FROM trusted_peer_keys WHERE trusted_by_lan = 0 AND trusted_by_cloud = 0",
            [],
        )?;
        Ok(())
    }

    pub fn ensure_trusted_peer_keys_for_devices(
        &self,
        devices: &[DeviceInfo],
        local_device_id: Option<&str>,
    ) -> AppResult<()> {
        let mut records = self.load_trusted_peer_keys()?;
        let now = unix_now_millis();
        let mut changed = false;
        let mut cloud_device_ids = Vec::new();

        for device in devices {
            if local_device_id == Some(device.device_id.as_str()) {
                continue;
            }
            if device.public_key.trim().is_empty() {
                continue;
            }
            cloud_device_ids.push(device.device_id.as_str());

            if let Some(record) = records
                .iter_mut()
                .find(|record| record.device_id == device.device_id)
            {
                let key_differs = record.public_key != device.public_key;
                let cloud_timestamp_newer = device
                    .public_key_updated_at
                    .is_some_and(|updated_at| updated_at > record.key_updated_at);
                let accept_cloud_key = key_differs && cloud_timestamp_newer;

                if accept_cloud_key {
                    record.public_key = device.public_key.clone();
                    record.key_updated_at = device
                        .public_key_updated_at
                        .unwrap_or(record.key_updated_at);
                    record.trusted_by_lan = false;
                    record.trusted_by_cloud = true;
                    changed = true;
                } else if !key_differs {
                    if let Some(updated_at) = device.public_key_updated_at {
                        if updated_at > record.key_updated_at {
                            record.key_updated_at = updated_at;
                            changed = true;
                        }
                    }
                    if !record.trusted_by_cloud {
                        record.trusted_by_cloud = true;
                        changed = true;
                    }
                } else if record.trusted_by_cloud {
                    record.trusted_by_cloud = false;
                    changed = true;
                }

                if record.name != device.name {
                    record.name = device.name.clone();
                    changed = true;
                }
            } else {
                records.push(TrustedPeerKeyRecord {
                    device_id: device.device_id.clone(),
                    name: device.name.clone(),
                    public_key: device.public_key.clone(),
                    key_updated_at: device.public_key_updated_at.unwrap_or(now),
                    trusted_by_lan: false,
                    trusted_by_cloud: true,
                });
                changed = true;
            }
        }

        for record in records.iter_mut() {
            if local_device_id == Some(record.device_id.as_str()) {
                continue;
            }
            if record.trusted_by_cloud && !cloud_device_ids.contains(&record.device_id.as_str()) {
                record.trusted_by_cloud = false;
                changed = true;
            }
        }

        if changed {
            self.save_trusted_peer_keys(&records)?;
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

fn load_trusted_peer_keys_from_connection(
    connection: &Connection,
) -> AppResult<Vec<TrustedPeerKeyRecord>> {
    let mut statement = connection.prepare(
        "
        SELECT device_id, name, public_key, key_updated_at, trusted_by_lan, trusted_by_cloud
        FROM trusted_peer_keys
        ORDER BY name COLLATE NOCASE ASC, device_id ASC
        ",
    )?;
    let rows = statement.query_map([], map_trusted_peer_key_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn load_trusted_peer_keys_from_transaction(
    transaction: &Transaction<'_>,
) -> AppResult<Vec<TrustedPeerKeyRecord>> {
    let mut statement = transaction.prepare(
        "
        SELECT device_id, name, public_key, key_updated_at, trusted_by_lan, trusted_by_cloud
        FROM trusted_peer_keys
        ORDER BY name COLLATE NOCASE ASC, device_id ASC
        ",
    )?;
    let rows = statement.query_map([], map_trusted_peer_key_row)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn map_trusted_peer_key_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<TrustedPeerKeyRecord> {
    Ok(TrustedPeerKeyRecord {
        device_id: row.get(0)?,
        name: row.get(1)?,
        public_key: row.get(2)?,
        key_updated_at: row.get(3)?,
        trusted_by_lan: row.get(4)?,
        trusted_by_cloud: row.get(5)?,
    })
}

fn upsert_trusted_peer_key_row(
    connection: &Connection,
    record: &TrustedPeerKeyRecord,
) -> AppResult<()> {
    connection.execute(
        "
        INSERT INTO trusted_peer_keys (device_id, name, public_key, key_updated_at, trusted_by_lan, trusted_by_cloud)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(device_id) DO UPDATE SET
            name = excluded.name,
            public_key = excluded.public_key,
            key_updated_at = excluded.key_updated_at,
            trusted_by_lan = excluded.trusted_by_lan,
            trusted_by_cloud = excluded.trusted_by_cloud
        ",
        params![
            record.device_id.trim(),
            record.name.trim(),
            record.public_key.trim(),
            normalize_timestamp_millis(record.key_updated_at),
            record.trusted_by_lan,
            record.trusted_by_cloud,
        ],
    )?;
    Ok(())
}

fn upsert_trusted_peer_key_row_in_transaction(
    transaction: &Transaction<'_>,
    record: &TrustedPeerKeyRecord,
) -> AppResult<()> {
    transaction.execute(
        "
        INSERT INTO trusted_peer_keys (device_id, name, public_key, key_updated_at, trusted_by_lan, trusted_by_cloud)
        VALUES (?1, ?2, ?3, ?4, ?5, ?6)
        ON CONFLICT(device_id) DO UPDATE SET
            name = excluded.name,
            public_key = excluded.public_key,
            key_updated_at = excluded.key_updated_at,
            trusted_by_lan = excluded.trusted_by_lan,
            trusted_by_cloud = excluded.trusted_by_cloud
        ",
        params![
            record.device_id.trim(),
            record.name.trim(),
            record.public_key.trim(),
            normalize_timestamp_millis(record.key_updated_at),
            record.trusted_by_lan,
            record.trusted_by_cloud,
        ],
    )?;
    Ok(())
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

fn table_exists(connection: &Transaction<'_>, table_name: &str) -> AppResult<bool> {
    let count: i64 = connection.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![table_name],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn table_has_column(
    connection: &Transaction<'_>,
    table_name: &str,
    column_name: &str,
) -> AppResult<bool> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table_name})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(columns.iter().any(|column| column == column_name))
}

fn migrate_1_baseline(transaction: &Transaction<'_>) -> AppResult<()> {
    transaction.execute_batch(BASELINE_SCHEMA_SQL)?;
    Ok(())
}

fn migrate_2_normalize_kv_records(transaction: &Transaction<'_>) -> AppResult<()> {
    migrate_json_record(
        transaction,
        SETTINGS_KEY,
        normalize_settings_before_clipboard_json,
    )?;
    migrate_json_record(transaction, SESSION_KEY, normalize_session_json)?;
    migrate_json_record(
        transaction,
        DEVICE_IDENTITY_KEY,
        normalize_device_identity_json,
    )?;
    migrate_json_record(transaction, DEVICE_CACHE_KEY, normalize_device_cache_json)
}

fn migrate_3_add_device_public_key_updated_at(transaction: &Transaction<'_>) -> AppResult<()> {
    migrate_json_record(transaction, DEVICE_CACHE_KEY, normalize_device_cache_json)
}

fn migrate_4_move_trusted_peer_keys_to_table(transaction: &Transaction<'_>) -> AppResult<()> {
    transaction.execute_batch(TRUSTED_PEER_KEYS_SCHEMA_SQL)?;

    let Some(raw) = transaction
        .query_row(
            "SELECT value FROM kv_store WHERE key = ?1",
            params![LAN_TRUST_KEY],
            |row| row.get::<_, String>(0),
        )
        .optional()?
    else {
        return Ok(());
    };

    let trusts = normalize_legacy_lan_trust_json(serde_json::from_str(&raw)?)?;
    for record in trusts {
        upsert_trusted_peer_key_row_in_transaction(transaction, &record)?;
    }

    transaction.execute(
        "DELETE FROM kv_store WHERE key = ?1",
        params![LAN_TRUST_KEY],
    )?;
    let _ = load_trusted_peer_keys_from_transaction(transaction)?;
    Ok(())
}

fn migrate_5_migrate_lan_trusts_table_to_trusted_peer_keys(
    transaction: &Transaction<'_>,
) -> AppResult<()> {
    transaction.execute_batch(TRUSTED_PEER_KEYS_SCHEMA_SQL)?;
    if !table_exists(transaction, "lan_trusts")? {
        return Ok(());
    }

    let mut statement = transaction.prepare(
        "
        SELECT device_id, name, public_key, trusted_at
        FROM lan_trusts
        ORDER BY name COLLATE NOCASE ASC, device_id ASC
        ",
    )?;
    let records = statement
        .query_map([], |row| {
            let trusted_at = normalize_timestamp_millis(row.get::<_, i64>(3)?);
            Ok(TrustedPeerKeyRecord {
                device_id: row.get(0)?,
                name: row.get(1)?,
                public_key: row.get(2)?,
                key_updated_at: trusted_at,
                trusted_by_lan: true,
                trusted_by_cloud: false,
            })
        })?
        .collect::<Result<Vec<_>, _>>()?;
    drop(statement);

    for record in records {
        upsert_trusted_peer_key_row_in_transaction(transaction, &record)?;
    }
    transaction.execute("DROP TABLE lan_trusts", [])?;
    Ok(())
}

fn migrate_6_rename_lan_trust_device_source(transaction: &Transaction<'_>) -> AppResult<()> {
    migrate_json_record(transaction, DEVICE_CACHE_KEY, normalize_device_cache_json)
}

fn migrate_7_add_lan_paired_to_trusted_peer_keys(transaction: &Transaction<'_>) -> AppResult<()> {
    transaction.execute_batch(TRUSTED_PEER_KEYS_SCHEMA_SQL)?;
    let has_trusted_at = table_has_column(transaction, "trusted_peer_keys", "trusted_at")?;
    if has_trusted_at && !table_has_column(transaction, "trusted_peer_keys", "lan_paired")? {
        transaction.execute(
            "ALTER TABLE trusted_peer_keys ADD COLUMN lan_paired INTEGER NOT NULL DEFAULT 0",
            [],
        )?;
    }
    if has_trusted_at {
        transaction.execute(
            "UPDATE trusted_peer_keys SET lan_paired = 1 WHERE trusted_at IS NOT NULL",
            [],
        )?;
    }
    Ok(())
}

fn migrate_8_add_clipboard_sync_setting(transaction: &Transaction<'_>) -> AppResult<()> {
    migrate_json_record(transaction, SETTINGS_KEY, normalize_current_settings_json)
}

fn migrate_9_add_lan_state_to_device_cache(transaction: &Transaction<'_>) -> AppResult<()> {
    migrate_json_record(transaction, DEVICE_CACHE_KEY, normalize_device_cache_json)
}

fn migrate_10_split_trusted_peer_key_sources(transaction: &Transaction<'_>) -> AppResult<()> {
    transaction.execute_batch(TRUSTED_PEER_KEYS_SCHEMA_SQL)?;
    let has_trusted_by_lan = table_has_column(transaction, "trusted_peer_keys", "trusted_by_lan")?;
    let has_trusted_by_cloud =
        table_has_column(transaction, "trusted_peer_keys", "trusted_by_cloud")?;
    let has_lan_paired = table_has_column(transaction, "trusted_peer_keys", "lan_paired")?;
    let has_trusted_at = table_has_column(transaction, "trusted_peer_keys", "trusted_at")?;

    let trusted_by_lan_expr = if has_trusted_by_lan {
        "trusted_by_lan"
    } else if has_lan_paired {
        "lan_paired"
    } else if has_trusted_at {
        "CASE WHEN trusted_at IS NOT NULL THEN 1 ELSE 0 END"
    } else {
        "0"
    };
    let trusted_by_cloud_expr = if has_trusted_by_cloud {
        "trusted_by_cloud"
    } else {
        "0"
    };

    transaction.execute(
        "ALTER TABLE trusted_peer_keys RENAME TO trusted_peer_keys_old",
        [],
    )?;
    transaction.execute_batch(TRUSTED_PEER_KEYS_SCHEMA_SQL)?;
    transaction.execute(
        &format!(
            "
            INSERT INTO trusted_peer_keys (
                device_id,
                name,
                public_key,
                key_updated_at,
                trusted_by_lan,
                trusted_by_cloud
            )
            SELECT
                device_id,
                name,
                public_key,
                key_updated_at,
                {trusted_by_lan_expr},
                {trusted_by_cloud_expr}
            FROM trusted_peer_keys_old
            "
        ),
        [],
    )?;
    transaction.execute("DROP TABLE trusted_peer_keys_old", [])?;
    let _ = load_trusted_peer_keys_from_transaction(transaction)?;
    Ok(())
}

fn migrate_11_add_device_cache_trust_flags(transaction: &Transaction<'_>) -> AppResult<()> {
    migrate_json_record(transaction, DEVICE_CACHE_KEY, normalize_device_cache_json)
}

fn migrate_12_add_device_cache_lan_endpoint(transaction: &Transaction<'_>) -> AppResult<()> {
    migrate_json_record(transaction, DEVICE_CACHE_KEY, normalize_device_cache_json)
}

fn migrate_13_add_music_providers(transaction: &Transaction<'_>) -> AppResult<()> {
    transaction.execute_batch(MUSIC_PROVIDERS_SCHEMA_SQL)?;
    transaction.execute(
        "
        INSERT INTO music_providers (id, enabled, priority)
        VALUES ('qqmusic', 1, 0), ('ncm', 1, 1)
        ON CONFLICT(id) DO NOTHING
        ",
        [],
    )?;
    Ok(())
}

fn migrate_14_drop_device_identity_secret(transaction: &Transaction<'_>) -> AppResult<()> {
    migrate_json_record(
        transaction,
        DEVICE_IDENTITY_KEY,
        normalize_device_identity_json,
    )
}

fn canonicalize_music_providers(providers: &[MusicProviderConfig]) -> Vec<MusicProviderConfig> {
    let mut normalized = Vec::new();
    for provider in providers {
        let id = provider.id.trim();
        let Some(meta) = known_provider(id) else {
            continue;
        };
        if normalized
            .iter()
            .any(|item: &MusicProviderConfig| item.id == meta.id)
        {
            continue;
        }
        normalized.push(MusicProviderConfig {
            id: meta.id.to_string(),
            enabled: provider.enabled && meta.implemented,
            priority: normalized.len() as i32,
        });
    }
    normalized
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

fn normalize_settings_before_clipboard_json(value: Value) -> AppResult<Value> {
    let object = normalize_settings_base_json(value)?;
    serde_json::from_value::<AppSettingsBeforeClipboard>(Value::Object(object.clone()))?;
    Ok(Value::Object(object))
}

fn normalize_current_settings_json(value: Value) -> AppResult<Value> {
    let mut object = normalize_settings_base_json(value)?;
    object
        .entry("clipboardSync".to_string())
        .or_insert(Value::Bool(true));
    serde_json::from_value::<AppSettings>(Value::Object(object.clone()))?;
    Ok(Value::Object(object))
}

fn normalize_settings_base_json(value: Value) -> AppResult<Map<String, Value>> {
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
    Ok(object)
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AppSettingsBeforeClipboard {
    server_url: String,
    auto_start: bool,
    start_minimized: bool,
    lan_discovery: bool,
    download_path: String,
    notifications: bool,
    language: String,
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
    object.remove("deviceSecret");
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
        if !object.contains_key("lanState") {
            let lan_state = if object
                .get("lanAvailable")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                "alive"
            } else {
                "unavailable"
            };
            object.insert("lanState".to_string(), Value::String(lan_state.to_string()));
        }
        normalize_lan_state_json(&mut object)?;
        object
            .entry("activeRoute".to_string())
            .or_insert(Value::Null);
        object
            .entry("deviceSources".to_string())
            .or_insert_with(|| Value::Array(Vec::new()));
        object
            .entry("trustedByLan".to_string())
            .or_insert(Value::Bool(false));
        object
            .entry("trustedByCloud".to_string())
            .or_insert(Value::Bool(false));
        normalize_device_sources_json(&mut object)?;
        object
            .entry("securityState".to_string())
            .or_insert_with(|| Value::String("unverified".to_string()));
        object
            .entry("publicKeyUpdatedAt".to_string())
            .or_insert(Value::Null);
        object.entry("localIp".to_string()).or_insert(Value::Null);
        object.entry("localPort".to_string()).or_insert(Value::Null);
        normalized.push(Value::Object(object));
    }

    serde_json::from_value::<Vec<DeviceInfo>>(Value::Array(normalized.clone()))?;
    Ok(Value::Array(normalized))
}

fn normalize_lan_state_json(object: &mut Map<String, Value>) -> AppResult<()> {
    let state = object
        .get("lanState")
        .and_then(Value::as_str)
        .unwrap_or("unavailable")
        .trim();
    let normalized = match state {
        "alive" | "suspect" => state,
        "unavailable" | "" => "unavailable",
        _ => {
            return Err(AppError::message(
                "lanState must be alive, suspect, or unavailable",
            ))
        }
    };
    object.insert(
        "lanState".to_string(),
        Value::String(normalized.to_string()),
    );
    Ok(())
}

fn normalize_device_sources_json(object: &mut Map<String, Value>) -> AppResult<()> {
    let Some(value) = object.get_mut("deviceSources") else {
        return Ok(());
    };
    let Value::Array(sources) = value else {
        return Err(AppError::message("deviceSources must be a JSON array"));
    };
    for source in sources {
        let Some(raw) = source.as_str() else {
            return Err(AppError::message("deviceSources items must be strings"));
        };
        if raw == "lan_trust" {
            *source = Value::String("trusted_peer_key".to_string());
        }
    }
    Ok(())
}

fn normalize_legacy_lan_trust_json(value: Value) -> AppResult<Vec<TrustedPeerKeyRecord>> {
    let Value::Array(records) = value else {
        return Err(AppError::message("lan_trust must be a JSON array"));
    };

    let mut normalized = Vec::with_capacity(records.len());
    for record in records {
        let mut object = into_object(record, LAN_TRUST_KEY)?;
        trim_string_field(&mut object, "deviceId")?;
        trim_string_field(&mut object, "name")?;
        trim_string_field(&mut object, "publicKey")?;
        let trusted_at = object
            .get("trustedAt")
            .and_then(Value::as_i64)
            .ok_or_else(|| AppError::message("trustedAt must be an integer"))?;
        let timestamp = normalize_timestamp_millis(trusted_at);
        object.insert("keyUpdatedAt".to_string(), Value::Number(timestamp.into()));
        object.insert("trustedByLan".to_string(), Value::Bool(true));
        object.insert("trustedByCloud".to_string(), Value::Bool(false));
        object.remove("trustedAt");
        normalized.push(Value::Object(object));
    }

    Ok(serde_json::from_value::<Vec<TrustedPeerKeyRecord>>(
        Value::Array(normalized),
    )?)
}

fn normalize_timestamp_millis(value: i64) -> i64 {
    if value > 0 && value < TIMESTAMP_SECONDS_CUTOFF {
        value * 1000
    } else {
        value
    }
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
        AppLogEntry, AppSettings, DeviceIdentity, DeviceInfo, FileTransferRecord,
        MusicProviderConfig, TextMessageRecord, TrustedPeerKeyRecord,
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
                clipboard_sync: true,
                language: "unknown".to_string(),
            })
            .expect("save settings");

        let settings = database
            .load_settings()
            .expect("load settings")
            .expect("settings");

        assert_eq!(settings.server_url, "http://127.0.0.1:8080");
        assert_eq!(settings.download_path, "D:/downloads");
        assert!(settings.clipboard_sync);
        assert_eq!(settings.language, crate::i18n::default_language_code());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn save_music_providers_canonicalizes_at_storage_boundary() {
        let path = temp_db_path();
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        database
            .save_music_providers(&[
                MusicProviderConfig {
                    id: "unknown".to_string(),
                    enabled: true,
                    priority: 99,
                },
                MusicProviderConfig {
                    id: " ncm ".to_string(),
                    enabled: true,
                    priority: 42,
                },
                MusicProviderConfig {
                    id: "qqmusic".to_string(),
                    enabled: false,
                    priority: 7,
                },
                MusicProviderConfig {
                    id: "ncm".to_string(),
                    enabled: false,
                    priority: 0,
                },
            ])
            .expect("save music providers");

        let providers = database
            .load_music_providers()
            .expect("load music providers");
        assert_eq!(providers.len(), 2);
        assert_eq!(providers[0].id, "ncm");
        assert!(providers[0].enabled);
        assert_eq!(providers[0].priority, 0);
        assert_eq!(providers[1].id, "qqmusic");
        assert!(!providers[1].enabled);
        assert_eq!(providers[1].priority, 1);

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
        assert!(settings.clipboard_sync);
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
        assert_eq!(identity.name, "Desktop");
        assert!(!identity.cloud_key_sync_pending);

        let devices = database.load_cached_devices().expect("load devices");
        assert_eq!(devices.len(), 1);
        assert!(devices[0].cloud_available);
        assert!(!devices[0].lan_available);
        assert_eq!(devices[0].active_route, None);
        assert_eq!(devices[0].device_sources, Vec::<String>::new());
        assert_eq!(devices[0].security_state, "unverified");
        assert_eq!(devices[0].public_key_updated_at, None);

        let messages = database.load_messages(10).expect("load messages");
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].message_id, "m1");

        let music_providers = database
            .load_music_providers()
            .expect("load music providers");
        assert_eq!(music_providers.len(), 2);
        assert_eq!(music_providers[0].id, "qqmusic");
        assert!(music_providers[0].enabled);
        assert_eq!(music_providers[0].priority, 0);
        assert_eq!(music_providers[1].id, "ncm");
        assert!(music_providers[1].enabled);
        assert_eq!(music_providers[1].priority, 1);

        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn initialize_is_idempotent_after_migrations() {
        let path = temp_db_path();
        let database = Database::new(path.clone());

        database.initialize().expect("first init");
        database.initialize().expect("second init");

        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn applies_music_provider_migration_after_v12() {
        let path = temp_db_path();
        create_legacy_database(&path);
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let connection = Connection::open(&path).expect("open db");
        connection
            .execute(
                "DELETE FROM schema_migrations WHERE version IN (13, 14)",
                [],
            )
            .expect("remove v13 marker");
        connection
            .execute("DROP TABLE music_providers", [])
            .expect("drop music providers");
        drop(connection);

        let database = Database::new(path.clone());
        database.initialize().expect("rerun db init");

        let providers = database
            .load_music_providers()
            .expect("load music providers");
        assert_eq!(
            providers
                .iter()
                .map(|provider| (provider.id.as_str(), provider.enabled, provider.priority))
                .collect::<Vec<_>>(),
            vec![("qqmusic", true, 0), ("ncm", true, 1)]
        );
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

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
        let settings = database
            .load_settings()
            .expect("load settings")
            .expect("settings");
        assert!(settings.clipboard_sync);
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn applies_public_key_timestamp_migration_after_normalized_records() {
        let path = temp_db_path();
        create_legacy_database(&path);
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let connection = Connection::open(&path).expect("open db");
        connection
            .execute(
                "DELETE FROM schema_migrations WHERE version IN (3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14)",
                [],
            )
            .expect("remove v3 marker");
        connection
            .execute(
                "UPDATE kv_store SET value = ?2 WHERE key = ?1",
                params![
                    "device_cache",
                    r#"[{
                        "deviceId": "device-2",
                        "name": "Peer",
                        "type": "windows",
                        "online": true,
                        "lastSeen": null,
                        "publicKey": "peer-pk",
                        "cloudAvailable": true,
                        "lanAvailable": false,
                        "activeRoute": null,
                        "deviceSources": [],
                        "securityState": "unverified"
                    }]"#
                ],
            )
            .expect("seed v2 cache");
        drop(connection);

        let database = Database::new(path.clone());
        database.initialize().expect("rerun db init");

        let devices = database.load_cached_devices().expect("load devices");
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].public_key_updated_at, None);
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn migrates_lan_trust_json_to_table_and_normalizes_seconds() {
        let path = temp_db_path();
        create_legacy_database(&path);
        let connection = Connection::open(&path).expect("open legacy db");
        connection
            .execute(
                "
                INSERT INTO kv_store (key, value, updated_at)
                VALUES (?1, ?2, 1)
                ",
                params![
                    "lan_trust",
                    r#"[
                        {
                            "deviceId": "peer-ms",
                            "name": "Peer Millis",
                            "publicKey": "pk-ms",
                            "trustedAt": 1780077510545
                        },
                        {
                            "deviceId": "peer-sec",
                            "name": "Peer Seconds",
                            "publicKey": "pk-sec",
                            "trustedAt": 1780045153
                        }
                    ]"#
                ],
            )
            .expect("insert legacy lan trust");
        drop(connection);

        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let trusts = database.load_trusted_peer_keys().expect("load trusts");
        assert_eq!(trusts.len(), 2);
        let millis = trusts
            .iter()
            .find(|record| record.device_id == "peer-ms")
            .expect("millis trust");
        assert_eq!(millis.key_updated_at, 1_780_077_510_545);
        assert!(millis.trusted_by_lan);
        assert!(!millis.trusted_by_cloud);
        let seconds = trusts
            .iter()
            .find(|record| record.device_id == "peer-sec")
            .expect("seconds trust");
        assert_eq!(seconds.key_updated_at, 1_780_045_153_000);
        assert!(seconds.trusted_by_lan);
        assert!(!seconds.trusted_by_cloud);

        let connection = Connection::open(&path).expect("open migrated db");
        let legacy_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM kv_store WHERE key = ?1",
                params!["lan_trust"],
                |row| row.get(0),
            )
            .expect("count legacy lan trust");
        assert_eq!(legacy_count, 0);
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn migrates_existing_lan_trusts_table_to_trusted_peer_keys() {
        let path = temp_db_path();
        create_legacy_database(&path);
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let connection = Connection::open(&path).expect("open db");
        connection
            .execute(
                "DELETE FROM schema_migrations WHERE version IN (5, 6, 7, 8, 9, 10, 11, 12, 13, 14)",
                [],
            )
            .expect("remove v5 marker");
        connection
            .execute_batch(
                "
                CREATE TABLE lan_trusts (
                    device_id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    public_key TEXT NOT NULL,
                    trusted_at INTEGER NOT NULL
                );
                INSERT INTO lan_trusts (device_id, name, public_key, trusted_at)
                VALUES ('peer-old-table', 'Old Table Peer', 'pk-old-table', 1780045153);
                ",
            )
            .expect("seed old lan_trusts table");
        drop(connection);

        let database = Database::new(path.clone());
        database.initialize().expect("rerun db init");

        let trusts = database.load_trusted_peer_keys().expect("load trusts");
        let record = trusts
            .iter()
            .find(|record| record.device_id == "peer-old-table")
            .expect("migrated old table trust");
        assert_eq!(record.key_updated_at, 1_780_045_153_000);
        assert!(record.trusted_by_lan);
        assert!(!record.trusted_by_cloud);

        let connection = Connection::open(&path).expect("open migrated db");
        let old_table_exists: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = 'lan_trusts'",
                [],
                |row| row.get(0),
            )
            .expect("check old table");
        assert_eq!(old_table_exists, 0);
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn migrates_lan_trust_device_source_to_trusted_peer_key() {
        let path = temp_db_path();
        create_legacy_database(&path);
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let connection = Connection::open(&path).expect("open db");
        connection
            .execute(
                "DELETE FROM schema_migrations WHERE version IN (6, 7, 8, 9, 10, 11, 12, 13, 14)",
                [],
            )
            .expect("remove v6 marker");
        connection
            .execute(
                "UPDATE kv_store SET value = ?2 WHERE key = ?1",
                params![
                    "device_cache",
                    r#"[{
                        "deviceId": "device-2",
                        "name": "Peer",
                        "type": "windows",
                        "online": false,
                        "lastSeen": null,
                        "publicKey": "peer-pk",
                        "publicKeyUpdatedAt": null,
                        "cloudAvailable": false,
                        "lanAvailable": false,
                        "activeRoute": null,
                        "deviceSources": ["lan_trust"],
                        "securityState": "verified"
                    }]"#
                ],
            )
            .expect("seed old source");
        drop(connection);

        let database = Database::new(path.clone());
        database.initialize().expect("rerun db init");

        let devices = database.load_cached_devices().expect("load devices");
        assert_eq!(
            devices[0].device_sources,
            vec!["trusted_peer_key".to_string()]
        );
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn migrates_lan_paired_column_from_trusted_at() {
        let path = temp_db_path();
        let connection = Connection::open(&path).expect("open db");
        connection
            .execute_batch(super::BASELINE_SCHEMA_SQL)
            .expect("create baseline schema");
        connection
            .execute_batch(
                "
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at INTEGER NOT NULL
                );
                INSERT INTO schema_migrations (version, name, applied_at)
                VALUES
                    (1, 'baseline', 1),
                    (2, 'normalize_kv_records', 1),
                    (3, 'add_device_public_key_updated_at', 1),
                    (4, 'move_trusted_peer_keys_to_table', 1),
                    (5, 'migrate_lan_trusts_table_to_trusted_peer_keys', 1),
                    (6, 'rename_lan_trust_device_source', 1);
                DROP TABLE IF EXISTS trusted_peer_keys;
                CREATE TABLE trusted_peer_keys (
                    device_id TEXT PRIMARY KEY,
                    name TEXT NOT NULL,
                    public_key TEXT NOT NULL,
                    key_updated_at INTEGER NOT NULL,
                    trusted_at INTEGER
                );
                INSERT INTO trusted_peer_keys (device_id, name, public_key, key_updated_at, trusted_at)
                VALUES
                    ('paired', 'Paired', 'pk-paired', 2000, 2000),
                    ('cloud', 'Cloud', 'pk-cloud', 3000, NULL);
                ",
            )
            .expect("seed v6 db");
        drop(connection);

        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let trusts = database.load_trusted_peer_keys().expect("load trusts");
        let paired = trusts
            .iter()
            .find(|record| record.device_id == "paired")
            .expect("paired record");
        let cloud = trusts
            .iter()
            .find(|record| record.device_id == "cloud")
            .expect("cloud record");
        assert!(paired.trusted_by_lan);
        assert!(!paired.trusted_by_cloud);
        assert!(!cloud.trusted_by_lan);
        assert!(!cloud.trusted_by_cloud);
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn migrates_clipboard_sync_setting_from_v7() {
        let path = temp_db_path();
        let connection = Connection::open(&path).expect("open db");
        connection
            .execute_batch(super::BASELINE_SCHEMA_SQL)
            .expect("create baseline schema");
        connection
            .execute_batch(super::TRUSTED_PEER_KEYS_SCHEMA_SQL)
            .expect("create trusted peer schema");
        connection
            .execute_batch(
                "
                CREATE TABLE schema_migrations (
                    version INTEGER PRIMARY KEY,
                    name TEXT NOT NULL,
                    applied_at INTEGER NOT NULL
                );
                INSERT INTO schema_migrations (version, name, applied_at)
                VALUES
                    (1, 'baseline', 1),
                    (2, 'normalize_kv_records', 1),
                    (3, 'add_device_public_key_updated_at', 1),
                    (4, 'move_trusted_peer_keys_to_table', 1),
                    (5, 'migrate_lan_trusts_table_to_trusted_peer_keys', 1),
                    (6, 'rename_lan_trust_device_source', 1),
                    (7, 'add_lan_paired_to_trusted_peer_keys', 1);
                ",
            )
            .expect("seed v7 migrations");
        connection
            .execute(
                "
                INSERT INTO kv_store (key, value, updated_at)
                VALUES (?1, ?2, 1)
                ",
                params![
                    "settings",
                    r#"{
                        "serverUrl": "http://127.0.0.1:8080",
                        "autoStart": true,
                        "startMinimized": true,
                        "lanDiscovery": true,
                        "downloadPath": "D:/downloads",
                        "notifications": true,
                        "language": "en"
                    }"#
                ],
            )
            .expect("seed settings");
        drop(connection);

        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let settings = database
            .load_settings()
            .expect("load settings")
            .expect("settings");
        assert!(settings.clipboard_sync);
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn preserves_existing_clipboard_sync_setting() {
        let path = temp_db_path();
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let mut settings = AppSettings::new("D:/downloads".to_string()).normalize();
        settings.clipboard_sync = false;
        database.save_settings(&settings).expect("save settings");
        database.initialize().expect("rerun db init");

        let settings = database
            .load_settings()
            .expect("load settings")
            .expect("settings");
        assert!(!settings.clipboard_sync);
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

        let _ = fs::remove_file(path);
    }

    #[test]
    fn migrates_lan_state_from_lan_available() {
        let path = temp_db_path();
        create_legacy_database(&path);
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        let connection = Connection::open(&path).expect("open db");
        connection
            .execute(
                "DELETE FROM schema_migrations WHERE version IN (9, 10, 11, 12, 13, 14)",
                [],
            )
            .expect("remove v9 marker");
        connection
            .execute(
                "UPDATE kv_store SET value = ?2 WHERE key = ?1",
                params![
                    "device_cache",
                    r#"[{
                        "deviceId": "device-2",
                        "name": "Peer",
                        "type": "windows",
                        "online": true,
                        "lastSeen": null,
                        "publicKey": "peer-pk",
                        "publicKeyUpdatedAt": null,
                        "cloudAvailable": false,
                        "lanAvailable": true,
                        "activeRoute": "lan",
                        "deviceSources": ["trusted_peer_key"],
                        "securityState": "verified"
                    }]"#
                ],
            )
            .expect("seed v8 cache");
        drop(connection);

        let database = Database::new(path.clone());
        database.initialize().expect("rerun db init");

        let devices = database.load_cached_devices().expect("load devices");
        assert_eq!(devices[0].lan_state, "alive");
        assert_eq!(
            migration_versions(&path),
            vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14]
        );

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

        let raw_cache: String = connection
            .query_row(
                "SELECT value FROM kv_store WHERE key = ?1",
                params!["device_cache"],
                |row| row.get(0),
            )
            .expect("device cache json");
        assert!(!raw_cache.contains("publicKeyUpdatedAt"));

        let _ = fs::remove_file(path);
    }

    #[test]
    fn ensure_trusted_peer_keys_preserves_newer_local_pairing() {
        let path = temp_db_path();
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        database
            .save_trusted_peer_keys(&[TrustedPeerKeyRecord {
                device_id: "peer-1".to_string(),
                name: "old".to_string(),
                public_key: "local-new".to_string(),
                key_updated_at: 2_000_000_000_000,
                trusted_by_lan: true,
                trusted_by_cloud: false,
            }])
            .expect("save trust");

        database
            .ensure_trusted_peer_keys_for_devices(
                &[DeviceInfo {
                    device_id: "peer-1".to_string(),
                    name: "cloud-name".to_string(),
                    device_type: "windows".to_string(),
                    online: true,
                    cloud_available: true,
                    last_seen: None,
                    public_key: "cloud-old".to_string(),
                    public_key_updated_at: Some(1_000_000_000_000),
                    local_ip: None,
                    local_port: None,
                    lan_available: false,
                    lan_state: "unavailable".to_string(),
                    active_route: None,
                    device_sources: vec!["cloud".to_string()],
                    trusted_by_lan: false,
                    trusted_by_cloud: false,
                    security_state: "unverified".to_string(),
                }],
                None,
            )
            .expect("ensure trusts");

        let trusts = database.load_trusted_peer_keys().expect("load trusts");
        assert_eq!(trusts.len(), 1);
        assert_eq!(trusts[0].name, "cloud-name");
        assert_eq!(trusts[0].public_key, "local-new");
        assert_eq!(trusts[0].key_updated_at, 2_000_000_000_000);
        assert!(trusts[0].trusted_by_lan);
        assert!(!trusts[0].trusted_by_cloud);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn ensure_trusted_peer_keys_accepts_newer_cloud_key() {
        let path = temp_db_path();
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        database
            .save_trusted_peer_keys(&[TrustedPeerKeyRecord {
                device_id: "peer-1".to_string(),
                name: "old".to_string(),
                public_key: "local-old".to_string(),
                key_updated_at: 1_000_000_000_000,
                trusted_by_lan: true,
                trusted_by_cloud: false,
            }])
            .expect("save trust");

        database
            .ensure_trusted_peer_keys_for_devices(
                &[DeviceInfo {
                    device_id: "peer-1".to_string(),
                    name: "cloud-name".to_string(),
                    device_type: "windows".to_string(),
                    online: true,
                    cloud_available: true,
                    last_seen: None,
                    public_key: "cloud-new".to_string(),
                    public_key_updated_at: Some(2_000_000_000_000),
                    local_ip: None,
                    local_port: None,
                    lan_available: false,
                    lan_state: "unavailable".to_string(),
                    active_route: None,
                    device_sources: vec!["cloud".to_string()],
                    trusted_by_lan: false,
                    trusted_by_cloud: false,
                    security_state: "unverified".to_string(),
                }],
                None,
            )
            .expect("ensure trusts");

        let trusts = database.load_trusted_peer_keys().expect("load trusts");
        assert_eq!(trusts.len(), 1);
        assert_eq!(trusts[0].name, "cloud-name");
        assert_eq!(trusts[0].public_key, "cloud-new");
        assert_eq!(trusts[0].key_updated_at, 2_000_000_000_000);
        assert!(!trusts[0].trusted_by_lan);
        assert!(trusts[0].trusted_by_cloud);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn ensure_trusted_peer_keys_skips_key_overwrite_without_timestamp() {
        let path = temp_db_path();
        let database = Database::new(path.clone());
        database.initialize().expect("db init");

        database
            .save_trusted_peer_keys(&[TrustedPeerKeyRecord {
                device_id: "peer-1".to_string(),
                name: "old".to_string(),
                public_key: "local-new".to_string(),
                key_updated_at: 2_000_000_000_000,
                trusted_by_lan: true,
                trusted_by_cloud: false,
            }])
            .expect("save trust");

        database
            .ensure_trusted_peer_keys_for_devices(
                &[DeviceInfo {
                    device_id: "peer-1".to_string(),
                    name: "cloud-name".to_string(),
                    device_type: "windows".to_string(),
                    online: true,
                    cloud_available: true,
                    last_seen: None,
                    public_key: "cloud-unknown".to_string(),
                    public_key_updated_at: None,
                    local_ip: None,
                    local_port: None,
                    lan_available: false,
                    lan_state: "unavailable".to_string(),
                    active_route: None,
                    device_sources: vec!["cloud".to_string()],
                    trusted_by_lan: false,
                    trusted_by_cloud: false,
                    security_state: "unverified".to_string(),
                }],
                None,
            )
            .expect("ensure trusts");

        let trusts = database.load_trusted_peer_keys().expect("load trusts");
        assert_eq!(trusts.len(), 1);
        assert_eq!(trusts[0].name, "cloud-name");
        assert_eq!(trusts[0].public_key, "local-new");
        assert_eq!(trusts[0].key_updated_at, 2_000_000_000_000);
        assert!(trusts[0].trusted_by_lan);
        assert!(!trusts[0].trusted_by_cloud);

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
