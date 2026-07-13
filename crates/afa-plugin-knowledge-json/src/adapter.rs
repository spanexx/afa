//! Code Map: JsonKnowledgeAdapter
//! - `JsonKnowledgeAdapter`: The
//!   `KnowledgeV1`-implementing adapter the
//!   kernel hands out via
//!   `CapabilityRegistry::knowledge`. The
//!   concrete type wraps a
//!   `JsonKnowledgeConfig` (settings card),
//!   an `Arc<EventBus>` (audit trail), and
//!   an `Arc<tokio::sync::RwLock<InMemoryIndex>>`
//!   (the in-RAM search index). The
//!   `KnowledgeV1` trait is implemented
//!   here for the two methods Phase 1
//!   implements end-to-end:
//!   `store_information` and
//!   `describe_capabilities`. The other two
//!   trait methods (`find_information` and
//!   `list_topics`) are stubs that return
//!   `KnowledgeErrorV1::CapabilityUnsupported`
//!   — the JSON v1 backend has no find/list
//!   support (the README/PRD note that
//!   semantic search is a future-v2
//!   feature, and `list_topics` is
//!   also deferred). The stubs exist so
//!   the trait is fully implemented and
//!   the adapter can be held behind
//!   `Arc<dyn KnowledgeV1>`.
//!
//! Story (plain English): The adapter is the
//! part of the knowledge engine that talks to
//! the file system. It is the "filing clerk":
//! when the kernel asks it to store a new
//! knowledge record, the adapter checks the
//! input, writes the file atomically, updates
//! the in-memory index, and publishes an
//! audit event on the bus. The Phase 1
//! implementation focuses on the write path
//! and the capabilities handshake; the find
//! and list paths land in later phases.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-adapter-001 -> JsonKnowledgeAdapter
//! CID:afa-plugin-knowledge-json-adapter-002 -> JsonKnowledgeAdapter::new
//! CID:afa-plugin-knowledge-json-adapter-003 -> JsonKnowledgeAdapter::store_information
//! CID:afa-plugin-knowledge-json-adapter-004 -> JsonKnowledgeAdapter::describe_capabilities
//! CID:afa-plugin-knowledge-json-adapter-005 -> JsonKnowledgeAdapter::find_information (stub)
//! CID:afa-plugin-knowledge-json-adapter-006 -> JsonKnowledgeAdapter::list_topics (stub)
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-adapter-" crates/afa-plugin-knowledge-json/src/adapter.rs

use std::path::PathBuf;
use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::{
    ExecutionContext, FindInformationRequest, FindInformationResponse, KnowledgeCapabilities,
    KnowledgeErrorV1, KnowledgeRecordInput, KnowledgeV1, RecordId, Topic,
};
use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;

use crate::atomic_write::atomic_write;
use crate::config::JsonKnowledgeConfig;
use crate::index::InMemoryIndex;
use crate::topic_slug::topic_slug;

// CID:afa-plugin-knowledge-json-adapter-001 - JsonKnowledgeAdapter
// Purpose: The concrete adapter. The
// `Arc<tokio::sync::RwLock<InMemoryIndex>>`
// is the in-RAM index the search path
// consults; the `Arc<EventBus>` is the
// audit trail every successful
// `store_information` publishes to.
pub struct JsonKnowledgeAdapter {
    pub config: JsonKnowledgeConfig,
    pub event_bus: Arc<EventBus>,
    pub index: Arc<RwLock<InMemoryIndex>>,
}

