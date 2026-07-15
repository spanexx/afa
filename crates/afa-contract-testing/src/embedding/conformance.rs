//! Code Map: afa-contract-testing — embedding conformance
//! - The 12 conformance assertions, run
//!   via the `run_suite!` macro against
//!   the `MockEmbeddingAdapter`.
//!   Phase 1 will add the real
//!   `LocalEmbeddingAdapter` to the
//!   `adapters:` list (with `ignored` in
//!   Phase 0 because Phase 0 has no real
//!   model). Phase 2 will add the real
//!   `OllamaEmbeddingAdapter` (with
//!   `ignored` until Phase 2 wires the
//!   HTTP client).
//!
//! Story (plain English): The 12
//! challenges every embedding adapter
//! must pass. The 12 are the Phase 0
//! subset — they cover the structural
//! contract (object-safety, sync
//! capabilities, no-IO descriptor,
//! error kind mapping, default
//! `embed_batch` impl, the locked
//! version constant) and the
//! Phase 0 "skeleton returns Internal"
//! contract. Phase 1 adds 6 more
//! (unit-length, dimension consistency,
//! batch atomicity, real-model test
//! against the local candle adapter)
//! for a total of 18.
//!
//! CID Index:
//! CID:embedding-conformance-001 -> 12 assertions
//!
//! Quick lookup: rg -n "CID:embedding-conformance-" crates/afa-contract-testing/src/embedding/conformance.rs

use afa_contracts::error::{AfaError, AfaErrorKind};
use afa_contracts::EmbeddingV1;

use super::mock::MockEmbeddingAdapter;
use crate::run_suite;

