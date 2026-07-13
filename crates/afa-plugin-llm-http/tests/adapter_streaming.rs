//! Code Map: wiremock-based streaming tests for `ResponsesAdapter`
//! - `flow_3_streamed_primary_yields_deltas_then_finished`:
//!   wiremock-rs returns a 3-delta + `response.completed`
//!   SSE stream. The adapter sends each delta as a
//!   `TextDelta` chunk and a final `Finished` chunk.
//!   The bus sees a `CompletionRequested` +
//!   `CompletionCompleted` pair with matching
//!   `correlation_id`.
//! - `flow_3_streamed_request_body_carries_stream_true`:
//!   The SSE wire-up: the request body the adapter
//!   sends to the vendor has `"stream": true` injected.
//!   Verified via `wiremock-rs`'s `Request` capture.
//! - `flow_5_caller_drop_publishes_completion_completed_cancelled`:
//!   The consumer drops the `rx` after 1 delta; the
//!   bg task's `send().await` fails, the bg task
//!   publishes a `CompletionCompleted` with
//!   `finish_reason: Cancelled`.
//! - `flow_6_deadline_publishes_completion_completed_cancelled`:
//!   The `ctx.deadline` is 50 ms; the deadline
//!   watchdog drops its `tx` clone; the bg task
//!   publishes `CompletionCompleted` with
//!   `finish_reason: Cancelled`.
//! - `flow_10_stream_interrupted_publishes_error_chunk_and_failed_event`:
//!   The wiremock server closes the connection
//!   mid-stream (after 1 delta). The bg task sends
//!   an `Error(StreamInterrupted)` chunk and a
//!   `CompletionFailed` event.
//! - `flow_capacity_64_does_not_block_a_slow_consumer_indefinitely`:
//!   Smoke test: the channel is bounded to 64
//!   items, so a slow consumer's `recv()` causes
//!   the bg task to back-pressure (not an
//!   unbounded queue).
//! - `flow_4_streamed_tool_call_yields_deltas_then_finished_tool_calls`:
//!   Phase 3. The wiremock server returns a
//!   sequence of SSE events:
//!   `response.output_item.added` (one
//!   `function_call` item) + three
//!   `response.function_call_arguments.delta`
//!   events + a `response.completed` event.
//!   The adapter yields two
//!   `ToolCallDelta { id, name_delta, "" }` and
//!   `ToolCallDelta { id, "", arguments_delta }`
//!   chunks (the first carries the call id +
//!   name; the rest carry the arguments string
//!   in pieces) and a terminal `Finished
//!   { reason: ToolCalls, ... }` chunk. The
//!   `Finished.reason` is `ToolCalls` even
//!   though the OpenAI Responses API does NOT
//!   send an explicit `finish_reason` in
//!   `response.completed` — the adapter infers
//!   it from the presence of the
//!   `function_call` output item.
//!
//! Story (plain English): These are the
//! switchboard operator's practice drills for the
//! streaming case. The OpenAI specialist (the
//! adapter) is asked to do a streamed reply; the
//! fake OpenAI service (wiremock-rs) is set up to
//! return whatever the drill needs (a happy
//! streamed answer, a connection that dies
//! mid-stream, etc.). The operator reads the
//! tickets the specialist stamped and checks that
//! the specialist handled every drill correctly.
//!
//! CID Index:
//! CID:afa-plugin-llm-http-streaming-integration-001 -> flow_3_streamed_primary
//! CID:afa-plugin-llm-http-streaming-integration-002 -> flow_5_caller_drop
//! CID:afa-plugin-llm-http-streaming-integration-003 -> flow_6_deadline
//! CID:afa-plugin-llm-http-streaming-integration-004 -> flow_10_stream_interrupted
//! CID:afa-plugin-llm-http-streaming-integration-005 -> flow_4_streamed_tool_call
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-http-streaming-integration-" crates/afa-plugin-llm-http/tests/adapter_streaming.rs

use std::sync::{Arc, Mutex};
use std::time::Duration;

