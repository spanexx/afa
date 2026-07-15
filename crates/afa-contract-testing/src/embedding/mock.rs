//! Code Map: afa-contract-testing — embedding mock
//! - `MockEmbeddingAdapter`: The reference
//!   adapter the conformance suite runs
//!   against. Conforms to the `EmbeddingV1`
//!   contract: returns `Internal` from
//!   every `embed` call (Phase 0
//!   "skeleton" behavior), and reports the
//!   same v1 all-MiniLM-L6-v2 capabilities
//!   card the production `LocalEmbeddingAdapter`
//!   skeleton reports. The mock exists
//!   because (a) the conformance suite needs
//!   a `Default` adapter to feed the
//!   `run_suite!` macro, and (b) the suite
//!   must work without touching the network
//!   or the file system — the mock is
//!   pure-Rust, no I/O.
//!
//! Story (plain English): The mock is the
//! "perfect student" the driving school
//! uses to demo the score sheet before any
//! real student has been enrolled. It
//! passes the structural challenges
//! (returns the right capability card, has
//! the right method signatures, returns
//! the expected `Internal` for Phase 0)
//! and only those. The real adapters
//! (`LocalEmbeddingAdapter` in Phase 1,
//! `OllamaEmbeddingAdapter` in Phase 2)
//! are the "real students" that must
//! pass the same challenges with real
//! behavior.
//!
//! CID Index:
//! CID:embedding-mock-001 -> MockEmbeddingAdapter
//! CID:embedding-mock-002 -> impl EmbeddingV1
//!
//! Quick lookup: rg -n "CID:embedding-mock-" crates/afa-contract-testing/src/embedding/mock.rs

use async_trait::async_trait;

use afa_contracts::{EmbeddingCapabilitiesV1, EmbeddingErrorV1, EmbeddingV1, ExecutionContext};

// CID:embedding-mock-001 - MockEmbeddingAdapter
// Purpose: The reference adapter the
// conformance suite runs against. The
// `Default` impl is required because the
// `run_suite!` macro calls
// `<$adapter_ty as ::core::default::Default>::default()`
// to construct the adapter under test.
// Uses: EmbeddingV1, EmbeddingErrorV1,
// EmbeddingCapabilitiesV1.
// Used by: the `run_suite!`-driven
// conformance tests in `conformance.rs`.
#[derive(Default, Debug)]
pub struct MockEmbeddingAdapter;

// CID:embedding-mock-002 - impl EmbeddingV1
// Purpose: The trait impl. The mock
// returns `Internal` from every `embed`
// call (the "we are the example student
// and we have not yet learned any tricks"
// sentinel). The default `embed_batch`
// impl from the trait is used (which
// loops over `embed` and concatenates the
// results); the mock does not override
// it, so the conformance suite can also
// assert on the default impl's behavior.
//
// `describe_capabilities` returns the
// same v1 all-MiniLM-L6-v2 card the
// production `LocalEmbeddingAdapter`
// skeleton reports. The mock's card is
// pinned (not random) so the conformance
// suite can assert on it.
// Uses: EmbeddingErrorV1,
// EmbeddingCapabilitiesV1.
// Used by: every conformance test in
// `conformance.rs` (the `run_suite!`
// macro constructs the mock and runs
// every assertion against it).
#[async_trait]
impl EmbeddingV1 for MockEmbeddingAdapter {
    async fn embed(
        &self,
        _text: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Vec<f32>, EmbeddingErrorV1> {
        // The Phase 0 "we are the example
        // student and we have not yet
        // learned any tricks" sentinel.
        // The conformance suite's
        // `embed_returns_internal_in_phase_0`
        // test pins this.
        Err(EmbeddingErrorV1::Internal {
            reason: "MockEmbeddingAdapter Phase 0: example student has not yet learned any tricks"
                .into(),
        })
    }

    fn describe_capabilities(&self) -> EmbeddingCapabilitiesV1 {
        // The same v1 all-MiniLM-L6-v2
        // card the production
        // `LocalEmbeddingAdapter`
        // skeleton reports. Pinned so
        // the conformance suite can
        // assert on it.
        EmbeddingCapabilitiesV1 {
            model_name: "all-MiniLM-L6-v2".to_string(),
            dimension: 384,
            max_batch_size: 64,
            max_sequence_length: 512,
            supports_batching: true,
        }
    }
}
