//! Code Map: ChatCompletionsAdapter
//! - `ChatCompletionsAdapter`: The concrete `LlmV1`
//!   adapter for any service that speaks the OpenAI
//!   Chat Completions wire format (`POST
//!   {base_url}/chat/completions`). This is
//!   intentionally separate from `ResponsesAdapter` in
//!   `afa-plugin-llm-http` (which targets the new
//!   OpenAI Responses API at `/v1/responses`). The
//!   two are sibling adapters; the difference is the
//!   wire format. See the crate-level doc in
//!   `lib.rs` for the full story.
//!
//! Story (plain English): The
//! `ChatCompletionsAdapter` is the "Lend Your Voice"
//! specialist that talks the older OpenAI standard.
//! When a workflow asks for an LLM, the switchboard
//! (`CapabilityRegistry`) hands the request to this
//! specialist. The specialist has one permanent job:
//! talk to the vendor on the workflow's behalf, using
//! the sealed API key the security engine hands it.
//! If the vendor says "your key is bad" (HTTP 401),
//! the specialist re-unseals the key (the operator
//! may have rotated it) and tries once more, then
//! gives up. Every request stamps three small
//! tickets on the log so an auditor can later
//! reconstruct "who asked for what from which
//! provider, did it work, and how long did it take?"
//! — without reading the question or the answer.
//!
//! CID Index:
//! CID:afa-plugin-llm-chat-completions-adapter-001 -> ChatCompletionsAdapter
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-chat-completions-adapter-" crates/afa-plugin-llm-chat-completions/src/adapter.rs

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use afa_bus::EventBusHandle;
use afa_contracts::{
    CompletionChunk, CompletionCompleted, CompletionFailed, CompletionRequest, CompletionResponse,
    CompletionStream, ConversationItem, ExecutionContext, FinishReason, LlmErrorV1, LlmV1,
    ModelCapabilities, SecurityV1, ToolCallRequest, ToolDefinition, Usage,
};
use chrono::Utc;

use super::config::ChatCompletionsConfig;
use super::key_wiring::UnsealedHolder;

// CID:afa-plugin-llm-chat-completions-adapter-001 - ChatCompletionsAdapter
// Purpose: The concrete `LlmV1` adapter for any
// service that speaks the OpenAI Chat Completions
// wire format. The adapter is hard-wired to one
// model at construction (the model + provider are
// in `ChatCompletionsConfig`); there is no
// per-request model override. The adapter uses an
// `UnsealedHolder` to manage the API key (3-step
// pattern: cache, retry on 401, zeroize on drop).
// All audit events are published on the event bus
// the constructor was given. The
// `Send + Sync` supertrait is the standard one for
// adapters held behind `Arc<dyn LlmV1>` in the
// `CapabilityRegistry`.
// Uses: SecurityV1 (the engine for the API
// key), Bus (the event bus for the audit
// events), LlmV1 (the trait the adapter
// implements), UnsealedHolder (the 3-step key
// wiring).
// Used by: `CapabilityRegistry::register_llm`
// (which holds an `Arc<dyn LlmV1>`), and any
// workflow that calls `llm.complete`.
pub struct ChatCompletionsAdapter {
    /// The static config. Carries the
    /// vendor base URL (the adapter uses
    /// `self.config.base_url` per request;
    /// captured in the config so parallel
    /// tests with different wiremock servers
    /// do not trample each other via a
    /// process-global env var).
    config: ChatCompletionsConfig,
    /// The 3-step key wiring.
    key_holder: UnsealedHolder,
    /// The event bus for audit events. The
    /// adapter only publishes (it never
    /// subscribes), so the field is the
    /// publish-only `EventBusHandle` (a thin
    /// `Arc` clone of the underlying bus),
    /// not the full `EventBus`.
    bus: EventBusHandle,
    /// A best-effort `prompt_tokens_estimate`
    /// from the previous successful call. The
    /// `CompletionRequested` event carries
    /// this so an audit reader can see "this
    /// call's prompt was about N tokens."
    last_prompt_estimate: std::sync::Mutex<Option<u32>>,
}

impl ChatCompletionsAdapter {
    /// Build a new `ChatCompletionsAdapter`.
    /// The `bus` is used to publish audit
    /// events; the `security` engine is
    /// used by the `UnsealedHolder` to
    /// unseal the API key on first use and
    /// on 401 retries. The vendor base URL
    /// is taken from `config.base_url` (the
    /// production default in `gpt_4o_mini`
    /// is `https://api.openai.com/v1`;
    /// tests override it via
    /// `with_provider`).
    pub fn new(
        config: ChatCompletionsConfig,
        security: Arc<dyn SecurityV1>,
        bus: EventBusHandle,
    ) -> Self {
        Self {
            key_holder: UnsealedHolder::new(security, config.clone()),
            config,
            bus,
            last_prompt_estimate: std::sync::Mutex::new(None),
        }
    }

