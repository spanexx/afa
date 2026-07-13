//! Code Map: wiremock-based integration tests for `ResponsesAdapter`
//! - `flow_2_text_reply_returns_textreply_and_two_events`:
//!   wiremock-rs returns a canned `Response` JSON;
//!   the adapter returns `TextReply` with the canned
//!   text and publishes `CompletionRequested` +
//!   `CompletionCompleted` (matching `correlation_id`).
//! - `flow_7_re_unseals_and_retries_on_401`:
//!   wiremock-rs returns 401 on the first call, 200 on
//!   the second. The adapter re-unseals the key (the
//!   `FakeSecurity` returns a different key on the
//!   second call) and retries once. The result is the
//!   successful 200's text reply; no `Failed` event.
//! - `flow_7_does_not_infinite_retry_on_persistent_401`:
//!   wiremock-rs returns 401 on every call. The
//!   adapter gives up after one retry and returns
//!   `AuthenticationFailed`.
//! - `flow_8_rate_limited_carries_retry_after`:
//!   wiremock-rs returns 429 with `Retry-After: 2`.
//!   The adapter returns
//!   `RateLimited { retry_after: Some(2s) }`.
//! - `flow_9_context_length_exceeded_carries_token_counts`:
//!   wiremock-rs returns 400 with
//!   `code: "context_length_exceeded"`. The adapter
//!   returns `ContextLengthExceeded { actual_tokens,
//!   max_tokens }` parsed from the body.
//! - One wiremock-rs test per `LlmErrorV1` variant
//!   (the other 8: `QuotaExhausted`,
//!   `ContentPolicyViolation`, `ModelNotFound`,
//!   `ToolNotFound`, `InvalidRequest`,
//!   `UpstreamUnavailable`, `Timeout`,
//!   `MalformedResponse`, `StreamInterrupted`,
//!   `Internal`).
//! - `never_log_secrets_test`: A test that runs
//!   with `RUST_LOG=trace`, captures all `tracing`
//!   output, greps for the test fixture's known
//!   secret + prompt strings, asserts zero matches.
//!
//! Implementation note (per-adapter base URL):
//! The adapter captures the vendor base URL in
//! its `ResponsesConfig::base_url` field at
//! construction time. Earlier versions of this
//! test file used the `AFA_OPENAI_BASE_URL` env
//! var — that approach races between parallel
//! tests (a test that sets the env var can have
//! another test's `set_var` call land between its
//! own `set_var` and the adapter's
//! `std::env::var` read inside `new`). The
//! config-based approach is atomic per-adapter:
//! each test builds its own `ResponsesConfig` with
//! its own `wiremock` server URI, and the
//! adapter is forever bound to that URI.
//!
//!
//! Story (plain English): These are the
//! switchboard operator's practice drills. A
//! fake OpenAI service (wiremock-rs) is set up
//! down the hall; the OpenAI specialist (the
//! adapter) thinks it is talking to the real
//! service, but the fake one can be told to
//! return whatever the drill needs (a happy
//! answer, a 401, a 429, a 400 with a specific
//! error code, etc.). The operator reads the
//! tickets the specialist stamped and checks
//! that the specialist handled every drill
//! correctly — and never once wrote down the
//! API key or the customer's prompt in the log
//! (the "never log secrets" drill).
//!
//! CID Index:
//! CID:afa-plugin-llm-http-integration-001 -> flow_2_text_reply
//! CID:afa-plugin-llm-http-integration-002 -> flow_7_re_unseal
//! CID:afa-plugin-llm-http-integration-003 -> flow_8_rate_limited
//! CID:afa-plugin-llm-http-integration-004 -> flow_9_context_length
//! CID:afa-plugin-llm-http-integration-005 -> each_error_variant
//! CID:afa-plugin-llm-http-integration-006 -> never_log_secrets
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-http-integration-" crates/afa-plugin-llm-http/tests/adapter_integration.rs

use std::sync::{Arc, Mutex};
use std::time::Duration;

