//! Real-provider smoke test for `ChatCompletionsAdapter`.
//!
//! This example runs the **real** `ChatCompletionsAdapter`
//! against a **real** OpenAI-compatible vendor (the
//! canonical use case: the user's local `freellmapi`
//! proxy on `http://localhost:3001/v1`). It is **not**
//! hermetic — it makes a live HTTP call — so it lives in
//! `examples/` (cargo builds it on demand; CI does not
//! run it).
//!
//! ## What it does
//!
//! 1. Reads the vendor URL, model name, and API key from
//!    env vars (`FREELLMAPI_URL`, `FREELLMAPI_MODEL`,
//!    `FREELLMAPI_KEY`).
//! 2. Builds a `ChatCompletionsConfig` bound to that URL.
//! 3. Builds a `StaticSecurity` (a fake `SecurityV1` that
//!    returns the env-var key on every `unseal` call).
//!    A real deployment uses the production security
//!    engine and pre-seals the key at startup; this
//!    example skips the seal step for ergonomics.
//! 4. Sends a `complete` request asking for "PONG".
//! 5. Prints the response + the 3 audit events received
//!    on the bus.
//!
//! ## How to run it
//!
//! ```bash
//! FREELLMAPI_URL="http://localhost:3001/v1" \
//! FREELLMAPI_MODEL="auto" \
//! FREELLMAPI_KEY="..." \
//! cargo run --example real_provider_smoke -p afa-plugin-llm-openai-compat
//! ```
//!
//! ## Safety
//!
//! The API key is read from the env, never hard-coded.
//! The example prints `Loaded key: true|false` (boolean
//! only) so a user can verify the env was set without
//! the key itself landing in the terminal log.
//!
//!
//! Story (plain English): The "Lend Your Voice"
//! specialist normally only practices against a fake
//! vendor down the hall (wiremock-rs). This example
//! is the day the specialist goes to the real customer
//! site. The user gives the specialist an address, a
//! model name, and a key; the specialist talks to the
//! real vendor the same way it talks to the fake one.
//! The point of the run is to prove the wire mapping
//! and the audit-event order work against a real
//! server, not just a drill. The example is opt-in
//! (cargo does not run it in CI; the user runs it
//! locally with `cargo run --example`).
//!
//! CID Index:
//! CID:afa-plugin-llm-openai-compat-example-001 -> real_provider_smoke
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-openai-compat-example-" crates/afa-plugin-llm-openai-compat/examples/

use std::env;
use std::sync::Arc;

use afa_bus::EventBus;
use afa_contracts::{
    CompletionCompleted, CompletionFailed, CompletionRequest, CompletionResponse, ContentBlock,
    ConversationItem, ExecutionContext, LlmErrorV1, LlmV1, ModelCapabilities, SecretRef,
    SecurityErrorV1, SecurityV1, UnsealedSecret,
};
use afa_plugin_llm_openai_compat::adapter::ChatCompletionsAdapter;
use afa_plugin_llm_openai_compat::config::ChatCompletionsConfig;
use async_trait::async_trait;
use chrono::Utc;

/// A fake `SecurityV1` that returns a
/// hard-coded key on every `unseal`
/// call. In production the security
/// engine is real; this example skips
/// the seal step so the user can run
/// it with just an env-var key. The
/// `unseal` result is wrapped in
/// `UnsealedSecret` (a `Zeroizing<Vec<u8>>`)
/// so the key is wiped on drop.
///
/// `seal` and `rotate` are `unimplemented!`
/// — this example never calls them. A
/// real deployment uses the production
/// security engine and pre-seals the key
/// at startup.
struct StaticKey {
    key: String,
}

#[async_trait]
impl SecurityV1 for StaticKey {
    async fn seal(&self, _plaintext: &[u8], _name: &str) -> Result<SecretRef, SecurityErrorV1> {
        // This example is read-only: it
        // never seals a secret (the user
        // passes the key via env var).
        // Calling seal in this example
        // would be a logic bug.
        unimplemented!("StaticKey does not seal — example is read-only")
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
        // Same as `seal` — this example
        // is read-only. A real
        // deployment would call the
        // production engine to rotate
        // the sealed key.
        unimplemented!("StaticKey does not rotate — example is read-only")
    }
}