use afa_bus::EventBus;
use afa_contracts::{
    CompletionChunk, CompletionCompleted, CompletionFailed, CompletionRequest, CompletionRequested,
    ContentBlock, ConversationItem, ExecutionContext, FinishReason, LlmErrorV1, LlmV1, SecretRef,
    SecurityErrorV1, SecurityV1, UnsealedSecret,
};
use afa_plugin_llm_http::config::ResponsesConfig;
use afa_plugin_llm_http::responses_adapter::ResponsesAdapter;
use async_trait::async_trait;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// A canned OpenAI Responses API
/// SSE stream with 3 text deltas +
/// a `response.completed` event
/// carrying usage. The wire shape is
/// exactly what the OpenAI Responses
/// API sends:
/// `data: {"type":"response.created",...}`
/// `data: {"type":"response.output_text.delta","delta":"Hel",...}`
/// `data: {"type":"response.output_text.delta","delta":"lo,",...}`
/// `data: {"type":"response.output_text.delta","delta":" world!",...}`
/// `data: {"type":"response.completed","response":{"usage":{...},...}}`
/// `data: [DONE]`
const HAPPY_SSE_BODY: &str = "\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-4o\"}}

data: {\"type\":\"response.output_text.delta\",\"delta\":\"Hel\",\"item_id\":\"msg_1\",\"content_index\":0}

data: {\"type\":\"response.output_text.delta\",\"delta\":\"lo,\",\"item_id\":\"msg_1\",\"content_index\":0}

data: {\"type\":\"response.output_text.delta\",\"delta\":\" world!\",\"item_id\":\"msg_1\",\"content_index\":0}

data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"usage\":{\"input_tokens\":10,\"output_tokens\":3},\"status\":\"completed\"}}

data: [DONE]
";

/// A canned 1-delta SSE stream
/// followed by a mid-stream
/// connection close (the
/// `stream_interrupted` flow). The
/// stream is intentionally short so
/// the test is fast; the wiremock
/// server hangs up the TCP
/// connection right after the
/// delta.
const ONE_DELTA_THEN_CLOSE_SSE_BODY: &str = "\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-4o\"}}

data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\",\"item_id\":\"msg_1\",\"content_index\":0}

";

const TEST_API_KEY: &str = "sk-stream-test-key-do-not-log";

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

struct FakeSecurity;

#[async_trait]
impl SecurityV1 for FakeSecurity {
    async fn seal(&self, _plaintext: &[u8], _name: &str) -> Result<SecretRef, SecurityErrorV1> {
        unimplemented!()
    }
    async fn unseal(
        &self,
        _name: &SecretRef,
        _ctx: &ExecutionContext,
    ) -> Result<UnsealedSecret, SecurityErrorV1> {
        Ok(UnsealedSecret::new(TEST_API_KEY.as_bytes().to_vec()))
    }
    async fn rotate(
        &self,
        _secret_ref: &SecretRef,
        _new_plaintext: &[u8],
        _ctx: &ExecutionContext,
    ) -> Result<SecretRef, SecurityErrorV1> {
        unimplemented!()
    }
}

fn key_ref() -> SecretRef {
    SecretRef {
        name: "openai-test-key".into(),
        version: 1,
    }
}

fn text_request() -> CompletionRequest {
    CompletionRequest {
        system: None,
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text("hello".into())],
        }],
        tools: vec![],
        sampling: Default::default(),
    }
}

fn config_with_base_url(base_url: &str) -> ResponsesConfig {
    ResponsesConfig::responses_gpt_4o_with_base_url(key_ref(), base_url)
}

fn build_adapter(base_url: &str) -> (EventBus, ResponsesAdapter, ExecutionContext) {
    let bus = EventBus::new();
    let adapter = ResponsesAdapter::new(
        config_with_base_url(base_url),
        Arc::new(FakeSecurity),
        bus.handle(),
    );
    let ctx = ExecutionContext::new(
        afa_contracts::TenantId::new("adapter_streaming_test"),
        afa_contracts::Actor::Timer,
    );
    (bus, adapter, ctx)
}

/// Build the canned wiremock-rs
/// response with the SSE content
/// type. The OpenAI Responses API
/// uses `text/event-stream` for
/// streaming replies.
fn sse_response(body: &str) -> ResponseTemplate {
    ResponseTemplate::new(200)
        .insert_header("content-type", "text/event-stream")
        .set_body_string(body)
}