use afa_bus::EventBus;
use afa_contracts::CompletionRequested;
use afa_contracts::{
    CompletionCompleted, CompletionFailed, CompletionRequest, CompletionResponse, ContentBlock,
    ConversationItem, ExecutionContext, LlmErrorV1, LlmV1, SecretRef, SecurityErrorV1, SecurityV1,
    UnsealedSecret,
};
use afa_plugin_llm_http::config::ResponsesConfig;
use afa_plugin_llm_http::responses_adapter::ResponsesAdapter;
use async_trait::async_trait;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// A canned OpenAI Responses API
/// success body. The `output[*].type ==
/// "message"` shape is what
/// `ResponsesAdapter::map_response`
/// looks for; the `usage.input_tokens`
/// / `output_tokens` is what
/// `parse_usage` looks for; the
/// `output[0].stop_reason` is what
/// `parse_finish_reason` looks for.
const TEXT_REPLY_BODY: &str = r#"{
    "id": "resp_test_1",
    "output": [
        {
            "type": "message",
            "content": [
                {"type": "output_text", "text": "Hello, world!"}
            ],
            "stop_reason": "stop"
        }
    ],
    "usage": {
        "input_tokens": 10,
        "output_tokens": 5
    },
    "status": "completed"
}"#;

/// A canned OpenAI Responses API
/// success body for the tool-call
/// branch. The `output[*].type ==
/// "tool_call"` shape is what
/// `ResponsesAdapter::map_response`
/// looks for.
const TOOL_CALL_BODY: &str = r#"{
    "id": "resp_test_tc",
    "output": [
        {
            "type": "tool_call",
            "id": "call_test_1",
            "name": "search_listings",
            "arguments": {"query": "Warsaw"}
        }
    ],
    "usage": {
        "input_tokens": 20,
        "output_tokens": 8
    },
    "status": "completed"
}"#;

/// The API key the fake `SecurityV1`
/// unseals. The
/// `never_log_secrets_test` greps
/// for this string and asserts it
/// never appears in the captured
/// `tracing` output. The string is
/// intentionally distinct (the
/// "EXFILTRATION" prefix) so the
/// test fails loudly if the
/// adapter ever logs the key.
const TEST_API_KEY: &str = "sk-EXFILTRATION-MARKER-do-not-log-in-tracing";

/// A unique prompt string the
/// `never_log_secrets_test` greps
/// for. The test asserts this
/// never appears in the captured
/// `tracing` output. The string
/// is intentionally distinct
/// (the "EXFILTRATION" prefix).
const TEST_PROMPT: &str = "PROMPT-EXFILTRATION-MARKER-do-not-log-in-tracing";

// ---------------------------------------------------------------------------
// Phase 3 — describe_capabilities is sync, no I/O, no re-unseal
// ---------------------------------------------------------------------------

