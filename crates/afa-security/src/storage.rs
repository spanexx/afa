//! Code Map: SQLite-backed sealed-secret store
//! - `SealedSecretStore`: The vault. Owns the `rusqlite::Connection`
//!   behind a `tokio::sync::Mutex` so the engine can hand it
//!   across `await` points without `unsafe`. Exposes
//!   `open_or_create`, `insert_active`, `get_active`, `get_any`
//!   (Phase 2), and `rotate` (Phase 2). See the `impl` block below.
//! - `SCHEMA_VERSION`: The on-disk schema version this engine
//!   supports. A future pack that changes the schema increments
//!   this constant and adds a migration step to `open_or_create`.
//!
//! Story (plain English): Imagine the vault's index card file.
//! Every box that has ever been filed has a card. The card
//! says: the box's name, its version number, whether it is
//! still the active one (or whether a newer version has
//! replaced it), the serial number the seal machine printed
//! when the box was filed, the sealed envelope itself, and
//! the time the box was filed. The clerk can flip through
//! the cards to find a particular `(name, version)`, can file
//! a new card, or can update the "rotated" stamp on an old
//! card when a new one is filed. The file is the
//! `sealed_secrets` table; the index cards are the rows.
//!
//! A second card file (`afa_security_meta`) records the
//! schema version, so a future pack that changes the file's
//! format can detect "I cannot read this old file" before
//! it tries to do anything. Today, the schema version is
//! always 1; tomorrow, a v2 schema ships and `open_or_create`
//! will fail with `SchemaVersionMismatch` on a v1 file.
//!
//! Every write (`insert_active`, `rotate`) goes through a
//! `BEGIN IMMEDIATE` transaction so the engine's "no two
//! callers ever receive the same version number" rule holds
//! even when two adapters race to seal or rotate the same
//! name at the same time.
//!
//! CID Index:
//! CID:afa-security-storage-001 -> SealedSecretStore struct
//! CID:afa-security-storage-002 -> open_or_create
//! CID:afa-security-storage-003 -> insert_active
//! CID:afa-security-storage-004 -> get_active
//!
//! Quick lookup: rg -n "CID:afa-security-storage-" crates/afa-security/src/storage.rs

use crate::crypto::NONCE_LEN;
use crate::SecurityError;
use rusqlite::{params, Connection, OptionalExtension};
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

/// The on-disk schema version this engine supports. A future
/// pack that changes the schema increments this constant and
/// adds a migration step to `open_or_create`.
pub const SCHEMA_VERSION: u32 = 1;

/// The `status` column value for a row that is the live one
/// to use. The other value is `STATUS_ROTATED`, set by
/// `rotate` when a newer version takes over.
pub const STATUS_ACTIVE: &str = "active";
/// The `status` column value for a row that has been replaced
/// by a newer version. The row stays on disk for forensic
/// audit; `get_active` skips it.
pub const STATUS_ROTATED: &str = "rotated";

// CID:afa-security-storage-001 - SealedSecretStore struct
// Purpose: The vault's index card file. Owns the
// `rusqlite::Connection` behind a `tokio::sync::Mutex` (so
// the engine can hand it across `await` points without
// `unsafe` and so the `Connection`'s non-`Sync` borrow does
// not escape the lock guard). Cheap to clone (`Arc<Mutex<...>>`).
// Used by: `engine::SecurityEngine`.
#[derive(Clone)]
pub struct SealedSecretStore {
    conn: Arc<Mutex<Connection>>,
}

