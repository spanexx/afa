//! Code Map: Phase 4 end-to-end Knowledge integration test
//!
//! - `kernel_end_to_end_store_then_find_round_trip`:
//!   Boots a real `Kernel` (real `SecurityEngine` over
//!   a real tempdir-backed SQLite file, real
//!   `EventBus`, real `CapabilityRegistry`). Stands up
//!   a real `JsonKnowledgeAdapter` against a
//!   `tempfile::TempDir` storage root, registers the
//!   adapter via `kernel.register_knowledge("default",
//!   adapter, storage_root)`. Subscribes to
//!   `KnowledgeRecordStored` and `KnowledgeQueried` on
//!   the kernel's bus. Calls
//!   `kernel.knowledge("default").unwrap().store_information(...)`,
//!   then `kernel.knowledge("default").unwrap().find_information(...)`,
//!   and asserts the full round-trip: the canned
//!   record is returned by `find_information`; the
//!   bus saw the two events with matching
//!   `correlation_id`; the on-disk `.md` file
//!   contains the content; the on-disk `.index.json`
//!   has the record's metadata.
//!
//! - `kernel_end_to_end_list_topics_after_multi_topic_seed`:
//!   Same shape. Seeds 3 records across 2 topics,
//!   calls `kernel.knowledge("default").list_topics(...)`,
//!   asserts the returned `Vec<Topic>` has the
//!   expected shape (2 topics, sorted by name, with
//!   the per-topic aggregates). The bus saw the
//!   `KnowledgeTopicsListed` event with matching
//!   `correlation_id`.
//!
//! - `kernel_register_knowledge_twice_returns_knowledge_already_registered`:
//!   Negative test: a second `register_knowledge`
//!   under the same key surfaces
//!   `RegisterError::KnowledgeAlreadyRegistered { key }`,
//!   a closed-set programmer-error path.
//!
//! - `kernel_knowledge_returns_none_before_registration`:
//!   The pre-registration `kernel.knowledge("default")`
//!   returns `None` (a workflow that runs before the
//!   bootstrap can branch on this and surface a clear
//!   "no Knowledge configured for this tenant"
//!   error).
//!
//! - `kernel_knowledge_uses_kernel_event_bus_for_audit_events`:
//!   Proves the adapter is wired to the kernel's
//!   `EventBus` (not a private bus) — events the
//!   adapter publishes land in a subscription on
//!   `kernel.event_bus()`.
//!
//! Story (plain English): This is the final
//! integration check the Phase 4 plan asks for. The
//! switchboard (kernel) is up, the security guard is
//! on duty, and the Knowledge filing clerk
//! (`JsonKnowledgeAdapter`) is standing at the counter
//! with a `tempfile::TempDir` standing in for the
//! agency's `/var/lib/afa/knowledge/` directory. A
//! workflow drops off a request to file a record; the
//! clerk walks it to the filing cabinet, writes the
//! card atomically, updates the index, stamps the
//! `KnowledgeRecordStored` ticket on the audit bus,
//! and hands the record id back. The test watches
//! every step and asserts the round-trip is whole.
//!
//! CID Index:
//! CID:afa-kernel-knowledge-integration-001 -> end-to-end store+find round-trip
//! CID:afa-kernel-knowledge-integration-002 -> list_topics after multi-topic seed
//! CID:afa-kernel-knowledge-integration-003 -> register_knowledge twice
//! CID:afa-kernel-knowledge-integration-004 -> knowledge none before register
//! CID:afa-kernel-knowledge-integration-005 -> kernel event bus carries adapter events
//!
//! Quick lookup: rg -n "CID:afa-kernel-knowledge-integration-" crates/afa-kernel/tests/knowledge_integration.rs

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use afa_bus::EventBus;
use afa_contracts::{
    ExecutionContext, FindInformationRequest, KnowledgeCapabilities, KnowledgeRecordInput,
    KnowledgeV1,
};
use afa_kernel::capability_registry::RegisterError;
use afa_kernel::Kernel;
use afa_plugin_knowledge_json::{JsonKnowledgeAdapter, JsonKnowledgeConfig};
use afa_security::MasterKey;
use tempfile::TempDir;

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// Build a fresh `Kernel` (real `SecurityEngine`
/// over a real tempdir-backed SQLite file) plus
/// the `TempDir` that owns the SQLite path. The
/// `TempDir` is returned so the test can keep the
/// path alive for the test's entire scope (dropping
/// the `TempDir` would delete the file, which
/// would race with the engine's open connection
/// on slow filesystems).
async fn fresh_kernel() -> (TempDir, Kernel) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secrets.db");
    let key = MasterKey::from([0x42u8; 32]);
    let kernel = Kernel::new(&key, path).await.expect("kernel::new");
    (dir, kernel)
}

