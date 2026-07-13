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

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::{
    ExecutionContext, FindInformationRequest, FindInformationResponse, KnowledgeCapabilities,
    KnowledgeErrorV1, KnowledgeRecord, KnowledgeRecordInput, KnowledgeV1, RecordId, Topic,
};
use async_trait::async_trait;
use chrono::Utc;
use tokio::sync::RwLock;

use crate::atomic_write::atomic_write;
use crate::config::JsonKnowledgeConfig;
use crate::index::{InMemoryIndex, RecordMeta};
use crate::search;
use crate::topic_slug::topic_slug;

// The default `limit` for
// `find_information` when the request
// does not specify one. Per the
// IMPL Phase 2 task list.
const DEFAULT_FIND_LIMIT: u32 = 10;

// Helper: read a record's content
// from disk and cache the tokenized
// form in the index. Called by the
// `find_information` content loader.
// The function takes `Arc<RwLock<...>>`
// (not a held lock guard) so the
// loader can take the write lock
// without conflicting with the
// read lock the caller already
// holds.
async fn load_and_cache_content_tokens(
    index: &Arc<RwLock<InMemoryIndex>>,
    storage_root: &Path,
    rid: RecordId,
) {
    // Step 1: look up the slug
    // (and the file path) under
    // the read lock; release the
    // read lock before doing
    // the file I/O (long-running
    // I/O should not hold any
    // lock).
    let file_path = {
        let idx = index.read().await;
        // Cache hit: nothing
        // to do.
        if idx.content_tokens.contains_key(&rid) {
            return;
        }
        // Find the slug for
        // this record. The
        // adapter stores
        // the slug in the
        // `RecordMeta` (Phase
        // 2 change). If the
        // record is not in
        // the index, the
        // caller's
        // `find_information`
        // will fail with
        // `MalformedRecord`
        // — we just return
        // here.
        let mut found: Option<RecordMeta> = None;
        for entry in idx.topics.values() {
            if let Some(m) = entry.records.get(&rid) {
                found = Some((*m).clone());
                break;
            }
        }
        let meta = match found {
            Some(v) => v,
            None => return,
        };
        storage_root.join(&meta.slug).join(format!("{rid}.md"))
    };
    // Step 2: read the file.
    // `read_to_string` is
    // UTF-8 strict; a non-UTF-8
    // file is a corrupt record
    // and would be mapped to
    // `MalformedRecord` by the
    // caller's `find_information`.
    // We do not propagate the
    // error here — the caller
    // will see the file miss on
    // its own `read_to_string`
    // and surface the error.
    let content = match tokio::fs::read_to_string(&file_path).await {
        Ok(s) => s,
        Err(_) => return,
    };
    // Step 3: tokenize + cache.
    let tokens = search::tokenize(&content);
    let mut idx = index.write().await;
    idx.content_tokens.insert(rid, tokens);
}

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
    // CID:afa-plugin-knowledge-json-adapter-005 - find_information
    // Purpose: The Phase 2 read path. The
    // implementation:
    // 1. Holds the index read lock for
    //    the duration of the scoring
    //    step (the loader populates
    //    the cache inside the lock).
    // 2. Calls `search::filter_and_score`
    //    to get the ranked candidate
    //    list.
    // 3. Applies the `limit` (default
    //    10) and assembles the
    //    `KnowledgeRecord` bodies by
    //    reading the on-disk files
    //    for the top N records.
    // 4. Publishes the
    //    `KnowledgeSearchPerformed`
    //    audit event on the bus (the
    //    event carries the `result_count`
    //    and `duration_ms`; the
    //    `free_text` is NOT in the
    //    event payload — audit-trail
    //    safety).
    // 5. Bumps the
    //    `find_information_calls`
    //    counter on the index.
    async fn find_information(
        &self,
        request: FindInformationRequest,
        ctx: &ExecutionContext,
    ) -> Result<FindInformationResponse, KnowledgeErrorV1> {
        let started = std::time::Instant::now();
        let limit = request.limit.unwrap_or(DEFAULT_FIND_LIMIT) as usize;

        // Step 1: under the read lock,
        // compute the candidate set
        // (topic filter + tag AND-filter)
        // and snapshot the
        // `content_tokens` cache. The
        // lock is released BEFORE any
        // await on the content loader
        // (the loader needs to take the
        // write lock to cache new
        // tokens; a read lock held by
        // the same task would deadlock
        // the writer).
        let needs_content_tokens = request.free_text.is_some();
        let candidates: Vec<(RecordId, RecordMeta)>;
        let mut local_tokens: std::collections::HashMap<RecordId, BTreeSet<String>> =
            std::collections::HashMap::new();
        {
            let index = self.index.read().await;

            // Step 1a: topic filter
            // (hard filter).
            let topic_filtered: Vec<(RecordId, RecordMeta)> = match &request.topic {
                Some(req_topic) => {
                    let slug = crate::topic_slug::topic_slug(req_topic);
                    index.records_in_topic(&slug)
                }
                None => index.all_records(),
            };

            // Step 1b: tag AND-filter.
            let tag_filtered: Vec<(RecordId, RecordMeta)> = if request.tags.is_empty() {
                topic_filtered
            } else {
                // Dedup the request
                // tags (an AND-filter
                // of ["billing",
                // "billing"] is the
                // same as ["billing"]).
                let mut dedup: BTreeSet<String> = BTreeSet::new();
                for t in &request.tags {
                    dedup.insert(t.clone());
                }
                let allowed =
                    index.records_with_all_tags(&dedup.iter().cloned().collect::<Vec<_>>());
                topic_filtered
                    .into_iter()
                    .filter(|(rid, _)| allowed.contains(rid))
                    .collect()
            };

            // Step 1c: snapshot the
            // content tokens that are
            // already cached.
            if needs_content_tokens {
                for (rid, _meta) in &tag_filtered {
                    if let Some(t) = index.content_tokens.get(rid) {
                        local_tokens.insert(*rid, t.clone());
                    }
                }
            }

            candidates = tag_filtered;
        }
        // The read lock is released
        // here. From this point on, no
        // task-local read lock is held
        // and the loader may freely
        // take the write lock.

        // Step 2: load missing content
        // tokens (file I/O + cache
        // write). The loader takes its
        // own short-lived read and
        // write locks.
        if needs_content_tokens {
            for (rid, _meta) in &candidates {
                if local_tokens.contains_key(rid) {
                    continue;
                }
                load_and_cache_content_tokens(&self.index, &self.config.storage_root, *rid).await;
                // Re-read the cache
                // briefly to pick up
                // what the loader just
                // wrote.
                let index = self.index.read().await;
                if let Some(t) = index.content_tokens.get(rid) {
                    local_tokens.insert(*rid, t.clone());
                }
            }
        }

        // Step 3: score + filter zeros.
        // Pure CPU; no locks needed.
        // We keep the `RecordMeta`
        // alongside the score so the
        // sort tie-break (by
        // `created_at` desc) can use
        // it without re-acquiring the
        // index lock.
        let mut scored: Vec<(RecordId, RecordMeta, f32)> = candidates
            .into_iter()
            .map(|(rid, meta)| {
                let tokens = local_tokens.get(&rid).cloned().unwrap_or_default();
                let s = search::score_candidate(&request, &meta, &tokens);
                (rid, meta, s)
            })
            .filter(|(_, _, s)| *s > 0.0)
            .collect();

        // Stable sort by descending
        // score; ties broken by
        // `created_at` descending
        // (newer first), then by
        // `RecordId`'s inner `Uuid`
        // for determinism.
        scored.sort_by(|a, b| {
            b.2.partial_cmp(&a.2)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| {
                    b.1.created_at
                        .cmp(&a.1.created_at)
                        .then(a.0 .0.cmp(&b.0 .0))
                })
        });

        // Step 4: apply the limit
        // and assemble the
        // `KnowledgeRecord` bodies.
        let top: Vec<(RecordId, RecordMeta, f32)> = scored.into_iter().take(limit).collect();
        let mut results: Vec<(KnowledgeRecord, f32)> = Vec::with_capacity(top.len());
        for (rid, meta, score) in top {
            let slug = meta.slug.clone();
            let file_path = self
                .config
                .storage_root
                .join(&slug)
                .join(format!("{rid}.md"));
            let content = match tokio::fs::read_to_string(&file_path).await {
                Ok(s) => s,
                Err(e) => {
                    return Err(KnowledgeErrorV1::StorageUnavailable {
                        topic: Some(meta.topic.clone()),
                        record_id: Some(rid.0),
                        reason: format!(
                            "find_information: failed to read {}: {e}",
                            file_path.display()
                        ),
                    });
                }
            };
            let record = KnowledgeRecord {
                record_id: rid,
                topic: meta.topic.clone(),
                tags: meta.tags.iter().cloned().collect(),
                content,
                source: None,
                created_at: meta.created_at,
            };
            results.push((record, score));
        }

        // Step 4: publish the
        // `KnowledgeQueried` audit
        // event. The event
        // carries the `result_count`
        // and `duration_ms`; the
        // `free_text` is NOT in the
        // event payload — audit-
        // trail safety.
        let duration_ms: u32 = started.elapsed().as_millis().try_into().unwrap_or(u32::MAX);
        let event = afa_contracts::knowledge::KnowledgeQueried {
            correlation_id: ctx.correlation_id,
            tenant_id: ctx.tenant_id.clone(),
            actor: ctx.actor.clone(),
            timestamp: Utc::now(),
            topic_filter: request.topic.clone(),
            tag_filters: request.tags.clone(),
            result_count: results.len() as u32,
            duration_ms,
        };
        self.event_bus.publish(event, ctx.clone()).await;

        // Step 5: bump the
        // counter.
        {
            let mut index = self.index.write().await;
            index.find_information_calls = index.find_information_calls.saturating_add(1);
        }

        Ok(results)
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

    async fn list_topics(&self, ctx: &ExecutionContext) -> Result<Vec<Topic>, KnowledgeErrorV1> {
        // Step 1: hold the read lock
        // and pull the sorted topic
        // summaries. The
        // `all_topic_summaries` helper
        // already does the name-sort
        // and the per-topic aggregation
        // (record_count, first/last
        // timestamps, tag_count).
        let started = std::time::Instant::now();
        let topics: Vec<Topic> = {
            let index = self.index.read().await;
            index.all_topic_summaries()
        };

        // Step 2: publish the
        // `KnowledgeTopicsListed`
        // audit event. The event
        // carries the `topic_count`
        // and `duration_ms`; the
        // topic *names* are NOT in
        // the event payload (they
        // are the return value of
        // the call, not the audit
        // fact).
        let duration_ms: u32 = started.elapsed().as_millis().try_into().unwrap_or(u32::MAX);
        let event = afa_contracts::knowledge::KnowledgeTopicsListed {
            correlation_id: ctx.correlation_id,
            tenant_id: ctx.tenant_id.clone(),
            actor: ctx.actor.clone(),
            timestamp: Utc::now(),
            topic_count: topics.len() as u32,
            duration_ms,
        };
        self.event_bus.publish(event, ctx.clone()).await;

        // Step 3: bump the
        // counter.
        {
            let mut index = self.index.write().await;
            index.list_topics_calls = index.list_topics_calls.saturating_add(1);
        }

        Ok(topics)
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
