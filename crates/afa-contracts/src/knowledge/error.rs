//! Code Map: Knowledge error surface
//! - `KnowledgeErrorV1`: The 5 typed "what went
//!   wrong with the Knowledge engine?" buckets.
//!   The closed set maps onto the 6 coarse
//!   `AfaErrorKind` buckets the kernel already
//!   understands, with no new kinds introduced.
//! - `KnowledgeErrorKind`: A re-export of
//!   `AfaErrorKind` under a more specific name,
//!   so callers of the Knowledge module can
//!   name the kind without reaching into
//!   `error`.
//! - `impl AfaError for KnowledgeErrorV1`: The
//!   5-to-6 mapping.
//!
//! Story (plain English): The error set is the
//! small chart of "why might a knowledge call
//! fail?" Five rows: caller bug (bad input),
//! dependency down (storage unreachable), data
//! trouble (record corrupt), wrong tool (this
//! backend cannot do what you asked), and the
//! catch-all (something we did not predict).
//! Six colours (the kernel's six coarse
//! buckets). The operator picks the row, stamps
//! the colour, hands the chart to the workflow.
//! The workflow knows the colour tells it what
//! to do (don't retry caller bugs, retry storage
//! outages with backoff, alert on plain broken).
//!
//! CID Index:
//! CID:knowledge-error-001 -> KnowledgeErrorV1
//! CID:knowledge-error-002 -> KnowledgeErrorKind
//! CID:knowledge-error-003 -> impl AfaError for KnowledgeErrorV1
//!
//! Quick lookup: rg -n "CID:knowledge-error-" crates/afa-contracts/src/knowledge/error.rs

use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::error::{AfaError, AfaErrorKind};

// CID:knowledge-error-002 - KnowledgeErrorKind
// Purpose: A re-export of `AfaErrorKind` under a
// more specific name, so callers of the
// Knowledge module can name the kind without
// reaching into `error`. The underlying set is
// the same 6 coarse buckets; the re-export is a
// naming convenience, not a new type. A future
// contributor who tries to add a new variant
// here would be silently adding a 7th colour
// to the kernel's chart, which is an
// ADR-backed change, not a doc tweak.
// Uses: `crate::error::AfaErrorKind`.
// Used by: callers of the Knowledge module
// that want to branch on the kind without
// naming the concrete `KnowledgeErrorV1`
// variant.
pub use crate::error::AfaErrorKind as KnowledgeErrorKind;

// CID:knowledge-error-001 - KnowledgeErrorV1
// Purpose: The 5 typed "what went wrong with
// the Knowledge engine?" buckets. The closed
// set from the TRD §2.2.6. Each variant carries
// `topic: Option<String>` (which topic the call
// was about), `record_id: Option<Uuid>` (which
// record, if applicable — `None` for calls that
// do not reference a record, like a
// `find_information` on a topic that does not
// exist), and `reason: String` (human-readable
// detail for the audit log). The three common
// fields are public for direct access by
// workflows and the audit dashboard.
// Uses: thiserror, uuid::Uuid, `AfaError`.
// Used by: every `KnowledgeV1` method (and,
// transitively, by every workflow that calls
// `knowledge.find_information` /
// `knowledge.store_information` /
// `knowledge.list_topics`).
#[derive(Debug, Clone, Serialize, Deserialize, Error)]
pub enum KnowledgeErrorV1 {
    /// Caller bug: the input was invalid
    /// (empty topic, empty content, content
    /// too large, etc.). Maps to `Internal` —
    /// the caller should fix the bug, not
    /// retry.
    #[error("invalid input (topic: {topic:?}, record_id: {record_id:?}): {reason}")]
    InvalidInput {
        topic: Option<String>,
        record_id: Option<Uuid>,
        reason: String,
    },
    /// The storage backend is unavailable
    /// (disk full, read-only filesystem, lost
    /// write permission, topic-name slug
    /// collision). Maps to `Unavailable` —
    /// the caller may retry with backoff.
    #[error("storage unavailable (topic: {topic:?}, record_id: {record_id:?}): {reason}")]
    StorageUnavailable {
        topic: Option<String>,
        record_id: Option<Uuid>,
        reason: String,
    },
    /// A stored record is corrupt (a partial
    /// write that survived a crash, a manual
    /// edit gone wrong). Maps to `Internal` —
    /// operator action required; do not
    /// retry.
    #[error("malformed record (topic: {topic:?}, record_id: {record_id:?}): {reason}")]
    MalformedRecord {
        topic: Option<String>,
        record_id: Option<Uuid>,
        reason: String,
    },
    /// The caller asked for a capability the
    /// adapter does not have (e.g. semantic
    /// search against the JSON adapter).
    /// Maps to `CapabilityUnsupported` —
    /// use a different adapter; do not retry
    /// the same one.
    #[error("capability unsupported (topic: {topic:?}, record_id: {record_id:?}): {reason}")]
    CapabilityUnsupported {
        topic: Option<String>,
        record_id: Option<Uuid>,
        reason: String,
    },
    /// Catch-all for unexpected internal
    /// failures (bugs, invariant
    /// violations). Maps to `Internal`.
    #[error("knowledge internal error (topic: {topic:?}, record_id: {record_id:?}): {reason}")]
    Internal {
        topic: Option<String>,
        record_id: Option<Uuid>,
        reason: String,
    },
}

