//! Code Map: afa-plugin-embedding-local — adapter
//! - `LocalEmbeddingAdapter`: The
//!   concrete candle-based adapter
//!   the kernel registers via
//!   `CapabilityRegistry::register_embedding`.
//!   Phase 1 wires the candle
//!   model load (via the
//!   `BertEmbedder`), the lazy
//!   HuggingFace download (via
//!   the `Downloader`), and the
//!   strict / degraded mode logic
//!   (via the `Offline` policy).
//!   The adapter returns real
//!   vectors on every `embed` /
//!   `embed_batch` call when the
//!   model is loaded; returns
//!   `Err(ModelUnavailable)` in
//!   strict mode when the model
//!   is missing; returns a
//!   384-element zero vector in
//!   degraded mode when the model
//!   is missing.
//!
//! Story (plain English): The
//! adapter is the kitchen's
//! front door. The customer
//! (the kernel) hands the
//! adapter a text; the adapter
//! (1) hands the text to the
//! candle workbench
//! (`BertEmbedder::embed_batch`),
//! (2) receives a vector back,
//! (3) hands the vector to the
//! customer. If the model is
//! not loaded, the adapter
//! either refuses (strict
//! mode) or returns a zero
//! vector (degraded mode) per
//! the operator's config.
//!
//! CID Index:
//! CID:afa-plugin-embedding-local-adapter-001 -> LocalEmbeddingAdapter
//! CID:afa-plugin-embedding-local-adapter-002 -> impl EmbeddingV1
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-local-adapter-" crates/afa-plugin-embedding-local/src/adapter.rs

use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use tokio::task;

use afa_contracts::{EmbeddingCapabilitiesV1, EmbeddingErrorV1, EmbeddingV1, ExecutionContext};

use super::config::{DownloadStrategy, LocalEmbeddingConfig, OfflineMode};
use super::download::Downloader;
use super::model::BertEmbedder;
use super::offline::{degraded_vector, Offline, OfflineAction};

// CID:afa-plugin-embedding-local-adapter-001 - LocalEmbeddingAdapter
// Purpose: The concrete
// candle-based adapter the
// kernel registers via
// `CapabilityRegistry::register_embedding`.
// Phase 1 wires the candle
// model load + the lazy
// HuggingFace download +
// the offline mode logic.
// The `Arc<BertEmbedder>`
// is shared across the
// adapter's methods (the
// `Arc` makes the
// `BertEmbedder` cheaply
// cloneable for the
// `tokio::task::spawn_blocking`
// call). The `state` enum
// tracks the adapter's
// runtime mode: `Real` (a
// model is loaded and a
// forward pass is the
// next call),
// `Degraded` (the model
// is missing; return
// zero vectors), or
// `Unloaded` (the model
// has not been loaded
// yet; the first
// `embed` call
// triggers the
// load).
// Uses: EmbeddingV1,
// EmbeddingErrorV1,
// EmbeddingCapabilitiesV1,
// ExecutionContext,
// LocalEmbeddingConfig,
// BertEmbedder,
// Downloader, Offline.
// Used by: the kernel's
// bootstrap (which calls
// `register_embedding(local_adapter)`),
// the conformance suite.
pub struct LocalEmbeddingAdapter {
    config: LocalEmbeddingConfig,
    /// The model state. The
    /// `Arc<BertEmbedder>`
    /// is `None` until the
    /// model is loaded
    /// (either eagerly at
    /// construction or
    /// lazily on the first
    /// `embed` call). The
    /// `Mutex<...>` wrapper
    /// is the
    /// interior-mutability
    /// seam: the trait
    /// methods are
    /// `&self` (the locked
    /// `EmbeddingV1`
    /// signature), but the
    /// `ensure_loaded`
    /// trampoline needs to
    /// transition the
    /// state from
    /// `Unloaded` to
    /// `Real`. The `Mutex`
    /// serializes the
    /// transition; the
    /// forward pass is
    /// itself a `Mutex` on
    /// the `BertEmbedder`.
    state: std::sync::Mutex<AdapterState>,
}