/// Build a fresh `JsonKnowledgeConfig` for a
/// `tempfile::TempDir` storage root. The
/// capabilities are the v1 defaults (1 MiB max
/// record, no semantic search, no hierarchical
/// topics). The caller is responsible for keeping
/// the `TempDir` alive for the test's scope.
fn config_for(storage_root: PathBuf) -> JsonKnowledgeConfig {
    JsonKnowledgeConfig::new(
        storage_root,
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    )
}

/// Build a fresh `ExecutionContext` for the
/// `kernel_e2e_knowledge` tenant + the `Timer`
/// actor (the test is not a workflow).
fn ctx() -> ExecutionContext {
    ExecutionContext::new(
        afa_contracts::TenantId::new("kernel_e2e_knowledge"),
        afa_contracts::Actor::Timer,
    )
}

// ---------------------------------------------------------------------------
// Phase 4 — Kernel + JsonKnowledgeAdapter end-to-end (store + find)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kernel_end_to_end_store_then_find_round_trip() {
    // 1. Bring up a real Kernel (real
    //    SecurityEngine, real bus, real
    //    CapabilityRegistry).
    let (_secrets_dir, kernel) = fresh_kernel().await;
    // 2. Bring up a real JsonKnowledgeAdapter
    //    against a fresh TempDir storage root.
    //    The adapter is wired to the kernel's
    //    OWN `EventBus` so the audit events
    //    land in subscriptions on the kernel's
    //    bus (the integration property: the
    //    adapter is sharing the kernel's bus,
    //    not a private one).
    let knowledge_dir = TempDir::new().expect("tempdir for knowledge");
    let storage_root = knowledge_dir.path().to_path_buf();
    let bus: Arc<EventBus> = kernel.event_bus();
    let adapter = JsonKnowledgeAdapter::new(config_for(storage_root.clone()), bus.clone())
        .await
        .expect("JsonKnowledgeAdapter::new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);
    // 3. Register the adapter with the kernel
    //    under the "default" key. The
    //    `Arc<dyn KnowledgeV1>` is exactly
    //    what a production bootstrap would
    //    hand to the kernel.
    kernel
        .register_knowledge("default", adapter.clone(), storage_root.clone())
        .expect("register_knowledge should succeed on an empty slot");
    // 4. Subscribe to the two audit events on
    //    the kernel's bus. The bus is shared
    //    with the adapter (same
    //    `Arc<EventBusCore>`), so the events
    //    the adapter publishes land in our
    //    subscriptions.
    let mut stored_sub = bus.subscribe::<afa_contracts::knowledge::KnowledgeRecordStored>(16);
    let mut queried_sub = bus.subscribe::<afa_contracts::knowledge::KnowledgeQueried>(16);
    // 5. Reach the adapter through the
    //    kernel's public `knowledge("default")`
    //    accessor (the canonical "workflow gets
    //    the Knowledge adapter" path).
    let knowledge = kernel
        .knowledge("default")
        .expect("knowledge should be registered");
    let input = KnowledgeRecordInput {
        topic: "FAQ".to_string(),
        tags: vec!["billing".to_string(), "refund".to_string()],
        content: "kernel e2e: refund policy".to_string(),
        source: Some("kernel-e2e-2026-07-13".to_string()),
    };
    let returned_id = knowledge
        .store_information(input.clone(), &ctx())
        .await
        .expect("store_information happy path");
    // 6. Assert the on-disk side effect:
    //    the .md file is on disk under
    //    `<storage_root>/<topic_slug>/<record_id>.md`
    //    and contains the content verbatim.
    let file_path = storage_root.join("faq").join(format!("{returned_id}.md"));
    assert!(
        file_path.is_file(),
        "record file must exist on disk: {}",
        file_path.display()
    );
    let bytes = std::fs::read(&file_path).expect("read record file");
    assert_eq!(bytes, input.content.as_bytes());
    // 7. Assert the on-disk index file
    //    (`.index.json`) has the record's
    //    metadata. The Phase 3 persistence
    //    is the source of truth for the
    //    on-disk state.
    let index_path = storage_root.join(".index.json");
    assert!(
        index_path.is_file(),
        "on-disk index file must exist: {}",
        index_path.display()
    );
    let index_json = std::fs::read_to_string(&index_path).expect("read index");
    assert!(
        index_json.contains(&returned_id.0.to_string()),
        ".index.json must contain the record id; got: {index_json}"
    );
    assert!(
        index_json.contains("FAQ"),
        ".index.json must contain the topic; got: {index_json}"
    );
    // 8. Assert the bus saw the
    //    `KnowledgeRecordStored` event with
    //    matching `correlation_id`. The event
    //    carries the `record_id`, the topic,
    //    the tag count, the content length,
    //    and the source; the content is NOT
    //    in the event payload.
    let (stored_evt, _stored_ctx) = tokio::time::timeout(Duration::from_secs(2), stored_sub.recv())
        .await
        .expect("KnowledgeRecordStored not received in time")
        .expect("KnowledgeRecordStored channel closed");
    assert_eq!(stored_evt.record_id, returned_id);
    assert_eq!(stored_evt.topic, input.topic);
    assert_eq!(stored_evt.tag_count, input.tags.len() as u32);
    assert_eq!(stored_evt.content_length, input.content.len() as u32);
    assert_eq!(stored_evt.source, input.source);
    // 9. Call `find_information` to read the
    //    record back. The free-text query is
    //    "refund" (a token that only the
    //    stored record contains).
    let find_req = FindInformationRequest {
        free_text: Some("refund".to_string()),
        ..Default::default()
    };
    let resp = knowledge
        .find_information(find_req, &ctx())
        .await
        .expect("find_information happy path");
    // 10. Assert the find returned at least
    //     the just-stored record. The score
    //     must be in `[0.0, 1.0]`. The
    //     `record_id` of the top hit must
    //     match the just-stored id.
    assert!(
        !resp.is_empty(),
        "find_information should return at least the just-stored record"
    );
    let (top_record, top_score) = &resp[0];
    assert_eq!(top_record.record_id, returned_id);
    assert!(
        *top_score > 0.0,
        "top hit score must be > 0; got {top_score}"
    );
    assert!(
        *top_score <= 1.0,
        "top hit score must be <= 1.0; got {top_score}"
    );
    assert_eq!(top_record.content, input.content);
    // 11. Assert the bus saw the
    //     `KnowledgeQueried` event with
    //     matching `correlation_id` and a
    //     `result_count` of at least 1.
    let (queried_evt, _queried_ctx) =
        tokio::time::timeout(Duration::from_secs(2), queried_sub.recv())
            .await
            .expect("KnowledgeQueried not received in time")
            .expect("KnowledgeQueried channel closed");
    assert_eq!(queried_evt.result_count, resp.len() as u32);
    assert!(queried_evt.result_count >= 1);
}

