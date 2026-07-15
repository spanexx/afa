//! Code Map: afa-plugin-embedding-local — config
//! - `LocalEmbeddingConfig`: The settings card
//!   the `LocalEmbeddingAdapter` is built with.
//!   Phase 0 declares the fields; Phase 1 reads
//!   them (model path, offline mode, download
//!   strategy). The defaults are the v1
//!   operator-friendly values (model name
//!   "all-MiniLM-L6-v2", offline mode strict,
//!   download strategy lazy).
//!
//! Story (plain English): The config is the
//! little card the operator hands the adapter
//! at construction. It says "here is the model
//! name, here is the directory it lives in,
//! here is what to do if the model is missing
//! (refuse, or fall back to a sentinel), and
//! here is what to do if a download is needed
//! (do it now, do it on the first embed, or
//! never)."
//!
//! CID Index:
//! CID:afa-plugin-embedding-local-config-001 -> LocalEmbeddingConfig
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-local-config-" crates/afa-plugin-embedding-local/src/config.rs

use std::path::PathBuf;

/// The settings card the
/// `LocalEmbeddingAdapter` is built with.
/// Phase 0 declares the fields; Phase 1
/// reads them.
///
/// Defaults (v1 operator-friendly values,
/// per ADR-025, ADR-026, ADR-027, ADR-028):
/// - `model_name = "all-MiniLM-L6-v2"`
/// - `model_dir = "<afa_data_root>/embedding/models"`
/// - `offline_mode = "strict"` (refuse if
///   the model file is missing)
/// - `download_strategy = "lazy"` (download
///   on the first `embed` call, not at
///   adapter construction)
#[derive(Debug, Clone)]
pub struct LocalEmbeddingConfig {
    /// The model name (HuggingFace
    /// identifier, e.g. "all-MiniLM-L6-v2"
    /// or "bge-small-en-v1.5"). Phase 1
    /// uses this to pick the right files
    /// from the local model directory and
    /// to publish the model name on the
    /// audit event.
    pub model_name: String,
    /// The directory the model files live
    /// in (or will be downloaded to).
    /// Phase 1 reads `<model_dir>/<model_name>/config.json`,
    /// `<model_dir>/<model_name>/tokenizer.json`,
    /// and `<model_dir>/<model_name>/model.safetensors`.
    pub model_dir: PathBuf,
    /// What to do if the model file is
    /// missing. `"strict"` returns
    /// `EmbeddingErrorV1::ModelUnavailable`
    /// (the operator must pre-download the
    /// model or the adapter refuses to
    /// construct). `"degraded"` constructs
    /// the adapter and returns a sentinel
    /// zero vector on every `embed` call
    /// (useful for CI environments that
    /// cannot download the model).
    pub offline_mode: OfflineMode,
    /// When to download the model.
    /// `"eager"` downloads at adapter
    /// construction. `"lazy"` downloads on
    /// the first `embed` call. `"never"`
    /// never downloads (the operator must
    /// pre-place the files).
    pub download_strategy: DownloadStrategy,
}

/// The offline-mode policy. Closed set;
/// adding a new variant is a deliberate
/// ADR-backed change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OfflineMode {
    /// Refuse to construct the adapter if
    /// the model file is missing.
    Strict,
    /// Construct the adapter even if the
    /// model file is missing; return a
    /// sentinel zero vector on every
    /// `embed` call. The first call
    /// publishes an `EmbeddingModelDegraded`
    /// event.
    Degraded,
}

/// The download strategy. Closed set;
/// adding a new variant is a deliberate
/// ADR-backed change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadStrategy {
    /// Download at adapter construction.
    /// Fails fast if the network is down.
    Eager,
    /// Download on the first `embed`
    /// call. The first call may take
    /// 30-60 seconds for an 80 MB model.
    Lazy,
    /// Never download. The operator
    /// must pre-place the model files
    /// in `model_dir`.
    Never,
}

impl Default for LocalEmbeddingConfig {
    fn default() -> Self {
        // The defaults match the v1
        // operator-friendly values from
        // the IMPL §4 baseline section.
        // The `model_dir` default of
        // `./models` is the developer
        // convenience; production
        // installs override it via
        // `afa.toml[embedding.model_dir]`.
        Self {
            model_name: "all-MiniLM-L6-v2".to_string(),
            model_dir: PathBuf::from("./models"),
            offline_mode: OfflineMode::Strict,
            download_strategy: DownloadStrategy::Lazy,
        }
    }
}