// CID:afa-plugin-knowledge-json-adapter-002 - JsonKnowledgeAdapter::new
// Purpose: The boot sequence. The Phase 1
// implementation creates the storage root
// (if missing) and returns an empty index.
// The "load .index.json or rebuild" step
// from the IMPL Phase 1 task list is added
// in Phase 3 (the JSON index file). For
// Phase 1 the adapter always boots empty;
// the on-disk scan to repopulate the index
// after a clean restart lands in Phase 3.
impl JsonKnowledgeAdapter {
    /// Build a new `JsonKnowledgeAdapter`.
    /// The boot sequence is:
    /// 1. Verify `config.storage_root`
    ///    exists or can be created (the
    ///    "first boot" case).
    /// 2. Initialize an empty in-memory
    ///    index.
    /// 3. Return the adapter. The on-disk
    ///    scan to repopulate the index
    ///    after a clean restart lands in
    ///    Phase 3.
    pub async fn new(
        config: JsonKnowledgeConfig,
        event_bus: Arc<EventBus>,
    ) -> Result<Self, KnowledgeErrorV1> {
        // Step 1: ensure the storage root
        // exists. `create_dir_all` is
        // idempotent — a clean restart with
        // the root already present is a
        // no-op. An `io::Error` (e.g.
        // permission denied) is mapped to
        // `StorageUnavailable`.
        if let Err(e) = tokio::fs::create_dir_all(&config.storage_root).await {
            return Err(KnowledgeErrorV1::StorageUnavailable {
                topic: None,
                record_id: None,
                reason: format!(
                    "JsonKnowledgeAdapter::new: failed to create storage root {}: {e}",
                    config.storage_root.display()
                ),
            });
        }
        // Step 2: empty index.
        let index = Arc::new(RwLock::new(InMemoryIndex::new()));
        Ok(Self {
            config,
            event_bus,
            index,
        })
    }
}

// CID:afa-plugin-knowledge-json-adapter-003 - JsonKnowledgeAdapter::store_information
// Purpose: The write path. The Phase 1
// implementation:
// 1. Validates the input (size + topic
//    non-empty + content non-empty).
// 2. Slugifies the topic into a safe
//    on-disk directory name.
// 3. Generates the engine-assigned
//    `RecordId` (v4 UUID from OS entropy).
// 4. Ensures the per-topic subdirectory
//    exists under the storage root.
// 5. Writes the content to
//    `<storage_root>/<slug>/<record_id>.md`
//    via `atomic_write` (the temp-then-
//    rename helper from `atomic_write.rs`).
// 6. Updates the in-memory index under
//    the write lock.
// 7. Publishes a `KnowledgeRecordStored`
//    audit event on the bus.
//
// Errors are mapped to the
// `KnowledgeErrorV1` taxonomy:
// - `InvalidInput` for size > max
//   (`config.capabilities.max_record_size_bytes`)
//   or empty topic or empty content.
// - `StorageUnavailable` for any
//   filesystem I/O error during the
//   write sequence.
#[async_trait]
impl KnowledgeV1 for JsonKnowledgeAdapter {
    async fn find_information(
        &self,
        _request: FindInformationRequest,
        _ctx: &ExecutionContext,
    ) -> Result<FindInformationResponse, KnowledgeErrorV1> {
        // CID:afa-plugin-knowledge-json-adapter-005 - find_information stub
        // Phase 1 stub: the JSON v1 backend
        // does not support find. Returns
        // `CapabilityUnsupported` per the
        // contract — the caller may switch
        // to a different adapter rather
        // than retrying. Phase 2 populates
        // the body.
        Err(KnowledgeErrorV1::CapabilityUnsupported {
            topic: None,
            record_id: None,
            reason: "JsonKnowledgeAdapter: find_information is not supported in v1".to_string(),
        })
    }