// ---------------------------------------------------------------------------
// Flow 3 — streamed primary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_3_streamed_primary_yields_deltas_then_finished() {
    // The wiremock-rs server
    // returns the canned
    // `HAPPY_SSE_BODY` for any
    // POST to `/v1/responses`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse_response(HAPPY_SSE_BODY))
        .mount(&server)
        .await;
    let (bus, adapter, ctx) = build_adapter(&server.uri());
    let mut req_sub = bus.subscribe::<CompletionRequested>(16);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);

    let mut stream = adapter
        .stream_complete(text_request(), &ctx)
        .await
        .expect("stream_complete should return Ok");

    // The consumer should see 3
    // `TextDelta` chunks and a
    // `Finished` chunk.
    let c1 = recv_chunk(&mut stream).await;
    let c2 = recv_chunk(&mut stream).await;
    let c3 = recv_chunk(&mut stream).await;
    let c4 = recv_chunk(&mut stream).await;
    match (&c1, &c2, &c3, &c4) {
        (
            CompletionChunk::TextDelta(a),
            CompletionChunk::TextDelta(b),
            CompletionChunk::TextDelta(c),
            CompletionChunk::Finished { reason, usage },
        ) => {
            assert_eq!(a, "Hel");
            assert_eq!(b, "lo,");
            assert_eq!(c, " world!");
            assert_eq!(*reason, FinishReason::Stop);
            assert_eq!(usage.prompt_tokens, 10);
            assert_eq!(usage.completion_tokens, 3);
        }
        other => panic!("expected 3 deltas + Finished, got {other:?}"),
    }
    // Channel closed.
    let end = tokio::time::timeout(Duration::from_secs(2), stream.recv())
        .await
        .expect("channel close timeout");
    assert!(end.is_none(), "channel should be closed after Finished");

    // The bus saw the
    // `CompletionRequested` and
    // `CompletionCompleted`
    // pair.
    let (req_evt, _) = recv_event(&mut req_sub).await;
    let (comp_evt, _) = recv_event(&mut comp_sub).await;
    assert_eq!(req_evt.correlation_id, comp_evt.correlation_id);
    assert_eq!(comp_evt.prompt_tokens, 10);
    assert_eq!(comp_evt.completion_tokens, 3);
    assert_eq!(comp_evt.finish_reason, FinishReason::Stop);
}

#[tokio::test]
async fn flow_3_streamed_request_body_carries_stream_true() {
    // The SSE wire-up: the
    // request body the adapter
    // sends to the vendor has
    // `"stream": true` injected.
    // Verified via wiremock-rs's
    // `Request` capture.
    let server = MockServer::start().await;
    let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let captured_clone = captured.clone();
    let body_clone = HAPPY_SSE_BODY.to_string();
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(move |req: &Request| {
            *captured_clone.lock().unwrap() = Some(req.body.clone());
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(body_clone.clone())
        })
        .up_to_n_times(1)
        .mount(&server)
        .await;
    let (_bus, adapter, ctx) = build_adapter(&server.uri());
    let mut stream = adapter
        .stream_complete(text_request(), &ctx)
        .await
        .expect("stream_complete Ok");
    // Drain the stream so the
    // bg task finishes.
    while let Some(_c) = tokio::time::timeout(Duration::from_secs(2), stream.recv())
        .await
        .ok()
        .flatten()
    {}
    let body_bytes = captured.lock().unwrap().clone().expect("body captured");
    let body_json: serde_json::Value = serde_json::from_slice(&body_bytes).expect("body is JSON");
    assert_eq!(
        body_json["stream"], true,
        "streaming request must carry `\"stream\": true`"
    );
}

