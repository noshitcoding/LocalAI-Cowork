---
title: SQLite Durability and Migration Contract
type: api
doc_type: compatibility
status: current
owner: product-docs
last_updated: 2026-07-10
last_verified: 2026-07-10
endpoint: Database::open and Database::open_in_memory
purpose: Open, validate, back up, and migrate the local SQLite database without partial schema upgrades or silent corruption.
userStory: As a user I can upgrade or restart the app without losing confirmed local data or continuing on an unsupported database state.
visibleText: none
sizeToken: none
states: new, healthy, upgrade-required, backed-up, migrated, busy, corrupt, unsupported-version
interactions: open database, validate integrity, back up old schema, migrate atomically, reopen
dataSource: app-data/open_cowork.db and app-data/database-backups/pre-migration-*.db
accessibility: not applicable
tests: db.rs PRAGMA, WAL reopen, upgrade backup, migration rollback, corruption, foreign-key, and schema-version tests
source_files:
  - app/src-tauri/src/db.rs
  - app/src-tauri/Cargo.toml
canonical_for:
  - SQLite connection policy
  - schema migration atomicity
  - pre-migration backup retention
  - database startup validation
rationale: A schema version is not proof of a complete migration. Durability settings, integrity checks, online backups, and a single write transaction are required together.
---

# SQLite Durability and Migration Contract

The persistent database is `open_cowork.db` under the Tauri app-data directory. Opening it enables foreign keys, a five-second busy timeout, `synchronous=FULL`, in-memory temporary storage, WAL mode, a 1,000-page automatic checkpoint, and a 64 MiB journal-size limit. Directory creation errors are fatal rather than ignored.

Before schema changes, SQLite `quick_check` and `foreign_key_check` must pass. `schema_version` must be absent for a new database or contain exactly one nonnegative integer no newer than this build supports. A newer, malformed, corrupt, or referentially inconsistent database is rejected without migration.

When an existing non-empty database is older than schema version `23`, the SQLite online-backup API creates a consistent copy under `database-backups`. The copy is opened and integrity-checked before migration starts. At most three `pre-migration-*.db` files are retained. Reopening an already-current database does not create another backup.

All required schema versions run inside one `BEGIN IMMEDIATE` transaction. Any failed DDL, data update, index creation, or version write rolls back the entire upgrade. The database is checked again after commit. Tests inject a failure after a successful index creation and prove that neither the partial index nor a new schema version survives.

These backups contain the same sensitive local content as the primary database and inherit the app-data directory trust boundary. Automated restoration and user-facing recovery selection are separate release work; operators must not overwrite the primary database while Open Cowork is running.