// CID:knowledge-error-003 - impl AfaError for KnowledgeErrorV1
// Purpose: The 5-to-6 mapping from
// `KnowledgeErrorV1` variants to `AfaErrorKind`
// buckets. Per the TRD §2.2.6 table:
// `InvalidInput` → `Internal` (caller bug),
// `StorageUnavailable` → `Unavailable` (retry
// possible), `MalformedRecord` → `Internal`
// (operator action), `CapabilityUnsupported` →
// `CapabilityUnsupported`, `Internal` →
// `Internal`. Generic code (e.g. the
// conformance suite) can branch on the kind
// without naming the concrete variant. Note
// that the 5-to-6 mapping is many-to-one: three
// variants map to `Internal` (the "caller bug
// or data trouble or unknown" bucket).
// Uses: `AfaError`, `AfaErrorKind`.
// Used by: every generic caller that wants to
// react to the kind of trouble without knowing
// the exact error type.
impl AfaError for KnowledgeErrorV1 {
    fn kind(&self) -> AfaErrorKind {
        match self {
            // Caller bug: the input was bad. Do
            // not retry.
            Self::InvalidInput { .. } => AfaErrorKind::Internal,
            // Dependency is down or refusing
            // writes. The caller may retry with
            // backoff.
            Self::StorageUnavailable { .. } => AfaErrorKind::Unavailable,
            // Data trouble: a record on disk
            // is corrupt. Operator action
            // required; do not retry.
            Self::MalformedRecord { .. } => AfaErrorKind::Internal,
            // The contract is known but not
            // supported by this build. Use a
            // different adapter.
            Self::CapabilityUnsupported { .. } => AfaErrorKind::CapabilityUnsupported,
            // Unknown internal failure. Do
            // not retry; alert engineering.
            Self::Internal { .. } => AfaErrorKind::Internal,
        }
    }
}

// CID:knowledge-error-004 - common-field accessors
// Purpose: The three common fields
// (`topic`, `record_id`, `reason`) are
// accessed by workflows and the audit
// dashboard. Rust's struct-variant enum
// fields are not directly accessible by
// name (you must pattern-match the
// variant), so the three accessors below
// provide "direct access" semantics. The
// accessors are total — every variant
// carries all three fields — so the
// caller does not need to know which
// variant it has. The `topic` and
// `record_id` accessors return
// `Option<...>` (mirroring the field
// type) so a caller can pass through the
// "no topic / no record" case; the
// `reason` accessor returns `&str`
// (always present in every variant).
impl KnowledgeErrorV1 {
    /// The topic the failing call was
    /// about, or `None` if the call did
    /// not reference a topic.
    pub fn topic(&self) -> Option<&str> {
        match self {
            Self::InvalidInput { topic, .. }
            | Self::StorageUnavailable { topic, .. }
            | Self::MalformedRecord { topic, .. }
            | Self::CapabilityUnsupported { topic, .. }
            | Self::Internal { topic, .. } => topic.as_deref(),
        }
    }

    /// The record the failing call was
    /// about, or `None` if the call did
    /// not reference a record.
    pub fn record_id(&self) -> Option<Uuid> {
        match self {
            Self::InvalidInput { record_id, .. }
            | Self::StorageUnavailable { record_id, .. }
            | Self::MalformedRecord { record_id, .. }
            | Self::CapabilityUnsupported { record_id, .. }
            | Self::Internal { record_id, .. } => *record_id,
        }
    }