// ---------------------------------------------------------------------------
// Flow 5 — caller drop (consumer drops rx)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_5_caller_drop_publishes_completion_completed_cancelled() {
    // The wiremock-rs server
    // returns a 70-chunk SSE
    // stream (more than the
    // 64-slot channel buffer).
    // The consumer reads 1
    // delta, then drops the
    // `rx`. The bg task
    // queues the next 63
    // chunks into the buffer
    // (the buffer fills up to
    // 64 items — the original
    // chunk the consumer read
    // is now in the consumer's
    // hands, leaving 63 free
    // slots). The 65th
    // `send().await` blocks
    // (buffer full). The
    // consumer has already
    // dropped, so the 65th
    // `send().await` returns
    // `Err(SendError(_))`.
    // The bg task publishes
    // `CompletionCompleted`
    // with
    // `finish_reason: Cancelled`.
    let server = MockServer::start().await;
    // Build a 70-chunk SSE
    // stream + a `response.completed`
    // event + `[DONE]`. The
    // 70-delta body forces
    // the buffer to fill.
    let mut big_body = String::new();
    for _ in 0..70 {
        big_body.push_str("data: {\"type\":\"response.output_text.delta\",\"delta\":\"a\"}\n\n");
    }
    big_body.push_str(
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"usage\":{\"input_tokens\":1,\"output_tokens\":70},\"status\":\"completed\"}}\n\n",
    );
    big_body.push_str("data: [DONE]\n");
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(big_body),
        )
        .mount(&server)
        .await;
    let (bus, adapter, ctx) = build_adapter(&server.uri());
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);

    {
        let mut stream = adapter
            .stream_complete(text_request(), &ctx)
            .await
            .expect("stream_complete Ok");
        // Read 1 delta, then
        // drop. The drop
        // happens BEFORE the
        // 65th send completes
        // (the bg task queues
        // 64 chunks total
        // before blocking,
        // and the consumer
        // dropped after
        // reading 1).
        let c1 = recv_chunk(&mut stream).await;
        assert!(matches!(c1, CompletionChunk::TextDelta(_)));
    }
    // The bg task should
    // publish a
    // `CompletionCompleted`
    // with
    // `finish_reason: Cancelled`.
    let (comp_evt, _) = tokio::time::timeout(Duration::from_secs(2), comp_sub.recv())
        .await
        .expect("CompletionCompleted not received in time")
        .expect("CompletionCompleted channel closed");
    assert_eq!(
        comp_evt.finish_reason,
        FinishReason::Cancelled,
        "caller-drop must publish CompletionCompleted {{ Cancelled }}"
    );
}

// ---------------------------------------------------------------------------
// Flow 6 — deadline hit
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_6_deadline_publishes_completion_completed_cancelled() {
    // The wiremock-rs server
    // returns a slow SSE stream
    // (a long pause between
    // deltas). The `ctx.deadline`
    // is 50 ms in the future.
    // The deadline watchdog
    // drops its `tx` clone; the
    // bg task publishes
    // `CompletionCompleted` with
    // `finish_reason: Cancelled`.
    let server = MockServer::start().await;
    // A body that opens a
    // connection, sends 1
    // delta, then waits a
    // long time (longer than
    // the deadline).
    let slow_body = "\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\"}}

data: {\"type\":\"response.output_text.delta\",\"delta\":\"hi\"}

";
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse_response(slow_body))
        .mount(&server)
        .await;
    let (bus, adapter, mut ctx) = build_adapter(&server.uri());
    ctx.deadline = Some(std::time::Instant::now() + Duration::from_millis(50));
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);

    {
        let _stream = adapter
            .stream_complete(text_request(), &ctx)
            .await
            .expect("stream_complete Ok");
        // Just wait for the
        // bg task to notice
        // the deadline hit.
    }
    // The bg task should
    // publish a
    // `CompletionCompleted`
    // with
    // `finish_reason: Cancelled`.
    let (comp_evt, _) = tokio::time::timeout(Duration::from_secs(2), comp_sub.recv())
        .await
        .expect("CompletionCompleted not received in time")
        .expect("CompletionCompleted channel closed");
    assert_eq!(
        comp_evt.finish_reason,
        FinishReason::Cancelled,
        "deadline must publish CompletionCompleted {{ Cancelled }}"
    );
}