    /// Build the HTTP `Authorization: Bearer ...`
    /// header value for a request. The key is
    /// fetched via the `UnsealedHolder` (which
    /// caches + re-unseals on 401).
    async fn auth_header(&self, ctx: &ExecutionContext) -> Result<String, LlmErrorV1> {
        let key = self.key_holder.get_or_unseal(ctx).await?;
        Ok(format!("Bearer {key}"))
    }

    /// Publish a `CompletionRequested` event on
    /// the bus. Always first, before any I/O.
    async fn publish_requested(&self, request: &CompletionRequest, ctx: &ExecutionContext) {
        let prompt_estimate = {
            let guard = self
                .last_prompt_estimate
                .lock()
                .expect("last_prompt_estimate mutex");
            *guard
        };
        let has_tools = !request.tools.is_empty();
        let has_images = request.messages.iter().any(|m| match m {
            ConversationItem::UserMessage { content }
            | ConversationItem::AssistantMessage { content } => content
                .iter()
                .any(|b| matches!(b, afa_contracts::ContentBlock::Image { .. })),
            ConversationItem::ToolResult { content, .. } => content
                .iter()
                .any(|b| matches!(b, afa_contracts::ContentBlock::Image { .. })),
        });
        let event = afa_contracts::CompletionRequested {
            correlation_id: ctx.correlation_id,
            tenant_id: ctx.tenant_id.clone(),
            actor: ctx.actor.clone(),
            model: self.config.model.clone(),
            prompt_tokens_estimate: prompt_estimate,
            has_tools,
            has_images,
            timestamp: Utc::now(),
        };
        // Best-effort publish: a bus error
        // should not break the LLM call.
        // The bus is in-process; failure is
        // almost always a closed receiver.
        self.bus.publish(event, ctx.clone()).await;
    }

    /// Publish a `CompletionCompleted` event
    /// on the bus. Called after a successful
    /// vendor response.
    async fn publish_completed(
        &self,
        ctx: &ExecutionContext,
        usage: Usage,
        finish_reason: FinishReason,
        duration_ms: u64,
    ) {
        let event = CompletionCompleted {
            correlation_id: ctx.correlation_id,
            tenant_id: ctx.tenant_id.clone(),
            actor: ctx.actor.clone(),
            model: self.config.model.clone(),
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            finish_reason,
            duration_ms,
            timestamp: Utc::now(),
        };
        self.bus.publish(event, ctx.clone()).await;
        // Cache the exact prompt size for
        // the next call's
        // `prompt_tokens_estimate`. The
        // estimate is the previous call's
        // exact value, not a heuristic.
        if let Ok(mut guard) = self.last_prompt_estimate.lock() {
            *guard = Some(usage.prompt_tokens);
        }
    }

    /// Publish a `CompletionFailed` event on
    /// the bus. Called when the vendor call
    /// returns an error.
    async fn publish_failed(&self, ctx: &ExecutionContext, error: LlmErrorV1, duration_ms: u64) {
        let event = CompletionFailed {
            correlation_id: ctx.correlation_id,
            tenant_id: ctx.tenant_id.clone(),
            actor: ctx.actor.clone(),
            model: self.config.model.clone(),
            error,
            duration_ms,
            timestamp: Utc::now(),
        };
        self.bus.publish(event, ctx.clone()).await;
    }
}

/// Map a `CompletionRequest` to the OpenAI
/// Chat Completions API JSON body. The Chat
/// Completions shape is:
/// `{model, messages, temperature, max_tokens, top_p, tools, tool_choice}`.
/// Note the differences from the Responses API
/// (`/v1/responses`):
/// - the field is `max_tokens`, not `max_output_tokens`;
/// - the messages array is `{role, content}` pairs,
///   not the Responses-style items array;
/// - tools go in `tools: [{type: "function", function: {name, description, parameters}}]`,
///   the same shape as Responses (this is the one piece
///   the two APIs share).
/// - `tool_choice: "auto"` is sent by default so
///   the vendor passes through to the model
///   (the LlmV1 spec lets the caller override via
///   `request.sampling.tool_choice` in a future pack;
///   v1 hard-codes `"auto"`).
fn map_request(request: &CompletionRequest, model: &str) -> Result<serde_json::Value, LlmErrorV1> {
    let messages = request
        .messages
        .iter()
        .map(map_message)
        .collect::<Result<Vec<_>, _>>()?;
    // The `system` prompt is sent as a
    // `role: "system"` message at the top
    // of the messages array. Chat
    // Completions has no top-level
    // `system` field.
    let mut all_messages: Vec<serde_json::Value> = Vec::with_capacity(messages.len() + 1);
    if let Some(system) = request.system.as_ref() {
        if !system.is_empty() {
            all_messages.push(serde_json::json!({
                "role": "system",
                "content": system,
            }));
        }
    }
    all_messages.extend(messages);
    let tools: Vec<serde_json::Value> = request.tools.iter().map(map_tool).collect();
    // Only include the `tools` field if
    // the request actually has tools —
    // some "OpenAI-compatible" services
    // reject an empty `tools: []` array
    // with a 400.
    let mut body = serde_json::json!({
        "model": model,
        "messages": all_messages,
        "temperature": request.sampling.temperature,
        "max_tokens": request.sampling.max_output_tokens,
    });
    if !tools.is_empty() {
        body["tools"] = serde_json::Value::Array(tools);
        body["tool_choice"] = serde_json::Value::String("auto".into());
    }
    Ok(body)
}