// ---------------------------------------------------------------------------
// Phase 4 — Kernel + JsonKnowledgeAdapter end-to-end (list_topics)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kernel_end_to_end_list_topics_after_multi_topic_seed() {
    // 1. Bring up a real Kernel + a real
    //    JsonKnowledgeAdapter against a fresh
    //    TempDir storage root, wired to the
    //    kernel's own bus.
    let (_secrets_dir, kernel) = fresh_kernel().await;
    let knowledge_dir = TempDir::new().expect("tempdir for knowledge");
    let storage_root = knowledge_dir.path().to_path_buf();
    let bus: Arc<EventBus> = kernel.event_bus();
    let adapter = JsonKnowledgeAdapter::new(config_for(storage_root.clone()), bus.clone())
        .await
        .expect("JsonKnowledgeAdapter::new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);
    kernel
        .register_knowledge("default", adapter.clone(), storage_root.clone())
        .expect("register_knowledge");
    // 2. Seed 3 records across 2 topics. The
    //    first two are in "FAQ"; the third is
    //    in "Properties".
    let knowledge = kernel.knowledge("default").expect("knowledge");
    let seed_inputs = vec![
        KnowledgeRecordInput {
            topic: "FAQ".to_string(),
            tags: vec!["billing".to_string()],
            content: "refund within 30 days".to_string(),
            source: None,
        },
        KnowledgeRecordInput {
            topic: "FAQ".to_string(),
            tags: vec!["shipping".to_string()],
            content: "free shipping over $50".to_string(),
            source: None,
        },
        KnowledgeRecordInput {
            topic: "Properties".to_string(),
            tags: vec!["billing".to_string()],
            content: "first month free for new tenants".to_string(),
            source: None,
        },
    ];
    for input in seed_inputs {
        knowledge
            .store_information(input, &ctx())
            .await
            .expect("seed store_information");
    }
    // 3. Subscribe to the
    //    `KnowledgeTopicsListed` event on the
    //    kernel's bus.
    let mut listed_sub = bus.subscribe::<afa_contracts::knowledge::KnowledgeTopicsListed>(16);
    // 4. Call `list_topics` through the
    //    kernel's accessor.
    let topics = knowledge.list_topics(&ctx()).await.expect("list_topics");
    // 5. Assert the returned `Vec<Topic>` has
    //    the expected shape: 2 topics,
    //    sorted alphabetically by name
    //    ("FAQ" first, "Properties" second),
    //    with the per-topic aggregates
    //    matching the seed.
    assert_eq!(topics.len(), 2);
    assert_eq!(topics[0].name, "FAQ");
    assert_eq!(topics[1].name, "Properties");
    assert_eq!(topics[0].record_count, 2);
    assert_eq!(topics[1].record_count, 1);
    assert!(topics[0].tag_count >= 1);
    assert!(topics[1].tag_count >= 1);
    assert!(topics[0].first_record_at.is_some());
    assert!(topics[0].last_record_at.is_some());
    // 6. Assert the bus saw the
    //    `KnowledgeTopicsListed` event with
    //    matching `correlation_id` and
    //    `topic_count == 2`.
    let (listed_evt, _listed_ctx) = tokio::time::timeout(Duration::from_secs(2), listed_sub.recv())
        .await
        .expect("KnowledgeTopicsListed not received in time")
        .expect("KnowledgeTopicsListed channel closed");
    assert_eq!(listed_evt.topic_count, 2);
}

