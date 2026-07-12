//! Code Map: Boot-failure integration tests for the
//! security engine + the `Kernel::new` boot path.
//!
//! - `read_master_key_from_env`: The reference
//!   env-var reader for the future
//!   `bin/afa.rs` `main` function. Reads
//!   `AFA_MASTER_KEY` from the process env; if
//!   unset returns `MasterKeyMissing`; if not
//!   64 hex chars or non-hex returns
//!   `MasterKeyMalformed { reason }` with one of
//!   the four stable reason strings
//!   (`"odd length"`, `"too short"`, `"too long"`,
//!   `"non-hex character"`); on success returns
//!   a `MasterKey` (the type-safe envelope around
//!   the 32 raw bytes).
//! - `read_db_path_from_env`: The companion
//!   env-var reader for the `AFA_SECRETS_DB_PATH`
//!   env var. Returns the configured path, or
//!   `/var/lib/afa/secrets.db` as the default if
//!   the env var is unset or empty.
//! - The E-1 / E-2 / E-3 / E-7 sub-tests: Each
//!   exercises one failure mode the future
//!   `main` must handle cleanly (no panic, no
//!   stack trace, just a typed error the `main`
//!   can log and exit on).
//!
//! Story (plain English): Imagine the very first
//! minute of a brand-new deployment. The
//! operator runs the binary, and the binary has
//! to figure out (1) what key to lock the
//! safe with, and (2) where the index card
//! file lives. If the operator forgot to set
//! the env var, or set it to a typo, or pointed
//! the file at a directory that does not exist
//! and cannot be created, the binary must say
//! so politely and refuse to start. These four
//! tests are the rehearsal for those four
//! "polite refusal" paths. The other error
//! modes (the AEAD tag check failing, a row
//! marked `rotated`, etc.) are exercised by the
//! other test files in this crate — those are
//! runtime errors, not boot errors, so they
//! have a different test surface.
//!
//! CID Index:
//! CID:afa-security-boot-001 -> read_master_key_from_env
//! CID:afa-security-boot-002 -> read_db_path_from_env
//! CID:afa-security-boot-003 -> e1_returns_master_key_missing_when_env_var_is_unset
//! CID:afa-security-boot-004 -> e2_returns_master_key_malformed_when_env_var_is_malformed
//! CID:afa-security-boot-005 -> e3_returns_schema_version_mismatch_when_existing_file_has_wrong_version
//! CID:afa-security-boot-006 -> e7_returns_storage_unreachable_when_parent_dir_cannot_be_created
//!
//! Quick lookup: rg -n "CID:afa-security-boot-" crates/afa-security/tests/boot_failures.rs

use afa_contracts::SecurityErrorV1;
use afa_security::{MasterKey, SealedSecretStore};
use rusqlite::Connection;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Process-wide serial gate for the env-var-mutating
/// sub-tests in this file. `std::env` is
/// process-global, so two tests in this binary
/// that both call `std::env::set_var` would
/// otherwise race each other and see the wrong
/// value mid-call. The gate is acquired before
/// every env-var mutation and held for the
/// duration of the closure.
static ENV_GATE: Mutex<()> = Mutex::new(());

/// Canonical env-var names. The future `bin/afa.rs`
/// `main` function is expected to use the exact same
/// names; tests in this file mutate the process env
/// with these same constants, so the boot path and
/// the test path cannot drift.
pub const AFA_MASTER_KEY_ENV: &str = "AFA_MASTER_KEY";
/// Canonical env-var name for the secrets database
/// file path.
pub const AFA_SECRETS_DB_PATH_ENV: &str = "AFA_SECRETS_DB_PATH";
/// Default path the kernel uses if
/// `AFA_SECRETS_DB_PATH` is unset or empty. The
/// `bin/afa.rs` `main` is expected to ship a
/// `~/.config/afa/secrets.db` per-user override at
/// a later pack, but for the v1 the path is a
/// single canonical system path.
pub const DEFAULT_SECRETS_DB_PATH: &str = "/var/lib/afa/secrets.db";

