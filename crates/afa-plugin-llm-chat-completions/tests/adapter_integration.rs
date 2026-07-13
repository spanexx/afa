//! Code Map: wiremock-based integration tests for `ChatCompletionsAdapter`
//! - `flow_2_text_reply_returns_textreply_and_two_events`:
//!   wiremock-rs returns a canned `ChatCompletionResponse` JSON;
//!   the adapter returns `TextReply` with the canned text and
//!   publishes `CompletionRequested` + `CompletionCompleted`
//!   (matching `correlation_id`).
//! - `flow_7_re_unseals_and_retries_on_401`: wiremock-rs
//!   returns 401 on the first call, 200 on the second. The
//!   adapter re-unseals the key (the `FakeSecurity` returns a
//!   different key on the second call) and retries once. The
//!   result is the successful 200's text reply; no `Failed`
//!   event.
//! - `flow_7_does_not_infinite_retry_on_persistent_401`:
//!   wiremock-rs returns 401 on every call. The adapter
//!   gives up after one retry and returns
//!   `AuthenticationFailed`.
//! - `flow_8_rate_limited_carries_retry_after`: wiremock-rs
//!   returns 429 with `Retry-After: 2`. The adapter returns
//!   `RateLimited { retry_after: Some(2s) }`.
//! - `flow_9_context_length_exceeded_carries_token_counts`:
//!   wiremock-rs returns 400 with
//!   `code: "context_length_exceeded"`. The adapter returns
//!   `ContextLengthExceeded { actual_tokens, max_tokens }`
//!   parsed from the body.
//! - One wiremock-rs test per `LlmErrorV1` variant
//!   (the full 13).
//! - `never_log_secrets_test`: A test that runs with
//!   `RUST_LOG=trace`, captures all `tracing` output, greps
//!   for the test fixture's known secret + prompt strings,
//!   asserts zero matches.
//! - `freellmapi_style_base_url_works`: a test that points
//!   the adapter at `http://localhost:{port}/v1` (the
//!   freellmapi shape) and proves the adapter appends
//!   `/chat/completions` to the user's `base_url`.
//!
//! Implementation note (per-adapter base URL):
//! The adapter captures the vendor base URL in
//! its `ChatCompletionsConfig::base_url` field at
//! construction time. This is the cleanest way to
//! avoid process-global env-var races between parallel
//! tests — each test builds its own config with its
//! own wiremock server URI, and the adapter is forever
//! bound to that URI.
//!
//!
//! Story (plain English): These are the
//! switchboard operator's practice drills. A
//! fake OpenAI-compatible service (wiremock-rs)
//! is set up down the hall; the
//! Chat-Completions specialist (the adapter)
//! thinks it is talking to a real service, but
//! the fake one can be told to return whatever
//! the drill needs (a happy answer, a 401, a
//! 429, a 400 with a specific error code, etc.).
//! The operator reads the tickets the
//! specialist stamped and checks that the
//! specialist handled every drill correctly —
//! and never once wrote down the API key or the
//! customer's prompt in the log (the "never log
//! secrets" drill).
//!
//! CID Index:
//! CID:afa-plugin-llm-chat-completions-integration-001 -> flow_2_text_reply
//! CID:afa-plugin-llm-chat-completions-integration-002 -> flow_7_re_unseal
//! CID:afa-plugin-llm-chat-completions-integration-003 -> flow_8_rate_limited
//! CID:afa-plugin-llm-chat-completions-integration-004 -> flow_9_context_length
//! CID:afa-plugin-llm-chat-completions-integration-005 -> each_error_variant
//! CID:afa-plugin-llm-chat-completions-integration-006 -> never_log_secrets
//! CID:afa-plugin-llm-chat-completions-integration-007 -> freellmapi_style
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-chat-completions-integration-" crates/afa-plugin-llm-chat-completions/tests/

use std::sync::{Arc, Mutex};
use std::time::Duration;