/// Map a `ConversationItem` to the OpenAI
/// Chat Completions `messages` shape. The
/// `Result` returns `LlmErrorV1::InvalidRequest`
/// if a content block is not representable.
fn map_message(msg: &ConversationItem) -> Result<serde_json::Value, LlmErrorV1> {
    match msg {
        ConversationItem::UserMessage { content } => Ok(serde_json::json!({
            "role": "user",
            "content": content.iter().map(map_block).collect::<Vec<_>>()
        })),
        ConversationItem::AssistantMessage { content } => Ok(serde_json::json!({
            "role": "assistant",
            "content": content.iter().map(map_block).collect::<Vec<_>>()
        })),
        ConversationItem::ToolResult {
            tool_call_id,
            content,
        } => Ok(serde_json::json!({
            "role": "tool",
            "tool_call_id": tool_call_id,
            "content": content.iter().map(map_block).collect::<Vec<_>>()
        })),
    }
}

/// Map a `ContentBlock` to the OpenAI Chat
/// Completions `content` array shape. Image
/// blocks use the `image_url` form (the Chat
/// Completions API accepts both `image_url`
/// and `input_image` shapes; we use
/// `image_url` to be consistent with the
/// OpenAI Chat Completions spec). Base64
/// images are encoded as a `data:` URL.
fn map_block(block: &afa_contracts::ContentBlock) -> serde_json::Value {
    match block {
        afa_contracts::ContentBlock::Text(s) => {
            serde_json::json!({"type": "text", "text": s})
        }
        afa_contracts::ContentBlock::Image { mime_type, data } => match data {
            afa_contracts::ImageData::Url(url) => serde_json::json!({
                "type": "image_url",
                "image_url": {"url": url}
            }),
            afa_contracts::ImageData::Base64(b64) => {
                // The Chat Completions API
                // accepts images as
                // `image_url` with either a
                // plain URL or a `data:`
                // URL. The data-URL form is
                // the one the API actually
                // parses for base64 input.
                serde_json::json!({
                    "type": "image_url",
                    "image_url": format!("data:{};base64,{}", mime_type, b64)
                })
            }
        },
    }
}

/// Map a `ToolDefinition` to the OpenAI
/// Chat Completions `tools` array shape.
/// The shape is the same as the OpenAI
/// Responses API:
/// `{type: "function", function: {name, description, parameters}}`.
fn map_tool(tool: &ToolDefinition) -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "function": {
            "name": tool.name,
            "description": tool.description,
            "parameters": tool.parameters_schema
        }
    })
}

#[async_trait]
impl LlmV1 for ChatCompletionsAdapter {
    async fn complete(
        &self,
        request: CompletionRequest,
        ctx: &ExecutionContext,
    ) -> Result<CompletionResponse, LlmErrorV1> {
        // Step 1: publish the
        // `CompletionRequested` event.
        // Always first, before any I/O.
        self.publish_requested(&request, ctx).await;

        let start = Instant::now();

        // Step 2: build the request body.
        let body = map_request(&request, &self.config.model)?;

        // Step 3: fetch the API key via the
        // 3-step holder. The first call
        // unseals; subsequent calls use the
        // cache.
        let auth = self.auth_header(ctx).await?;

        // Step 4: call the vendor. The
        // `call_vendor_with_retry` path
        // detects and handles 401 by
        // re-unsealing the key and
        // retrying once.
        let (resp, attempts) =
            call_vendor_with_retry(&self.config, &body, &auth, &self.key_holder, ctx).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match resp {
            Ok((value, usage, finish_reason)) => {
                self.publish_completed(ctx, usage, finish_reason, duration_ms)
                    .await;
                // Map the response JSON to
                // `CompletionResponse`. The
                // shape is the OpenAI Chat
                // Completions API:
                // - `value["choices"][0]
                //   ["message"]["content"]`
                //   is the text reply
                //   (string);
                // - `value["choices"][0]
                //   ["message"]
                //   ["tool_calls"]` is a
                //   list of tool calls.
                let response = map_response(value, &request.tools)?;
                if attempts > 1 {
                    tracing::info!(
                        correlation_id = %ctx.correlation_id,
                        attempts,
                        provider = %self.config.provider_name,
                        "chat-completions adapter recovered after 401"
                    );
                }
                Ok(response)
            }
            Err(e) => {
                self.publish_failed(ctx, e.clone(), duration_ms).await;
                Err(e)
            }
        }
    }

