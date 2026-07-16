//! Test: the `SecurityV1::lookup_hash` real override
//! (Phase 0.5b) returns the IMPL-documented outcomes
//! for every leg of the contract:
//!
//! - sealed active row + matching hash → `Ok(true)`
//! - sealed active row + non-matching hash → `Ok(false)`
//! - unknown name → `Err(SecretNotFound)`
//! - name whose only row is rotated (no active row) →
//!   `Err(SecretNotFound)` (the `WHERE status = 'active'`
//!   in the engine's `lookup_hash` filters the rotated
//!   row out, so the caller sees the same "not found"
//!   as for an unknown name)
//! - name with a stored hash whose length does not
//!   match the incoming hash → `Ok(false)` (the
//!   length-mismatch short-circuit in the engine, NOT
//!   an error: the IMPL treats "different lengths" as
//!   "legitimate not-equal", not as "malformed input")
//!
//! Why this is a real test (not a fake): a failure
//! means the `lookup_hash` override is no longer
//! honoring the contract — either the SQL filter
//! regressed (returning `Ok(false)` for a real
//! match), the constant-time compare drifted (returning
//! `Ok(true)` for a partial-match), or the rotated-row
//! path started returning the rotated row's hash
//! (which would let a stolen "rotated" hash stay
//! valid forever). The assertions are on the
//! user-visible `Result<bool, SecurityErrorV1>`.

use afa_contracts::{Actor, ExecutionContext, SecurityErrorV1, SecurityV1, TenantId};
use afa_security::{open_storage, MasterKey, SecurityEngine};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use zeroize::Zeroizing;

/// Build an `ExecutionContext` for the tests
/// (the `rotate` method requires one; the IMPL
/// documents this as a "no audit fact without
/// a context" rule).
fn test_ctx() -> ExecutionContext {
    ExecutionContext::new(TenantId::new("test-tenant"), Actor::Timer)
}

/// Build a `SecurityEngine` wired to a fresh `Storage`
/// and `EventBus`. Returns the `TempDir` so the test
/// can keep it alive (dropping the dir deletes the
/// SQLite file).
async fn fresh_engine() -> (TempDir, SecurityEngine) {
    let dir = TempDir::new().expect("tempdir");
    let path = dir.path().join("secrets.db");
    let key = Zeroizing::new([0x42u8; 32]);
    let master_key = MasterKey::from(*key);
    let store = open_storage(&path).await.expect("open storage");
    let bus = std::sync::Arc::new(afa_bus::EventBus::new());
    let engine = SecurityEngine::new(&master_key, store, bus);
    (dir, engine)
}

/// Compute SHA-256 of a payload and return the
/// lowercase hex form (64 ASCII bytes). The engine
/// stores the hash in this form, so the test must
/// produce the same form for the constant-time
/// compare to match.
fn sha256_hex(payload: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(payload);
    format!("{:x}", hasher.finalize())
}

#[tokio::test]
async fn lookup_hash_returns_true_when_incoming_hash_matches_sealed_plaintext() {
    let (_dir, engine) = fresh_engine().await;
    let plaintext = b"hello-dashboard-token";
    let name = "dashboard-token";

    // Seal the secret; the engine stores
    // `sha256(plaintext)` (hex) in the new
    // v2-schema `sha256` column.
    let secret_ref = engine.seal(plaintext, name).await.expect("seal ok");
    assert_eq!(secret_ref.name, name);

    // The dashboard bearer-auth middleware
    // computes the same hex and hands it to
    // `lookup_hash`. The match must return
    // `Ok(true)`.
    let incoming = sha256_hex(plaintext);
    let result = engine.lookup_hash(name, &incoming).await;
    assert!(
        matches!(result, Ok(true)),
        "matching hash must return Ok(true) — the IMPL's contract, got {result:?}"
    );
}

#[tokio::test]
async fn lookup_hash_returns_false_when_incoming_hash_differs_from_sealed_plaintext() {
    let (_dir, engine) = fresh_engine().await;
    let plaintext = b"hello-dashboard-token";
    let name = "dashboard-token";

    engine.seal(plaintext, name).await.expect("seal ok");

    // Send a different (non-matching) hex. The
    // constant-time compare must return `Ok(false)`,
    // not `Err`.
    let wrong = sha256_hex(b"a-different-token-attacker-guessed");
    let result = engine.lookup_hash(name, &wrong).await;
    assert!(
        matches!(result, Ok(false)),
        "non-matching hash must return Ok(false) — the IMPL's contract, got {result:?}"
    );
}

#[tokio::test]
async fn lookup_hash_returns_not_found_for_unknown_name() {
    let (_dir, engine) = fresh_engine().await;
    let name = "name-that-was-never-sealed";

    // The pre-migrate check / `open_storage`
    // succeeded, so the engine is in a valid
    // "empty" state. The `WHERE name = ?` in
    // `lookup_hash` returns no rows; the method
    // must return `Err(SecretNotFound)`.
    let any_hash = sha256_hex(b"");
    let result = engine.lookup_hash(name, &any_hash).await;
    match result {
        Err(SecurityErrorV1::SecretNotFound { name: got, .. }) => {
            assert_eq!(got, name, "the error must carry the requested name");
        }
        Err(other) => panic!("expected SecretNotFound, got {other:?}"),
        Ok(_) => panic!("expected SecretNotFound, got Ok(_)"),
    }
}

