//! `OllamaEmbeddingAdapter` — the `EmbeddingV1`
//! implementation for Ollama.
//!
//! Single file in the `adapter.rs` module of the
//! `afa-plugin-embedding-ollama` crate. Owns the
//! adapter struct, its `EmbeddingV1` trait impl,
//! and the typed factory that the `Kernel` uses
//! to construct an instance from a validated
//! `OllamaEmbeddingConfig`.
//!
//! **The contract** (see
//! `afa-contracts/src/embedding/traits.rs`):
//! - `embed(text, ctx)` — embed one string.
//!   Returns `Vec<f32>` of the model's
//!   dimensionality.
//! - `embed_batch(texts, ctx)` — embed a batch
//!   in a single HTTP call. The default trait
//!   impl loops over `embed`; we override it
//!   to take advantage of Ollama's batched
//!   `/v1/embeddings` endpoint (one round-trip
//!   instead of N).
//! - `describe_capabilities()` — return the
//!   `EmbeddingCapabilitiesV1` struct
//!   (model_name, dimension, max_batch_size,
//!   max_sequence_length, supports_batching).
//!
//! **Why a separate `client` module:** the HTTP
//! call is a separate concern. The adapter is
//! the contract surface; the client is the wire
//! surface. Splitting them keeps each file
//! small (under 250 lines) and lets the
//! conformance tests swap in a `wiremock-rs`
//! server for the client without touching the
//! adapter.

use async_trait::async_trait;
use tracing::{debug, info, warn};

use afa_contracts::{EmbeddingCapabilitiesV1, EmbeddingErrorV1, EmbeddingV1, ExecutionContext};

use crate::client::OllamaHttpClient;
use crate::config::OllamaEmbeddingConfig;

/// The Ollama HTTP embedding adapter.
///
/// Cheap to clone (the inner `OllamaHttpClient`
/// is a `reqwest::Client` wrapper; both are
/// internally `Arc`-backed). The `Kernel` clones
/// it once per `EmbeddingV1` capability lookup.
#[derive(Debug, Clone)]
pub struct OllamaEmbeddingAdapter {
    /// The validated config. The `name()` is a
    /// separate field on the trait — this is the
    /// full config (used by `describe_capabilities`
    /// and the `config()` accessor).
    config: OllamaEmbeddingConfig,

    /// The HTTP client. Owns the `reqwest::Client`
    /// + the validated config. Cheap to clone.
    client: OllamaHttpClient,
}

impl OllamaEmbeddingAdapter {
    /// Construct a new adapter from a validated
    /// `OllamaEmbeddingConfig`. The `Kernel` calls
    /// `config.validate()` first; this method
    /// does not re-validate.
    ///
    /// # Returns
    ///
    /// `Ok(OllamaEmbeddingAdapter)` on success.
    /// `Err(EmbeddingErrorV1::Internal)` if the
    /// `reqwest::Client` builder fails (e.g. the
    /// system TLS config is broken). In practice
    /// this never happens — `reqwest::Client::builder()`
    /// only fails on platform TLS init errors.
    pub fn new(config: OllamaEmbeddingConfig) -> Result<Self, EmbeddingErrorV1> {
        let client = OllamaHttpClient::new(config);
        let config = client.config().clone();
        Ok(Self { config, client })
    }

    /// Build the adapter and log the registration
    /// event. Used by the `Kernel` at startup.
    pub fn register(config: OllamaEmbeddingConfig) -> Result<Self, EmbeddingErrorV1> {
        let adapter = Self::new(config)?;
        info!(
            adapter = %adapter.config.name,
            base_url = %adapter.config.base_url,
            model = %adapter.config.model,
            timeout_secs = adapter.config.timeout_secs,
            max_batch_size = adapter.config.max_batch_size,
            "ollama embedding adapter registered"
        );
        Ok(adapter)
    }

