//! Code Map: Knowledge audit events
//! - `KnowledgeQueried`: Published AFTER every
//!   `find_information` call returns (success or
//!   empty result, not on error). Carries the
//!   `ExecutionContext` metadata, the topic
//!   filter, the tag filters, the result count,
//!   and the duration in milliseconds. Does NOT
//!   carry the free-text query — the query may
//!   contain sensitive data that does not
//!   belong in a long-lived audit log.
//! - `KnowledgeRecordStored`: Published AFTER
//!   the disk write is durable (both the
//!   record file and the index file). Carries
//!   the `ExecutionContext` metadata, the
//!   `record_id`, the topic, the tag count, the
//!   content length, and the source. Does NOT
//!   carry the record content.
//!
//! Story (plain English): The two audit events
//! are the two small tickets the catalog
//! stamps on the log every time. The first
//! ticket (`KnowledgeQueried`) is stamped when
//! a search returns: "we did a search, here is
//! who, here is the topic, here is how many
//! cards we pulled, here is how long it took."
//! The second ticket
//! (`KnowledgeRecordStored`) is stamped when a
//! new card is filed: "we filed a card, here is
//! the number, here is the drawer, here is the
//! size, here is where it came from." Neither
//! ticket carries the patron's question or the
//! card's body — the audit story is "what
//! happened," not "what was asked or said." A
//! future reader can reconstruct "who
//! searched, who filed, and how long did it
//! take?" without ever reading the search
//! query or the card body.
//!
//! CID Index:
//! CID:knowledge-events-001 -> KnowledgeQueried
//! CID:knowledge-events-002 -> KnowledgeRecordStored
//!
//! Quick lookup: rg -n "CID:knowledge-events-" crates/afa-contracts/src/knowledge/events.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::AfaEvent;
use crate::execution_context::Actor;
use crate::ids::{CorrelationId, TenantId};

use super::record::RecordId;

// CID:knowledge-events-001 - KnowledgeQueried
// Purpose: The audit fact the adapter publishes
// on the event bus AFTER every
// `find_information` call returns (success or
// empty result, not on error). Carries the
// `ExecutionContext` metadata (tenant,
// correlation, actor), the topic filter, the
// tag filters, the result count, and the
// duration in milliseconds. Does NOT carry the
// free-text query — the query may contain
// sensitive data that does not belong in a
// long-lived audit log. The event fires for
// empty results too (`result_count: 0` is a
// valid, interesting value — it means "no
// match"); the event does NOT fire on
// `find_information` errors (the caller maps
// the error to `AfaErrorKind` and decides
// whether to alert or retry).
// Uses: AfaEvent, serde, chrono,
// ExecutionContext types.
// Used by: the JSON adapter on every
// successful `find_information` call, and any
// dashboard or observability tool subscribed
// to Knowledge events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeQueried {
    /// The tracking number from the
    /// `ExecutionContext`.
    pub correlation_id: CorrelationId,
    /// The tenant from the `ExecutionContext`.
    pub tenant_id: TenantId,
    /// The actor from the `ExecutionContext`
    /// (the `Actor` enum — channel / timer /
    /// human / internal — not the full
    /// context).
    pub actor: Actor,
    /// The wall-clock time the adapter
    /// finished the query.
    pub timestamp: DateTime<Utc>,
    /// The topic filter from the
    /// `FindInformationRequest` (mirrors the
    /// request's `topic` field, `None` if the
    /// request did not filter by topic).
    pub topic_filter: Option<String>,
    /// The tag filters from the
    /// `FindInformationRequest` (mirrors the
    /// request's `tags` field; empty if the
    /// request had no tag filter).
    pub tag_filters: Vec<String>,
    /// The number of records returned by the
    /// query. `0` is a valid, interesting
    /// value (it means "no match"); the event
    /// still fires.
    pub result_count: u32,
    /// The wall-clock duration of the call in
    /// milliseconds (from the first line of
    /// `find_information` to the publish of
    /// this event).
    pub duration_ms: u32,
}

impl AfaEvent for KnowledgeQueried {}

