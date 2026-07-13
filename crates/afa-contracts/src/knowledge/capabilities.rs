//! Code Map: Knowledge capabilities
//! - `KnowledgeCapabilities`: The "what can this
//!   storage backend do?" description. Three
//!   fields: `max_record_size_bytes` (the largest
//!   record the adapter will store), and two
//!   feature flags (`supports_semantic_search`,
//!   `supports_hierarchical_topics`). The values
//!   are decided at adapter construction and
//!   never change for the process lifetime.
//!
//! Story (plain English): The capabilities card
//! is the small name tag the storage backend wears
//! on its lapel. It says "I will hold records up
//! to 1 MB each," "I do not do vector search," and
//! "I treat topic names as flat strings." A
//! workflow that wants to send a 2 MB record checks
//! the card first and errors early if the storage
//! cannot take it. A workflow that wants to do
//! semantic search checks the card and picks a
//! different adapter if this one says "no."
//!
//! CID Index:
//! CID:knowledge-capabilities-001 -> KnowledgeCapabilities
//!
//! Quick lookup: rg -n "CID:knowledge-capabilities-" crates/afa-contracts/src/knowledge/capabilities.rs

use serde::{Deserialize, Serialize};

// CID:knowledge-capabilities-001 - KnowledgeCapabilities
// Purpose: The "what can this storage backend do?"
// description. Three fields:
// `max_record_size_bytes` (the hard cap on a
// single record's `content` length in bytes —
// 1_048_576 / 1 MB for the JSON v1 adapter), and
// two feature flags
// (`supports_semantic_search: false` for v1 — the
// JSON adapter does keyword search only;
// `supports_hierarchical_topics: false` for v1 —
// topic names are flat strings, no `billing/
// refunds` parent-child). The values are decided
// at adapter construction and never change for
// the process lifetime. There is no per-request
// negotiation: the capabilities are the
// capabilities.
// Uses: nothing.
// Used by: `KnowledgeV1::describe_capabilities`,
// `JsonKnowledgeConfig` (the source of the
// values for the JSON adapter).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KnowledgeCapabilities {
    /// The hard cap on a single record's
    /// `content` length in bytes. The JSON v1
    /// adapter sets this to 1_048_576 (1 MB).
    /// A workflow that wants to send a 2 MB
    /// record must check this first and
    /// surface a clear error before the
    /// adapter rejects the call.
    pub max_record_size_bytes: u32,
    /// Whether the adapter supports vector /
    /// semantic search. `false` for the JSON
    /// v1 adapter (keyword search only). A
    /// future Postgres + pgVector adapter
    /// will set this to `true`.
    pub supports_semantic_search: bool,
    /// Whether the adapter supports
    /// hierarchical topic namespaces (e.g.
    /// `billing/refunds` as a true
    /// parent-child relationship). `false`
    /// for v1 (topic names are flat strings).
    pub supports_hierarchical_topics: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knowledge_capabilities_carries_all_three_fields() {
        // The three fields are the locked
        // shape from the TRD §2.2.5. A
        // workflow that wants to check any
        // of the three must find them on
        // this struct.
        let caps = KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        };
        assert_eq!(caps.max_record_size_bytes, 1_048_576);
        assert!(!caps.supports_semantic_search);
        assert!(!caps.supports_hierarchical_topics);
    }

    #[test]
    fn knowledge_capabilities_round_trips_through_serde() {
        // A workflow may persist the
        // capabilities (e.g. to log which
        // backend handled a query). The
        // round-trip must preserve every
        // field exactly.
        let caps = KnowledgeCapabilities {
            max_record_size_bytes: 5_242_880,
            supports_semantic_search: true,
            supports_hierarchical_topics: false,
        };
        let json = serde_json::to_string(&caps).expect("serialize");
        let back: KnowledgeCapabilities = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, caps);
    }
}
