use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::execution_context::Actor;
use afa_contracts::ids::TenantId;
use afa_contracts::{
    ExecutionContext, KnowledgeCapabilities, KnowledgeRecordInput, KnowledgeV1, RecordId,
};
use afa_plugin_knowledge_json::{JsonKnowledgeAdapter, JsonKnowledgeConfig};
use tempfile::TempDir;

fn make_ctx() -> ExecutionContext {
    ExecutionContext::new(TenantId::new("test-tenant"), Actor::Timer)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hundred_parallel_stores_lose_no_records() {
    let dir = TempDir::new().expect("tempdir");
    let storage_root: PathBuf = dir.path().to_path_buf();
    let cfg = JsonKnowledgeConfig::new(
        storage_root.clone(),
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    );
    let bus = Arc::new(EventBus::new());
    let adapter = Arc::new(
        JsonKnowledgeAdapter::new(cfg, bus.clone())
            .await
            .expect("new"),
    );
    let adapter_dyn: Arc<dyn KnowledgeV1> = adapter.clone();

    let n = 100usize;
    let mut handles = Vec::with_capacity(n);
    for i in 0..n {
        let adapter = adapter_dyn.clone();
        handles.push(tokio::spawn(async move {
            let input = KnowledgeRecordInput {
                topic: format!("Topic-{i:03}"),
                tags: vec![format!("tag-{i:03}")],
                content: format!("body of record {i}"),
                source: None,
            };
            adapter.store_information(input, &make_ctx()).await
        }));
    }
    let mut returned_ids: HashSet<RecordId> = HashSet::with_capacity(n);
    for h in handles {
        let id = h.await.expect("join").expect("store_information");
        assert!(returned_ids.insert(id), "duplicate record_id");
    }
    assert_eq!(returned_ids.len(), n);

    let resp = adapter_dyn
        .find_information(
            afa_contracts::FindInformationRequest {
                free_text: None,
                topic: None,
                tags: vec![],
                limit: Some(n as u32 + 1),
            },
            &make_ctx(),
        )
        .await
        .expect("find_information");
    let indexed: HashSet<RecordId> = resp.iter().map(|(r, _)| r.record_id).collect();
    assert_eq!(
        indexed, returned_ids,
        "all 100 returned record_ids are present in the index"
    );
    assert_eq!(resp.len(), n);

    // Fresh boot from disk: the
    // `.index.json` must contain
    // all 100 records.
    let cfg2 = JsonKnowledgeConfig::new(
        storage_root.clone(),
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        },
    );
    let bus2 = Arc::new(EventBus::new());
    let adapter2 = Arc::new(JsonKnowledgeAdapter::new(cfg2, bus2).await.expect("boot 2"));
    let adapter2_dyn: Arc<dyn KnowledgeV1> = adapter2.clone();
    let resp2 = adapter2_dyn
        .find_information(
            afa_contracts::FindInformationRequest {
                free_text: None,
                topic: None,
                tags: vec![],
                limit: Some(n as u32 + 1),
            },
            &make_ctx(),
        )
        .await
        .expect("find_information 2");
    assert_eq!(
        resp2.len(),
        n,
        "all 100 records recovered from .index.json on fresh boot"
    );
}