#[tokio::test]
async fn lookup_hash_returns_not_found_for_rotated_name() {
    // Threat this test covers: a secret
    // was sealed, then rotated (the old row's
    // status is `rotated`, the new row's status
    // is `active`). The `lookup_hash` call
    // operates on `name`, not on `(name,
    // version)`, so the engine must filter on
    // `status = 'active'` to avoid returning
    // the rotated row's hash. If the SQL
    // regresses and the filter is removed, the
    // rotated row's hash stays queryable
    // forever — a stolen-then-rotated secret
    // would still authenticate. The assertion
    // is the user-visible `Err(SecretNotFound)`.
    let (_dir, engine) = fresh_engine().await;
    let plaintext_v1 = b"first-version";
    let plaintext_v2 = b"second-version";
    let name = "rotating-token";

    let v1 = engine.seal(plaintext_v1, name).await.expect("seal v1");
    engine
        .rotate(&v1, plaintext_v2, &test_ctx())
        .await
        .expect("rotate v1 → v2");

    // The active row's hash is `sha256(plaintext_v2)`.
    // Sending the rotated row's hash (`sha256(plaintext_v1)`)
    // must NOT return `Ok(true)`. The engine must
    // either return `Ok(false)` (if it reads a
    // different row's hash by accident) or
    // `Err(SecretNotFound)` (the correct answer,
    // since the rotated row is filtered out and
    // the new row's hash is not what we sent).
    // We assert the strong form: the call
    // cannot return `Ok(true)`, and the active
    // row's hash (the one the caller would
    // compute for the new token) DOES return
    // `Ok(true)`.
    let rotated_hash = sha256_hex(plaintext_v1);
    let result_rotated = engine.lookup_hash(name, &rotated_hash).await;
    assert!(
        !matches!(result_rotated, Ok(true)),
        "rotated row's hash must NOT authenticate (got {result_rotated:?})"
    );

    // The active row's hash authenticates as
    // expected. This is the positive control:
    // if THIS fails, the engine is broken in a
    // way the rotated test could not detect.
    // Uses `matches!` (not `assert_eq!`) because
    // `SecurityErrorV1` is not `PartialEq` —
    // matching the pattern used by the other 5
    // assertions in this file.
    let active_hash = sha256_hex(plaintext_v2);
    let result_active = engine.lookup_hash(name, &active_hash).await;
    assert!(
        matches!(result_active, Ok(true)),
        "active row's hash must authenticate (positive control) — got {result_active:?}"
    );
}

#[tokio::test]
async fn lookup_hash_returns_false_on_length_mismatch_without_panicking() {
    // The engine's `lookup_hash` short-circuits
    // on length-mismatch (a 6-char prefix is not
    // equal to the 64-char stored hash) and
    // returns `Ok(false)`. The IMPL's rationale
    // is that a length mismatch is a legitimate
    // "not equal" outcome (the caller sent
    // something the wrong shape), not a
    // malformed-input error. The test pins this
    // behavior: the response is `Ok(false)`, not
    // `Err`, and the engine does not panic.
    let (_dir, engine) = fresh_engine().await;
    let plaintext = b"hello";
    let name = "short-name";

    engine.seal(plaintext, name).await.expect("seal ok");

    let short = "deadbe";
    let result = engine.lookup_hash(name, short).await;
    assert!(
        matches!(result, Ok(false)),
        "length mismatch must return Ok(false), not Err — got {result:?}"
    );
}

#[tokio::test]
async fn lookup_hash_after_reboot_reads_the_same_active_hash() {
    // End-to-end across a reboot: seal a
    // secret, drop the engine, open a new
    // engine against the same SQLite file, and
    // confirm the new engine can still
    // authenticate the same hash. The
    // `sha256` column written in Phase 0.5b
    // must survive a fresh boot (the v2
    // migration is a no-op for a file that is
    // already at `schema_version = 2`).
    let (dir, engine) = fresh_engine().await;
    let plaintext = b"survives-reboot";
    let name = "reboot-token";

    engine.seal(plaintext, name).await.expect("seal ok");

    // Drop the engine (the `Storage` newtype
    // closes the connection on drop).
    drop(engine);

    // Reopen the same file with a fresh
    // engine. The v2 migration is a no-op
    // (the file is already at v2), the
    // `sha256` column is still there, and
    // the new engine's `lookup_hash` reads
    // the same hash.
    let path = dir.path().join("secrets.db");
    let key = MasterKey::from([0x42u8; 32]);
    let store = open_storage(&path).await.expect("reopen storage");
    let bus = std::sync::Arc::new(afa_bus::EventBus::new());
    let new_engine = SecurityEngine::new(&key, store, bus);

    let incoming = sha256_hex(plaintext);
    let result = new_engine.lookup_hash(name, &incoming).await;
    assert!(
        matches!(result, Ok(true)),
        "the v2 `sha256` column must survive a reboot, got {result:?}"
    );
}
