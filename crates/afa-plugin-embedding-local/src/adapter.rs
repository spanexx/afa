//! Code Map: afa-plugin-embedding-local — adapter
//! - `LocalEmbeddingAdapter`: The concrete
//!   candle-based adapter the kernel registers
//!   via `CapabilityRegistry::register_embedding`.
//!   Phase 0 is a skeleton: the struct is
//!   built, the `EmbeddingV1` trait is
//!   implemented, but every `embed` /
//!   `embed_batch` call returns
//!   `EmbeddingErrorV1::Internal` with a
//!   "Phase 0 skeleton" reason. Phase 1 wires
//!   the candle model load, the lazy
//!   HuggingFace download, the batched forward
//!   pass, and the offline-mode logic.
//!
//! Story (plain English): The adapter is the
//! translation desk: it takes a chunk of text
//! from the kernel and turns it into a vector.
//! In Phase 0 the desk is closed — the
//! translator is on holiday, so every request
//! is politely turned away with a "we are
//! not yet serving customers" message. The
//! desk is built and the door is open (a
//! `register_embedding` call succeeds) so
//! the kernel can wire it up and the
//! conformance suite can verify the
//! `describe_capabilities` shape (which
//! does not require I/O).
//!
//! CID Index:
//! CID:afa-plugin-embedding-local-adapter-001 -> LocalEmbeddingAdapter
//! CID:afa-plugin-embedding-local-adapter-002 -> impl EmbeddingV1
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-local-adapter-" crates/afa-plugin-embedding-local/src/adapter.rs

use async_trait::async_trait;

use afa_contracts::{EmbeddingCapabilitiesV1, EmbeddingErrorV1, EmbeddingV1, ExecutionContext};

use super::config::LocalEmbeddingConfig;

// CID:afa-plugin-embedding-local-adapter-001 - LocalEmbeddingAdapter
// Purpose: The concrete candle-based adapter
// the kernel registers via
// `CapabilityRegistry::register_embedding`.
// Phase 0 is a skeleton: the struct is built,
// the `EmbeddingV1` trait is implemented, but
// every `embed` / `embed_batch` call returns
// `EmbeddingErrorV1::Internal`. The
// `describe_capabilities` method returns the
// v1 defaults (the all-MiniLM-L6-v2 card:
// 384-dim, max batch 64, max sequence 512,
// supports_batching = true). Phase 1 wires
// the real model load.
//
// The adapter holds the `LocalEmbeddingConfig`
// (the settings card) and a
// `MockEmbeddingMode` flag (always `Phase0`
// for now; future phases add `Strict`,
// `Degraded`, `Loaded`). The
// `MockEmbeddingMode` is the seam where
// Phase 1's real model load + offline-mode
// logic will plug in.
// Uses: EmbeddingV1, EmbeddingErrorV1,
// EmbeddingCapabilitiesV1,
// ExecutionContext, LocalEmbeddingConfig.
// Used by: the kernel's bootstrap (which
// calls `register_embedding(local_adapter)`),
// the conformance suite (which calls
// `embed` / `embed_batch` and expects
// `Internal` in Phase 0 and real vectors
// in Phase 1+).
#[derive(Debug)]
pub struct LocalEmbeddingAdapter {
    config: LocalEmbeddingConfig,
    /// The phase-mode flag. Always
    /// `Phase0` for the skeleton. Phase 1
    /// will rename this to `Phase1` once
    /// the candle model load is wired, and
    /// add a `Degraded` variant for the
    /// "model missing in offline mode =
    /// degraded" case.
    ///
    /// `#[allow(dead_code)]` is
    /// deliberate: Phase 0 reserves the
    /// field; Phase 1 reads it. The
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

impl LocalEmbeddingAdapter {
    /// Build a new `LocalEmbeddingAdapter`
    /// from a `LocalEmbeddingConfig`. The
    /// skeleton does NOT check the model
    /// directory; the call always succeeds.
    /// Phase 1 will add the
    /// `model_dir/<model_name>/config.json`
    /// existence check (the failure mode
    /// depends on `offline_mode`: `Strict`
    /// returns `ModelUnavailable`,
    /// `Degraded` constructs with the
    /// sentinel mode).
    pub fn new(config: LocalEmbeddingConfig) -> Self {
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
    pub fn config(&self) -> &LocalEmbeddingConfig {
        &self.config
    }
}

// CID:afa-plugin-embedding-local-adapter-002 - impl EmbeddingV1
// Purpose: The trait impl. Phase 0 returns
// `Internal` from every `embed` call (the
// "we are not yet serving customers"
// sentinel). The default `embed_batch`
// impl from the trait (which loops over
// `embed`) is used; Phase 1 will override
// it with the real candle batched forward
// pass.
//
// `describe_capabilities` returns the
// v1 all-MiniLM-L6-v2 card. No `async`,
// no `ctx`, no I/O (per the locked
// `describe_capabilities` contract).
// Uses: EmbeddingErrorV1,
// EmbeddingCapabilitiesV1,
// ExecutionContext.
// Used by: every workflow that calls
// `embed` / `embed_batch` (in Phase 0,
// every call gets `Internal`; in
// Phase 1+, every call gets a real
// vector).
#[async_trait]
impl EmbeddingV1 for LocalEmbeddingAdapter {
    async fn embed(
        &self,
        _text: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Vec<f32>, EmbeddingErrorV1> {
        // Phase 0 skeleton: the model
        // is not loaded. The "not yet
        // serving customers" message
        // is the contract — the
        // conformance suite asserts
        // on it.
        Err(EmbeddingErrorV1::Internal {
            reason: "afa-plugin-embedding-local Phase 0 skeleton: model not yet loaded (Phase 1 wires candle)"
                .to_string(),
        })
    }

    fn describe_capabilities(&self) -> EmbeddingCapabilitiesV1 {
        // The v1 all-MiniLM-L6-v2
        // card. Phase 1 will read
        // these from the model file
        // (the model has a
        // `config.json` with the
        // hidden_size, max_position_embeddings,
        // etc.). The Phase 0 values
        // are hard-coded so a
        // workflow that calls
        // `describe_capabilities` on
        // a fresh skeleton gets the
        // right answer.
        EmbeddingCapabilitiesV1 {
            model_name: self.config.model_name.clone(),
            dimension: 384,
            max_batch_size: 64,
            max_sequence_length: 512,
            supports_batching: true,
        }
    }
}
