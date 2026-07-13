//! Code Map: `tests/adapter_unit.rs`
//! - 7 unit tests for `JsonKnowledgeAdapter`
//!   that exercise the adapter's behavior
//!   through the public `KnowledgeV1` trait
//!   surface. The tests are in
//!   `tests/adapter_unit.rs` (a Cargo
//!   integration test target) rather than
//!   inside `src/` so they exercise the
//!   public surface only — no `pub` items
//!   are accessible from inside the crate.
//!
//! Story (plain English): The unit tests are
//! the safety net for the adapter's write
//! path. They cover the happy path, the
//! three InvalidInput rejection paths
//! (oversized content, empty topic, empty
//! content), the multi-record per-topic
//! scenario, the topic-slug safety
//! guarantee, and the audit-event shape
//! (the event must carry the record_id +
//! topic_slug, never the content).
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-tests-unit-001 -> adapter_unit tests
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-tests-unit-" crates/afa-plugin-knowledge-json/tests/adapter_unit.rs

use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::execution_context::Actor;
use afa_contracts::ids::TenantId;
use afa_contracts::knowledge::KnowledgeRecordStored;
use afa_contracts::{
    ExecutionContext, KnowledgeCapabilities, KnowledgeErrorV1, KnowledgeRecordInput, KnowledgeV1,
};
use afa_plugin_knowledge_json::{JsonKnowledgeAdapter, JsonKnowledgeConfig};
use tempfile::TempDir;

fn make_ctx() -> ExecutionContext {
    ExecutionContext::new(TenantId::new("tenant-a"), Actor::Timer)
}

fn make_config(dir: &TempDir) -> JsonKnowledgeConfig {
    JsonKnowledgeConfig::new(
        dir.path().to_path_buf(),
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    )
}

async fn make_adapter(dir: &TempDir) -> (JsonKnowledgeAdapter, Arc<EventBus>) {
    let cfg = make_config(dir);
    let bus = Arc::new(EventBus::new());
    let adapter = JsonKnowledgeAdapter::new(cfg, bus.clone())
        .await
        .expect("new");
    (adapter, bus)
}

// CID:afa-plugin-knowledge-json-tests-unit-001 - adapter_unit tests
// Purpose: 7 unit tests covering the
// `JsonKnowledgeAdapter` write path. The
// tests are run as a Cargo integration
// target (under `tests/`), which means
// they only have access to the public
// surface (the `KnowledgeV1` trait +
// the `JsonKnowledgeConfig` +
// `JsonKnowledgeAdapter` types). This
// is intentional: a future contributor
// who breaks the public surface
// (removes a method, changes a
// signature) would be forced to update
// these tests.
#[tokio::test]
async fn unit_store_information_happy_path_writes_file_and_updates_index() {
    // Test 1: the happy path. Store
    // a record; the file appears on
    // disk; the index is updated.
    let dir = TempDir::new().expect("tempdir");
    let (adapter, _bus) = make_adapter(&dir).await;
    let input = KnowledgeRecordInput {
        topic: "FAQ".to_string(),
        tags: vec!["billing".to_string()],
        content: "refund policy".to_string(),
        source: Some("chat-2026-07-11".to_string()),
    };
    let ctx = make_ctx();
    let returned_id = adapter
        .store_information(input, &ctx)
        .await
        .expect("store_information");
    let file_path = dir.path().join("faq").join(format!("{returned_id}.md"));
    assert!(
        file_path.is_file(),
        "file must exist: {}",
        file_path.display()
    );
    assert_eq!(std::fs::read(&file_path).expect("read"), b"refund policy");
    let idx = adapter.index.read().await;
    assert_eq!(idx.topic_count(), 1);
    assert_eq!(idx.total_record_count(), 1);
    assert!(idx.known_tags().contains("billing"));
}

