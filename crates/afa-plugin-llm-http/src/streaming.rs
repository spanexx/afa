//! Code Map: Phase 2 — `stream_complete` for the
//! OpenAI Responses API
//!
//! - `StreamBg`: The struct that the
//!   `ResponsesAdapter::stream_complete` method moves
//!   into a `tokio::spawn` background task. Holds
//!   the cloned `ResponsesConfig`, the cloned
//!   `Arc<dyn SecurityV1>` (for the 401-re-unseal
//!   retry path), the `EventBusHandle` (for the
//!   `CompletionCompleted` / `CompletionFailed`
//!   audit events), the `ExecutionContext` (for
//!   the `CorrelationId` on the audit events), the
//!   request body (with `"stream": true` already
//!   injected), the initial `Authorization` header
//!   value, the start time, the bounded
//!   `Sender<CompletionChunk>` (capacity 64), and a
//!   per-task `reqwest::Client`.
//! - `StreamBg::run`: The main loop. Two
//!   top-level steps: (1) call the vendor once; on
//!   HTTP 401, re-unseal via the security engine
//!   and call once more (same 3-step key wiring as
//!   `complete`); (2) iterate the SSE stream,
//!   mapping each event to a `CompletionChunk`
//!   and forwarding it to the `tx`. On any of
//!   { error, consumer dropped, deadline watchdog
//!   dropped `tx`, stream ended before
//!   `response.completed` } the task publishes the
//!   right audit event on the bus and exits.
//! - `map_responses_sse_event`: Pure function
//!   that maps one parsed JSON SSE event (the
//!   `data: {...}` payload) to an
//!   `Option<CompletionChunk>`. `None` for events
//!   the adapter does not care about (e.g.
//!   `response.created`, `response.output_text.done`).
//!   Takes a `&mut has_function_call` flag the bg
//!   task maintains; the mapper sets it to `true`
//!   when it sees a
//!   `response.output_item.added` event for a
//!   `function_call` output item, and the
//!   `response.completed` handler uses it to pick
//!   `FinishReason::ToolCalls` instead of `Stop`
//!   (the Responses API does NOT send an
//!   explicit `finish_reason` in
//!   `response.completed`).
//!
//! Story (plain English): When a workflow asks
//! for a streamed completion, the OpenAI
//! specialist does not wait at the desk for the
//! whole answer — that would block the workflow
//! for many seconds. Instead, the specialist
//! opens a small in-box (`mpsc::channel(64)`)
//! and hires a runner (`tokio::spawn`) to go to
//! the OpenAI front desk and stream letters
//! back into the in-box one at a time. The
//! runner speaks SSE (Server-Sent Events) — the
//! protocol OpenAI uses for streaming — and
//! translates each letter into a
//! `CompletionChunk` (a text delta, a "finished"
//! terminal, or an error). If the consumer
//! throws the in-box away (caller-drop) or the
//! deadline hits, the in-box is closed, the
//! runner's next letter-delivery fails, and the
//! runner stamps a "cancelled" ticket on the
//! log and walks away. If the front desk hangs
//! up early, the runner stamps a "stream
//! interrupted" ticket and walks away. The
//! in-box holds at most 64 letters at a time —
//! if the consumer is slow, the runner waits at
//! the desk until the consumer catches up, so
//! the vendor can never push the system past
//! its memory budget.
//!
//! CID Index:
//! CID:afa-plugin-llm-http-streaming-001 -> StreamBg
//! CID:afa-plugin-llm-http-streaming-002 -> run (the SSE main loop)
//! CID:afa-plugin-llm-http-streaming-003 -> call_once (one HTTP attempt)
//! CID:afa-plugin-llm-http-streaming-004 -> map_responses_sse_event
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-http-streaming-" crates/afa-plugin-llm-http/src/streaming.rs

use std::sync::Arc;
use std::time::Instant;