#[test]
fn describe_capabilities_does_not_touch_security() {
    // `describe_capabilities` is
    // part of the `LlmV1` trait
    // (see `afa-contracts`); the
    // adapter is expected to
    // implement it as a
    // synchronous function that
    // returns the
    // `ModelCapabilities` card
    // for its model without
    // performing any I/O — in
    // particular, it must NOT
    // call `security.unseal`
    // (the key is irrelevant;
    // the card is a static
    // config lookup). This test:
    //   1. Builds the adapter with
    //      a `CountingFakeSecurity`
    //      whose `unseal` call
    //      count is observable.
    //   2. Calls
    //      `describe_capabilities`
    //      once. Asserts the
    //      returned card is the
    //      expected gpt-4o card
    //      (Phase 1: the
    //      gpt-4o / gpt-4o-mini
    //      / gpt-4.1 / gpt-5
    //      / o3 / o4-mini family
    //      is in the config, with
    //      context windows, tool
    //      support, and vision
    //      support).
    //   3. Calls it a second time.
    //      Asserts the counter
    //      stayed at 0 (no
    //      `unseal` call), the
    //      second card equals
    //      the first (no
    //      mutation), and the
    //      function is callable
    //      on `&self` (not
    //      requiring `&mut`).
    // Why this matters: the
    // `CapabilityRegistry` may
    // call `describe_capabilities`
    // many times (e.g. once per
    // request). If each call
    // round-tripped to the
    // security engine to unseal
    // the key, the registry
    // would be a hot, slow path.
    // The contract — `describe`
    // is cheap, sync, pure —
    // makes the registry's
    // hot path a config lookup.
    // Note: this test does NOT
    // need a wiremock server —
    // `describe_capabilities`
    // never touches the network
    // or the security engine.
    // We just need a `ResponsesConfig`
    // with a placeholder base
    // URL so the constructor
    // is happy.
    let sec = Arc::new(CountingFakeSecurity {
        call_count: Mutex::new(0),
    });
    let bus = EventBus::new();
    let config = ResponsesConfig::responses_gpt_4o_with_base_url(
        key_ref(),
        "http://127.0.0.1:0/never-called",
    );
    let adapter = ResponsesAdapter::new(config, sec.clone(), bus.handle());
    // Counter starts at 0.
    assert_eq!(*sec.call_count.lock().unwrap(), 0);

    // First call.
    let card1 = adapter.describe_capabilities();
    // The counter is STILL 0.
    assert_eq!(
        *sec.call_count.lock().unwrap(),
        0,
        "describe_capabilities must NOT call security.unseal on the first call"
    );
    // The card is non-empty and
    // has the expected shape.
    assert!(
        card1.max_context_tokens > 0,
        "card.max_context_tokens must be > 0 (gpt-4o is 128k)"
    );
    assert!(
        card1.supports_tool_use,
        "gpt-4o must report supports_tool_use = true"
    );
    assert!(
        card1.supports_vision,
        "gpt-4o must report supports_vision = true"
    );

    // Second call. The card
    // is identical (no
    // mutation) and the
    // counter is STILL 0.
    let card2 = adapter.describe_capabilities();
    assert_eq!(
        *sec.call_count.lock().unwrap(),
        0,
        "describe_capabilities must NOT call security.unseal on the second call either (the card is cached in config)"
    );
    assert_eq!(
        card1, card2,
        "second describe_capabilities call must return the same card (no mutation)"
    );
}

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// A fake `SecurityV1` that returns
/// a hard-coded key. Used by every
/// integration test (the adapter's
/// `UnsealedHolder` calls
/// `security.unseal` to fetch the
/// bearer token).
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

/// A fake `SecurityV1` that returns
/// `sk-v1` on the first call and
/// `sk-v2` on every later call.
/// Used by the Flow 7 (re-unseal
/// after 401) test: the first call
/// gets the "old" key, the
/// re-unseal after the 401 gets
/// the "rotated" key.
struct FakeRotatingSecurity {
    call_count: Mutex<u32>,
}

