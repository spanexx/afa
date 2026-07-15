//! Code Map: afa-plugin-embedding-ollama — adapter
//! - `OllamaEmbeddingAdapter`: The concrete
//!   HTTP adapter the kernel registers via
//!   `CapabilityRegistry::register_embedding`.
//!   Phase 0 is a skeleton: the struct is built,
//!   the `EmbeddingV1` trait is implemented, but
//!   every `embed` / `embed_batch` call returns
//!   `EmbeddingErrorV1::Internal` with a
//!   "Phase 0 skeleton" reason. Phase 2 wires
//!   the HTTP client, the request building, the
//!   response parsing, and the offline-mode
//!   logic.
//!
//! Story (plain English): The adapter is the
//! postal worker who walks to the local Ollama
//! daemon and brings back vectors. In Phase 0
//! the worker is at the desk but has not yet
//! been given the address book — every request
//! is politely turned away with a "we are
//! not yet delivering" message. The desk is
//! built and the door is open (a
//! `register_embedding` call succeeds) so the
//! kernel can wire it up and the conformance
//! suite can verify the
//! `describe_capabilities` shape (which does
//! not require I/O).
//!
//! CID Index:
//! CID:afa-plugin-embedding-ollama-adapter-001 -> OllamaEmbeddingAdapter
//! CID:afa-plugin-embedding-ollama-adapter-002 -> impl EmbeddingV1
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-ollama-adapter-" crates/afa-plugin-embedding-ollama/src/adapter.rs

use async_trait::async_trait;

use afa_contracts::{EmbeddingCapabilitiesV1, EmbeddingErrorV1, EmbeddingV1, ExecutionContext};

use super::config::OllamaEmbeddingConfig;

// CID:afa-plugin-embedding-ollama-adapter-001 - OllamaEmbeddingAdapter
// Purpose: The concrete HTTP adapter the
// kernel registers via
// `CapabilityRegistry::register_embedding`.
// Phase 0 is a skeleton: the struct is built,
// the `EmbeddingV1` trait is implemented, but
// every `embed` / `embed_batch` call returns
// `EmbeddingErrorV1::Internal`. The
// `describe_capabilities` method returns the
// v1 Ollama card (nomic-embed-text, 768-dim,
// max batch 2048, max sequence 8192,
// supports_batching = true). Phase 2 wires
// the real HTTP call.
//
// The adapter holds the `OllamaEmbeddingConfig`
// (the settings card) and a `MockEmbeddingMode`
// flag (always `Phase0` for now; Phase 2 will
// add a `Degraded` variant for the
// "Ollama not reachable" case).
// Uses: EmbeddingV1, EmbeddingErrorV1,
// EmbeddingCapabilitiesV1,
// ExecutionContext,
// OllamaEmbeddingConfig.
// Used by: the kernel's bootstrap (which
// calls `register_embedding(ollama_adapter)`),
// the conformance suite (which calls
// `embed` / `embed_batch` and expects
// `Internal` in Phase 0 and real vectors
// in Phase 2+).
#[derive(Debug)]
pub struct OllamaEmbeddingAdapter {
    config: OllamaEmbeddingConfig,
    /// The phase-mode flag. Always
    /// `Phase0` for the skeleton. Phase 2
    /// will rename this to `Phase2` once
    /// the HTTP client is wired.
    ///
    /// `#[allow(dead_code)]` is
    /// deliberate: Phase 0 reserves the
    /// field; Phase 2 reads it. The
    /// `Debug` derive does not silence
    /// the dead_code lint (the field is
    /// never read by any code path in
    /// Phase 0).
    #[allow(dead_code)]
    mode: MockEmbeddingMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MockEmbeddingMode {
    /// Phase 0 skeleton: every
    /// `embed` returns `Internal`. The
    /// conformance suite's Phase 0 tests
    /// are the only ones that pass.
    Phase0,
}

impl OllamaEmbeddingAdapter {
    /// Build a new `OllamaEmbeddingAdapter`
    /// from an `OllamaEmbeddingConfig`. The
    /// skeleton does NOT check the Ollama
    /// reachability (per the IMPL §"Phase 2
    /// constructor" rule); the call always
    /// succeeds. Phase 2 will add the URL
    /// validation (a malformed URL returns
    /// `InvalidInput`; an unreachable
    /// daemon is a runtime failure surfaced
    /// as `AdapterUnavailable`).
    pub fn new(config: OllamaEmbeddingConfig) -> Self {
        Self {
            config,
            mode: MockEmbeddingMode::Phase0,
        }
    }

    /// Hand back a reference to the
    /// adapter's config. Used by the
    /// conformance suite to assert the
    /// adapter is built with the
    /// expected settings.
    pub fn config(&self) -> &OllamaEmbeddingConfig {
        &self.config
    }
}

// CID:afa-plugin-embedding-ollama-adapter-002 - impl EmbeddingV1
// Purpose: The trait impl. Phase 0 returns
// `Internal` from every `embed` call (the
// "we are not yet delivering" sentinel).
// The default `embed_batch` impl from the
// trait (which loops over `embed`) is used;
// Phase 2 will override it with the real
// HTTP POST.
//
// `describe_capabilities` returns the
// v1 Ollama card. No `async`, no `ctx`,
// no I/O. The dimension is hard-coded
// for the configured model; Phase 2 will
// read it from a `model_capabilities.toml`
// lookup (or, for unknown models, the
// first `embed` call validates the
// response dimension).
// Uses: EmbeddingErrorV1,
// EmbeddingCapabilitiesV1,
// ExecutionContext.
// Used by: every workflow that calls
// `embed` / `embed_batch` (in Phase 0,
// every call gets `Internal`; in
// Phase 2+, every call gets a real
// vector from Ollama).
#[async_trait]
impl EmbeddingV1 for OllamaEmbeddingAdapter {
    async fn embed(
        &self,
        _text: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Vec<f32>, EmbeddingErrorV1> {
        // Phase 0 skeleton: the HTTP
        // client is not wired. The
        // "not yet delivering" message
        // is the contract — the
        // conformance suite asserts
        // on it.
        Err(EmbeddingErrorV1::Internal {
            reason: "afa-plugin-embedding-ollama Phase 0 skeleton: HTTP client not yet wired (Phase 2 wires reqwest)"
                .to_string(),
        })
    }

    fn describe_capabilities(&self) -> EmbeddingCapabilitiesV1 {
        // The v1 Ollama card. The
        // dimension is hard-coded
        // for the most common model
        // (nomic-embed-text = 768);
        // Phase 2 will add a
        // `model_capabilities.toml`
        // lookup.
        EmbeddingCapabilitiesV1 {
            model_name: self.config.model_name.clone(),
            dimension: 768,
            max_batch_size: 2048,
            max_sequence_length: 8192,
            supports_batching: true,
        }
    }
}
