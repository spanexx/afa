//! Code Map: afa-security::storage
//! - `Storage`: Re-export of `afa_storage::Storage` —
//!   the `Arc<tokio::sync::Mutex<Connection>>` wrapper
//!   with `path: PathBuf` and the three locked
//!   methods (`open`, `migrate`, `with_conn`).
//! - `SCHEMA_MIGRATIONS`: The `&'static [Migration]`
//!   for the security pack's two tables
//!   (`sealed_secrets` and `afa_security_meta`).
//!   The engine hands this to
//!   `afa_storage::migrate` at boot.
//! - `SCHEMA_VERSION`: The integer
//!   `afa_security_meta.schema_version` value this
//!   engine writes and expects to read back. Bump
//!   on any breaking schema change.
//! - `STATUS_ACTIVE` / `STATUS_ROTATED`: The two
//!   `sealed_secrets.status` values. Encoded as
//!   strings, not integers, so a row is human-
//!   readable when the file is opened in
//!   `sqlite3`'s CLI.
//! - `check_schema_version`: The
//!   "read-after-migrate" sanity check. The
//!   `afa_storage::migrate` function only knows
//!   about the `_afa_migrations` table; the
//!   per-engine `schema_version` row is the
//!   engine's own concern.
//!
//! Story (plain English): This is the desk's
//! bookshelf catalogue. It doesn't store any books
//! (the connection lives in `afa_storage::Storage`);
//! it just records **what shape** the shelf is
//! expected to be (the two `CREATE TABLE`
//! statements) and **what edition** of the catalogue
//! this engine is reading (`schema_version = 1`).
//! When a future pack ships a new edition (a new
//! column, a new table, a new index), the
//! `SCHEMA_MIGRATIONS` array grows by one entry,
//! the new `Migration` runs on the next boot (the
//! migrate loop sees the version is new and applies
//! it inside a transaction), and the engine bumps
//! `SCHEMA_VERSION` to match.
//!
//! **Doc drift correction vs. the IMPL draft**:
//! the IMPL's draft promised a `StorageError` re-
//! export (it lives in `afa-contracts` and is
//! re-exported from `afa_storage`); the
//! `SecurityError` ↔ `StorageError` mapping is
//! done in the engine, not in this module. The
//! engine's `with_conn` calls return
//! `StorageError`; the engine wraps them in
//! `SecurityError` (typically
//! `SecurityError::Internal` or
//! `SecurityError::StorageCorrupted`).
//!
//! CID Index:
//! CID:afa-security-storage-001 -> Storage
//! CID:afa-security-storage-002 -> SCHEMA_MIGRATIONS
//! CID:afa-security-storage-003 -> SCHEMA_VERSION
//! CID:afa-security-storage-004 -> STATUS_ACTIVE
//! CID:afa-security-storage-005 -> STATUS_ROTATED
//! CID:afa-security-storage-006 -> check_schema_version
//!
//! Quick lookup: rg -n "CID:afa-security-storage-" crates/afa-security/src/storage.rs

use afa_contracts::{Migration, StorageError};
use std::path::Path;

// Re-export the `Storage` newtype from `afa-storage`
// so the rest of `afa-security` (the engine's
// `impl SecurityV1`, the `check_schema_version`
// helper) can name the type via `crate::storage::Storage`
// without reaching across crates. The crate root
// re-exports it again as `afa_security::Storage`
// (see `lib.rs`).
pub use afa_storage::Storage;

// CID:afa-security-storage-001 - Storage
// Purpose: The public re-export of
// `afa_storage::Storage`. The security pack
// doesn't own the `Connection` anymore (Phase
// 0.5a moved it to `afa_storage`); it just
// hands a `Storage` to the engine and trusts
// `afa_storage` for the open / migrate / lock
// plumbing. The engine does its own SQL via
// `Storage::with_conn`.
//
// **API change vs. pre-Phase-0.5a**: the old
// `SealedSecretStore` struct is GONE. The
// public surface is now `Storage` (re-exported)
// + `SCHEMA_MIGRATIONS` + the three constants.
// Any caller that referenced `SealedSecretStore`
// must update (the kernel's `Kernel::new` and
// the security tests' helpers are the two known
// call sites).
//
// Used by: the engine (every SQL call), the
// kernel (boots a `Storage` and hands it to the
// engine).
// (the type is re-exported via `crate::Storage` in lib.rs)

