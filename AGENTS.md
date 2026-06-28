# CoLink Desktop Agent Notes

## Protocol Versions

- `.colink/protocol/version.yml` records the existing Business and P2P protocol versions with which this project is currently aligned.
- When the implementation changes to align with a different published protocol version, update the corresponding value in this file in the same change.

## Database Migrations

- Every persistent database change must add an explicit migration and bump the migration version.
- Database changes include SQLite tables, indexes, constraints, and persisted JSON fields, meaning, or format inside `kv_store`.
- Migrations must be registered in the migration list in `src-tauri/src/store/db.rs` and run through the startup initialization flow.
- Do not perform implicit migrations in business read/write paths or use `serde(default)`, read-after-load writeback, runtime fallbacks, or similar mechanisms to support old persisted formats or fields.
- Handle old formats only inside explicit migration code. When migrating JSON, prefer editing known fields with `serde_json::Value` while preserving unknown fields instead of deserializing into the current model and rewriting the whole record. After migration, validate the data with the latest strict runtime model; corrupt data or missing required fields must make startup fail.
- Run each migration in a single SQLite transaction so that the migration logic and the `schema_migrations` insert commit together.
- If the database version is newer than the application supports, startup must fail. Do not continue and write to the database.

### Required Tests

- When adding or changing migrations, tests must cover:
  - Fresh database initialization.
  - Upgrading an old database while preserving data.
  - Incremental upgrade, for example v1 already applied and v2 not yet applied.
  - Repeated startup without rerunning applied migrations.
  - Rejecting a database version newer than the application supports.
  - Transaction rollback when a migration fails.
  - Save entry points still normalize data before writing.

## Storage Boundaries

- The storage layer is responsible for ensuring that written data is in canonical format.
- Save entry points such as `save_settings()` and `save_device_identity()` should normalize before writing to the database.
- Business code may normalize earlier for validation or response consistency, but it must not be the only line of defense.
- Keep cloud API DTOs separate from local persisted models. Tolerance for missing external API fields must not leak into local cache models.