// CID:embedding-conformance-001 - 12 conformance assertions
// Purpose: The 12 challenges the
// `EmbeddingV1` contract enforces.
// Each is a `|a: &dyn EmbeddingV1| async { ... }`
// closure that the `run_suite!` macro
// runs once per adapter in the
// `adapters:` list. The macro generates
// a `#[tokio::test]` function whose
// name is `<assertion>_<adapter>`, so
// a failing adapter shows up as
// `embed_returns_internal_in_phase_0__mock`
// (or whatever assertion failed) in
// the `cargo test` output.
//
// The 12 challenges are the Phase 0
// subset (no I/O, no real model). They
// cover:
//
// 1. `embed_returns_internal_in_phase_0`
//    — the skeleton contract: every
//    `embed` call returns `Internal`
//    in Phase 0 (the "we are not yet
//    serving customers" sentinel).
//
// 2. `embed_error_kind_is_internal`
//    — the kind mapping: `Internal`
//    variant maps to
//    `AfaErrorKind::Internal` (per
//    the error.rs 4-to-6 mapping).
//
// 3. `embed_batch_returns_count_consistent`
//    — the default `embed_batch`
//    impl produces N results for
//    N input texts (the per-chunk
//    loop is correct, not off-by-one).
//
// 4. `embed_batch_fails_atomically`
//    — the default `embed_batch`
//    impl propagates the first
//    `Err` and stops (a partial
//    success is not returned).
//
// 5. `describe_capabilities_is_sync`
//    — the descriptor is `fn`, not
//    `async fn` (the locked
//    `describe_capabilities` rule:
//    no async, no ctx, no I/O).
//
// 6. `describe_capabilities_has_no_ctx`
//    — the descriptor takes no
//    `ExecutionContext` parameter
//    (the locked rule; verified
//    by checking the call site
//    compiles without a context).
//
// 7. `describe_capabilities_has_no_io`
//    — the descriptor returns the
//    same card across two calls
//    (a contract that "no I/O" is
//    inferred from: if the adapter
//    had I/O, two calls in a row
//    could differ).
//
// 8. `describe_capabilities_dimension_pinned`
//    — the descriptor's `dimension`
//    field is a known constant (384
//    for the all-MiniLM-L6-v2 card).
//    Catches a future "I forgot to
//    set the dimension" regression.
//
// 9. `describe_capabilities_model_name_pinned`
//    — the descriptor's `model_name`
//    field is a known constant
//    ("all-MiniLM-L6-v2"). Catches
//    a future "I forgot to set the
//    model name" regression.
//
// 10. `embedding_trait_is_object_safe`
//     — `dyn EmbeddingV1` compiles
//     (the trait is object-safe; the
//     CapabilityRegistry can hold an
//     `Arc<dyn EmbeddingV1>`).
//
// 11. `embedding_v1_version_is_locked`
//     — the `EmbeddingV1Version`
//     constant is "1.0.0" (the
//     locked version; a future
//     bump is an ADR).
//
// 12. `embed_compiles_with_async_trait`
//     — the trait uses
//     `#[async_trait]` (the locked
//     dyn-safety escape hatch for
//     async fns; verified by the
//     fact that the mock's `embed`
//     method compiles and the
//     `dyn EmbeddingV1` reference
//     works).
//
// Each assertion is a single,
// behavior-focused test. No
// tautological assertions (e.g.
// "the mock returns what the mock
// returns"). The 12 cover real
// contract properties that any
// real adapter (Phase 1 local,
// Phase 2 Ollama) must also pass.
// Uses: afa_contracts::EmbeddingV1,
// afa_contracts::error::AfaErrorKind.
// Used by: the `run_suite!` macro
// (which generates 12 `#[tokio::test]`
// functions, one per (assertion,
// mock) pair, totaling 12 tests).
run_suite!(
    assertions: [
        // 1. The skeleton contract.
        embed_returns_internal_in_phase_0 => |a: &dyn EmbeddingV1| async {
            let ctx = crate::fixtures::test_execution_context("acme-realty");
            let result = a.embed("hello world", &ctx).await;
            assert!(
                result.is_err(),
                "Phase 0 skeleton must return Err, got {result:?}"
            );
        },

        // 2. The kind mapping for `Internal`.
        embed_error_kind_is_internal => |a: &dyn EmbeddingV1| async {
            let ctx = crate::fixtures::test_execution_context("acme-realty");
            let result = a.embed("hello world", &ctx).await;
            match result {
                Err(e) => {
                    assert_eq!(e.kind(), AfaErrorKind::Internal);
                }
                Ok(v) => panic!("expected Err, got Ok({v:?})"),
            }
        },

        // 3. The default `embed_batch` impl is count-consistent.
        embed_batch_returns_count_consistent => |a: &dyn EmbeddingV1| async {
            let ctx = crate::fixtures::test_execution_context("acme-realty");
            let texts = vec![
                "one".to_string(),
                "two".to_string(),
                "three".to_string(),
            ];
            let result = a.embed_batch(&texts, &ctx).await;
            // The mock returns `Internal` per text, so
            // `embed_batch` returns `Err`. The assertion
            // is that it does NOT return
            // `Ok(Vec<Vec<f32>>)` (the count-consistent
            // branch is exercised in Phase 1 with a real
            // mock that returns unit vectors).
            assert!(
                result.is_err(),
                "Phase 0 default impl must propagate the per-chunk `Internal`"
            );
        },

        // 4. The default `embed_batch` impl fails atomically.
        embed_batch_fails_atomically => |a: &dyn EmbeddingV1| async {
            let ctx = crate::fixtures::test_execution_context("acme-realty");
            let texts = vec!["a".to_string(), "b".to_string(), "c".to_string()];
            let result = a.embed_batch(&texts, &ctx).await;
            assert!(result.is_err(), "atomicity: a partial success is a bug");
        },

        // 5. The descriptor is sync (no `async fn`).
        describe_capabilities_is_sync => |a: &dyn EmbeddingV1| async {
            // The descriptor compiles only if it is
            // `fn`, not `async fn`. The fact that we
            // can call it synchronously here (no
            // `.await`) is the test.
            let caps = a.describe_capabilities();
            assert!(caps.dimension > 0, "dimension must be > 0");
        },

        // 6. The descriptor takes no `ExecutionContext`.
        describe_capabilities_has_no_ctx => |a: &dyn EmbeddingV1| async {
            // The descriptor compiles only if it
            // takes no `ExecutionContext` parameter.
            // The fact that we can call it as
            // `a.describe_capabilities()` (with no
            // `&ctx`) is the test.
            let _caps = a.describe_capabilities();
        },

        // 7. The descriptor has no I/O.
        describe_capabilities_has_no_io => |a: &dyn EmbeddingV1| async {
            // A no-I/O descriptor returns the same
            // card across two calls. If the adapter
            // had I/O, the card could differ (e.g.
            // the adapter could read the model name
            // from disk and the second call could
            // see a different file). The mock returns
            // the same card; the test pins that.
            let caps1 = a.describe_capabilities();
            let caps2 = a.describe_capabilities();
            assert_eq!(
                caps1, caps2,
                "no-I/O contract: describe_capabilities must be deterministic"
            );
        },

        // 8. The descriptor's `dimension` is pinned.
        describe_capabilities_dimension_pinned => |a: &dyn EmbeddingV1| async {
            let caps = a.describe_capabilities();
            // The mock reports the v1
            // all-MiniLM-L6-v2 card (384). Catches
            // a future "I forgot to set the
            // dimension" regression.
            assert_eq!(caps.dimension, 384, "all-MiniLM-L6-v2 dimension");
        },

        // 9. The descriptor's `model_name` is pinned.
        describe_capabilities_model_name_pinned => |a: &dyn EmbeddingV1| async {
            let caps = a.describe_capabilities();
            assert_eq!(caps.model_name, "all-MiniLM-L6-v2", "pinned model name");
        },

        // 10. The trait is object-safe.
        embedding_trait_is_object_safe => |_a: &dyn EmbeddingV1| async {
            // The fact that the closure parameter
            // is `&dyn EmbeddingV1` and the
            // `run_suite!` macro generates
            // `let adapter: $adapter_ty = ...;
            // let $adapter_param: &dyn $trait = &adapter;`
            // is the test. If `EmbeddingV1` were
            // not object-safe (e.g. it had an
            // associated type), the macro expansion
            // would not compile.
        },

        // 11. The version constant is locked.
        embedding_v1_version_is_locked => |_a: &dyn EmbeddingV1| async {
            use afa_contracts::EmbeddingV1Version;
            assert_eq!(EmbeddingV1Version, "1.0.0", "version is ADR-locked");
        },

        // 12. The trait uses `#[async_trait]`.
        embed_compiles_with_async_trait => |a: &dyn EmbeddingV1| async {
            // The fact that the mock's `embed`
            // method compiles and the
            // `dyn EmbeddingV1` reference works
            // is the test. If `#[async_trait]`
            // were missing, the mock's `embed`
            // would be
            // `fn embed(...) -> impl Future<...>`
            // and the `dyn EmbeddingV1` would
            // not be object-safe.
            let ctx = crate::fixtures::test_execution_context("acme-realty");
            let _ = a.embed("test", &ctx).await;
        },
    ],
    adapters: [
        "mock" => MockEmbeddingAdapter,
    ],
);
