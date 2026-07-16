//! Code Map: afa-storage::with_conn
//! - `with_conn`: The "lend me the `Connection`
//!   for the duration of an `async` closure, behind
//!   an async lock" helper. The runtime path. Used
//!   by every engine's read and write methods (the
//!   `seal` / `unseal` / `rotate` / `lookup_hash`
//!   paths in `afa-security`, the `record_span` /
//!   `run_purge_loop` paths in `afa-observability`,
//!   etc.).
//!
//! Story (plain English): Imagine the librarian is
//! at a reading desk and the reader (a request
//! handler) wants to look at one card. The reader
//! says "let me see the card for `secrets.db`",
//! and the librarian hands over the drawer for as
//! long as the reader needs to write a note on
//! the card. While the drawer is out, no one else
//! can read it (the `tokio::sync::Mutex` enforces
//! that — it would deadlock to let two readers
//! have the drawer at the same time). When the
//! reader hands the drawer back, the librarian
//! files any new notes the reader made and moves
//! on. The "lend the drawer for one operation" is
//! `with_conn`; the "no two at once" is the
//! `Mutex`; the "file any new notes" is the
//! closure's commit (or rollback on error).
//!
//! Why an `async` closure (and not a sync one)?
//! The `afa-security` engine's `seal` flow needs
//! to hold a **single** transaction across (a)
//! read `MAX(version)`, (b) compute the AEAD
//! ciphertext (CPU, fast), and (c) `INSERT` the
//! new row. If `with_conn` were sync-only, the
//! engine would have to either (i) read the
//! version in one transaction and `INSERT` in a
//! second (race condition: two parallel `seal`
//! calls would both read 0 and both try to
//! `INSERT` (name, 1), with the second failing
//! the unique constraint), or (ii) encrypt
//! before opening the transaction (but the
//! ciphertext's AAD includes the version, which
//! is not yet known). The async closure lets the
//! engine hold one transaction across all three
//! steps, preserving the `BEGIN IMMEDIATE`
//! atomicity the original code relied on.
//!
//! **Doc drift correction #5 vs. the IMPL draft**:
//! the IMPL's example used a sync `with_conn`
//! and a `seal_secret` helper that took a
//! `current_version` argument. That design
//! re-introduces the race and would break the
//! existing `concurrent_rotate.rs` test (which
//! asserts all 16 parallel rotates succeed).
//! The async-closure design preserves the
//! atomicity.
//!
//! CID Index:
//! CID:afa-storage-with-conn-001 -> with_conn
//!
//! Quick lookup: rg -n "CID:afa-storage-with-conn-" crates/afa-storage/src/with_conn.rs

use crate::Storage;
use afa_contracts::StorageError;
use rusqlite::Connection;
use std::future::Future;
use std::pin::Pin;

// CID:afa-storage-with-conn-001 - with_conn
// Purpose: Run the supplied **async** closure
// against the inner `Connection`, holding the
// `tokio::sync::Mutex` lock for the duration of
// the future. The closure receives a
// `&mut Connection` (the rusqlite connection
// handle, which is `Send` but `!Sync`) and may
// `.await` inside the transaction — the lock is
// held across the `.await` (this is what
// `tokio::sync::Mutex` exists for, as opposed
// to `std::sync::Mutex` which would deadlock
// the runtime).
//
// The closure returns a
// `Pin<Box<dyn Future<Output = rusqlite::Result<T>> + Send + 'a>>` —
// a heap-allocated, `Send`, lifetime-bound future.
// The HRTB (`for<'a>`) on `F` says the closure
// works for any borrow lifetime `'a`; the
// `Pin<Box<...>>` is what makes the lifetime
// bound expressible in stable Rust (the
// `AsyncFnOnce` trait that would express this
// directly is unstable as of 2026-07). The
// `Box::pin` allocation per call is the cost of
// using the stable future API; the alternative
// is to wait for `AsyncFnOnce` to stabilize.
//
// A simple sync-only caller writes the closure
// as `|conn| Box::pin(async move {
// conn.execute(...)?; Ok(()) })` — the future is
// empty (no `.await`), so the lock is held only
// for the sync work duration, identical to a
// sync-closure `with_conn` would be.
//
// Errors: any `rusqlite::Error` returned by the
// closure is wrapped in
// `StorageError::Migrate { version: 0, source }`
// (the `version: 0` is a placeholder; the
// per-migration semantic doesn't apply to
// `with_conn`).
//
// Used by: every engine that needs to read or
// write a row in the SQLite file at runtime.
// The afa-security engine uses it for `seal`,
// `unseal`, `rotate`, and the new `lookup_hash`.
// The afa-observability engine (Phase 1) will
// use it for `record_span` and the retention
// purge loop.
//
// The IMPL's draft promised a
// `debug_assert!(!in_async_block())` catch for
// any caller that holds the lock across an
// `.await`. That is **not** implemented here:
// the `tokio::sync::MutexGuard` is `!Send` (not
// `Send`), so holding it across an `.await`
// is already a compile error, not a runtime
// check. The `debug_assert!` is belt-and-
// braces dead code; dropping it.
pub async fn with_conn<F, E, T>(storage: &Storage, f: F) -> Result<T, StorageError>
where
    // HRTB on `F`: the closure must work for any
    // borrow lifetime `'a`. This is what makes
    // the closure-callable with any
    // `&'a mut Connection` (the `'a` is chosen
    // by the caller — inside `with_conn`, the
    // borrow is to the local `guard`, with
    // lifetime equal to the function body).
    F: for<'a> FnOnce(
        &'a mut Connection,
    ) -> Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>,
    // The closure's error type `E` is anything
    // that converts to `StorageError`. The most
    // common case is `E = rusqlite::Error` (the
    // engine returns `rusqlite::Error` directly
    // from the closure; we already have a
    // mapping for that in the
    // `StorageError::Migrate { source: e }`
    // wrapping below). The second most common
    // case is `E = SecurityErrorV1` (the
    // afa-security engine returns its own
    // error from the closure and the kernel
    // wraps the `StorageError` from `with_conn`
    // in a `SecurityError::StorageCorrupted`).
    // The `Into<StorageError>` bound lets
    // each engine pick its own error type
    // without coupling `afa-storage` to any
    // specific engine's error type.
    E: Into<StorageError> + Send + 'static,
    T: Send + 'static,
{
    // The lock is `lock().await` (the async
    // variant). The `MutexGuard` is held for
    // the duration of the future; when the
    // future completes (or returns an error),
    // the guard is dropped (the lock is
    // released). If the future `.await`s, the
    // lock is held across the `.await` (this
    // is the `tokio::sync::Mutex` semantic).
    let mut guard = storage.inner.lock().await;
    // `&mut guard` re-borrows the inner
    // `Connection` as `&mut Connection` via
    // the `DerefMut` impl on
    // `tokio::sync::MutexGuard` (clippy
    // `explicit_auto_deref` prefers the auto-
    // deref form). The HRTB on `F` allows the
    // call to succeed: `f` accepts any
    // `&'a mut Connection`, and the borrow
    // here has lifetime equal to the function
    // body (the guard's drop scope).
    match f(&mut guard).await {
        Ok(value) => Ok(value),
        Err(e) => Err(e.into()),
    }
}
