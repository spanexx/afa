//! Code Map: Phase 4 end-to-end integration test
//!
//! - `kernel_end_to_end_round_trip_buffers_response_and_publishes_audit_pair`:
//!   Boots a real `Kernel` (real `SecurityEngine` over a
//!   real tempdir-backed SQLite file, real `EventBus`,
//!   real `CapabilityRegistry`). Seals a fake API key
//!   via the kernel's own `security()` handle. Builds a
//!   real `ResponsesAdapter` pointed at a wiremock-rs
//!   server that returns a canned OpenAI Responses
//!   success body. Registers the adapter via
//!   `kernel.register_llm(...)`. Subscribes to
//!   `CompletionRequested` and `CompletionCompleted` on
//!   the kernel's bus, calls `kernel.llm().unwrap().complete(...)`,
//!   and asserts the full round-trip: the canned
//!   `TextReply` is returned, the bus saw the two
//!   events with matching `correlation_id`, the
//!   `prompt_tokens` / `completion_tokens` match the
//!   wire, and the `finish_reason` is `Stop`.
//!
//! - `kernel_end_to_end_streaming_chunk_ordering`:
//!   Same shape, but the wiremock-rs server returns a
//!   canned SSE `response.completed` event sequence.
//!   The test calls `kernel.llm().unwrap().stream_complete(...)`
//!   and asserts the chunk ordering matches the locked
//!   streaming contract (`TextDelta` then `Finished`),
//!   and the bus saw the same
//!   Requested + Completed pair with matching
//!   `correlation_id`.
//!
//! - `kernel_register_llm_twice_returns_lm_already_registered`:
//!   Negative test: a second `register_llm` call
//!   surfaces `RegisterError::LlmAlreadyRegistered` (a
//!   closed-set, programmer-error path, not a panic).
//!
//! - `kernel_llm_returns_none_before_registration`:
//!   The pre-registration `kernel.llm()` returns
//!   `None` (a workflow that runs before the
//!   bootstrap can branch on this and surface a
//!   clear "no LLM configured" error).
//!
//! Story (plain English): This is the final
//! integration check the Phase 4 plan asks for. The
//! switchboard (kernel) is up, the security guard
//! is on duty, the API key is sealed in the vault,
//! and the OpenAI Responses-API specialist is
//! standing at the counter with the wiremock-rs
//! server standing in for OpenAI's front desk. A
//! workflow drops off a request; the specialist
//! walks it to the wiremock desk, gets the canned
//! reply, walks it back, stamps the two audit
//! tickets (one before, one after), and hands the
//! reply to the workflow. The test watches every
//! step and asserts the round-trip is whole.
//!
//! CID Index:
//! CID:afa-kernel-llm-integration-001 -> end-to-end buffered
//! CID:afa-kernel-llm-integration-002 -> end-to-end streaming
//! CID:afa-kernel-llm-integration-003 -> register_llm_twice
//! CID:afa-kernel-llm-integration-004 -> llm_none_before_register
//!
//! Quick lookup: rg -n "CID:afa-kernel-llm-integration-" crates/afa-kernel/tests/llm_integration.rs

use std::sync::Arc;
use std::time::Duration;

use afa_bus::EventBus;
use afa_contracts::{
    CompletionChunk, CompletionCompleted, CompletionRequest, CompletionRequested,
    CompletionResponse, ContentBlock, ConversationItem, ExecutionContext, FinishReason, LlmV1,
    SecretRef,
};
use afa_kernel::capability_registry::RegisterError;
use afa_kernel::Kernel;
use afa_plugin_llm_http::config::ResponsesConfig;
use afa_plugin_llm_http::responses_adapter::ResponsesAdapter;
use afa_security::MasterKey;
use tempfile::TempDir;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// A canned OpenAI Responses API success body.
/// The `output[*].type == "message"` shape is what
/// `ResponsesAdapter::map_response` looks for; the
/// `usage.input_tokens` / `output_tokens` is what
/// `parse_usage` looks for; the `output[0].stop_reason`
/// is what `parse_finish_reason` looks for. The
/// `prompt_tokens=12` / `completion_tokens=7` are
/// the values the test asserts on, so a future
/// change to the canned body is a deliberate
/// contract change (the test will catch it).
const TEXT_REPLY_BODY: &str = r#"{
    "id": "resp_kernel_e2e_1",
    "output": [
        {
            "type": "message",
            "content": [
                {"type": "output_text", "text": "kernel says hi"}
            ],
            "stop_reason": "stop"
        }
    ],
    "usage": {
        "input_tokens": 12,
        "output_tokens": 7
    },
    "status": "completed"
}"#;

