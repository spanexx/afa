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
//! - `knowledge_v1_conformance_suite_against_json_adapter`:
//!   Phase 4: the same conformance suite
//!   is run against the real
//!   `JsonKnowledgeAdapter` (in addition
//!   to the `MockAdapter`). The real
//!   adapter is built against a
//!   `tempfile::TempDir` storage root;
//!   the test asserts the on-disk side
//!   effect (the `.md` file is on
//!   disk under `<root>/<topic_slug>/
//!   <record_id>.md`) and the
//!   `find_information` call returns
//!   the just-stored record. This is
//!   the proof that the conformance
//!   suite is not over-fitted to the
//!   mock — the same contract holds
//!   for the real v1 storage adapter.
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
//! CID:afa-knowledge-tests-knowledge-v1-002 -> knowledge_v1 conformance vs. real JSON adapter
//!
//! Quick lookup: rg -n "CID:afa-knowledge-tests-knowledge-v1-" crates/afa-knowledge/tests/knowledge_v1.rs

use std::path::PathBuf;
use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::{
    execution_context::Actor, ids::TenantId, ExecutionContext, FindInformationRequest,
    KnowledgeCapabilities, KnowledgeRecordInput, KnowledgeV1,
};
use afa_knowledge::{run_conformance_suite, MockAdapter, MockCall};
use afa_plugin_knowledge_json::{JsonKnowledgeAdapter, JsonKnowledgeConfig};
use tempfile::TempDir;

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

// CID:afa-knowledge-tests-knowledge-v1-002 - knowledge_v1 conformance vs. real JSON adapter
// Purpose: The Phase 4 proof that the
// conformance suite is not over-fitted
// to the in-crate `MockAdapter`. The
// same `run_conformance_suite` is run
// against a real
// `JsonKnowledgeAdapter` built against
// a `tempfile::TempDir` storage root.
// The test asserts the on-disk side
// effect (the `.md` file is on disk
// under
// `<root>/<topic_slug>/<record_id>.md`
// and contains the canned content
// verbatim), the `find_information`
// call returns the just-stored
// record, and the `list_topics` call
// returns the just-created topic. A
// future storage adapter (Postgres,
// Neo4j) is the next consumer of the
// same `run_conformance_suite` — the
// mock and the real adapter agree on
// the contract, so the workflow
// surface is stable.
#[tokio::test]
async fn knowledge_v1_conformance_suite_against_json_adapter() {
    // 1. Build a fresh TempDir storage
    //    root and a real
    //    `JsonKnowledgeAdapter` against
    //    it. The adapter is wired to a
    //    real `EventBus` (the conformance
    //    suite itself does not need the
    //    bus, but the adapter's
    //    constructor requires it).
    let dir = TempDir::new().expect("tempdir");
    let storage_root: PathBuf = dir.path().to_path_buf();
    let bus = Arc::new(EventBus::new());
    let cfg = JsonKnowledgeConfig::new(
        storage_root.clone(),
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    );
    let adapter = JsonKnowledgeAdapter::new(cfg, bus)
        .await
        .expect("JsonKnowledgeAdapter::new");
    let trait_arc: Arc<dyn KnowledgeV1> = Arc::new(adapter);

    // 2. Run the same `run_conformance_suite`
    //    the mock-driven test uses. The
    //    suite is adapter-agnostic — its
    //    signature is
    //    `Arc<dyn KnowledgeV1>`. A future
    //    adapter passes the same suite.
    run_conformance_suite(trait_arc.clone()).await;

    // 3. Verify the on-disk side effect
    //    of the `store_information` call
    //    in the suite (the suite's
    //    canned `store_information`
    //    input is
    //    `topic="FAQ"`, `content="conformance
    //    hello"`). The file is on disk
    //    under
    //    `<root>/faq/<record_id>.md` and
    //    contains the canned content
    //    verbatim.
    let mut stored_count = 0;
    let mut last_record_id: Option<afa_contracts::RecordId> = None;
    let entries = std::fs::read_dir(storage_root.join("faq")).expect("read faq dir");
    for entry in entries {
        let entry = entry.expect("entry");
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("md") {
            stored_count += 1;
            let bytes = std::fs::read(&path).expect("read .md");
            assert_eq!(
                bytes, b"conformance hello",
                "stored record content must match the suite's canned content"
            );
            // Capture the record id from
            // the filename
            // (`<record_id>.md`) so the
            // find call below can
            // assert the on-disk file
            // matches the just-stored
            // id.
            let stem = path
                .file_stem()
                .and_then(|s| s.to_str())
                .expect("file stem");
            let uuid = uuid::Uuid::parse_str(stem).expect("record id is a UUID");
            last_record_id = Some(afa_contracts::RecordId(uuid));
        }
    }
    assert_eq!(
        stored_count, 1,
        "the suite's store_information call must produce exactly one on-disk .md file"
    );

    // 4. Verify `find_information` can
    //    read the just-stored record
    //    back. The suite called
    //    `find_information` with
    //    `Default::default()` (no
    //    filters), so the response
    //    should include the just-stored
    //    record.
    let resp = trait_arc
        .find_information(FindInformationRequest::default(), &ctx())
        .await
        .expect("find_information on the just-stored record");
    assert!(
        !resp.is_empty(),
        "find_information must return the just-stored record"
    );
    let (top, top_score) = &resp[0];
    assert_eq!(
        top.content, "conformance hello",
        "find_information must return the canned content"
    );
    assert_eq!(
        top.record_id,
        last_record_id.expect("a record id was stored"),
        "find_information must return the just-stored record id"
    );
    assert!(
        *top_score > 0.0,
        "top hit score must be > 0; got {top_score}"
    );

    // 5. Verify `list_topics` returns
    //    the just-created topic. The
    //    suite's canned
    //    `store_information` created
    //    one topic ("FAQ"), so the
    //    list must contain exactly one
    //    entry.
    let topics = trait_arc
        .list_topics(&ctx())
        .await
        .expect("list_topics on the just-stored topic");
    assert_eq!(topics.len(), 1);
    assert_eq!(topics[0].name, "FAQ");
    assert_eq!(topics[0].record_count, 1);

    // 6. Verify `describe_capabilities`
    //    returns the locked v1 shape.
    let caps = trait_arc.describe_capabilities();
    assert_eq!(caps.max_record_size_bytes, 1_048_576);
    assert!(!caps.supports_semantic_search);
    assert!(!caps.supports_hierarchical_topics);
}

fn ctx() -> ExecutionContext {
    ExecutionContext::new(TenantId::new("c"), Actor::Timer)
}