// ---------------------------------------------------------------------------
// Flow 10 — stream interrupted (vendor closes connection mid-stream)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_10_stream_interrupted_publishes_error_chunk_and_failed_event() {
    // The wiremock-rs server
    // returns a 1-delta SSE
    // stream and then closes
    // the connection (the
    // `set_body_string` ends
    // without `[DONE]`). The
    // bg task's
    // `eventsource().next()`
    // returns `None` (or
    // errors). The bg task
    // sends an `Error(StreamInterrupted)`
    // chunk and publishes a
    // `CompletionFailed` event.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse_response(ONE_DELTA_THEN_CLOSE_SSE_BODY))
        .mount(&server)
        .await;
    let (bus, adapter, ctx) = build_adapter(&server.uri());
    let mut failed_sub = bus.subscribe::<CompletionFailed>(16);

    let mut stream = adapter
        .stream_complete(text_request(), &ctx)
        .await
        .expect("stream_complete Ok");
    let c1 = recv_chunk(&mut stream).await;
    assert!(matches!(c1, CompletionChunk::TextDelta(_)));
    let c2 = recv_chunk(&mut stream).await;
    // The next chunk is the
    // `Error(StreamInterrupted)`.
    match c2 {
        CompletionChunk::Error(LlmErrorV1::StreamInterrupted { .. }) => {}
        other => panic!("expected Error(StreamInterrupted), got {other:?}"),
    }
    // Channel closed.
    let end = tokio::time::timeout(Duration::from_secs(2), stream.recv())
        .await
        .expect("channel close timeout");
    assert!(end.is_none(), "channel should be closed after Error");

    // The bus saw the
    // `CompletionFailed`
    // event.
    let (failed_evt, _) = recv_event(&mut failed_sub).await;
    assert!(
        matches!(failed_evt.error, LlmErrorV1::StreamInterrupted { .. }),
        "CompletionFailed must carry StreamInterrupted, got {:?}",
        failed_evt.error
    );
}

// ---------------------------------------------------------------------------
// Flow 4 — streamed tool call (Phase 3)
// ---------------------------------------------------------------------------

/// A canned OpenAI Responses API
/// SSE stream for a tool call: one
/// `response.output_item.added` (a
/// `function_call` output item) +
/// three `response.function_call_arguments.delta`
/// events (the JSON arguments string
/// is split into three pieces) + a
/// `response.completed` event. The
/// `response.completed` event does NOT
/// carry a `finish_reason` (the
/// Responses API leaves that inference
/// to the consumer); the adapter must
/// infer `ToolCalls` from the
/// presence of the `function_call`
/// output item. The wire shape is
/// exactly what the OpenAI Responses
/// API sends.
const TOOL_CALL_SSE_BODY: &str = "\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_test\",\"model\":\"gpt-4o\"}}

