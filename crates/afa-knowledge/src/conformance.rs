//! Code Map: Conformance suite
//! - `run_conformance_suite`: The contract-conformance
//!   tests run against any `Arc<dyn KnowledgeV1>`.
//!   Phase 0: stub (returns `Ok(())` so the test
//!   binary compiles and the crate's `cargo test`
//!   passes). Phase 1: stub for
//!   `store_information` (a no-op section that
//!   Phase 2 will populate with the per-method
//!   test cases). Phase 2: full per-method
//!   coverage (happy path, edge cases, error
//!   mapping, "no free_text in event", "no
//!   content in event", atomic-write guarantee,
//!   topic slug rules).
//!
//! Story (plain English): The conformance suite is
//! the safety net. A new storage adapter (JSON
//! today, Postgres tomorrow) plugs in by
//! implementing `KnowledgeV1` and passing the
//! suite. The suite answers one question: "does
//! this adapter honor the contract we promised
//! the workflows?" Phase 1 lands the
//! `store_information` section (a stub that Phase
//! 2 populates); Phase 2 fills in the body.
//!
//! CID Index:
//! CID:afa-knowledge-conformance-001 -> run_conformance_suite
//! CID:afa-knowledge-conformance-store-001 -> store_information_conformance
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
// the test binary compiles. Phase 1 body: the
// same no-op, but it now calls the
// `store_information_conformance` stub (which
// itself is a no-op until Phase 2). Phase 2
// body: the per-method test cases.
pub async fn run_conformance_suite(adapter: Arc<dyn KnowledgeV1>) {
    // Phase 0 stub: nothing to verify yet.
    // The Phase 2 body will drive the
    // per-method cases.
    let _ = adapter;
    // Phase 1 stub: the `store_information`
    // section. The body is a no-op for now;
    // Phase 2 populates the per-method test
    // cases (happy path, oversized body,
    // empty topic, empty body, etc.).
    store_information_conformance(adapter).await;
}

// CID:afa-knowledge-conformance-store-001 - store_information_conformance
// Purpose: The per-method test cases for
// `store_information`. Phase 1 lands the
// function (a no-op so the call site compiles);
// Phase 2 populates the body with the per-
// method cases.
//
// **Phase 1 scope** (no-op): the function
// exists, takes the adapter, and returns.
// **Phase 2 scope** (planned): happy path
// (valid input → Ok(())); oversized body →
// InvalidInput; empty body → InvalidInput;
// empty topic → InvalidInput; valid input
// with tags → file on disk + audit event on
// bus; the "no free_text in event" and "no
// content in event" assertions (the audit
// event must NOT carry the record body).
async fn store_information_conformance(_adapter: Arc<dyn KnowledgeV1>) {
    // Phase 1: no-op.
    // Phase 2 will populate this with the
    // per-method test cases.
}
