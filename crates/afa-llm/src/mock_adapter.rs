//! Code Map: MockAdapter
//! - `MockAdapter`: A fake LLM engine that returns canned
//!   responses based on the request's `system` prompt. No
//!   network, no API key, no async-openai — every method
//!   resolves in microseconds. The conformance suite
//!   runs against this adapter for hermetic, fast tests.
//! - `MockAdapter::request_for_text_reply` /
//!   `request_for_tool_call`: Tiny builders that construct
//!   a `CompletionRequest` whose `system` prompt encodes
//!   which canned response the mock should return. The
//!   mock reads the prompt and picks the response. This
//!   keeps the test code free of "magic id" plumbing.
//!
//! Story (plain English): The `MockAdapter` is the
//! practice-room specialist. It does not talk to a real
//! service; it just looks at the customer's "system
//! prompt" (a hint the workflow uses to tell the model
//! how to behave) and hands back a canned envelope.
//! The test cases in the conformance suite each set
//! the system prompt to a secret phrase like
//! "conformance:text_reply_basic" or
//! "conformance:rate_limited" — the mock reads the
//! phrase and returns the matching canned response.
//! A real adapter reads no such phrase; it talks to
//! OpenAI instead.
//!
//! CID Index:
//! CID:afa-llm-mock-001 -> MockAdapter
//! CID:afa-llm-mock-002 -> request builders
//!
//! Quick lookup: rg -n "CID:afa-llm-mock-" crates/afa-llm/src/mock_adapter.rs

use afa_bus::EventBusHandle;
use afa_contracts::{
    CompletionChunk, CompletionCompleted, CompletionRequest, CompletionResponse, CompletionStream,
    ContentBlock, ConversationItem, ExecutionContext, LlmErrorV1, LlmV1, ModelCapabilities,
    ToolCallRequest, ToolDefinition, Usage,
};
use async_trait::async_trait;
use std::sync::OnceLock;

// CID:afa-llm-mock-001 - MockAdapter
// Purpose: A fake LLM engine for the conformance
// suite. The mock's "routing" is a simple match on
// the request's `system` prompt: if the prompt
// starts with `"conformance:"`, the mock returns
// the canned response for the rest of the string.
// A request that does not start with `"conformance:`
// returns `Err(MalformedResponse)` (the mock is for
// tests, not for production).
// Uses: LlmV1 (the trait it implements),
// CompletionRequest, CompletionResponse,
// CompletionStream, ModelCapabilities, LlmErrorV1.
// Used by: `run_conformance_suite` and any
// downstream test that wants a `&dyn LlmV1`
// without bringing up a real adapter.
#[derive(Debug, Default, Clone)]
pub struct MockAdapter {
    /// Optional event-bus handle. When `Some`,
    /// the `complete` method publishes a
    /// `CompletionRequested` event before
    /// dispatching the canned response and a
    /// `CompletionCompleted` event after the
    /// response (or a `CompletionFailed` event
    /// on error). When `None` (the default
    /// `MockAdapter::new()`), no events are
    /// published — the conformance suite
    /// deliberately avoids the bus so it
    /// stays hermetic. The bus-aware variant
    /// is built via `MockAdapter::with_event_bus`
    /// and is used by the multi-turn integration
    /// test (`tests/multi_turn.rs`).
    bus: Option<EventBusHandle>,
    /// The last `Usage` the mock returned,
    /// cached so the bus-aware `complete`
    /// method can stamp a realistic
    /// `prompt_tokens_estimate` on the next
    /// call's `CompletionRequested` event
    /// (the real adapters use the same
    /// `OnceLock<Usage>` pattern: a per-
    /// adapter-instance cache of the last
    /// `Usage`, the audit-event estimate is
    /// `None` on the first call, then the
    /// cached value on subsequent calls).
    /// Only meaningful when `bus.is_some()`
    /// (the conformance suite never reads
    /// it).
    last_usage: OnceLock<Usage>,
}

impl MockAdapter {
    /// Build a fresh `MockAdapter`. Cheap; the
    /// struct holds no state. The
    /// conformance-suite form: no bus, no
    /// audit events, fully hermetic.
    pub fn new() -> Self {
        Self {
            bus: None,
            last_usage: OnceLock::new(),
        }
    }

