//! Code Map: afa-contracts (the front door)
//! - `error`: The "what kind of trouble?" buckets and the
//!   "I'm an error" badge. See `error.rs`.
//! - `events`: The "I'm an event" badge and the sample event used
//!   by the conformance tests. See `events.rs`.
//! - `execution_context`: The envelope that travels with every
//!   request, plus the "who started this?" label. See
//!   `execution_context.rs`.
//! - `ids`: The tracking number and the tenant name tag. See
//!   `ids.rs`.
//! - `llm`: The v1 LLM engine contract: the trait `LlmV1`,
//!   the request/response/stream shapes, the capabilities
//!   card, the 13 typed errors, and the three audit events.
//!   See `llm/mod.rs`.
//! - `security`: The v1 security engine contract: the trait
//!   `SecurityV1`, the receipt `SecretRef`, the zeroing-on-drop
//!   handle `UnsealedSecret`, the eleven error buckets, and
//!   three audit events. See `security.rs`.
//! - `versioning_example`: A worked example of the "add a new
//!   socket, never change V1" versioning rule. See
//!   `versioning_example.rs`.
//!
//! Story (plain English): This file is the front door of the
//! shared types library. The other files in this folder hold the
//! actual types; this one just re-exports the most-used ones at
//! the top level so callers can write `use afa_contracts::Foo`
//! instead of `use afa_contracts::some_module::Foo`. The
//! `afa-contracts` crate is intentionally tiny: no I/O, no
//! async runtimes, no databases. It is the dictionary the rest
//! of the kernel agrees on, and a dictionary does not run a
//! post office.
//!
//! CID Index:
//! CID:afa-contracts-lib-001 -> error
//! CID:afa-contracts-lib-002 -> events
//! CID:afa-contracts-lib-003 -> execution_context
//! CID:afa-contracts-lib-004 -> ids
//! CID:afa-contracts-lib-005 -> llm
//! CID:afa-contracts-lib-006 -> security
//! CID:afa-contracts-lib-007 -> versioning_example
//!
//! Quick lookup: rg -n "CID:afa-contracts-lib-" crates/afa-contracts/src/lib.rs

#![doc(html_root_url = "https://docs.rs/afa-contracts/0.1.0")]

// CID:afa-contracts-lib-001 - error
// Purpose: Re-export the error-convention module so callers can
// reach it as `afa_contracts::error` (and the most-used items as
// `afa_contracts::AfaError`, `afa_contracts::AfaErrorKind`,
// `afa_contracts::ExampleStoreErrorV1`).
// Used by: every other AFA crate.
pub mod error;
// CID:afa-contracts-lib-002 - events
// Purpose: Re-export the event-convention module.
// Used by: every publisher and subscriber of events.
pub mod events;
// CID:afa-contracts-lib-003 - execution_context
// Purpose: Re-export the per-request context and actor label.
// Used by: every engine and plugin call signature.
pub mod execution_context;
// CID:afa-contracts-lib-004 - ids
// Purpose: Re-export the tracking number and tenant name tag.
// Used by: every request, every event, every log line.
pub mod ids;
// CID:afa-contracts-lib-005 - llm
// Purpose: Re-export the v1 LLM engine contract: the trait, the
// request/response/stream shapes, the capabilities card, the 13
// typed errors, and the three audit events. See `llm/mod.rs`
// for the Code Map.
// Used by: the `afa-plugin-llm-http` adapter (which implements
// the trait) and every workflow that calls `llm.complete` /
// `llm.stream_complete`.
pub mod llm;

// CID:afa-contracts-lib-008 - knowledge
// Purpose: Re-export the v1 Knowledge engine contract: the
// trait (`KnowledgeV1`), the record shape
// (`KnowledgeRecord`, `RecordId`, `KnowledgeRecordInput`),
// the query shape (`FindInformationRequest`,
// `FindInformationResponse`), the topic summary (`Topic`),
// the capabilities card (`KnowledgeCapabilities`), the 5
// typed errors (`KnowledgeErrorV1`, `KnowledgeErrorKind`),
// and the two audit events (`KnowledgeQueried`,
// `KnowledgeRecordStored`). See `knowledge/mod.rs` for the
// Code Map.
// Used by: the `afa-knowledge` engine (the conformance
// suite), the `afa-plugin-knowledge-json` adapter (the first
// concrete implementation), and every workflow that calls
// `knowledge.find_information` /
// `knowledge.store_information` / `knowledge.list_topics`.
pub mod knowledge;

// CID:afa-contracts-lib-009 - bus types
// Purpose: Re-export the LLM + Knowledge contract surface
// so consumers can write `use afa_contracts::KnowledgeV1`
// / `use afa_contracts::LlmV1` rather than reaching into
// the `llm::*` or `knowledge::*` submodules. The
// `find_information` / `store_information` methods take
// `ExecutionContext` (from `execution_context.rs`) and
// return `AfaError` / `AfaErrorKind` (from `error.rs`).
// Used by: every adapter and engine crate that implements
// or calls the v1 contracts.
pub use knowledge::{
    FindInformationRequest, FindInformationResponse, KnowledgeCapabilities, KnowledgeErrorKind,
    KnowledgeErrorV1, KnowledgeQueried, KnowledgeRecord, KnowledgeRecordInput,
    KnowledgeRecordStored, KnowledgeV1, RecordId, Topic,
};
// CID:afa-contracts-lib-006 - security
// Purpose: Re-export the v1 security engine contract: the trait,
// the receipt, the zeroing handle, the error type, and the
// three audit events. See `security.rs` for the Code Map.
// Used by: the `afa-security` engine (which implements the
// trait) and every adapter that needs a secret.
pub mod security;
// CID:afa-contracts-lib-007 - versioning_example
// Purpose: Re-export the worked example of the V1/V2 versioning
// rule and the dyn-compatibility pattern.
// Used by: the conformance test in `afa-contract-testing` and
// every real plugin that follows the pattern.
pub mod versioning_example;

pub use error::{AfaError, AfaErrorKind, ExampleStoreErrorV1};
pub use events::{AfaEvent, ExampleLessonCreated};
pub use execution_context::{Actor, ExecutionContext};
pub use ids::{CorrelationId, TenantId};
pub use llm::{
    CompletionChunk, CompletionCompleted, CompletionFailed, CompletionRequest, CompletionRequested,
    CompletionResponse, CompletionStream, ContentBlock, ConversationItem, FinishReason, ImageData,
    LlmErrorKind, LlmErrorV1, LlmV1, ModelCapabilities, SamplingParams, ToolCallRequest,
    ToolDefinition, Usage,
};
pub use security::{
    SecretRef, SecretRotated, SecretSealed, SecretUnsealed, SecurityErrorV1, SecurityV1,
    UnsealedSecret,
};
pub use versioning_example::{ExampleThingImpl, ExampleThingV1, ExampleThingV2};