enum AdapterState {
    /// The model is loaded.
    /// A real forward pass
    /// is the next call.
    Real(Arc<BertEmbedder>),
    /// The model is missing
    /// and the operator is
    /// in degraded mode.
    /// Every `embed` call
    /// returns the
    /// sentinel zero
    /// vector.
    Degraded,
    /// The model has not
    /// been loaded yet.
    /// The first `embed`
    /// call triggers the
    /// load (or the
    /// download, if the
    /// `download_strategy`
    /// is `Lazy`).
    ///
    /// Phase 1.5 NOTE: the
    /// `Unloaded` state IS
    /// constructed by
    /// `LocalEmbeddingAdapter::new`
    /// (one-line change
    /// from Phase 1's
    /// `Real(Arc::new(embedder))`).
    /// The `ensure_loaded`
    /// trampoline
    /// pattern-matches
    /// on `Unloaded` and
    /// transitions to
    /// `Real` on the
    /// first `embed`
    /// call.
    Unloaded,
}

impl LocalEmbeddingAdapter {
    /// Build a new
    /// `LocalEmbeddingAdapter`
    /// from a
    /// `LocalEmbeddingConfig`.
    /// The constructor applies
    /// the offline policy:
    /// - Model present →
    ///   construct with
    ///   `Unloaded` (the
    ///   model is loaded
    ///   lazily on the first
    ///   `embed` call — this
    ///   is the Phase 1.5
    ///   lazy-load design;
    ///   Phase 1 used
    ///   `Real(Arc::new(embedder))`
    ///   to load eagerly)
    /// - Model missing +
    ///   `Strict` → return
    ///   `Err(ModelUnavailable)`
    ///   (the operator
    ///   must pre-place
    ///   the files)
    /// - Model missing +
    ///   `Degraded` →
    ///   construct with
    ///   `Degraded` (the
    ///   sentinel mode; no
    ///   lazy transition
    ///   because there is
    ///   no model to load)
    ///
    /// Phase 1.5 NOTE: the
    /// `Unloaded` state was
    /// reachable but never
    /// constructed in Phase
    /// 1. This is the
    /// one-line change the
    /// Phase 1 comment on
    /// the `Unloaded` variant
    /// promised. The
    /// `ensure_loaded`
    /// trampoline already
    /// pattern-matches on
    /// `Unloaded` and
    /// transitions to `Real`
    /// on the first `embed`
    /// call. The 6 Phase 1
    /// conformance tests
    /// still pass because
    /// the offline policy
    /// (Refuse on strict +
    /// missing, Degraded on
    /// degraded + missing)
    /// is unchanged.
    ///
    /// Phase 1.5 also does
    /// NOT auto-download in
    /// the `Eager` strategy
    /// case (the `Eager`
    /// download is a Phase
    /// 4 story; Phase 1.5
    /// only supports the
    /// `Lazy` and `Never`
    /// strategies). The
    /// eager download is
    /// reached via
    /// `afa-cli embedding
    /// download` (the
    /// Pack #7a CLI).
    pub fn new(config: LocalEmbeddingConfig) -> Result<Self, EmbeddingErrorV1> {
        let model_path = config.model_dir.join(&config.model_name);
        let config_json = model_path.join("config.json");
        let model_present = config_json.exists();
        let policy = Offline::new(
            config.offline_mode,
            model_present,
            config.model_name.clone(),
        );
        let state = match policy.decide() {
            OfflineAction::Construct => AdapterState::Unloaded,
            OfflineAction::ConstructDegraded => AdapterState::Degraded,
            OfflineAction::Refuse(e) => return Err(e),
        };
        Ok(Self {
            config,
            state: std::sync::Mutex::new(state),
        })
    }

    /// Hand back a reference
    /// to the adapter's
    /// config. Used by the
    /// conformance suite to
    /// assert the adapter
    /// is built with the
    /// expected settings.
    pub fn config(&self) -> &LocalEmbeddingConfig {
        &self.config
    }

    /// Check whether the
    /// adapter is in
    /// degraded mode. Used
    /// by the conformance
    /// suite to assert the
    /// offline-mode logic.
    pub fn is_degraded(&self) -> bool {
        matches!(
            *self.state.lock().expect("adapter state mutex"),
            AdapterState::Degraded
        )
    }