use afa_bus::EventBusHandle;
use afa_contracts::{
    CompletionChunk, CompletionCompleted, CompletionFailed, ExecutionContext, FinishReason,
    LlmErrorV1, SecurityV1, Usage,
};
use chrono::Utc;
use eventsource_stream::Eventsource;
use futures_util::StreamExt;
use serde_json::Value;
use tokio::sync::mpsc::Sender;

use super::config::ResponsesConfig;
// CID:afa-plugin-llm-http-streaming-001 - StreamBg
// Purpose: The 'static struct moved into
// the `tokio::spawn` background task. Owns
// clones of everything the task needs.
// Does NOT own `ResponsesAdapter` — the bg
// task only needs the config, the security
// engine, and the bus, and the bus is just
// the publish-side handle, so the adapter
// itself stays on the caller's stack.
// Used by: `ResponsesAdapter::stream_complete`.
pub struct StreamBg {
    /// The static config (model, base URL,
    /// key_ref).
    config: ResponsesConfig,
    /// The shared `SecurityV1` engine (for
    /// the 401 re-unseal retry path). The
    /// bg task does NOT go through
    /// `UnsealedHolder` (the holder's cache
    /// is per-adapter; a streaming call is
    /// a single round-trip so there is no
    /// value in caching, and a fresh
    /// `unseal()` keeps the key in a
    /// different zeroize-on-drop scope).
    security: Arc<dyn SecurityV1>,
    /// The publish-only event bus handle.
    bus: EventBusHandle,
    /// The execution context (carries the
    /// `CorrelationId` for the audit
    /// events).
    ctx: ExecutionContext,
    /// The request body (with `"stream":
    /// true` already injected).
    body: Value,
    /// The initial `Authorization` header
    /// value (the result of
    /// `UnsealedHolder::get_or_unseal` +
    /// `"Bearer "` prefix). On 401 the bg
    /// task re-unseals and gets a second
    /// value; the initial value is not
    /// reused.
    initial_auth: String,
    /// The start time (for `duration_ms`
    /// in the audit event).
    start: Instant,
    /// The bounded `mpsc::Sender`. The bg
    /// task holds the original; the
    /// deadline watchdog holds a clone
    /// (and drops it on timeout, which
    /// makes the bg task's `send().await`
    /// fail).
    tx: Sender<CompletionChunk>,
    /// The per-task `reqwest::Client`.
    /// Building it in the bg task keeps
    /// the bg task self-contained (the
    /// adapter does not need to expose a
    /// shared client).
    http: reqwest::Client,
}

