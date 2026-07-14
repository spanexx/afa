use std::path::PathBuf;
use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::execution_context::Actor;
use afa_contracts::ids::TenantId;
use afa_contracts::knowledge::events::{
    KnowledgeQueried, KnowledgeRecordStored, KnowledgeTopicsListed,
};
use afa_contracts::{
    ExecutionContext, FindInformationRequest, KnowledgeCapabilities, KnowledgeRecordInput,
    KnowledgeV1,
};
use afa_plugin_knowledge_json::{JsonKnowledgeAdapter, JsonKnowledgeConfig};
use tempfile::TempDir;

fn make_ctx() -> ExecutionContext {
    ExecutionContext::new(TenantId::new("test-tenant"), Actor::Timer)
}

fn make_config(storage_root: PathBuf) -> JsonKnowledgeConfig {
    JsonKnowledgeConfig::new(
        storage_root,
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    )
}

#[tokio::test]
async fn store_information_publishes_event_with_expected_fields() {
    let dir = TempDir::new().expect("tempdir");
    let cfg = make_config(dir.path().to_path_buf());
    let bus = Arc::new(EventBus::new());
    let adapter = JsonKnowledgeAdapter::new(cfg, bus.clone())
        .await
        .expect("new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);

    let mut sub = bus.subscribe::<KnowledgeRecordStored>(16);

    let secret_body = "this content is secret and must not appear in the event";
    let id = adapter
        .store_information(
            KnowledgeRecordInput {
                topic: "FAQ".to_string(),
                tags: vec!["billing".to_string(), "FAQ".to_string()],
                content: secret_body.to_string(),
                source: Some("manual-entry".to_string()),
            },
            &make_ctx(),
        )
        .await
        .expect("store_information");

    let (event, _ctx) = sub.recv().await.expect("event must be published");
    let ev = &*event;
    assert_eq!(ev.record_id, id);
    assert_eq!(ev.topic, "FAQ");
    assert_eq!(ev.tag_count, 2, "two distinct tags after dedup");
    assert_eq!(ev.content_length, secret_body.len() as u32);
    let event_debug = format!("{ev:?}");
    assert!(
        !event_debug.contains(secret_body),
        "event payload leaked record body: {event_debug}"
    );
}

#[tokio::test]
async fn find_information_publishes_event_with_no_payload_leak() {
    let dir = TempDir::new().expect("tempdir");
    let cfg = make_config(dir.path().to_path_buf());
    let bus = Arc::new(EventBus::new());
    let adapter = JsonKnowledgeAdapter::new(cfg, bus.clone())
        .await
        .expect("new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);

    let mut sub = bus.subscribe::<KnowledgeQueried>(16);

    let secret_query = "find me the secret billing question";
    let _ = adapter
        .find_information(
            FindInformationRequest {
                free_text: Some(secret_query.to_string()),
                topic: None,
                tags: vec![],
                limit: Some(10),
            },
            &make_ctx(),
        )
        .await
        .expect("find_information");

    let (event, _ctx) = sub.recv().await.expect("event must be published");
    let ev = &*event;
    assert_eq!(ev.result_count, 0);
    let event_debug = format!("{ev:?}");
    assert!(
        !event_debug.contains(secret_query),
        "event payload leaked free_text query: {event_debug}"
    );
}

#[tokio::test]
async fn list_topics_publishes_event_with_topic_count() {
    let dir = TempDir::new().expect("tempdir");
    let cfg = make_config(dir.path().to_path_buf());
    let bus = Arc::new(EventBus::new());
    let adapter = JsonKnowledgeAdapter::new(cfg, bus.clone())
        .await
        .expect("new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);

    for (topic, body) in [("FAQ", "body 1"), ("Properties", "body 2")] {
        adapter
            .store_information(
                KnowledgeRecordInput {
                    topic: topic.to_string(),
                    tags: vec![],
                    content: body.to_string(),
                    source: None,
                },
                &make_ctx(),
            )
            .await
            .expect("store");
    }

    let mut sub = bus.subscribe::<KnowledgeTopicsListed>(16);
    let _ = adapter.list_topics(&make_ctx()).await.expect("list_topics");

    let (event, _ctx) = sub.recv().await.expect("event must be published");
    let ev = &*event;
    assert_eq!(ev.topic_count, 2, "two topics present");
    let event_debug = format!("{ev:?}");
    assert!(
        !event_debug.contains("FAQ") && !event_debug.contains("Properties"),
        "event payload leaked topic names: {event_debug}"
    );
}

#[tokio::test]
async fn each_call_publishes_exactly_one_event() {
    let dir = TempDir::new().expect("tempdir");
    let cfg = make_config(dir.path().to_path_buf());
    let bus = Arc::new(EventBus::new());
    let adapter = JsonKnowledgeAdapter::new(cfg, bus.clone())
        .await
        .expect("new");
    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);

    let mut stored_sub = bus.subscribe::<KnowledgeRecordStored>(16);
    let mut queried_sub = bus.subscribe::<KnowledgeQueried>(16);
    let mut listed_sub = bus.subscribe::<KnowledgeTopicsListed>(16);

    adapter
        .store_information(
            KnowledgeRecordInput {
                topic: "FAQ".to_string(),
                tags: vec![],
                content: "body".to_string(),
                source: None,
            },
            &make_ctx(),
        )
        .await
        .expect("store");
    adapter
        .find_information(
            FindInformationRequest {
                free_text: None,
                topic: None,
                tags: vec![],
                limit: Some(10),
            },
            &make_ctx(),
        )
        .await
        .expect("find");
    adapter.list_topics(&make_ctx()).await.expect("list");

    let (_s, _) = stored_sub.recv().await.expect("one store event");
    let (_q, _) = queried_sub.recv().await.expect("one search event");
    let (_l, _) = listed_sub.recv().await.expect("one list event");
}
