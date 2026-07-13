//! Code Map: real-adapter conformance proof
//!
//! This test runs `afa_llm::conformance::run_conformance_suite`
//! against the **real** `ChatCompletionsAdapter` (not the
//! `MockAdapter`). Each conformance case is mounted as a
//! `wiremock-rs` mock that matches the request body on the
//! `system` prompt tag (the `MockAdapter`'s
//! match-on-`system`-prompt convention; the real adapter
//! sends the system prompt as a `role: "system"` message).
//! When the suite reports `passed == 8 && failed == 0`,
//! the adapter is conformance-clean for the 8 standard
//! cases.
//!
//! Story (plain English): The conformance suite is the
//! industry-standard test drill: "does this LLM specialist
//! handle the 8 standard cases (text reply, tool call, the
//! 6 failure shapes)?" The `MockAdapter` proves the drill
//! can be run; this test proves the real
//! `ChatCompletionsAdapter` *passes* the drill. Each
//! `wiremock-rs` mock is one "drill question" — a fake
//! vendor service that returns the canned response the
//! drill expects for that case.
//!
//! CID Index:
//! CID:afa-plugin-llm-openai-compat-conformance-001 -> run_suite_against_real_adapter
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-openai-compat-conformance-" crates/afa-plugin-llm-openai-compat/tests/

