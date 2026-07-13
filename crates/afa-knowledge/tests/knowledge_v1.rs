//! Code Map: `afa-knowledge/tests/knowledge_v1.rs`
//! - The single conformance-suite
//!   integration test. Runs the
//!   `run_conformance_suite` from
//!   `afa_knowledge::conformance`
//!   against a `MockAdapter` and
//!   asserts the call log shows the
//!   expected call shape (one
//!   `store_information`, one
//!   `find_information`, one
//!   `list_topics`).
//!
//! Story (plain English): The
//! conformance suite is the safety
//! net. The integration test in
//! `afa-knowledge/tests/` runs the
//! suite against the canonical
//! `MockAdapter` (the simplest
//! possible `KnowledgeV1` impl). A
//! future contributor who breaks the
//! contract — by changing the method
//! signature, adding a required
//! field, etc. — would be forced to
//! update this test.
//!
//! CID Index:
//! CID:afa-knowledge-tests-knowledge-v1-001 -> knowledge_v1 conformance test
//!
//! Quick lookup: rg -n "CID:afa-knowledge-tests-knowledge-v1-" crates/afa-knowledge/tests/knowledge_v1.rs

use std::sync::Arc;

use afa_contracts::{
    execution_context::Actor, ids::TenantId, ExecutionContext, FindInformationRequest,
    KnowledgeRecordInput, KnowledgeV1,
};
use afa_knowledge::{run_conformance_suite, MockAdapter, MockCall};

// CID:afa-knowledge-tests-knowledge-v1-001 - knowledge_v1 test
// Purpose: Run the contract-conformance
// suite against the `MockAdapter`
// and assert the call log shows
// exactly one call per method
// (the suite's happy path). The
// test keeps a `Arc<MockAdapter>`
// reference so the call-log
// assertion does not need a
// downcast.
#[tokio::test]
async fn knowledge_v1_conformance_suite_against_mock() {
    // Build a `MockAdapter` and
    // hold it in two `Arc`s:
    // one as the trait object
    // (the suite's signature
    // requires `Arc<dyn
    // KnowledgeV1>`), one as
    // the concrete type
    // (for the call-log
    // assertion). The two
    // `Arc`s point to the
    // same allocation; the
    // `call_log` lives on the
    // concrete type.
    let mock_arc: Arc<MockAdapter> = Arc::new(MockAdapter::new());
    let trait_arc: Arc<dyn KnowledgeV1> = mock_arc.clone();
    run_conformance_suite(trait_arc).await;
    let log = mock_arc.call_log();
    assert_eq!(log.len(), 3, "suite must make exactly 3 method calls");
    assert!(matches!(log[0], MockCall::Store(_)));
    assert!(matches!(log[1], MockCall::Find(_)));
    assert!(matches!(log[2], MockCall::List));
}

// A separate test that exercises
// the `MockAdapter` directly to
// confirm the call log records the
// right shape.
#[tokio::test]
async fn mock_adapter_records_call_shape() {
    let mock_arc: Arc<MockAdapter> = Arc::new(MockAdapter::new());
    let adapter: Arc<dyn KnowledgeV1> = mock_arc.clone();
    let ctx = ExecutionContext::new(TenantId::new("c"), Actor::Timer);
    adapter
        .store_information(
            KnowledgeRecordInput {
                topic: "FAQ".to_string(),
                tags: vec![],
                content: "x".to_string(),
                source: None,
            },
            &ctx,
        )
        .await
        .unwrap();
    adapter
        .find_information(FindInformationRequest::default(), &ctx)
        .await
        .unwrap();
    adapter.list_topics(&ctx).await.unwrap();

    let log = mock_arc.call_log();
    assert_eq!(log.len(), 3);
}
