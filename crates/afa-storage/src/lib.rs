//! Code Map: afa-storage (the SQLite vault wrapper)
//! - `Storage`: The wrapped SQLite connection. Owns
//!   `Arc<tokio::sync::Mutex<rusqlite::Connection>>`
//!   so the kernel can hand the same `Storage` across
//!   `await` points without `unsafe` and so the
//!   `Connection`'s non-`Sync` borrow never escapes the
//!   lock guard. Cheap to clone (`Arc<Mutex<...>>`).
//!   See `Storage` below.
//! - `open`: The "open the SQLite file (create parent
//!   dir if missing), wrap it in a `Storage`" entry
//!   point. Used at boot by every engine. See
//!   `open::open`.
//! - `migrate`: The "apply the engine's `&[Migration]`
//!   list, skipping ones already applied" function.
//!   Used at boot by every engine. See
//!   `migrate::migrate`.
//! - `with_conn`: The "lend me the `Connection` for one
//!   synchronous operation, behind an async lock"
//!   helper. Used at runtime by every engine's read
//!   and write paths. See `with_conn::with_conn`.
//! - `StorageError`: The "what went wrong opening /
//!   migrating the SQLite file?" enum. Re-exported
//!   from `afa_contracts` (the contract surface) so
//!   the error type is a single dictionary entry, not
//!   two. See `error::StorageError`.
//!
//! Story (plain English): Imagine a small library with
//! one card-catalog drawer. Every engine that needs
//! storage (the security engine, the observability
//! engine, future engines) shares the same drawer
//! because building a separate drawer per engine
//! would mean three file paths, three schema-migration
//! stories, three backup scripts. This crate is the
//! "shared drawer" wrapper: the file lives at
//! `<secrets_db_path>` (or, for the spans engine, at
//! `<spans_db_path>` — a future pack may put them on
//! the same path, but in Phase 0.5a every engine has
//! its own `Storage` instance pointing at its own
//! file). The drawer is opened with `open`, the index
//! cards are filed with `migrate` (a card is one
//! `Migration` — a SQL block + a version number), and
//! the librarian (the engine) flips through them with
//! `with_conn`. The wrapper's job is small: make sure
//! two librarians never read the same card at the
//! same time (the `tokio::sync::Mutex` does that),
//! and make sure a new card is only filed once (the
//! `_afa_migrations` table does that).
//!
//! This file is just the public surface and the
//! `Storage` struct definition. The three locked
//! methods live in the sibling files (`open`,
//! `migrate`, `with_conn`).
//!
//! CID Index:
//! CID:afa-storage-lib-001 -> Storage struct
//! CID:afa-storage-lib-002 -> open re-export
//! CID:afa-storage-lib-003 -> migrate re-export
//! CID:afa-storage-lib-004 -> with_conn re-export
//! CID:afa-storage-lib-005 -> StorageError re-export
//!
//! Quick lookup: rg -n "CID:afa-storage-lib-" crates/afa-storage/src/lib.rs

use rusqlite::Connection;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Mutex;

// CID:afa-storage-lib-001 - Storage struct
// Purpose: The wrapped SQLite connection. Owns
// `Arc<tokio::sync::Mutex<rusqlite::Connection>>` so
// the kernel can hand the same `Storage` across `await`
// points without `unsafe` and so the `Connection`'s
// non-`Sync` borrow never escapes the lock guard.
// Cheap to clone (`Arc<Mutex<...>>`). The `path` is
// stored alongside the connection for two reasons:
// (a) the dashboard's `GET /spans/storage` (Pack #6
// Phase 3) reports the on-disk path of the spans
// file, and (b) `migrate` (Phase 0.5a) needs the path
// to log which file the migration was applied to.
// Used by: every engine that ships a SQLite schema
// (`afa-security`'s `seal_secret` / `fetch_secret` /
// `fetch_active_hash` helpers in Phase 0.5b,
// `afa-observability`'s `record_span` and
// `run_purge_loop` in Phase 1, future engines' tables).
#[derive(Clone, Debug)]
pub struct Storage {
    /// The shared, mutex-guarded SQLite connection.
    /// `Arc` so `Storage: Clone` is cheap; `Mutex` so
    /// `with_conn` can `lock().await` for runtime
    /// callers and `migrate` can `lock().await` for
    /// boot-time callers (see `migrate::migrate` for the
    /// rationale on the lock variant).
    pub(crate) inner: Arc<Mutex<Connection>>,
    /// The on-disk path the connection was opened on.
    /// Stored for diagnostic logging (the migration
    /// trace logs "applied migration N to {path}") and
    /// for the dashboard's `GET /spans/storage` handler.
    pub path: PathBuf,
}

// CID:afa-storage-lib-002 - open re-export
// Re-export the `open` function at the crate root so
// callers write `afa_storage::open(...)` instead of
// `afa_storage::open::open(...)`. The actual
// implementation is in `open.rs`.
pub use open::open;

// CID:afa-storage-lib-003 - migrate re-export
// Re-export the `migrate` function at the crate root.
// Implementation in `migrate.rs`.
pub use migrate::migrate;

// CID:afa-storage-lib-004 - with_conn re-export
// Re-export the `with_conn` function at the crate root.
// Implementation in `with_conn.rs`.
pub use with_conn::with_conn;

// CID:afa-storage-lib-005 - StorageError re-export
// Re-export the `StorageError` enum at the crate root.
// The canonical definition lives in `afa-contracts`
// (the dictionary every consumer agrees on); this
// re-export means callers can write
// `use afa_storage::StorageError;` and never have to
// know the type's contract-crate address.
pub use afa_contracts::StorageError;

// The three sibling modules — each holds one of the
// three locked methods. Kept as separate files so the
// per-method code stays under 250 lines (the
// workspace's "files stay small" rule) and so a future
// contributor can find the method by filename alone.
mod migrate;
mod open;
mod with_conn;
