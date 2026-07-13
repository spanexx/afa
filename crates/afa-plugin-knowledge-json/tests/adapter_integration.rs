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