    /// Build a fresh `MockAdapter` that
    /// publishes `CompletionRequested` /
    /// `CompletionCompleted` /
    /// `CompletionFailed` audit events on
    /// the given bus for every `complete`
    /// call. The canned-response behavior is
    /// identical to `MockAdapter::new()`; the
    /// only difference is the side-effect on
    /// the bus. Used by the multi-turn
    /// integration test
    /// (`tests/multi_turn.rs`) to verify that
    /// 3 sequential `complete()` calls publish
    /// 3 matching pairs of events with
    /// distinct correlation ids, and that
    /// the kernel/engine itself carries no
    /// per-tenant state between calls.
    pub fn with_event_bus(bus: EventBusHandle) -> Self {
        Self {
            bus: Some(bus),
            last_usage: OnceLock::new(),
        }
    }

    /// A canned `CompletionRequest` whose
    /// `system` prompt asks for a text reply.
    /// The mock returns `CompletionResponse::
    /// TextReply { content: "Hello, world!",
    /// usage: positive }`.
    pub fn request_for_text_reply(extra: &str) -> CompletionRequest {
        CompletionRequest {
            system: Some(format!("conformance:text_reply_basic:{extra}")),
            messages: vec![ConversationItem::UserMessage {
                content: vec![ContentBlock::Text("hi".into())],
            }],
            tools: vec![],
            sampling: Default::default(),
        }
    }

    /// A canned `CompletionRequest` whose
    /// `system` prompt asks for a single tool
    /// call to `"search_listings"`. The mock
    /// returns `CompletionResponse::ToolCalls`
    /// with one `ToolCallRequest`.
    pub fn request_for_tool_call(extra: &str) -> CompletionRequest {
        CompletionRequest {
            system: Some(format!("conformance:tool_call_basic:{extra}")),
            messages: vec![ConversationItem::UserMessage {
                content: vec![ContentBlock::Text("search".into())],
            }],
            tools: vec![ToolDefinition {
                name: "search_listings".into(),
                description: "search the catalog".into(),
                parameters_schema: serde_json::json!({
                    "type": "object",
                    "properties": {"query": {"type": "string"}}
                }),
            }],
            sampling: Default::default(),
        }
    }

    /// Build a canned `CompletionRequest` whose
    /// `system` prompt asks for a specific
    /// canned-failure case. The suite uses
    /// `request_for_text_reply("rate_limited")`
    /// etc. and the mock's match-on-prompt does
    /// the rest.
    pub fn request_for_failure(case: FailureCase) -> CompletionRequest {
        let tag = match case {
            FailureCase::RateLimited => "conformance:rate_limited",
            FailureCase::ContextTooLong => "conformance:context_too_long",
            FailureCase::ModelNotFound => "conformance:model_not_found",
            FailureCase::ToolNotFound => "conformance:tool_not_found",
            FailureCase::MalformedResponse => "conformance:malformed_response",
            FailureCase::QuotaExhausted => "conformance:quota_exhausted",
        };
        CompletionRequest {
            system: Some(tag.into()),
            messages: vec![ConversationItem::UserMessage {
                content: vec![ContentBlock::Text("x".into())],
            }],
            tools: vec![],
            sampling: Default::default(),
        }
    }

    /// Read the `system` prompt and return the
    /// canned response. The `&str` is the
    /// prompt; the `&[ToolDefinition]` is the
    /// tools (the tool-call case checks the
    /// first tool's name).
    fn dispatch(
        system: Option<&str>,
        tools: &[ToolDefinition],
    ) -> Result<CompletionResponse, LlmErrorV1> {
        let Some(s) = system else {
            return Err(LlmErrorV1::MalformedResponse {
                reason: "no system prompt".into(),
            });
        };
        let tag = s
            .strip_prefix("conformance:")
            .unwrap_or(s)
            .split(':')
            .next()
            .unwrap_or("");
        match tag {
            "text_reply_basic" => Ok(CompletionResponse::TextReply {
                content: "Hello, world!".into(),
                usage: Usage {
                    prompt_tokens: 5,
                    completion_tokens: 5,
                },
            }),
            "tool_call_basic" => {
                if tools.is_empty() {
                    return Err(LlmErrorV1::ToolNotFound {
                        tool_name: "<no tools>".into(),
                    });
                }
                Ok(CompletionResponse::ToolCalls {
                    calls: vec![ToolCallRequest {
                        id: "call_mock_1".into(),
                        name: tools[0].name.clone(),
                        arguments: serde_json::json!({"query": "Warsaw"}),
                    }],
                    usage: Usage {
                        prompt_tokens: 20,
                        completion_tokens: 8,
                    },
                })
            }
            "rate_limited" => Err(LlmErrorV1::RateLimited {
                retry_after: Some(std::time::Duration::from_secs(2)),
            }),
            "context_too_long" => Err(LlmErrorV1::ContextLengthExceeded {
                actual_tokens: 200_010,
                max_tokens: 200_000,
            }),
            "model_not_found" => Err(LlmErrorV1::ModelNotFound {
                model: "gpt-99".into(),
            }),
            "tool_not_found" => Err(LlmErrorV1::ToolNotFound {
                tool_name: "search_galaxy".into(),
            }),
            "malformed_response" => Err(LlmErrorV1::MalformedResponse {
                reason: "garbled JSON from the vendor".into(),
            }),
            "quota_exhausted" => Err(LlmErrorV1::QuotaExhausted {
                reason: "billing cycle ended".into(),
            }),
            other => Err(LlmErrorV1::MalformedResponse {
                reason: format!("unknown mock tag: {other}"),
            }),
        }
    }
}

