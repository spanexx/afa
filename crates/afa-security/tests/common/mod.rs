//! Code Map: Shared test helpers for `crates/afa-security`
//! integration tests
//! - `test_key()`: A fixed 32-byte master key for tests
//!   (deterministic — every test that uses it gets the
//!   same AEAD output for the same input).
//! - `new_test_db_path()`: A fresh `secrets.db` path
//!   inside a per-test tempdir, so parallel tests do
//!   not race on a shared file.
//! - `new_engine_with_bus()`: A `SecurityEngine`
//!   wired to a fresh `EventBus` and a fresh
//!   `SealedSecretStore` on the test's tempdir. The
//!   returned tuple also hands the caller a clone of
//!   the `Arc<EventBus>` so the test can
//!   `bus.subscribe()` without reaching through the
//!   engine.
//! - `ctx_for()`: An `ExecutionContext` for a given
//!   tenant + actor, ready to pass to `unseal` /
//!   `rotate`.
//!
//! Story (plain English): The shared desk the security
//! tests all sit at. The tests are different customers
//! (a "rotate" customer, a "concurrent rotate" customer,
//! a "audit-event" customer) but they all need the same
//! desk: a fresh desk (a fresh `EventBus` so a
//! subscription in one test never sees events from
//! another), a fresh notepad (a fresh
//! `SealedSecretStore` on a fresh tempdir so two
//! tests cannot clobber each other's audit trail),
//! and a fresh key (a deterministic 32-byte master
//! key so the AEAD output is reproducible for any
//! test failure).
//!
//! CID Index:
//! CID:afa-security-test-common-001 -> test_key
//! CID:afa-security-test-common-002 -> new_test_db_path
//! CID:afa-security-test-common-003 -> new_engine_with_bus
//! CID:afa-security-test-common-004 -> ctx_for
//!
//! Quick lookup: rg -n "CID:afa-security-test-common-" crates/afa-security/tests/common/mod.rs

use afa_bus::EventBus;
use afa_contracts::{Actor, ExecutionContext, TenantId};
use afa_security::open_storage;
use afa_security::{MasterKey, SecurityEngine};
use std::path::PathBuf;
use std::sync::Arc;
use tempfile::TempDir;
use zeroize::Zeroizing;

// CID:afa-security-test-common-001 - test_key
// Purpose: A fixed 32-byte master key every test uses
// (so the AEAD output is reproducible for any
// failure — important when the test report needs to
// be diffed against a golden file). Not secret
// (test-only, never touches production), so the
// `0xA5u8` pattern is fine.
// Used by: every test file in this crate.
pub fn test_key() -> MasterKey {
    // The newtype's `From<[u8; 32]>` impl wraps the
    // raw bytes; the temporary `Zeroizing` here
    // guarantees the `0xA5` bytes are wiped on drop
    // even if a future refactor accidentally drops
    // the `MasterKey` newtype's wipe-on-Drop
    // guarantee.
    let raw = Zeroizing::new([0xA5u8; 32]);
    MasterKey::from(*raw)
}

// CID:afa-security-test-common-002 - new_test_db_path
// Purpose: Build a fresh path for the test's
// `secrets.db` file inside a per-test `TempDir`.
// The `TempDir` is returned too so the test can keep
// it alive for the duration of the test (dropping
// the `TempDir` would delete the file, which would
// race with the engine's open connection on slow
// filesystems — the IMPL's principle #4: "tests
// must hold their tempdirs alive for the test's
// entire scope").
// Used by: every test file in this crate.
pub fn new_test_db_path() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create tempdir");
    let path = dir.path().join("secrets.db");
    (dir, path)
}

// CID:afa-security-test-common-003 - new_engine_with_bus
// Purpose: Build a fresh `SecurityEngine` wired to a
// fresh `EventBus` and a fresh `Storage` (the
// Phase-0.5a re-export of `afa_storage::Storage`,
// formerly a `SealedSecretStore`).
// The bus is also returned (as a separate
// `Arc<EventBus>`) so the test can `subscribe()`
// without reaching through the engine. The
// `TempDir` is returned so the test can keep it
// alive (see the doc comment on `new_test_db_path`).
// Errors: propagates `open_storage` failures (e.g.
// permission denied on the tempdir).
// Used by: every test file in this crate.
pub async fn new_engine_with_bus() -> (TempDir, Arc<EventBus>, SecurityEngine) {
    let (dir, path) = new_test_db_path();
    let bus = Arc::new(EventBus::new());
    let store = open_storage(&path).await.expect("open store");
    let engine = SecurityEngine::new(&test_key(), store, Arc::clone(&bus));
    (dir, bus, engine)
}

// CID:afa-security-test-common-004 - ctx_for
// Purpose: A short helper that builds an
// `ExecutionContext` for the given tenant + actor.
// The tests do this five or six times each, so the
// helper saves a few lines per test and keeps the
// tenant / actor names consistent across tests
// (`"rotate-test"`, `"concurrent-test"`, etc.).
// Used by: `rotate_invalidates_old`,
// `concurrent_rotate`, `audit_event_shape`.
pub fn ctx_for(tenant: &str, actor: Actor) -> ExecutionContext {
    ExecutionContext::new(TenantId::new(tenant), actor)
}