impl StreamBg {
    // CID:afa-plugin-llm-http-streaming-002 - run
    // Purpose: The SSE main loop. Two
    // steps: (1) call the vendor (with
    // one 401 retry); (2) iterate the
    // SSE stream and map each event to
    // a `CompletionChunk`. On any
    // terminal condition the bg task
    // publishes the matching audit event
    // and returns.
    pub async fn run(self) {
        // Step 1: first call. On HTTP 401
        // we re-unseal and try once more
        // (same 3-step wiring as
        // `complete`).
        let resp = match self.call_once(&self.initial_auth).await {
            Ok(r) => r,
            Err((401, _)) => {
                // Re-unseal. We do NOT
                // share `UnsealedHolder`'s
                // cache (the bg task is
                // its own scope).
                let new_key = match self.security.unseal(&self.config.key_ref, &self.ctx).await {
                    Ok(secret) => match String::from_utf8(secret.to_vec()) {
                        Ok(s) => s,
                        Err(_) => {
                            self.fail_terminal(LlmErrorV1::AuthenticationFailed {
                                reason: "unsealed key is not valid UTF-8".into(),
                            })
                            .await;
                            return;
                        }
                    },
                    Err(e) => {
                        self.fail_terminal(LlmErrorV1::AuthenticationFailed {
                            reason: format!("re-unseal after 401 failed: {e:?}"),
                        })
                        .await;
                        return;
                    }
                };
                let new_auth = format!("Bearer {new_key}");
                match self.call_once(&new_auth).await {
                    Ok(r) => r,
                    Err((status, body)) => {
                        let err = super::responses_adapter::map_http_error(
                            status,
                            &body,
                            &self.config.model,
                        );
                        self.fail_terminal(err).await;
                        return;
                    }
                }
            }
            Err((status, body)) => {
                let err =
                    super::responses_adapter::map_http_error(status, &body, &self.config.model);
                self.fail_terminal(err).await;
                return;
            }
        };

        // Step 2: iterate the SSE stream.
        let mut stream = resp.bytes_stream().eventsource();
        // Track whether any function_call
        // output item was added during the
        // stream. The OpenAI Responses API
        // does NOT send an explicit
        // `finish_reason` in
        // `response.completed` — the
        // consumer infers it from the
        // output items. If any
        // function_call was added, the
        // model stopped because it wanted
        // to call a tool, and the
        // `Finished` chunk's reason is
        // `ToolCalls`; otherwise it's
        // `Stop` (or `MaxTokens` if the
        // response was truncated).
        let mut has_function_call = false;
        while let Some(event_result) = stream.next().await {
            let event = match event_result {
                Ok(e) => e,
                Err(e) => {
                    // SSE stream itself
                    // errored — vendor
                    // connection died
                    // mid-stream.
                    self.fail_terminal(LlmErrorV1::StreamInterrupted {
                        reason: format!("sse stream error: {e:?}"),
                    })
                    .await;
                    return;
                }
            };
            // `data: [DONE]` is the
            // sentinel after
            // `response.completed`.
            if event.data == "[DONE]" {
                return;
            }
            let parsed: Value = match serde_json::from_str(&event.data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let Some(chunk) =
                map_responses_sse_event(&parsed, &self.config.model, &mut has_function_call)
            else {
                continue;
            };
            // If the chunk is a
            // `Finished`, capture
            // its usage + reason for
            // the audit event BEFORE
            // the send (the audit
            // event is published
            // after the consumer
            // sees the chunk).
            let audit = if let CompletionChunk::Finished { reason, usage } = &chunk {
                Some((usage.clone(), *reason))
            } else {
                None
            };
            if self.tx.send(chunk).await.is_err() {
                // Consumer dropped
                // (caller-drop) OR
                // the deadline
                // watchdog dropped
                // its `tx` clone
                // (deadline). Both
                // publish
                // `CompletionCompleted
                // { Cancelled }`.
                self.cancelled_terminal().await;
                return;
            }
            if let Some((usage, reason)) = audit {
                // Success path —
                // the consumer
                // saw the
                // terminal
                // `Finished`
                // chunk. Now
                // publish the
                // matching
                // audit event.
                self.completed_terminal(usage, reason).await;
                return;
            }
        }
        // The SSE stream ended
        // without a
        // `response.completed`
        // event. The vendor
        // closed the
        // connection early.
        self.fail_terminal(LlmErrorV1::StreamInterrupted {
            reason: "vendor closed stream before response.completed".into(),
        })
        .await;
    }

    // CID:afa-plugin-llm-http-streaming-003 - call_once
    // Purpose: One raw HTTP call to the
    // vendor. Returns `Ok(Response)` on
    // 2xx; returns `Err((status, body))`
    // on non-2xx so the caller can
    // decide whether to map to a typed
    // error or retry.
    async fn call_once(&self, auth: &str) -> Result<reqwest::Response, (u16, Vec<u8>)> {
        let resp = self
            .http
            .post(format!("{}/v1/responses", self.config.base_url))
            .header("Authorization", auth)
            .json(&self.body)
            .send()
            .await
            .map_err(|e| {
                // Network-level
                // failure. We
                // synthesise a
                // sentinel `(status,
                // body)` so the
                // caller can call
                // `map_http_error`;
                // the mapper's
                // `UpstreamUnavailable`
                // branch handles
                // 0 (no status) as
                // a 5xx-range
                // upstream
                // failure.
                let status = e.status().map(|s| s.as_u16()).unwrap_or(0);
                (status, Vec::new())
            })?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.bytes().await.map(|b| b.to_vec()).unwrap_or_default();
            return Err((status.as_u16(), body));
        }
        Ok(resp)
    }