    async fn store_information(
        &self,
        record: KnowledgeRecordInput,
        ctx: &ExecutionContext,
    ) -> Result<RecordId, KnowledgeErrorV1> {
        // Step 1: validate input. The
        // `content_len` is the byte length
        // of the content (UTF-8 byte
        // length); the `max_size` is the
        // engine's upper bound from
        // `KnowledgeCapabilities` (a `u32`
        // by the TRD §2.2.5 locked
        // shape).
        let content_len: usize = record.content.len();
        let max_size: u32 = self.config.capabilities.max_record_size_bytes;
        if content_len > max_size as usize {
            return Err(KnowledgeErrorV1::InvalidInput {
                topic: Some(record.topic.clone()),
                record_id: None,
                reason: format!("store_information: content size {content_len} > max {max_size}"),
            });
        }
        if record.topic.trim().is_empty() {
            return Err(KnowledgeErrorV1::InvalidInput {
                topic: Some(record.topic.clone()),
                record_id: None,
                reason: "store_information: topic is empty".to_string(),
            });
        }
        if record.content.is_empty() {
            return Err(KnowledgeErrorV1::InvalidInput {
                topic: Some(record.topic.clone()),
                record_id: None,
                reason: "store_information: content is empty".to_string(),
            });
        }

        // Step 2: slugify the topic.
        let slug = topic_slug(&record.topic);

        // Step 3: generate the engine-
        // assigned `RecordId` (v4 UUID from
        // OS entropy).
        let record_id = RecordId::new();

        // Step 4: ensure the per-topic
        // subdirectory exists.
        let topic_dir: PathBuf = self.config.storage_root.join(&slug);
        if let Err(e) = tokio::fs::create_dir_all(&topic_dir).await {
            return Err(KnowledgeErrorV1::StorageUnavailable {
                topic: Some(record.topic.clone()),
                record_id: Some(record_id.0),
                reason: format!(
                    "store_information: failed to create topic dir {}: {e}",
                    topic_dir.display()
                ),
            });
        }

        // Step 5: write the content to
        // `<storage_root>/<slug>/<record_id>.md`
        // via `atomic_write`. The filename
        // is the `Display` form of the
        // `RecordId` (the hyphenated
        // lowercase UUID) with a `.md`
        // extension.
        let target = topic_dir.join(format!("{record_id}.md"));
        if let Err(e) = atomic_write(&target, record.content.as_bytes()).await {
            return Err(KnowledgeErrorV1::StorageUnavailable {
                topic: Some(record.topic.clone()),
                record_id: Some(record_id.0),
                reason: format!(
                    "store_information: atomic_write failed for {}: {}",
                    target.display(),
                    e
                ),
            });
        }

        // Step 6: update the in-memory
        // index under the write lock.
        {
            let mut idx = self.index.write().await;
            idx.add_record(&record, &slug, record_id, content_len as u64);
        }

        // Step 7: publish the
        // `KnowledgeRecordStored` audit
        // event on the bus. The event
        // carries the `ExecutionContext`
        // metadata (correlation, tenant,
        // actor), the `record_id`, the
        // topic, the tag count, the
        // content length, and the source.
        // The event does NOT carry the
        // record content (audit-trail
        // safety).
        let tag_count: u32 = {
            // Distinct-tag count (after
            // dedup). v1 does not normalize
            // tag spelling, but two tags
            // that are equal as strings are
            // the same tag.
            let set: std::collections::BTreeSet<&String> = record.tags.iter().collect();
            set.len() as u32
        };
        let event = afa_contracts::knowledge::KnowledgeRecordStored {
            correlation_id: ctx.correlation_id,
            tenant_id: ctx.tenant_id.clone(),
            actor: ctx.actor.clone(),
            timestamp: Utc::now(),
            record_id,
            topic: record.topic.clone(),
            tag_count,
            content_length: content_len as u32,
            source: record.source.clone(),
        };
        self.event_bus.publish(event, ctx.clone()).await;

        Ok(record_id)
    }

    async fn list_topics(&self, _ctx: &ExecutionContext) -> Result<Vec<Topic>, KnowledgeErrorV1> {
        // CID:afa-plugin-knowledge-json-adapter-006 - list_topics stub
        // Phase 1 stub: the JSON v1 backend
        // does not support list_topics.
        // Returns `CapabilityUnsupported`
        // per the contract. Phase 4
        // populates the body.
        Err(KnowledgeErrorV1::CapabilityUnsupported {
            topic: None,
            record_id: None,
            reason: "JsonKnowledgeAdapter: list_topics is not supported in v1".to_string(),
        })
    }

    // CID:afa-plugin-knowledge-json-adapter-004 - JsonKnowledgeAdapter::describe_capabilities
    // Purpose: Returns the
    // `KnowledgeCapabilities` the adapter was
    // configured with. The trait method is
    // `fn` (synchronous, not async — no
    // I/O, no ctx). Phase 1 stores the
    // capabilities in the config; the
    // method is a thin accessor.
    fn describe_capabilities(&self) -> KnowledgeCapabilities {
        self.config.capabilities.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use afa_contracts::execution_context::Actor;
    use afa_contracts::ids::TenantId;

    fn test_capabilities() -> KnowledgeCapabilities {
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        }
    }

    fn make_ctx() -> ExecutionContext {
        ExecutionContext::new(TenantId::new("t1"), Actor::Timer)
    }