/// A canned SSE stream the wiremock-rs
/// server returns for the streaming
/// end-to-end test. The shape is one
/// `response.output_text.delta` event
/// carrying the canned text, followed
/// by one `response.completed` event
/// with usage. The `data:` payload
/// (not the `event:` header) carries
/// the event type — the
/// `eventsource-stream` decoder feeds
/// each `data: {...}` JSON line into
/// the adapter's
/// `map_responses_sse_event` parser.
/// Mirrors the `HAPPY_SSE_BODY`
/// fixture in
/// `afa-plugin-llm-http/tests/adapter_streaming.rs`.
const STREAMED_SSE_BODY: &str = "\
data: {\"type\":\"response.created\",\"response\":{\"id\":\"resp_kernel_stream_1\",\"model\":\"gpt-4o\"}}

data: {\"type\":\"response.output_text.delta\",\"delta\":\"hello from stream\",\"item_id\":\"msg_1\",\"content_index\":0}

data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_kernel_stream_1\",\"usage\":{\"input_tokens\":15,\"output_tokens\":3},\"status\":\"completed\"}}

data: [DONE]
";

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a fresh `Kernel` (real `SecurityEngine`
/// over a real tempdir-backed SQLite file) plus the
/// `TempDir` that owns the SQLite path. The
/// `TempDir` is returned so the test can keep the
/// path alive for the test's entire scope (dropping
/// the `TempDir` would delete the file, which would
/// race with the engine's open connection on slow
/// filesystems).
async fn fresh_kernel() -> (TempDir, Kernel) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secrets.db");
    let key = MasterKey::from([0x42u8; 32]);
    let kernel = Kernel::new(&key, path).await.expect("kernel::new");
    (dir, kernel)
}

/// Seal `plaintext` under `name` on the kernel's
/// `security()` engine, returning the
/// `SecretRef` the adapter will use. The
/// `ExecutionContext` is the audit context
/// (tenant + correlation) for the seal.
async fn seal_key(kernel: &Kernel, plaintext: &[u8], name: &str) -> SecretRef {
    kernel
        .security()
        .seal(plaintext, name)
        .await
        .expect("seal should succeed on a fresh engine")
}

/// Build a `ResponsesConfig` for `gpt-4o` pointed
/// at the given `wiremock` base URL. The
/// `SecretRef` is the one the security engine
/// handed back from `seal_key`.
fn config_for(base_url: &str, key_ref: SecretRef) -> ResponsesConfig {
    ResponsesConfig::responses_gpt_4o_with_base_url(key_ref, base_url)
}

/// Build a fresh `ExecutionContext` for the
/// `kernel_e2e` tenant + the `Timer` actor (the
/// test is not a workflow).
fn ctx() -> ExecutionContext {
    ExecutionContext::new(
        afa_contracts::TenantId::new("kernel_e2e"),
        afa_contracts::Actor::Timer,
    )
}

/// Build a `CompletionRequest` that the canned
/// wiremock responses can answer.
fn text_request() -> CompletionRequest {
    CompletionRequest {
        system: Some("you are a kernel e2e test".into()),
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text("hello, kernel".into())],
        }],
        tools: vec![],
        sampling: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Phase 4 — Kernel + ResponsesAdapter end-to-end (buffered)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kernel_end_to_end_round_trip_buffers_response_and_publishes_audit_pair() {
    // 1. Bring up a real Kernel (real
    //    SecurityEngine, real bus, real
    //    CapabilityRegistry).
    let (_dir, kernel) = fresh_kernel().await;
    // 2. Stand up a wiremock-rs server that
    //    returns the canned `TEXT_REPLY_BODY`
    //    for any POST to `/v1/responses`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TEXT_REPLY_BODY))
        .mount(&server)
        .await;
    // 3. Seal a fake API key on the kernel's
    //    own security engine. The
    //    `ResponsesAdapter` will `unseal` it
    //    on the first request.
    let key_ref = seal_key(&kernel, b"sk-fake-kernel-e2e", "kernel-e2e-key").await;
    // 4. Build a real `ResponsesAdapter`
    //    bound to the wiremock base URL. The
    //    adapter takes the kernel's
    //    `security()` and `event_bus_handle()`
    //    so every `unseal` and every
    //    audit-event publish goes through the
    //    same components a production
    //    bootstrap would use.
    let adapter = ResponsesAdapter::new(
        config_for(&server.uri(), key_ref),
        kernel.security(),
        kernel.event_bus_handle(),
    );
    // 5. Register the adapter with the
    //    kernel. The `Arc<dyn LlmV1>` is
    //    exactly what a production bootstrap
    //    would hand to the kernel.
    let adapter: Arc<dyn LlmV1> = Arc::new(adapter);
    kernel
        .register_llm(adapter.clone())
        .expect("register_llm should succeed on an empty slot");
    // 6. Subscribe to the two audit events
    //    on the kernel's bus. The bus is
    //    shared with the adapter (same
    //    `Arc<EventBusCore>`), so the events
    //    the adapter publishes land in our
    //    subscriptions.
    let bus: Arc<EventBus> = kernel.event_bus();
    let mut req_sub = bus.subscribe::<CompletionRequested>(16);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);
    // 7. Reach the adapter through the
    //    kernel's public `llm()` accessor
    //    (the canonical "workflow gets the
    //    LLM" path).
    let llm = kernel.llm().expect("llm should be registered");
    let response = llm.complete(text_request(), &ctx()).await;
    let value = response.expect("happy path should return Ok");
    // 8. Assert the canned text + the
    //    canned usage values round-tripped
    //    through the adapter and the kernel.
    match value {
        CompletionResponse::TextReply { content, usage } => {
            assert_eq!(content, "kernel says hi");
            assert_eq!(usage.prompt_tokens, 12);
            assert_eq!(usage.completion_tokens, 7);
        }
        other => panic!("expected TextReply, got {other:?}"),
    }
    // 9. Assert the bus saw the two audit
    //    events with matching
    //    `correlation_id` (the audit story:
    //    "the workflow asked, the adapter
    //    answered; the same correlation id
    //    ties the two halves of the
    //    request together").
    let (req_evt, _req_ctx) = tokio::time::timeout(Duration::from_secs(2), req_sub.recv())
        .await
        .expect("CompletionRequested not received in time")
        .expect("CompletionRequested channel closed");
    let (comp_evt, _comp_ctx) = tokio::time::timeout(Duration::from_secs(2), comp_sub.recv())
        .await
        .expect("CompletionCompleted not received in time")
        .expect("CompletionCompleted channel closed");
    assert_eq!(
        req_evt.correlation_id, comp_evt.correlation_id,
        "the Requested/Completed pair must share a correlation id"
    );
    assert_eq!(comp_evt.prompt_tokens, 12);
    assert_eq!(comp_evt.completion_tokens, 7);
    assert_eq!(comp_evt.finish_reason, FinishReason::Stop);
    assert_eq!(comp_evt.model, "gpt-4o");
    assert!(!req_evt.has_tools);
    assert!(!req_evt.has_images);
    // 10. Health check: no extra events
    //     were published (the pair is
    //     exactly the two the contract
    //     promises — a third event would
    //     mean the adapter published a
    //     duplicate or the kernel
    //     accidentally double-stamped).
    assert!(
        tokio::time::timeout(Duration::from_millis(50), req_sub.recv())
            .await
            .is_err(),
        "no extra CompletionRequested should have been published"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), comp_sub.recv())
            .await
            .is_err(),
        "no extra CompletionCompleted should have been published"
    );
}