    /// Read-only access to the config. Used by
    /// the conformance tests and the
    /// `afa-cli embedding status` command.
    pub fn config(&self) -> &OllamaEmbeddingConfig {
        &self.config
    }
}

#[async_trait]
impl EmbeddingV1 for OllamaEmbeddingAdapter {
    /// Embed a single string. Implemented as
    /// `embed_batch(&[text.to_string()])` then
    /// returning the first vector. This is
    /// strictly equivalent to the default trait
    /// impl but is inlined so the conformance
    /// test can observe the underlying HTTP
    /// call count (1 call per `embed`, regardless
    /// of `embed` vs `embed_batch`).
    async fn embed(
        &self,
        text: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Vec<f32>, EmbeddingErrorV1> {
        // Empty input check — fail fast, do not
        // hit the wire. The contract says empty
        // input is `InvalidInput` BEFORE any I/O.
        if text.trim().is_empty() {
            return Err(EmbeddingErrorV1::InvalidInput {
                reason: "ollama adapter: `text` must be non-empty and not whitespace-only"
                    .to_string(),
            });
        }
        let mut out: Vec<Vec<f32>> = self.client.embed_batch(&[text.to_string()]).await?;
        // `embed_batch` returns the same length
        // as the input, so `out.pop()` is safe.
        Ok(out.swap_remove(0))
    }

    /// Embed a batch in a single HTTP call. The
    /// batched endpoint is strictly faster than
    /// looping over `embed` (one round-trip
    /// instead of N), and Ollama parallelizes
    /// the per-input forward pass internally.
    ///
    /// Overrides the default trait impl, which
    /// loops over `embed()`. The default would
    /// make N HTTP calls per `embed_batch(N)` —
    /// unacceptable for a 100-input batch.
    async fn embed_batch(
        &self,
        texts: &[String],
        _ctx: &ExecutionContext,
    ) -> Result<Vec<Vec<f32>>, EmbeddingErrorV1> {
        let result = self.client.embed_batch(texts).await;
        match &result {
            Ok(vectors) => {
                debug!(
                    adapter = %self.config.name,
                    input_count = texts.len(),
                    output_count = vectors.len(),
                    "ollama embed_batch ok"
                );
            }
            Err(e) => {
                warn!(
                    adapter = %self.config.name,
                    input_count = texts.len(),
                    error = %e,
                    "ollama embed_batch failed"
                );
            }
        }
        result
    }

    /// Describe the adapter's capabilities.
    ///
    /// The `model_name` is the configured Ollama
    /// model tag. The `dimension` is the model's
    /// output dimensionality — for Phase 2 we
    /// hard-code the common cases (384 for
    /// `all-minilm`, 768 for `nomic-embed-text`,
    /// 1024 for `mxbai-embed-large`); unknown
    /// models return 0 and the caller falls back
    /// to introspection (the first `embed` call's
    /// `Vec<f32>` length). The `max_batch_size`
    /// comes from the config. The
    /// `max_sequence_length` is the model's
    /// context window (2048 for `nomic-embed`,
    /// 512 for `all-minilm`); again, hard-coded
    /// for the common cases, 0 otherwise. The
    /// `supports_batching` is `true` (we use the
    /// batched endpoint).
    fn describe_capabilities(&self) -> EmbeddingCapabilitiesV1 {
        let (dimension, max_sequence_length) = known_model_specs(&self.config.model);
        EmbeddingCapabilitiesV1 {
            model_name: self.config.model.clone(),
            dimension,
            max_batch_size: self.config.max_batch_size as u32,
            max_sequence_length,
            supports_batching: true,
        }
    }
}

/// Look up known model specs by name. Returns
/// `(dimension, max_sequence_length)`. Unknown
/// models get `(0, 0)` — the caller falls back to
/// introspection (the first `embed()` call's
/// `Vec<f32>` length tells the dimension).
fn known_model_specs(model: &str) -> (u32, u32) {
    match model {
        "nomic-embed-text" => (768, 2048),
        "nomic-embed-text:v1.5" => (768, 2048),
        "all-minilm" => (384, 512),
        "all-minilm:33m" => (384, 512),
        "mxbai-embed-large" => (1024, 512),
        "snowflake-arctic-embed" => (1024, 512),
        "snowflake-arctic-embed:335m" => (1024, 512),
        "snowflake-arctic-embed:137m" => (768, 512),
        _ => (0, 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use afa_contracts::{Actor, ExecutionContext, TenantId};

    fn ctx() -> ExecutionContext {
        ExecutionContext::new(
            TenantId::new("test"),
            Actor::Internal {
                caller: "unit-test".into(),
            },
        )
    }

    fn config() -> OllamaEmbeddingConfig {
        OllamaEmbeddingConfig {
            name: "test-ollama".to_string(),
            base_url: "http://localhost:11434".to_string(),
            model: "nomic-embed-text".to_string(),
            timeout_secs: 30,
            max_batch_size: 100,
            keep_alive_secs: 300,
        }
    }

    #[test]
    fn known_model_specs_returns_correct_dimension() {
        assert_eq!(known_model_specs("nomic-embed-text"), (768, 2048));
        assert_eq!(known_model_specs("all-minilm"), (384, 512));
        assert_eq!(known_model_specs("mxbai-embed-large"), (1024, 512));
        assert_eq!(known_model_specs("unknown-model"), (0, 0));
    }

    #[test]
    fn describe_capabilities_reports_configured_values() {
        let adapter = OllamaEmbeddingAdapter::new(config()).unwrap();
        let caps = adapter.describe_capabilities();
        assert_eq!(caps.model_name, "nomic-embed-text");
        assert_eq!(caps.dimension, 768);
        assert_eq!(caps.max_sequence_length, 2048);
        assert_eq!(caps.max_batch_size, 100);
        assert!(caps.supports_batching);
    }

    #[test]
    fn describe_capabilities_unknown_model_returns_zeros() {
        let mut cfg = config();
        cfg.model = "unknown-model-xyz".to_string();
        let adapter = OllamaEmbeddingAdapter::new(cfg).unwrap();
        let caps = adapter.describe_capabilities();
        assert_eq!(caps.dimension, 0);
        assert_eq!(caps.max_sequence_length, 0);
        assert_eq!(caps.model_name, "unknown-model-xyz");
    }

    #[test]
    fn new_returns_adapter_with_config() {
        let adapter = OllamaEmbeddingAdapter::new(config()).unwrap();
        assert_eq!(adapter.config().name, "test-ollama");
        assert_eq!(adapter.config().model, "nomic-embed-text");
    }

    #[tokio::test]
    async fn empty_input_rejected_before_http() {
        // Point at a guaranteed-unreachable port.
        let cfg = OllamaEmbeddingConfig {
            base_url: "http://127.0.0.1:1".to_string(),
            timeout_secs: 1,
            ..config()
        };
        let adapter = OllamaEmbeddingAdapter::new(cfg).unwrap();
        // Empty single → InvalidInput.
        let err = adapter.embed("", &ctx()).await.unwrap_err();
        assert!(
            matches!(err, EmbeddingErrorV1::InvalidInput { .. }),
            "expected InvalidInput, got {err:?}"
        );
        // Empty batch → Ok(empty vec) per the v1 contract
        // (the batch is a no-op, NOT an error; the conformance
        // suite asserts this so the same call shape works on
        // the local adapter and the mock).
        let vectors = adapter
            .embed_batch(&[], &ctx())
            .await
            .expect("empty batch should succeed with no vectors");
        assert!(
            vectors.is_empty(),
            "empty batch should return an empty vector list"
        );
        // Batch with one empty string → InvalidInput.
        let err = adapter
            .embed_batch(&["".to_string()], &ctx())
            .await
            .unwrap_err();
        assert!(
            matches!(err, EmbeddingErrorV1::InvalidInput { .. }),
            "expected InvalidInput, got {err:?}"
        );
    }
}