// CID:afa-security-boot-001 - read_master_key_from_env
// Purpose: The reference env-var reader for
// `AFA_MASTER_KEY`. Returns a `MasterKey` (the
// type-safe envelope around the 32 raw bytes)
// or one of three stable `SecurityErrorV1`
// variants. The future `bin/afa.rs` `main` is
// expected to copy this function verbatim.
// Errors: `MasterKeyMissing` (env var unset or
// empty), `MasterKeyMalformed { reason }` with
// one of the four stable reason strings the
// dashboard maps to a one-line operator hint
// (`"odd length"`, `"too short"`, `"too long"`,
// `"non-hex character"`). The reasons are kept
// as `&'static str` so they show up verbatim in
// the operator's log line and in the dashboard
// without an allocation.
// Used by: the future `bin/afa.rs` `main`
// (copy-paste); the `Kernel::new`-and-reject path
// is the production caller.
pub fn read_master_key_from_env() -> Result<MasterKey, SecurityErrorV1> {
    // Step 1: read the env var. An unset env var
    // and an explicitly-empty env var are
    // semantically the same thing (the operator
    // has not provided a key), so both fall
    // through to `MasterKeyMissing`. We do NOT
    // try to be clever about whitespace here —
    // a key with leading or trailing whitespace
    // is a malformed key (the hex decoder will
    // reject the non-hex space character with
    // `non-hex character`).
    let hex_value = std::env::var(AFA_MASTER_KEY_ENV)
        .ok()
        .filter(|s| !s.is_empty())
        .ok_or(SecurityErrorV1::MasterKeyMissing)?;

    // Step 2: decode. `MasterKey::from_hex`
    // enforces the length gate, the hex-character
    // gate, and the wipe-on-drop envelope. We do
    // NOT touch the env-var string after this
    // call returns so the env-var buffer can be
    // reaped on the next env-mutation (or, more
    // realistically, on process exit).
    MasterKey::from_hex(&hex_value)
}

// CID:afa-security-boot-002 - read_db_path_from_env
// Purpose: The reference env-var reader for
// `AFA_SECRETS_DB_PATH`. Returns the configured
// path, or the canonical default
// (`/var/lib/afa/secrets.db`) if the env var
// is unset or empty. Does NOT return an error
// — a missing env var is a *configuration
// choice* (the operator wants the default), not
// a boot failure. The "the path you chose
// cannot be opened" check happens later, in
// `Kernel::new` → `SealedSecretStore::open_or_create`.
// Used by: the future `bin/afa.rs` `main`
// (copy-paste).
pub fn read_db_path_from_env() -> PathBuf {
    match std::env::var(AFA_SECRETS_DB_PATH_ENV) {
        Ok(s) if !s.is_empty() => PathBuf::from(s),
        // Unset OR empty → use the default. We
        // do NOT distinguish the two cases; both
        // mean "the operator has not expressed
        // a preference."
        _ => PathBuf::from(DEFAULT_SECRETS_DB_PATH),
    }
}

/// Test helper: set the two boot env vars for
/// the duration of `f`, restoring the previous
/// values (or unsetting them) afterwards. Holds
/// the process-wide `ENV_GATE` lock for the
/// whole call so a parallel test in this binary
/// cannot race the env-var mutation.
fn with_env<F, R>(master_key: Option<&str>, db_path: Option<&str>, f: F) -> R
where
    F: FnOnce() -> R,
{
    let _gate = ENV_GATE.lock().unwrap_or_else(|e| e.into_inner());
    let prev_master = std::env::var(AFA_MASTER_KEY_ENV).ok();
    let prev_db = std::env::var(AFA_SECRETS_DB_PATH_ENV).ok();
    match master_key {
        Some(v) => std::env::set_var(AFA_MASTER_KEY_ENV, v),
        None => std::env::remove_var(AFA_MASTER_KEY_ENV),
    }
    match db_path {
        Some(v) => std::env::set_var(AFA_SECRETS_DB_PATH_ENV, v),
        None => std::env::remove_var(AFA_SECRETS_DB_PATH_ENV),
    }
    let result = f();
    match prev_master {
        Some(v) => std::env::set_var(AFA_MASTER_KEY_ENV, v),
        None => std::env::remove_var(AFA_MASTER_KEY_ENV),
    }
    match prev_db {
        Some(v) => std::env::set_var(AFA_SECRETS_DB_PATH_ENV, v),
        None => std::env::remove_var(AFA_SECRETS_DB_PATH_ENV),
    }
    result
}

/// A valid 64-char hex string (all `0xA5`
/// bytes, matching the rest of the test
/// suite's deterministic key).
const VALID_HEX: &str = "a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5";

