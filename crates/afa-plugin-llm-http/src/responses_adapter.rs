//! Code Map: ResponsesAdapter
//! - `ResponsesAdapter`: The concrete `LlmV1` adapter for
//!   the OpenAI Responses API. Hard-wired to one
//!   model at construction (the model is in
//!   `ResponsesConfig`). All audit events
//!   (`CompletionRequested`, `CompletionCompleted`,
//!   `CompletionFailed`) are published on the event bus
//!   the constructor was given. The adapter uses the
//!   `UnsealedHolder` to manage the API key (3-step
//!   pattern: cache, retry on 401, zeroize on drop).
//!
//! Story (plain English): The `ResponsesAdapter` is the
//! OpenAI Responses-API specialist on the switchboard. When
//! a workflow asks for an LLM, the switchboard
//! (`CapabilityRegistry`) hands the request to this
//! specialist. The specialist has one permanent job:
//! talk to the OpenAI Responses API on the model's
//! behalf, using the sealed API key the security
//! engine hands it. If the OpenAI service says "your
//! key is bad" (HTTP 401), the specialist re-unseals
//! the key (the operator may have rotated it) and
//! tries once more, then gives up. Every request
//! stamps three small tickets on the log so an
//! auditor can later reconstruct "who asked for what
//! from the Responses-API specialist, did it work, and
//! how long did it take?" — without reading the
//! question or the answer.
//!
//! CID Index:
//! CID:afa-plugin-llm-http-adapter-001 -> ResponsesAdapter
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-http-adapter-" crates/afa-plugin-llm-http/src/responses_adapter.rs

use std::sync::Arc;
use std::time::Instant;

use async_trait::async_trait;

use afa_bus::EventBusHandle;
use afa_contracts::{
    CompletionChunk, CompletionCompleted, CompletionFailed, CompletionRequest, CompletionResponse,
    CompletionStream, ConversationItem, ExecutionContext, FinishReason, LlmErrorV1, LlmV1,
    ModelCapabilities, SecurityV1, ToolCallRequest, ToolDefinition, Usage,
};
use chrono::Utc;

use super::config::ResponsesConfig;
use super::key_wiring::UnsealedHolder;

// CID:afa-plugin-llm-http-adapter-001 - ResponsesAdapter
// Purpose: The concrete `LlmV1` adapter for the
// OpenAI Responses API. The adapter is
// hard-wired to one model at construction (the
// model is in `ResponsesConfig::model`); there is
// no per-request model override. The adapter
// uses an `UnsealedHolder` to manage the API
// key (3-step pattern: cache, retry on 401,
// zeroize on drop). All audit events are
// published on the event bus the constructor was
// given. The `Send + Sync` supertrait is the
// standard one for adapters held behind
// `Arc<dyn LlmV1>` in the `CapabilityRegistry`.
// Uses: SecurityV1 (the engine for the API
// key), Bus (the event bus for the audit
// events), LlmV1 (the trait the adapter
// implements), UnsealedHolder (the 3-step key
// wiring).
// Used by: `CapabilityRegistry::register_llm`
// (which holds an `Arc<dyn LlmV1>`), and any
// workflow that calls `llm.complete` /
// `llm.stream_complete`.
pub struct ResponsesAdapter {
    /// The static config. Carries the
    /// vendor base URL (the adapter uses
    /// `self.config.base_url` per request;
    /// captured in the config so parallel
    /// tests with different wiremock servers
    /// do not trample each other via a
    /// process-global env var).
    config: ResponsesConfig,
    /// The 3-step key wiring.
    key_holder: UnsealedHolder,
    /// The event bus for audit events. The
    /// adapter only publishes (it never
    /// subscribes), so the field is the
    /// publish-only `EventBusHandle`
    /// (a thin `Arc` clone of the
    /// underlying bus), not the full
    /// `EventBus`.
    bus: EventBusHandle,
    /// A best-effort `prompt_tokens_estimate`
    /// from the previous successful call. The
    /// `CompletionRequested` event carries
    /// this so an audit reader can see "this
    /// call's prompt was about N tokens." A
    /// `OnceLock` (or `tokio::sync::Mutex` if
    /// mutability is needed) is the standard
    /// idiom. We use a `Mutex<Option<u32>>`
    /// because the value can be updated
    /// concurrently.
    last_prompt_estimate: std::sync::Mutex<Option<u32>>,
}