    /// Publish the `CompletionFailed`
    /// audit event and send the
    /// `Error(_)` chunk. The chunk is
    /// sent first so any in-flight
    /// consumer wakes up with the
    /// error; the publish happens
    /// after so an observer of the
    /// bus sees the same `duration_ms`
    /// as the chunk. On consumer-
    /// drop the `send` may fail; in
    /// that case we still publish
    /// (the audit is independent of
    /// the consumer).
    async fn fail_terminal(&self, error: LlmErrorV1) {
        let _ = self.tx.send(CompletionChunk::Error(error.clone())).await;
        let duration_ms = self.start.elapsed().as_millis() as u64;
        let event = CompletionFailed {
            correlation_id: self.ctx.correlation_id,
            tenant_id: self.ctx.tenant_id.clone(),
            actor: self.ctx.actor.clone(),
            model: self.config.model.clone(),
            error,
            duration_ms,
            timestamp: Utc::now(),
        };
        let _ = self.bus.publish(event, self.ctx.clone()).await;
    }

    /// Publish the `CompletionCompleted`
    /// audit event with
    /// `FinishReason::Cancelled`. Used
    /// for both caller-drop and
    /// deadline.
    async fn cancelled_terminal(&self) {
        let duration_ms = self.start.elapsed().as_millis() as u64;
        let event = CompletionCompleted {
            correlation_id: self.ctx.correlation_id,
            tenant_id: self.ctx.tenant_id.clone(),
            actor: self.ctx.actor.clone(),
            model: self.config.model.clone(),
            prompt_tokens: 0,
            completion_tokens: 0,
            finish_reason: FinishReason::Cancelled,
            duration_ms,
            timestamp: Utc::now(),
        };
        let _ = self.bus.publish(event, self.ctx.clone()).await;
    }

    /// Publish the `CompletionCompleted`
    /// audit event with the given
    /// finish reason and usage. Used
    /// on the normal-end path: the bg
    /// task has already sent a
    /// `Finished { reason, usage }`
    /// chunk to the consumer, and
    /// now stamps the matching audit
    /// ticket.
    async fn completed_terminal(&self, usage: Usage, reason: FinishReason) {
        let duration_ms = self.start.elapsed().as_millis() as u64;
        let event = CompletionCompleted {
            correlation_id: self.ctx.correlation_id,
            tenant_id: self.ctx.tenant_id.clone(),
            actor: self.ctx.actor.clone(),
            model: self.config.model.clone(),
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
            finish_reason: reason,
            duration_ms,
            timestamp: Utc::now(),
        };
        let _ = self.bus.publish(event, self.ctx.clone()).await;
    }
}