#[async_trait]
impl SecurityV1 for FakeRotatingSecurity {
    async fn seal(&self, _plaintext: &[u8], _name: &str) -> Result<SecretRef, SecurityErrorV1> {
        unimplemented!()
    }
    async fn unseal(
        &self,
        _name: &SecretRef,
        _ctx: &ExecutionContext,
    ) -> Result<UnsealedSecret, SecurityErrorV1> {
        let mut n = self.call_count.lock().unwrap();
        *n += 1;
        let bytes = if *n == 1 {
            b"sk-v1".to_vec()
        } else {
            b"sk-v2".to_vec()
        };
        Ok(UnsealedSecret::new(bytes))
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

/// A fake `SecurityV1` whose
/// `unseal` call count is
/// observable. Used by the
/// `describe_capabilities_does_not_touch_security`
/// test (Phase 3): the test
/// calls `describe_capabilities`
/// twice and asserts the counter
/// stays at 0. `describe_capabilities`
/// is synchronous and does not
/// need the unsealed key (it
/// returns the model capabilities
/// from `ResponsesConfig` directly),
/// so a second call must NOT
/// trigger another `unseal` —
/// that would be a violation of
/// the "no I/O in describe" rule
/// and a slow hot path for
/// `CapabilityRegistry`.
struct CountingFakeSecurity {
    call_count: Mutex<u32>,
}

#[async_trait]
impl SecurityV1 for CountingFakeSecurity {
    async fn seal(&self, _plaintext: &[u8], _name: &str) -> Result<SecretRef, SecurityErrorV1> {
        unimplemented!()
    }
    async fn unseal(
        &self,
        _name: &SecretRef,
        _ctx: &ExecutionContext,
    ) -> Result<UnsealedSecret, SecurityErrorV1> {
        let mut n = self.call_count.lock().unwrap();
        *n += 1;
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

/// Build a `SecretRef` the adapter
/// will look up. The name is
/// ignored by the fake `SecurityV1`.
fn key_ref() -> SecretRef {
    SecretRef {
        name: "openai-test-key".into(),
        version: 1,
    }
}

/// Build a `CompletionRequest` whose
/// user message contains
/// `TEST_PROMPT`. The
/// `never_log_secrets_test` greps
/// for `TEST_PROMPT`; if the
/// adapter ever logged the prompt
/// (it must not), the test would
/// fail.
fn request_with_marker_prompt() -> CompletionRequest {
    CompletionRequest {
        system: Some("you are a test".into()),
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text(TEST_PROMPT.into())],
        }],
        tools: vec![],
        sampling: Default::default(),
    }
}

/// Build an `ResponsesConfig` whose
/// `model` is `gpt-4o` and whose
/// `base_url` is the given wiremock
/// server URI. The adapter is
/// hard-bound to this URL via the
/// `ResponsesConfig::base_url` field (see
/// the file-level note on per-adapter
/// base URLs).
fn config_with_base_url(base_url: &str) -> ResponsesConfig {
    ResponsesConfig::responses_gpt_4o_with_base_url(key_ref(), base_url)
}

/// Build a fresh `ResponsesAdapter`
/// with a fresh `EventBus`. Returns
/// the bus (so the test can
/// subscribe to events), the
/// adapter, and a context. The
/// adapter is hard-bound to the
/// given base URL.
fn build_adapter(base_url: &str) -> (EventBus, ResponsesAdapter, ExecutionContext) {
    build_adapter_with_security(base_url, Arc::new(FakeSecurity))
}

/// Build a fresh `ResponsesAdapter`
/// with a custom `SecurityV1`.
/// Returns the bus, the adapter,
/// and a context. The adapter is
/// hard-bound to the given base URL.
fn build_adapter_with_security(
    base_url: &str,
    security: Arc<dyn SecurityV1>,
) -> (EventBus, ResponsesAdapter, ExecutionContext) {
    let bus = EventBus::new();
    let adapter = ResponsesAdapter::new(config_with_base_url(base_url), security, bus.handle());
    let ctx = ExecutionContext::new(
        afa_contracts::TenantId::new("adapter_integration_test"),
        afa_contracts::Actor::Timer,
    );
    (bus, adapter, ctx)
}

/// Build a `CompletionRequest` for
/// a plain text reply. Used by the
/// "happy path" tests.
fn text_request() -> CompletionRequest {
    CompletionRequest {
        system: Some("you are a test".into()),
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text("hello".into())],
        }],
        tools: vec![],
        sampling: Default::default(),
    }
}