/// The 6 canned-failure cases the mock knows
/// about. Each maps to one of the 13 `LlmErrorV1`
/// variants. The conformance suite uses these so
/// the failure-path tests can use the same
/// match-on-prompt pattern as the success-path
/// tests.
#[derive(Debug, Clone, Copy)]
pub enum FailureCase {
    RateLimited,
    ContextTooLong,
    ModelNotFound,
    ToolNotFound,
    MalformedResponse,
    QuotaExhausted,
}

#[async_trait]
impl LlmV1 for MockAdapter {
    async fn complete(
        &self,
        request: CompletionRequest,
        ctx: &ExecutionContext,
    ) -> Result<CompletionResponse, LlmErrorV1> {
        // The bus-aware variant (built via
        // `MockAdapter::with_event_bus`) publishes
        // the three audit events for the
        // multi-turn test. The bus-less variant
        // (the default `MockAdapter::new()`) skips
        // the publishing entirely so the
        // conformance suite stays hermetic.
        // We publish `CompletionRequested` BEFORE
        // dispatch (the audit story: "the
        // adapter is about to do work"), then
        // either `CompletionCompleted` (success) or
        // `CompletionFailed` (error) AFTER. The
        // `prompt_tokens_estimate` is left as
        // `None` (the mock has no prior Usage to
        // draw from — that field exists for
        // real adapters that cache their last
        // `Usage` for richer audit estimates).
        if let Some(bus) = &self.bus {
            use chrono::Utc;
            // The audit-estimate pattern:
            // first call has no prior Usage
            // (None); subsequent calls use the
            // cached `last_usage` from the
            // previous call's response. Same
            // shape the real adapters use
            // (their `OnceLock<Usage>` is
            // populated by the response, then
            // read on the next call's
            // `CompletionRequested`).
            let prompt_tokens_estimate = self.last_usage.get().map(|u| u.prompt_tokens);
            bus.publish(
                afa_contracts::CompletionRequested {
                    correlation_id: ctx.correlation_id,
                    tenant_id: ctx.tenant_id.clone(),
                    actor: ctx.actor.clone(),
                    model: "mock".into(),
                    prompt_tokens_estimate,
                    has_tools: !request.tools.is_empty(),
                    has_images: request.messages.iter().any(|m| match m {
                        ConversationItem::UserMessage { content } => content
                            .iter()
                            .any(|b| matches!(b, ContentBlock::Image { .. })),
                        _ => false,
                    }),
                    timestamp: Utc::now(),
                },
                ctx.clone(),
            )
            .await;
        }
        let started = std::time::Instant::now();
        let result = Self::dispatch(request.system.as_deref(), &request.tools);
        if let Some(bus) = &self.bus {
            use chrono::Utc;
            let duration_ms = started.elapsed().as_millis() as u64;
            match &result {
                Ok(CompletionResponse::TextReply { usage, .. })
                | Ok(CompletionResponse::ToolCalls { usage, .. }) => {
                    // Cache the Usage for the next
                    // call's audit estimate. The
                    // `OnceLock` is "set once" —
                    // the FIRST successful call's
                    // Usage sticks for the rest of
                    // the adapter's life (the real
                    // adapters use the same
                    // once-set pattern; see
                    // `ResponsesAdapter` for the
                    // exact shape).
                    let _ = self.last_usage.set(usage.clone());
                    bus.publish(
                        CompletionCompleted {
                            correlation_id: ctx.correlation_id,
                            tenant_id: ctx.tenant_id.clone(),
                            actor: ctx.actor.clone(),
                            model: "mock".into(),
                            prompt_tokens: usage.prompt_tokens,
                            completion_tokens: usage.completion_tokens,
                            finish_reason: afa_contracts::FinishReason::Stop,
                            duration_ms,
                            timestamp: Utc::now(),
                        },
                        ctx.clone(),
                    )
                    .await;
                }
                Err(e) => {
                    bus.publish(
                        afa_contracts::CompletionFailed {
                            correlation_id: ctx.correlation_id,
                            tenant_id: ctx.tenant_id.clone(),
                            actor: ctx.actor.clone(),
                            model: "mock".into(),
                            error: e.clone(),
                            duration_ms,
                            timestamp: Utc::now(),
                        },
                        ctx.clone(),
                    )
                    .await;
                }
            }
        }
        result
    }

