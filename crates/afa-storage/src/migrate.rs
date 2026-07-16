//! Code Map: afa-storage::migrate
//! - `migrate`: The "apply the engine's `&[Migration]`
//!   list, skipping ones already applied" function.
//!   The engine's list is built at boot and handed to
//!   `migrate`; the storage layer sorts by `version`
//!   and applies the ones greater than the current
//!   `_afa_migrations.version` max.
//!
//! Story (plain English): Imagine a librarian with a
//! stack of "how to build drawer 7", "how to build
//! drawer 8", ... cards, and a little notebook that
//! records which drawers have already been built.
//! They sort the cards by number, then for each one
//! they ask: "have I built this one yet?" If yes,
//! skip. If no, run the card, stamp "built at
//! 2026-07-15 14:23" in the notebook, and move on.
//! The notebook is the `_afa_migrations` table; the
//! cards are the `&[Migration]`; the librarian is
//! `migrate`.
//!
//! The lock is taken with `lock().await` (the
//! async variant). The IMPL's draft used
//! `blocking_lock()` with the rationale "migrate
//! runs at boot, before the runtime is fully up";
//! in practice the kernel boots inside
//! `#[tokio::main]`, so the runtime IS up by the
//! time the engine's `new` calls `migrate`. The
//! async lock is the only correct choice; the
//! `blocking_lock` would deadlock the runtime
//! (and panic in `#[tokio::test]`).
//!
//! CID Index:
//! CID:afa-storage-migrate-001 -> migrate
//!
//! Quick lookup: rg -n "CID:afa-storage-migrate-" crates/afa-storage/src/migrate.rs

use crate::Storage;
use afa_contracts::StorageError;

// CID:afa-storage-migrate-001 - migrate
// Purpose: Apply the engine's `&[Migration]` list
// idempotently. The algorithm:
//
// 1. CREATE TABLE IF NOT EXISTS _afa_migrations (
//      version INTEGER PRIMARY KEY,
//      applied_at TEXT NOT NULL
//    ) — the "notebook".
// 2. Read the current `MAX(version)` (or 0 if the
//    table is empty / new file).
// 3. Sort the input `&[Migration]` by `version`
//    (the caller may not have sorted it; we sort
//    here so the public API is order-independent).
// 4. For each migration:
//    a. Skip if `migration.version <= current_version`.
//    b. Otherwise: start a transaction, run
//       `migration.sql`, INSERT the new row into
//       `_afa_migrations`, commit. If anything
//       fails, the per-migration transaction is
//       rolled back (so a bad migration 2 does not
//       leave migration 1's row in
//       `_afa_migrations` — wait, that's wrong, see
//       the per-migration-transaction note below).
//
// The per-migration transaction is the
// `idempotent` semantic: a FAILED migration rolls
// back its own `_afa_migrations` row (the one we
// tried to INSERT inside the same transaction), but
// the PREVIOUS migration's row (if any) is
// committed and stays. So after a failed
// migration, re-running the list re-applies the
// failed one (and only the failed one) — the
// `idempotent` semantic. The IMPL's wording
// "re-running the list applies migration 1 again"
// is wrong on the surface; the correct reading is
// "re-running the list re-applies the FAILED
// migration, leaving the previously-applied ones
// alone".
//
// Errors:
// - `StorageError::Migrate { version: 0, source }`
//   for the CREATE TABLE / SELECT COALESCE(MAX(...))
//   steps (the "0" is a placeholder — these steps
//   are not migration-versioned).
// - `StorageError::Migrate { version, source }`
//   for the per-migration steps (transaction
//   start, `execute_batch`, INSERT, commit).
//
// Used by: every engine that ships a SQLite schema
// at boot. The engine builds a `&[Migration]`
// (with `version` and `sql` fields, baked into the
// binary as `&'static str` slices) and hands it to
// `migrate` once, at boot.
//
// Async note: this function is `async` (returns
// `Future`, not the bare `Result`) so the lock can
// be acquired with `lock().await`. The IMPL's
// draft was sync with `blocking_lock()`; the async
// version is the architectural correction (see the
// module-level doc comment).
pub async fn migrate(
    storage: &Storage,
    migrations: &[afa_contracts::Migration],
) -> Result<(), StorageError> {
    // Async lock: `lock().await` is the runtime
    // variant. The IMPL's draft used
    // `blocking_lock()`; that would deadlock
    // (and panic) if called from inside a tokio
    // runtime, which is always the case in
    // practice (the kernel boots inside
    // `#[tokio::main]`).
    let mut conn = storage.inner.lock().await;

    // Step 1: ensure the `_afa_migrations`
    // notebook exists. The `IF NOT EXISTS` makes
    // the call idempotent — a fresh file gets the
    // table; an existing file with the table
    // already in place is a no-op.
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS _afa_migrations (
             version INTEGER PRIMARY KEY,
             applied_at TEXT NOT NULL
         )",
    )
    .map_err(|e| StorageError::Migrate {
        version: 0,
        source: e,
    })?;

    // Step 2: read the current `MAX(version)`. A
    // fresh file returns 0 (the `COALESCE` covers
    // the empty-table case). The version here
    // drives the "skip" decision in the loop.
    let current_version: u32 = conn
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM _afa_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(|e| StorageError::Migrate {
            version: 0,
            source: e,
        })?;

    // Step 3: sort by `version`. The caller may
    // have hand-written the list out of order, or
    // (more likely) a future pack's migration
    // list is built by a macro that emits
    // migrations in declaration order. Sorting
    // here is cheap and removes a footgun.
    let mut sorted: Vec<&afa_contracts::Migration> = migrations.iter().collect();
    sorted.sort_by_key(|m| m.version);

    // Step 4: apply each new migration in its own
    // transaction.
    for migration in sorted {
        // Skip already-applied migrations. The
        // `<=` (not `<`) is correct: a migration
        // whose `version` is already in the
        // notebook was applied, so we don't apply
        // it again.
        if migration.version <= current_version {
            continue;
        }
        // The transaction is per-migration: a
        // failed migration rolls back its own
        // `_afa_migrations` INSERT, but the
        // previously-committed migrations' rows
        // are unaffected.
        let tx = conn.transaction().map_err(|e| StorageError::Migrate {
            version: migration.version,
            source: e,
        })?;
        tx.execute_batch(migration.sql)
            .map_err(|e| StorageError::Migrate {
                version: migration.version,
                source: e,
            })?;
        tx.execute(
            "INSERT INTO _afa_migrations (version, applied_at) VALUES (?1, ?2)",
            rusqlite::params![migration.version, chrono::Utc::now().to_rfc3339()],
        )
        .map_err(|e| StorageError::Migrate {
            version: migration.version,
            source: e,
        })?;
        tx.commit().map_err(|e| StorageError::Migrate {
            version: migration.version,
            source: e,
        })?;
    }
    Ok(())
}