// CID:afa-plugin-llm-http-streaming-004 - map_responses_sse_event
// Purpose: Pure mapper. Takes the
// parsed JSON payload of one SSE event
// (the `data: {...}` line) and returns
// `Some(CompletionChunk)` for events the
// adapter cares about, `None` for the
// rest. See the IMPL doc's
// Streaming Events Table for the full
// taxonomy.
fn map_responses_sse_event(
    parsed: &Value,
    model: &str,
    has_function_call: &mut bool,
) -> Option<CompletionChunk> {
    let event_type = parsed["type"].as_str().unwrap_or("");
    match event_type {
        "response.output_text.delta" => {
            let delta = parsed["delta"].as_str().unwrap_or("").to_string();
            if delta.is_empty() {
                None
            } else {
                Some(CompletionChunk::TextDelta(delta))
            }
        }
        // `response.output_item.added` fires
        // once per output item as the
        // response builds. When the item
        // is a `function_call`, we emit the
        // first `ToolCallDelta` (id +
        // name) and remember the call so
        // the terminal `Finished` chunk
        // uses `FinishReason::ToolCalls`
        // instead of `Stop`.
        "response.output_item.added" => {
            let item = &parsed["item"];
            if item["type"].as_str() == Some("function_call") {
                *has_function_call = true;
                let id = item["id"].as_str().unwrap_or("").to_string();
                let name = item["name"].as_str().unwrap_or("").to_string();
                Some(CompletionChunk::ToolCallDelta {
                    id,
                    name_delta: name,
                    arguments_delta: String::new(),
                })
            } else {
                None
            }
        }
        // `response.function_call_arguments.delta`
        // fires many times per call as the
        // model streams the JSON arguments
        // string. We emit one
        // `ToolCallDelta` per event with
        // the new piece of arguments. The
        // `id` is the function call's
        // `item_id` — the consumer
        // reassembles by id and uses the
        // first non-empty one as the
        // call id.
        "response.function_call_arguments.delta" => {
            let id = parsed["item_id"].as_str().unwrap_or("").to_string();
            let delta = parsed["delta"].as_str().unwrap_or("").to_string();
            if delta.is_empty() {
                None
            } else {
                Some(CompletionChunk::ToolCallDelta {
                    id,
                    name_delta: String::new(),
                    arguments_delta: delta,
                })
            }
        }
        "response.completed" => {
            let usage = Usage {
                prompt_tokens: parsed["response"]["usage"]["input_tokens"]
                    .as_u64()
                    .unwrap_or(0) as u32,
                completion_tokens: parsed["response"]["usage"]["output_tokens"]
                    .as_u64()
                    .unwrap_or(0) as u32,
            };
            let status = parsed["response"]["status"].as_str().unwrap_or("completed");
            let reason = if *has_function_call {
                // The OpenAI Responses API
                // does NOT send an
                // explicit `finish_reason`
                // in `response.completed`
                // — the wire leaves that
                // inference to the
                // consumer. We saw at
                // least one function_call
                // output item above, so
                // the model stopped
                // because it wanted to
                // call a tool.
                FinishReason::ToolCalls
            } else if status == "incomplete" {
                let why = parsed["response"]["incomplete_details"]["reason"]
                    .as_str()
                    .unwrap_or("");
                if why == "max_tokens" {
                    FinishReason::MaxTokens
                } else {
                    FinishReason::Stop
                }
            } else {
                FinishReason::Stop
            };
            Some(CompletionChunk::Finished { reason, usage })
        }
        "response.error" => {
            let code = parsed["error"]["code"].as_str().unwrap_or("");
            let msg = parsed["error"]["message"]
                .as_str()
                .unwrap_or("vendor stream error")
                .to_string();
            let body = format!("{{\"error\":{{\"code\":\"{code}\",\"message\":\"{msg}\"}}}}");
            let err = super::responses_adapter::map_http_error(400, body.as_bytes(), model);
            Some(CompletionChunk::Error(err))
        }
        _ => None,
    }
}

/// Spawn the background streaming task.
/// Called by `ResponsesAdapter::stream_complete`
/// after it has built the request body and
/// unsealed the initial key. The bg task
/// owns the `tx` clone and the consumer
/// owns the `rx` half; the deadline
/// watchdog (in the adapter) gets a third
/// clone of `tx` and drops it on timeout.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_streaming(
    config: ResponsesConfig,
    security: Arc<dyn SecurityV1>,
    bus: EventBusHandle,
    ctx: ExecutionContext,
    body: Value,
    initial_auth: String,
    start: Instant,
    tx: Sender<CompletionChunk>,
) {
    let bg = StreamBg {
        config,
        security,
        bus,
        ctx,
        body,
        initial_auth,
        start,
        tx,
        http: reqwest::Client::builder()
            .build()
            .expect("reqwest client build"),
    };
    tokio::spawn(bg.run());
}