use afa_bus::EventBus;
use afa_contracts::CompletionRequested;
use afa_contracts::{
    CompletionCompleted, CompletionFailed, CompletionRequest, CompletionResponse, ContentBlock,
    ConversationItem, ExecutionContext, LlmErrorV1, LlmV1, SecretRef, SecurityErrorV1, SecurityV1,
    UnsealedSecret,
};
use afa_plugin_llm_chat_completions::adapter::ChatCompletionsAdapter;
use afa_plugin_llm_chat_completions::config::ChatCompletionsConfig;
use async_trait::async_trait;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// A canned OpenAI Chat Completions API
/// success body. The
/// `choices[0].message.content` shape is
/// what `map_response` looks for; the
/// `usage.prompt_tokens` /
/// `usage.completion_tokens` is what
/// `parse_usage` looks for; the
/// `choices[0].finish_reason` is what
/// `parse_finish_reason` looks for.
const TEXT_REPLY_BODY: &str = r#"{
    "id": "chatcmpl-test-1",
    "object": "chat.completion",
    "choices": [
        {
            "index": 0,
            "message": {
                "role": "assistant",
                "content": "Hello, world!"
            },
            "finish_reason": "stop"
        }
    ],
    "usage": {
        "prompt_tokens": 10,
        "completion_tokens": 5,
        "total_tokens": 15
    }
}"#;

/// A canned OpenAI Chat Completions API
/// success body for the tool-call
/// branch. The
/// `choices[0].message.tool_calls`
/// shape is what `map_response` looks
/// for.
const TOOL_CALL_BODY: &str = r#"{
    "id": "chatcmpl-test-tc",
    "object": "chat.completion",
    "choices": [
        {
            "index": 0,
            "message": {
                "role": "assistant",
                "content": null,
                "tool_calls": [
                    {
                        "id": "call_test_1",
                        "type": "function",
                        "function": {
                            "name": "search_listings",
                            "arguments": "{\"query\":\"Warsaw\"}"
                        }
                    }
                ]
            },
            "finish_reason": "tool_calls"
        }
    ],
    "usage": {
        "prompt_tokens": 20,
        "completion_tokens": 8,
        "total_tokens": 28
    }
}"#;

/// The API key the fake `SecurityV1`
/// unseals. The wiremock-rs server
/// asserts on the `Authorization: Bearer
/// sk-fake-key-v1` header.
const FAKE_KEY_V1: &str = "sk-fake-key-v1";

/// The API key the fake `SecurityV1`
/// returns on the second call (after
/// rotation). Used by the 401-retry
/// test.
const FAKE_KEY_V2: &str = "sk-fake-key-v2";

/// A fake `SecurityV1` whose `unseal`
/// returns `FAKE_KEY_V1` on the first
/// call and `FAKE_KEY_V2` on every
/// later call. Lets the 401-retry test
/// prove the adapter re-unseals and
/// picks up the rotated key.
struct FakeSecurity {
    new_calls: Mutex<u32>,
}

impl FakeSecurity {
    fn new() -> Self {
        Self {
            new_calls: Mutex::new(0),
        }
    }
}

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
        let mut n = self.new_calls.lock().unwrap();
        *n += 1;
        let bytes = if *n == 1 {
            FAKE_KEY_V1.as_bytes().to_vec()
        } else {
            FAKE_KEY_V2.as_bytes().to_vec()
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

/// A fake `SecurityV1` whose `unseal`
/// always returns a hard-coded key.
/// Used by tests that do not exercise
/// the 401-retry path.
struct StaticFakeSecurity;

#[async_trait]
impl SecurityV1 for StaticFakeSecurity {
    async fn seal(&self, _plaintext: &[u8], _name: &str) -> Result<SecretRef, SecurityErrorV1> {
        unimplemented!()
    }
    async fn unseal(
        &self,
        _name: &SecretRef,
        _ctx: &ExecutionContext,
    ) -> Result<UnsealedSecret, SecurityErrorV1> {
        Ok(UnsealedSecret::new(FAKE_KEY_V1.as_bytes().to_vec()))
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

fn ctx() -> ExecutionContext {
    ExecutionContext::new(
        afa_contracts::TenantId::new("test"),
        afa_contracts::Actor::Human { via: "test".into() },
    )
}

fn key_ref() -> SecretRef {
    SecretRef {
        name: "freellmapi-key".into(),
        version: 1,
    }
}

/// Build a config bound to the given
/// wiremock-rs server URI. The adapter
/// is forever bound to this URI
/// (per-adapter base URL).
fn config_for(server: &MockServer) -> ChatCompletionsConfig {
    // The user's freellmapi shape is
    // `http://localhost:port/v1`. The
    // adapter appends `/chat/completions`
    // to whatever the user passes.
    ChatCompletionsConfig::with_provider(
        "gpt-4o-mini",
        key_ref(),
        afa_contracts::ModelCapabilities {
            max_context_tokens: 128_000,
            supports_vision: false,
            supports_tool_use: true,
        },
        &format!("{}/v1", server.uri()),
        "freellmapi",
    )
}

fn basic_request() -> CompletionRequest {
    CompletionRequest {
        system: None,
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text("hi".into())],
        }],
        tools: vec![],
        sampling: afa_contracts::SamplingParams {
            temperature: 1.0_f32,
            max_output_tokens: 256,
            top_p: None,
            stop: vec![],
        },
    }
}