// CID:afa-security-boot-003 - e1_returns_master_key_missing_when_env_var_is_unset
// Purpose: Confirms the env-var reader rejects
// an unset `AFA_MASTER_KEY` with the stable
// `MasterKeyMissing` variant. This is the
// most common operator mistake on day 0;
// the dashboard surfaces this exact reason
// string in the "you forgot to set the
// env var" hint.
#[test]
fn e1_returns_master_key_missing_when_env_var_is_unset() {
    with_env(None, None, || {
        let result = read_master_key_from_env();
        match result {
            Err(SecurityErrorV1::MasterKeyMissing) => {}
            other => panic!("expected MasterKeyMissing, got {other:?}"),
        }
    });
}

#[test]
fn e1_also_returns_master_key_missing_when_env_var_is_empty() {
    // The IMPL's E-1 sub-case: the env var IS
    // set, but to an empty string. Semantically
    // equivalent to "the operator has not
    // provided a key" — same error, same log
    // line, same exit code.
    with_env(Some(""), None, || {
        let result = read_master_key_from_env();
        match result {
            Err(SecurityErrorV1::MasterKeyMissing) => {}
            other => panic!("expected MasterKeyMissing, got {other:?}"),
        }
    });
}

// CID:afa-security-boot-004 - e2_returns_master_key_malformed_when_env_var_is_malformed
// Purpose: Confirms the env-var reader rejects
// a malformed `AFA_MASTER_KEY` with the stable
// `MasterKeyMalformed { reason }` variant and
// one of the four stable reason strings the
// dashboard maps to a one-line operator hint.
#[test]
fn e2_returns_master_key_malformed_with_odd_length_reason() {
    with_env(Some("abc"), None, || match read_master_key_from_env() {
        Err(SecurityErrorV1::MasterKeyMalformed { reason }) => {
            assert_eq!(reason, "odd length");
        }
        other => panic!("expected MasterKeyMalformed(odd length), got {other:?}"),
    });
}

#[test]
fn e2_returns_master_key_malformed_with_too_short_reason() {
    let too_short = "a".repeat(62);
    with_env(Some(&too_short), None, || {
        match read_master_key_from_env() {
            Err(SecurityErrorV1::MasterKeyMalformed { reason }) => {
                assert_eq!(reason, "too short");
            }
            other => panic!("expected MasterKeyMalformed(too short), got {other:?}"),
        }
    });
}

#[test]
fn e2_returns_master_key_malformed_with_too_long_reason() {
    let too_long = "a".repeat(66);
    with_env(Some(&too_long), None, || match read_master_key_from_env() {
        Err(SecurityErrorV1::MasterKeyMalformed { reason }) => {
            assert_eq!(reason, "too long");
        }
        other => panic!("expected MasterKeyMalformed(too long), got {other:?}"),
    });
}

#[test]
fn e2_returns_master_key_malformed_with_non_hex_character_reason() {
    // 64 chars total, but the last two are `zz`
    // (not hex digits) — the length gate passes,
    // the hex decoder rejects it.
    let mut bad = String::from(VALID_HEX);
    bad.replace_range(62..64, "zz");
    with_env(Some(&bad), None, || match read_master_key_from_env() {
        Err(SecurityErrorV1::MasterKeyMalformed { reason }) => {
            assert_eq!(reason, "non-hex character");
        }
        other => {
            panic!("expected MasterKeyMalformed(non-hex character), got {other:?}")
        }
    });
}

#[test]
fn e2_happy_path_returns_a_usable_master_key() {
    // Sanity: a well-formed env var round-trips
    // into a `MasterKey` whose 32 bytes match
    // the expected `0xA5` pattern.
    with_env(Some(VALID_HEX), None, || {
        let key = read_master_key_from_env().expect("valid hex should decode");
        let raw: [u8; 32] = key.into();
        assert_eq!(raw, [0xA5u8; 32]);
    });
}

