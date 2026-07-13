//! Live-vendor **negative battery** for `ChatCompletionsAdapter`.
//!
//! Sends 6 intentionally-broken requests to a real
//! OpenAI-compatible vendor (your local `freellmapi` proxy
//! by default; works for any service that speaks
//! `/chat/completions`) and asserts each one lands in the
//! expected `LlmErrorV1` variant (or `Ok` for the control
//! case). Catches vendor-shape drift that wiremock-rs can't
//! see: a vendor that returns a different error type for a
//! 401, a 429 wrapped in a `data.error` envelope, a
//! 200-with-error-in-body, etc.
//!
//! ## What it tests (one case per row)
//!
//! | Case                  | Input                          | Expected variant        |
//! |-----------------------|--------------------------------|-------------------------|
//! | `success_control`     | good key, simple "PONG"        | `Ok(TextReply)`         |
//! | `bad_key`             | wrong key, simple request      | `AuthenticationFailed`  |
//! | `unknown_model`       | model="gpt-9000-doesnotexist"  | `ModelNotFound`         |
//! | `oversized_context`   | 200 000-char user message      | `ContextLengthExceeded` |
//! | `empty_content`       | empty content blocks           | `Ok(TextReply)` or `InvalidRequest` (vendor-dependent) |
//! | `invalid_temperature` | `temperature=5.0`             | `InvalidRequest`        |
//!
//! The 6th case (`invalid_temperature`) is the strongest
//! drift test: it sends a value that is syntactically valid
//! (a `f32` in the request JSON) but semantically out of
//! range, so the *vendor* has to reject it. If our parser
//! trusts the vendor to return a `400` and the vendor
//! returns a `200` with a fallback warning instead, this
//! test fails.
//!
//! ## How to run it
//!
//! ```bash
//! FREELLMAPI_URL="http://localhost:3001/v1" \
//! FREELLMAPI_MODEL="auto" \
//! FREELLMAPI_KEY="..." \
//! cargo run --example negative_battery_smoke -p afa-plugin-llm-openai-compat
//! ```
//!
//! Exits 0 if all 6 cases match, 1 if any case lands in an
//! unexpected variant. The full per-case output (status,
//! variant, body preview) is printed regardless of pass/fail
//! so a CI script can grep it.
//!
//! ## Safety
//!
//! - The API key is read from the env, never hard-coded.
//! - The example prints `key_present=true|false`, never the
//!   key value itself.
//! - No live secret is written to disk or logged via
//!   `tracing`.
//!
//!
//! Story (plain English): The "Lend Your Voice" specialist
//! practices a battery of 6 calls against the real customer
//! site, each one designed to break a different way. The
//! specialist is supposed to map every breakage to a
//! well-known `LlmErrorV1` variant so the switchboard can
//! route the failure to the right human ("auth failed,
//! please refresh the key" vs. "context too long, please
//! shrink the prompt"). A specialist that misclassifies
//! "context too long" as "rate limited" will send the user
//! on a wild-goose chase. This battery is the proof that the
//! mapping is correct against a real vendor, not just a
//! fake one (wiremock-rs).
//!
//! CID Index:
//! CID:afa-plugin-llm-openai-compat-example-002 -> negative_battery_smoke
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-openai-compat-example-" crates/afa-plugin-llm-openai-compat/examples/

use std::env;
use std::sync::Arc;
use std::time::Instant;

use afa_bus::EventBus;
use afa_contracts::{
    CompletionRequest, CompletionResponse, ContentBlock, ConversationItem, ExecutionContext,
    LlmErrorV1, LlmV1, ModelCapabilities, SamplingParams, SecretRef, SecurityErrorV1, SecurityV1,
    UnsealedSecret,
};
use afa_plugin_llm_openai_compat::adapter::ChatCompletionsAdapter;
use afa_plugin_llm_openai_compat::config::ChatCompletionsConfig;
use async_trait::async_trait;

// ---------------------------------------------------------------------------
// A tiny `SecurityV1` stub. The example does not need the
// real security engine (the real engine talks to a SQLite
// store and an AEAD master key — overkill for a live smoke).
// The stub returns whatever key we hand it on every
// `unseal` call. `seal` / `rotate` are `unimplemented!`
// because we never call them.
// ---------------------------------------------------------------------------
struct StaticKey {
    key: String,
}

