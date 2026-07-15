//! Code Map: afa-contract-testing — embedding
//! - `mock`: A `MockEmbeddingAdapter` that
//!   conforms to the `EmbeddingV1` contract
//!   but does nothing real in Phase 0 (it
//!   returns `Internal` from every `embed`
//!   call, exactly like the production
//!   `LocalEmbeddingAdapter` and
//!   `OllamaEmbeddingAdapter` skeletons).
//!   Used as the reference adapter for the
//!   Phase 0 conformance tests, and later
//!   for the Phase 1+ tests once the mock
//!   grows real behavior.
//! - `conformance`: The 12 conformance
//!   assertions, run via the `run_suite!`
//!   macro against the mock. The same
//!   assertions will be run against the
//!   real adapters in Phase 1 (local) and
//!   Phase 2 (Ollama).
//!
//! Story (plain English): The embedding
//! conformance suite is the "driving
//! school" for the embedding adapters. The
//! mock is the example student (who always
//! does the right thing), and the 12
//! assertions are the 12 challenges every
//! adapter must pass. Add a new adapter →
//! add it to the `adapters:` list, and the
//! same 12 challenges are run against it.
//! Fail an adapter → you see exactly which
//! challenge, in the test name.
//!
//! CID Index:
//! CID:embedding-mod-001 -> module re-exports
//!
//! Quick lookup: rg -n "CID:embedding-mod-" crates/afa-contract-testing/src/embedding/mod.rs

pub mod conformance;
pub mod mock;
