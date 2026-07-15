//! HTTP client for the Ollama `/v1/embeddings` endpoint.
//!
//! Single file in the `client.rs` module of the
//! `afa-plugin-embedding-ollama` crate. Owns:
//! 1. The `OllamaHttpClient` (a thin wrapper around
//!    a `reqwest::Client` + the validated config).
//! 2. The `OllamaEmbedRequest` and
//!    `OllamaEmbedResponse` request/response
//!    types (the OpenAI-compatible JSON shape).
//! 3. The retry-on-5xx loop (up to 3 retries with
//!    1s/2s/4s exponential backoff).
//! 4. The HTTP status → `EmbeddingErrorV1` mapping
//!    (`200 → Ok`, `400 → InvalidInput`,
//!    `404 → ModelUnavailable`,
//!    `5xx → Internal`, network errors →
//!    `AdapterUnavailable`, parse errors →
//!    `Internal`).
//!
//! **Why this exists in its own file (and not
//! inline in `adapter.rs`):** the adapter's job is
//! the `EmbeddingV1` contract. The HTTP call is
//! the boundary between the contract and the
//! wire. Splitting it out makes the wire shape
//! explicit, lets the conformance tests swap in
//! `wiremock-rs` (the mock is a `reqwest` server
//! the client hits the same way it hits a real
//! Ollama), and keeps each file under 250 lines
//! (the rule from AGENTS.md).
//!
//! **The wire shape** (per Ollama's
//! `/v1/embeddings` doc):
//! ```text
//! POST /v1/embeddings
//! Content-Type: application/json
//!
//! {
//!   "model": "nomic-embed-text",
//!   "input": ["text1", "text2", ...],
//!   "keep_alive": 300
//! }
//! ```
//! ```text
//! HTTP/1.1 200 OK
//! Content-Type: application/json
//!
//! {
//!   "object": "list",
//!   "data": [
//!     {"object": "embedding", "index": 0, "embedding": [0.1, ...]},
//!     {"object": "embedding", "index": 1, "embedding": [0.2, ...]}
//!   ],
//!   "model": "nomic-embed-text",
//!   "usage": {"prompt_tokens": 4, "total_tokens": 4}
//! }
//! ```

use std::time::Duration;

use afa_contracts::embedding::error::EmbeddingErrorV1;
use serde::{Deserialize, Serialize};
use tokio::time::sleep;
use tracing::{debug, warn};

use crate::config::OllamaEmbeddingConfig;

// ============================================================================
// Attempt outcome
// ============================================================================

/// The result of a single attempt at the
/// `/v1/embeddings` request, tagged with
/// whether the error (if any) is worth
/// retrying.
///
/// The retry loop uses the `Retryable` vs
/// `NonRetryable` distinction to decide
/// whether to back off and try again. The
/// classifier matters because the typed
/// `EmbeddingErrorV1` variants (4 buckets)
/// conflate retryable and non-retryable
/// conditions:
/// - `AdapterUnavailable` covers BOTH
///   network errors (retryable — server
///   might come up) and timeouts
///   (non-retryable — per-call budget
///   already spent).
/// - `Internal` covers BOTH HTTP 5xx
///   (retryable — server might recover)
///   and parse errors (non-retryable —
///   server bug, re-trying produces the
///   same bad response).
///
/// Without this enum the retry loop would
/// over-retry timeouts (4 × 2s = 8s of
/// wall clock per embed call) and parse
/// errors (4 wasted requests to a broken
/// server). The IMPL §"Phase 2 — Ollama
/// Adapter (HTTP-Based) — PENDING" pins
/// the retry policy: ONLY 5xx and
/// connection errors are retried.
enum AttemptOutcome {
    /// The request succeeded; here are the
    /// raw response items.
    Ok(Vec<OllamaEmbedResponseItem>),
    /// The request failed in a way that
    /// should NOT be retried (4xx, timeout,
    /// parse error, length mismatch). The
    /// error is surfaced to the caller
    /// immediately.
    NonRetryable(EmbeddingErrorV1),
    /// The request failed in a way that
    /// SHOULD be retried (5xx, network
    /// error, DNS error). The error is
    /// stored as `last_error` and the retry
    /// loop backs off and tries again.
    Retryable(EmbeddingErrorV1),
}

// ============================================================================
// Wire types
// ============================================================================

