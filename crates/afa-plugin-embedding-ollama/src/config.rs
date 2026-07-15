//! Configuration types for the Ollama HTTP embedding adapter.
//!
//! Single file in the `config.rs` module of the
//! `afa-plugin-embedding-ollama` crate. Defines
//! `OllamaEmbeddingConfig` — the parsed shape of the
//! `[[embedding.adapters.ollama]]` TOML section the
//! `Kernel` reads at startup.
//!
//! **Why this exists in its own file (and not
//! inline in `lib.rs`):** the config is the
//! boundary between the operator's TOML file and
//! the runtime adapter. It is the single place
//! where the `Toml` ↔ `Runtime` deserialization
//! lives; the adapter reads the deserialized
//! `OllamaEmbeddingConfig` directly. Splitting it
//! out makes the adapter's inputs explicit and
//! keeps the test surface small (tests can build
//! the config by hand without going through TOML).
//!
//! **Hard rules (per ADR-013 and the PRDs):**
//! 1. `base_url` must parse as `http://` or
//!    `https://` and must not end with a trailing
//!    slash.
//! 2. `model` must be non-empty and lowercase
//!    ASCII (Ollama's `library/<model>` tags are
//!    strict-case).
//! 3. `timeout_secs` must be ≥ 1 and ≤ 600 (10
//!    minutes; anything longer than that is
//!    almost certainly a misconfiguration).
//! 4. `max_batch_size` must be ≥ 1 and ≤ 1024
//!    (Ollama's batched `/v1/embeddings` endpoint
//!    hard-limits at 1024 inputs per request).
//! 5. `keep_alive_secs` (the server-side model
//!    keep-alive window — Ollama's `keep_alive`
//!    parameter, default 5 minutes = 300s) must
//!    be ≥ 0; 0 means "unload immediately after
//!    the request finishes".
//! 6. All fields are `pub` so the `Kernel` and
//!    the conformance test fixtures can read
//!    them; the struct itself is `pub` so the
//!    `CapabilityRegistry` can move it into the
//!    adapter's `new()`.

use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Runtime configuration for the Ollama embedding adapter.
///
/// Parsed from the `[[embedding.adapters.ollama]]`
/// TOML section by the `Kernel` at startup.
///
/// # Example TOML
///
/// ```toml
/// [[embedding.adapters.ollama]]
/// name = "default-ollama"
/// base_url = "http://localhost:11434"
/// model = "nomic-embed-text"
/// timeout_secs = 30
/// max_batch_size = 100
/// keep_alive_secs = 300
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OllamaEmbeddingConfig {
    /// Adapter instance name (e.g. `default-ollama`).
    /// The `Kernel`'s `CapabilityRegistry` looks
    /// this up when an `EmbeddingV1` capability is
    /// requested. Must be unique within the
    /// `[embedding.adapters.*]` section.
    pub name: String,

    /// Ollama server URL, e.g. `http://localhost:11434`.
    /// Must start with `http://` or `https://` and
    /// must NOT end with a trailing slash (the
    /// adapter appends `/v1/embeddings` directly).
    pub base_url: String,

    /// Ollama model tag, e.g. `nomic-embed-text` or
    /// `all-minilm`. Must be non-empty lowercase
    /// ASCII (no spaces, no uppercase). The
    /// adapter sends this verbatim in the
    /// `model` field of the `/v1/embeddings`
    /// request body.
    pub model: String,

    /// Request timeout. Each HTTP call to
    /// `/v1/embeddings` is bounded by this
    /// duration. The retry-on-5xx loop does not
    /// count against this — only the per-call
    /// wall-clock time. Must be in `[1, 600]`.
    pub timeout_secs: u64,

    /// Maximum number of inputs the adapter will
    /// pack into a single `/v1/embeddings`
    /// request. Inputs beyond this are split
    /// into chunks of this size and sent as
    /// separate requests. Must be in `[1, 1024]`.
    pub max_batch_size: usize,

    /// Server-side model keep-alive window, in
    /// seconds. Passed as the `keep_alive`
    /// field of the `/v1/embeddings` request
    /// body. `0` means "unload immediately".
    /// Must be in `[0, 86400]` (24 hours).
    #[serde(default = "default_keep_alive_secs")]
    pub keep_alive_secs: u64,
}

impl OllamaEmbeddingConfig {
    /// Construct the request-time `Duration` for
    /// the HTTP timeout. Pulled into a `Duration`
    /// here so the `reqwest::ClientBuilder` can
    /// take it directly without re-converting.
    pub fn timeout(&self) -> Duration {
        Duration::from_secs(self.timeout_secs)
    }

    /// Construct the request-time `Duration` for
    /// the server-side `keep_alive` parameter.
    pub fn keep_alive(&self) -> Duration {
        Duration::from_secs(self.keep_alive_secs)
    }

