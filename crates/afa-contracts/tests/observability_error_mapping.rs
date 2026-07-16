//! Code Map: ObservabilityErrorV1 + StorageError -> AfaErrorKind mapping
//! - 4 tests, one per `ObservabilityErrorV1` variant, each
//!   asserts the variant maps to the correct
//!   `AfaErrorKind`. The mapping is the contract every
//!   caller depends on (a `match` on `kind()` in a generic
//!   error handler must produce the right HTTP status /
//!   retry behaviour / log severity for each variant).
//! - 3 tests, one per `StorageError` variant, asserting
//!   the same mapping.
//! - 1 closure test that asserts the closed set is
//!   intact — no variant maps to a non-`AfaErrorKind`
//!   value.
//!
//! Story (plain English): The observability engine's
//! "what went wrong?" list has four entries; the storage
//! engine's "what went wrong opening the SQLite file?"
//! list has three. Both are folded into the kernel's
//! six coarse buckets ("not found", "not allowed",
//! "service down", "too slow", "not supported", "weird
//! internal error") so a generic error handler in the
//! dashboard can decide the HTTP status / retry
//! behaviour / log severity without naming the concrete
//! type. The mapping is the dictionary the rest of the
//! kernel relies on. This file's eight tiny tests are the
//! regression-proof for that dictionary — if a future
//! pack renames a variant or moves it to a different
//! bucket, the right test fails first.

use afa_contracts::{AfaError, AfaErrorKind, ObservabilityErrorV1, SecurityErrorV1, SecurityV1};
use async_trait::async_trait;

