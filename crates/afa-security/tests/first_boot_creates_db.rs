//! Test: `SealedSecretStore::open_or_create` on a path that
//! does not exist creates the SQLite file, runs the
//! idempotent schema, and records `schema_version = 1` in
//! the `afa_security_meta` table. The next call to
//! `open_or_create` on the same path is a no-op (the
//! schema `CREATE TABLE IF NOT EXISTS` and `INSERT OR
//! IGNORE` are idempotent).
//!
//! Why this is a real test (not a fake): a failure means
//! the "first deploy" path is broken — the operator starts
//! the kernel, the kernel tries to open the secrets
//! database, and the database is not there. The assertion
//! is on the on-disk schema state (the file exists, the
//! tables exist, the `schema_version` row is `1`).

use afa_security::SealedSecretStore;
use rusqlite::Connection;
use std::path::PathBuf;
use tempfile::TempDir;

fn db_path(dir: &TempDir, name: &str) -> PathBuf {
    dir.path().join(name)
}

#[test]
fn open_or_create_creates_file_and_schema() {
    let dir = TempDir::new().expect("tempdir");
    let path = db_path(&dir, "secrets.db");
    assert!(!path.exists(), "precondition: file should not exist");

    let _store = SealedSecretStore::open_or_create(&path).expect("open_or_create ok");

    // File was created.
    assert!(path.exists(), "file should be created");

    // Schema was created: open a second connection and
    // assert the tables and the `schema_version` row are
    // there.
    let conn = Connection::open(&path).expect("open as plain sqlite");
    let table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('sealed_secrets', 'afa_security_meta')",
            [],
            |row| row.get(0),
        )
        .expect("count tables");
    assert_eq!(
        table_count, 2,
        "expected sealed_secrets and afa_security_meta tables"
    );

    let schema_version: String = conn
        .query_row(
            "SELECT value FROM afa_security_meta WHERE key = 'schema_version'",
            [],
            |row| row.get(0),
        )
        .expect("read schema_version");
    assert_eq!(schema_version, "1", "schema_version should be 1");
}

#[test]
fn open_or_create_is_idempotent() {
    let dir = TempDir::new().expect("tempdir");
    let path = db_path(&dir, "secrets.db");

    let _store1 = SealedSecretStore::open_or_create(&path).expect("first open ok");
    let _store2 = SealedSecretStore::open_or_create(&path).expect("second open ok");

    // Schema is still the same; no duplicate-table errors.
    let conn = Connection::open(&path).expect("open");
    let table_count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('sealed_secrets', 'afa_security_meta')",
            [],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(table_count, 2);
}

#[test]
fn open_or_create_creates_parent_directories() {
    let dir = TempDir::new().expect("tempdir");
    // Path has a parent that does not exist yet.
    let path = db_path(&dir, "nested/sub/dir/secrets.db");
    assert!(
        !path.parent().unwrap().exists(),
        "precondition: parent missing"
    );

    let _store = SealedSecretStore::open_or_create(&path).expect("open_or_create creates parents");

    assert!(path.exists(), "file should be created with parents");
}

#[test]
fn open_or_create_rejects_wrong_schema_version() {
    let dir = TempDir::new().expect("tempdir");
    let path = db_path(&dir, "secrets.db");

    // First open creates the file with `schema_version = 1`.
    let _store = SealedSecretStore::open_or_create(&path).expect("first open ok");

    // Tamper: rewrite `schema_version` to `99` directly,
    // bypassing the store. This simulates a future pack
    // that ships a v99 schema, or a corrupted file that
    // happens to have the wrong version row.
    {
        let conn = Connection::open(&path).expect("open");
        conn.execute(
            "UPDATE afa_security_meta SET value = '99' WHERE key = 'schema_version'",
            [],
        )
        .expect("tamper");
    }

    // Second open should reject the file with
    // `SchemaVersionMismatch`.
    let result = SealedSecretStore::open_or_create(&path);
    match result {
        Err(afa_contracts::SecurityErrorV1::SchemaVersionMismatch { found, expected }) => {
            assert_eq!(found, 99);
            assert_eq!(expected, 1);
        }
        Err(other) => panic!("expected SchemaVersionMismatch, got {other:?}"),
        Ok(_) => panic!("expected SchemaVersionMismatch, got Ok"),
    }
}
