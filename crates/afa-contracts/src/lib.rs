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
//! - `embedding`: The v1 Embedding engine contract: the
//!   trait (`EmbeddingV1`), the error type
//!   (`EmbeddingErrorV1`), the capabilities card
//!   (`EmbeddingCapabilitiesV1`), and the three audit
//!   events. See `embedding/mod.rs` for the Code Map.
//! - `kernel`: The four-state lifecycle the kernel walks
//!   through from `Booting` to `Full`
//!   (`Booting → PreBootstrap → Sealing → Full`, with a
//!   `Sealing → PreBootstrap` failure branch). See
//!   `kernel.rs` for the Code Map.
//! - `observability`: The v1 observability contract: the
//!   `SpanRecord` / `SpanOutcome` shape (the one row in the
//!   spans table), the `HealthStatus` / `HealthReport`
//!   shape (the per-engine health envelope), the three
//!   audit events (`SpansWriteFailed` / `SpansPurged` /
//!   `SpansPurgeFailed`), the `StorageError` and
//!   `ObservabilityErrorV1` enums, and the `HealthCheck`
//!   trait every engine implements. See
//!   `observability.rs` for the Code Map.
//! - `storage`: The `Migration` DTO the `afa-storage` crate
//!   consumes at boot. See `storage.rs` for the Code Map.
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
//! CID:afa-contracts-lib-008 -> knowledge
//! CID:afa-contracts-lib-009 -> bus types
//! CID:afa-contracts-lib-010 -> embedding
//! CID:afa-contracts-lib-011 -> observability
//! CID:afa-contracts-lib-012 -> storage
//! CID:afa-contracts-lib-013 -> kernel
//! CID:afa-contracts-lib-014 -> cli
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
// CID:afa-contracts-lib-010 - embedding
// Purpose: Re-export the v1 Embedding engine contract: the
// trait (`EmbeddingV1`), the error type
// (`EmbeddingErrorV1`), the capabilities card
// (`EmbeddingCapabilitiesV1`), and the three audit events
// (`EmbeddingRequested`/`EmbeddingCompleted`/`EmbeddingFailed`).
// See `embedding/mod.rs` for the Code Map.
// Used by: the `afa-plugin-embedding-local` and
// `afa-plugin-embedding-ollama` adapters (which implement the
// trait), Pack #24 ingestion (which calls `embed_batch`),
// and the conformance suite in `afa-contract-testing`.
pub mod embedding;

// CID:afa-contracts-lib-011 - observability
// Purpose: Re-export the v1 Observability contract: the
// `SpanRecord` / `SpanOutcome` / `HealthStatus` /
// `HealthReport` types, the three audit events
// (`SpansWriteFailed` / `SpansPurged` / `SpansPurgeFailed`),
// the `StorageError` and `ObservabilityErrorV1` enums, and
// the `HealthCheck` trait every engine implements. See
// `observability.rs` for the Code Map.
// Used by: the `afa-observability` engine (the writer of
// spans and the publisher of the three audit events), every
// engine's `HealthCheck` impl, the kernel's
// `aggregate_health()` aggregator, and the dashboard's
// `GET /health` and `/spans/*` handlers.
pub mod observability;

// CID:afa-contracts-lib-012 - storage
// Purpose: Re-export the `Migration` DTO the `afa-storage`
// crate consumes at boot. See `storage.rs` for the Code
// Map. Used by: every engine that ships a SQLite schema
// (`afa-security` for the `sealed_secrets` table,
// `afa-observability` for the `spans` table, future
// engines' tables).
pub mod storage;

// CID:afa-contracts-lib-014 - cli
// Purpose: Re-export the `afa-cli` shared wire +
// state types (`PreBootstrapState`,
// `PreBootstrapSealRequest`, `PreBootstrapSealResponse`,
// `SecretListEntry`). These mirror the wire shapes
// the kernel's `POST /pre-bootstrap/seal` handler
// already accepts; the CLI uses them to build the
// same body as the dashboard's SPA Setup Wizard.
// Used by: `afa-cli` (future pack) and the
// kernel's `dashboard::pre_bootstrap` adapter.
// See `cli.rs` for the Code Map.
pub mod cli;

// CID:afa-contracts-lib-013 - kernel
// Purpose: Re-export the kernel's four-state lifecycle
// (`KernelMode`) and the helper predicates
// (`is_sealed`, `is_pre_bootstrap`, etc.) the
// dashboard transport uses to gate `/health`
// responses and the future `POST /pre-bootstrap/seal`
// endpoint. The `Display` impl is the wire shape
// the dashboard surfaces verbatim. See `kernel.rs`
// for the Code Map.
// Used by: afa-kernel::dashboard (Phase 3 + Phase 4),
// and the kernel's own boot path (Phase 4b).
pub mod kernel;

pub use embedding::{
    EmbeddingCapabilitiesV1, EmbeddingCompleted, EmbeddingErrorKind, EmbeddingErrorV1,
    EmbeddingFailed, EmbeddingRequested, EmbeddingV1, EmbeddingV1Version,
};
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
// CID:afa-contracts-lib-011 re-exports - observability
// Flatten the observability module's most-used types to
// the crate root so callers can write
// `use afa_contracts::SpanRecord;` instead of
// `use afa_contracts::observability::SpanRecord;`. The
// `StorageError` re-export is the one the `afa-storage`
// crate itself uses (`pub use afa_contracts::StorageError;`).
pub use observability::{
    HealthCheck, HealthReport, HealthStatus, ObservabilityErrorV1, SpanOutcome, SpanRecord,
    SpansPurgeFailed, SpansPurged, SpansWriteFailed, StorageError,
};
pub use security::{
    SecretRef, SecretRotated, SecretSealed, SecretUnsealed, SecurityErrorV1, SecurityV1,
    UnsealedSecret,
};
// CID:afa-contracts-lib-012 re-exports - storage
// Flatten the `Migration` DTO to the crate root so
// `use afa_contracts::Migration;` works.
pub use cli::{
    PreBootstrapSealRequest, PreBootstrapSealResponse, PreBootstrapState, SecretListEntry,
};
pub use storage::Migration;
// CID:afa-contracts-lib-013 re-exports - kernel
// Flatten the `KernelMode` enum + helpers to the
// crate root so callers can write
// `use afa_contracts::KernelMode;` instead of
// `use afa_contracts::kernel::KernelMode;`.
pub use kernel::KernelMode;
pub use versioning_example::{ExampleThingImpl, ExampleThingV1, ExampleThingV2};
