//! Code Map: Conformance suite
//! - `run_conformance_suite`: The contract-conformance
//!   tests run against any `Arc<dyn KnowledgeV1>`.
//!   Phase 0: stub (returns `Ok(())` so the test
//!   binary compiles and the crate's `cargo test`
//!   passes). Phase 2: populated with the per-
//!   method test cases (happy path, edge cases,
//!   error mapping, "no free_text in event",
//!   "no content in event", atomic-write guarantee,
//!   topic slug rules).
//!
//! Story (plain English): The conformance suite is
//! the safety net. A new storage adapter (JSON
//! today, Postgres tomorrow) plugs in by
//! implementing `KnowledgeV1` and passing the
//! suite. The suite answers one question: "does
//! this adapter honor the contract we promised
//! the workflows?" Phase 0 lands the harness
//! (the `pub async fn
//! run_conformance_suite(adapter: Arc<dyn
//! KnowledgeV1>)` signature); Phase 2 populates
//! the body.
//!
//! CID Index:
//! CID:afa-knowledge-conformance-001 -> run_conformance_suite
//!
//! Quick lookup: rg -n "CID:afa-knowledge-conformance-" crates/afa-knowledge/src/conformance.rs

use std::sync::Arc;

use afa_contracts::KnowledgeV1;

// CID:afa-knowledge-conformance-001 - run_conformance_suite
// Purpose: The contract-conformance entry point.
// The signature is locked (the Phase 2 body will
// drop in under it): one `Arc<dyn KnowledgeV1>`
// in, no return value, the suite fails the
// surrounding test if any case panics. The
// signature takes an `Arc<dyn KnowledgeV1>`
// rather than a concrete type so the same suite
// can run against a `MockAdapter` (Phase 2
// canonical mock), the JSON adapter, and any
// future adapter (Postgres, Neo4j) without
// modification. The signature is `pub async fn`
// because every per-method case will `await` the
// adapter's async methods.
//
// Phase 0 body: a no-op that returns `Ok(())` so
// the test binary compiles. Phase 2 body: the
// per-method test cases. The body is wrapped in
// `let _ = adapter;` to silence the "unused
// parameter" lint in Phase 0; Phase 2 will use
// the parameter to drive the per-method cases.
pub async fn run_conformance_suite(adapter: Arc<dyn KnowledgeV1>) {
    // Phase 0 stub: nothing to verify yet.
    // The Phase 2 body will drive the
    // per-method cases.
    let _ = adapter;
}