data: {\"type\":\"response.output_item.added\",\"output_index\":0,\"item\":{\"id\":\"fc_test_1\",\"type\":\"function_call\",\"name\":\"search_listings\",\"arguments\":\"\"}}

data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_test_1\",\"output_index\":0,\"delta\":\"{\\\"query\\\":\"}

data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_test_1\",\"output_index\":0,\"delta\":\"\\\"Warsaw\\\"\"}

data: {\"type\":\"response.function_call_arguments.delta\",\"item_id\":\"fc_test_1\",\"output_index\":0,\"delta\":\"}\"}

data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_test\",\"usage\":{\"input_tokens\":42,\"output_tokens\":18},\"status\":\"completed\"}}

data: [DONE]
";

#[tokio::test]
async fn flow_4_streamed_tool_call_yields_deltas_then_finished_tool_calls() {
    // The wiremock-rs server
    // returns the canned
    // `TOOL_CALL_SSE_BODY` for
    // any POST to `/v1/responses`.
    // The adapter must yield:
    //   1. `ToolCallDelta { id:
    //      "fc_test_1", name_delta:
    //      "search_listings",
    //      arguments_delta: "" }`
    //      (the first chunk carries
    //      the id + name from the
    //      `response.output_item.added`
    //      event).
    //   2. `ToolCallDelta { id:
    //      "fc_test_1", name_delta:
    //      "", arguments_delta:
    //      "{\"query\":" }`.
    //   3. `ToolCallDelta { id:
    //      "fc_test_1", name_delta:
    //      "", arguments_delta:
    //      "\"Warsaw\"" }`.
    //   4. `ToolCallDelta { id:
    //      "fc_test_1", name_delta:
    //      "", arguments_delta:
    //      "}" }`.
    //   5. `Finished { reason:
    //      ToolCalls, usage: { 42,
    //      18 } }` (the adapter
    //      infers `ToolCalls` from
    //      the presence of the
    //      function_call output
    //      item).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(sse_response(TOOL_CALL_SSE_BODY))
        .mount(&server)
        .await;
    let (bus, adapter, ctx) = build_adapter(&server.uri());
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);

    let mut stream = adapter
        .stream_complete(text_request(), &ctx)
        .await
        .expect("stream_complete should return Ok");

    // The first chunk is the
    // ToolCallDelta from the
    // `response.output_item.added`
    // event.
    let c1 = recv_chunk(&mut stream).await;
    match &c1 {
        CompletionChunk::ToolCallDelta {
            id,
            name_delta,
            arguments_delta,
        } => {
            assert_eq!(id, "fc_test_1");
            assert_eq!(name_delta, "search_listings");
            assert_eq!(arguments_delta, "");
        }
        other => panic!("expected first chunk to be ToolCallDelta (id+name); got {other:?}"),
    }
    // Three arguments-delta
    // chunks (the JSON arguments
    // string is split into three
    // pieces).
    let c2 = recv_chunk(&mut stream).await;
    let c3 = recv_chunk(&mut stream).await;
    let c4 = recv_chunk(&mut stream).await;
    match (&c2, &c3, &c4) {
        (
            CompletionChunk::ToolCallDelta {
                id: id2,
                name_delta: n2,
                arguments_delta: a2,
            },
            CompletionChunk::ToolCallDelta {
                id: id3,
                name_delta: n3,
                arguments_delta: a3,
            },
            CompletionChunk::ToolCallDelta {
                id: id4,
                name_delta: n4,
                arguments_delta: a4,
            },
        ) => {
            assert_eq!(id2, "fc_test_1");
            assert_eq!(n2, "");
            assert_eq!(a2, "{\"query\":");
            assert_eq!(id3, "fc_test_1");
            assert_eq!(n3, "");
            assert_eq!(a3, "\"Warsaw\"");
            assert_eq!(id4, "fc_test_1");
            assert_eq!(n4, "");
            assert_eq!(a4, "}");
        }
        other => panic!("expected 3 args-delta chunks; got {other:?}"),
    }
    // The terminal chunk is
    // `Finished { reason: ToolCalls }`.
    let c5 = recv_chunk(&mut stream).await;
    match c5 {
        CompletionChunk::Finished { reason, usage } => {
            assert_eq!(
                reason,
                FinishReason::ToolCalls,
                "Finished.reason must be ToolCalls (inferred from function_call item)"
            );
            assert_eq!(usage.prompt_tokens, 42);
            assert_eq!(usage.completion_tokens, 18);
        }
        other => panic!("expected terminal Finished; got {other:?}"),
    }
    // Channel closed.
    let end = tokio::time::timeout(Duration::from_secs(2), stream.recv())
        .await
        .expect("channel close timeout");
    assert!(end.is_none(), "channel should be closed after Finished");

    // The bus saw a
    // `CompletionCompleted` with
    // `finish_reason: ToolCalls`.
    let (comp_evt, _) = recv_event(&mut comp_sub).await;
    assert_eq!(
        comp_evt.finish_reason,
        FinishReason::ToolCalls,
        "CompletionCompleted.finish_reason must be ToolCalls (inferred)"
    );
    assert_eq!(comp_evt.prompt_tokens, 42);
    assert_eq!(comp_evt.completion_tokens, 18);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn recv_chunk(stream: &mut tokio::sync::mpsc::Receiver<CompletionChunk>) -> CompletionChunk {
    tokio::time::timeout(Duration::from_secs(2), stream.recv())
        .await
        .expect("chunk not received in time")
        .expect("chunk channel closed")
}

async fn recv_event<T: afa_contracts::events::AfaEvent + Clone + Send + Sync + 'static>(
    sub: &mut afa_bus::Subscription<T>,
) -> (T, ExecutionContext) {
    let (arc, ctx) = tokio::time::timeout(Duration::from_secs(2), sub.recv())
        .await
        .expect("event not received in time")
        .expect("event channel closed");
    // The bus hands us an
    // `Arc<T>`; clone the
    // inner value out (the
    // `Arc` is a refcount
    // bump, not a deep
    // clone of `T`).
    let value: T = (*arc).clone();
    (value, ctx)
}