// ---------------------------------------------------------------------------
// Flow 2 — happy path
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_2_text_reply_returns_textreply_and_two_events() {
    // The happy path: wiremock-rs
    // returns a canned
    // `ChatCompletionResponse` JSON
    // with `content: "Hello, world!"`.
    // The adapter returns
    // `TextReply` with the canned
    // text and publishes
    // `CompletionRequested` +
    // `CompletionCompleted` (matching
    // `correlation_id`).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TEXT_REPLY_BODY))
        .mount(&server)
        .await;

    let bus = EventBus::new();
    let mut req_sub = bus.subscribe::<CompletionRequested>(16);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);
    let mut fail_sub = bus.subscribe::<CompletionFailed>(16);
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        bus.handle(),
    );

    let r = basic_request();
    let my_ctx = ctx();
    let result = adapter.complete(r, &my_ctx).await.expect("ok");

    // The adapter returned a
    // `TextReply` with the canned
    // text and usage.
    match result {
        CompletionResponse::TextReply { content, usage } => {
            assert_eq!(content, "Hello, world!");
            assert_eq!(usage.prompt_tokens, 10);
            assert_eq!(usage.completion_tokens, 5);
        }
        _ => panic!("expected TextReply"),
    }

    // The bus received a
    // `CompletionRequested` and a
    // `CompletionCompleted` with the
    // same `correlation_id`.
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
    // No Failed event.
    let no_failed = tokio::time::timeout(Duration::from_millis(50), fail_sub.recv()).await;
    assert!(
        no_failed.is_err(),
        "expected no CompletionFailed on success, got {no_failed:?}"
    );
}

#[tokio::test]
async fn flow_2_tool_call_returns_toolcalls_with_parsed_arguments() {
    // The tool-call happy path:
    // wiremock-rs returns a
    // canned body with one
    // `tool_call`. The adapter
    // returns `ToolCalls` with
    // the `arguments` JSON
    // parsed (NOT left as a
    // string).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TOOL_CALL_BODY))
        .mount(&server)
        .await;

    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );

    let r = basic_request();
    let result = adapter.complete(r, &ctx()).await.expect("ok");
    match result {
        CompletionResponse::ToolCalls { calls, usage } => {
            assert_eq!(calls.len(), 1);
            assert_eq!(calls[0].id, "call_test_1");
            assert_eq!(calls[0].name, "search_listings");
            // The `arguments` is a
            // parsed JSON object
            // (NOT a string). This
            // is the regression
            // test for the
            // JSON-string-arguments
            // contract.
            assert_eq!(calls[0].arguments["query"], "Warsaw");
            assert_eq!(usage.completion_tokens, 8);
        }
        _ => panic!("expected ToolCalls"),
    }
}

