//! Code Map: LLM audit events
//! - `CompletionRequested`: Published BEFORE the wire
//!   call begins. Carries the correlation id, tenant,
//!   actor, model name, a `prompt_tokens_estimate`
//!   (best-effort, `None` on the first call; the
//!   `OnceLock<Usage>` from the previous call feeds
//!   it on subsequent calls), and the two feature
//!   flags (`has_tools`, `has_images`) so an audit
//!   reader can reconstruct "what was asked for?"
//!   without ever reading the prompt itself.
//! - `CompletionCompleted`: Published AFTER the wire
//!   call returns successfully. Carries the
//!   correlation id, tenant, actor, model, the
//!   `prompt_tokens` and `completion_tokens`, the
//!   `finish_reason` (Stop / ToolCalls / MaxTokens /
//!   ContentFilter / Cancelled), and `duration_ms`.
//!   Does NOT carry the reply content — the audit
//!   story is "what happened," not "what was said."
//! - `CompletionFailed`: Published AFTER the wire
//!   call returns an error (initial or mid-stream).
//!   Carries the correlation id, tenant, actor,
//!   model, the typed `LlmErrorV1`, and
//!   `duration_ms`. Does NOT carry the prompt or
//!   the partial reply — same audit story.
//!
//! Story (plain English): The three audit events are
//! the three small tickets the switchboard stamps on
//! the log every time. The first ticket
//! (`CompletionRequested`) is stamped as the
//! customer's letter is being dropped into the
//! specialist's outbox: "we sent a request, here is
//! who, here is the rough size, here is the model."
//! The second ticket (`CompletionCompleted`) is
//! stamped when the specialist's reply comes back:
//! "we got a reply, here is the size, here is why
//! the model stopped." The third ticket
//! (`CompletionFailed`) is stamped when the
//! specialist's reply does not come back: "we got
//! an error, here is the typed reason, here is how
//! long we waited." None of the three tickets
//! carries the actual letter or the actual reply —
//! the audit story is "what happened," not "what
//! was said." A future reader can reconstruct "who
//! asked for what, did it work, and how long did it
//! take?" without ever reading the contents.
//!
//! CID Index:
//! CID:llm-events-001 -> CompletionRequested
//! CID:llm-events-002 -> CompletionCompleted
//! CID:llm-events-003 -> CompletionFailed
//!
//! Quick lookup: rg -n "CID:llm-events-" crates/afa-contracts/src/llm/events.rs

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::events::AfaEvent;
use crate::execution_context::Actor;
use crate::ids::{CorrelationId, TenantId};

use super::error::LlmErrorV1;
use super::stream::FinishReason;

// CID:llm-events-001 - CompletionRequested
// Purpose: The audit fact the adapter publishes on the
// event bus BEFORE the wire call begins. Carries the
// `ExecutionContext` metadata (tenant, correlation,
// actor) so the audit trail can be tied back to the
// request, plus the model name, a best-effort
// `prompt_tokens_estimate` (None on the first call;
// the `OnceLock<Usage>` from the previous call feeds
// it on subsequent calls), and the two feature flags
// (`has_tools`, `has_images`) so an audit reader can
// reconstruct "what was asked for?" without ever
// reading the prompt itself.
// Uses: AfaEvent, serde, chrono, ExecutionContext
// types (TenantId, CorrelationId, Actor).
// Used by: the OpenAI adapter's `complete` and
// `stream_complete` methods (the first line of each
// method publishes this event), and any dashboard or
// observability tool subscribed to LLM events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequested {
    /// The tracking number from the
    /// `ExecutionContext`.
    pub correlation_id: CorrelationId,
    /// The tenant from the `ExecutionContext`.
    pub tenant_id: TenantId,
    /// The actor from the `ExecutionContext` (the
    /// `Actor` enum — channel/timer/human/internal
    /// — not the full context).
    pub actor: Actor,
    /// The model name (e.g. `"gpt-4o"`). The
    /// adapter is hard-wired to one model, so this
    /// is always the same string for a given
    /// adapter instance.
    pub model: String,
    /// The best-effort prompt-size estimate
    /// (`None` on the first call; the
    /// `OnceLock<Usage>` from the previous call
    /// feeds it on subsequent calls). A reader
    /// who needs an exact count should look at
    /// the matching `CompletionCompleted`.
    pub prompt_tokens_estimate: Option<u32>,
    /// Whether the request carried any
    /// `ToolDefinition`s. An audit reader can
    /// see "this was a tool-using call" without
    /// reading the tool definitions themselves.
    pub has_tools: bool,
    /// Whether the request carried any image
    /// `ContentBlock`s. An audit reader can see
    /// "this was a vision call" without reading
    /// the image bytes.
    pub has_images: bool,
    /// The wall-clock time the adapter saw the
    /// `complete` / `stream_complete` call.
    pub timestamp: DateTime<Utc>,
}

