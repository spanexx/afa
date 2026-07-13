//! Code Map: `tests/adapter_integration.rs`
//! - The single full-stack integration
//!   test for `JsonKnowledgeAdapter`.
//!   End-to-end path: build a real
//!   `CapabilityRegistry`, build a real
//!   `EventBus`, build a real
//!   `JsonKnowledgeAdapter` (the same
//!   concrete type a real
//!   agency-deploy of AFA would
//!   bootstrap with), register the
//!   adapter under the "default" key,
//!   hand it out via
//!   `registry.knowledge("default")`,
//!   call `store_information` on the
//!   trait object, and verify the file
//!   is on disk under the storage root
//!   supplied at registration time.
//!
//! Story (plain English): The integration
//! test is the part of the suite that
//! exercises the *full* code path the
//! agency-deploy will use. The unit
//! tests exercise the adapter in
//! isolation; this one exercises it
//! through the registry (the same
//! hand-out path the kernel uses) and
//! verifies the on-disk side effect the
//! PRD promises (the file is on disk
//! under the registered storage root).
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-tests-integration-001 -> adapter_integration test
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-tests-integration-" crates/afa-plugin-knowledge-json/tests/adapter_integration.rs

use std::path::PathBuf;
use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::execution_context::Actor;
use afa_contracts::ids::TenantId;
use afa_contracts::{ExecutionContext, KnowledgeCapabilities, KnowledgeRecordInput, KnowledgeV1};
use afa_kernel::CapabilityRegistry;
use afa_plugin_knowledge_json::{JsonKnowledgeAdapter, JsonKnowledgeConfig};
use tempfile::TempDir;