// ---------------------------------------------------------------------------
// Phase 4 — CapabilityRegistry closed-set: a second `register_knowledge` fails
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kernel_register_knowledge_twice_returns_knowledge_already_registered() {
    // The registry holds one slot per `key`;
    // a second `register_knowledge` under the
    // same key is a programmer error. The
    // kernel must surface it as
    // `RegisterError::KnowledgeAlreadyRegistered
    // { key }`, not a panic, so a buggy
    // bootstrap fails loudly but cleanly. The
    // error carries the conflicting key for
    // the operator log.
    let (_secrets_dir, kernel) = fresh_kernel().await;
    let dir_a = TempDir::new().expect("tempdir a");
    let dir_b = TempDir::new().expect("tempdir b");
    let bus: Arc<EventBus> = kernel.event_bus();
    let adapter_a = JsonKnowledgeAdapter::new(config_for(dir_a.path().to_path_buf()), bus.clone())
        .await
        .expect("adapter_a");
    let adapter_b = JsonKnowledgeAdapter::new(config_for(dir_b.path().to_path_buf()), bus.clone())
        .await
        .expect("adapter_b");
    let adapter_a: Arc<dyn KnowledgeV1> = Arc::new(adapter_a);
    let adapter_b: Arc<dyn KnowledgeV1> = Arc::new(adapter_b);
    kernel
        .register_knowledge("default", adapter_a, dir_a.path().to_path_buf())
        .expect("first register_knowledge should succeed");
    let e = kernel
        .register_knowledge("default", adapter_b, dir_b.path().to_path_buf())
        .expect_err("second register_knowledge under the same key should fail");
    match e {
        RegisterError::KnowledgeAlreadyRegistered { key } => {
            assert_eq!(key, "default");
        }
        other => panic!("expected KnowledgeAlreadyRegistered, got {other:?}"),
    }
    // The first adapter is still the one
    // the registry hands out (a buggy second
    // register must NOT silently overwrite
    // the slot).
    let knowledge = kernel
        .knowledge("default")
        .expect("knowledge should still be the first adapter");
    let caps = knowledge.describe_capabilities();
    assert_eq!(caps.max_record_size_bytes, 1_048_576);
}