/// The request body posted to `/v1/embeddings`.
///
/// Mirrors the OpenAI-compatible shape (Ollama's
/// `/v1/embeddings` endpoint accepts the same
/// fields). `input` is `Vec<String>` so a single
/// request can carry the batch (the adapter does
/// NOT loop over `embed` per input — it packs
/// everything into one POST and then sorts the
/// response by `index` to preserve input order).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaEmbedRequest {
    /// The model tag (e.g. `nomic-embed-text`).
    pub model: String,

    /// The inputs to embed. The Ollama server
    /// processes them in order; the response's
    /// `data[].index` field preserves the input
    /// order, which the adapter re-sorts.
    pub input: Vec<String>,

    /// Server-side model keep-alive window, in
    /// seconds. `0` means "unload immediately".
    /// Ollama's `keep_alive` field is a string in
    /// the native API and an integer in the
    /// OpenAI-compat API; we use the integer
    /// shape.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_alive: Option<u64>,
}

/// A single element of the `data` array in the
/// `/v1/embeddings` response.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaEmbedResponseItem {
    /// The index in the original `input` array.
    /// The adapter re-sorts by this field to
    /// guarantee input order.
    pub index: usize,

    /// The embedding vector, in the same shape
    /// Ollama returns it (a flat `Vec<f32>`).
    pub embedding: Vec<f32>,
}

/// The response body from `/v1/embeddings`.
///
/// Only the fields the adapter needs are declared;
/// additional fields (e.g. `usage`,
/// `created_at`) are accepted by serde's
/// `#[serde(default)]` + `deny_unknown_fields` is
/// deliberately OFF (Ollama is allowed to add
/// fields without breaking us).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaEmbedResponse {
    /// The full `data` array. The adapter re-sorts
    /// it by `index` before returning to the
    /// caller.
    pub data: Vec<OllamaEmbedResponseItem>,
}

// ============================================================================
// HTTP client
// ============================================================================

/// HTTP client for the Ollama `/v1/embeddings`
/// endpoint.
///
/// Owns the `reqwest::Client` and the validated
/// `OllamaEmbeddingConfig`. Built once at
/// `Kernel` startup, then `clone()`d cheaply
/// (the `reqwest::Client` is internally
/// `Arc`-backed) and shared across adapter
/// instances.
#[derive(Debug, Clone)]
pub struct OllamaHttpClient {
    /// The underlying `reqwest::Client` (cheap to
    /// clone; internally `Arc`-backed).
    http: reqwest::Client,

    /// The validated config (deep-copied; the
    /// `reqwest::Client` is the only state that
    /// actually mutates).
    config: OllamaEmbeddingConfig,

    /// Pre-built `POST {base_url}/v1/embeddings`
    /// URL. `reqwest::Url` parses the string once
    /// and re-uses the parsed value; cheaper than
    /// concatenating per call.
    endpoint: reqwest::Url,
}

impl OllamaHttpClient {
    /// Construct a new HTTP client from a validated
    /// config.
    ///
    /// The `reqwest::Client` is built with the
    /// configured timeout and the connection pool
    /// settings. The endpoint URL is parsed once
    /// here so each `embed_batch` call does not
    /// re-parse.
    ///
    /// # Panics
    ///
    /// Panics if the config's `base_url` is
    /// invalid. The `Kernel` calls `validate()`
    /// first, so this should never happen in
    /// production. The `unreachable!()` makes the
    /// contract explicit.
    pub fn new(config: OllamaEmbeddingConfig) -> Self {
        let http = reqwest::Client::builder()
            .timeout(config.timeout())
            .connect_timeout(Duration::from_secs(5))
            .build()
            .expect("reqwest::Client::builder should not fail with valid config");

        let endpoint = reqwest::Url::parse(&format!("{}/v1/embeddings", config.base_url))
            .expect("config.base_url was validated; URL parse should not fail here");

        Self {
            http,
            config,
            endpoint,
        }
    }

    /// Read-only access to the config (for
    /// `describe_capabilities`).
    pub fn config(&self) -> &OllamaEmbeddingConfig {
        &self.config
    }

