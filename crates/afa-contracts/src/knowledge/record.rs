//! Code Map: Knowledge record shape
//! - `KnowledgeRecord`: The full record as returned by
//!   `find_information`. Carries the engine-generated
//!   `record_id` and `created_at` plus all caller-
//!   supplied fields.
//! - `KnowledgeRecordInput`: The "bare" record a writer
//!   hands to `store_information`. Does NOT carry
//!   `record_id` or `created_at` — the engine fills
//!   those at store time.
//! - `RecordId`: A `Uuid` newtype for the engine-
//!   assigned identifier. `Display` renders the
//!   hyphenated lowercase form used for the on-disk
//!   filename.
//!
//! Story (plain English): A record is the card in the
//! card catalog. The card has a topic ("FAQ"), a few
//! tags ("billing", "refund"), a body (the content), a
//! note about where the fact came from (the source),
//! and a stamp (the creation time). The engine stamps
//! every card with a unique number (the `RecordId`)
//! the moment the card is filed, so a future reader
//! can refer to "the card numbered 550e8400...".
//!
//! CID Index:
//! CID:knowledge-record-001 -> KnowledgeRecord
//! CID:knowledge-record-002 -> KnowledgeRecordInput
//! CID:knowledge-record-003 -> RecordId
//!
//! Quick lookup: rg -n "CID:knowledge-record-" crates/afa-contracts/src/knowledge/record.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// CID:knowledge-record-003 - RecordId
// Purpose: The engine-assigned `Uuid` newtype for a
// record. `Display` renders the hyphenated lowercase
// form (`550e8400-e29b-41d4-a716-446655440000`) so
// the on-disk filename `<record_id>.md` is the
// `Display` output with a `.md` extension.
// Uses: uuid::Uuid.
// Used by: `KnowledgeRecord::record_id`,
// `KnowledgeRecordStored::record_id`, the
// `find_information` response (each
// `(KnowledgeRecord, f32)` tuple carries a
// `RecordId` inside the `KnowledgeRecord`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RecordId(pub Uuid);

impl RecordId {
    /// A new v4 `RecordId` from OS entropy. The
    /// engine calls this at store time.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for RecordId {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Display for RecordId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The hyphenated lowercase form is the
        // canonical `Uuid` `Display` output (the
        // `550e8400-e29b-41d4-a716-446655440000`
        // shape) and the shape the on-disk filename
        // needs.
        write!(f, "{}", self.0)
    }
}

// CID:knowledge-record-001 - KnowledgeRecord
// Purpose: The full record shape. Carries the
// engine-generated `record_id` and `created_at`
// (set by the engine at store time) plus all
// caller-supplied fields (`topic`, `tags`,
// `content`, `source`). The `content` is opaque to
// the engine — it is stored, retrieved, and
// tokenized for free-text search, but the engine
// never interprets what the content means.
// Uses: RecordId, chrono::DateTime<Utc>.
// Used by: `FindInformationResponse` (each result
// is a `(KnowledgeRecord, f32)` tuple).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeRecord {
    /// The engine-assigned unique id (set at
    /// store time; the writer never supplies it).
    pub record_id: RecordId,
    /// The human-readable topic name (e.g. "FAQ",
    /// "Property listings"). The on-disk directory
    /// is the slugified form (`topic_slug(topic)`).
    pub topic: String,
    /// Cross-cutting labels. AND-filterable in
    /// `find_information` (every tag in the
    /// request's `tags` must be present in the
    /// record's `tags`). v1 does not normalize
    /// tag spelling — the caller is responsible
    /// for consistent tag spelling.
    pub tags: Vec<String>,
    /// The opaque record body. Stored verbatim in
    /// the per-record `.md` file. The engine never
    /// interprets this; it only tokenizes it for
    /// free-text search.
    pub content: String,
    /// Optional human-readable note about where
    /// this fact came from (e.g.
    /// "chat-2026-07-11"). `None` = no source note.
    pub source: Option<String>,
    /// The wall-clock time the engine filed the
    /// record (set at store time from the calling
    /// `ExecutionContext`'s clock or the local
    /// clock).
    pub created_at: DateTime<Utc>,
}