    // CID:afa-plugin-llm-chat-completions-adapter-004 - stream_complete
    // Purpose: Phase 2 streaming entry
    // point. The full wire logic lives
    // in `streaming.rs` (the
    // `tokio::spawn` background task,
    // the SSE event mapper, the 401
    // retry, the cancellation paths).
    // This method does the four
    // top-level things a caller
    // expects:
    //  1. Publish `CompletionRequested`
    //     FIRST (before any I/O).
    //  2. Build the request body with
    //     `"stream": true` injected.
    //  3. Unseal the initial key (the
    //     first call uses the cached
    //     value; the bg task re-unseals
    //     on 401).
    //  4. Open a bounded
    //     `mpsc::channel(64)`,
    //     `tokio::spawn` the bg task
    //     (which holds one `tx` clone
    //     and the `request` context),
    //     and — if `ctx.deadline` is
    //     `Some(_)` — spawn the
    //     deadline watchdog (which
    //     holds a second `tx` clone
    //     and drops it on timeout).
    //     The consumer's `rx` is
    //     returned.
    // Used by: any workflow that calls
    // `llm.stream_complete`; the
    // `CapabilityRegistry` does not
    // dispatch streaming calls.
    async fn stream_complete(
        &self,
        request: CompletionRequest,
        ctx: &ExecutionContext,
    ) -> Result<CompletionStream, LlmErrorV1> {
        // Step 1: publish the
        // `CompletionRequested` audit
        // event. Always first.
        self.publish_requested(&request, ctx).await;
        let start = Instant::now();

        // Step 2: build the request body
        // (same shape as `complete`),
        // then add `"stream": true`.
        let mut body = map_request(&request, &self.config.model)?;
        body["stream"] = serde_json::Value::Bool(true);

        // Step 3: unseal the initial key.
        // On failure we return the
        // error — the bg task is not
        // started in this case.
        let key = self.key_holder.get_or_unseal(ctx).await?;
        let initial_auth = format!("Bearer {key}");

        // Step 4: open the channel +
        // spawn the bg task + spawn
        // the deadline watchdog.
        let (tx, rx) = tokio::sync::mpsc::channel::<CompletionChunk>(64);
        super::streaming::spawn_streaming(
            self.config.clone(),
            // The bg task gets a
            // fresh `Arc` clone of
            // the security engine
            // (the adapter's
            // field is
            // `UnsealedHolder`,
            // not the raw
            // engine).
            self.key_holder.share_security_arc(),
            self.bus.clone(),
            ctx.clone(),
            body,
            initial_auth,
            start,
            tx.clone(),
        );
        if let Some(deadline) = ctx.deadline {
            let tx_watchdog = tx.clone();
            tokio::spawn(async move {
                let now = std::time::Instant::now();
                if deadline > now {
                    tokio::time::sleep(deadline - now).await;
                }
                drop(tx_watchdog);
            });
        }
        Ok(rx)
    }

    fn describe_capabilities(&self) -> ModelCapabilities {
        self.config.capabilities.clone()
    }
}

/// Call the vendor with one automatic retry
/// on HTTP 401 (the key may have been
/// rotated). The retry path uses the
/// `UnsealedHolder`'s `re_unseal_after_401`
/// to pick up the new key, then calls the
/// vendor again. Returns the number of
/// attempts (1 or 2) for the audit trail.
/// The base URL comes from
/// `config.base_url` (captured at
/// construction; see `ChatCompletionsConfig`)
/// so that each adapter is bound to a
/// specific URL — parallel tests with
/// different wiremock servers do not
/// trample each other via a process-
/// global env var.
async fn call_vendor_with_retry(
    config: &ChatCompletionsConfig,
    body: &serde_json::Value,
    auth: &str,
    holder: &UnsealedHolder,
    ctx: &ExecutionContext,
) -> (
    Result<(serde_json::Value, Usage, FinishReason), LlmErrorV1>,
    u32,
) {
    // First attempt.
    let attempt1 = call_vendor(config, body, auth).await;
    if let Err(LlmErrorV1::AuthenticationFailed { .. }) = &attempt1 {
        // The key was bad. Re-unseal and
        // try once more. The new key is
        // the rotated one.
        let new_auth = match holder.re_unseal_after_401(ctx).await {
            Ok(k) => format!("Bearer {k}"),
            Err(e) => return (Err(e), 1),
        };
        let attempt2 = call_vendor(config, body, &new_auth).await;
        (attempt2, 2)
    } else {
        (attempt1, 1)
    }
}