use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::{
    CompletionRequest, CompletionResponse, CompletionStream, ContentBlock, ConversationItem,
    ExecutionContext, FinishReason, LlmErrorV1, LlmV1, ModelCapabilities, SecretRef,
    SecurityErrorV1, SecurityV1, ToolDefinition, UnsealedSecret, Usage,
};
use afa_llm::conformance::run_conformance_suite;
use afa_llm::mock_adapter::{FailureCase, MockAdapter};
use afa_plugin_llm_openai_compat::adapter::ChatCompletionsAdapter;
use afa_plugin_llm_openai_compat::config::ChatCompletionsConfig;
use async_trait::async_trait;
use wiremock::matchers::{body_string_contains, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

// ---------------------------------------------------------------------------
// Test fixtures
// ---------------------------------------------------------------------------

/// A fake `SecurityV1` whose `unseal`
/// always returns a hard-coded key.
struct StaticSecurity;

#[async_trait]
impl SecurityV1 for StaticSecurity {
    async fn seal(&self, _plaintext: &[u8], _name: &str) -> Result<SecretRef, SecurityErrorV1> {
        unimplemented!()
    }
    async fn unseal(
        &self,
        _name: &SecretRef,
        _ctx: &ExecutionContext,
    ) -> Result<UnsealedSecret, SecurityErrorV1> {
        Ok(UnsealedSecret::new(b"sk-test".to_vec()))
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

// Canned response bodies. The
// request body for each case
// contains the `system` prompt
// tag (`"conformance:<case>:..."`),
// which the wiremock-rs matchers
// use to pick the right canned
// response.
const TEXT_REPLY: &str = r#"{
    "choices": [{"index": 0, "message": {"role": "assistant", "content": "Hello, world!"}, "finish_reason": "stop"}],
    "usage": {"prompt_tokens": 5, "completion_tokens": 5, "total_tokens": 10}
}"#;
const TOOL_CALL: &str = r#"{
    "choices": [{"index": 0, "message": {"role": "assistant", "content": null, "tool_calls": [{"id": "call_1", "type": "function", "function": {"name": "search_listings", "arguments": "{\"query\":\"Warsaw\"}"}}]}, "finish_reason": "tool_calls"}],
    "usage": {"prompt_tokens": 20, "completion_tokens": 8, "total_tokens": 28}
}"#;
const RATE_LIMITED: &str = r#"{"error": {"code": "rate_limit_exceeded", "message": "slow down"}}"#;
const CONTEXT_TOO_LONG: &str = r#"{"error": {"code": "context_length_exceeded", "message": "too long", "actual_tokens": 200010, "max_tokens": 200000}}"#;
const MODEL_NOT_FOUND: &str =
    r#"{"error": {"code": "model_not_found", "message": "no such model"}}"#;
const TOOL_NOT_FOUND: &str = r#"{"error": {"code": "tool_not_found", "message": "no such tool", "tool_name": "search_galaxy"}}"#;
const MALFORMED: &str = "not-json-at-all";
const QUOTA_EXHAUSTED: &str =
    r#"{"error": {"code": "quota_exceeded", "message": "billing ended"}}"#;

/// Mount a wiremock-rs mock that
/// matches on the system prompt
/// tag in the request body and
/// returns the given status +
/// body.
async fn mount_case(server: &MockServer, tag: &str, status: u16, body: &str) {
    Mock::given(method("POST"))
        .and(path("/v1/chat/completions"))
        .and(body_string_contains(format!(
            "\"content\":\"conformance:{tag}"
        )))
        .respond_with(ResponseTemplate::new(status).set_body_string(body))
        .mount(server)
        .await;
}

fn basic_caps() -> ModelCapabilities {
    ModelCapabilities {
        max_context_tokens: 200_000,
        supports_vision: true,
        supports_tool_use: true,
    }
}

// CID:afa-plugin-llm-openai-compat-conformance-001
#[tokio::test]
async fn run_suite_against_real_adapter_passes_all_8_cases() {
    // The conformance suite has 8
    // standard cases. We mount 8
    // wiremock-rs mocks, one per
    // case, each matching on the
    // system prompt tag.
    let server = MockServer::start().await;
    // 2 success cases.
    mount_case(&server, "text_reply_basic", 200, TEXT_REPLY).await;
    mount_case(&server, "tool_call_basic", 200, TOOL_CALL).await;
    // 6 failure cases.
    mount_case(&server, "rate_limited", 429, RATE_LIMITED).await;
    mount_case(&server, "context_too_long", 400, CONTEXT_TOO_LONG).await;
    mount_case(&server, "model_not_found", 404, MODEL_NOT_FOUND).await;
    mount_case(&server, "tool_not_found", 400, TOOL_NOT_FOUND).await;
    mount_case(&server, "malformed_response", 200, MALFORMED).await;
    mount_case(&server, "quota_exhausted", 429, QUOTA_EXHAUSTED).await;

    let config = ChatCompletionsConfig::with_provider(
        "gpt-4o-mini",
        SecretRef {
            name: "test-key".into(),
            version: 1,
        },
        basic_caps(),
        &format!("{}/v1", server.uri()),
        "conformance-test",
    );
    let adapter: Arc<dyn LlmV1> = Arc::new(ChatCompletionsAdapter::new(
        config,
        Arc::new(StaticSecurity),
        EventBus::new().handle(),
    ));

    let report = run_conformance_suite(adapter.as_ref()).await;
    assert_eq!(
        report.passed, 8,
        "expected all 8 conformance cases to pass; failed_cases: {:#?}",
        report.failed_cases
    );
    assert_eq!(report.failed, 0);
}

/// Control test: the suite
/// passes against the
/// `MockAdapter`. If this test
/// fails, the suite itself is
/// broken (or the
/// `MockAdapter` is).
#[tokio::test]
async fn run_suite_against_mock_adapter_passes_all_8_cases() {
    let mock: Arc<dyn LlmV1> = Arc::new(MockAdapter::new());
    let report = run_conformance_suite(mock.as_ref()).await;
    assert_eq!(report.passed, 8);
    assert_eq!(report.failed, 0);
}

/// Negative test: the suite
/// catches a wrong-text-reply
/// adapter (proves the suite
/// asserts on the response
/// *content*, not just the
/// variant).
#[tokio::test]
async fn suite_catches_wrong_text_reply_content() {
    struct WrongTextReply;
    #[async_trait]
    impl LlmV1 for WrongTextReply {
        async fn complete(
            &self,
            _request: CompletionRequest,
            _ctx: &ExecutionContext,
        ) -> Result<CompletionResponse, LlmErrorV1> {
            Ok(CompletionResponse::TextReply {
                content: "WRONG".into(),
                usage: Usage {
                    prompt_tokens: 5,
                    completion_tokens: 5,
                },
            })
        }
        async fn stream_complete(
            &self,
            _request: CompletionRequest,
            _ctx: &ExecutionContext,
        ) -> Result<CompletionStream, LlmErrorV1> {
            unimplemented!()
        }
        fn describe_capabilities(&self) -> ModelCapabilities {
            ModelCapabilities {
                max_context_tokens: 1,
                supports_vision: false,
                supports_tool_use: false,
            }
        }
    }
    let wrong: Arc<dyn LlmV1> = Arc::new(WrongTextReply);
    let report = run_conformance_suite(wrong.as_ref()).await;
    // At least the
    // `text_reply_basic` case
    // failed.
    assert!(report.failed >= 1);
    let any_text_failure = report.failed_cases.iter().any(|s| s.contains("text_reply"));
    assert!(
        any_text_failure,
        "expected text_reply_basic in the failure list: {:#?}",
        report.failed_cases
    );
    // The `FailureCase`
    // marker is
    // importable (so
    // future callers
    // can build the
    // same canned
    // requests).
    let _ = FailureCase::RateLimited;
    // The `FinishReason`
    // enum and
    // `ToolDefinition`
    // are also
    // importable (so
    // future
    // conformance
    // cases can use
    // them).
    let _ = FinishReason::Stop;
    let _ = ToolDefinition {
        name: "x".into(),
        description: "x".into(),
        parameters_schema: serde_json::json!({}),
    };
    let _ = ConversationItem::UserMessage {
        content: vec![ContentBlock::Text("x".into())],
    };
}