    /// Validate the parsed config.
    ///
    /// Called by the `Kernel` after deserializing
    /// the TOML but before constructing the
    /// adapter. Returns the first `String` error
    /// found, with a human-readable message that
    /// names the offending field.
    ///
    /// # Errors
    ///
    /// Returns `Err(message)` if any field fails
    /// the hard rules above. The error message is
    /// suitable for logging at `warn!` level — the
    /// `Kernel` does that and aborts adapter
    /// registration (the registry remains
    /// available for the other adapters).
    pub fn validate(&self) -> Result<(), String> {
        // (1) name: non-empty
        if self.name.trim().is_empty() {
            return Err("ollama adapter: `name` must be non-empty".to_string());
        }

        // (2) base_url: starts with http(s)://, no trailing slash
        let trimmed = self.base_url.trim();
        if !(trimmed.starts_with("http://") || trimmed.starts_with("https://")) {
            return Err(format!(
                "ollama adapter `{}`: `base_url` must start with `http://` or `https://` (got `{}`)",
                self.name, trimmed
            ));
        }
        if trimmed.ends_with('/') {
            return Err(format!(
                "ollama adapter `{}`: `base_url` must not end with a trailing slash (got `{}`)",
                self.name, trimmed
            ));
        }

        // (3) model: non-empty, lowercase ASCII, no spaces
        if self.model.trim().is_empty() {
            return Err(format!(
                "ollama adapter `{}`: `model` must be non-empty",
                self.name
            ));
        }
        if self.model.contains(' ') {
            return Err(format!(
                "ollama adapter `{}`: `model` must not contain spaces (got `{}`)",
                self.name, self.model
            ));
        }
        if self
            .model
            .chars()
            .any(|c| c.is_ascii_uppercase() || !c.is_ascii())
        {
            return Err(format!(
                "ollama adapter `{}`: `model` must be lowercase ASCII (got `{}`)",
                self.name, self.model
            ));
        }

        // (4) timeout_secs: in [1, 600]
        if !(1..=600).contains(&self.timeout_secs) {
            return Err(format!(
                "ollama adapter `{}`: `timeout_secs` must be in [1, 600] (got `{}`)",
                self.name, self.timeout_secs
            ));
        }

        // (5) max_batch_size: in [1, 1024]
        if !(1..=1024).contains(&self.max_batch_size) {
            return Err(format!(
                "ollama adapter `{}`: `max_batch_size` must be in [1, 1024] (got `{}`)",
                self.name, self.max_batch_size
            ));
        }

        // (6) keep_alive_secs: in [0, 86400]
        if self.keep_alive_secs > 86_400 {
            return Err(format!(
                "ollama adapter `{}`: `keep_alive_secs` must be in [0, 86400] (got `{}`)",
                self.name, self.keep_alive_secs
            ));
        }

        Ok(())
    }
}

impl Default for OllamaEmbeddingConfig {
    fn default() -> Self {
        Self {
            name: "default-ollama".to_string(),
            base_url: "http://localhost:11434".to_string(),
            model: "nomic-embed-text".to_string(),
            timeout_secs: 30,
            max_batch_size: 100,
            keep_alive_secs: default_keep_alive_secs(),
        }
    }
}

/// Default value for `keep_alive_secs` (5 minutes
/// = 300 seconds — Ollama's documented default).
fn default_keep_alive_secs() -> u64 {
    300
}

#[cfg(test)]
mod tests {
    use super::*;

    // A complete, valid config — used as the base
    // for the negative tests below.
    fn valid() -> OllamaEmbeddingConfig {
        OllamaEmbeddingConfig {
            name: "default-ollama".to_string(),
            base_url: "http://localhost:11434".to_string(),
            model: "nomic-embed-text".to_string(),
            timeout_secs: 30,
            max_batch_size: 100,
            keep_alive_secs: 300,
        }
    }

    #[test]
    fn default_is_valid() {
        let cfg = OllamaEmbeddingConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn valid_passes() {
        assert!(valid().validate().is_ok());
    }

    #[test]
    fn empty_name_fails() {
        let mut cfg = valid();
        cfg.name = "".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn base_url_must_start_with_http() {
        let mut cfg = valid();
        cfg.base_url = "localhost:11434".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn base_url_https_is_ok() {
        let mut cfg = valid();
        cfg.base_url = "https://ollama.example.com".to_string();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn base_url_trailing_slash_fails() {
        let mut cfg = valid();
        cfg.base_url = "http://localhost:11434/".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn empty_model_fails() {
        let mut cfg = valid();
        cfg.model = "".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn model_with_space_fails() {
        let mut cfg = valid();
        cfg.model = "nomic embed".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn model_uppercase_fails() {
        let mut cfg = valid();
        cfg.model = "Nomic-Embed-Text".to_string();
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn timeout_zero_fails() {
        let mut cfg = valid();
        cfg.timeout_secs = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn timeout_too_large_fails() {
        let mut cfg = valid();
        cfg.timeout_secs = 601;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn max_batch_size_zero_fails() {
        let mut cfg = valid();
        cfg.max_batch_size = 0;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn max_batch_size_too_large_fails() {
        let mut cfg = valid();
        cfg.max_batch_size = 1025;
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn keep_alive_zero_is_ok() {
        let mut cfg = valid();
        cfg.keep_alive_secs = 0;
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn keep_alive_too_large_fails() {
        let mut cfg = valid();
        cfg.keep_alive_secs = 86_401;
        assert!(cfg.validate().is_err());
    }
}
