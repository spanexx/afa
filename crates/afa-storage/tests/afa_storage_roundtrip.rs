//! Code Map: afa-storage end-to-end tests
//! - `open_creates_parent_dir`: a `Storage::open` on
//!   a path under a non-existent parent dir
//!   succeeds; the parent dir is created.
//! - `open_succeeds_on_existing_file`: a
//!   `Storage::open` on an existing SQLite file
//!   succeeds; the file is opened, not overwritten.
//! - `migrate_runs_idempotently`: a `Storage::migrate`
//!   with a 2-migration list succeeds; re-running
//!   the same list is a no-op (no extra
//!   `_afa_migrations` rows).
//! - `migrate_rolls_back_failed_migration`: a
//!   `Storage::migrate` with a 2-migration list
//!   where the 2nd migration has a syntax error
//!   succeeds for migration 1 and fails for
//!   migration 2; the FAILED migration's
//!   `_afa_migrations` row is rolled back (per-
//!   migration transaction semantic).
//! - `with_conn_write_then_read`: opens, migrates
//!   a synthetic table, writes a row via
//!   `with_conn`, reads it back via `with_conn`.
//!
//! Story (plain English): These five tests are the
//! "yes the wrapper does what the doc says"
//! regression-proofs. Each one exercises a single
//! public method (or pair of public methods) and
//! asserts the behaviour the IMPL guarantees: the
//! parent dir gets created, the file doesn't get
//! clobbered, the migration table is idempotent, a
//! failed migration rolls back its own row, and
//! the runtime read/write path works end-to-end.

use afa_contracts::Migration;
use afa_storage::{migrate, open, with_conn, StorageError};
use rusqlite::params;

#[test]
fn open_creates_parent_dir() {
    let dir = tempfile::tempdir().unwrap();
    // A 3-deep path: dir/a/b/c/afa.db. The
    // `dir/a` does not exist when we call `open`,
    // so `create_dir_all` must walk the full chain.
    let path = dir.path().join("a").join("b").join("c").join("afa.db");
    assert!(
        !path.parent().unwrap().exists(),
        "precondition: parent dir missing"
    );
    let storage = open(&path).expect("open should create the parent dir");
    assert!(
        path.parent().unwrap().is_dir(),
        "parent dir should be created"
    );
    assert_eq!(storage.path, path);
}

#[test]
fn open_succeeds_on_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("afa.db");
    // First open creates the file (at 0 bytes —
    // no schema yet, since the schema is only
    // written on the first `migrate` call).
    let first = open(&path).expect("first open");
    drop(first);
    // Capture the file's pre-existing size; the
    // second open must NOT truncate it. A fresh
    // SQLite file is 0 bytes; the second open
    // leaves it at 0.
    let pre_size = std::fs::metadata(&path).unwrap().len();
    // Second open must succeed.
    let _second = open(&path).expect("second open");
    // The file size must not have shrunk.
    let post_size = std::fs::metadata(&path).unwrap().len();
    assert_eq!(
        post_size, pre_size,
        "second open must not truncate or grow the file"
    );
}

/// The "two good migrations, twice in a row" test.
/// The primary regression-proof for the
/// `idempotent` semantic: re-running `migrate` is
/// a no-op (no extra `_afa_migrations` rows, no
/// "table already exists" SQL errors).
#[tokio::test]
async fn migrate_runs_idempotently() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("afa.db");
    let storage = open(&path).expect("open");

    const MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            sql: "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL)",
        },
        Migration {
            version: 2,
            sql: "CREATE INDEX notes_id_idx ON notes(id)",
        },
    ];

    // First run: both migrations apply, both rows
    // are in `_afa_migrations`.
    migrate(&storage, MIGRATIONS).await.expect("first migrate");
    let count_after_first: u32 = with_conn(&storage, |conn| {
        Box::pin(async move {
            conn.query_row("SELECT COUNT(*) FROM _afa_migrations", [], |row| row.get(0))
        })
    })
    .await
    .expect("count after first");
    assert_eq!(count_after_first, 2, "first migrate should add 2 rows");

    // Second run: same list. Both migrations are
    // already applied (version <= current_version),
    // so the loop skips both. No new rows.
    migrate(&storage, MIGRATIONS)
        .await
        .expect("second migrate (no-op)");
    let count_after_second: u32 = with_conn(&storage, |conn| {
        Box::pin(async move {
            conn.query_row("SELECT COUNT(*) FROM _afa_migrations", [], |row| row.get(0))
        })
    })
    .await
    .expect("count after second");
    assert_eq!(
        count_after_second, 2,
        "second migrate should be a no-op (still 2 rows)"
    );
}