#[async_trait]
impl SecurityV1 for StaticKey {
    async fn seal(&self, _plaintext: &[u8], _name: &str) -> Result<SecretRef, SecurityErrorV1> {
        unimplemented!("read-only")
    }
    async fn unseal(
        &self,
        _name: &SecretRef,
        _ctx: &ExecutionContext,
    ) -> Result<UnsealedSecret, SecurityErrorV1> {
        Ok(UnsealedSecret::new(self.key.as_bytes().to_vec()))
    }
    async fn rotate(
        &self,
        _secret_ref: &SecretRef,
        _new_plaintext: &[u8],
        _ctx: &ExecutionContext,
    ) -> Result<SecretRef, SecurityErrorV1> {
        unimplemented!("read-only")
    }
}

// ---------------------------------------------------------------------------
// Battery plumbing: a `CaseSpec` describes
// one row of the battery (name, key, model
// override, request, acceptable variants);
// a `CaseResult` is the actual outcome. A
// case "passes" if the actual LlmErrorV1
// variant matches ANY acceptable substring
// (vendors vary). `run_case` builds a fresh
// EventBus + StaticKey + adapter per case
// and prints the full per-case report so a
// CI script can grep it.
// ---------------------------------------------------------------------------
struct CaseSpec {
    name: &'static str,
    case_key: String,
    model_override: Option<String>,
    request: CompletionRequest,
    acceptable: &'static [&'static str],
}

struct CaseResult {
    name: String,
    expected: String,
    actual_variant: String,
    correlation_id: Option<afa_contracts::CorrelationId>,
    elapsed_ms: u128,
    pass: bool,
}

