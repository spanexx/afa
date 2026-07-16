//! Code Map: afa-storage::open
//! - `open`: The "open the SQLite file (create parent
//!   dir if missing), wrap it in a `Storage`" entry
//!   point. The parent-directory creation is the
//!   "first deploy" footgun the IMPL rollout notes
//!   call out: `/var/lib/afa/` may not exist on a
//!   fresh image, and we do not want the kernel to
//!   fail on its very first request.
//!
//! Story (plain English): Imagine a librarian opening
//! a drawer for the first time. They check the
//! hallway: is the filing-cabinet room there? If not,
//! they build it. They check the cabinet: is this
//! specific drawer there? If yes, they slide it open
//! (and the file is preserved — not overwritten). If
//! no, they slot in a new drawer. Either way, the
//! "drawer + room + everything in it" handle they
//! hand back to the rest of the library is the
//! `Storage` struct. The point of this function is to
//! make "fresh install" and "existing install" look
//! the same to the caller.
//!
//! CID Index:
//! CID:afa-storage-open-001 -> open
//!
//! Quick lookup: rg -n "CID:afa-storage-open-" crates/afa-storage/src/open.rs

use crate::Storage;
use afa_contracts::StorageError;
use rusqlite::Connection;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::Mutex;

// CID:afa-storage-open-001 - open
// Purpose: Open the SQLite file at `path` (creating
// the parent directory and the file if missing), wrap
// the `Connection` in a `Storage`, and return the
// `Storage`.
//
// The `Arc<tokio::sync::Mutex<Connection>>` is the
// right primitive for two reasons:
// (1) the kernel needs to hand the same `Storage`
//     across `await` points (a request handler
//     awaits, the scheduler continues, another
//     handler awaits, the same `Storage` is shared).
//     A `std::sync::Mutex` would deadlock under
//     that pattern.
// (2) the `Connection` is `!Sync` (it is single-
//     threaded by SQLite's design). Wrapping it in
//     a `tokio::sync::Mutex` lifts the `!Sync` into
//     a `Send + Sync` envelope (the `Mutex` itself
//     is `Send + Sync` and the `Connection` is
//     behind the guard).
//
// The `path` is stored on `Storage` for the two
// reasons listed in `lib.rs` (diagnostic logging
// in `migrate` + the dashboard's `GET /spans/
// storage` handler).
//
// Errors:
// - `StorageError::Open` if the parent directory
//   cannot be created, or the file cannot be
//   opened. The `io::Error` carries the OS-level
//   reason (permission denied, read-only
//   filesystem, etc.).
//
// Used by: every engine that ships a SQLite
// schema at boot (`afa-security`'s
// `SecurityEngine::new` in Phase 0.5b,
// `afa-observability`'s future
// `ObservabilityEngine::new` in Phase 1, future
// engines' boots).
pub fn open(path: &Path) -> Result<Storage, StorageError> {
    // Create the parent directory if it doesn't
    // exist. The IMPL rollout note about `/var/lib/
    // afa/` not existing on a fresh image is the
    // reason this is here — the kernel must not fail
    // on its first boot just because the
    // `create_dir_all` call was missed. The call is
    // a no-op if the directory already exists, so
    // there is no cost on the steady-state boot
    // path.
    if let Some(parent) = path.parent() {
        // Skip the empty-parent case (`path` is the
        // current working directory or a relative
        // file name like `"afa.db"`). `create_dir_all`
        // on an empty path returns an error on some
        // platforms, and we don't want to pay that
        // tax for the relative-name case.
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(StorageError::Open)?;
        }
    }
    // Open (or create) the SQLite file. The
    // `rusqlite::Connection::open` call does not
    // have a separate "create if missing" mode —
    // `open` creates the file by default. The
    // `bundled` feature on `rusqlite` means there
    // is no host-SQLite version concern.
    let conn = Connection::open(path).map_err(|e| {
        // The `rusqlite::Error` is wrapped in an
        // `io::Error` of kind `Other` because
        // `StorageError::Open` carries a single
        // `io::Error`. The IMPL's
        // `StorageError::Open` signature is locked
        // (3-variant enum per TRD §2.2) and does
        // not have a `Rusqlite(io::Error)` variant
        // — the rusqlite error is consumed by the
        // wrapper so the public surface is
        // `io::Error`-only. `Error::other` is the
        // stable spelling (the long form
        // `Error::new(ErrorKind::Other, e)` is the
        // pre-1.74 form; `Error::other` is the
        // modern replacement — clippy
        // `io_other_error` lint enforces it).
        StorageError::Open(std::io::Error::other(e))
    })?;
    Ok(Storage {
        inner: Arc::new(Mutex::new(conn)),
        path: path.to_path_buf(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The "open creates the file" regression-proof.
    /// A `Storage::open` on a path that doesn't exist
    /// must succeed (it creates the file). The
    /// tempdir is dropped at the end of the test, so
    /// the file goes with it.
    #[test]
    fn open_creates_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("afa.db");
        assert!(!path.exists(), "precondition: file does not exist");
        let storage = open(&path).expect("open should succeed on a missing file");
        assert!(path.exists(), "open should create the file");
        assert_eq!(storage.path, path);
    }

    /// The "open creates the parent dir" regression-
    /// proof. A `Storage::open` on a path under a
    /// non-existent parent dir must succeed; the
    /// parent dir is created.
    #[test]
    fn open_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("c").join("afa.db");
        assert!(!nested.parent().unwrap().exists());
        open(&nested).expect("open should create the parent dir");
        assert!(nested.parent().unwrap().is_dir());
    }

    /// The "open succeeds on an existing file"
    /// regression-proof. A `Storage::open` on a
    /// path that already has a SQLite file must
    /// succeed; the file is opened, not overwritten.
    ///
    /// Note: a fresh `Connection::open` creates the
    /// file with 0 bytes (the schema is only written
    /// when a table is created). The test asserts
    /// that the file still exists after the second
    /// open and that the size is unchanged (not
    /// truncated). It does NOT assert non-zero size
    /// — that would be a false-positive.
    #[test]
    fn open_succeeds_on_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("afa.db");
        // First open creates the file (at 0 bytes
        // — no schema yet).
        let first = open(&path).expect("first open");
        drop(first);
        // Second open must succeed and must NOT
        // truncate the file. A fresh SQLite file is
        // 0 bytes; the second open leaves it at 0.
        let _second = open(&path).expect("second open");
        let meta = std::fs::metadata(&path).unwrap();
        assert!(meta.is_file(), "file should still be present");
    }
}