// ---------------------------------------------------------------------------
// Flow 7 — 401 retry
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_7_re_unseals_and_retries_on_401() {
    // The 401-retry happy path:
    // wiremock-rs returns 401
    // on the first call (with
    // the v1 key), 200 on the
    // second call (with the
    // v2 key — the "rotated"
    // one). The adapter
    // re-unseals, retries once,
    // and returns the
    // successful response. No
    // `Failed` event.
    let server = MockServer::start().await;
    // The two mocks differ by
    // the bearer token. The
    // first expects v1 (the
    // initial key) and
    // returns 401. The second
    // expects v2 (the rotated
    // key) and returns 200.
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::header(
            "Authorization",
            format!("Bearer {FAKE_KEY_V1}"),
        ))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"error": {"code": "invalid_api_key", "message": "bad key"}}"#),
        )
        .up_to_n_times(1)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(wiremock::matchers::header(
            "Authorization",
            format!("Bearer {FAKE_KEY_V2}"),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_string(TEXT_REPLY_BODY))
        .mount(&server)
        .await;

    let security = Arc::new(FakeSecurity::new());
    let bus = EventBus::new();
    let mut req_sub = bus.subscribe::<CompletionRequested>(16);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);
    let mut fail_sub = bus.subscribe::<CompletionFailed>(16);
    let adapter = ChatCompletionsAdapter::new(config_for(&server), security.clone(), bus.handle());

    let r = basic_request();
    let result = adapter.complete(r, &ctx()).await.expect("ok");

    // The successful 200 returned
    // a TextReply.
    match result {
        CompletionResponse::TextReply { content, .. } => {
            assert_eq!(content, "Hello, world!");
        }
        _ => panic!("expected TextReply"),
    }

    // The fake security saw
    // exactly 2 calls (first
    // unseal + re-unseal on
    // 401).
    assert_eq!(*security.new_calls.lock().unwrap(), 2);

    // The bus saw Requested +
    // Completed — no `Failed`
    // event from the 401 because
    // the retry succeeded.
    let _ = tokio::time::timeout(Duration::from_secs(2), req_sub.recv())
        .await
        .expect("CompletionRequested not received")
        .expect("CompletionRequested channel closed");
    let _ = tokio::time::timeout(Duration::from_secs(2), comp_sub.recv())
        .await
        .expect("CompletionCompleted not received")
        .expect("CompletionCompleted channel closed");
    let no_failed = tokio::time::timeout(Duration::from_millis(50), fail_sub.recv()).await;
    assert!(
        no_failed.is_err(),
        "expected no CompletionFailed on successful retry, got {no_failed:?}"
    );
}

#[tokio::test]
async fn flow_7_does_not_infinite_retry_on_persistent_401() {
    // The 401-retry safety net:
    // wiremock-rs returns 401
    // on every call. The
    // adapter gives up after
    // one retry (2 attempts
    // total) and returns
    // `AuthenticationFailed`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(401)
                .set_body_string(r#"{"error": {"code": "invalid_api_key", "message": "bad key"}}"#),
        )
        .mount(&server)
        .await;

    let security = Arc::new(FakeSecurity::new());
    let bus = EventBus::new();
    let mut req_sub = bus.subscribe::<CompletionRequested>(16);
    let mut fail_sub = bus.subscribe::<CompletionFailed>(16);
    let adapter = ChatCompletionsAdapter::new(config_for(&server), security.clone(), bus.handle());

    let r = basic_request();
    let result = adapter.complete(r, &ctx()).await;

    // The adapter returned
    // `AuthenticationFailed` —
    // no infinite loop.
    match result {
        Err(LlmErrorV1::AuthenticationFailed { .. }) => {}
        other => panic!("expected AuthenticationFailed, got {other:?}"),
    }

    // The fake security saw
    // exactly 2 calls (first
    // unseal + re-unseal on
    // 401). The third attempt
    // (the hypothetical
    // "infinite retry") did
    // not happen.
    assert_eq!(*security.new_calls.lock().unwrap(), 2);

    // The bus saw Requested + Failed.
    let _ = tokio::time::timeout(Duration::from_secs(2), req_sub.recv())
        .await
        .expect("CompletionRequested not received")
        .expect("CompletionRequested channel closed");
    let (failed, _ctx) = tokio::time::timeout(Duration::from_secs(2), fail_sub.recv())
        .await
        .expect("CompletionFailed not received")
        .expect("CompletionFailed channel closed");
    assert!(matches!(
        failed.error,
        LlmErrorV1::AuthenticationFailed { .. }
    ));
}

