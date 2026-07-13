//! Code Map: Knowledge Engine contract surface
//! - `KnowledgeV1`: The "I am a Knowledge engine" badge. The
//!   locked v1 trait every storage adapter implements.
//!   See `mod.rs` for the trait signature and CID
//!   commentary.
//! - `KnowledgeRecord`, `RecordId`, `KnowledgeRecordInput`:
//!   The shape of a stored record. See `record.rs`.
//! - `FindInformationRequest`, `FindInformationResponse`:
//!   The shape of a query. See `request.rs`.
//! - `Topic`: The shape of a topic summary. See `topic.rs`.
//! - `KnowledgeCapabilities`: The "what can this storage do?"
//!   description. See `capabilities.rs`.
//! - `KnowledgeErrorV1`, `KnowledgeErrorKind`: The 5 typed
//!   "what went wrong?" buckets. See `error.rs`.
//! - `KnowledgeQueried`, `KnowledgeRecordStored`: The two
//!   audit facts. See `events.rs`.
//!
//! Story (plain English): The Knowledge Engine is the card
//! catalog. The catalog holds cards for every fact the
//! agency has ever learned. A workflow that wants to know
//! "do we have a refund policy?" searches the catalog. A
//! workflow that wants to commit a new fact (after a
//! human approves it) adds a card. The catalog does not
//! read the cards; the catalog just stores them,
//! retrieves them, and ranks them. The storage (JSON
//! files today, a database tomorrow) is the adapter's
//! concern; the contract is the engine's concern.
//!
//! CID Index:
//! CID:knowledge-001 -> KnowledgeV1
//! CID:knowledge-002 -> KnowledgeRecord, RecordId, KnowledgeRecordInput
//! CID:knowledge-003 -> FindInformationRequest, FindInformationResponse
//! CID:knowledge-004 -> Topic
//! CID:knowledge-005 -> KnowledgeCapabilities
//! CID:knowledge-006 -> KnowledgeErrorV1, KnowledgeErrorKind
//! CID:knowledge-007 -> KnowledgeQueried, KnowledgeRecordStored
//!
//! Quick lookup: rg -n "CID:knowledge-" crates/afa-contracts/src/knowledge/

pub mod capabilities;
pub mod error;
pub mod events;
pub mod record;
pub mod request;
pub mod topic;

use async_trait::async_trait;

use crate::execution_context::ExecutionContext;
pub use capabilities::KnowledgeCapabilities;
pub use error::{KnowledgeErrorKind, KnowledgeErrorV1};
pub use events::{KnowledgeQueried, KnowledgeRecordStored, KnowledgeTopicsListed};
pub use record::{KnowledgeRecord, KnowledgeRecordInput, RecordId};
pub use request::{FindInformationRequest, FindInformationResponse};
pub use topic::Topic;

// CID:knowledge-001 - KnowledgeV1
// Purpose: The locked v1 contract for any Knowledge
// storage adapter. Four methods: `find_information`
// (ranked query), `store_information` (append a
// record), `list_topics` (list all topics with
// metadata), and `describe_capabilities`
// (synchronous, returns the static
// `KnowledgeCapabilities` of the storage backend).
// All async methods are `#[async_trait]`-decorated
// so adapters can be held behind `Arc<dyn
// KnowledgeV1>` in the `CapabilityRegistry`. The
// `Send + Sync` supertrait lets the registry share
// one adapter across many concurrent callers.
// Uses: FindInformationRequest,
// FindInformationResponse, KnowledgeRecordInput,
// RecordId, Topic, ExecutionContext,
// KnowledgeCapabilities, KnowledgeErrorV1.
// Used by: the `CapabilityRegistry` (holds
// `Arc<dyn KnowledgeV1>`), every workflow that
// calls `knowledge.find_information` /
// `knowledge.store_information` /
// `knowledge.list_topics`, and the conformance
// suite in `afa-knowledge`.
#[async_trait]
pub trait KnowledgeV1: Send + Sync {
    /// Find records matching the request. Returns
    /// the top N records ordered by descending
    /// score, or an empty `Vec` if nothing matches
    /// (empty is not an error). The adapter does
    /// not interpret the `content` field; it only
    /// stores it, retrieves it, and does free-text
    /// token matching.
    async fn find_information(
        &self,
        request: FindInformationRequest,
        ctx: &ExecutionContext,
    ) -> Result<FindInformationResponse, KnowledgeErrorV1>;

    /// Commit a new record to storage. The engine
    /// generates the `RecordId` and the
    /// `created_at` timestamp; the writer supplies
    /// everything else via `KnowledgeRecordInput`.
    /// Returns the engine-assigned `RecordId` on
    /// success. Publishes `KnowledgeRecordStored`
    /// after the disk write is durable.
    async fn store_information(
        &self,
        record: KnowledgeRecordInput,
        ctx: &ExecutionContext,
    ) -> Result<RecordId, KnowledgeErrorV1>;

    /// List every topic with at least one record,
    /// sorted alphabetically by name. Each `Topic`
    /// carries the record count, the first and
    /// last record timestamps, and the
    /// distinct-tag count. v1 does NOT publish an
    /// event for `list_topics` (the PRD locks
    /// exactly two events: one for
    /// `find_information`, one for
    /// `store_information`).
    async fn list_topics(&self, ctx: &ExecutionContext) -> Result<Vec<Topic>, KnowledgeErrorV1>;

    /// Synchronously return the static capabilities
    /// of the storage backend. The values are
    /// decided at adapter construction and never
    /// change for the process lifetime. No `async`,
    /// no `ctx`, no I/O.
    fn describe_capabilities(&self) -> KnowledgeCapabilities;
}