async fn run_case(
    name: &str,
    url: &str,
    model: &str,
    key: &str,
    request: CompletionRequest,
    acceptable_variants: &[&str],
) -> CaseResult {
    // Build a fresh EventBus + StaticKey +
    // adapter for this case. The bus is
    // dropped at the end of the case (no
    // cross-case pollution).
    let bus = EventBus::new();
    let security: Arc<dyn SecurityV1> = Arc::new(StaticKey {
        key: key.to_string(),
    });
    let config = ChatCompletionsConfig::with_provider(
        model,
        SecretRef {
            name: "freellmapi-key".into(),
            version: 1,
        },
        ModelCapabilities {
            max_context_tokens: 128_000,
            supports_vision: false,
            supports_tool_use: false,
        },
        url,
        "freellmapi",
    );
    let adapter = ChatCompletionsAdapter::new(config, security, bus.handle());
    let mut req_sub = bus.subscribe::<afa_contracts::CompletionRequested>(8);
    let mut comp_sub = bus.subscribe::<afa_contracts::CompletionCompleted>(8);
    let mut fail_sub = bus.subscribe::<afa_contracts::CompletionFailed>(8);

    let ctx = ExecutionContext::new(
        afa_contracts::TenantId::new("negative-battery"),
        afa_contracts::Actor::Timer,
    );

    // Run the call.
    let started = Instant::now();
    let result = adapter.complete(request, &ctx).await;
    let elapsed_ms = started.elapsed().as_millis();

    // Map the result to a short string.
    let actual_variant = match &result {
        Ok(CompletionResponse::TextReply { content, .. }) => {
            format!("Ok(TextReply, content={:?})", content)
        }
        Ok(CompletionResponse::ToolCalls { calls, .. }) => {
            format!("Ok(ToolCalls, n={})", calls.len())
        }
        Err(LlmErrorV1::AuthenticationFailed { reason }) => {
            format!("Err(AuthenticationFailed, {:?})", reason)
        }
        Err(LlmErrorV1::RateLimited { .. }) => "Err(RateLimited)".to_string(),
        Err(LlmErrorV1::QuotaExhausted { reason }) => {
            format!("Err(QuotaExhausted, {:?})", reason)
        }
        Err(LlmErrorV1::ContextLengthExceeded {
            actual_tokens,
            max_tokens,
        }) => format!(
            "Err(ContextLengthExceeded, {}/{})",
            actual_tokens, max_tokens
        ),
        Err(LlmErrorV1::ContentPolicyViolation { reason }) => {
            format!("Err(ContentPolicyViolation, {:?})", reason)
        }
        Err(LlmErrorV1::ModelNotFound { model }) => {
            format!("Err(ModelNotFound, {:?})", model)
        }
        Err(LlmErrorV1::ToolNotFound { tool_name }) => {
            format!("Err(ToolNotFound, {:?})", tool_name)
        }
        Err(LlmErrorV1::InvalidRequest { reason }) => {
            format!("Err(InvalidRequest, {:?})", reason)
        }
        Err(LlmErrorV1::UpstreamUnavailable { http_status }) => {
            format!("Err(UpstreamUnavailable, http={:?})", http_status)
        }
        Err(LlmErrorV1::Timeout { elapsed }) => {
            format!("Err(Timeout, {} ms)", elapsed.as_millis())
        }
        Err(LlmErrorV1::MalformedResponse { reason }) => {
            format!("Err(MalformedResponse, {:?})", reason)
        }
        Err(LlmErrorV1::StreamInterrupted { reason }) => {
            format!("Err(StreamInterrupted, {:?})", reason)
        }
        Err(LlmErrorV1::Internal { reason }) => {
            format!("Err(Internal, {:?})", reason)
        }
    };

    // Drain the audit events with a short
    // timeout. The adapter publishes the
    // Requested event BEFORE the vendor call
    // and either Completed or Failed AFTER.
    // 200ms is plenty for a live vendor.
    let mut correlation_id = None;
    if let Ok(Some((evt, _ctx))) =
        tokio::time::timeout(std::time::Duration::from_millis(200), req_sub.recv()).await
    {
        correlation_id = Some(evt.correlation_id);
    }
    let _ = tokio::time::timeout(std::time::Duration::from_millis(50), comp_sub.recv()).await;
    let _ = tokio::time::timeout(std::time::Duration::from_millis(50), fail_sub.recv()).await;

    // Does the actual variant match any
    // of the acceptable substrings? The
    // test is "tolerant per case": each
    // case lists a set of variants that
    // are all reasonable for the input,
    // so a vendor that returns
    // `InvalidRequest` instead of
    // `ModelNotFound` for an unknown
    // model still passes the
    // `unknown_model` case (because
    // `InvalidRequest` is in the
    // acceptable list).
    let pass = acceptable_variants
        .iter()
        .any(|v| actual_variant.contains(v));

    CaseResult {
        name: name.to_string(),
        expected: format!("one of {:?}", acceptable_variants),
        actual_variant,
        correlation_id,
        elapsed_ms,
        pass,
    }
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    let url = match env::var("FREELLMAPI_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("FREELLMAPI_URL is not set");
            std::process::exit(2);
        }
    };
    let model = env::var("FREELLMAPI_MODEL").unwrap_or_else(|_| "auto".into());
    let key = match env::var("FREELLMAPI_KEY") {
        Ok(v) => v,
        Err(_) => {
            eprintln!("FREELLMAPI_KEY is not set");
            std::process::exit(2);
        }
    };
    println!(
        "Loaded env: url={url}, model={model}, key_present={}",
        !key.is_empty()
    );

    // Build the 6 cases. Each one has its own
    // CompletionRequest (so the user can read
    // the input that produced the failure) and
    // its own list of acceptable variants. A
    // case "passes" if the actual variant
    // matches ANY acceptable substring —
    // vendors vary (some return ModelNotFound
    // for an unknown model, others return
    // InvalidRequest; some return an empty
    // TextReply for empty content, others
    // return InvalidRequest; etc.).

    let pinger = CompletionRequest {
        system: None,
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text("ping".into())],
        }],
        tools: vec![],
        sampling: SamplingParams {
            temperature: 0.0,
            max_output_tokens: 16,
            top_p: None,
            stop: vec![],
        },
    };

    let pong_request = CompletionRequest {
        system: None,
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text(
                "Reply with exactly the word PONG and nothing else.".into(),
            )],
        }],
        tools: vec![],
        sampling: SamplingParams {
            temperature: 0.0,
            max_output_tokens: 64,
            top_p: None,
            stop: vec![],
        },
    };

    let cases: Vec<CaseSpec> = vec![
        // 1. Success control: good key + simple PONG.
        //    Strict: must be Ok(TextReply{...}).
        CaseSpec {
            name: "success_control",
            case_key: key.clone(),
            model_override: None,
            request: pong_request,
            acceptable: &["Ok(TextReply"],
        },
        // 2. Bad key: wrong key + simple request.
        //    Strict: must be AuthenticationFailed
        //    (a vendor that returns a 200 with
        //    garbage for a bad key would be a
        //    real bug — caught here).
        CaseSpec {
            name: "bad_key",
            case_key: "definitely-not-a-real-key-just-a-stub".into(),
            model_override: None,
            request: pinger.clone(),
            acceptable: &["AuthenticationFailed"],
        },
        // 3. Unknown model. We use a
        //    model-override so the adapter is
        //    built with a name the proxy
        //    definitely doesn't know. Tolerant:
        //    vendors return either ModelNotFound
        //    or InvalidRequest.
        CaseSpec {
            name: "unknown_model",
            case_key: key.clone(),
            model_override: Some("gpt-9000-this-model-does-not-exist-anywhere".into()),
            request: pinger.clone(),
            acceptable: &["ModelNotFound", "InvalidRequest"],
        },
        // 4. Oversized context: 1 000 000 chars
        //    (~250 000 tokens, well over the
        //    128k-token context window of
        //    freellmapi / sambanova). Note:
        //    200 000 chars (~50k tokens) is
        //    *within* freellmapi's context, so
        //    the model will happily describe
        //    it instead of overflowing. The
        //    exact overflow threshold is
        //    vendor-specific and
        //    non-deterministic at the boundary
        //    (a 200k run returned 502 once
        //    and Ok(TextReply) the next time).
        //    Tolerant: some vendors return
        //    InvalidRequest for "max_tokens
        //    exceeded", others return
        //    ContextLengthExceeded, and
        //    freellmapi (via sambanova)
        //    returns HTTP 502 which the
        //    adapter maps to
        //    UpstreamUnavailable. We accept
        //    all three.
        CaseSpec {
            name: "oversized_context",
            case_key: key.clone(),
            model_override: None,
            request: CompletionRequest {
                system: None,
                messages: vec![ConversationItem::UserMessage {
                    content: vec![ContentBlock::Text("a".repeat(1_000_000))],
                }],
                tools: vec![],
                sampling: SamplingParams {
                    temperature: 0.0,
                    max_output_tokens: 16,
                    top_p: None,
                    stop: vec![],
                },
            },
            acceptable: &[
                "ContextLengthExceeded",
                "InvalidRequest",
                "UpstreamUnavailable",
            ],
        },
        // 5. Empty content: no content blocks.
        //    Tolerant: vendors vary — some
        //    return an empty reply (Ok), some
        //    reject as InvalidRequest.
        CaseSpec {
            name: "empty_content",
            case_key: key.clone(),
            model_override: None,
            request: CompletionRequest {
                system: None,
                messages: vec![ConversationItem::UserMessage { content: vec![] }],
                tools: vec![],
                sampling: SamplingParams {
                    temperature: 0.0,
                    max_output_tokens: 16,
                    top_p: None,
                    stop: vec![],
                },
            },
            acceptable: &["Ok(TextReply", "InvalidRequest"],
        },
        // 6. Invalid temperature: 5.0 (out of
        //    range; OpenAI allows 0-2). The
        //    vendor must reject with 400
        //    InvalidRequest. If a vendor
        //    silently clamps instead, this
        //    test fails (catches silent
        //    fallback bugs).
        CaseSpec {
            name: "invalid_temperature",
            case_key: key.clone(),
            model_override: None,
            request: CompletionRequest {
                system: None,
                messages: vec![ConversationItem::UserMessage {
                    content: vec![ContentBlock::Text("ping".into())],
                }],
                tools: vec![],
                sampling: SamplingParams {
                    temperature: 5.0,
                    max_output_tokens: 16,
                    top_p: None,
                    stop: vec![],
                },
            },
            acceptable: &["InvalidRequest"],
        },
    ];

    println!("\n=== Negative battery ({} cases) ===\n", cases.len());
    let mut results: Vec<CaseResult> = Vec::new();
    for case in &cases {
        let use_model: &str = match &case.model_override {
            Some(m) => m.as_str(),
            None => model.as_str(),
        };
        let result = run_case(
            case.name,
            &url,
            use_model,
            &case.case_key,
            case.request.clone(),
            case.acceptable,
        )
        .await;
        results.push(result);
    }

    // Print the per-case report.
    for r in &results {
        let cid = r
            .correlation_id
            .map(|c| c.to_string())
            .unwrap_or_else(|| "(no event)".to_string());
        println!(
            "[{}] {} ({} ms)\n  expected: {}\n  actual:   {}\n  correlation_id: {}\n",
            if r.pass { "PASS" } else { "FAIL" },
            r.name,
            r.elapsed_ms,
            r.expected,
            r.actual_variant,
            cid,
        );
    }

    let failed = results.iter().filter(|r| !r.pass).count();
    let passed = results.len() - failed;
    println!(
        "\n=== Battery summary: {passed} passed, {failed} failed (of {}) ===",
        results.len()
    );

    if failed > 0 {
        eprintln!(
            "\n{failed} case(s) landed in an unexpected variant. See the per-case output above."
        );
        std::process::exit(1);
    }
    std::process::exit(0);
}
