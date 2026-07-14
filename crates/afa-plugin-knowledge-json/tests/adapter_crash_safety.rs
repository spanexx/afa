use std::path::PathBuf;
use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::execution_context::Actor;
use afa_contracts::ids::TenantId;
use afa_contracts::{ExecutionContext, KnowledgeCapabilities, KnowledgeV1, RecordId};
use afa_plugin_knowledge_json::{JsonKnowledgeAdapter, JsonKnowledgeConfig};
use tempfile::TempDir;

fn make_ctx() -> ExecutionContext {
    ExecutionContext::new(TenantId::new("test-tenant"), Actor::Timer)
}

#[tokio::test]
async fn boot_recovers_from_corrupt_index_json() {
    let dir = TempDir::new().expect("tempdir");
    let storage_root: PathBuf = dir.path().to_path_buf();
    let topic_dir = storage_root.join("billing");
    tokio::fs::create_dir_all(&topic_dir)
        .await
        .expect("mkdir topic dir");
    let id = RecordId(uuid::Uuid::new_v4());
    let body = b"recovered body content";
    tokio::fs::write(topic_dir.join(format!("{id}.md")), body)
        .await
        .expect("write body");

    tokio::fs::write(
        storage_root.join(".index.json"),
        b"{\"version\": 1, NOT VALID JSON",
    )
    .await
    .expect("plant corrupt index");

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
        .expect("boot recovers from corrupt index.json");

    let adapter: Arc<dyn KnowledgeV1> = Arc::new(adapter);
    let resp = adapter
        .find_information(
            afa_contracts::FindInformationRequest {
                free_text: None,
                topic: None,
                tags: vec![],
                limit: Some(100),
            },
            &make_ctx(),
        )
        .await
        .expect("find_information");
    assert_eq!(resp.len(), 1, "one recovered record");
    assert_eq!(resp[0].0.record_id, id);
    assert_eq!(resp[0].0.content, "recovered body content");
    assert_eq!(resp[0].0.topic, "billing");
}

#[tokio::test]
async fn boot_removes_orphan_temp_files() {
    let dir = TempDir::new().expect("tempdir");
    let storage_root: PathBuf = dir.path().to_path_buf();
    let topic_dir = storage_root.join("faq");
    tokio::fs::create_dir_all(&topic_dir)
        .await
        .expect("mkdir topic dir");
    let orphan_a = topic_dir.join("abc.tmp.123");
    let orphan_b = topic_dir.join("def.tmp.456");
    tokio::fs::write(&orphan_a, b"half-written content a")
        .await
        .expect("plant orphan a");
    tokio::fs::write(&orphan_b, b"half-written content b")
        .await
        .expect("plant orphan b");
    assert!(orphan_a.exists());
    assert!(orphan_b.exists());

    let cfg = JsonKnowledgeConfig::new(
        storage_root.clone(),
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    );
    let bus = Arc::new(EventBus::new());
    let _adapter = JsonKnowledgeAdapter::new(cfg, bus.clone())
        .await
        .expect("boot");

    assert!(!orphan_a.exists(), "orphan a was removed");
    assert!(!orphan_b.exists(), "orphan b was removed");
}