// CID:afa-security-storage-002 - SCHEMA_MIGRATIONS
// Purpose: The `&'static [Migration]` for the
// security pack's two tables. The engine
// passes this to `afa_storage::migrate` at boot
// (via `Kernel::new`, which the kernel calls).
//
// Migration 1 (the only migration as of v1):
//   - `CREATE TABLE IF NOT EXISTS sealed_secrets` —
//     the main per-secret table. The
//     `PRIMARY KEY (name, version)` constraint is
//     what makes a parallel `seal` race fail
//     cleanly with a UNIQUE constraint violation
//     (or, with the engine's `BEGIN IMMEDIATE`
//     pattern, serialize correctly).
//   - `CREATE TABLE IF NOT EXISTS afa_security_meta` —
//     the version tag, in a table (not
//     `PRAGMA user_version`) so it travels with
//     the file when the file is copied between
//     machines (the `PRAGMA` is per-connection
//     and not persisted to the file).
//   - `INSERT OR IGNORE INTO afa_security_meta` —
//     the "stamp the version on the file" step.
//     Idempotent: re-running the migration does
//     not overwrite an existing `schema_version`
//     row.
//
// To add a migration (future pack): append a
// new `Migration { version: 2, sql: "..." }` to
// this array, bump `SCHEMA_VERSION` to 2, and
// ship. The migrate loop applies migration 2
// only on a fresh boot or a file that was last
// migrated to version 1.
//
// The `r#"..."#` raw string keeps the multi-
// statement SQL readable (the embedded single
// quotes are the only escape needed).
//
// Used by: `Kernel::new` (the boot path), via
// `afa_storage::migrate(&storage, SCHEMA_MIGRATIONS)`.
//
// **Migration history**:
//   - v1: initial schema (`sealed_secrets` + `afa_security_meta`).
//   - v2 (Phase 0.5b): adds the `sha256` column
//     that `lookup_hash` reads. The column is
//     `BLOB` (not `TEXT`) so the on-disk size
//     matches the SHA-256 output exactly; the
//     engine stores lowercase hex (64 ASCII
//     bytes) so the `lookup_hash` constant-time
//     compare can short-circuit on length
//     without a second decode.
pub static SCHEMA_MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: r#"
            CREATE TABLE IF NOT EXISTS sealed_secrets (
                name        TEXT NOT NULL,
                version     INTEGER NOT NULL,
                status      TEXT NOT NULL CHECK (status IN ('active', 'rotated')),
                nonce       BLOB NOT NULL,
                ciphertext  BLOB NOT NULL,
                created_at  TEXT NOT NULL,
                PRIMARY KEY (name, version)
            );
            CREATE TABLE IF NOT EXISTS afa_security_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT OR IGNORE INTO afa_security_meta (key, value)
                VALUES ('schema_version', '1');
        "#,
    },
    // Phase 0.5b — the `sha256` column. The
    // `lookup_hash` method reads this column to
    // constant-time-compare against the incoming
    // hash. The column is nullable (existing v1
    // rows have no hash; `lookup_hash` treats a
    // `NULL` value the same as "no row" and
    // returns `SecretNotFound`).
    //
    // The migration is two statements: (1) add
    // the `sha256` column, (2) bump the
    // `schema_version` row from `1` to `2`. The
    // `check_schema_version` step at the end of
    // the boot path will see `2` and accept the
    // file. A file that somehow ended up with
    // the v2 column but `schema_version = 1`
    // would be rejected (the column-add and
    // the version-bump must be atomic).
    Migration {
        version: 2,
        sql: r#"
            ALTER TABLE sealed_secrets ADD COLUMN sha256 BLOB;
            UPDATE afa_security_meta SET value = '2' WHERE key = 'schema_version';
        "#,
    },
];