    #[tokio::test]
    async fn new_creates_storage_root_and_empty_index() {
        // Boot the adapter in a fresh
        // tempdir; the storage root must
        // exist and the index must be
        // empty.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cfg = JsonKnowledgeConfig::new(dir.path().to_path_buf(), test_capabilities());
        let bus = Arc::new(EventBus::new());
        let adapter = JsonKnowledgeAdapter::new(cfg, bus).await.expect("new");
        let idx = adapter.index.read().await;
        assert_eq!(idx.topic_count(), 0);
        assert_eq!(idx.total_record_count(), 0);
        assert!(dir.path().is_dir());
    }

    #[tokio::test]
    async fn describe_capabilities_returns_configured_shape() {
        // The Phase 1 describe_capabilities
        // path: the adapter reports the
        // shape it was built with.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cfg = JsonKnowledgeConfig::new(
            dir.path().to_path_buf(),
            KnowledgeCapabilities {
                max_record_size_bytes: 4096,
                supports_semantic_search: false,
                supports_hierarchical_topics: false,
            },
        );
        let bus = Arc::new(EventBus::new());
        let adapter = JsonKnowledgeAdapter::new(cfg, bus).await.expect("new");
        let caps = adapter.describe_capabilities();
        assert_eq!(caps.max_record_size_bytes, 4096);
        assert!(!caps.supports_semantic_search);
        assert!(!caps.supports_hierarchical_topics);
    }

    #[tokio::test]
    async fn store_information_rejects_oversized_content() {
        // The InvalidInput path: content
        // larger than `max_record_size_bytes`
        // is rejected with InvalidInput.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cfg = JsonKnowledgeConfig::new(
            dir.path().to_path_buf(),
            KnowledgeCapabilities {
                max_record_size_bytes: 10,
                supports_semantic_search: false,
                supports_hierarchical_topics: false,
            },
        );
        let bus = Arc::new(EventBus::new());
        let adapter = JsonKnowledgeAdapter::new(cfg, bus).await.expect("new");
        let input = KnowledgeRecordInput {
            topic: "FAQ".to_string(),
            tags: vec![],
            content: "x".repeat(11),
            source: None,
        };
        let err = adapter
            .store_information(input, &make_ctx())
            .await
            .expect_err("must reject");
        assert!(matches!(err, KnowledgeErrorV1::InvalidInput { .. }));
    }

    #[tokio::test]
    async fn store_information_rejects_empty_topic() {
        // The InvalidInput path: empty
        // topic is rejected.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cfg = JsonKnowledgeConfig::new(dir.path().to_path_buf(), test_capabilities());
        let bus = Arc::new(EventBus::new());
        let adapter = JsonKnowledgeAdapter::new(cfg, bus).await.expect("new");
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
    async fn store_information_rejects_empty_content() {
        // The InvalidInput path: empty
        // content is rejected.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cfg = JsonKnowledgeConfig::new(dir.path().to_path_buf(), test_capabilities());
        let bus = Arc::new(EventBus::new());
        let adapter = JsonKnowledgeAdapter::new(cfg, bus).await.expect("new");
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
    async fn store_information_writes_file_and_publishes_event() {
        // The happy path: the file is on
        // disk, the index is updated, and
        // the audit event is published.
        let dir = tempfile::TempDir::new().expect("tempdir");
        let cfg = JsonKnowledgeConfig::new(dir.path().to_path_buf(), test_capabilities());
        let bus = Arc::new(EventBus::new());
        let adapter = JsonKnowledgeAdapter::new(cfg, bus).await.expect("new");
        let input = KnowledgeRecordInput {
            topic: "FAQ".to_string(),
            tags: vec!["billing".to_string()],
            content: "refund policy".to_string(),
            source: Some("chat-2026-07-11".to_string()),
        };
        let returned_id = adapter
            .store_information(input.clone(), &make_ctx())
            .await
            .expect("store_information");
        // The file is on disk under
        // `<root>/faq/<record_id>.md`.
        let file_path = dir.path().join("faq").join(format!("{returned_id}.md"));
        assert!(
            file_path.is_file(),
            "file must exist: {}",
            file_path.display()
        );
        let bytes = std::fs::read(&file_path).expect("read");
        assert_eq!(bytes, b"refund policy");
        // The index is updated.
        {
            let idx = adapter.index.read().await;
            assert_eq!(idx.topic_count(), 1);
            assert_eq!(idx.total_record_count(), 1);
            assert_eq!(idx.store_information_calls, 1);
            assert!(idx.known_tags().contains("billing"));
            assert!(idx.records_with_tag("billing").contains(&returned_id));
        }
    }
}