/// Drain one bus subscription
/// with a 100ms timeout and
/// print the correlation_id of
/// the first event delivered
/// (if any). Prints a fallback
/// line if no event arrives
/// in time, or if the channel
/// is closed.
///
/// `$name` is the human label
/// printed in the output.
/// `$sub` is the typed
/// `Subscription<T>`. The
/// macro is the only practical
/// way to do this for three
/// different concrete
/// subscription types without
/// a custom `HasCorrelationId`
/// trait or a `dyn`-dispatch
/// helper (the bus API does
/// not expose either, by
/// design — see the comment
/// block in `main` above the
/// call site).
macro_rules! drain_audit {
    ($name:expr, $sub:ident) => {
        match tokio::time::timeout(std::time::Duration::from_millis(100), $sub.recv()).await {
            Ok(Some((evt, _ctx))) => {
                println!(
                    "{}: ts={} correlation_id={}",
                    $name,
                    Utc::now().to_rfc3339(),
                    evt.correlation_id
                );
            }
            Ok(None) => println!("{}: channel closed", $name),
            Err(_) => println!("{}: (no event received in 100ms)", $name),
        }
    };
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    // Install a basic tracing
    // subscriber so audit-event
    // log lines are visible.
    // Idempotent: if a global
    // subscriber is already
    // installed (e.g. by another
    // tool), we skip.
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info,afa=debug")),
        )
        .try_init();

    // Step 1: read the env vars.
    // All three are required; the
    // example bails out with a
    // clear message if any is
    // missing. The key is read
    // but **never printed**.
    let url = match env::var("FREELLMAPI_URL") {
        Ok(v) => v,
        Err(_) => {
            eprintln!(
                "FREELLMAPI_URL is not set. Example usage:\n  \
                 FREELLMAPI_URL=http://localhost:3001/v1\n  \
                 FREELLMAPI_MODEL=auto\n  \
                 FREELLMAPI_KEY=... cargo run --example real_provider_smoke -p afa-plugin-llm-openai-compat"
            );
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

    // Step 2: build the config. The
    // adapter appends `/chat/completions`
    // to `base_url`, so the user's
    // `http://localhost:3001/v1`
    // works as-is. The `provider_name`
    // lands on every audit event so a
    // dashboard can group by provider.
    let config = ChatCompletionsConfig::with_provider(
        &model,
        SecretRef {
            name: "freellmapi-key".into(),
            version: 1,
        },
        ModelCapabilities {
            max_context_tokens: 128_000,
            supports_vision: false,
            supports_tool_use: true,
        },
        &url,
        "freellmapi",
    );

    // Step 3: build the security
    // engine (a `StaticKey` that
    // returns the env-var key) and
    // the event bus.
    let security: Arc<dyn SecurityV1> = Arc::new(StaticKey { key });
    let bus = EventBus::new();
    let mut req_sub = bus.subscribe::<afa_contracts::CompletionRequested>(8);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(8);
    let mut fail_sub = bus.subscribe::<CompletionFailed>(8);

    // Step 4: build the adapter and
    // send the request. The
    // `ExecutionContext::new`
    // already generates a fresh
    // `CorrelationId` (a UUID
    // v4). The context carries
    // the tenant id (so a
    // multi-tenant deployment
    // can tell which customer
    // asked) and the actor (so
    // the audit log can
    // distinguish a customer
    // from a cron job).
    let adapter = ChatCompletionsAdapter::new(config, security, bus.handle());
    let ctx = ExecutionContext::new(
        afa_contracts::TenantId::new("smoke-test"),
        afa_contracts::Actor::Human {
            via: "real_provider_smoke".into(),
        },
    );
    let request = CompletionRequest {
        system: None,
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text(
                "Reply with exactly the word PONG and nothing else.".into(),
            )],
        }],
        tools: vec![],
        sampling: afa_contracts::SamplingParams {
            temperature: 0.0_f32,
            max_output_tokens: 64,
            top_p: None,
            stop: vec![],
        },
    };

    let started = std::time::Instant::now();
    let result = adapter.complete(request, &ctx).await;
    let elapsed = started.elapsed();

    // Step 5: print the result + the
    // 3 audit events. We use
    // `tokio::time::timeout` on the
    // bus subscriptions because the
    // events may have already been
    // published by the time we get
    // here (the adapter publishes
    // them BEFORE the vendor call,
    // so they are buffered on the
    // subscription).
    println!("\n=== Response (elapsed: {} ms) ===", elapsed.as_millis());
    match &result {
        Ok(CompletionResponse::TextReply { content, usage }) => {
            println!("content: {content:?}");
            println!(
                "usage:   prompt={}, completion={}",
                usage.prompt_tokens, usage.completion_tokens
            );
        }
        Ok(CompletionResponse::ToolCalls { calls, usage }) => {
            println!("tool_calls: {} call(s)", calls.len());
            for c in calls {
                println!("  - {} ({})", c.name, c.id);
            }
            println!(
                "usage:   prompt={}, completion={}",
                usage.prompt_tokens, usage.completion_tokens
            );
        }
        Err(e) => match e {
            LlmErrorV1::AuthenticationFailed { reason } => println!("auth failed: {reason}"),
            LlmErrorV1::RateLimited { .. } => println!("rate limited"),
            LlmErrorV1::QuotaExhausted { reason } => println!("quota exhausted: {reason}"),
            LlmErrorV1::ContextLengthExceeded {
                actual_tokens,
                max_tokens,
            } => println!("context too long: {actual_tokens}/{max_tokens}"),
            LlmErrorV1::ContentPolicyViolation { reason } => {
                println!("content policy: {reason}")
            }
            LlmErrorV1::ModelNotFound { model } => println!("model not found: {model}"),
            LlmErrorV1::ToolNotFound { tool_name } => println!("tool not found: {tool_name}"),
            LlmErrorV1::InvalidRequest { reason } => println!("invalid request: {reason}"),
            LlmErrorV1::UpstreamUnavailable { http_status } => {
                println!("upstream unavailable: http={http_status:?}")
            }
            LlmErrorV1::Timeout { elapsed } => println!("timeout: {} ms", elapsed.as_millis()),
            LlmErrorV1::MalformedResponse { reason } => {
                println!("malformed response: {reason}")
            }
            LlmErrorV1::StreamInterrupted { reason } => {
                println!("stream interrupted: {reason}")
            }
            LlmErrorV1::Internal { reason } => println!("internal: {reason}"),
        },
    }

    // Drain the audit-event
    // subscriptions. We wait up to
    // 100ms for each (events are
    // published synchronously by
    // the adapter; they should
    // already be in the channel
    // buffer). The bus returns
    // `Option<(Arc<T>,
    // ExecutionContext)>` — `None`
    // means the channel is closed.
    //
    // We CANNOT loop over the three
    // subs in a single `for`
    // because `Subscription<T>` is
    // a generic type — `&mut
    // Subscription<CompletionRequested>`,
    // `&mut Subscription<CompletionCompleted>`,
    // and `&mut
    // Subscription<CompletionFailed>`
    // are three different concrete
    // types and a homogeneous array
    // of `&mut dyn` references would
    // require an `async fn` on a
    // trait object, which the
    // current bus API does not
    // expose. Instead we drain each
    // subscription with its own
    // typed match.
    println!("\n=== Audit events ===");
    drain_audit!("CompletionRequested", req_sub);
    drain_audit!("CompletionCompleted", comp_sub);
    drain_audit!("CompletionFailed", fail_sub);

    // Final exit code: 0 on
    // `Ok(CompletionResponse::
    // TextReply { content, .. })`
    // where content contains
    // "PONG" (case-insensitive).
    // This makes the example
    // scriptable for CI-style
    // integration runs.
    let success = matches!(
        &result,
        Ok(CompletionResponse::TextReply { content, .. })
            if content.to_uppercase().contains("PONG")
    );
    println!(
        "\n=== Smoke test result: {} ===",
        if success { "PASS" } else { "FAIL" }
    );
    std::process::exit(if success { 0 } else { 1 });
}