#[tokio::test]
async fn unit_store_information_rejects_oversized_content() {
    // Test 2: a content larger than
    // `max_record_size_bytes` is
    // rejected with InvalidInput.
    let dir = TempDir::new().expect("tempdir");
    let cfg = JsonKnowledgeConfig::new(
        dir.path().to_path_buf(),
        KnowledgeCapabilities {
            max_record_size_bytes: 8,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    );
    let bus = Arc::new(EventBus::new());
    let adapter = JsonKnowledgeAdapter::new(cfg, bus).await.expect("new");
    let input = KnowledgeRecordInput {
        topic: "FAQ".to_string(),
        tags: vec![],
        content: "x".repeat(9),
        source: None,
    };
    let err = adapter
        .store_information(input, &make_ctx())
        .await
        .expect_err("must reject");
    assert!(matches!(err, KnowledgeErrorV1::InvalidInput { .. }));
}

#[tokio::test]
async fn unit_store_information_rejects_empty_topic() {
    // Test 3: an empty / whitespace
    // topic is rejected with
    // InvalidInput.
    let dir = TempDir::new().expect("tempdir");
    let (adapter, _bus) = make_adapter(&dir).await;
    let input = KnowledgeRecordInput {
        topic: "   ".to_string(),
        tags: vec![],
        content: "x".to_string(),
        source: None,
    };
    let err = adapter
        .store_information(input, &make_ctx())
        .await
        .expect_err("must reject");
    assert!(matches!(err, KnowledgeErrorV1::InvalidInput { .. }));
}

#[tokio::test]
async fn unit_store_information_rejects_empty_content() {
    // Test 4: an empty content is
    // rejected with InvalidInput.
    let dir = TempDir::new().expect("tempdir");
    let (adapter, _bus) = make_adapter(&dir).await;
    let input = KnowledgeRecordInput {
        topic: "FAQ".to_string(),
        tags: vec![],
        content: "".to_string(),
        source: None,
    };
    let err = adapter
        .store_information(input, &make_ctx())
        .await
        .expect_err("must reject");
    assert!(matches!(err, KnowledgeErrorV1::InvalidInput { .. }));
}

#[tokio::test]
async fn unit_store_information_two_records_same_topic_distinct_files() {
    // Test 5: two records in the
    // same topic get two distinct
    // files (the record_id is the
    // filename). The index has
    // `record_count == 2`.
    let dir = TempDir::new().expect("tempdir");
    let (adapter, _bus) = make_adapter(&dir).await;
    let a = KnowledgeRecordInput {
        topic: "FAQ".to_string(),
        tags: vec![],
        content: "a".to_string(),
        source: None,
    };
    let b = KnowledgeRecordInput {
        topic: "FAQ".to_string(),
        tags: vec![],
        content: "b".to_string(),
        source: None,
    };
    let ctx = make_ctx();
    let id_a = adapter.store_information(a, &ctx).await.expect("a");
    let id_b = adapter.store_information(b, &ctx).await.expect("b");
    let fa = dir.path().join("faq").join(format!("{id_a}.md"));
    let fb = dir.path().join("faq").join(format!("{id_b}.md"));
    assert!(fa.is_file());
    assert!(fb.is_file());
    let idx = adapter.index.read().await;
    assert_eq!(idx.total_record_count(), 2);
}

#[tokio::test]
async fn unit_store_information_topic_slug_uses_safe_directory_name() {
    // Test 6: a topic with spaces
    // and special characters is
    // slugged into a safe
    // on-disk directory name.
    // "Property listings" →
    // "property-listings". The
    // file is created under
    // `<root>/property-listings/`.
    let dir = TempDir::new().expect("tempdir");
    let (adapter, _bus) = make_adapter(&dir).await;
    let input = KnowledgeRecordInput {
        topic: "Property listings".to_string(),
        tags: vec![],
        content: "x".to_string(),
        source: None,
    };
    let ctx = make_ctx();
    let returned_id = adapter
        .store_information(input, &ctx)
        .await
        .expect("store_information");
    let file_path = dir
        .path()
        .join("property-listings")
        .join(format!("{returned_id}.md"));
    assert!(
        file_path.is_file(),
        "file must exist under property-listings/: {}",
        file_path.display()
    );
    let idx = adapter.index.read().await;
    assert_eq!(
        idx.topic_for_slug("property-listings"),
        Some("Property listings".to_string())
    );
}

#[tokio::test]
async fn unit_store_information_publishes_audit_event_with_id_and_metadata() {
    // Test 7: the audit event
    // published on the bus carries
    // the `record_id`, the topic
    // name, the `content_length`,
    // and the `source`. The
    // content must NOT be present
    // in the event (the audit trail
    // is metadata only, never the
    // record content). The
    // conformance suite (Phase 2)
    // also asserts on the "no
    // content in event" rule; this
    // test is the adapter-level
    // check.
    let dir = TempDir::new().expect("tempdir");
    let (adapter, bus) = make_adapter(&dir).await;
    // Subscribe with a generous
    // capacity so the publish
    // never blocks.
    let mut sub = bus.subscribe::<KnowledgeRecordStored>(16);
    let input = KnowledgeRecordInput {
        topic: "FAQ".to_string(),
        tags: vec!["billing".to_string()],
        content: "secret content that must NOT appear in the event".to_string(),
        source: Some("chat-2026-07-11".to_string()),
    };
    let ctx = make_ctx();
    let returned_id = adapter
        .store_information(input.clone(), &ctx)
        .await
        .expect("store_information");
    // The audit event arrives
    // asynchronously; we await it
    // (the bus uses a bounded
    // channel; a brief recv
    // suffices).
    let (event, _event_ctx) = sub.recv().await.expect("audit event must be published");
    let stored = &*event;
    assert_eq!(stored.record_id, returned_id);
    assert_eq!(stored.topic, input.topic);
    assert_eq!(stored.content_length as usize, input.content.len());
    assert_eq!(stored.source, input.source);
    assert_eq!(stored.tag_count, 1);
    // The content must NOT be in
    // the event payload (audit-
    // trail safety). The
    // `Debug` form is the
    // practical way to check.
    let serialized = format!("{stored:?}");
    assert!(
        !serialized.contains("secret content that must NOT appear"),
        "audit event must NOT contain the content; got: {serialized}"
    );
    // The event must implement
    // `AfaEvent` (compile-time
    // assertion via the static
    // type bound on the bus
    // subscription).
    fn _is_afa_event<T: afa_contracts::events::AfaEvent>(_: &T) {}
    _is_afa_event(stored);
}
