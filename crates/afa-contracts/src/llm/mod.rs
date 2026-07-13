//! Code Map: LLM Engine contract surface
//! - `LlmV1`: The "I am an LLM engine" badge. The locked v1
//!   trait every adapter implements. See `mod.rs`.
//! - `CompletionRequest`, `ConversationItem`, `ContentBlock`,
//!   `ToolDefinition`, `SamplingParams`: The shape of a
//!   request. See `request.rs`.
//! - `CompletionResponse`, `ToolCallRequest`, `Usage`: The
//!   shape of a non-streaming reply. See `response.rs`.
//! - `CompletionChunk`, `CompletionStream`, `FinishReason`:
//!   The shape of a streaming reply. See `stream.rs`.
//! - `ModelCapabilities`: The "what can this model do?"
//!   description. See `capabilities.rs`.
//! - `LlmErrorV1`, `LlmErrorKind`: The 13 typed "what went
//!   wrong?" buckets and their mapping to the 6 coarse
//!   `AfaErrorKind`s. See `error.rs`.
//! - `CompletionRequested`, `CompletionCompleted`,
//!   `CompletionFailed`: The three audit facts the adapter
//!   publishes on the event bus. See `events.rs`.
//!
//! Story (plain English): Imagine the operator of a switchboard
//! that connects a customer to one of several specialists. The
//! operator does not know which specialist is best for the
//! question — that is the customer's choice ("give me a model
//! that supports vision"). The switchboard hands the customer a
//! little card (`CompletionRequest`) that lists what they want
//! to ask. The specialist (one of several adapters — an OpenAI
//! one, a future Claude one, a local one) takes the card,
//! talks to the right service, and hands back either a single
//! sealed envelope (`CompletionResponse::TextReply` or
//! `ToolCalls`) or a stream of envelopes (`CompletionStream` of
//! `CompletionChunk`s). The switchboard stamps three little
//! tickets (`CompletionRequested`, `CompletionCompleted`,
//! `CompletionFailed`) on the log every time, so an audit
//! reader can later reconstruct "who asked for what, and did it
//! work?" — without ever reading the contents of the envelopes
//! themselves.
//!
//! This file is just the contract — the dictionary the
//! engine promises to honour. The actual switchboard and
//! specialists are in the `afa-llm` and `afa-plugin-llm-http`
//! crates. The dictionary is the only thing in `afa-contracts`,
//! because the dictionary is small, never does I/O, and is the
//! same for every deployment (a real adapter, a test fixture,
//! and a future Claude adapter all use exactly the same
//! words).
//!
//! CID Index:
//! CID:llm-001 -> LlmV1
//! CID:llm-002 -> CompletionRequest
//! CID:llm-003 -> CompletionResponse
//! CID:llm-004 -> CompletionChunk
//! CID:llm-005 -> ModelCapabilities
//! CID:llm-006 -> LlmErrorV1
//! CID:llm-007 -> CompletionRequested/Completed/Failed events
//!
//! Quick lookup: rg -n "CID:llm-" crates/afa-contracts/src/llm/

pub mod capabilities;
pub mod error;
pub mod events;
pub mod request;
pub mod response;
pub mod stream;

use async_trait::async_trait;

use crate::execution_context::ExecutionContext;
pub use capabilities::ModelCapabilities;
pub use error::{LlmErrorKind, LlmErrorV1};
pub use events::{CompletionCompleted, CompletionFailed, CompletionRequested};
pub use request::{
    CompletionRequest, ContentBlock, ConversationItem, ImageData, SamplingParams, ToolDefinition,
};
pub use response::{CompletionResponse, ToolCallRequest, Usage};
pub use stream::{CompletionChunk, CompletionStream, FinishReason};

// CID:llm-001 - LlmV1
// Purpose: The locked v1 contract for any LLM engine adapter.
// Three methods: `complete` (buffered, returns a single
// `CompletionResponse`), `stream_complete` (returns a
// `CompletionStream` of `CompletionChunk`s), and
// `describe_capabilities` (synchronous, returns the static
// `ModelCapabilities` of the underlying model). All async
// methods are `#[async_trait]`-decorated so adapters can be
// held behind `Arc<dyn LlmV1>` in the CapabilityRegistry.
// The `Send + Sync` supertrait lets the registry share one
// adapter across many concurrent callers.
// Uses: CompletionRequest, CompletionResponse,
// CompletionStream, ExecutionContext, LlmErrorV1.
// Used by: the CapabilityRegistry (holds `Arc<dyn LlmV1>`),
// every workflow that calls `llm.complete(...)` or
// `llm.stream_complete(...)`, and the conformance suite in
// `afa-llm`.
#[async_trait]
pub trait LlmV1: Send + Sync {
    /// Run a single non-streaming completion. Returns the
    /// model's reply (text or tool calls) on success, or one
    /// of the 13 typed `LlmErrorV1` variants on failure.
    /// Publishes `CompletionRequested` before the vendor
    /// call, then either `CompletionCompleted` (with `Usage`
    /// and `finish_reason`) on success or `CompletionFailed`
    /// (with the typed error) on failure.
    async fn complete(
        &self,
        request: CompletionRequest,
        ctx: &ExecutionContext,
    ) -> Result<CompletionResponse, LlmErrorV1>;

    /// Open a streaming completion. The returned
    /// `CompletionStream` (a bounded
    /// `tokio::sync::mpsc::Receiver<CompletionChunk>` with
    /// capacity 64) yields `CompletionChunk::TextDelta`,
    /// `CompletionChunk::ToolCallDelta`, and finally one
    /// `CompletionChunk::Finished` (or `CompletionChunk::Error`
    /// on mid-stream vendor death). Publishes
    /// `CompletionRequested` first, then either
    /// `CompletionCompleted` on normal end or
    /// `CompletionFailed` on error.
    async fn stream_complete(
        &self,
        request: CompletionRequest,
        ctx: &ExecutionContext,
    ) -> Result<CompletionStream, LlmErrorV1>;

    /// Synchronously return the static capabilities of the
    /// underlying model (`max_context_tokens`,
    /// `supports_vision`, `supports_tool_use`). The values
    /// are decided at adapter construction and never change
    /// for the process lifetime. No `async`, no `ctx`, no
    /// I/O.
    fn describe_capabilities(&self) -> ModelCapabilities;
}