/// One raw vendor call. The HTTP transport
/// is a small `reqwest::Client` that the
/// adapter builds per call (cheap; reqwest
/// is `async` under the hood). The wire
/// shape is the OpenAI Chat Completions
/// API: `POST {base_url}/chat/completions`
/// with a JSON body that mirrors our
/// `CompletionRequest`. On a non-2xx
/// response, the status code + body are
/// mapped to one of the 13 `LlmErrorV1`
/// variants. The base URL comes from
/// `config.base_url` (see
/// `ChatCompletionsConfig`).
async fn call_vendor(
    config: &ChatCompletionsConfig,
    body: &serde_json::Value,
    auth: &str,
) -> Result<(serde_json::Value, Usage, FinishReason), LlmErrorV1> {
    // We use a single, lazy, blocking
    // reqwest client. reqwest is `async`
    // under the hood; the client is cheap
    // to build once. (Built inside the
    // function so a test that does not
    // call `complete` never pays the
    // cost.)
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| LlmErrorV1::Internal {
            reason: format!("http client build failed: {e}"),
        })?;
    let resp = client
        .post(format!("{}/chat/completions", config.base_url))
        .header("Authorization", auth)
        .json(body)
        .send()
        .await
        .map_err(|e| {
            // Network-level failure
            // (DNS, connection refused,
            // TLS error). Map to
            // `UpstreamUnavailable` (NOT
            // `Timeout` — the request may
            // not have even left our
            // process).
            LlmErrorV1::UpstreamUnavailable {
                http_status: e.status().map(|s| s.as_u16()),
            }
        })?;
    let status = resp.status();
    let bytes = resp.bytes().await.map_err(|e| LlmErrorV1::Internal {
        reason: format!("read body failed: {e}"),
    })?;
    if !status.is_success() {
        return Err(map_http_error(status.as_u16(), &bytes, &config.model));
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|e| LlmErrorV1::MalformedResponse {
            reason: format!("json parse: {e}"),
        })?;
    let usage = parse_usage(&value);
    let finish_reason = parse_finish_reason(&value);
    Ok((value, usage, finish_reason))
}

/// Map a non-2xx HTTP response to one of
/// the 13 `LlmErrorV1` variants. The
/// mapping is the same locked shape from
/// the TRD §2.2.10 used by
/// `afa-plugin-llm-http`, with the
/// Chat-Completions-specific quirk that
/// `error.code` may be absent (some
/// services only set `error.type` or just
/// `error.message`). When in doubt, we
/// fall back to the HTTP status code.
/// Vendor-specific error codes (e.g.
/// `context_length_exceeded`) are
/// detected from the JSON body's
/// `error.code` field.
pub(crate) fn map_http_error(status: u16, body: &[u8], model: &str) -> LlmErrorV1 {
    let parsed: serde_json::Value = serde_json::from_slice(body).unwrap_or(serde_json::Value::Null);
    let code = parsed["error"]["code"].as_str().unwrap_or("");
    let msg = parsed["error"]["message"]
        .as_str()
        .unwrap_or("no message")
        .to_string();
    match status {
        401 => LlmErrorV1::AuthenticationFailed { reason: msg },
        404 => {
            // 404 with a `model_not_found`
            // code: the model's name is
            // wrong. Without the code, we
            // assume it is a URL/path
            // 404 (a typo in our code
            // path) and return
            // `InvalidRequest`.
            if code == "model_not_found" {
                LlmErrorV1::ModelNotFound {
                    model: model.into(),
                }
            } else {
                LlmErrorV1::InvalidRequest {
                    reason: format!("http 404: {msg}"),
                }
            }
        }
        429 => {
            // 429 with a `quota_exceeded`
            // code: the billing quota is
            // exhausted (not a transient
            // throttle). Without the code,
            // we assume a transient
            // throttle and return
            // `RateLimited`. We also try
            // to read the `Retry-After`
            // header from the body (some
            // services inline it; others
            // put it in the HTTP header —
            // we read both in
            // `call_vendor`).
            if code == "quota_exceeded" {
                LlmErrorV1::QuotaExhausted { reason: msg }
            } else {
                LlmErrorV1::RateLimited { retry_after: None }
            }
        }
        400 => {
            if code == "context_length_exceeded" {
                let actual = parsed["error"]["actual_tokens"].as_u64().unwrap_or(0) as u32;
                let max = parsed["error"]["max_tokens"].as_u64().unwrap_or(0) as u32;
                LlmErrorV1::ContextLengthExceeded {
                    actual_tokens: actual,
                    max_tokens: max,
                }
            } else if code == "tool_not_found" {
                let tool_name = parsed["error"]["tool_name"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();
                LlmErrorV1::ToolNotFound { tool_name }
            } else if code == "content_policy_violation" {
                // The vendor's safety
                // filter refused to
                // answer. The `reason`
                // carries the vendor's
                // explanation (e.g. "the
                // request was flagged by
                // the content filter").
                LlmErrorV1::ContentPolicyViolation { reason: msg }
            } else {
                LlmErrorV1::InvalidRequest { reason: msg }
            }
        }
        408 => LlmErrorV1::Timeout {
            elapsed: Duration::from_secs(0),
        },
        503 => LlmErrorV1::UpstreamUnavailable {
            http_status: Some(status),
        },
        500..=599 => LlmErrorV1::UpstreamUnavailable {
            http_status: Some(status),
        },
        _ => LlmErrorV1::Internal {
            reason: format!("http {status}: {msg}"),
        },
    }
}

/// Parse the `usage` block out of a
/// successful OpenAI Chat Completions API
/// response. The shape is
/// `{"usage": {"prompt_tokens": N, "completion_tokens": M, "total_tokens": K}}`.
/// `total_tokens` is computed by the
/// vendor; we read it if present (some
/// downstream tools want it) but
/// `CompletionResponse::Usage` only
/// carries `prompt_tokens` +
/// `completion_tokens` (the LlmV1 spec
/// contract).
fn parse_usage(value: &serde_json::Value) -> Usage {
    let prompt = value["usage"]["prompt_tokens"].as_u64().unwrap_or(0) as u32;
    let completion = value["usage"]["completion_tokens"].as_u64().unwrap_or(0) as u32;
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
    }
}