// ---------------------------------------------------------------------------
// Phase 4 — Kernel + ResponsesAdapter end-to-end (streaming)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kernel_end_to_end_streaming_chunk_ordering() {
    // The streaming half of the end-to-end
    // check. wiremock-rs returns a canned
    // SSE stream; the adapter decodes the
    // `data:` lines into typed
    // `CompletionChunk`s; the test asserts
    // the chunk ordering matches the
    // locked streaming contract
    // (`TextDelta` then `Finished`) and
    // that the bus saw the matching pair
    // of audit events.
    let (_dir, kernel) = fresh_kernel().await;
    let server = MockServer::start().await;
    // The OpenAI Responses adapter does NOT
    // set an `Accept: text/event-stream`
    // header on the streaming POST — it
    // only sets `Authorization` and the JSON
    // body (with `"stream": true` already
    // injected). So the wiremock-rs match
    // is just `method("POST")` +
    // `path("/v1/responses")`, mirroring
    // the `flow_3` + `flow_4` integration
    // tests in
    // `afa-plugin-llm-http/tests/adapter_streaming.rs`.
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(STREAMED_SSE_BODY),
        )
        .mount(&server)
        .await;
    let key_ref = seal_key(&kernel, b"sk-fake-kernel-stream", "kernel-stream-key").await;
    let adapter = ResponsesAdapter::new(
        config_for(&server.uri(), key_ref),
        kernel.security(),
        kernel.event_bus_handle(),
    );
    let adapter: Arc<dyn LlmV1> = Arc::new(adapter);
    kernel.register_llm(adapter.clone()).expect("register");
    let bus: Arc<EventBus> = kernel.event_bus();
    let mut req_sub = bus.subscribe::<CompletionRequested>(16);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);
    let llm = kernel.llm().expect("llm");
    let mut stream = llm
        .stream_complete(text_request(), &ctx())
        .await
        .expect("stream_complete should be Ok");
    // Collect the chunks. The contract:
    //   1. `TextDelta("hello from stream")`
    //   2. `Finished { reason: Stop, usage: { 15, 3 } }`
    let mut chunks: Vec<CompletionChunk> = Vec::new();
    while let Some(c) = stream.recv().await {
        chunks.push(c);
    }
    assert_eq!(chunks.len(), 2, "expected 2 chunks; got {chunks:?}");
    match &chunks[0] {
        CompletionChunk::TextDelta(s) => assert_eq!(s, "hello from stream"),
        other => panic!("expected first chunk to be TextDelta; got {other:?}"),
    }
    match &chunks[1] {
        CompletionChunk::Finished { reason, usage } => {
            assert_eq!(*reason, FinishReason::Stop);
            assert_eq!(usage.prompt_tokens, 15);
            assert_eq!(usage.completion_tokens, 3);
        }
        other => panic!("expected second chunk to be Finished; got {other:?}"),
    }
    // Bus saw the same Requested + Completed pair.
    let (req_evt, _) = tokio::time::timeout(Duration::from_secs(2), req_sub.recv())
        .await
        .expect("CompletionRequested not received in time")
        .expect("CompletionRequested channel closed");
    let (comp_evt, _) = tokio::time::timeout(Duration::from_secs(2), comp_sub.recv())
        .await
        .expect("CompletionCompleted not received in time")
        .expect("CompletionCompleted channel closed");
    assert_eq!(req_evt.correlation_id, comp_evt.correlation_id);
    assert_eq!(comp_evt.prompt_tokens, 15);
    assert_eq!(comp_evt.completion_tokens, 3);
    assert_eq!(comp_evt.finish_reason, FinishReason::Stop);
}

