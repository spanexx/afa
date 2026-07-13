//! Code Map: JsonKnowledgeAdapter skeleton
//! - `JsonKnowledgeAdapter`: The Phase 1 concrete
//!   implementation of `KnowledgeV1`. Phase 0:
//!   skeleton with `unimplemented!()` methods +
//!   a `new` constructor that holds the
//!   `JsonKnowledgeConfig` (which Phase 0
//!   also stubs out as a unit struct).
//!
//! Story (plain English): The adapter is the
//! card-catalog file cabinet. It knows the
//! filing system: one drawer per topic, one
//! card per record, one index card per drawer.
//! It also knows the rules of the filing
//! system: temp-then-rename for the index card
//! (so the index never goes half-written), and
//! the topic-slug rules (lowercase, no
//! non-ASCII, collapsed dashes). Phase 0 lands
//! the cabinet shell with the drawers glued
//! shut; Phase 1 wires up the filing.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-adapter-001 -> JsonKnowledgeAdapter
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-adapter-" crates/afa-plugin-knowledge-json/src/adapter.rs

use std::sync::Arc;

use afa_contracts::{
    ExecutionContext, FindInformationRequest, FindInformationResponse, KnowledgeCapabilities,
    KnowledgeErrorV1, KnowledgeRecordInput, KnowledgeV1, RecordId, Topic,
};
use async_trait::async_trait;

use crate::config::JsonKnowledgeConfig;
use crate::index::InMemoryIndex;

// CID:afa-plugin-knowledge-json-adapter-001 - JsonKnowledgeAdapter
// Purpose: The Phase 1 concrete implementation of
// `KnowledgeV1`. Phase 0: the struct exists (so
// `Arc<dyn KnowledgeV1>` can be parameterized at
// the call site) and the trait impl exists (so
// the registry's `register_knowledge` can store
// it), but every method body is `unimplemented!`
// — the test binary will panic if it tries to
// call any method. The Phase 0 validation
// condition is that the crate compiles; Phase 1
// validates the happy path; Phase 2 validates
// the read path; Phase 3 validates the
// resilience properties.
//
// The constructor takes a `JsonKnowledgeConfig`
// (a Phase 0 stub) and a `tokio::sync::Mutex`-less
// `Arc<InMemoryIndex>` (Phase 0: the inner type
// is a stub unit struct). The boot sequence
// (Phase 1) will populate the index from
// `.index.json`; Phase 0 holds an empty stub.
pub struct JsonKnowledgeAdapter {
    _config: JsonKnowledgeConfig,
    _index: Arc<InMemoryIndex>,
}

impl JsonKnowledgeAdapter {
    /// Construct a new `JsonKnowledgeAdapter`
    /// from the given `JsonKnowledgeConfig`.
    /// Phase 1 will: (a) verify the storage
    /// root exists and is writable, (b) load
    /// `.index.json` into the in-memory
    /// index, (c) return the constructed
    /// adapter. Phase 0: just stores the
    /// config + an empty index stub.
    pub fn new(config: JsonKnowledgeConfig) -> Self {
        Self {
            _config: config,
            _index: Arc::new(InMemoryIndex::new()),
        }
    }
}

#[async_trait]
impl KnowledgeV1 for JsonKnowledgeAdapter {
    async fn find_information(
        &self,
        _request: FindInformationRequest,
        _ctx: &ExecutionContext,
    ) -> Result<FindInformationResponse, KnowledgeErrorV1> {
        // Phase 0 stub. Phase 2 populates.
        unimplemented!("populate in Phase 2")
    }

    async fn store_information(
        &self,
        _record: KnowledgeRecordInput,
        _ctx: &ExecutionContext,
    ) -> Result<RecordId, KnowledgeErrorV1> {
        // Phase 0 stub. Phase 1 populates.
        unimplemented!("populate in Phase 1")
    }

    async fn list_topics(&self, _ctx: &ExecutionContext) -> Result<Vec<Topic>, KnowledgeErrorV1> {
        // Phase 0 stub. Phase 2 populates.
        unimplemented!("populate in Phase 2")
    }

    fn describe_capabilities(&self) -> KnowledgeCapabilities {
        // The v1 JSON adapter capabilities are
        // locked at construction. The values
        // match the PRD: 1 MB max record,
        // no semantic search, no hierarchical
        // topics. Returning the same struct
        // on every call is the locked
        // behavior; the `describe_capabilities`
        // method is a synchronous accessor, not
        // a query.
        KnowledgeCapabilities {
            max_record_size_bytes: 1_048_576,
            supports_semantic_search: false,
            supports_hierarchical_topics: false,
        }
    }
}