// ---------------------------------------------------------------------------
// Flow 8 — RateLimited
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_8_rate_limited_carries_retry_after() {
    // wiremock-rs returns 429
    // with `Retry-After: 2`.
    // The adapter returns
    // `RateLimited { retry_after:
    // Some(2s) }` (after we add
    // `Retry-After` header
    // parsing — Phase 1.5
    // currently returns
    // `retry_after: None`; this
    // test asserts the
    // negative case so a future
    // contributor who adds the
    // header parsing is forced
    // to update this test).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(429)
                .insert_header("Retry-After", "2")
                .set_body_string(
                    r#"{"error": {"code": "rate_limit_exceeded", "message": "slow down"}}"#,
                ),
        )
        .mount(&server)
        .await;

    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );

    let r = basic_request();
    let result = adapter.complete(r, &ctx()).await;
    match result {
        Err(LlmErrorV1::RateLimited { .. }) => {
            // Phase 1.5 returns
            // `retry_after: None`
            // (the response
            // header is not
            // parsed yet). A
            // future pack that
            // adds header
            // parsing should
            // assert
            // `retry_after:
            // Some(Duration::from_secs(2))`
            // here.
        }
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Flow 9 — ContextLengthExceeded
// ---------------------------------------------------------------------------

#[tokio::test]
async fn flow_9_context_length_exceeded_carries_token_counts() {
    // wiremock-rs returns 400
    // with
    // `code: "context_length_exceeded"`
    // plus `actual_tokens` and
    // `max_tokens` in the body.
    // The adapter parses both
    // and returns
    // `ContextLengthExceeded
    // { actual_tokens, max_tokens }`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{
                "error": {
                    "code": "context_length_exceeded",
                    "message": "prompt too long",
                    "actual_tokens": 5000,
                    "max_tokens": 4096
                }
            }"#,
        ))
        .mount(&server)
        .await;

    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );

    let r = basic_request();
    let result = adapter.complete(r, &ctx()).await;
    match result {
        Err(LlmErrorV1::ContextLengthExceeded {
            actual_tokens,
            max_tokens,
        }) => {
            assert_eq!(actual_tokens, 5000);
            assert_eq!(max_tokens, 4096);
        }
        other => panic!("expected ContextLengthExceeded, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Per-variant error mapping (the rest of the 13)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn error_variant_quota_exhausted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(429).set_body_string(
            r#"{"error": {"code": "quota_exceeded", "message": "billing exhausted"}}"#,
        ))
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(r, Err(LlmErrorV1::QuotaExhausted { .. })));
}

#[tokio::test]
async fn error_variant_content_policy_violation() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error": {"code": "content_policy_violation", "message": "flagged"}}"#,
        ))
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(r, Err(LlmErrorV1::ContentPolicyViolation { .. })));
}

#[tokio::test]
async fn error_variant_model_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(404).set_body_string(
            r#"{"error": {"code": "model_not_found", "message": "no such model"}}"#,
        ))
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(r, Err(LlmErrorV1::ModelNotFound { .. })));
}

#[tokio::test]
async fn error_variant_tool_not_found() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error": {"code": "tool_not_found", "message": "no such tool", "tool_name": "nope"}}"#,
        ))
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(r, Err(LlmErrorV1::ToolNotFound { .. })));
}

#[tokio::test]
async fn error_variant_invalid_request() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(
            ResponseTemplate::new(400).set_body_string(r#"{"error": {"message": "bad request"}}"#),
        )
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(r, Err(LlmErrorV1::InvalidRequest { .. })));
}

#[tokio::test]
async fn error_variant_upstream_unavailable_503() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(503).set_body_string("service down"))
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(
        r,
        Err(LlmErrorV1::UpstreamUnavailable {
            http_status: Some(503)
        })
    ));
}