// CID:afa-security-storage-002 - open_or_create
// Purpose: Open the SQLite file at `path` (creating it and
// the parent directory if missing), run the idempotent
// schema, record `schema_version` in `afa_security_meta`,
// and reject the file if its `schema_version` is not the one
// this engine supports.
// Errors: `StorageUnreachable` on path/permission failures,
// `SchemaVersionMismatch { found, expected }` on a v!=1 file.
// Used by: `engine::SecurityEngine::new` (Phase 3 wires
// `Kernel::new` to call it) and the Phase 1 test
// `tests/first_boot_creates_db.rs`.
impl SealedSecretStore {
    pub fn open_or_create(path: &Path) -> Result<Self, SecurityError> {
        // Ensure the parent directory exists. This is the
        // "first deploy" footgun the IMPL rollout notes call
        // out: `/var/lib/afa/` may not exist on a fresh
        // image, and we do not want the kernel to fail on
        // its very first request. `create_dir_all` is a
        // no-op if the directory already exists.
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| SecurityError::StorageUnreachable {
                    reason: format!("could not create parent dir {}: {}", parent.display(), e),
                })?;
            }
        }

        // Open (or create) the SQLite file. The `bundled`
        // feature in `Cargo.toml` removes any host-SQLite
        // version concern.
        let conn = Connection::open(path).map_err(|e| SecurityError::StorageUnreachable {
            reason: format!("could not open {}: {}", path.display(), e),
        })?;

        // Run the idempotent schema. Every statement is
        // `IF NOT EXISTS` so `open_or_create` is safe to
        // call on every boot. The `PRAGMA user_version`
        // line is the standard SQLite pattern for
        // recording a schema version without needing a
        // second table; we use a second table
        // (`afa_security_meta`) so the value travels with
        // the file when it is copied between machines.
        conn.execute_batch(
            r#"
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
        )
        .map_err(|_| SecurityError::StorageCorrupted)?;

        // Sanity-check the schema version. If a future
        // pack migrates the schema, this check is the
        // one that turns "the engine silently misreads
        // the file" into "the engine fails fast at boot
        // with a clear error."
        let found: Option<String> = conn
            .query_row(
                "SELECT value FROM afa_security_meta WHERE key = 'schema_version'",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(|_| SecurityError::StorageCorrupted)?;
        let found_version: u32 = found.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0);
        if found_version != SCHEMA_VERSION {
            return Err(SecurityError::SchemaVersionMismatch {
                found: found_version,
                expected: SCHEMA_VERSION,
            });
        }

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Borrow the underlying connection (behind the
    /// `tokio::sync::Mutex`) for engine-internal use. The
    /// engine's `seal` flow needs to compute the next
    /// version number and insert the row in the SAME
    /// `BEGIN IMMEDIATE` transaction (so two parallel
    /// `seal` calls cannot pick the same version). Keeping
    /// the transaction at the engine layer (rather than
    /// inside `SealedSecretStore`) lets the engine put the
    /// AEAD `seal` call between the version read and the
    /// row insert — the AAD string
    /// `format!("{}:{}", name, version)` needs the
    /// version visible to the encrypt step, so the
    /// version and the insert have to share a transaction.
    /// Marked `pub(crate)` so the accessor does not leak
    /// to downstream adapters (who only ever hold an
    /// `Arc<dyn SecurityV1>` and never see the
    /// `SealedSecretStore` at all).
    pub(crate) fn conn(&self) -> &Arc<Mutex<Connection>> {
        &self.conn
    }

    // CID:afa-security-storage-003 - insert_active
    // Purpose: Insert a new `(name, version, 'active', nonce,
    // ciphertext, created_at)` row. Caller computes `version`
    // (= `MAX(version) + 1` for the same `name`, inside the
    // same `BEGIN IMMEDIATE` transaction in the engine). The
    // store does NOT compute the version itself because the
    // engine's `seal` flow needs the version visible to the
    // AEAD-AAD string BEFORE the row is written; splitting
    // the version read from the row insert is the engine's
    // job, not the store's.
    // Errors: `StorageCorrupted` on SQL failures (the
    // `BEGIN IMMEDIATE` for the `seal` flow prevents the
    // one realistic race: two `seal` calls computing the
    // same `MAX(version)+1` and trying to insert with the
    // same `(name, version)`).
    // Used by: `engine::SecurityEngine::seal`.
    pub async fn insert_active(
        &self,
        name: &str,
        version: u32,
        nonce: &[u8; NONCE_LEN],
        ciphertext: &[u8],
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<(), SecurityError> {
        let conn = self.conn.lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| SecurityError::StorageCorrupted)?;
        tx.execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| SecurityError::StorageCorrupted)?;
        tx.execute(
            "INSERT INTO sealed_secrets \
             (name, version, status, nonce, ciphertext, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![
                name,
                version,
                STATUS_ACTIVE,
                &nonce[..],
                ciphertext,
                timestamp.to_rfc3339(),
            ],
        )
        .map_err(|_| SecurityError::StorageCorrupted)?;
        tx.execute_batch("COMMIT")
            .map_err(|_| SecurityError::StorageCorrupted)?;
        Ok(())
    }

    // CID:afa-security-storage-004 - get_active
    // Purpose: Look up the row for `(name, version)` with
    // `status='active'`. Returns `Some((ciphertext, nonce))`
    // if found, `None` otherwise. The engine uses `None` to
    // mean "either the secret was never sealed under that
    // name, or the version was wrong, or the version was
    // rotated." Phase 2 adds `get_any` so the engine can
    // distinguish `SecretNotFound` from `SecretRotated`.
    // Errors: `StorageCorrupted` on SQL failures.
    // Used by: `engine::SecurityEngine::unseal` (Phase 1).
    pub async fn get_active(
        &self,
        name: &str,
        version: u32,
    ) -> Result<Option<(Vec<u8>, [u8; NONCE_LEN])>, SecurityError> {
        let conn = self.conn.lock().await;
        let mut stmt = conn
            .prepare(
                "SELECT nonce, ciphertext FROM sealed_secrets \
                 WHERE name = ?1 AND version = ?2 AND status = ?3",
            )
            .map_err(|_| SecurityError::StorageCorrupted)?;
        let mut rows = stmt
            .query(params![name, version, STATUS_ACTIVE])
            .map_err(|_| SecurityError::StorageCorrupted)?;
        if let Some(row) = rows.next().map_err(|_| SecurityError::StorageCorrupted)? {
            let nonce_vec: Vec<u8> = row.get(0).map_err(|_| SecurityError::StorageCorrupted)?;
            let ciphertext: Vec<u8> = row.get(1).map_err(|_| SecurityError::StorageCorrupted)?;
            if nonce_vec.len() != NONCE_LEN {
                return Err(SecurityError::StorageCorrupted);
            }
            let mut nonce = [0u8; NONCE_LEN];
            nonce.copy_from_slice(&nonce_vec);
            Ok(Some((ciphertext, nonce)))
        } else {
            Ok(None)
        }
    }
}