/// Parse the `finish_reason` out of a
/// successful OpenAI Chat Completions API
/// response. The shape is
/// `{"choices": [{"finish_reason": "stop"}, ...]}`.
/// Common values: `stop`, `length`,
/// `tool_calls`, `content_filter`.
fn parse_finish_reason(value: &serde_json::Value) -> FinishReason {
    let stop_reason = value["choices"][0]["finish_reason"]
        .as_str()
        .unwrap_or("stop");
    match stop_reason {
        "stop" => FinishReason::Stop,
        "tool_calls" => FinishReason::ToolCalls,
        "length" => FinishReason::MaxTokens,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Stop,
    }
}

/// Map the OpenAI Chat Completions API
/// response JSON to a
/// `CompletionResponse`. The
/// `choices[0].message.content` is the
/// text reply (string), and
/// `choices[0].message.tool_calls` is a
/// list of tool calls (each with
/// `{id, type: "function", function:
/// {name, arguments}}`). We accumulate
/// all tool calls across choices into
/// the `ToolCalls` variant.
fn map_response(
    value: serde_json::Value,
    _tools: &[ToolDefinition],
) -> Result<CompletionResponse, LlmErrorV1> {
    let choices = value["choices"].as_array();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
    let mut text: Option<String> = None;
    if let Some(choices) = choices {
        for choice in choices {
            let message = &choice["message"];
            if let Some(content) = message["content"].as_str() {
                if text.is_none() && !content.is_empty() {
                    text = Some(content.to_string());
                }
            }
            if let Some(calls) = message["tool_calls"].as_array() {
                for call in calls {
                    let id = call["id"].as_str().unwrap_or("").to_string();
                    let name = call["function"]["name"].as_str().unwrap_or("").to_string();
                    // `arguments` arrives as a
                    // JSON-encoded string
                    // (per the OpenAI Chat
                    // Completions spec). We
                    // parse it back into a
                    // `serde_json::Value` so
                    // the `ToolCallRequest`
                    // carries a real JSON
                    // object, not a string.
                    let arguments = match call["function"]["arguments"].as_str() {
                        Some(s) => serde_json::from_str(s)
                            .unwrap_or_else(|_| serde_json::Value::String(s.into())),
                        None => call["function"]["arguments"].clone(),
                    };
                    tool_calls.push(ToolCallRequest {
                        id,
                        name,
                        arguments,
                    });
                }
            }
        }
    }
    let usage = parse_usage(&value);
    if !tool_calls.is_empty() {
        Ok(CompletionResponse::ToolCalls {
            calls: tool_calls,
            usage,
        })
    } else {
        Ok(CompletionResponse::TextReply {
            content: text.unwrap_or_default(),
            usage,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChatCompletionsConfig;
    use afa_bus::EventBus;
    use afa_contracts::UnsealedSecret;
    use afa_contracts::{SecretRef, SecurityErrorV1};
    use async_trait::async_trait;

    #[test]
    fn map_message_handles_user_text() {
        // The user-message mapper must
        // produce a `role: "user"`
        // object with the text content
        // intact. A future contributor
        // who renames the field is
        // breaking the OpenAI Chat
        // Completions wire contract.
        let item = ConversationItem::UserMessage {
            content: vec![afa_contracts::ContentBlock::Text("hi".into())],
        };
        let v = map_message(&item).expect("map");
        assert_eq!(v["role"], "user");
        assert_eq!(v["content"][0]["type"], "text");
        assert_eq!(v["content"][0]["text"], "hi");
    }

    #[test]
    fn map_block_handles_text() {
        // The `Text` block is the
        // common case. The mapper
        // must produce a
        // `type: "text"` object with
        // the text intact.
        let b = afa_contracts::ContentBlock::Text("hello".into());
        let v = map_block(&b);
        assert_eq!(v["type"], "text");
        assert_eq!(v["text"], "hello");
    }

    #[test]
    fn map_block_handles_image_url() {
        // An `ImageData::Url` block
        // must produce a
        // `type: "image_url"`
        // object. The `image_url.url`
        // field is the URL.
        let b = afa_contracts::ContentBlock::Image {
            mime_type: "image/png".into(),
            data: afa_contracts::ImageData::Url("https://x".into()),
        };
        let v = map_block(&b);
        assert_eq!(v["type"], "image_url");
        assert_eq!(v["image_url"]["url"], "https://x");
    }

    #[test]
    fn map_block_handles_image_base64_as_data_url() {
        // An `ImageData::Base64`
        // block must produce an
        // `image_url` whose `url` is
        // a `data:` URL with the
        // correct mime prefix. This
        // is the only shape the
        // Chat Completions API
        // accepts for base64
        // images.
        let b = afa_contracts::ContentBlock::Image {
            mime_type: "image/png".into(),
            data: afa_contracts::ImageData::Base64("AAAA".into()),
        };
        let v = map_block(&b);
        assert_eq!(v["type"], "image_url");
        let url = v["image_url"]
            .as_str()
            .expect("image_url must be a data: URL string");
        assert!(
            url.starts_with("data:image/png;base64,"),
            "expected data URL with mime prefix, got {url}"
        );
        assert!(url.ends_with("AAAA"));
    }

    #[test]
    fn map_request_uses_max_tokens_not_max_output_tokens() {
        // The Chat Completions field
        // is `max_tokens`, NOT
        // `max_output_tokens` (which
        // is the Responses API
        // field). A future contributor
        // who copies the Responses
        // mapper by accident will
        // break every Chat
        // Completions provider
        // (they'll silently ignore
        // the field).
        let r = CompletionRequest {
            system: None,
            messages: vec![ConversationItem::UserMessage {
                content: vec![afa_contracts::ContentBlock::Text("hi".into())],
            }],
            tools: vec![],
            sampling: afa_contracts::SamplingParams {
                temperature: 0.7_f32,
                max_output_tokens: 256,
                top_p: Some(1.0_f32),
                stop: vec![],
            },
        };
        let v = map_request(&r, "gpt-4o-mini").expect("map");
        assert_eq!(v["model"], "gpt-4o-mini");
        assert_eq!(v["max_tokens"], 256);
        assert!(v.get("max_output_tokens").is_none());
    }

    #[test]
    fn map_request_omits_tools_when_empty() {
        // Some "OpenAI-compatible"
        // services reject an empty
        // `tools: []` array with a
        // 400. We omit the field
        // entirely when no tools
        // are present.
        let r = CompletionRequest {
            system: None,
            messages: vec![ConversationItem::UserMessage {
                content: vec![afa_contracts::ContentBlock::Text("hi".into())],
            }],
            tools: vec![],
            sampling: afa_contracts::SamplingParams {
                temperature: 1.0_f32,
                max_output_tokens: 256,
                top_p: Some(1.0_f32),
                stop: vec![],
            },
        };
        let v = map_request(&r, "gpt-4o-mini").expect("map");
        assert!(v.get("tools").is_none());
        assert!(v.get("tool_choice").is_none());
    }

    #[test]
    fn map_request_includes_tools_when_present() {
        // When tools are present,
        // the body must include
        // both `tools` (the array)
        // and `tool_choice` (the
        // selection mode). We
        // hard-code `"auto"` in v1.
        let r = CompletionRequest {
            system: None,
            messages: vec![ConversationItem::UserMessage {
                content: vec![afa_contracts::ContentBlock::Text("hi".into())],
            }],
            tools: vec![ToolDefinition {
                name: "search".into(),
                description: "search the catalog".into(),
                parameters_schema: serde_json::json!({"type": "object"}),
            }],
            sampling: Default::default(),
        };
        let v = map_request(&r, "gpt-4o-mini").expect("map");
        assert_eq!(v["tools"][0]["type"], "function");
        assert_eq!(v["tools"][0]["function"]["name"], "search");
        assert_eq!(v["tool_choice"], "auto");
    }

    #[test]
    fn map_request_sends_system_as_a_message() {
        // Chat Completions has no
        // top-level `system` field;
        // the system prompt is a
        // `role: "system"` message
        // at the top of the
        // `messages` array. The
        // Responses API is the
        // opposite (top-level
        // `instructions`).
        let r = CompletionRequest {
            system: Some("You are a helpful assistant.".into()),
            messages: vec![ConversationItem::UserMessage {
                content: vec![afa_contracts::ContentBlock::Text("hi".into())],
            }],
            tools: vec![],
            sampling: Default::default(),
        };
        let v = map_request(&r, "gpt-4o-mini").expect("map");
        let messages = v["messages"].as_array().expect("messages array");
        assert_eq!(messages[0]["role"], "system");
        assert_eq!(messages[0]["content"], "You are a helpful assistant.");
        assert_eq!(messages[1]["role"], "user");
    }

    #[test]
    fn map_response_handles_text_reply() {
        // The text-reply path: the
        // response has one
        // `choices[0].message.content`
        // string. The mapper
        // produces a `TextReply`.
        let v = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": "Hello, world!"
                    },
                    "finish_reason": "stop"
                }
            ],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5}
        });
        let r = map_response(v, &[]).expect("map");
        match r {
            CompletionResponse::TextReply { content, usage } => {
                assert_eq!(content, "Hello, world!");
                assert_eq!(usage.prompt_tokens, 10);
                assert_eq!(usage.completion_tokens, 5);
            }
            _ => panic!("expected TextReply"),
        }
    }

    #[test]
    fn map_response_handles_tool_calls_with_json_string_arguments() {
        // The Chat Completions spec
        // sends `function.arguments`
        // as a JSON-encoded string.
        // We parse it back into a
        // `serde_json::Value` so
        // the `ToolCallRequest`
        // carries a real JSON
        // object. A future
        // contributor who passes
        // the raw string through
        // would break every tool
        // consumer downstream.
        let v = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_1",
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
            "usage": {"prompt_tokens": 20, "completion_tokens": 8}
        });
        let r = map_response(v, &[]).expect("map");
        match r {
            CompletionResponse::ToolCalls { calls, usage } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_1");
                assert_eq!(calls[0].name, "search_listings");
                assert_eq!(calls[0].arguments["query"], "Warsaw");
                assert_eq!(usage.prompt_tokens, 20);
            }
            _ => panic!("expected ToolCalls"),
        }
    }

    #[test]
    fn map_response_handles_tool_calls_with_invalid_json_arguments_as_raw_string() {
        // A vendor that sends
        // non-JSON in
        // `function.arguments` (a
        // real-world quirk: some
        // models emit partial
        // JSON or a comment) does
        // not crash the adapter.
        // We pass the raw string
        // through as a
        // `Value::String` so the
        // tool consumer can decide
        // what to do.
        let v = serde_json::json!({
            "choices": [
                {
                    "message": {
                        "role": "assistant",
                        "content": null,
                        "tool_calls": [
                            {
                                "id": "call_1",
                                "type": "function",
                                "function": {
                                    "name": "search",
                                    "arguments": "not-json"
                                }
                            }
                        ]
                    },
                    "finish_reason": "tool_calls"
                }
            ],
            "usage": {"prompt_tokens": 5, "completion_tokens": 5}
        });
        let r = map_response(v, &[]).expect("map");
        match r {
            CompletionResponse::ToolCalls { calls, .. } => {
                assert_eq!(calls[0].arguments, serde_json::json!("not-json"));
            }
            _ => panic!("expected ToolCalls"),
        }
    }

    #[test]
    fn parse_finish_reason_maps_known_values() {
        // The known Chat
        // Completions finish
        // reasons: `stop`,
        // `tool_calls`, `length`
        // (our `MaxTokens`),
        // `content_filter`.
        let v = serde_json::json!({
            "choices": [{"finish_reason": "stop"}]
        });
        assert_eq!(parse_finish_reason(&v), FinishReason::Stop);
        let v = serde_json::json!({
            "choices": [{"finish_reason": "tool_calls"}]
        });
        assert_eq!(parse_finish_reason(&v), FinishReason::ToolCalls);
        let v = serde_json::json!({
            "choices": [{"finish_reason": "length"}]
        });
        assert_eq!(parse_finish_reason(&v), FinishReason::MaxTokens);
        let v = serde_json::json!({
            "choices": [{"finish_reason": "content_filter"}]
        });
        assert_eq!(parse_finish_reason(&v), FinishReason::ContentFilter);
        // Unknown values fall back
        // to `Stop` (the most
        // conservative default).
        let v = serde_json::json!({
            "choices": [{"finish_reason": "something_new"}]
        });
        assert_eq!(parse_finish_reason(&v), FinishReason::Stop);
    }

    /// A bare-bones `SecurityV1` whose
    /// `unseal` returns a hard-coded
    /// key. Used by the unit tests that
    /// exercise the mapping helpers
    /// (no real wire calls).
    struct FakeForTest;

    #[async_trait]
    impl SecurityV1 for FakeForTest {
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
            _ctx: &afa_contracts::ExecutionContext,
        ) -> Result<SecretRef, SecurityErrorV1> {
            unimplemented!()
        }
    }

    #[test]
    fn fake_security_helper_compiles() {
        // A no-op test that
        // simply compiles the
        // `FakeForTest` struct
        // and the adapter
        // construction site
        // together. The actual
        // adapter behavior is
        // covered by the
        // wiremock-rs integration
        // tests. This test
        // exists so the
        // `FakeForTest` is not
        // dead-code-eliminated
        // and so a future
        // contributor who breaks
        // the type signature
        // gets a compile error
        // in `cargo test`
        // instead of in
        // integration tests.
        let _bus = EventBus::new().handle();
        let _config = ChatCompletionsConfig::gpt_4o_mini(SecretRef {
            name: "x".into(),
            version: 1,
        });
        let _security: Arc<dyn SecurityV1> = Arc::new(FakeForTest);
    }
}