impl ResponsesAdapter {
    /// Build a new `ResponsesAdapter`. The
    /// `bus` is used to publish audit events;
    /// the `security` engine is used by the
    /// `UnsealedHolder` to unseal the API key
    /// on first use and on 401 retries. The
    /// vendor base URL is taken from
    /// `config.base_url` (the production
    /// default in `gpt_4o` is
    /// `https://api.openai.com`; tests
    /// override it via `gpt_4o_with_base_url`).
    pub fn new(
        config: ResponsesConfig,
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

    /// Cheap best-effort prompt-size estimate
    /// from the request. Used for the
    /// `prompt_tokens_estimate` field on
    /// `CompletionRequested`. The estimate is
    /// the number of `ContentBlock::Text`
    /// chars in `system + messages` divided
    /// by 4 (the common heuristic for
    /// English-language token count). It is
    /// deliberately a heuristic — the exact
    /// count comes from the vendor in
    /// `CompletionCompleted`.
    ///
    /// `dead_code` is allowed because the
    /// function is exercised by the
    /// in-file `#[cfg(test)]` block (the
    /// `estimate_prompt_tokens_handles_empty_request`
    /// test) but is not on the production
    /// call path; the production
    /// `publish_requested` uses the cached
    /// value from the previous successful
    /// call instead.
    #[allow(dead_code)]
    fn estimate_prompt_tokens(&self, request: &CompletionRequest) -> u32 {
        let mut chars: usize = 0;
        if let Some(s) = request.system.as_ref() {
            chars += s.len();
        }
        for msg in &request.messages {
            match msg {
                ConversationItem::UserMessage { content }
                | ConversationItem::AssistantMessage { content } => {
                    for b in content {
                        if let afa_contracts::ContentBlock::Text(s) = b {
                            chars += s.len();
                        }
                    }
                }
                ConversationItem::ToolResult {
                    tool_call_id,
                    content,
                } => {
                    chars += tool_call_id.len();
                    for b in content {
                        if let afa_contracts::ContentBlock::Text(s) = b {
                            chars += s.len();
                        }
                    }
                }
            }
        }
        for tool in &request.tools {
            chars += tool.name.len();
            chars += tool.description.len();
        }
        // 4 chars per token is a common English
        // heuristic. Round up so the estimate
        // is at least 1 even for tiny prompts.
        ((chars / 4) as u32).max(1)
    }

    /// Map the request's tools + messages to
    /// the OpenAI Responses API shape
    /// (`CreateResponse`). The mapping is
    /// vendor-specific and lives in this
    /// crate, not in `afa-contracts`. The
    /// `LlmErrorV1` return is for I/O or
    /// serialization failures, NOT for
    /// vendor-side validation (those are
    /// the vendor's HTTP 400 responses,
    /// which the adapter maps separately).
    fn map_request_to_responses(
        &self,
        request: &CompletionRequest,
    ) -> Result<serde_json::Value, LlmErrorV1> {
        let messages = request
            .messages
            .iter()
            .map(map_message)
            .collect::<Result<Vec<_>, _>>()?;
        let tools = request.tools.iter().map(map_tool).collect::<Vec<_>>();
        let body = serde_json::json!({
            "model": self.config.model,
            "input": messages,
            "tools": tools,
            "temperature": request.sampling.temperature,
            "max_output_tokens": request.sampling.max_output_tokens,
        });
        Ok(body)
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
            | ConversationItem::AssistantMessage { content }
            | ConversationItem::ToolResult { content, .. } => content
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

/// Map a `ConversationItem` to the OpenAI
/// Responses API JSON shape. The mapping is
/// vendor-specific; the `Result` returns
/// `LlmErrorV1::InvalidRequest` if a content
/// block is not representable (e.g. an image
/// block with base64 data is encoded inline,
/// not a URL).
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

/// Map a `ContentBlock` to the OpenAI
/// Responses API content-array shape.
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
                // The OpenAI Responses API accepts images
                // only as an `input_image` content block
                // whose `image_url` is a `data:` URL. A
                // bare `type: "image"` block (with
                // top-level `mime_type` / `base64` keys)
                // is not a valid Responses API shape and
                // the vendor rejects it with a 400. The
                // data-URL form is the one the API
                // actually parses.
                serde_json::json!({
                    "type": "input_image",
                    "image_url": format!("data:{};base64,{}", mime_type, b64)
                })
            }
        },
    }
}

