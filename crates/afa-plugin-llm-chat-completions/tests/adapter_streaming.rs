//! Code Map: wiremock-based streaming tests for `ChatCompletionsAdapter`
//! - `flow_3_streamed_primary_yields_deltas_then_finished`:
//!   wiremock-rs returns a 3-delta + `finish_reason: "stop"`
//!   Chat Completions SSE stream. The adapter sends each
//!   delta as a `TextDelta` chunk and a final `Finished`
//!   chunk. The bus sees a `CompletionRequested` +
//!   `CompletionCompleted` pair.
//! - `flow_3_streamed_request_body_carries_stream_true`:
//!   The request body the adapter sends carries
//!   `"stream": true`.
//! - `flow_5_caller_drop_publishes_completion_completed_cancelled`:
//!   The consumer drops the `rx` after 1 delta. The bg
//!   task publishes `CompletionCompleted` with
//!   `finish_reason: Cancelled`.
//! - `flow_6_deadline_publishes_completion_completed_cancelled`:
//!   The `ctx.deadline` is 50 ms. The deadline watchdog
//!   drops its `tx` clone. The bg task publishes
//!   `CompletionCompleted { Cancelled }`.
//! - `flow_10_stream_interrupted_publishes_error_chunk_and_failed_event`:
//!   The wiremock server closes the connection
//!   mid-stream. The bg task sends an
//!   `Error(StreamInterrupted)` chunk and a
//!   `CompletionFailed` event.
//!
//! Story (plain English): Same as the OpenAI
//! Responses streaming tests — the "Lend Your Voice"
//! specialist (the Chat Completions adapter) is
//! drilled on its streaming cases. The fake vendor
//! (wiremock-rs) is set up to return whatever the
//! drill needs.
//!
//! CID Index:
//! CID:afa-plugin-llm-chat-completions-streaming-integration-001 -> flow_3_streamed_primary
//! CID:afa-plugin-llm-chat-completions-streaming-integration-002 -> flow_5_caller_drop
//! CID:afa-plugin-llm-chat-completions-streaming-integration-003 -> flow_6_deadline
//! CID:afa-plugin-llm-chat-completions-streaming-integration-004 -> flow_10_stream_interrupted
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-chat-completions-streaming-integration-" crates/afa-plugin-llm-chat-completions/tests/adapter_streaming.rs

use std::sync::{Arc, Mutex};
use std::time::Duration;

use afa_bus::EventBus;
use afa_contracts::{
    CompletionChunk, CompletionCompleted, CompletionFailed, CompletionRequest, CompletionRequested,
    ContentBlock, ConversationItem, ExecutionContext, FinishReason, LlmErrorV1, LlmV1, SecretRef,
    SecurityErrorV1, SecurityV1, UnsealedSecret,
};
use afa_plugin_llm_chat_completions::adapter::ChatCompletionsAdapter;
use afa_plugin_llm_chat_completions::config::ChatCompletionsConfig;
use async_trait::async_trait;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// A canned OpenAI Chat
/// Completions API SSE stream
/// with 3 text deltas + a
/// final `finish_reason: "stop"`
/// chunk + `[DONE]` sentinel.
const HAPPY_SSE_BODY: &str = "\
data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}

data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"Hel\"},\"finish_reason\":null}]}

data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo,\"},\"finish_reason\":null}]}

data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\" world!\"},\"finish_reason\":null}]}

data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":10,\"completion_tokens\":3,\"total_tokens\":13}}

data: [DONE]
";

/// A canned 1-delta SSE stream
/// followed by a mid-stream
/// connection close.
const ONE_DELTA_THEN_CLOSE_SSE_BODY: &str = "\
data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hi\"},\"finish_reason\":null}]}

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
        name: "chat-completions-test-key".into(),
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

fn config_with_base_url(base_url: &str) -> ChatCompletionsConfig {
    ChatCompletionsConfig::with_provider(
        "gpt-4o-mini",
        key_ref(),
        afa_contracts::ModelCapabilities {
            max_context_tokens: 128_000,
            supports_vision: false,
            supports_tool_use: true,
        },
        base_url,
        "test-provider",
    )
}

fn build_adapter(base_url: &str) -> (EventBus, ChatCompletionsAdapter, ExecutionContext) {
    let bus = EventBus::new();
    let adapter = ChatCompletionsAdapter::new(
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

// ---------------------------------------------------------------------------
// Flow 3 — streamed primary
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_3_streamed_primary_yields_deltas_then_finished() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(HAPPY_SSE_BODY),
        )
        .mount(&server)
        .await;
    let (bus, adapter, ctx) = build_adapter(&server.uri());
    let mut req_sub = bus.subscribe::<CompletionRequested>(16);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);

    let mut stream = adapter
        .stream_complete(text_request(), &ctx)
        .await
        .expect("stream_complete should return Ok");

    // The first chunk in
    // Chat Completions is the
    // role-establishing chunk
    // (delta: {role: "assistant",
    // content: ""}); we skip
    // that one and read the 3
    // text deltas + the
    // terminal.
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
    let server = MockServer::start().await;
    let captured: Arc<Mutex<Option<Vec<u8>>>> = Arc::new(Mutex::new(None));
    let captured_clone = captured.clone();
    let body_clone = HAPPY_SSE_BODY.to_string();
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
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
// Flow 5 — caller drop
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_5_caller_drop_publishes_completion_completed_cancelled() {
    // 70-chunk stream to
    // force the buffer to
    // fill and the bg task
    // to block on the 65th
    // send.
    let server = MockServer::start().await;
    let mut big_body = String::new();
    big_body.push_str("data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"\"},\"finish_reason\":null}]}\n\n");
    for _ in 0..70 {
        big_body.push_str("data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"a\"},\"finish_reason\":null}]}\n\n");
    }
    big_body.push_str("data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n");
    big_body.push_str("data: [DONE]\n");
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
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
        let c1 = recv_chunk(&mut stream).await;
        assert!(matches!(c1, CompletionChunk::TextDelta(_)));
    }
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
// Flow 6 — deadline
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_6_deadline_publishes_completion_completed_cancelled() {
    let server = MockServer::start().await;
    let slow_body = "\
data: {\"id\":\"chatcmpl-test\",\"object\":\"chat.completion.chunk\",\"created\":1700000000,\"model\":\"gpt-4o-mini\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"hi\"},\"finish_reason\":null}]}

";
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(slow_body),
        )
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
    }
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
// Flow 10 — stream interrupted
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_10_stream_interrupted_publishes_error_chunk_and_failed_event() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/chat/completions"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(ONE_DELTA_THEN_CLOSE_SSE_BODY),
        )
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
    match c2 {
        CompletionChunk::Error(LlmErrorV1::StreamInterrupted { .. }) => {}
        other => panic!("expected Error(StreamInterrupted), got {other:?}"),
    }
    let end = tokio::time::timeout(Duration::from_secs(2), stream.recv())
        .await
        .expect("channel close timeout");
    assert!(end.is_none(), "channel should be closed after Error");

    let (failed_evt, _) = recv_event(&mut failed_sub).await;
    assert!(
        matches!(failed_evt.error, LlmErrorV1::StreamInterrupted { .. }),
        "CompletionFailed must carry StreamInterrupted, got {:?}",
        failed_evt.error
    );
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
    let value: T = (*arc).clone();
    (value, ctx)
}