// CID:observability-error-mapping-001 - observability variants
#[test]
fn storage_unreachable_maps_to_unavailable() {
    let e = ObservabilityErrorV1::StorageUnreachable {
        reason: "spans DB parent dir not writable".into(),
    };
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn storage_corrupted_maps_to_unavailable() {
    let e = ObservabilityErrorV1::StorageCorrupted;
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn schema_version_mismatch_maps_to_unavailable() {
    let e = ObservabilityErrorV1::SchemaVersionMismatch {
        found: 2,
        expected: 1,
    };
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn internal_maps_to_internal() {
    let e = ObservabilityErrorV1::Internal {
        reason: "invariant broken".into(),
    };
    assert_eq!(e.kind(), AfaErrorKind::Internal);
}

// CID:observability-error-mapping-002 - no new AfaErrorKind introduced
#[test]
fn no_new_afa_error_kind_variants_are_introduced() {
    // The contract is: every ObservabilityErrorV1 variant
    // maps to one of the 6 existing AfaErrorKind values.
    // No new kind. This test asserts that the closed set is
    // intact by enumerating the bucket each variant goes
    // to and confirming the result is one of the 6.
    let variants: Vec<AfaErrorKind> = vec![
        ObservabilityErrorV1::StorageUnreachable { reason: "x".into() }.kind(),
        ObservabilityErrorV1::StorageCorrupted.kind(),
        ObservabilityErrorV1::SchemaVersionMismatch {
            found: 1,
            expected: 1,
        }
        .kind(),
        ObservabilityErrorV1::Internal { reason: "x".into() }.kind(),
    ];
    for k in &variants {
        let ok = matches!(
            k,
            AfaErrorKind::NotFound
                | AfaErrorKind::Unauthorized
                | AfaErrorKind::Unavailable
                | AfaErrorKind::Timeout
                | AfaErrorKind::CapabilityUnsupported
                | AfaErrorKind::Internal
        );
        assert!(
            ok,
            "ObservabilityErrorV1 mapped to a non-AfaErrorKind value: {k:?}"
        );
    }
}

// CID:observability-error-mapping-003 - storage error variants
#[test]
fn storage_error_open_maps_to_unavailable() {
    let e = afa_contracts::StorageError::Open(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "no write",
    ));
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn storage_error_migrate_maps_to_unavailable() {
    let e = afa_contracts::StorageError::Migrate {
        version: 1,
        source: rusqlite::Error::QueryReturnedNoRows,
    };
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn storage_error_locked_maps_to_unavailable() {
    let e = afa_contracts::StorageError::Locked;
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

// CID:observability-error-mapping-004 - generic classification
#[test]
fn generic_classification_without_concrete_type() {
    // A function bounded only on `impl AfaError` can
    // branch on `.kind()` without naming the concrete
    // type. This is the regression-proof that the
    // observability errors are usable in a generic
    // handler (e.g. the dashboard's catch-all 500 path
    // that maps `Internal -> 500`, `Unavailable -> 503`).
    fn classify(err: &dyn AfaError) -> &'static str {
        // `AfaErrorKind` is `#[non_exhaustive]`, so a
        // wildcard arm is required even though the kernel
        // currently uses only the 6 listed variants.
        match err.kind() {
            AfaErrorKind::NotFound => "missing",
            AfaErrorKind::Unauthorized => "not_allowed",
            AfaErrorKind::Unavailable => "service_down",
            AfaErrorKind::Timeout => "too_slow",
            AfaErrorKind::CapabilityUnsupported => "not_supported",
            AfaErrorKind::Internal => "broken",
            _ => "other",
        }
    }
    assert_eq!(
        classify(&ObservabilityErrorV1::StorageUnreachable { reason: "x".into() }),
        "service_down"
    );
    assert_eq!(
        classify(&ObservabilityErrorV1::Internal { reason: "x".into() }),
        "broken"
    );
    assert_eq!(
        classify(&afa_contracts::StorageError::Locked),
        "service_down"
    );
}

// CID:observability-error-mapping-005 - lookup_hash default impl
#[test]
fn security_v1_lookup_hash_default_returns_internal() {
    // The default impl in `SecurityV1::lookup_hash` (added
    // in Pack #6 Phase 0) returns `Internal` for any
    // `impl SecurityV1` block that does not override it.
    // The 17 fakes in the workspace rely on this default —
    // if a future change makes the default return `Ok(true)`
    // (e.g. by accident), the bearer-auth middleware in
    // Pack #6 Phase 3 would silently accept arbitrary
    // hashes. This test is the regression-proof that the
    // default is the strict "not implemented" return.
    struct StubSecurity;
    #[async_trait]
    impl SecurityV1 for StubSecurity {
        async fn seal(
            &self,
            _plaintext: &[u8],
            _name: &str,
        ) -> Result<afa_contracts::SecretRef, SecurityErrorV1> {
            unimplemented!()
        }
        async fn unseal(
            &self,
            _secret_ref: &afa_contracts::SecretRef,
            _ctx: &afa_contracts::ExecutionContext,
        ) -> Result<afa_contracts::UnsealedSecret, SecurityErrorV1> {
            unimplemented!()
        }
        async fn rotate(
            &self,
            _secret_ref: &afa_contracts::SecretRef,
            _new_plaintext: &[u8],
            _ctx: &afa_contracts::ExecutionContext,
        ) -> Result<afa_contracts::SecretRef, SecurityErrorV1> {
            unimplemented!()
        }
        // `lookup_hash` is intentionally NOT overridden
        // here — we are testing the default behaviour.
    }
    // The dev-dep `tokio` only enables the `rt` feature
    // (not `rt-multi-thread`), so use
    // `Builder::new_current_thread()` which works with
    // just `rt`. The default `Runtime::new()` requires
    // `rt-multi-thread` and would not compile.
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let result = rt.block_on(StubSecurity.lookup_hash("dashboard-token", "deadbeef"));
    match result {
        Err(SecurityErrorV1::Internal { reason }) => {
            assert!(
                reason.contains("lookup_hash not implemented"),
                "default impl must return the locked 'not implemented' message; got {reason:?}"
            );
        }
        other => panic!("expected Err(Internal), got {other:?}"),
    }
}