    /// Ensure the model is
    /// loaded. The method
    /// is the lazy-load
    /// trampoline: if the
    /// adapter is
    /// `Unloaded`, it
    /// triggers a download
    /// (if the
    /// `download_strategy`
    /// is `Lazy`) and a
    /// model load. If the
    /// adapter is
    /// `Real` or
    /// `Degraded`, the
    /// method is a no-op.
    ///
    /// The download and
    /// the model load are
    /// both blocking I/O;
    /// the method is
    /// therefore `async`
    /// and wraps the
    /// blocking work in
    /// `tokio::task::spawn_blocking`
    /// so the async
    /// runtime can keep
    /// doing other work.
    async fn ensure_loaded(&self) -> Result<(), EmbeddingErrorV1> {
        // The state check is
        // a single Mutex
        // acquire (no I/O).
        let needs_load = {
            let state = self.state.lock().expect("adapter state mutex");
            matches!(*state, AdapterState::Unloaded)
        };
        if !needs_load {
            return Ok(());
        }
        let model_path = self.config.model_dir.join(&self.config.model_name);
        let downloader = Downloader::new(model_path.clone(), self.config.model_name.clone());
        let strategy = self.config.download_strategy;
        let model_path_for_blocking = model_path.clone();
        let result = task::spawn_blocking(move || -> Result<(), EmbeddingErrorV1> {
            if matches!(strategy, DownloadStrategy::Lazy) {
                downloader.download()?;
            }
            BertEmbedder::load(&model_path_for_blocking).map_err(EmbeddingErrorV1::from)?;
            Ok(())
        })
        .await
        .map_err(|e| EmbeddingErrorV1::Internal {
            reason: format!("spawn_blocking join error: {e}"),
        })?;
        result?;
        // The re-load (the
        // `spawn_blocking`
        // closure built
        // and dropped a
        // `BertEmbedder`;
        // we need to
        // build it again
        // in the async
        // context to
        // hold the
        // `Arc`).
        let embedder = task::spawn_blocking(move || BertEmbedder::load(&model_path))
            .await
            .map_err(|e| EmbeddingErrorV1::Internal {
                reason: format!("spawn_blocking join error: {e}"),
            })?
            .map_err(EmbeddingErrorV1::from)?;
        let mut state = self.state.lock().expect("adapter state mutex");
        *state = AdapterState::Real(Arc::new(embedder));
        Ok(())
    }
}

