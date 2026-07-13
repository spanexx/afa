//! Code Map: InMemoryIndex
//! - `InMemoryIndex`: The in-RAM index the
//!   adapter holds. Four pieces of state:
//!   `topics: BTreeMap<Slug, TopicEntry>`,
//!   `tag_index: BTreeMap<Tag, HashSet<RecordId>>`,
//!   `slug_to_topic: BTreeMap<Slug, TopicName>`,
//!   `store_information_calls: u64` (the
//!   audit-trail counter for the
//!   `store_information` method).
//! - `TopicEntry`: One topic's worth of
//!   records. Fields: `records:
//!   HashMap<RecordId, RecordMeta>`
//!   (keyed by `RecordId`; the inner
//!   map is a `HashMap` because
//!   `RecordId` does not implement
//!   `Ord`, only `Eq + Hash`),
//!   `record_count: u64`.
//! - `RecordMeta`: One record's worth of
//!   searchable metadata. Fields: `record_id`,
//!   `topic`, `tags`, `size_bytes`, `created_at`
//!   (`DateTime<Utc>`), `preview` (first 256
//!   chars of the content).
//!
//! Story (plain English): The in-memory index
//! is the part of the adapter that lets the
//! "find" path skip parsing every file on
//! disk. The adapter always writes the file
//! (the on-disk record is the source of
//! truth) and then adds an entry to the
//! index. The index is rebuilt on boot by
//! scanning the storage root.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-index-001 -> InMemoryIndex
//! CID:afa-plugin-knowledge-json-index-002 -> TopicEntry
//! CID:afa-plugin-knowledge-json-index-003 -> RecordMeta
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-index-" crates/afa-plugin-knowledge-json/src/index.rs

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use afa_contracts::{KnowledgeRecordInput, RecordId};
use chrono::{DateTime, Utc};

// CID:afa-plugin-knowledge-json-index-003 - RecordMeta
// Purpose: One record's worth of searchable
// metadata. The body of the record is on
// disk; the index holds only the fields the
// `find_information` path needs to score +
// filter without re-parsing every file.
// `preview` is the first 256 chars of the
// content (the LLM gets a free preview
// without the adapter having to re-read the
// file).
pub struct RecordMeta {
    pub record_id: RecordId,
    pub topic: String,
    pub tags: BTreeSet<String>,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
    pub preview: String,
}

// CID:afa-plugin-knowledge-json-index-002 - TopicEntry
// Purpose: One topic's worth of records. The
// `records` map is `HashMap<RecordId,
// RecordMeta>` because `RecordId` implements
// `Eq + Hash` but not `Ord` (the contract
// shape is the minimum needed for the
// `HashSet<RecordId>` reverse index in
// `tag_index`; the engine does not need a
// total order on `RecordId`s). A future
// contributor who needs sorted iteration
// can sort the values on the spot (the
// `find_information` Phase 2 path uses
// `BTreeMap`-equivalent ordering via the
// `created_at` field). `record_count` is
// kept in sync with `records.len()` (a small
// redundancy that saves a `.len()` on the
// hot path).
pub struct TopicEntry {
    pub records: HashMap<RecordId, RecordMeta>,
    pub record_count: u64,
}

// CID:afa-plugin-knowledge-json-index-001 - InMemoryIndex
// Purpose: The in-RAM index the adapter holds.
// The `topics` map is keyed by topic slug
// (the on-disk directory name, e.g.
// "faq"); the `slug_to_topic` map turns the
// slug back into the human-readable topic
// name (e.g. "FAQ") the search path reports
// in result records. The `tag_index` is a
// simple `Tag -> HashSet<RecordId>` reverse
// index that the search path consults when
// the query has tag filters.
#[derive(Default)]
pub struct InMemoryIndex {
    pub topics: BTreeMap<String, TopicEntry>,
    pub slug_to_topic: BTreeMap<String, String>,
    pub tag_index: BTreeMap<String, HashSet<RecordId>>,
    /// Audit-trail counter. Increments on
    /// every successful `store_information`
    /// call. The Phase 4 health check uses
    /// this; the Phase 2 search path does
    /// not.
    pub store_information_calls: u64,
}

