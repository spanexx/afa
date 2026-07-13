//! Code Map: afa-plugin-knowledge-json
//! - `adapter`: The `JsonKnowledgeAdapter` struct (the
//!   Phase 1 concrete implementation of `KnowledgeV1`).
//!   Phase 0: skeleton with `unimplemented!()` methods.
//!   Phase 1: real `store_information` + atomic write +
//!   boot + `KnowledgeRecordStored` event.
//!   Phase 2: real `find_information` + `list_topics`.
//! - `config`, `storage`, `index`, `search`,
//!   `atomic_write`, `topic_slug`: The sub-modules the
//!   adapter composes. Phase 0: empty stubs. Phase 1+
//!   populate.
//!
//! Story (plain English): This crate is the first
//! concrete storage adapter for the Knowledge engine.
//! It writes one Markdown file per record
//! (`<storage_root>/<topic_slug>/<record_id>.md`), one
//! JSON index file per storage root
//! (`<storage_root>/.index.json`), keeps an in-memory
//! inverted index for fast keyword search, and uses
//! the temp-then-rename atomic-write pattern for
//! crash safety. Phase 0 lands only the skeleton
//! (the struct, the trait impl, the empty sub-
//! modules) so the crate compiles; Phase 1+ populate
//! the per-method logic.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-001 -> adapter
//! CID:afa-plugin-knowledge-json-002 -> config
//! CID:afa-plugin-knowledge-json-003 -> storage
//! CID:afa-plugin-knowledge-json-004 -> index
//! CID:afa-plugin-knowledge-json-005 -> search
//! CID:afa-plugin-knowledge-json-006 -> atomic_write
//! CID:afa-plugin-knowledge-json-007 -> topic_slug
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-" crates/afa-plugin-knowledge-json/src/

// Phase 0 module declarations. The sub-modules
// are empty stubs that compile to "type X not
// used" warnings (which the build accepts in
// Phase 0; Phase 1+ populate them).
pub mod adapter;
pub mod atomic_write;
pub mod config;
pub mod index;
pub mod search;
pub mod storage;
pub mod topic_slug;