// CID:knowledge-record-002 - KnowledgeRecordInput
// Purpose: The "bare" record a writer hands to
// `store_information`. Does NOT carry `record_id`
// (the engine generates it) or `created_at` (the
// engine sets it). The writer supplies everything
// else. The two-type design
// (`KnowledgeRecordInput` for input,
// `KnowledgeRecord` for output) is the locked
// shape from the TRD §2.2.2: it makes the engine's
// responsibility for the two engine-generated
// fields explicit at the type level.
// Uses: nothing.
// Used by: `KnowledgeV1::store_information` (the
// input type).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeRecordInput {
    /// The human-readable topic name. Must be
    /// non-empty (the adapter validates this in
    /// Phase 1 and returns
    /// `KnowledgeErrorV1::InvalidInput` if it is
    /// empty).
    pub topic: String,
    /// Cross-cutting labels. May be empty (a
    /// record with no tags is valid). v1 does
    /// not normalize tag spelling.
    pub tags: Vec<String>,
    /// The opaque record body. Must be non-empty
    /// and at most
    /// `KnowledgeCapabilities::max_record_size_bytes`
    /// bytes (the adapter validates this in
    /// Phase 1).
    pub content: String,
    /// Optional human-readable note about where
    /// this fact came from. `None` = no source
    /// note.
    pub source: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_id_new_is_unique_and_non_nil() {
        // Two `RecordId::new()` calls must produce
        // different ids (a v4 UUID's collision
        // probability is ~2^-122). Neither should
        // be the nil UUID (the engine should never
        // hand out a "placeholder" id).
        let a = RecordId::new();
        let b = RecordId::new();
        assert_ne!(a, b);
        assert_ne!(a.0, Uuid::nil());
        assert_ne!(b.0, Uuid::nil());
    }

    #[test]
    fn record_id_display_renders_hyphenated_lowercase() {
        // The on-disk filename is
        // `<record_id>.md`, so the `Display` form
        // must be the hyphenated lowercase shape
        // (e.g. `550e8400-e29b-41d4-a716-446655440000`).
        let id = RecordId(Uuid::nil());
        // The nil UUID's `Display` form is
        // `00000000-0000-0000-0000-000000000000`.
        assert_eq!(format!("{id}"), "00000000-0000-0000-0000-000000000000");
    }

    #[test]
    fn knowledge_record_carries_every_field() {
        // The record is the full shape returned
        // by `find_information`. A future
        // contributor who drops a field would
        // be forced to update this test.
        let r = KnowledgeRecord {
            record_id: RecordId::new(),
            topic: "FAQ".into(),
            tags: vec!["billing".into(), "refund".into()],
            content: "We offer a full refund within 30 days.".into(),
            source: Some("chat-2026-07-11".into()),
            created_at: DateTime::from_timestamp(1_700_000_000, 0).unwrap(),
        };
        assert_eq!(r.topic, "FAQ");
        assert_eq!(r.tags.len(), 2);
        assert!(r.content.contains("refund"));
        assert_eq!(r.source, Some("chat-2026-07-11".into()));
    }

    #[test]
    fn knowledge_record_input_does_not_carry_engine_fields() {
        // The input type intentionally does NOT
        // have `record_id` or `created_at` — those
        // are engine-generated at store time. A
        // future contributor who adds them to the
        // input type would be reverting the
        // TRD-locked two-type design.
        let i = KnowledgeRecordInput {
            topic: "FAQ".into(),
            tags: vec!["billing".into()],
            content: "refund within 30 days".into(),
            source: None,
        };
        // Compile-time proof: the struct has
        // exactly the 4 fields we expect.
        assert_eq!(i.topic, "FAQ");
        assert_eq!(i.tags.len(), 1);
        assert_eq!(i.source, None);
    }
}