    async fn stream_complete(
        &self,
        request: CompletionRequest,
        _ctx: &ExecutionContext,
    ) -> Result<CompletionStream, LlmErrorV1> {
        // The mock's stream is a 3-chunk reply
        // (`text` + `Finished`) for the success
        // cases, a 4-chunk reply (id+name
        // `ToolCallDelta` + 2-arg `ToolCallDelta`
        // + `Finished { ToolCalls }`) for the
        // tool-call success case, or a 1-chunk
        // reply (`Error(...)`) for the failure
        // cases. The mpsc channel has capacity 4
        // (just enough for the tool-call case;
        // the `send().await` would only block on
        // additional capacity, which the suite
        // never needs).
        let (tx, rx) = tokio::sync::mpsc::channel(4);
        match Self::dispatch(request.system.as_deref(), &request.tools) {
            Ok(CompletionResponse::TextReply { content, usage }) => {
                // For the mock, send the whole
                // text in one `TextDelta` (real
                // adapters would split on the
                // vendor's streaming protocol).
                let _ = tx.send(CompletionChunk::TextDelta(content)).await;
                let _ = tx
                    .send(CompletionChunk::Finished {
                        reason: afa_contracts::FinishReason::Stop,
                        usage,
                    })
                    .await;
            }
            Ok(CompletionResponse::ToolCalls { calls, usage }) => {
                // Phase 3: emit a 3-chunk tool-call
                // stream so the streaming
                // conformance suite can assert on
                // the same shape the real adapters
                // produce:
                //   1. `ToolCallDelta { id, name_delta, "" }`
                //      — the first chunk carries
                //      the call id + the tool's
                //      name.
                //   2. `ToolCallDelta { "", "", arguments_delta }`
                //      — the second chunk carries
                //      the arguments JSON string.
                //   3. `Finished { reason: ToolCalls, usage }`
                //      — the terminal chunk.
                if let Some(first) = calls.first() {
                    let _ = tx
                        .send(CompletionChunk::ToolCallDelta {
                            id: first.id.clone(),
                            name_delta: first.name.clone(),
                            arguments_delta: String::new(),
                        })
                        .await;
                    let _ = tx
                        .send(CompletionChunk::ToolCallDelta {
                            id: String::new(),
                            name_delta: String::new(),
                            arguments_delta: first.arguments.to_string(),
                        })
                        .await;
                }
                let _ = tx
                    .send(CompletionChunk::Finished {
                        reason: afa_contracts::FinishReason::ToolCalls,
                        usage,
                    })
                    .await;
            }
            Err(e) => {
                let _ = tx.send(CompletionChunk::Error(e)).await;
            }
        }
        // The sender is dropped here, which
        // closes the channel and signals `None`
        // to the consumer.
        Ok(rx)
    }