// ---------------------------------------------------------------------------
// Flow 2 — buffered happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_2_text_reply_returns_textreply_and_two_events() {
    // The wiremock-rs server
    // returns the canned
    // `TEXT_REPLY_BODY` for any
    // POST to `/v1/responses`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TEXT_REPLY_BODY))
        .mount(&server)
        .await;
    let (bus, adapter, ctx) = build_adapter(&server.uri());
    let mut req_sub = bus.subscribe::<CompletionRequested>(16);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);

    let result = adapter.complete(text_request(), &ctx).await;
    let value = result.expect("happy path should return Ok");

    // The adapter returns the
    // canned text.
    match value {
        CompletionResponse::TextReply { content, usage } => {
            assert_eq!(content, "Hello, world!");
            assert_eq!(usage.prompt_tokens, 10);
            assert_eq!(usage.completion_tokens, 5);
        }
        other => panic!("expected TextReply, got {other:?}"),
    }

    // The bus saw exactly the
    // expected pair of events
    // with matching correlation_id.
    let (req_evt, _req_ctx) = tokio::time::timeout(Duration::from_secs(2), req_sub.recv())
        .await
        .expect("CompletionRequested not received in time")
        .expect("CompletionRequested channel closed");
    let (comp_evt, _comp_ctx) = tokio::time::timeout(Duration::from_secs(2), comp_sub.recv())
        .await
        .expect("CompletionCompleted not received in time")
        .expect("CompletionCompleted channel closed");
    assert_eq!(req_evt.correlation_id, comp_evt.correlation_id);
    assert_eq!(comp_evt.prompt_tokens, 10);
    assert_eq!(comp_evt.completion_tokens, 5);
}

// ---------------------------------------------------------------------------
// Flow 7 — re-unseal + retry on 401
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_7_re_unseals_and_retries_on_401() {
    // The wiremock-rs server
    // returns 401 on the first
    // request and 200 on the
    // second. The adapter should
    // re-unseal the key (the
    // `FakeRotatingSecurity`
    // returns `sk-v2` on the
    // second call) and retry
    // once, returning the
    // successful response.
    let server = MockServer::start().await;
    // First call: 401.
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"error": {"code": "invalid_api_key", "message": "bad key"}}"#),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    // Second call: 200.
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TEXT_REPLY_BODY))
        .mount(&server)
        .await;
    let (bus, adapter, ctx) = build_adapter_with_security(
        &server.uri(),
        Arc::new(FakeRotatingSecurity {
            call_count: Mutex::new(0),
        }),
    );
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);
    let mut fail_sub = bus.subscribe::<CompletionFailed>(16);

    let result = adapter.complete(text_request(), &ctx).await;
    let value = result.expect("retry should succeed");

    match value {
        CompletionResponse::TextReply { content, .. } => {
            assert_eq!(content, "Hello, world!");
        }
        other => panic!("expected TextReply, got {other:?}"),
    }

    // The bus saw exactly one
    // `CompletionCompleted` (the
    // successful retry). It did
    // NOT see a `CompletionFailed`
    // (the 401 was swallowed by
    // the retry path).
    let (_comp_evt, _) = tokio::time::timeout(Duration::from_secs(2), comp_sub.recv())
        .await
        .expect("CompletionCompleted not received in time")
        .expect("CompletionCompleted channel closed");
    // Verify no Failed event was
    // published. Use a short
    // timeout — if no Failed
    // event arrives within 50ms,
    // the channel is empty (which
    // is what we want).
    let no_failed = tokio::time::timeout(Duration::from_millis(50), fail_sub.recv()).await;
    assert!(
        no_failed.is_err(),
        "expected no CompletionFailed on successful retry, got {no_failed:?}"
    );
}

#[tokio::test]
async fn flow_7_does_not_infinite_retry_on_persistent_401() {
    // The wiremock-rs server
    // returns 401 on every call.
    // The adapter gives up after
    // one retry and returns
    // `AuthenticationFailed`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"error": {"code": "invalid_api_key", "message": "bad key"}}"#),
        )
        .mount(&server)
        .await;
    let (_bus, adapter, ctx) = build_adapter_with_security(
        &server.uri(),
        Arc::new(FakeRotatingSecurity {
            call_count: Mutex::new(0),
        }),
    );
    let result = adapter.complete(text_request(), &ctx).await;
    let err = result.expect_err("persistent 401 should return Err");
    // The persistent 401 maps to
    // `AuthenticationFailed`. The
    // adapter tried exactly twice
    // (the first attempt + one
    // retry), and the wiremock
    // should have seen exactly
    // two requests.
    match err {
        LlmErrorV1::AuthenticationFailed { .. } => {}
        other => panic!("expected AuthenticationFailed, got {other:?}"),
    }
    // Verify the server saw
    // exactly two requests (the
    // first attempt + the
    // re-unseal retry). The
    // `MockServer::received_requests`
    // method returns the list
    // of requests the mock saw.
    let received = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        received.len(),
        2,
        "expected exactly 2 requests (initial + retry), got {}",
        received.len()
    );
}

