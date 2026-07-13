//! Code Map: knowledge_v1 tests
//!
//! Phase 0 test binary for the Knowledge V1
//! conformance suite. The `cargo test` run for
//! `afa-knowledge` is `cargo test -p
//! afa-knowledge`. Phase 0: this binary contains
//! a single trivial test that exercises the
//! trait object type and the re-exports; the
//! real conformance cases land in Phase 2.
//!
//! Story (plain English): The test binary is the
//! place the conformance suite will live. The
//! `MockAdapter` (the canonical mock) will sit
//! in `tests/common/mod.rs` and the per-method
//! cases will be `#[tokio::test]` functions that
//! build a `MockAdapter`, register it with the
//! suite, and assert on the observable behavior.
//! Phase 0 only proves the suite signature
//! compiles and the trait compiles behind
//! `Arc<dyn KnowledgeV1>`.
//!
//! CID Index:
//! CID:afa-knowledge-tests-001 -> knowledge_v1_trait_object_can_be_typed
//!
//! Quick lookup: rg -n "CID:afa-knowledge-tests-" crates/afa-knowledge/tests/knowledge_v1.rs

use afa_knowledge::{KnowledgeV1, Topic};
use std::sync::Arc;

#[tokio::test]
async fn knowledge_v1_trait_object_can_be_typed() {
    // Phase 0 proof that the trait is
    // object-safe and the `Arc<dyn KnowledgeV1>`
    // type the registry will use compiles. The
    // function `_assert_object_safe` is a
    // compile-only check (Rust will fail to
    // compile the body if the trait is not
    // object-safe, e.g. if a method took `self`
    // by value or had a generic type parameter).
    // A future contributor who makes the trait
    // non-object-safe would be forced to remove
    // this test or change the trait.
    fn _assert_object_safe(_a: Arc<dyn KnowledgeV1>) {}

    // Also exercise the `Topic` re-export so
    // the `cargo test` run is not "0 tests
    // run" in Phase 0 (a CI dashboard that
    // shows "0 tests" for a brand-new crate
    // looks like a packaging mistake).
    let _t = Topic {
        name: "FAQ".into(),
        record_count: 0,
        first_record_at: None,
        last_record_at: None,
        tag_count: 0,
    };
}