// CID:afa-plugin-embedding-local-adapter-002 - impl EmbeddingV1
// Purpose: The trait impl.
// `embed` and
// `embed_batch` both
// route through
// `ensure_loaded` (the
// lazy-load trampoline)
// and then dispatch on
// the `AdapterState`:
// `Real` → real
// forward pass,
// `Degraded` → zero
// vector,
// `Unloaded` → should
// not happen (the
// `ensure_loaded`
// call promoted the
// state).
//
// `describe_capabilities`
// returns the v1
// all-MiniLM-L6-v2 card
// (the model_name,
// dimension, max_batch_size,
// max_sequence_length,
// supports_batching are
// read from the
// loaded
// `BertEmbedder` if
// available, or
// hard-coded to the
// all-MiniLM-L6-v2
// values if the
// adapter is
// `Degraded` or
// `Unloaded`).
// Uses: EmbeddingV1,
// EmbeddingErrorV1,
// EmbeddingCapabilitiesV1,
// ExecutionContext.
// Used by: every
// workflow that calls
// `embed` /
// `embed_batch`.
#[async_trait]
impl EmbeddingV1 for LocalEmbeddingAdapter {
    async fn embed(
        &self,
        text: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Vec<f32>, EmbeddingErrorV1> {
        // Phase 1.5: the empty-input
        // check. The
        // `Tokenizer::encode`
        // method accepts
        // empty text (it
        // returns a single
        // `[CLS]` token or
        // similar), but the
        // downstream RAG
        // retrieval engine
        // never embeds
        // empty strings
        // (the ingestion
        // pipeline
        // short-circuits
        // empty chunks in
        // Pack #24). The
        // check fails fast
        // with
        // `InvalidInput` so
        // the bug is caught
        // at the call site
        // instead of
        // producing a
        // confusing
        // near-zero
        // embedding.
        if text.trim().is_empty() {
            return Err(EmbeddingErrorV1::InvalidInput {
                reason: "text is empty or whitespace-only; the embedding engine does not embed blank strings"
                    .to_string(),
            });
        }
        self.ensure_loaded().await?;
        // The `Arc<BertEmbedder>` decision is scoped to a block
        // so the `MutexGuard` is dropped at the closing brace
        // (the guard is `!Send`; the future must be `Send` for
        // the kernel to hold the adapter behind an
        // `Arc<dyn EmbeddingV1>` and call it from any thread).
        // If we held the lock across the `.await` below, the
        // compiler would reject the future as `!Send`.
        let embedder: Option<Arc<BertEmbedder>> = {
            let state = self.state.lock().expect("adapter state mutex");
            match &*state {
                AdapterState::Real(embedder) => Some(embedder.clone()),
                AdapterState::Degraded => None,
                AdapterState::Unloaded => {
                    return Err(EmbeddingErrorV1::Internal {
                        reason: "ensure_loaded did not promote the state".to_string(),
                    });
                }
            }
        };
        let embedder = match embedder {
            Some(e) => e,
            None => return Ok(degraded_vector()),
        };
        let texts = vec![text.to_string()];
        let result = task::spawn_blocking(move || embedder.embed_batch(&texts))
            .await
            .map_err(|e| EmbeddingErrorV1::Internal {
                reason: format!("spawn_blocking join error: {e}"),
            })?;
        let mut vectors = result.map_err(EmbeddingErrorV1::from)?;
        Ok(vectors.remove(0))
    }

    async fn embed_batch(
        &self,
        texts: &[String],
        _ctx: &ExecutionContext,
    ) -> Result<Vec<Vec<f32>>, EmbeddingErrorV1> {
        // Phase 1.5: the empty-input
        // check. Any empty
        // string in the
        // batch is rejected
        // (the whole batch
        // is rejected — the
        // operator must
        // filter empty
        // strings before
        // calling
        // `embed_batch`).
        if texts.is_empty() {
            return Ok(Vec::new());
        }
        if let Some(empty_idx) = texts.iter().position(|text| text.trim().is_empty()) {
            return Err(EmbeddingErrorV1::InvalidInput {
                reason: format!(
                    "text at index {empty_idx} is empty or whitespace-only; the embedding engine does not embed blank strings"
                ),
            });
        }
        self.ensure_loaded().await?;
        // Same pattern as `embed`: scope the
        // `MutexGuard` to a block so it is dropped
        // before the `.await`. See the comment in
        // `embed` for the full rationale.
        let (embedder, is_degraded) = {
            let state = self.state.lock().expect("adapter state mutex");
            match &*state {
                AdapterState::Real(embedder) => (Some(embedder.clone()), false),
                AdapterState::Degraded => (None, true),
                AdapterState::Unloaded => {
                    return Err(EmbeddingErrorV1::Internal {
                        reason: "ensure_loaded did not promote the state".to_string(),
                    });
                }
            }
        };
        if is_degraded {
            return Ok(texts.iter().map(|_| degraded_vector()).collect());
        }
        let embedder = embedder.expect("is_degraded=false implies Some");
        let texts = texts.to_vec();
        let result = task::spawn_blocking(move || embedder.embed_batch(&texts))
            .await
            .map_err(|e| EmbeddingErrorV1::Internal {
                reason: format!("spawn_blocking join error: {e}"),
            })?;
        result.map_err(EmbeddingErrorV1::from)
    }

    fn describe_capabilities(&self) -> EmbeddingCapabilitiesV1 {
        // The v1
        // all-MiniLM-L6-v2
        // card. The
        // dimension and
        // model_name
        // are read
        // from the
        // loaded
        // `BertEmbedder`
        // if the
        // adapter is
        // `Real`;
        // hard-coded
        // to the
        // all-MiniLM-L6-v2
        // values if
        // the adapter
        // is
        // `Degraded`
        // or
        // `Unloaded`.
        let state = self.state.lock().expect("adapter state mutex");
        let (model_name, dimension) = match &*state {
            AdapterState::Real(embedder) => {
                (self.config.model_name.clone(), embedder.dimension() as u32)
            }
            AdapterState::Degraded | AdapterState::Unloaded => {
                (self.config.model_name.clone(), 384)
            }
        };
        EmbeddingCapabilitiesV1 {
            model_name,
            dimension,
            max_batch_size: 64,
            max_sequence_length: 512,
            supports_batching: true,
        }
    }
}

// The `OfflineMode` import
// is `dead_code` in some
// builds (the policy logic
// lives in the `Offline`
// struct, but the
// `OfflineMode` import is
// needed for the public
// `LocalEmbeddingConfig`
// surface).
#[allow(dead_code)]
fn _offline_mode_keep(_: OfflineMode) {}

// The `Path` import is
// used by the
// `AdapterState::Real`
// arm of the match.
#[allow(dead_code)]
fn _path_keep(_: &Path) {}