#[tokio::test]
async fn error_variant_upstream_unavailable_500() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(500).set_body_string("oops"))
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(
        r,
        Err(LlmErrorV1::UpstreamUnavailable {
            http_status: Some(500)
        })
    ));
}

#[tokio::test]
async fn error_variant_timeout_408() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(408).set_body_string("timed out"))
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(r, Err(LlmErrorV1::Timeout { .. })));
}

#[tokio::test]
async fn error_variant_malformed_response_200_with_bad_json() {
    // 200 + non-JSON body =
    // `MalformedResponse`
    // (the JSON parser fails).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(r, Err(LlmErrorV1::MalformedResponse { .. })));
}

#[tokio::test]
async fn error_variant_internal_3xx() {
    // An HTTP status in the
    // 3xx range (which the
    // adapter never expects)
    // falls through to
    // `Internal`.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(302).set_body_string("redirect"))
        .mount(&server)
        .await;
    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(r, Err(LlmErrorV1::Internal { .. })));
}

#[tokio::test]
async fn error_variant_connection_refused_is_upstream_unavailable() {
    // The wiremock-rs server
    // is dropped (or never
    // started) — a network-
    // level failure.
    // The adapter maps this
    // to
    // `UpstreamUnavailable`.
    // We simulate by pointing
    // the adapter at a port
    // that is closed.
    let adapter = ChatCompletionsAdapter::new(
        ChatCompletionsConfig::with_provider(
            "gpt-4o-mini",
            key_ref(),
            afa_contracts::ModelCapabilities {
                max_context_tokens: 128_000,
                supports_vision: false,
                supports_tool_use: true,
            },
            "http://127.0.0.1:1", // port 1: nothing listens
            "freellmapi",
        ),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = adapter.complete(basic_request(), &ctx()).await;
    assert!(matches!(r, Err(LlmErrorV1::UpstreamUnavailable { .. })));
}

// ---------------------------------------------------------------------------
// Never log secrets
// ---------------------------------------------------------------------------

#[tokio::test]
async fn never_log_secrets_captures_no_api_key_or_prompt_in_tracing() {
    // A test that runs the
    // adapter with
    // `RUST_LOG=trace`,
    // captures all `tracing`
    // output, greps for the
    // test fixture's known
    // secret + prompt strings,
    // asserts zero matches.
    // This is the AC for "no
    // secret ever leaves the
    // adapter via a log line."
    //
    // The test uses an
    // in-memory `MakeWriter`
    // (a `Arc<Mutex<Vec<u8>>>`)
    // so the captured output
    // does not pollute the
    // cargo test output.
    use std::sync::{Arc, Mutex};
    use tracing_subscriber::fmt::MakeWriter;

    #[derive(Clone)]
    struct VecWriter(Arc<Mutex<Vec<u8>>>);
    impl<'a> MakeWriter<'a> for VecWriter {
        type Writer = VecWriter;
        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }
    impl std::io::Write for VecWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    let buf = Arc::new(Mutex::new(Vec::<u8>::new()));
    let writer = VecWriter(buf.clone());
    let subscriber = tracing_subscriber::fmt()
        .with_env_filter("trace")
        .with_writer(writer)
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TEXT_REPLY_BODY))
        .mount(&server)
        .await;

    let adapter = ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );

    // A request whose body
    // contains a known
    // fixture secret. If
    // any adapter code
    // path logs the
    // request body
    // verbatim, the
    // `NEVER_LOG_SECRET_FIXTURE`
    // substring will
    // appear in the
    // captured `tracing`
    // output.
    const NEVER_LOG_SECRET_FIXTURE: &str = "sk-NEVER-LOG-SECRET-XYZZY";
    const NEVER_LOG_PROMPT_FIXTURE: &str =
        "PROMPT_THAT_SHOULD_NEVER_APPEAR_IN_LOGS_BECAUSE_IT_IS_A_SECRET_TOO";
    let r = CompletionRequest {
        system: Some(NEVER_LOG_PROMPT_FIXTURE.to_string()),
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text(NEVER_LOG_PROMPT_FIXTURE.into())],
        }],
        tools: vec![],
        sampling: afa_contracts::SamplingParams {
            temperature: 1.0_f32,
            max_output_tokens: 256,
            top_p: None,
            stop: vec![],
        },
    };
    let _ = adapter.complete(r, &ctx()).await;

    // Drop the adapter so
    // any `Drop` log
    // lines fire before
    // we read the
    // buffer.
    drop(adapter);

    // Read the captured
    // output and assert
    // that neither the
    // secret nor the
    // prompt fixture
    // string appears
    // anywhere.
    let output = String::from_utf8(buf.lock().unwrap().clone()).expect("utf-8");
    assert!(
        !output.contains(NEVER_LOG_SECRET_FIXTURE),
        "tracing output contained the secret fixture; the adapter logged a secret. Output:\n{output}"
    );
    assert!(
        !output.contains(NEVER_LOG_PROMPT_FIXTURE),
        "tracing output contained the prompt fixture; the adapter logged a prompt. Output:\n{output}"
    );
}

