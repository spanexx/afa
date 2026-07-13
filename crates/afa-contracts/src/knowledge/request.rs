//! Code Map: Knowledge query shape
//! - `FindInformationRequest`: The four-field query
//!   envelope. All four fields are optional; a
//!   request with no fields returns the most recent
//!   N records.
//! - `FindInformationResponse`: A type alias for
//!   `Vec<(KnowledgeRecord, f32)>` — the ranked
//!   results with their scores.
//!
//! Story (plain English): The query is the patron's
//! question card at the card-catalog desk. The card
//! has four boxes: the free-text question
//! ("cancellation policy"), the topic ("FAQ"), the
//! tags the patron cares about ("billing", "refund"),
//! and a limit on how many results to bring back.
//! The patron can leave any box empty. An empty
//! card means "show me whatever you have, newest
//! first."
//!
//! CID Index:
//! CID:knowledge-request-001 -> FindInformationRequest
//! CID:knowledge-request-002 -> FindInformationResponse
//!
//! Quick lookup: rg -n "CID:knowledge-request-" crates/afa-contracts/src/knowledge/request.rs

use serde::{Deserialize, Serialize};

use super::record::KnowledgeRecord;

// CID:knowledge-request-001 - FindInformationRequest
// Purpose: The query envelope. All four fields are
// optional. The filters compose: a request with
// `topic: Some("FAQ")` AND `tags: vec!["billing"]`
// AND `free_text: Some("refund")` returns records
// in topic "FAQ" that have the "billing" tag AND
// mention "refund" in the content. The
// `limit: None` defaults to 10 inside the adapter.
// Uses: nothing.
// Used by: `KnowledgeV1::find_information`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindInformationRequest {
    /// Free-text search query. Tokenized
    /// (whitespace split, lowercase, drop
    /// tokens shorter than 2 chars) and matched
    /// against the `content` field of candidate
    /// records. `None` = no free-text scoring.
    pub free_text: Option<String>,
    /// Filter to records in this topic only.
    /// `None` = consider records in any topic.
    pub topic: Option<String>,
    /// AND-filter: every tag in this list must
    /// be present in the record's `tags`. Empty
    /// = no tag filter.
    pub tags: Vec<String>,
    /// Maximum number of results to return.
    /// `None` = adapter default (10).
    pub limit: Option<u32>,
}

// CID:knowledge-request-002 - FindInformationResponse
// Purpose: The ranked result list. A type alias
// for `Vec<(KnowledgeRecord, f32)>` — each tuple
// is the matched record and its score (a `f32` in
// `[0.0, 1.0]`). Ordered by descending score
// (best match first). Empty result is `vec![]`,
// NOT an error — the workflow pattern-matches the
// list.
// Uses: KnowledgeRecord.
// Used by: `KnowledgeV1::find_information`.
pub type FindInformationResponse = Vec<(KnowledgeRecord, f32)>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_information_request_default_is_all_none_and_empty() {
        // The `Default` impl is the canonical
        // "show me whatever you have, newest
        // first" request: all four fields are
        // empty/none. The adapter's "no params"
        // path returns the most recent N
        // records sorted by `created_at`
        // descending, all with score `1.0`.
        let r = FindInformationRequest::default();
        assert!(r.free_text.is_none());
        assert!(r.topic.is_none());
        assert!(r.tags.is_empty());
        assert!(r.limit.is_none());
    }

    #[test]
    fn find_information_request_round_trips_through_serde() {
        // A request with all four fields set
        // must round-trip exactly. If a future
        // contributor drops a field, the
        // round-trip will lose data and this
        // test will fail.
        let r = FindInformationRequest {
            free_text: Some("refund policy".into()),
            topic: Some("FAQ".into()),
            tags: vec!["billing".into(), "refund".into()],
            limit: Some(5),
        };
        let json = serde_json::to_string(&r).expect("serialize");
        let back: FindInformationRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, r);
    }

    #[test]
    fn find_information_response_is_a_vec_of_record_score_tuples() {
        // The type alias is the locked shape:
        // `Vec<(KnowledgeRecord, f32)>`. A
        // future contributor who tries to
        // change this (e.g. a named-field
        // struct) would be breaking the
        // contract and would need a new
        // `KnowledgeV2`.
        let _resp: FindInformationResponse = Vec::new();
    }
}