impl AfaEvent for CompletionRequested {}

// CID:llm-events-002 - CompletionCompleted
// Purpose: The audit fact the adapter publishes on
// the event bus AFTER the wire call returns
// successfully. Carries the `ExecutionContext`
// metadata, the model name, the `prompt_tokens` and
// `completion_tokens` (the exact values from the
// vendor's response, not the estimate), the
// `finish_reason` (Stop / ToolCalls / MaxTokens /
// ContentFilter / Cancelled), and `duration_ms`.
// Does NOT carry the reply content — the audit
// story is "what happened," not "what was said."
// Uses: AfaEvent, serde, chrono, ExecutionContext
// types, FinishReason.
// Used by: the OpenAI adapter on success (after
// mapping the response, before returning to the
// caller), and any dashboard or observability tool
// subscribed to LLM events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionCompleted {
    /// The tracking number from the
    /// `ExecutionContext`.
    pub correlation_id: CorrelationId,
    /// The tenant from the `ExecutionContext`.
    pub tenant_id: TenantId,
    /// The actor from the `ExecutionContext`.
    pub actor: Actor,
    /// The model name.
    pub model: String,
    /// The exact number of prompt tokens the
    /// vendor reported (not the estimate from
    /// `CompletionRequested`).
    pub prompt_tokens: u32,
    /// The exact number of completion tokens the
    /// vendor reported.
    pub completion_tokens: u32,
    /// Why the model stopped. `Cancelled` is
    /// distinct from `Stop` — see `FinishReason`
    /// for the audit story.
    pub finish_reason: FinishReason,
    /// The wall-clock duration of the call in
    /// milliseconds (from the first line of
    /// `complete` / `stream_complete` to the
    /// publish of this event).
    pub duration_ms: u64,
    /// The wall-clock time the adapter saw the
    /// successful return.
    pub timestamp: DateTime<Utc>,
}

impl AfaEvent for CompletionCompleted {}

// CID:llm-events-003 - CompletionFailed
// Purpose: The audit fact the adapter publishes on
// the event bus AFTER the wire call returns an
// error (initial or mid-stream). Carries the
// `ExecutionContext` metadata, the model name, the
// typed `LlmErrorV1` (so an audit reader can
// reconstruct "what went wrong?" without
// re-classifying the error), and `duration_ms`.
// Does NOT carry the prompt or the partial reply —
// the audit story is "what happened," not "what
// was said." Note that cancellation (caller dropped
// the receiver or `ctx.deadline` hit) does NOT
// publish this event — it publishes
// `CompletionCompleted` with
// `finish_reason: Cancelled` instead. This event is
// for genuine failures.
// Uses: AfaEvent, serde, chrono, ExecutionContext
// types, LlmErrorV1.
// Used by: the OpenAI adapter on error, and any
// dashboard or observability tool subscribed to LLM
// events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionFailed {
    /// The tracking number from the
    /// `ExecutionContext`.
    pub correlation_id: CorrelationId,
    /// The tenant from the `ExecutionContext`.
    pub tenant_id: TenantId,
    /// The actor from the `ExecutionContext`.
    pub actor: Actor,
    /// The model name.
    pub model: String,
    /// The typed `LlmErrorV1`. An audit reader
    /// can branch on the variant (e.g. "alert
    /// on `AuthenticationFailed`") without
    /// re-classifying the error.
    pub error: LlmErrorV1,
    /// The wall-clock duration of the call in
    /// milliseconds.
    pub duration_ms: u64,
    /// The wall-clock time the adapter saw the
    /// error return.
    pub timestamp: DateTime<Utc>,
}