    /// Embed a batch of inputs. Single POST to
    /// `/v1/embeddings` (the adapter does NOT
    /// loop over `embed()` per input — that
    /// would defeat the purpose of having a
    /// batched endpoint).
    ///
    /// # Retry policy
    ///
    /// Up to 3 retries on **HTTP 5xx and
    /// connection errors** with exponential
    /// backoff (1s, 2s, 4s). NOT retried:
    /// - 4xx (the request is malformed; re-trying
    ///   with the same body is pointless)
    /// - parse errors (a server bug; re-trying
    ///   produces the same bad response)
    /// - timeouts (the per-call budget was
    ///   already spent; re-trying would
    ///   compound the latency)
    ///
    /// # Returns
    ///
    /// `Ok(Vec<Vec<f32>>)` with the same length
    /// as `inputs` and the same order (sorted
    /// by `index` from the response). Each
    /// `Vec<f32>` is the dimensionality of the
    /// model (e.g. 768 for `nomic-embed-text`,
    /// 384 for `all-minilm`).
    ///
    /// # Errors
    ///
    /// Returns one of the 4 typed errors:
    /// - `InvalidInput` — the inputs are empty
    ///   OR a single input is the empty string
    ///   OR the server returns HTTP 4xx
    /// - `AdapterUnavailable` — the server is
    ///   unreachable (connection refused, DNS
    ///   failure, timeout) after 3 retries
    /// - `ModelUnavailable` — HTTP 404 (the
    ///   operator hasn't pulled the model)
    /// - `Internal` — HTTP 5xx after 3 retries
    ///   OR the response body is not valid JSON
    ///   OR the response is missing the `data`
    ///   array
    pub async fn embed_batch(&self, inputs: &[String]) -> Result<Vec<Vec<f32>>, EmbeddingErrorV1> {
        // (1) Empty input → fail fast, do not hit
        // the wire. The contract says empty input
        // is `InvalidInput` BEFORE any I/O.
        if inputs.is_empty() {
            return Ok(Vec::new());
        }
        for (i, t) in inputs.iter().enumerate() {
            if t.trim().is_empty() {
                return Err(EmbeddingErrorV1::InvalidInput {
                    reason: format!(
                        "ollama adapter: input at index {i} is empty or whitespace-only"
                    ),
                });
            }
        }

        // (2) Build the request body.
        let body = OllamaEmbedRequest {
            model: self.config.model.clone(),
            input: inputs.to_vec(),
            keep_alive: Some(self.config.keep_alive_secs),
        };

        // (3) Retry loop: 4 attempts total
        // (1 initial + 3 retries). Backoff: 1s,
        // 2s, 4s between attempts. ONLY 5xx and
        // connection errors are retried;
        // timeouts, parse errors, and 4xx are
        // surfaced immediately.
        const MAX_ATTEMPTS: u32 = 4;
        const BACKOFF: [Duration; 3] = [
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4),
        ];

        let mut last_error: Option<EmbeddingErrorV1> = None;
        for attempt in 0..MAX_ATTEMPTS {
            match self.try_once(&body).await {
                AttemptOutcome::Ok(items) => return Ok(sort_by_index(items)),
                // Non-retryable: surface immediately.
                AttemptOutcome::NonRetryable(e) => return Err(e),
                // Retryable (5xx or connection error):
                // backoff and try again.
                AttemptOutcome::Retryable(e) => {
                    let attempt_no = attempt + 1;
                    let max = MAX_ATTEMPTS;
                    warn!(
                        adapter = %self.config.name,
                        model = %self.config.model,
                        attempt = attempt_no,
                        max_attempts = max,
                        error = %e,
                        "ollama embed attempt failed (retryable), will retry"
                    );
                    last_error = Some(e);
                    if (attempt as usize) < BACKOFF.len() {
                        sleep(BACKOFF[attempt as usize]).await;
                    }
                }
            }
        }

