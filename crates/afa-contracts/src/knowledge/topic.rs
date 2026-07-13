//! Code Map: Topic shape
//! - `Topic`: The summary of one topic — its name,
//!   the record count, the first and last record
//!   timestamps, and the number of distinct tags
//!   used. Returned by `KnowledgeV1::list_topics`,
//!   sorted alphabetically by name.
//!
//! Story (plain English): The topic is the card-
//! catalog drawer label. The summary card for the
//! "FAQ" drawer says "FAQ: 47 cards, oldest
//! 2026-07-11, newest 2026-07-13, 12 distinct
//! tags." A dashboard reads the drawer labels to
//! show the operator "you have 47 FAQ cards and
//! 312 property listings and 0 legal-questions"
//! at a glance.
//!
//! CID Index:
//! CID:knowledge-topic-001 -> Topic
//!
//! Quick lookup: rg -n "CID:knowledge-topic-" crates/afa-contracts/src/knowledge/topic.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// CID:knowledge-topic-001 - Topic
// Purpose: The summary of one topic — its name, the
// record count, the first and last record
// timestamps (`None` for an empty topic), and the
// distinct-tag count. Returned by
// `KnowledgeV1::list_topics`, sorted alphabetically
// by `name`. The `name` is the human-readable
// topic name (e.g. "FAQ" or "Property listings"),
// NOT the slugified on-disk form. v1 does not
// carry `description` or `metadata` — the topic
// is just a name and four aggregates.
// Uses: chrono::DateTime<Utc>.
// Used by: `KnowledgeV1::list_topics`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Topic {
    /// The human-readable topic name (NOT the
    /// slugified on-disk form). Two topics
    /// with the same `name` are the same topic.
    pub name: String,
    /// How many records are in this topic.
    pub record_count: u32,
    /// The timestamp of the oldest record in
    /// this topic. `None` if the topic is
    /// empty (a freshly created topic with no
    /// records yet).
    pub first_record_at: Option<DateTime<Utc>>,
    /// The timestamp of the newest record in
    /// this topic. `None` if the topic is
    /// empty.
    pub last_record_at: Option<DateTime<Utc>>,
    /// The number of distinct tags used across
    /// the records in this topic (after
    /// dedup).
    pub tag_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn topic_carries_all_five_fields() {
        // The five fields are the locked
        // shape from the TRD §2.2.4. A
        // dashboard that wants to render any
        // of the five must find them on this
        // struct.
        let t = Topic {
            name: "FAQ".into(),
            record_count: 47,
            first_record_at: Some(Utc.with_ymd_and_hms(2026, 7, 11, 12, 0, 0).unwrap()),
            last_record_at: Some(Utc.with_ymd_and_hms(2026, 7, 13, 14, 30, 0).unwrap()),
            tag_count: 12,
        };
        assert_eq!(t.name, "FAQ");
        assert_eq!(t.record_count, 47);
        assert!(t.first_record_at.is_some());
        assert!(t.last_record_at.is_some());
        assert_eq!(t.tag_count, 12);
    }

    #[test]
    fn empty_topic_has_no_first_or_last_timestamp() {
        // A freshly created topic with no
        // records has `None` for both
        // timestamps. A future contributor
        // who "fixes" this to `Some(epoch)`
        // would be lying about a real
        // timestamp.
        let t = Topic {
            name: "empty".into(),
            record_count: 0,
            first_record_at: None,
            last_record_at: None,
            tag_count: 0,
        };
        assert!(t.first_record_at.is_none());
        assert!(t.last_record_at.is_none());
    }
}