// ---------------------------------------------------------------------------
// Phase 4 — CapabilityRegistry closed-set: a second `register_llm` fails.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kernel_register_llm_twice_returns_lm_already_registered() {
    // The registry has a single LLM slot; a
    // second `register_llm` is a programmer
    // error. The kernel must surface it as
    // `RegisterError::LlmAlreadyRegistered`,
    // not a panic, so a buggy bootstrap
    // fails loudly but cleanly.
    let (_dir, kernel) = fresh_kernel().await;
    let adapter1: Arc<dyn LlmV1> = Arc::new(ResponsesAdapter::new(
        config_for(
            "http://127.0.0.1:1",
            SecretRef {
                name: "x".into(),
                version: 1,
            },
        ),
        kernel.security(),
        kernel.event_bus_handle(),
    ));
    let adapter2: Arc<dyn LlmV1> = Arc::new(ResponsesAdapter::new(
        config_for(
            "http://127.0.0.1:2",
            SecretRef {
                name: "y".into(),
                version: 1,
            },
        ),
        kernel.security(),
        kernel.event_bus_handle(),
    ));
    kernel
        .register_llm(adapter1)
        .expect("first register_llm should succeed");
    let e = kernel
        .register_llm(adapter2)
        .expect_err("second register_llm should fail");
    assert!(
        matches!(e, RegisterError::LlmAlreadyRegistered),
        "expected LlmAlreadyRegistered, got {e:?}"
    );
    // The first adapter is still the one
    // the registry hands out (a buggy
    // second register must NOT silently
    // overwrite the slot).
    let llm = kernel.llm().expect("llm should still be the first adapter");
    assert_eq!(llm.describe_capabilities().max_context_tokens, 128_000);
}

// ---------------------------------------------------------------------------
// Phase 4 — Pre-registration `kernel.llm()` returns `None`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn kernel_llm_returns_none_before_registration() {
    // A workflow that runs before the
    // bootstrap registers an LLM sees
    // `None` from `kernel.llm()`. The
    // workflow can branch on this and
    // surface a clear "no LLM configured"
    // error rather than a confusing
    // deref-of-None deep in the call
    // stack.
    let (_dir, kernel) = fresh_kernel().await;
    assert!(
        kernel.llm().is_none(),
        "kernel.llm() should return None before any register_llm call"
    );
}
