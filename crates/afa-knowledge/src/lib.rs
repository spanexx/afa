//! Code Map: afa-knowledge
//! - Re-exports: The v1 Knowledge contract from
//!   `afa-contracts::knowledge::*` (the trait, the 9
//!   types, the 2 events, the 5 error variants). Lets
//!   consumers write `use afa_knowledge::KnowledgeV1`
//!   rather than reaching into `afa-contracts::knowledge`.
//! - `conformance`: The contract-conformance suite
//!   stub. Phase 0: unimplemented; Phase 2: populated
//!   with the per-method test cases that any
//!   `Arc<dyn KnowledgeV1>` must pass.
//!
//! Story (plain English): `afa-knowledge` is the
//! switchboard for the Knowledge capability. It does
//! not know how records are stored; it just hands
//! workflows a vendor-neutral `KnowledgeV1` and runs
//! the conformance suite against any adapter that
//! implements the trait. The conformance suite is
//! the safety net: any adapter that claims to be
//! `KnowledgeV1` must pass the suite. Phase 0 lands
//! only the re-exports + the empty suite; Phase 2
//! populates the suite.
//!
//! CID Index:
//! CID:afa-knowledge-001 -> re-exports
//! CID:afa-knowledge-002 -> conformance
//!
//! Quick lookup: rg -n "CID:afa-knowledge-" crates/afa-knowledge/src/

pub mod conformance;

// CID:afa-knowledge-001 - re-exports
// Purpose: Re-export the v1 Knowledge contract
// surface from `afa-contracts::knowledge`. This is
// the stable public surface of the `afa-knowledge`
// crate; downstream consumers should `use
// afa_knowledge::KnowledgeV1` rather than reaching
// into `afa_contracts::knowledge::*` so a future
// version bump that moves the types out of
// `afa-contracts` (e.g. into `afa-knowledge`) only
// touches this re-export, not every consumer.
// Used by: every workflow + every adapter that
// touches the Knowledge capability; the
// conformance suite in `conformance.rs`.
pub use afa_contracts::knowledge::{
    FindInformationRequest, FindInformationResponse, KnowledgeCapabilities, KnowledgeErrorKind,
    KnowledgeErrorV1, KnowledgeQueried, KnowledgeRecord, KnowledgeRecordInput,
    KnowledgeRecordStored, KnowledgeTopicsListed, KnowledgeV1, RecordId, Topic,
};
pub use conformance::{run_conformance_suite, MockAdapter, MockCall};