// CID:afa-security-boot-005 - e3_returns_schema_version_mismatch_when_existing_file_has_wrong_version
// Purpose: Confirms the `SealedSecretStore::open_or_create`
// boot path rejects an existing SQLite file
// whose `schema_version` does not match the
// version this engine supports. This is the
// "you restored an old secrets.db" footgun —
// the engine must fail fast at boot with a
// typed error rather than silently
// misinterpreting the rows.
#[test]
fn e3_returns_schema_version_mismatch_when_existing_file_has_wrong_version() {
    // Build a fresh tempdir with a hand-crafted
    // `secrets.db` that has the right table
    // shape but the wrong `schema_version`.
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("secrets.db");
    {
        let conn = Connection::open(&db_path).expect("open db");
        // The schema is the same as
        // `SealedSecretStore::open_or_create` would
        // create, EXCEPT the `schema_version` row
        // is pre-populated with `99` (a "future"
        // version this engine cannot read).
        conn.execute_batch(
            r#"
            CREATE TABLE sealed_secrets (
                name        TEXT NOT NULL,
                version     INTEGER NOT NULL,
                status      TEXT NOT NULL,
                nonce       BLOB NOT NULL,
                ciphertext  BLOB NOT NULL,
                created_at  TEXT NOT NULL,
                PRIMARY KEY (name, version)
            );
            CREATE TABLE afa_security_meta (
                key   TEXT PRIMARY KEY,
                value TEXT NOT NULL
            );
            INSERT INTO afa_security_meta (key, value)
                VALUES ('schema_version', '99');
            "#,
        )
        .expect("create schema");
    }
    // The next call should reject the file with
    // `SchemaVersionMismatch { found: 99,
    // expected: 1 }` (the current engine's
    // `SCHEMA_VERSION` is `1`).
    let result = SealedSecretStore::open_or_create(&db_path);
    match result {
        Err(SecurityErrorV1::SchemaVersionMismatch { found, expected }) => {
            assert_eq!(found, 99);
            assert_eq!(expected, 1);
        }
        Err(other) => panic!("expected SchemaVersionMismatch, got {other:?}"),
        Ok(_) => panic!("expected SchemaVersionMismatch, got Ok(...)"),
    }
}

// CID:afa-security-boot-006 - e7_returns_storage_unreachable_when_parent_dir_cannot_be_created
// Purpose: Confirms the `SealedSecretStore::open_or_create`
// boot path rejects a path whose parent
// directory does not exist and cannot be
// created (e.g. a read-only mount, a path
// through a regular file as a directory
// component). The operator sees a clear
// `StorageUnreachable` error and the
// dashboard surfaces the embedded
// `reason` string.
#[test]
fn e7_returns_storage_unreachable_when_parent_dir_cannot_be_created() {
    // Build a parent that is a regular file,
    // so any attempt to `mkdir` underneath it
    // fails. This is portable across Linux +
    // macOS (CI runs on Linux); the
    // `create_dir_all` call inside
    // `SealedSecretStore::open_or_create`
    // will fail with `NotADirectory`, which
    // we wrap into `StorageUnreachable { ... }`.
    let dir = tempfile::tempdir().expect("tempdir");
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"i am a file, not a directory").expect("write blocker");
    let db_path: PathBuf = blocker.join("under-a-file/secrets.db");

    let result = SealedSecretStore::open_or_create(&db_path);
    match result {
        Err(SecurityErrorV1::StorageUnreachable { reason }) => {
            // The reason must be non-empty (the
            // dashboard surfaces it verbatim as
            // the "what to fix" hint).
            assert!(!reason.is_empty());
            // The reason must mention either the
            // parent dir or the underlying error.
            // We don't pin the exact wording (the
            // OS error string varies between
            // Linux and macOS) — we just pin
            // that it is *not* an empty string.
        }
        Err(other) => panic!("expected StorageUnreachable, got {other:?}"),
        Ok(_) => panic!("expected StorageUnreachable, got Ok(...)"),
    }
}

#[test]
fn read_db_path_from_env_returns_the_configured_path_when_set() {
    with_env(None, Some("/tmp/my-secrets.db"), || {
        let path = read_db_path_from_env();
        assert_eq!(path, Path::new("/tmp/my-secrets.db"));
    });
}

#[test]
fn read_db_path_from_env_returns_the_default_when_unset() {
    with_env(None, None, || {
        let path = read_db_path_from_env();
        assert_eq!(path, Path::new(DEFAULT_SECRETS_DB_PATH));
    });
}

#[test]
fn read_db_path_from_env_returns_the_default_when_empty() {
    with_env(None, Some(""), || {
        let path = read_db_path_from_env();
        assert_eq!(path, Path::new(DEFAULT_SECRETS_DB_PATH));
    });
}