    fn describe_capabilities(&self) -> ModelCapabilities {
        ModelCapabilities {
            max_context_tokens: 200_000,
            supports_vision: true,
            supports_tool_use: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use afa_contracts::ExecutionContext;

    fn ctx() -> ExecutionContext {
        ExecutionContext::new(
            afa_contracts::TenantId::new("t"),
            afa_contracts::Actor::Timer,
        )
    }

    #[tokio::test]
    async fn text_reply_dispatch_returns_canned_envelope() {
        // The dispatcher's text-reply branch
        // must return a `TextReply` with the
        // expected content and positive usage.
        let req = MockAdapter::request_for_text_reply("hi");
        let r = MockAdapter::dispatch(req.system.as_deref(), &req.tools);
        match r {
            Ok(CompletionResponse::TextReply { content, usage }) => {
                assert_eq!(content, "Hello, world!");
                assert!(usage.total() > 0);
            }
            _ => panic!("expected TextReply"),
        }
    }

    #[tokio::test]
    async fn tool_call_dispatch_picks_the_first_tool() {
        // The dispatcher's tool-call branch
        // must look at the first tool the
        // request provided and use its name on
        // the returned `ToolCallRequest`.
        let req = MockAdapter::request_for_tool_call("x");
        let r = MockAdapter::dispatch(req.system.as_deref(), &req.tools);
        match r {
            Ok(CompletionResponse::ToolCalls { calls, .. }) => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_listings");
            }
            _ => panic!("expected ToolCalls"),
        }
    }

    #[tokio::test]
    async fn unknown_prompt_returns_malformed_response() {
        // A prompt that does not start with
        // `"conformance:"` is an unknown
        // "tag" to the mock. The mock returns
        // `MalformedResponse` so the test
        // suite can assert on the variant
        // (not on a string).
        let r = MockAdapter::dispatch(Some("hello there"), &[]);
        assert!(matches!(r, Err(LlmErrorV1::MalformedResponse { .. })));
    }

    #[tokio::test]
    async fn stream_complete_emits_text_then_finished() {
        // The mock's stream is a 2-item
        // `mpsc::Receiver<CompletionChunk>`:
        // one `TextDelta` carrying the canned
        // text, then one `Finished` carrying
        // `Stop` and the usage. The test
        // asserts the order and the contents.
        let req = MockAdapter::request_for_text_reply("hi");
        let adapter = MockAdapter::new();
        let mut stream = adapter
            .stream_complete(req, &ctx())
            .await
            .expect("stream should be Ok");
        let mut chunks: Vec<CompletionChunk> = Vec::new();
        while let Some(c) = stream.recv().await {
            chunks.push(c);
        }
        assert_eq!(chunks.len(), 2);
        assert!(matches!(&chunks[0], CompletionChunk::TextDelta(s) if s == "Hello, world!"));
        assert!(
            matches!(&chunks[1], CompletionChunk::Finished { reason, .. } if *reason == afa_contracts::FinishReason::Stop)
        );
    }

    #[tokio::test]
    async fn stream_complete_emits_tool_call_deltas_then_finished() {
        // Phase 3: the mock's tool-call
        // stream is a 3-item
        // `mpsc::Receiver<CompletionChunk>`:
        //   1. `ToolCallDelta { id, name, "" }`
        //      — id + name.
        //   2. `ToolCallDelta { "", "", arguments_delta }`
        //      — the arguments JSON string.
        //   3. `Finished { reason: ToolCalls, usage }`.
        let req = MockAdapter::request_for_tool_call("x");
        let adapter = MockAdapter::new();
        let mut stream = adapter
            .stream_complete(req, &ctx())
            .await
            .expect("stream should be Ok");
        let mut chunks: Vec<CompletionChunk> = Vec::new();
        while let Some(c) = stream.recv().await {
            chunks.push(c);
        }
        assert_eq!(chunks.len(), 3);
        match &chunks[0] {
            CompletionChunk::ToolCallDelta {
                id,
                name_delta,
                arguments_delta,
            } => {
                assert_eq!(id, "call_mock_1");
                assert_eq!(name_delta, "search_listings");
                assert_eq!(arguments_delta, "");
            }
            other => panic!("expected first chunk to be ToolCallDelta(id+name); got {other:?}"),
        }
        match &chunks[1] {
            CompletionChunk::ToolCallDelta {
                id,
                name_delta,
                arguments_delta,
            } => {
                assert_eq!(id, "");
                assert_eq!(name_delta, "");
                assert!(
                    arguments_delta.contains("Warsaw"),
                    "arguments_delta should contain the canned args; got {arguments_delta:?}"
                );
            }
            other => panic!("expected second chunk to be ToolCallDelta(args); got {other:?}"),
        }
        match &chunks[2] {
            CompletionChunk::Finished { reason, .. } => {
                assert_eq!(*reason, afa_contracts::FinishReason::ToolCalls);
            }
            other => panic!("expected third chunk to be Finished {{ ToolCalls }}; got {other:?}"),
        }
    }
}