/// The "first good, second bad" test. Asserts the
/// per-migration-transaction rollback semantic:
/// the failed migration's `_afa_migrations` row is
/// rolled back, but the previous migration's row
/// stays. Re-running the list re-applies the
/// FAILED migration (and only the failed one).
///
/// Marked `#[tokio::test]` (not `#[test]`) because
/// the post-migrate assertions use `with_conn`,
/// which is `async` and `await`s the
/// `tokio::sync::Mutex` lock. The `migrate` call
/// itself is sync (it uses `blocking_lock`
/// internally — see `migrate::migrate`); only the
/// readback uses the async path.
#[tokio::test]
async fn migrate_rolls_back_failed_migration() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("afa.db");
    let storage = open(&path).expect("open");

    const MIGRATIONS: &[Migration] = &[
        Migration {
            version: 1,
            sql: "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL)",
        },
        Migration {
            // Intentional syntax error: the
            // `BOGUS_KEYWORD` is not valid SQL.
            // The error fires on the
            // `execute_batch` step, inside the
            // per-migration transaction, so the
            // INSERT into `_afa_migrations` is
            // rolled back.
            version: 2,
            sql: "BOGUS_KEYWORD FOR THIS IS NOT VALID SQL",
        },
    ];

    // First run: migration 1 commits, migration 2
    // fails. The `migrate` call returns Err.
    let result = migrate(&storage, MIGRATIONS).await;
    assert!(
        matches!(result, Err(StorageError::Migrate { version: 2, .. })),
        "migrate should return Err(Migrate {{ version: 2, .. }}) for a failed migration 2, got {result:?}"
    );

    // Only migration 1's row is in
    // `_afa_migrations`. Migration 2's row was
    // rolled back (per the per-migration
    // transaction semantic).
    let rows: Vec<u32> = with_conn(&storage, |conn| {
        Box::pin(async move {
            let mut stmt = conn.prepare("SELECT version FROM _afa_migrations ORDER BY version")?;
            let versions: rusqlite::Result<Vec<u32>> =
                stmt.query_map([], |row| row.get(0))?.collect();
            versions
        })
    })
    .await
    .expect("list rows");
    assert_eq!(
        rows,
        vec![1],
        "only migration 1 should have a row; the failed migration 2's row was rolled back"
    );

    // Re-running the list: migration 1 is skipped
    // (already applied); migration 2 is re-attempted
    // (and fails again with the same error). The
    // `_afa_migrations` table is unchanged.
    let result_again = migrate(&storage, MIGRATIONS).await;
    assert!(
        matches!(result_again, Err(StorageError::Migrate { version: 2, .. })),
        "second migrate should also fail on migration 2, got {result_again:?}"
    );
    let rows_after_retry: Vec<u32> = with_conn(&storage, |conn| {
        Box::pin(async move {
            let mut stmt = conn.prepare("SELECT version FROM _afa_migrations ORDER BY version")?;
            let versions: rusqlite::Result<Vec<u32>> =
                stmt.query_map([], |row| row.get(0))?.collect();
            versions
        })
    })
    .await
    .expect("list rows after retry");
    assert_eq!(
        rows_after_retry,
        vec![1],
        "after retry, still only migration 1 has a row"
    );
}

/// The "runtime read+write path" test. The
/// end-to-end "yes the wrapper actually does
/// I/O" proof: open, migrate a synthetic table,
/// write a row via `with_conn`, read it back
/// via `with_conn`. If this test passes, the
/// `tokio::sync::Mutex` integration is correct
/// (the lock is acquired with `.await`, the
/// closure is called with `&mut Connection`,
/// and the returned value bubbles up).
#[tokio::test]
async fn with_conn_write_then_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("afa.db");
    let storage = open(&path).expect("open");

    const MIGRATIONS: &[Migration] = &[Migration {
        version: 1,
        sql: "CREATE TABLE notes (id INTEGER PRIMARY KEY, body TEXT NOT NULL)",
    }];
    migrate(&storage, MIGRATIONS).await.expect("migrate");

    // Write a row via `with_conn`. The closure
    // returns a `Pin<Box<dyn Future + Send>>`
    // (the `with_conn` function signature uses
    // an HRTB with `Pin<Box<...>>` for the
    // future, so we wrap the `async move`
    // block in `Box::pin`). This closure has
    // no `.await`s, so the future is empty and
    // the lock is held only for the sync work
    // duration.
    let inserted_id: i64 = with_conn(&storage, |conn| {
        Box::pin(async move {
            conn.execute("INSERT INTO notes (body) VALUES (?1)", params!["hello"])?;
            Ok::<i64, StorageError>(conn.last_insert_rowid())
        })
    })
    .await
    .expect("write");
    assert_eq!(inserted_id, 1, "first insert should have id=1");

    // Read it back via a second `with_conn` call.
    // This proves the lock is released between
    // calls (a deadlock here would mean the
    // `tokio::sync::Mutex` integration is broken).
    let body: String = with_conn(&storage, |conn| {
        Box::pin(async move {
            conn.query_row(
                "SELECT body FROM notes WHERE id = ?1",
                params![inserted_id],
                |row| row.get(0),
            )
        })
    })
    .await
    .expect("read");
    assert_eq!(body, "hello", "round-trip should match");

    // A second write should also succeed (proves
    // the lock is fully released after the read,
    // and the connection is not in a "half-broken"
    // state).
    let inserted_id_2: i64 = with_conn(&storage, |conn| {
        Box::pin(async move {
            conn.execute("INSERT INTO notes (body) VALUES (?1)", params!["world"])?;
            Ok::<i64, StorageError>(conn.last_insert_rowid())
        })
    })
    .await
    .expect("second write");
    assert_eq!(inserted_id_2, 2, "second insert should have id=2");
}