/// Map a `ToolDefinition` to the OpenAI
/// Responses API tool shape.
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
impl LlmV1 for ResponsesAdapter {
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
        let body = self.map_request_to_responses(&request)?;

        // Step 3: fetch the API key via the
        // 3-step holder. The first call
        // unseals; subsequent calls use the
        // cache.
        let auth = self.auth_header(ctx).await?;

        // Step 4: call the vendor. The HTTP
        // client is a reqwest client built
        // once at construction; we use the
        // `?retry_after_401` path to detect
        // and handle rotation.
        let (resp, attempts) =
            call_vendor_with_retry(&self.config, &body, &auth, &self.key_holder, ctx).await;

        let duration_ms = start.elapsed().as_millis() as u64;

        match resp {
            Ok((value, usage, finish_reason)) => {
                self.publish_completed(ctx, usage, finish_reason, duration_ms)
                    .await;
                // Map the response JSON to
                // `CompletionResponse`. The
                // shape is the OpenAI
                // Responses API:
                // - `value["output_text"]` is
                //   the text reply (string)
                // - `value["output"][*]
                //   ["type"] == "tool_call"`
                //   is a tool call.
                let response = map_response(value, &request.tools)?;
                if attempts > 1 {
                    tracing::info!(
                        correlation_id = %ctx.correlation_id,
                        attempts,
                        "responses adapter recovered after 401"
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

    // CID:afa-plugin-llm-http-adapter-005 - stream_complete
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
    // dispatch streaming calls (it
    // only resolves the adapter).
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
        let mut body = self.map_request_to_responses(&request)?;
        body["stream"] = serde_json::Value::Bool(true);

        // Step 3: unseal the initial key.
        // On failure we publish the
        // failure audit event and
        // return the error — the bg
        // task is not started in this
        // case (the channel is never
        // opened, so the consumer can
        // never see a partial stream).
        let key = self.key_holder.get_or_unseal(ctx).await?;
        let initial_auth = format!("Bearer {key}");

        // Step 4: open the channel +
        // spawn the bg task + spawn
        // the deadline watchdog. The
        // `mpsc::channel(64)` is the
        // bounded queue the IMPL doc
        // specifies.
        let (tx, rx) = tokio::sync::mpsc::channel::<CompletionChunk>(64);
        super::streaming::spawn_streaming(
            self.config.clone(),
            // The bg task gets a fresh
            // `Arc` clone of the
            // security engine (the
            // adapter's field is
            // `UnsealedHolder`, not the
            // raw engine, so the bg
            // task can't use it
            // directly for the
            // 401-retry path).
            self.key_holder.share_security_arc(),
            self.bus.clone(),
            ctx.clone(),
            body,
            initial_auth,
            start,
            tx.clone(),
        );
        // Deadline watchdog: if
        // `ctx.deadline` is `Some`,
        // spawn a task that sleeps
        // until the deadline and
        // then drops its `tx` clone.
        // Dropping the only sender
        // makes the bg task's
        // `send().await` fail, which
        // is the "deadline hit"
        // signal.
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

/// Call the vendor with one automatic
/// retry on HTTP 401 (the key may have
/// been rotated). The retry path uses
/// the `UnsealedHolder`'s
/// `re_unseal_after_401` to pick up the
/// new key, then calls the vendor
/// again. Returns the number of
/// attempts (1 or 2) for the audit
/// trail. The base URL comes from
/// `config.base_url` (captured at
/// construction; see `ResponsesConfig`)
/// so that each adapter is bound to a
/// specific URL — parallel tests with
/// different wiremock servers do not
/// trample each other via a process-
/// global env var.
async fn call_vendor_with_retry(
    config: &ResponsesConfig,
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

/// One raw vendor call. The HTTP
/// transport is a small `reqwest::Client`
/// that the adapter shares across
/// requests. The wire shape is the
/// OpenAI Responses API:
/// `POST /v1/responses` with a JSON body
/// that mirrors our `CompletionRequest`.
/// On a non-2xx response, the
/// status code + body are mapped to one
/// of the 13 `LlmErrorV1` variants. The
/// base URL comes from
/// `config.base_url` (see
/// `ResponsesConfig`).
async fn call_vendor(
    config: &ResponsesConfig,
    body: &serde_json::Value,
    auth: &str,
) -> Result<(serde_json::Value, Usage, FinishReason), LlmErrorV1> {
    // We use a single, lazy, blocking
    // reqwest client. reqwest is `async`
    // under the hood; the client is
    // cheap to build once.
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| LlmErrorV1::Internal {
            reason: format!("http client build failed: {e}"),
        })?;
    let resp = client
        .post(format!("{}/v1/responses", config.base_url))
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

/// Map a non-2xx HTTP response to one
/// of the 13 `LlmErrorV1` variants.
/// The mapping is the locked shape
/// from the TRD §2.2.10. Vendor-
/// specific error codes (e.g.
/// `context_length_exceeded`) are
/// detected from the JSON body's
/// `error.code` field.
// Make `pub(crate)` so the streaming
// module (`streaming.rs`) can call it
// from inside the bg task's 401-retry
// path.
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
            // `RateLimited`.
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
/// successful OpenAI Responses API
/// response. The shape is
/// `{"usage": {"input_tokens": N,
/// "output_tokens": M}}`.
fn parse_usage(value: &serde_json::Value) -> Usage {
    let prompt = value["usage"]["input_tokens"].as_u64().unwrap_or(0) as u32;
    let completion = value["usage"]["output_tokens"].as_u64().unwrap_or(0) as u32;
    Usage {
        prompt_tokens: prompt,
        completion_tokens: completion,
    }
}

/// Parse the `finish_reason` out of a
/// successful OpenAI Responses API
/// response. The shape is
/// `{"output": [{"stop_reason": "stop"}, ...]}`
/// or `"max_tokens"` or
/// `"content_filter"`.
fn parse_finish_reason(value: &serde_json::Value) -> FinishReason {
    let stop_reason = value["output"][0]["stop_reason"].as_str().unwrap_or("stop");
    match stop_reason {
        "stop" => FinishReason::Stop,
        "tool_calls" => FinishReason::ToolCalls,
        "max_tokens" => FinishReason::MaxTokens,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Stop,
    }
}

/// Map the OpenAI Responses API
/// response JSON to a
/// `CompletionResponse`. The shape is:
/// - `value["output_text"]` is the text
///   reply (string).
/// - `value["output"][*]` with
///   `type == "tool_call"` is a tool
///   call. We accumulate all tool calls
///   into the `ToolCalls` variant.
fn map_response(
    value: serde_json::Value,
    _tools: &[ToolDefinition],
) -> Result<CompletionResponse, LlmErrorV1> {
    // The "output" array carries the
    // model's structured response. A
    // text reply is a single item with
    // `type == "message"`; a tool call
    // is a single item with
    // `type == "tool_call"`.
    let output = value["output"].as_array();
    let mut tool_calls: Vec<ToolCallRequest> = Vec::new();
    let mut text: Option<String> = None;
    if let Some(items) = output {
        for item in items {
            match item["type"].as_str() {
                Some("message") => {
                    if text.is_none() {
                        text = Some(
                            item["content"][0]["text"]
                                .as_str()
                                .unwrap_or("")
                                .to_string(),
                        );
                    }
                }
                Some("tool_call") => {
                    let id = item["id"].as_str().unwrap_or("").to_string();
                    let name = item["name"].as_str().unwrap_or("").to_string();
                    let arguments = item["arguments"].clone();
                    tool_calls.push(ToolCallRequest {
                        id,
                        name,
                        arguments,
                    });
                }
                _ => {}
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
        // The `output_text` field is the
        // flat text reply; fall back to
        // it if we did not find a
        // `message` item.
        let content = text
            .or_else(|| value["output_text"].as_str().map(String::from))
            .unwrap_or_default();
        Ok(CompletionResponse::TextReply { content, usage })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use afa_bus::EventBus;
    use afa_contracts::UnsealedSecret;
    use afa_contracts::{SecretRef, SecurityErrorV1};
    use async_trait::async_trait;

    #[test]
    fn estimate_prompt_tokens_handles_empty_request() {
        // A request with no `system`, no
        // messages, and no tools still
        // produces an estimate of at
        // least 1 token (the rounding-up
        // guard). An empty estimate
        // would be a regression: the
        // `CompletionRequested` event
        // would carry `None` instead of
        // a positive number.
        let r = CompletionRequest {
            system: None,
            messages: vec![],
            tools: vec![],
            sampling: Default::default(),
        };
        let adapter = ResponsesAdapter {
            config: ResponsesConfig::responses_gpt_4o(SecretRef {
                name: "x".into(),
                version: 1,
            }),
            key_holder: UnsealedHolder::new(
                Arc::new(FakeForTest) as Arc<dyn SecurityV1>,
                ResponsesConfig::responses_gpt_4o(SecretRef {
                    name: "x".into(),
                    version: 1,
                }),
            ),
            bus: EventBus::new().handle(),
            last_prompt_estimate: std::sync::Mutex::new(None),
        };
        let est = adapter.estimate_prompt_tokens(&r);
        assert_eq!(est, 1);
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
            _ctx: &ExecutionContext,
        ) -> Result<SecretRef, SecurityErrorV1> {
            unimplemented!()
        }
    }

    #[test]
    fn map_message_handles_user_text() {
        // The user-message mapper must
        // produce a `role: "user"`
        // object with the text content
        // intact. A future contributor
        // who renames the field is
        // breaking the OpenAI wire
        // contract.
        let item = ConversationItem::UserMessage {
            content: vec![afa_contracts::ContentBlock::Text("hi".into())],
        };
        let v = map_message(&item).expect("map");
        assert_eq!(v["role"], "user");
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
    fn map_block_handles_image_base64_as_input_image_data_url() {
        // Regression test for a real
        // bug: an `ImageData::Base64`
        // block must produce the
        // OpenAI Responses API
        // `input_image` content
        // block whose `image_url` is
        // a `data:` URL. The earlier
        // shape (`type: "image"` with
        // top-level `mime_type` /
        // `base64` keys) is not a
        // valid Responses API block
        // and the vendor rejects it
        // with a 400.
        let b = afa_contracts::ContentBlock::Image {
            mime_type: "image/png".into(),
            data: afa_contracts::ImageData::Base64("AAAA".into()),
        };
        let v = map_block(&b);
        assert_eq!(v["type"], "input_image");
        let url = v["image_url"]
            .as_str()
            .expect("input_image image_url must be a data: URL string");
        assert!(
            url.starts_with("data:image/png;base64,"),
            "expected data URL with mime prefix, got {url}"
        );
        assert!(url.ends_with("AAAA"));
    }

    #[test]
    fn map_response_handles_text_reply() {
        // The text-reply path: the
        // response has one
        // `type: "message"` item with
        // a `content[0].text`
        // field. The mapper produces
        // a `TextReply`.
        let v = serde_json::json!({
            "output": [
                {
                    "type": "message",
                    "content": [
                        {"type": "text", "text": "Hello, world!"}
                    ]
                }
            ],
            "usage": {"input_tokens": 5, "output_tokens": 3}
        });
        let r = map_response(v.clone(), &[]).expect("map");
        match r {
            CompletionResponse::TextReply { content, usage } => {
                assert_eq!(content, "Hello, world!");
                assert_eq!(usage.prompt_tokens, 5);
                assert_eq!(usage.completion_tokens, 3);
            }
            _ => panic!("expected TextReply"),
        }
    }

    #[test]
    fn map_response_handles_tool_calls() {
        // The tool-call path: the
        // response has one
        // `type: "tool_call"` item
        // with `id`, `name`, and
        // `arguments`. The mapper
        // produces a `ToolCalls`
        // with the parsed arguments
        // (not a string).
        let v = serde_json::json!({
            "output": [
                {
                    "type": "tool_call",
                    "id": "call_abc",
                    "name": "search_listings",
                    "arguments": {"query": "Warsaw"}
                }
            ],
            "usage": {"input_tokens": 20, "output_tokens": 8}
        });
        let r = map_response(v, &[]).expect("map");
        match r {
            CompletionResponse::ToolCalls { calls, usage } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].id, "call_abc");
                assert_eq!(calls[0].name, "search_listings");
                assert_eq!(calls[0].arguments["query"], "Warsaw");
                assert_eq!(usage.total(), 28);
            }
            _ => panic!("expected ToolCalls"),
        }
    }
}