    /// The human-readable reason for
    /// the failure (suitable for an
    /// audit log line). Always present
    /// in every variant.
    pub fn reason(&self) -> &str {
        match self {
            Self::InvalidInput { reason, .. }
            | Self::StorageUnavailable { reason, .. }
            | Self::MalformedRecord { reason, .. }
            | Self::CapabilityUnsupported { reason, .. }
            | Self::Internal { reason, .. } => reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn topic() -> Option<String> {
        Some("FAQ".into())
    }

    fn rid() -> Option<Uuid> {
        Some(Uuid::nil())
    }

    #[test]
    fn invalid_input_classifies_as_internal() {
        let e = KnowledgeErrorV1::InvalidInput {
            topic: topic(),
            record_id: rid(),
            reason: "content is empty".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::Internal);
    }

    #[test]
    fn storage_unavailable_classifies_as_unavailable() {
        let e = KnowledgeErrorV1::StorageUnavailable {
            topic: topic(),
            record_id: rid(),
            reason: "no space left on device".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::Unavailable);
    }

    #[test]
    fn malformed_record_classifies_as_internal() {
        let e = KnowledgeErrorV1::MalformedRecord {
            topic: topic(),
            record_id: rid(),
            reason: ".index.json is corrupt".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::Internal);
    }

    #[test]
    fn capability_unsupported_classifies_as_capability_unsupported() {
        let e = KnowledgeErrorV1::CapabilityUnsupported {
            topic: None,
            record_id: None,
            reason: "semantic search is not supported".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::CapabilityUnsupported);
    }

    #[test]
    fn internal_classifies_as_internal() {
        let e = KnowledgeErrorV1::Internal {
            topic: None,
            record_id: None,
            reason: "unexpected".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::Internal);
    }

    #[test]
    fn all_five_variants_have_a_kind_mapping() {
        // Regression-proof that the 5-to-6
        // mapping is complete. If a future
        // contributor adds a 6th variant, the
        // compiler will force them to add a
        // kind mapping (the match is
        // exhaustive on `Self`).
        let samples: Vec<KnowledgeErrorV1> = vec![
            KnowledgeErrorV1::InvalidInput {
                topic: None,
                record_id: None,
                reason: "x".into(),
            },
            KnowledgeErrorV1::StorageUnavailable {
                topic: None,
                record_id: None,
                reason: "x".into(),
            },
            KnowledgeErrorV1::MalformedRecord {
                topic: None,
                record_id: None,
                reason: "x".into(),
            },
            KnowledgeErrorV1::CapabilityUnsupported {
                topic: None,
                record_id: None,
                reason: "x".into(),
            },
            KnowledgeErrorV1::Internal {
                topic: None,
                record_id: None,
                reason: "x".into(),
            },
        ];
        assert_eq!(samples.len(), 5, "the pack is 5 variants");
        for e in samples {
            let kind = e.kind();
            assert!(
                matches!(
                    kind,
                    AfaErrorKind::NotFound
                        | AfaErrorKind::Unauthorized
                        | AfaErrorKind::Unavailable
                        | AfaErrorKind::Timeout
                        | AfaErrorKind::CapabilityUnsupported
                        | AfaErrorKind::Internal
                ),
                "variant {e:?} mapped to unknown kind {kind:?}"
            );
        }
    }

    #[test]
    fn error_carries_topic_record_id_and_reason() {
        // The three common fields are
        // accessed via the `topic()`,
        // `record_id()`, and `reason()`
        // accessors. Rust's struct-variant
        // enum fields are not directly
        // accessible by name (you must
        // pattern-match the variant), so
        // the three accessors provide
        // "direct access" semantics for
        // workflows and the audit
        // dashboard. The accessors are
        // total — every variant carries
        // all three fields — so the
        // caller does not need to know
        // which variant it has.
        let e = KnowledgeErrorV1::InvalidInput {
            topic: Some("FAQ".into()),
            record_id: Some(Uuid::nil()),
            reason: "content is empty".into(),
        };
        assert_eq!(e.topic(), Some("FAQ"));
        assert_eq!(e.record_id(), Some(Uuid::nil()));
        assert_eq!(e.reason(), "content is empty");
    }
}