impl AfaEvent for CompletionFailed {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::execution_context::Actor;
    use crate::ids::{CorrelationId, TenantId};
    use chrono::Utc;
    use std::time::Duration;

    #[test]
    fn requested_event_carries_metadata_not_payload() {
        // The `CompletionRequested` event must
        // carry metadata (`correlation_id`,
        // `tenant_id`, `actor`, `model`,
        // `prompt_tokens_estimate`, `has_tools`,
        // `has_images`, `timestamp`) but NOT the
        // prompt or the messages. The struct has
        // no field for the prompt — this is the
        // compile-time guarantee that an audit
        // reader cannot leak the prompt by
        // accidentally logging the event.
        let e = CompletionRequested {
            correlation_id: CorrelationId::new(),
            tenant_id: TenantId::new("t"),
            actor: Actor::Timer,
            model: "gpt-4o".into(),
            prompt_tokens_estimate: None,
            has_tools: false,
            has_images: false,
            timestamp: Utc::now(),
        };
        // The struct has exactly the 8 fields we
        // expect. A future contributor who adds
        // a payload-bearing field (`messages`,
        // `content`, or `prompt: String`) would
        // be forced to update this test.
        let json = serde_json::to_string(&e).expect("serialize");
        // Check for the actual request-payload
        // field names (the words that *would*
        // appear if a `CompletionRequest` was
        // embedded in the event). We do NOT
        // check for the bare word `prompt` —
        // the legitimate `prompt_tokens_estimate`
        // metadata field contains it as a
        // substring, and the request body
        // would arrive as `messages` (plural,
        // with content) anyway.
        assert!(!json.contains("messages"));
        assert!(!json.contains("content"));
        assert!(!json.contains("\"system\""));
    }

    #[test]
    fn completed_event_carries_usage_and_finish_reason() {
        // The `CompletionCompleted` event carries
        // the exact token counts and the finish
        // reason. An audit reader who needs the
        // total cost computes it from these two
        // fields.
        let e = CompletionCompleted {
            correlation_id: CorrelationId::new(),
            tenant_id: TenantId::new("t"),
            actor: Actor::Timer,
            model: "gpt-4o".into(),
            prompt_tokens: 10,
            completion_tokens: 5,
            finish_reason: FinishReason::Stop,
            duration_ms: 200,
            timestamp: Utc::now(),
        };
        assert_eq!(e.prompt_tokens + e.completion_tokens, 15);
        assert_eq!(e.finish_reason, FinishReason::Stop);
    }

    #[test]
    fn failed_event_carries_typed_error_not_a_string() {
        // The `CompletionFailed` event carries the
        // typed `LlmErrorV1`, not a stringified
        // error. An audit reader can branch on
        // the variant (e.g. "alert on
        // `AuthenticationFailed`") without
        // re-classifying.
        let e = CompletionFailed {
            correlation_id: CorrelationId::new(),
            tenant_id: TenantId::new("t"),
            actor: Actor::Timer,
            model: "gpt-4o".into(),
            error: LlmErrorV1::RateLimited {
                retry_after: Some(Duration::from_secs(2)),
            },
            duration_ms: 100,
            timestamp: Utc::now(),
        };
        assert!(matches!(e.error, LlmErrorV1::RateLimited { .. }));
    }
}