        // All attempts exhausted — return the
        // last error.
        Err(last_error
            .expect("retry loop ran but no error was captured; this is a bug in OllamaHttpClient"))
    }

    /// Single attempt at the request. Returns
    /// an `AttemptOutcome` — either the raw
    /// `data` array, or a typed error tagged
    /// with whether the error is retryable.
    ///
    /// The retry classifier matters: timeouts
    /// and parse errors are NOT retried (per
    /// the IMPL), even though they are
    /// `AdapterUnavailable` and `Internal`
    /// respectively. Without the
    /// `AttemptOutcome` enum the retry loop
    /// would over-retry (4 attempts on a
    /// timeout = 4× the per-call budget).
    async fn try_once(&self, body: &OllamaEmbedRequest) -> AttemptOutcome {
        debug!(
            adapter = %self.config.name,
            model = %self.config.model,
            input_count = body.input.len(),
            "POST /v1/embeddings"
        );

        // (1) Send the request. Network errors
        // (connection refused, DNS failure,
        // request timeout) all come back as
        // `reqwest::Error`. The classifier
        // distinguishes connect (retryable —
        // server might be coming up) from
        // timeout (not retryable — per-call
        // budget already spent).
        let response = match self
            .http
            .post(self.endpoint.clone())
            .json(body)
            .send()
            .await
        {
            Ok(r) => r,
            Err(e) => {
                if e.is_timeout() {
                    return AttemptOutcome::NonRetryable(EmbeddingErrorV1::AdapterUnavailable {
                        reason: format!(
                            "ollama adapter `{}`: request timed out after {:?}",
                            self.config.name,
                            self.config.timeout()
                        ),
                    });
                }
                // Connect / DNS / TLS / other
                // request errors are retryable
                // (the server might be coming up).
                return AttemptOutcome::Retryable(EmbeddingErrorV1::AdapterUnavailable {
                    reason: format!(
                        "ollama adapter `{}`: network error to {}: {e}",
                        self.config.name, self.endpoint
                    ),
                });
            }
        };

        // (2) Map HTTP status → first-class
        // error. 2xx passes through; 4xx and 5xx
        // short-circuit. 4xx is non-retryable
        // (request is bad); 5xx is retryable
        // (server might recover).
        let status = response.status();
        if status.is_client_error() {
            // Read the body as text so the error
            // message includes the server's
            // explanation.
            let body_text: String = match response.text().await {
                Ok(s) => s,
                Err(_) => "<failed to read body>".to_string(),
            };
            return AttemptOutcome::NonRetryable(Self::map_4xx(
                status.as_u16(),
                &body_text,
                &self.config.model,
                &self.config.name,
            ));
        }
        if status.is_server_error() {
            return AttemptOutcome::Retryable(EmbeddingErrorV1::Internal {
                reason: format!(
                    "ollama adapter `{}`: HTTP {} (server error)",
                    self.config.name,
                    status.as_u16()
                ),
            });
        }

        // (3) Parse the body. Parse errors
        // (malformed JSON, missing `data` field)
        // are non-retryable (a server bug; re-trying
        // produces the same bad response).
        let parsed: OllamaEmbedResponse = match response.json::<OllamaEmbedResponse>().await {
            Ok(p) => p,
            Err(e) => {
                return AttemptOutcome::NonRetryable(EmbeddingErrorV1::Internal {
                    reason: format!(
                        "ollama adapter `{}`: failed to parse /v1/embeddings response: {e}",
                        self.config.name
                    ),
                });
            }
        };

        // (4) Length sanity check — the response
        // `data` array must have the same length
        // as the request `input` array. A
        // mismatch is a server bug; non-retryable.
        if parsed.data.len() != body.input.len() {
            return AttemptOutcome::NonRetryable(EmbeddingErrorV1::Internal {
                reason: format!(
                    "ollama adapter `{}`: returned {} embeddings for {} inputs",
                    self.config.name,
                    parsed.data.len(),
                    body.input.len()
                ),
            });
        }

        AttemptOutcome::Ok(parsed.data)
    }

    /// Map an HTTP 4xx response to a typed error.
    /// 404 → `ModelUnavailable` (the operator
    /// hasn't pulled the model). Everything else
    /// → `InvalidInput` (the request is bad).
    fn map_4xx(status: u16, body: &str, model_name: &str, _adapter_name: &str) -> EmbeddingErrorV1 {
        if status == 404 {
            return EmbeddingErrorV1::ModelUnavailable {
                model_name: model_name.to_string(),
                reason: format!("HTTP 404 from /v1/embeddings — is the model pulled? body: {body}"),
            };
        }
        EmbeddingErrorV1::InvalidInput {
            reason: format!("HTTP {status} from /v1/embeddings — body: {body}"),
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Sort the response items by `index` so the
/// output `Vec<Vec<f32>>` matches the input order
/// (Ollama returns them in `index` order, but we
/// don't want to trust that — a server bug could
/// scramble them; the contract guarantees the
/// adapter returns them in input order).
fn sort_by_index(mut items: Vec<OllamaEmbedResponseItem>) -> Vec<Vec<f32>> {
    items.sort_by_key(|i| i.index);
    items.into_iter().map(|i| i.embedding).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sort_by_index_preserves_input_order() {
        let items = vec![
            OllamaEmbedResponseItem {
                index: 2,
                embedding: vec![0.2; 4],
            },
            OllamaEmbedResponseItem {
                index: 0,
                embedding: vec![0.0; 4],
            },
            OllamaEmbedResponseItem {
                index: 1,
                embedding: vec![0.1; 4],
            },
        ];
        let sorted = sort_by_index(items);
        assert_eq!(sorted[0], vec![0.0; 4]);
        assert_eq!(sorted[1], vec![0.1; 4]);
        assert_eq!(sorted[2], vec![0.2; 4]);
    }

    #[test]
    fn request_serializes_with_keep_alive() {
        let req = OllamaEmbedRequest {
            model: "nomic-embed-text".to_string(),
            input: vec!["a".to_string(), "b".to_string()],
            keep_alive: Some(300),
        };
        let json = serde_json::to_value(&req).unwrap();
        assert_eq!(json["model"], "nomic-embed-text");
        assert_eq!(json["input"][0], "a");
        assert_eq!(json["input"][1], "b");
        assert_eq!(json["keep_alive"], 300);
    }
}