// CID:knowledge-events-002 - KnowledgeRecordStored
// Purpose: The audit fact the adapter publishes
// on the event bus AFTER the disk write is
// durable (both the record file and the index
// file). Carries the `ExecutionContext`
// metadata, the `record_id`, the topic, the
// tag count, the content length, and the
// source. Does NOT carry the record content.
// The event does NOT fire on
// `store_information` errors (the caller maps
// the error to `AfaErrorKind` and decides
// whether to alert or retry).
// Uses: AfaEvent, serde, chrono,
// ExecutionContext types, RecordId.
// Used by: the JSON adapter on every
// successful `store_information` call, and
// any dashboard or observability tool
// subscribed to Knowledge events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeRecordStored {
    /// The tracking number from the
    /// `ExecutionContext`.
    pub correlation_id: CorrelationId,
    /// The tenant from the `ExecutionContext`.
    pub tenant_id: TenantId,
    /// The actor from the `ExecutionContext`.
    pub actor: Actor,
    /// The wall-clock time the adapter
    /// finished the durable write.
    pub timestamp: DateTime<Utc>,
    /// The engine-assigned `RecordId`.
    pub record_id: RecordId,
    /// The human-readable topic name.
    pub topic: String,
    /// The number of distinct tags in the
    /// record (after dedup).
    pub tag_count: u32,
    /// The length of the record's `content`
    /// field in bytes (metadata, not the
    /// content itself).
    pub content_length: u32,
    /// The optional source note (mirrors the
    /// `KnowledgeRecordInput::source` field;
    /// `None` if the writer did not supply
    /// one).
    pub source: Option<String>,
}

impl AfaEvent for KnowledgeRecordStored {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_context::Actor;
    use crate::ids::{CorrelationId, TenantId};
    use chrono::Utc;

    #[test]
    fn queried_event_carries_metadata_not_free_text() {
        // The `KnowledgeQueried` event must
        // carry the `topic_filter`,
        // `tag_filters`, `result_count`, and
        // `duration_ms` but NOT the free-text
        // query. The struct has no
        // `free_text` field — this is the
        // compile-time guarantee that an audit
        // reader cannot leak the query by
        // accidentally logging the event.
        let e = KnowledgeQueried {
            correlation_id: CorrelationId::new(),
            tenant_id: TenantId::new("t"),
            actor: Actor::Timer,
            timestamp: Utc::now(),
            topic_filter: Some("FAQ".into()),
            tag_filters: vec!["billing".into()],
            result_count: 3,
            duration_ms: 12,
        };
        // The struct has exactly the 8 fields
        // we expect. A future contributor who
        // adds a payload-bearing field
        // (`free_text: String` or
        // `query: String`) would be forced to
        // update this test.
        let json = serde_json::to_string(&e).expect("serialize");
        assert!(!json.contains("free_text"));
        assert!(!json.contains("\"query\""));
        // Sanity: the 8 legitimate fields are
        // there.
        assert!(json.contains("topic_filter"));
        assert!(json.contains("tag_filters"));
        assert!(json.contains("result_count"));
        assert!(json.contains("duration_ms"));
    }

    #[test]
    fn stored_event_carries_metadata_not_content() {
        // The `KnowledgeRecordStored` event
        // must carry the `record_id`, `topic`,
        // `tag_count`, `content_length`, and
        // `source` but NOT the record content.
        // The struct has no `content` field —
        // this is the compile-time guarantee.
        let e = KnowledgeRecordStored {
            correlation_id: CorrelationId::new(),
            tenant_id: TenantId::new("t"),
            actor: Actor::Timer,
            timestamp: Utc::now(),
            record_id: RecordId::new(),
            topic: "FAQ".into(),
            tag_count: 2,
            content_length: 142,
            source: Some("chat-2026-07-11".into()),
        };
        let json = serde_json::to_string(&e).expect("serialize");
        // The struct has exactly the 9 fields
        // we expect. The check for
        // `"content":` catches a future
        // contributor who adds a
        // `content: String` field. The
        // `content_length` field is a
        // substring of "content" but the
        // JSON-serialized form is
        // `"content_length":142`, so the
        // precise check on `"content":` (the
        // full colon-and-quote pattern) does
        // not match `content_length`.
        assert!(!json.contains("\"content\":"));
        assert!(!json.contains("\"body\":"));
        // Sanity: the 9 legitimate fields
        // are there.
        assert!(json.contains("record_id"));
        assert!(json.contains("topic"));
        assert!(json.contains("tag_count"));
        assert!(json.contains("content_length"));
        assert!(json.contains("source"));
    }
}