impl InMemoryIndex {
    /// Build a fresh, empty index. The boot
    /// path calls this when no `.index.json`
    /// exists (first boot) and then scans
    /// the storage root to populate it.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a new record to the index. The
    /// adapter calls this AFTER the on-disk
    /// write succeeded; if the on-disk write
    /// failed, the index is NOT updated
    /// (the source of truth is on disk; the
    /// index is a derived cache).
    /// **Returns**: `()` on success. The
    /// function cannot fail by design — the
    /// `store_information` upstream step is
    /// the one that maps errors to
    /// `KnowledgeErrorV1`.
    pub fn add_record(
        &mut self,
        input: &KnowledgeRecordInput,
        slug: &str,
        record_id: RecordId,
        size_bytes: u64,
    ) {
        let entry = self
            .topics
            .entry(slug.to_string())
            .or_insert_with(|| TopicEntry {
                records: HashMap::new(),
                record_count: 0,
            });
        entry.records.insert(
            record_id,
            RecordMeta {
                record_id,
                topic: input.topic.clone(),
                tags: input.tags.iter().cloned().collect(),
                size_bytes,
                created_at: Utc::now(),
                preview: input.content.chars().take(256).collect(),
            },
        );
        entry.record_count = entry.records.len() as u64;
        self.slug_to_topic
            .entry(slug.to_string())
            .or_insert_with(|| input.topic.clone());
        for tag in &input.tags {
            self.tag_index
                .entry(tag.clone())
                .or_default()
                .insert(record_id);
        }
        self.store_information_calls = self.store_information_calls.saturating_add(1);
    }

    /// Returns the number of topics in the
    /// index. The Phase 4 health check uses
    /// this to report "I see N topics on
    /// disk".
    pub fn topic_count(&self) -> u64 {
        self.topics.len() as u64
    }

    /// Returns the total number of records
    /// across all topics. The Phase 4 health
    /// check uses this to report "I see M
    /// records on disk".
    pub fn total_record_count(&self) -> u64 {
        self.topics.values().map(|e| e.record_count).sum()
    }

    /// Returns the tags known to the index.
    /// The Phase 2 search path uses this to
    /// validate a query's tag filters (an
    /// unknown tag → empty result set).
    pub fn known_tags(&self) -> HashSet<String> {
        self.tag_index.keys().cloned().collect()
    }

    /// Returns the `RecordId`s tagged with
    /// `tag`. The Phase 2 search path uses
    /// this as the starting point of a
    /// tag-filtered query (the rest of the
    /// filter is applied in-memory).
    pub fn records_with_tag(&self, tag: &str) -> HashSet<RecordId> {
        self.tag_index.get(tag).cloned().unwrap_or_default()
    }

    /// Returns the topic name for `slug`
    /// (the human-readable name the
    /// `find_information` results report
    /// under the `topic` field), or `None`
    /// if no such topic exists in the
    /// index. The Phase 2 search path uses
    /// this to translate a result's on-disk
    /// slug back into the name the caller
    /// wrote.
    pub fn topic_for_slug(&self, slug: &str) -> Option<String> {
        self.slug_to_topic.get(slug).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use afa_contracts::KnowledgeRecordInput;

    fn make_input(tags: &[&str], body: &str) -> KnowledgeRecordInput {
        KnowledgeRecordInput {
            topic: "FAQ".to_string(),
            tags: tags.iter().map(|s| s.to_string()).collect(),
            content: body.to_string(),
            source: None,
        }
    }

    #[test]
    fn empty_index_has_zero_topics_and_records() {
        let idx = InMemoryIndex::new();
        assert_eq!(idx.topic_count(), 0);
        assert_eq!(idx.total_record_count(), 0);
        assert!(idx.known_tags().is_empty());
    }

    #[test]
    fn add_record_increments_topic_and_record_counts() {
        let mut idx = InMemoryIndex::new();
        let input = make_input(&["billing"], "refund policy");
        let rid = RecordId::new();
        idx.add_record(&input, "faq", rid, 100);
        assert_eq!(idx.topic_count(), 1);
        assert_eq!(idx.total_record_count(), 1);
        assert_eq!(idx.store_information_calls, 1);
    }

    #[test]
    fn add_record_populates_tag_index_and_records_with_tag() {
        let mut idx = InMemoryIndex::new();
        let a = make_input(&["billing", "refund"], "a");
        let b = make_input(&["billing"], "b");
        let id_a = RecordId::new();
        let id_b = RecordId::new();
        idx.add_record(&a, "faq", id_a, 1);
        idx.add_record(&b, "faq", id_b, 1);
        let tags = idx.known_tags();
        assert!(tags.contains("billing"));
        assert!(tags.contains("refund"));
        let with_billing = idx.records_with_tag("billing");
        assert!(with_billing.contains(&id_a));
        assert!(with_billing.contains(&id_b));
        let with_refund = idx.records_with_tag("refund");
        assert!(with_refund.contains(&id_a));
        assert!(!with_refund.contains(&id_b));
    }

    #[test]
    fn topic_for_slug_returns_human_readable_name() {
        let mut idx = InMemoryIndex::new();
        let input = make_input(&[], "x");
        idx.add_record(&input, "faq", RecordId::new(), 1);
        assert_eq!(idx.topic_for_slug("faq"), Some("FAQ".to_string()));
        assert_eq!(idx.topic_for_slug("nope"), None);
    }

    #[test]
    fn records_with_tag_unknown_returns_empty() {
        let idx = InMemoryIndex::new();
        let set = idx.records_with_tag("nope");
        assert!(set.is_empty());
    }
}