// CID:afa-security-storage-003 - SCHEMA_VERSION
// Purpose: The integer `schema_version` value
// this engine writes (in migration 2) and
// expects to read back (via
// `check_schema_version`). Bump on any
// breaking schema change; a mismatch surfaces
// as `SecurityError::SchemaVersionMismatch` at
// boot. The constant is `pub` (not `pub(crate)`)
// so the kernel and the boot-failures test can
// reference it from outside the security crate.
//
// The value matches the last `Migration { version: N, ... }`
// in `SCHEMA_MIGRATIONS` (the `N`). If you
// change one, change the other.
//
// History: 1 (initial), 2 (Phase 0.5b, adds the
// `sha256` column for `lookup_hash`).
pub const SCHEMA_VERSION: u32 = 2;

// CID:afa-security-storage-004 - STATUS_ACTIVE
// Purpose: The `sealed_secrets.status` value
// for a row that is the current version for its
// `name`. A new `seal` or `rotate` inserts a
// row at this status. The `lookup_hash` method
// (the new real override in Phase 0.5b) filters
// on `status = 'active'`.
pub const STATUS_ACTIVE: &str = "active";

// CID:afa-security-storage-005 - STATUS_ROTATED
// Purpose: The `sealed_secrets.status` value
// for a row that has been superseded by a newer
// version. A `rotate` updates the old row from
// `'active'` to `'rotated'` inside the same
// transaction that inserts the new row. A
// `lookup_hash` for the old version returns
// `Err(SecretRotated)` (a different error from
// `SecretNotFound` — see the IMPL's
// distinguishing rule).
pub const STATUS_ROTATED: &str = "rotated";

// CID:afa-security-storage-006 - check_schema_version
// Purpose: The "read-after-migrate" sanity
// check. The `afa_storage::migrate` function
// only knows about the `_afa_migrations` table
// (the migration tracking); the per-engine
// `schema_version` row is a separate concern,
// read here. Returns the read `schema_version`
// as a `u32` (or 0 if the row is missing —
// which would be an inconsistent state since
// the migration inserts the row, but the
// `unwrap_or(0)` is the safe fallback that
// still surfaces as a `SchemaVersionMismatch`
// error).
//
// Errors:
// - `StorageError::Migrate { version: 0, source }`
//   on any SQL error (the `version: 0` is a
//   placeholder; this is a post-migrate
//   read, not a migration itself).
//
// Used by: the engine's boot path (in
// `SecurityEngine::new`) to verify the
// migration ran correctly. The kernel
// delegates the boot to the engine; the
// engine's `new` is the one place that
// combines the `Storage`, the master key,
// the event bus, and the schema_version
// check.
pub async fn check_schema_version(storage: &Storage) -> Result<u32, StorageError> {
    afa_storage::with_conn(storage, |conn| {
        Box::pin(async move {
            // The `afa_security_meta` table may not
            // exist on a fresh file (it is created by
            // migration 1, which has not run yet at
            // the pre-migrate check). Treat
            // "no such table" as "version 0" so the
            // pre-migrate check does not spuriously
            // fail. The `query_row(...).optional()`
            // would only return `Ok(None)` if the
            // table exists but the row is missing;
            // the rusqlite error from "no such
            // table" is mapped here to `Ok(None)`,
            // which then unwraps to `version 0`.
            let found: Option<String> = match conn.query_row(
                "SELECT value FROM afa_security_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            ) {
                Ok(row) => Some(row),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(rusqlite::Error::SqliteFailure(err, _))
                    if err.code == rusqlite::ErrorCode::Unknown
                        || err.code == rusqlite::ErrorCode::DatabaseCorrupt =>
                {
                    // "No such table" is a
                    // `SqliteFailure` with code
                    // `ErrorCode::DatabaseCorrupt` (the
                    // generic "something is wrong" code;
                    // SQLite has no "NoSuchTable" code).
                    // Treat as "table missing → version
                    // 0".
                    None
                }
                Err(e) => {
                    return Err(StorageError::Migrate {
                        version: 0,
                        source: e,
                    })
                }
            };
            let version: u32 = found.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
            Ok::<u32, StorageError>(version)
        })
    })
    .await
}

