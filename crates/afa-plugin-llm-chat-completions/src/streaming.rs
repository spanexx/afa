//! Code Map: Phase 2 — `stream_complete` for the
//! OpenAI Chat Completions API
//!
//! - `StreamBg`: The struct that the
//!   `ChatCompletionsAdapter::stream_complete`
//!   method moves into a `tokio::spawn` background
//!   task. Mirrors the OpenAI Responses streaming
//!   pattern (the `ResponsesAdapter` in
//!   `afa-plugin-llm-http` uses the same shape) but
//!   the SSE wire format is different — Chat
//!   Completions uses a per-chunk `choices[0]
//!   .delta.content` string (no `response.*`
//!   envelope) and a `data: [DONE]` sentinel for
//!   the end-of-stream.
//! - `StreamBg::run`: Two steps: (1) call the
//!   vendor once; on HTTP 401, re-unseal via the
//!   security engine and call once more (same
//!   3-step key wiring as `complete`); (2) iterate
//!   the SSE stream, mapping each event to a
//!   `CompletionChunk` and forwarding it to the
//!   `tx`. On any of { error, consumer dropped,
//!   deadline watchdog dropped `tx`, stream
//!   ended before `[DONE]` } the task publishes
//!   the right audit event on the bus and exits.
//! - `map_chat_completions_sse_event`: Pure
//!   function that maps one parsed JSON SSE event
//!   to an `Option<CompletionChunk>`. `None` for
//!   events the adapter does not care about (e.g.
//!   the first chunk that carries only `role`).
//!
//! Story (plain English): When a workflow asks
//! for a streamed Chat-Completions completion,
//! the "Lend Your Voice" specialist does not wait
//! at the desk for the whole answer — that would
//! block the workflow for many seconds. Instead,
//! the specialist opens a small in-box
//! (`mpsc::channel(64)`) and hires a runner
//! (`tokio::spawn`) to go to the vendor and
//! stream letters back into the in-box one at a
//! time. The runner speaks SSE (Server-Sent
//! Events) — the same wire protocol the Responses
//! API uses, but the per-event shape is a flat
//! `{"choices":[{"delta":{"content":"..."}
//! ,"index":0,"finish_reason":null}]}` instead of
//! a `response.*` envelope. The runner
//! translates each letter into a
//! `CompletionChunk`. If the consumer throws the
//! in-box away (caller-drop) or the deadline
//! hits, the in-box is closed, the runner's
//! letter-delivery fails, and the runner stamps a
//! "cancelled" ticket and walks away. If the
//! vendor's front desk hangs up early, the
//! runner stamps a "stream interrupted" ticket
//! and walks away. The in-box holds at most 64
//! letters at a time — if the consumer is slow,
//! the runner waits at the desk until the
//! consumer catches up, so the vendor can never
//! push the system past its memory budget.
//!
//! CID Index:
//! CID:afa-plugin-llm-chat-completions-streaming-001 -> StreamBg
//! CID:afa-plugin-llm-chat-completions-streaming-002 -> run (the SSE main loop)
//! CID:afa-plugin-llm-chat-completions-streaming-003 -> call_once (one HTTP attempt)
//! CID:afa-plugin-llm-chat-completions-streaming-004 -> map_chat_completions_sse_event
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-chat-completions-streaming-" crates/afa-plugin-llm-chat-completions/src/streaming.rs

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

use super::config::ChatCompletionsConfig;

// CID:afa-plugin-llm-chat-completions-streaming-001 - StreamBg
// Purpose: The 'static struct moved
// into the `tokio::spawn` background
// task. Owns clones of everything the
// task needs (config, security, bus,
// ctx, body, initial auth, tx, start,
// http). Mirrors the Responses-API
// streaming bg task in
// `afa-plugin-llm-http` but the SSE
// event mapper is different.
pub struct StreamBg {
    config: ChatCompletionsConfig,
    /// The shared `SecurityV1` engine
    /// (for the 401 re-unseal retry
    /// path). The bg task does NOT go
    /// through `UnsealedHolder` (the
    /// holder's cache is per-adapter; a
    /// streaming call is a single
    /// round-trip so there is no value
    /// in caching).
    security: Arc<dyn SecurityV1>,
    bus: EventBusHandle,
    ctx: ExecutionContext,
    /// The request body (with `"stream":
    /// true` already injected).
    body: Value,
    /// The initial `Authorization`
    /// header value (the result of
    /// `UnsealedHolder::get_or_unseal`
    /// + `"Bearer "` prefix).
    initial_auth: String,
    /// The start time (for
    /// `duration_ms` in the audit
    /// event).
    start: Instant,
    /// The bounded `mpsc::Sender`. The
    /// bg task holds the original; the
    /// deadline watchdog holds a clone
    /// (and drops it on timeout).
    tx: Sender<CompletionChunk>,
    /// The per-task `reqwest::Client`.
    http: reqwest::Client,
}

