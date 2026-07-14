//! Code Map: InMemoryIndex
//! - `InMemoryIndex`: The in-RAM index the
//!   adapter holds. Six pieces of state:
//!   `topics: BTreeMap<Slug, TopicEntry>`,
//!   `tag_index: BTreeMap<Tag, HashSet<RecordId>>`,
//!   `slug_to_topic: BTreeMap<Slug, TopicName>`,
//!   `store_information_calls: u64` (the
//!   audit-trail counter for the
//!   `store_information` method),
//!   `find_information_calls: u64` (the
//!   audit-trail counter for the
//!   `find_information` method, added
//!   in Phase 2), `list_topics_calls: u64`
//!   (same shape, Phase 2).
//! - `TopicEntry`: One topic's worth of
//!   records. Fields: `records:
//!   HashMap<RecordId, RecordMeta>`
//!   (keyed by `RecordId`; the inner
//!   map is a `HashMap` because
//!   `RecordId` does not implement
//!   `Ord`, only `Eq + Hash`),
//!   `record_count: u64`.
//! - `RecordMeta`: One record's worth of
//!   searchable metadata. Fields:
//!   `record_id`, `topic`, `tags`,
//!   `size_bytes`, `created_at`
//!   (`DateTime<Utc>`), `preview`
//!   (first 256 chars of the content),
//!   `slug` (the on-disk directory
//!   name; added in Phase 2 so the
//!   search path can compute the
//!   file path without re-slugifying
//!   the topic).
//!
//! Story (plain English): The in-memory
//! index is the part of the adapter that
//! lets the "find" path skip parsing
//! every file on disk. The adapter always
//! writes the file (the on-disk record is
//! the source of truth) and then adds an
//! entry to the index. The index is
//! rebuilt on boot by scanning the
//! storage root.
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
// file). `slug` is the on-disk directory
// name (the adapter computes it at store
// time and stores it here so the search
// path can build the file path
// `<storage_root>/<slug>/<record_id>.md`
// without re-slugifying the topic name).
#[derive(Debug, Clone)]
pub struct RecordMeta {
    pub record_id: RecordId,
    pub topic: String,
    pub tags: BTreeSet<String>,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
    pub preview: String,
    /// The on-disk directory name (the
    /// slugified topic). Computed by the
    /// adapter at store time and stored
    /// here so the `find_information`
    /// path can build the file path
    /// without re-slugifying.
    pub slug: String,
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
#[derive(Debug)]
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
// the query has tag filters. The three
// `*_calls` fields are the audit-trail
// counters the `describe_capabilities` (and,
// in Phase 4, the health check) consults to
// report "I have answered N find / M
// store / K list calls so far". The
// `content_tokens` cache is the Phase 2
// per-record tokenized content (the search
// path reads the file from disk on the
// first query that needs a record's
// content and caches the token set here;
// subsequent queries reuse the cache).
#[derive(Debug, Default)]
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
    /// Audit-trail counter. Increments on
    /// every successful `find_information`
    /// call (added in Phase 2).
    pub find_information_calls: u64,
    /// Audit-trail counter. Increments on
    /// every successful `list_topics` call
    /// (added in Phase 2).
    pub list_topics_calls: u64,
    /// Phase 2 lazy content token cache. The
    /// search path populates this the
    /// first time a record's content
    /// tokens are needed; subsequent
    /// queries reuse the cached tokens.
    /// Keyed by `RecordId` (the
    /// `HashSet<RecordId>` is fine for
    /// the same reason as in `tag_index`).
    pub content_tokens: HashMap<RecordId, BTreeSet<String>>,
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
        let meta = RecordMeta {
            record_id,
            topic: input.topic.clone(),
            tags: input.tags.iter().cloned().collect(),
            size_bytes,
            created_at: Utc::now(),
            preview: input.content.chars().take(256).collect(),
            slug: slug.to_string(),
        };
        self.add_meta(meta);
        self.store_information_calls = self.store_information_calls.saturating_add(1);
    }

    /// Add an already-built `RecordMeta` to
    /// the index. Used by the boot path
    /// (index-file load + disk rebuild)
    /// where the metadata comes from disk
    /// rather than from a fresh
    /// `store_information` call. The
    /// `store_information_calls` counter is
    /// NOT bumped (this is a recovery, not a
    /// fresh write).
    pub fn add_meta(&mut self, meta: RecordMeta) {
        let slug = meta.slug.clone();
        let entry = self
            .topics
            .entry(slug.clone())
            .or_insert_with(|| TopicEntry {
                records: HashMap::new(),
                record_count: 0,
            });
        entry.records.insert(meta.record_id, meta.clone());
        entry.record_count = entry.records.len() as u64;
        self.slug_to_topic
            .entry(slug)
            .or_insert_with(|| meta.topic.clone());
        for tag in &meta.tags {
            self.tag_index
                .entry(tag.clone())
                .or_default()
                .insert(meta.record_id);
        }
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

    /// Returns the intersection of the
    /// `RecordId` sets for every tag in
    /// `tags` (AND-filter). An empty
    /// `tags` slice returns an empty set
    /// (the caller short-circuits to "no
    /// tag filter" before this method is
    /// called; this method always
    /// interprets its argument as a
    /// non-empty filter).
    pub fn records_with_all_tags(&self, tags: &[String]) -> HashSet<RecordId> {
        if tags.is_empty() {
            return HashSet::new();
        }
        // Seed with the first tag's
        // record set; intersect the
        // rest. The caller is expected
        // to have already deduped the
        // input; an unknown tag
        // collapses the intersection
        // to empty (the
        // `unwrap_or_default` path).
        let mut iter = tags.iter();
        let first = iter.next().expect("non-empty");
        let mut acc = self.records_with_tag(first);
        for tag in iter {
            acc = acc
                .intersection(&self.records_with_tag(tag))
                .copied()
                .collect();
            if acc.is_empty() {
                break;
            }
        }
        acc
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

    /// Returns the records in `slug`
    /// (the topic filter for
    /// `find_information`). An empty
    /// `Vec` is returned for an unknown
    /// topic.
    pub fn records_in_topic(&self, slug: &str) -> Vec<(RecordId, RecordMeta)> {
        self.topics
            .get(slug)
            .map(|e| e.records.iter().map(|(k, v)| (*k, clone_meta(v))).collect())
            .unwrap_or_default()
    }

    /// Returns every record in the index
    /// (the "no topic filter" path for
    /// `find_information`). The records
    /// are returned as `(RecordId,
    /// RecordMeta)` pairs in arbitrary
    /// order (the search path sorts by
    /// score on top of this).
    pub fn all_records(&self) -> Vec<(RecordId, RecordMeta)> {
        self.topics
            .values()
            .flat_map(|e| e.records.iter().map(|(k, v)| (*k, clone_meta(v))))
            .collect()
    }

    /// Returns the `Topic` summary for
    /// `slug` (the per-topic rollup for
    /// `list_topics`). Returns `None` if
    /// the topic does not exist in the
    /// index.
    pub fn topic_summary(&self, slug: &str) -> Option<crate::Topic> {
        let entry = self.topics.get(slug)?;
        let topic_name = self.slug_to_topic.get(slug)?.clone();
        if entry.records.is_empty() {
            return Some(crate::Topic {
                name: topic_name,
                record_count: 0,
                first_record_at: None,
                last_record_at: None,
                tag_count: 0,
            });
        }
        let mut first: Option<DateTime<Utc>> = None;
        let mut last: Option<DateTime<Utc>> = None;
        let mut tag_union: BTreeSet<String> = BTreeSet::new();
        for meta in entry.records.values() {
            first = Some(first.map_or(meta.created_at, |f| f.min(meta.created_at)));
            last = Some(last.map_or(meta.created_at, |l| l.max(meta.created_at)));
            for tag in &meta.tags {
                tag_union.insert(tag.clone());
            }
        }
        Some(crate::Topic {
            name: topic_name,
            record_count: entry.record_count as u32,
            first_record_at: first,
            last_record_at: last,
            tag_count: tag_union.len() as u32,
        })
    }

    /// Returns the list of all topics in
    /// the index as `Topic` summaries,
    /// sorted alphabetically by `name`.
    /// The "no topics" path returns an
    /// empty `Vec` (NOT an error).
    pub fn all_topic_summaries(&self) -> Vec<crate::Topic> {
        let mut summaries: Vec<crate::Topic> = self
            .topics
            .keys()
            .filter_map(|slug| self.topic_summary(slug))
            .collect();
        summaries.sort_by(|a, b| a.name.cmp(&b.name));
        summaries
    }
}

/// Helper: clone a `RecordMeta` (the
/// fields are owned strings + small
/// integers; the clone is cheap and the
/// alternative — a `Cow`/borrow dance —
/// would leak the borrow lifetime into
/// every search path caller).
fn clone_meta(m: &RecordMeta) -> RecordMeta {
    RecordMeta {
        record_id: m.record_id,
        topic: m.topic.clone(),
        tags: m.tags.clone(),
        size_bytes: m.size_bytes,
        created_at: m.created_at,
        preview: m.preview.clone(),
        slug: m.slug.clone(),
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

    #[test]
    fn records_with_all_tags_is_intersection() {
        let mut idx = InMemoryIndex::new();
        let a = make_input(&["billing", "refund"], "a");
        let b = make_input(&["billing"], "b");
        let c = make_input(&["refund"], "c");
        let id_a = RecordId::new();
        let id_b = RecordId::new();
        let id_c = RecordId::new();
        idx.add_record(&a, "faq", id_a, 1);
        idx.add_record(&b, "faq", id_b, 1);
        idx.add_record(&c, "faq", id_c, 1);
        let both = idx.records_with_all_tags(&["billing".into(), "refund".into()]);
        assert_eq!(both.len(), 1);
        assert!(both.contains(&id_a));
        let only_billing = idx.records_with_all_tags(&["billing".into()]);
        assert_eq!(only_billing.len(), 2);
        assert!(only_billing.contains(&id_a));
        assert!(only_billing.contains(&id_b));
    }

    #[test]
    fn records_with_all_tags_empty_input_returns_empty() {
        // The caller is expected to
        // short-circuit before this
        // method, but the method's
        // contract must still be
        // total: empty input returns
        // an empty set (not all
        // records, not an error).
        let idx = InMemoryIndex::new();
        let set = idx.records_with_all_tags(&[]);
        assert!(set.is_empty());
    }

    #[test]
    fn records_in_topic_returns_records_for_known_slug() {
        let mut idx = InMemoryIndex::new();
        let input = make_input(&["billing"], "x");
        let rid = RecordId::new();
        idx.add_record(&input, "faq", rid, 1);
        let records = idx.records_in_topic("faq");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].0, rid);
        assert_eq!(records[0].1.topic, "FAQ");
        assert_eq!(records[0].1.slug, "faq");
    }

    #[test]
    fn records_in_topic_unknown_slug_returns_empty() {
        let idx = InMemoryIndex::new();
        assert!(idx.records_in_topic("nope").is_empty());
    }

    #[test]
    fn all_records_iterates_every_record() {
        let mut idx = InMemoryIndex::new();
        idx.add_record(&make_input(&[], "a"), "faq", RecordId::new(), 1);
        idx.add_record(
            &KnowledgeRecordInput {
                topic: "Properties".into(),
                tags: vec![],
                content: "b".into(),
                source: None,
            },
            "properties",
            RecordId::new(),
            1,
        );
        let all = idx.all_records();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn topic_summary_empty_topic_has_no_timestamps() {
        // A topic with no records has
        // `None` for both timestamps
        // (the empty fallback path).
        // We construct the index in a
        // way that puts a slug in the
        // map with no records by
        // round-tripping through
        // `topic_summary` after a
        // store + remove.
        let mut idx = InMemoryIndex::new();
        idx.add_record(&make_input(&[], "x"), "faq", RecordId::new(), 1);
        // Force-remove the record by
        // rebuilding the index
        // directly (the adapter
        // never does this; the test
        // exercises the
        // "empty topic" branch).
        idx.topics.get_mut("faq").unwrap().records.clear();
        idx.topics.get_mut("faq").unwrap().record_count = 0;
        let summary = idx.topic_summary("faq").expect("topic exists");
        assert_eq!(summary.name, "FAQ");
        assert_eq!(summary.record_count, 0);
        assert!(summary.first_record_at.is_none());
        assert!(summary.last_record_at.is_none());
        assert_eq!(summary.tag_count, 0);
    }

    #[test]
    fn topic_summary_rolls_up_counts_and_timestamps() {
        let mut idx = InMemoryIndex::new();
        idx.add_record(&make_input(&["billing"], "a"), "faq", RecordId::new(), 1);
        idx.add_record(&make_input(&["refund"], "b"), "faq", RecordId::new(), 1);
        let summary = idx.topic_summary("faq").expect("topic exists");
        assert_eq!(summary.name, "FAQ");
        assert_eq!(summary.record_count, 2);
        assert_eq!(summary.tag_count, 2);
        assert!(summary.first_record_at.is_some());
        assert!(summary.last_record_at.is_some());
    }

    #[test]
    fn all_topic_summaries_is_sorted_by_name() {
        let mut idx = InMemoryIndex::new();
        idx.add_record(&make_input(&[], "x"), "faq", RecordId::new(), 1);
        idx.add_record(
            &KnowledgeRecordInput {
                topic: "Zebra".into(),
                tags: vec![],
                content: "y".into(),
                source: None,
            },
            "zebra",
            RecordId::new(),
            1,
        );
        let summaries = idx.all_topic_summaries();
        assert_eq!(summaries.len(), 2);
        assert_eq!(summaries[0].name, "FAQ");
        assert_eq!(summaries[1].name, "Zebra");
    }
}