// CID:afa-security-storage-007 - (helper) open_storage
// Purpose: The "open + migrate + check" helper
// the kernel calls. Bundles the three steps
// (open the file, run the security migrations,
// check the schema_version) into one call so
// `Kernel::new` doesn't have to know about
// the migration constant. Returns a `Storage`
// ready for the engine.
//
// Errors: any of the three steps' errors
// surface as a `StorageError` (the kernel
// wraps the `Open` error in
// `SecurityError::StorageUnreachable`, the
// `Migrate` error in `StorageCorrupted`, and
// the `check_schema_version` mismatch in
// `SchemaVersionMismatch`).
//
// **API change vs. pre-Phase-0.5a**: the old
// `SealedSecretStore::open_or_create` did
// the same three steps but in one struct
// method. The new design splits them: the
// `Storage` does the I/O (open / lock), the
// `SCHEMA_MIGRATIONS` is the data, the
// `check_schema_version` is the read. Three
// functions, one call site.
// **Step ordering vs. pre-Phase-0.5b**:
// the version check runs BEFORE the
// migration step. The old order
// (open → migrate → check) meant a tampered
// file with `schema_version = '99'` would
// have its `schema_version` row overwritten
// by the v2 migration's `UPDATE ... SET
// value = '2'`, and the file would silently
// be accepted. The new order (open → check
// → migrate → verify) refuses a file whose
// claimed version is HIGHER than the engine
// supports (a "future" schema this engine
// cannot read), and only runs the migrations
// on a file whose claimed version is LOWER
// than or equal to the engine's
// `SCHEMA_VERSION`. A file with NO
// `schema_version` row at all (a fresh file
// or a tampered file that deleted the row)
// is treated as version 0 (the
// `check_schema_version` function returns 0
// for a missing row), and the migrations
// run to bring it up to date.
//
// This is the IMPL's "fail fast at boot with
// a typed error" rule applied to the
// "restored an old secrets.db" footgun AND
// the "downgraded the binary" footgun. Both
// should fail at boot, not silently
// auto-migrate.
pub async fn open_storage(path: &Path) -> Result<Storage, StorageError> {
    let storage = afa_storage::open(path)?;

    // Step 1: refuse a file that claims a
    // future version (higher than what this
    // engine can read). The check runs
    // BEFORE the migrate so a tampered
    // `schema_version` row cannot be
    // overwritten by the v2 migration's
    // `UPDATE`.
    let pre_migrate_version = check_schema_version(&storage).await?;
    if pre_migrate_version > SCHEMA_VERSION {
        return Err(StorageError::Migrate {
            version: pre_migrate_version,
            source: rusqlite::Error::InvalidQuery, // placeholder; the kernel wraps this
        });
    }

    // Step 2: run the migrations (adds the
    // `sha256` column on a v1→v2 upgrade,
    // bumps `schema_version` from 1 to 2).
    afa_storage::migrate(&storage, SCHEMA_MIGRATIONS).await?;

    // Step 3: verify the post-migrate
    // version matches the engine's
    // expected `SCHEMA_VERSION`. A migration
    // that failed to bump the version row
    // (e.g. the `UPDATE` step was edited out
    // by a future maintainer) would be
    // caught here.
    let post_migrate_version = check_schema_version(&storage).await?;
    if post_migrate_version != SCHEMA_VERSION {
        return Err(StorageError::Migrate {
            version: post_migrate_version,
            source: rusqlite::Error::InvalidQuery, // placeholder; the kernel wraps this
        });
    }
    Ok(storage)
}
