//! Code Map: afa-plugin-embedding-ollama — config
//! - `OllamaEmbeddingConfig`: The settings card
//!   the `OllamaEmbeddingAdapter` is built
//!   with. Phase 0 declares the fields; Phase 2
//!   reads them. The defaults are the v1
//!   operator-friendly values (Ollama URL
//!   `http://localhost:11434`, model name
//!   `nomic-embed-text`, timeout 5 seconds).
//!
//! Story (plain English): The config is the
//! little card the operator hands the adapter
//! at construction. It says "here is the
//! Ollama URL, here is the model name, here
//! is how long to wait for a response."
//!
//! CID Index:
//! CID:afa-plugin-embedding-ollama-config-001 -> OllamaEmbeddingConfig
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-ollama-config-" crates/afa-plugin-embedding-ollama/src/config.rs

use std::time::Duration;

/// The settings card the
/// `OllamaEmbeddingAdapter` is built with.
/// Phase 0 declares the fields; Phase 2
/// reads them.
///
/// Defaults (v1 operator-friendly values):
/// - `ollama_url = "http://localhost:11434"`
///   (the standard Ollama daemon URL)
/// - `model_name = "nomic-embed-text"`
///   (the standard 768-dim Ollama
///   embedding model)
/// - `timeout = 5 seconds` (the standard
///   Pack #4 LLM adapter timeout)
#[derive(Debug, Clone)]
pub struct OllamaEmbeddingConfig {
    /// The Ollama daemon URL. The
    /// adapter POSTs to
    /// `<ollama_url>/v1/embeddings`
    /// (the OpenAI-compatible endpoint
    /// Ollama added in v0.1.14, 2024).
    pub ollama_url: String,
    /// The Ollama model name (e.g.
    /// "nomic-embed-text",
    /// "mxbai-embed-large",
    /// "all-minilm"). The operator
    /// must `ollama pull <model>`
    /// before the adapter can use it.
    pub model_name: String,
    /// The HTTP request timeout. A
    /// request that exceeds this
    /// returns `AdapterUnavailable`
    /// (per the IMPL §"Phase 2
    /// timeout test").
    pub timeout: Duration,
}

impl Default for OllamaEmbeddingConfig {
    fn default() -> Self {
        Self {
            ollama_url: "http://localhost:11434".to_string(),
            model_name: "nomic-embed-text".to_string(),
            timeout: Duration::from_secs(5),
        }
    }
}