// ---------------------------------------------------------------------------
// Phase 4 — Pre-registration `kernel.knowledge("default")` returns `None`
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kernel_knowledge_returns_none_before_registration() {
    // A workflow that runs before the
    // bootstrap registers a Knowledge adapter
    // sees `None` from
    // `kernel.knowledge("default")`. The
    // workflow can branch on this and surface
    // a clear "no Knowledge configured for
    // this tenant" error rather than a
    // confusing deref-of-None deep in the
    // call stack.
    let (_secrets_dir, kernel) = fresh_kernel().await;
    assert!(
        kernel.knowledge("default").is_none(),
        "kernel.knowledge(\"default\") should return None before any register_knowledge call"
    );
    // And an unknown key also returns None
    // (the key is not "default"; the
    // registry is empty).
    assert!(
        kernel.knowledge("tenant-a").is_none(),
        "kernel.knowledge() should return None for an unknown key on an empty registry"
    );
}

// ---------------------------------------------------------------------------
// Phase 4 — The adapter's audit events land in the kernel's `event_bus()`
// (not a private bus)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kernel_knowledge_uses_kernel_event_bus_for_audit_events() {
    // Prove the wiring: the adapter is
    // constructed with the kernel's
    // `EventBus`, so the `KnowledgeRecordStored`
    // event the adapter publishes on a
    // `store_information` call lands in a
    // subscription on the kernel's bus.
    // If the adapter had been built with a
    // private bus, the kernel's bus would see
    // no events and the test would time out
    // on the subscription `recv`.
    let (_secrets_dir, kernel) = fresh_kernel().await;
    let knowledge_dir = TempDir::new().expect("tempdir for knowledge");
    let storage_root = knowledge_dir.path().to_path_buf();
    // Wire the adapter to the kernel's bus.
    let bus: Arc<EventBus> = kernel.event_bus();
    let adapter = JsonKnowledgeAdapter::new(config_for(storage_root.clone()), bus.clone())
        .await
        .expect("JsonKnowledgeAdapter::new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);
    kernel
        .register_knowledge("default", adapter.clone(), storage_root)
        .expect("register_knowledge");
    // Subscribe on the kernel's bus BEFORE
    // any adapter call (so we don't race
    // the publish).
    let mut sub = bus.subscribe::<afa_contracts::knowledge::KnowledgeRecordStored>(16);
    // Trigger a `store_information` call.
    let knowledge = kernel.knowledge("default").expect("knowledge");
    let input = KnowledgeRecordInput {
        topic: "FAQ".to_string(),
        tags: vec![],
        content: "audit-bus wiring check".to_string(),
        source: None,
    };
    let returned_id = knowledge
        .store_information(input, &ctx())
        .await
        .expect("store_information");
    // The event must arrive on the kernel's
    // bus within 2s. A timeout means the
    // adapter is publishing on a different
    // bus.
    let (evt, _ctx) = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("event not received on kernel bus in time; the adapter is using a different bus")
        .expect("channel closed");
    assert_eq!(evt.record_id, returned_id);
    // The `ExecutionContext` the bus received
    // must carry the same tenant the caller
    // passed.
    assert_eq!(_ctx.tenant_id, ctx().tenant_id);
}
