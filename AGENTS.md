# CoLink Desktop Agent Notes

## Database Migrations

- Every persistent database change must add an explicit migration and bump the migration version.
- Database changes include SQLite tables, indexes, constraints, and persisted JSON fields, meaning, or format inside `kv_store`.
- Migrations must be registered in the migration list in `src-tauri/src/store/db.rs` and run through the startup initialization flow.
- Do not perform implicit migrations in business read/write paths.
- Do not use `serde(default)`, read-after-load writeback, runtime fallbacks, or similar mechanisms to support old persisted formats or old fields.
- Old formats may only be handled inside explicit migration code. After migration, runtime models should read the latest format strictly.
- Each migration must run in a single SQLite transaction. The migration logic and the `schema_migrations` insert must commit together.
- If the database version is newer than the application supports, startup must fail. Do not continue and write to the database.
- When migrating old JSON, prefer editing known fields with `serde_json::Value` while preserving unknown fields. Do not migrate by deserializing into the current model and rewriting the whole record.
- After migration, validate that the data can be read by the latest strict model. Corrupt data or missing required fields should make startup fail.
- `lan_trust` currently has no historical compatibility fields. Unless its structure changes, it does not need content migration and only exists as part of the schema baseline.

## Storage Boundaries

- The storage layer is responsible for ensuring that written data is in canonical format.
- Save entry points such as `save_settings()` and `save_device_identity()` should normalize before writing to the database.
- Business code may normalize earlier for validation or response consistency, but it must not be the only line of defense.
- Keep cloud API DTOs separate from local persisted models. Tolerance for missing external API fields must not leak into local cache models.

## Required Tests

- When adding or changing migrations, tests must cover:
  - Fresh database initialization.
  - Upgrading an old database while preserving data.
  - Incremental upgrade, for example v1 already applied and v2 not yet applied.
  - Repeated startup without rerunning applied migrations.
  - Rejecting a database version newer than the application supports.
  - Transaction rollback when a migration fails.
  - Save entry points still normalize data before writing.