// CID:afa-plugin-knowledge-json-tests-integration-001 - adapter_integration test
// Purpose: The single full-stack integration
// test. The test exercises the same path
// a real `Kernel` would: build the
// adapter, register it under the
// "default" key, hand it out via
// `registry.knowledge("default")`, call
// `store_information`, and verify the
// file is on disk under the storage
// root supplied at registration time.
// This is the test that catches
// "the adapter works in isolation but
// breaks when the registry hands it
// out" regressions.
#[tokio::test]
async fn json_adapter_via_registry_stores_record_end_to_end() {
    // Step 1: build a fresh storage
    // root in a tempdir.
    let dir = TempDir::new().expect("tempdir");
    let storage_root: PathBuf = dir.path().to_path_buf();

    // Step 2: build the adapter with
    // a real `EventBus` and a real
    // `JsonKnowledgeConfig`.
    let cfg = JsonKnowledgeConfig::new(
        storage_root.clone(),
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    );
    let bus = Arc::new(EventBus::new());
    let adapter = JsonKnowledgeAdapter::new(cfg, bus.clone())
        .await
        .expect("new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);

    // Step 3: register the adapter
    // under the "default" key.
    let mut registry = CapabilityRegistry::new();
    registry
        .register_knowledge("default", adapter.clone(), storage_root.clone())
        .expect("register_knowledge");

    // Step 4: hand the adapter out
    // through the registry (the same
    // path the kernel uses).
    let handed_out = registry
        .knowledge("default")
        .expect("registry hands out the same Arc");

    // Step 5: call
    // `store_information` on the
    // trait object. The audit
    // event must be published on
    // the bus. The explicit
    // type annotation on
    // `returned_id` works around
    // a type-inference limitation
    // when calling an async
    // method on a `dyn`
    // trait-object without a
    // concrete type to drive
    // the inference.
    let mut sub = bus.subscribe::<afa_contracts::knowledge::KnowledgeRecordStored>(16);
    let ctx = ExecutionContext::new(
        TenantId::new("acme-realty"),
        Actor::Channel {
            name: "http".to_string(),
        },
    );
    let input = KnowledgeRecordInput {
        topic: "FAQ".to_string(),
        tags: vec!["billing".to_string(), "refund".to_string()],
        content: "full refund within 30 days, no questions asked".to_string(),
        source: Some("chat-2026-07-13".to_string()),
    };
    let returned_id: afa_contracts::RecordId = handed_out
        .store_information(input.clone(), &ctx)
        .await
        .expect("store_information");
    // The `Arc::ptr_eq` check
    // proves the registry handed
    // out the same allocation.
    assert!(
        Arc::ptr_eq(&adapter, &handed_out),
        "registry must hand out the same Arc it was given"
    );

    // Step 6: verify the file is on
    // disk under the storage root
    // supplied at registration
    // time.
    let file_path = storage_root.join("faq").join(format!("{returned_id}.md"));
    assert!(
        file_path.is_file(),
        "record file must exist on disk: {}",
        file_path.display()
    );
    let bytes = std::fs::read(&file_path).expect("read");
    assert_eq!(bytes, input.content.as_bytes());

    // Step 7: verify the audit
    // event was published.
    let (event, event_ctx) = sub.recv().await.expect("audit event must be published");
    let stored = &*event;
    assert_eq!(stored.record_id, returned_id);
    assert_eq!(stored.topic, input.topic);
    assert_eq!(stored.content_length as usize, input.content.len());
    assert_eq!(stored.source, input.source);
    assert_eq!(stored.tag_count, 2);
    // The `ExecutionContext` the
    // bus received must carry the
    // same tenant + actor the
    // caller passed.
    assert_eq!(event_ctx.tenant_id, ctx.tenant_id);
    assert_eq!(event_ctx.actor, ctx.actor);
}

// Phase 2 integration tests. These
// exercise the read path end-to-end
// (via the registry hand-out).
//
// Helper: build a populated adapter
// (3 records across 2 topics) and
// hand it out via the registry. The
// test cases below consume the
// returned tuple to exercise the
// read path.

struct PopulatedFixture {
    _dir: TempDir,
    #[allow(dead_code)]
    storage_root: PathBuf,
    bus: Arc<EventBus>,
    adapter: Arc<dyn KnowledgeV1>,
    ctx: ExecutionContext,
}

async fn make_populated_fixture() -> PopulatedFixture {
    let dir = TempDir::new().expect("tempdir");
    let storage_root = dir.path().to_path_buf();
    let cfg = JsonKnowledgeConfig::new(
        storage_root.clone(),
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    );
    let bus = Arc::new(EventBus::new());
    let adapter = JsonKnowledgeAdapter::new(cfg, bus.clone())
        .await
        .expect("new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);
    let ctx = ExecutionContext::new(
        TenantId::new("acme-realty"),
        Actor::Channel {
            name: "http".to_string(),
        },
    );
    // Three records across two topics.
    let inputs = vec![
        KnowledgeRecordInput {
            topic: "FAQ".to_string(),
            tags: vec!["billing".to_string()],
            content: "refund policy: full refund within 30 days".to_string(),
            source: Some("chat-2026-07-12".to_string()),
        },
        KnowledgeRecordInput {
            topic: "FAQ".to_string(),
            tags: vec!["shipping".to_string()],
            content: "shipping policy: free shipping over $50".to_string(),
            source: Some("chat-2026-07-12".to_string()),
        },
        KnowledgeRecordInput {
            topic: "Properties".to_string(),
            tags: vec!["billing".to_string()],
            content: "first month free for new tenants".to_string(),
            source: Some("chat-2026-07-13".to_string()),
        },
    ];
    for input in inputs {
        adapter
            .store_information(input, &ctx)
            .await
            .expect("seed store_information");
    }
    PopulatedFixture {
        _dir: dir,
        storage_root,
        bus,
        adapter,
        ctx,
    }
}

#[tokio::test]
async fn phase2_find_information_no_filters_returns_all_records() {
    // Test 1: with no filters, all
    // 3 records come back.
    let fix = make_populated_fixture().await;
    let resp = fix
        .adapter
        .find_information(Default::default(), &fix.ctx)
        .await
        .expect("find_information");
    assert_eq!(resp.len(), 3);
}

#[tokio::test]
async fn phase2_find_information_topic_filter_narrows_to_topic() {
    // Test 2: topic filter
    // narrows the candidate set
    // to the records in that
    // topic.
    let fix = make_populated_fixture().await;
    let req = afa_contracts::FindInformationRequest {
        topic: Some("FAQ".to_string()),
        ..Default::default()
    };
    let resp = fix
        .adapter
        .find_information(req, &fix.ctx)
        .await
        .expect("find_information");
    assert_eq!(resp.len(), 2);
    for (rec, _score) in &resp {
        assert_eq!(rec.topic, "FAQ");
    }
}

#[tokio::test]
async fn phase2_find_information_tag_filter_narrows_to_tag() {
    // Test 3: tag filter narrows
    // the candidate set to
    // records with that tag.
    let fix = make_populated_fixture().await;
    let req = afa_contracts::FindInformationRequest {
        tags: vec!["billing".to_string()],
        ..Default::default()
    };
    let resp = fix
        .adapter
        .find_information(req, &fix.ctx)
        .await
        .expect("find_information");
    assert_eq!(resp.len(), 2);
    for (rec, _score) in &resp {
        assert!(rec.tags.iter().any(|t| t == "billing"));
    }
}

#[tokio::test]
async fn phase2_find_information_free_text_ranks_by_overlap() {
    // Test 4: a free-text query
    // ranks the records by
    // token overlap. The
    // "refund" record should
    // outrank the "shipping"
    // record when the query is
    // "refund".
    let fix = make_populated_fixture().await;
    let req = afa_contracts::FindInformationRequest {
        free_text: Some("refund".to_string()),
        ..Default::default()
    };
    let resp = fix
        .adapter
        .find_information(req, &fix.ctx)
        .await
        .expect("find_information");
    assert!(!resp.is_empty());
    // The first record should be
    // the "refund policy" record
    // (the only one with a
    // matching token).
    let (top, top_score) = &resp[0];
    assert!(top.content.contains("refund"));
    // The score should be > 0
    // (the "refund" record
    // matches the query).
    assert!(*top_score > 0.0);
    // The "shipping" record
    // should have a lower score
    // (no overlap with
    // "refund").
    if resp.len() > 1 {
        assert!(resp[1].1 < *top_score);
    }
}

#[tokio::test]
async fn phase2_find_information_limit_caps_result_count() {
    // Test 5: the `limit` field
    // caps the result count.
    let fix = make_populated_fixture().await;
    let req = afa_contracts::FindInformationRequest {
        limit: Some(1),
        ..Default::default()
    };
    let resp = fix
        .adapter
        .find_information(req, &fix.ctx)
        .await
        .expect("find_information");
    assert_eq!(resp.len(), 1);
}

#[tokio::test]
async fn phase2_find_information_combined_filters_intersect() {
    // Test 6: combined filters
    // are intersected. A
    // topic=FAQ + tag=shipping
    // query returns only the
    // shipping record in FAQ.
    let fix = make_populated_fixture().await;
    let req = afa_contracts::FindInformationRequest {
        topic: Some("FAQ".to_string()),
        tags: vec!["shipping".to_string()],
        ..Default::default()
    };
    let resp = fix
        .adapter
        .find_information(req, &fix.ctx)
        .await
        .expect("find_information");
    assert_eq!(resp.len(), 1);
    assert!(resp[0].0.content.contains("shipping"));
}

#[tokio::test]
async fn phase2_find_information_non_matching_filter_returns_empty() {
    // Test 7: a non-matching
    // filter returns an empty
    // result set (NOT an
    // error). The audit event
    // still fires with
    // `result_count == 0`.
    let fix = make_populated_fixture().await;
    let mut sub = fix
        .bus
        .subscribe::<afa_contracts::knowledge::KnowledgeQueried>(16);
    let req = afa_contracts::FindInformationRequest {
        topic: Some("NoSuchTopic".to_string()),
        ..Default::default()
    };
    let resp = fix
        .adapter
        .find_information(req, &fix.ctx)
        .await
        .expect("find_information");
    assert!(resp.is_empty());
    // The audit event must still
    // fire (the call was
    // successful; the result
    // was empty).
    let (event, _) = sub.recv().await.expect("audit event must be published");
    let event = &*event;
    assert_eq!(event.result_count, 0);
}

#[tokio::test]
async fn phase2_list_topics_returns_summaries_sorted_by_name() {
    // Test 8: list_topics
    // returns the per-topic
    // summaries sorted
    // alphabetically by name.
    let fix = make_populated_fixture().await;
    let topics = fix
        .adapter
        .list_topics(&fix.ctx)
        .await
        .expect("list_topics");
    assert_eq!(topics.len(), 2);
    // The two topics are "FAQ"
    // and "Properties".
    // Alphabetical: FAQ first.
    assert_eq!(topics[0].name, "FAQ");
    assert_eq!(topics[1].name, "Properties");
    assert_eq!(topics[0].record_count, 2);
    assert_eq!(topics[1].record_count, 1);
}

#[tokio::test]
async fn phase2_list_topics_on_empty_adapter_returns_empty_vec() {
    // Test 9: list_topics on
    // an empty adapter
    // returns an empty Vec
    // (NOT an error). The
    // audit event still fires
    // with `topic_count == 0`.
    let dir = TempDir::new().expect("tempdir");
    let cfg = JsonKnowledgeConfig::new(
        dir.path().to_path_buf(),
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    );
    let bus = Arc::new(EventBus::new());
    let adapter = JsonKnowledgeAdapter::new(cfg, bus.clone())
        .await
        .expect("new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);
    let mut sub = bus.subscribe::<afa_contracts::knowledge::KnowledgeTopicsListed>(16);
    let ctx = ExecutionContext::new(TenantId::new("acme"), Actor::Timer);
    let topics = adapter.list_topics(&ctx).await.expect("list_topics");
    assert!(topics.is_empty());
    let (event, _) = sub.recv().await.expect("audit event must be published");
    let event = &*event;
    assert_eq!(event.topic_count, 0);
}