impl StreamBg {
    // CID:afa-plugin-llm-chat-completions-streaming-002 - run
    // Purpose: The SSE main loop. Two
    // steps: (1) call the vendor (with
    // one 401 retry); (2) iterate the
    // SSE stream and map each event to
    // a `CompletionChunk`. On any
    // terminal condition the bg task
    // publishes the matching audit
    // event and returns.
    pub async fn run(self) {
        // Step 1: first call. On HTTP
        // 401 we re-unseal and try once
        // more (same 3-step wiring as
        // `complete`).
        let resp = match self.call_once(&self.initial_auth).await {
            Ok(r) => r,
            Err((401, _)) => {
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
                        let err = super::adapter::map_http_error(status, &body, &self.config.model);
                        self.fail_terminal(err).await;
                        return;
                    }
                }
            }
            Err((status, body)) => {
                let err = super::adapter::map_http_error(status, &body, &self.config.model);
                self.fail_terminal(err).await;
                return;
            }
        };

        // Step 2: iterate the SSE
        // stream.
        let mut stream = resp.bytes_stream().eventsource();
        while let Some(event_result) = stream.next().await {
            let event = match event_result {
                Ok(e) => e,
                Err(e) => {
                    self.fail_terminal(LlmErrorV1::StreamInterrupted {
                        reason: format!("sse stream error: {e:?}"),
                    })
                    .await;
                    return;
                }
            };
            // `data: [DONE]` is the
            // sentinel after the last
            // `finish_reason: "stop"`
            // chunk.
            if event.data == "[DONE]" {
                return;
            }
            let parsed: Value = match serde_json::from_str(&event.data) {
                Ok(v) => v,
                Err(_) => continue,
            };
            let Some(chunk) = map_chat_completions_sse_event(&parsed, &self.config.model) else {
                continue;
            };
            // If the chunk is a
            // `Finished`, capture its
            // usage + reason for the
            // audit event BEFORE the
            // send.
            let audit = if let CompletionChunk::Finished { reason, usage } = &chunk {
                Some((usage.clone(), *reason))
            } else {
                None
            };
            if self.tx.send(chunk).await.is_err() {
                // Consumer dropped
                // (caller-drop) OR the
                // deadline watchdog
                // dropped its `tx`
                // clone (deadline).
                self.cancelled_terminal().await;
                return;
            }
            if let Some((usage, reason)) = audit {
                self.completed_terminal(usage, reason).await;
                return;
            }
        }
        // The SSE stream ended
        // without a `[DONE]`
        // sentinel. The vendor
        // closed the connection
        // early.
        self.fail_terminal(LlmErrorV1::StreamInterrupted {
            reason: "vendor closed stream before [DONE] sentinel".into(),
        })
        .await;
    }

    // CID:afa-plugin-llm-chat-completions-streaming-003 - call_once
    // Purpose: One raw HTTP call to the
    // vendor. Returns `Ok(Response)` on
    // 2xx; returns `Err((status, body))`
    // on non-2xx.
    async fn call_once(&self, auth: &str) -> Result<reqwest::Response, (u16, Vec<u8>)> {
        let resp = self
            .http
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Authorization", auth)
            .json(&self.body)
            .send()
            .await
            .map_err(|e| {
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
    /// `Error(_)` chunk.
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
    /// on the normal-end path.
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

// CID:afa-plugin-llm-chat-completions-streaming-004 - map_chat_completions_sse_event
// Purpose: Pure mapper. Takes the
// parsed JSON payload of one SSE
// event (the `data: {...}` line)
// and returns `Some(CompletionChunk)`
// for events the adapter cares about.
// The OpenAI Chat Completions
// streaming event shape:
// `{id, choices: [{index, delta: {role?, content?}, finish_reason}]}`.
// On the first chunk, `delta.role`
// is `"assistant"` and `delta.content`
// is absent. On every subsequent
// chunk, `delta.content` is a
// fragment of the assistant's reply.
// The last chunk (before `[DONE]`)
// has `finish_reason: "stop"` (or
// `"length"` for max-tokens, or
// `"content_filter"` for
// safety-refusal, or `"tool_calls"`
// for tool calls).
fn map_chat_completions_sse_event(parsed: &Value, model: &str) -> Option<CompletionChunk> {
    let choice = parsed["choices"].get(0)?;
    let delta_content = choice["delta"]["content"].as_str();
    let finish_reason = choice["finish_reason"].as_str();
    // The terminal chunk
    // carries
    // `finish_reason`
    // (and
    // optionally
    // an empty
    // `delta.content`).
    // The non-
    // terminal
    // chunks carry
    // `delta.content`
    // (and a
    // `null`
    // `finish_reason`).
    if let Some(reason_str) = finish_reason {
        // The terminal
        // chunk. We
        // build the
        // Usage from
        // the
        // `usage` field
        // if present
        // (OpenAI
        // only sends
        // `usage` when
        // `stream_options: {include_usage: true}`
        // is set, or
        // for some
        // OpenAI-
        // compatible
        // services
        // that always
        // include
        // it). For the
        // services that
        // don't, we
        // default to
        // zero usage
        // (the
        // consumer can
        // still see
        // the
        // `FinishReason`).
        let usage = if let Some(u) = parsed.get("usage") {
            Usage {
                prompt_tokens: u["prompt_tokens"].as_u64().unwrap_or(0) as u32,
                completion_tokens: u["completion_tokens"].as_u64().unwrap_or(0) as u32,
            }
        } else {
            Usage {
                prompt_tokens: 0,
                completion_tokens: 0,
            }
        };
        let reason = match reason_str {
            "stop" => FinishReason::Stop,
            "length" => FinishReason::MaxTokens,
            "content_filter" => FinishReason::ContentFilter,
            "tool_calls" => FinishReason::ToolCalls,
            _ => {
                // Unknown
                // finish
                // reason —
                // map to
                // `InvalidRequest`
                // (the
                // vendor
                // sent
                // something
                // we don't
                // understand).
                let body = format!(
                    "{{\"error\":{{\"code\":\"unknown_finish_reason\",\"message\":\"\
                     unknown finish_reason '{}' from vendor\"}}}}",
                    reason_str
                );
                let err = super::adapter::map_http_error(400, body.as_bytes(), model);
                return Some(CompletionChunk::Error(err));
            }
        };
        return Some(CompletionChunk::Finished { reason, usage });
    }
    // Non-terminal chunk — look
    // for `delta.content`. Some
    // first chunks carry only
    // `delta.role: "assistant"`
    // (no content yet); we
    // skip those.
    if let Some(text) = delta_content {
        if text.is_empty() {
            return None;
        }
        return Some(CompletionChunk::TextDelta(text.to_string()));
    }
    // Non-terminal tool-call
    // chunk. The Chat Completions
    // wire format sends one
    // `delta.tool_calls` array per
    // chunk; for v1 we collapse
    // to the first item (multiple
    // parallel tool calls is a
    // future LlmV1 v2 concern;
    // see IMPL §5 streaming
    // notes). The first chunk
    // for a given call carries
    // `id` + `function.name`; the
    // rest carry only
    // `function.arguments` (the
    // JSON string is streamed
    // piece by piece).
    if let Some(tool_calls) = choice["delta"]["tool_calls"].as_array() {
        if let Some(first) = tool_calls.first() {
            let id = first["id"].as_str().unwrap_or("").to_string();
            let name_delta = first["function"]["name"].as_str().unwrap_or("").to_string();
            let arguments_delta = first["function"]["arguments"]
                .as_str()
                .unwrap_or("")
                .to_string();
            if id.is_empty() && name_delta.is_empty() && arguments_delta.is_empty() {
                return None;
            }
            return Some(CompletionChunk::ToolCallDelta {
                id,
                name_delta,
                arguments_delta,
            });
        }
    }
    None
}

/// Spawn the background streaming task.
/// Called by
/// `ChatCompletionsAdapter::stream_complete`
/// after it has built the request body
/// and unsealed the initial key. The bg
/// task owns the `tx` clone and the
/// consumer owns the `rx` half; the
/// deadline watchdog (in the adapter)
/// gets a third clone of `tx` and
/// drops it on timeout.
#[allow(clippy::too_many_arguments)]
pub(crate) fn spawn_streaming(
    config: ChatCompletionsConfig,
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