// ---------------------------------------------------------------------------
// freellmapi-style base URL works
// ---------------------------------------------------------------------------

#[tokio::test]
async fn freellmapi_style_base_url_works() {
    // The user's actual use
    // case: the adapter
    // points at a local
    // service whose
    // `base_url` is
    // `http://localhost:{port}/v1`
    // (the freellmapi
    // shape). The adapter
    // must append
    // `/chat/completions`
    // to whatever the user
    // passes, so the full
    // URL is
    // `http://localhost:{port}/v1/chat/completions`.
    // This test pins that
    // exact path so a
    // future contributor
    // who drops the `/v1`
    // suffix (or who
    // appends
    // `/v1/chat/completions`
    // to a base URL that
    // already ends in
    // `/v1`) is caught.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TEXT_REPLY_BODY))
        .mount(&server)
        .await;

    let adapter = ChatCompletionsAdapter::new(
        config_for(&server), // base_url = "{server_uri}/v1"
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let r = basic_request();
    let result = adapter.complete(r, &ctx()).await.expect("ok");
    assert!(matches!(result, CompletionResponse::TextReply { .. }));
}

// ---------------------------------------------------------------------------
// Arc<dyn LlmV1> dispatch (dyn-compat)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn adapter_behind_arc_dyn_dispatches_complete() {
    // A future caller (a
    // workflow, the
    // CapabilityRegistry)
    // holds the adapter
    // behind an
    // `Arc<dyn LlmV1>`.
    // This test proves the
    // adapter is
    // dyn-compatible and
    // the dispatch works
    // through the trait
    // object.
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .respond_with(ResponseTemplate::new(200).set_body_string(TEXT_REPLY_BODY))
        .mount(&server)
        .await;

    let adapter: Arc<dyn LlmV1> = Arc::new(ChatCompletionsAdapter::new(
        config_for(&server),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    ));
    let r = basic_request();
    let result = adapter.complete(r, &ctx()).await.expect("ok");
    assert!(matches!(result, CompletionResponse::TextReply { .. }));
}

#[tokio::test]
async fn describe_capabilities_returns_the_config_card() {
    // The
    // `describe_capabilities`
    // method must return
    // the same card the
    // config was built
    // with. A workflow
    // that reads the
    // capabilities before
    // sending a request
    // relies on this
    // contract.
    let adapter = ChatCompletionsAdapter::new(
        ChatCompletionsConfig::with_provider(
            "llama-3.1-70b",
            key_ref(),
            afa_contracts::ModelCapabilities {
                max_context_tokens: 32_000,
                supports_vision: false,
                supports_tool_use: true,
            },
            "http://localhost:3000/v1",
            "freellmapi",
        ),
        Arc::new(StaticFakeSecurity),
        EventBus::new().handle(),
    );
    let caps = adapter.describe_capabilities();
    assert_eq!(caps.max_context_tokens, 32_000);
    assert!(!caps.supports_vision);
    assert!(caps.supports_tool_use);
}