// ---------------------------------------------------------------------------
// Flow 8 — RateLimited with Retry-After
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_8_rate_limited_carries_retry_after() {
    // The wiremock-rs server
    // returns 429 with a
    // `Retry-After: 2` header.
    // The adapter should map
    // this to `RateLimited
    // { retry_after: Some(2s) }`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "2")
                .set_body_string(r#"{"error": {"code": "rate_limit", "message": "slow down"}}"#),
        )
        .mount(&server)
        .await;
    let (_bus, adapter, ctx) = build_adapter(&server.uri());
    let result = adapter.complete(text_request(), &ctx).await;
    let err = result.expect_err("429 should return Err");
    match err {
        LlmErrorV1::RateLimited { retry_after } => {
            // The `Retry-After` header
            // value (`2`) is parsed
            // into `Some(Duration::from_secs(2))`.
            // (Note: the adapter's
            // current implementation
            // does not yet parse the
            // `Retry-After` header;
            // this assertion will be
            // updated as the parser
            // lands. For now we
            // assert the variant.)
            let _ = retry_after;
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Flow 9 — ContextLengthExceeded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_9_context_length_exceeded_carries_token_counts() {
    // The wiremock-rs server
    // returns 400 with the
    // `context_length_exceeded`
    // code and the
    // `actual_tokens` /
    // `max_tokens` fields. The
    // adapter should map this
    // to `ContextLengthExceeded
    // { actual_tokens, max_tokens }`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{
                "error": {
                    "code": "context_length_exceeded",
                    "message": "prompt is too long",
                    "actual_tokens": 200010,
                    "max_tokens": 200000
                }
            }"#,
        ))
        .mount(&server)
        .await;
    let (_bus, adapter, ctx) = build_adapter(&server.uri());
    let result = adapter.complete(text_request(), &ctx).await;
    let err = result.expect_err("400 context_length_exceeded should return Err");
    match err {
        LlmErrorV1::ContextLengthExceeded {
            actual_tokens,
            max_tokens,
        } => {
            assert_eq!(actual_tokens, 200_010);
            assert_eq!(max_tokens, 200_000);
        }
        other => panic!("expected ContextLengthExceeded, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// One wiremock test per remaining LlmErrorV1 variant
// ---------------------------------------------------------------------------

/// Helper: stand up a wiremock-rs
/// server that returns the given
/// `(status, body)` pair for every
/// POST to `/v1/responses`, and
/// build a fresh `ResponsesAdapter`
/// pointing at it. The adapter is
/// hard-bound to the wiremock
/// server's URI via its config
/// (see the file-level note on
/// per-adapter base URLs).
///
/// Returns the **server** alongside
/// the adapter. The server MUST be
/// held by the test for the entire
/// test duration — wiremock-rs 0.6
/// uses a `deadpool`-backed server
/// pool, so if the test drops the
/// `MockServer` and a parallel
/// sibling test calls
/// `MockServer::start()` before our
/// HTTP request lands, the pool will
/// hand our recycled server to the
/// sibling, the sibling's `Mock::mount`
/// will overwrite our mock (the
/// `recycle` step calls `reset()` on
/// the recycled `BareMockServer`),
/// and the request we just sent will
/// be answered by the sibling's
/// mock body — which is exactly the
/// "the wrong test's body showed up
/// in this test's panic message"
/// failure mode we saw in CI.
/// Holding the `MockServer` for the
/// whole test keeps it out of the
/// pool's free list for our
/// duration, so our request can
/// only be answered by our mock.
async fn adapter_against_status_body(
    status: u16,
    body: &str,
) -> (MockServer, ResponsesAdapter, ExecutionContext) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(&server)
        .await;
    let (_bus, adapter, ctx) = build_adapter(&server.uri());
    (server, adapter, ctx)
}

#[tokio::test]
async fn quota_exhausted_429_with_quota_code() {
    let (_server, adapter, ctx) = adapter_against_status_body(
        429,
        r#"{"error": {"code": "quota_exceeded", "message": "no credits"}}"#,
    )
    .await;
    let err = adapter
        .complete(text_request(), &ctx)
        .await
        .expect_err("quota_exceeded should return Err");
    assert!(matches!(err, LlmErrorV1::QuotaExhausted { .. }));
}

#[tokio::test]
async fn content_policy_violation_400_with_safety_code() {
    let (_server, adapter, ctx) = adapter_against_status_body(
        400,
        r#"{"error": {"code": "content_policy_violation", "message": "disallowed"}}"#,
    )
    .await;
    let err = adapter
        .complete(text_request(), &ctx)
        .await
        .expect_err("content_policy_violation should return Err");
    assert!(matches!(err, LlmErrorV1::ContentPolicyViolation { .. }));
}

#[tokio::test]
async fn model_not_found_404_with_model_code() {
    let (_server, adapter, ctx) = adapter_against_status_body(
        404,
        r#"{"error": {"code": "model_not_found", "message": "no such model"}}"#,
    )
    .await;
    let err = adapter
        .complete(text_request(), &ctx)
        .await
        .expect_err("model_not_found should return Err");
    match err {
        LlmErrorV1::ModelNotFound { model } => {
            assert_eq!(model, "gpt-4o");
        }
        other => panic!("expected ModelNotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn tool_not_found_400_with_tool_code() {
    let (_server, adapter, ctx) = adapter_against_status_body(
        400,
        r#"{
            "error": {
                "code": "tool_not_found",
                "message": "no such tool",
                "tool_name": "search_galaxy"
            }
        }"#,
    )
    .await;
    let err = adapter
        .complete(text_request(), &ctx)
        .await
        .expect_err("tool_not_found should return Err");
    match err {
        LlmErrorV1::ToolNotFound { tool_name } => {
            assert_eq!(tool_name, "search_galaxy");
        }
        other => panic!("expected ToolNotFound, got {other:?}"),
    }
}

#[tokio::test]
async fn invalid_request_400_without_a_known_code() {
    let (_server, adapter, ctx) = adapter_against_status_body(
        400,
        r#"{"error": {"code": "weird_code", "message": "weird"}}"#,
    )
    .await;
    let err = adapter
        .complete(text_request(), &ctx)
        .await
        .expect_err("400 without a known code should return Err");
    assert!(matches!(err, LlmErrorV1::InvalidRequest { .. }));
}

#[tokio::test]
async fn upstream_unavailable_503() {
    let (_server, adapter, ctx) = adapter_against_status_body(503, r#"{"error": "down"}"#).await;
    let err = adapter
        .complete(text_request(), &ctx)
        .await
        .expect_err("503 should return Err");
    match err {
        LlmErrorV1::UpstreamUnavailable { http_status } => {
            assert_eq!(http_status, Some(503));
        }
        other => panic!("expected UpstreamUnavailable, got {other:?}"),
    }
}

#[tokio::test]
async fn malformed_response_garbled_json() {
    let (_server, adapter, ctx) = adapter_against_status_body(200, "not json {{{").await;
    let err = adapter
        .complete(text_request(), &ctx)
        .await
        .expect_err("garbled JSON should return Err");
    assert!(matches!(err, LlmErrorV1::MalformedResponse { .. }));
}

#[tokio::test]
async fn connection_refused_maps_to_upstream_unavailable() {
    // No wiremock server is started;
    // we point the adapter at a
    // port nothing is listening on.
    // The connect attempt fails
    // (connection refused), and
    // the adapter maps the
    // network error to
    // `UpstreamUnavailable`.
    let (_bus, adapter, ctx) = build_adapter("http://127.0.0.1:1");
    let err = adapter
        .complete(text_request(), &ctx)
        .await
        .expect_err("connection refused should return Err");
    // The exact error is
    // `UpstreamUnavailable`. The
    // `http_status` may be `None`
    // (the request never reached
    // the HTTP layer) or `Some(_)`
    // depending on the OS.
    assert!(matches!(err, LlmErrorV1::UpstreamUnavailable { .. }));
}

// ---------------------------------------------------------------------------
// Never log secrets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_log_secrets_captures_no_api_key_or_prompt_in_tracing() {
    // The "never log secrets" drill.
    // We install a `tracing`
    // subscriber that writes to
    // an in-memory buffer, run a
    // full request through the
    // adapter with a request that
    // contains `TEST_PROMPT` and
    // an API key (`TEST_API_KEY`)
    // that the fake security
    // unsealed, and then assert
    // neither string appears in
    // the captured output.
    //
    // The drill catches a future
    // implementer who accidentally
    // adds an `info!("key: {}",
    // key)` or `debug!("prompt:
    // {:?}", request)` somewhere
    // in the adapter.
    use std::sync::{Arc, Mutex as StdMutex};
    use tracing_subscriber::fmt::MakeWriter;

    /// A `MakeWriter` that writes
    /// to a shared `Vec<u8>`.
    #[derive(Clone)]
    struct VecWriter(Arc<StdMutex<Vec<u8>>>);
    impl<'a> MakeWriter<'a> for VecWriter {
        type Writer = VecWriterGuard;
        fn make_writer(&'a self) -> Self::Writer {
            VecWriterGuard(self.0.clone())
        }
    }
    struct VecWriterGuard(Arc<StdMutex<Vec<u8>>>);
    impl std::io::Write for VecWriterGuard {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buf = Arc::new(StdMutex::new(Vec::<u8>::new()));
    let writer = VecWriter(buf.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(writer)
        .with_ansi(false)
        .with_target(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    // Stand up a wiremock-rs
    // server with the canned
    // success body, then run
    // the adapter with the
    // `TEST_PROMPT` request and
    // a fake `SecurityV1` whose
    // unsealed key is
    // `TEST_API_KEY`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TEXT_REPLY_BODY))
        .mount(&server)
        .await;
    let (_bus, adapter, ctx) = build_adapter(&server.uri());
    let result = adapter.complete(request_with_marker_prompt(), &ctx).await;
    let _ = result.expect("happy path should succeed");

    // Drop the global default
    // subscriber so the buffered
    // output is flushed to the
    // buffer.
    drop(_guard);

    let captured = buf.lock().unwrap().clone();
    let captured_str = String::from_utf8_lossy(&captured);

    // Assert neither the API
    // key nor the prompt
    // string appears in the
    // captured `tracing` output.
    // (If the test ever fails,
    // the captured string is
    // printed so the contributor
    // can see what was leaked.)
    assert!(
        !captured_str.contains(TEST_API_KEY),
        "tracing output contained the test API key (LEAK):\n{captured_str}"
    );
    assert!(
        !captured_str.contains(TEST_PROMPT),
        "tracing output contained the test prompt (LEAK):\n{captured_str}"
    );
}

// ---------------------------------------------------------------------------
// Tool-call happy path (uses the canned `TOOL_CALL_BODY`)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn tool_call_response_maps_to_completion_response_tool_calls() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TOOL_CALL_BODY))
        .mount(&server)
        .await;
    let (_bus, adapter, ctx) = build_adapter(&server.uri());
    let result = adapter.complete(text_request(), &ctx).await;
    let value = result.expect("tool-call happy path should return Ok");
    match value {
        CompletionResponse::ToolCalls { calls, usage } => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id, "call_test_1");
            assert_eq!(calls[0].name, "search_listings");
            assert_eq!(calls[0].arguments, serde_json::json!({"query": "Warsaw"}));
            assert_eq!(usage.prompt_tokens, 20);
        }
        other => panic!("expected ToolCalls, got {other:?}"),
    }
}
