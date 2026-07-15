//! `afa-plugin-embedding-ollama` — Ollama HTTP
//! embedding adapter for the AFA Embedding engine.
//!
//! Crate root. Re-exports the public surface:
//! - [`OllamaEmbeddingAdapter`] — the `EmbeddingV1`
//!   impl that POSTs to `<ollama_url>/v1/embeddings`.
//! - [`OllamaEmbeddingConfig`] — the parsed TOML
//!   config (`base_url`, `model`, `timeout_secs`,
//!   `max_batch_size`, `keep_alive_secs`).
//! - [`OllamaHttpClient`] — the wire-level HTTP
//!   client (re-exported so the conformance tests
//!   can spin one up against `wiremock-rs`).
//!
//! **Why a separate crate (and not a module of
//! `afa-plugin-embedding-local`):** the local and
//! Ollama adapters have completely different
//! backends (candle CPU vs. reqwest HTTP) and
//! different dependency footprints. The local
//! crate pulls in `candle-core 0.11.0` (~50 MB
//! compiled) + `tokenizers 0.23.1` + `sha2 0.11.0`;
//! the Ollama crate pulls in `reqwest 0.12` +
//! `rustls 0.23` (~5 MB compiled). Separating
//! them keeps the dependency graphs small and
//! the per-crate build times fast — operators
//! who only want the HTTP adapter (e.g. on a
//! container without AVX) don't pay the candle
//! build cost.
//!
//! **Lifecycle:**
//! 1. The `Kernel` reads the
//!    `[[embedding.adapters.ollama]]` TOML
//!    section at startup.
//! 2. It calls `OllamaEmbeddingConfig::validate()`
//!    (returns `Result<(), String>`).
//! 3. It calls `OllamaEmbeddingAdapter::new(cfg)`
//!    and registers the adapter with the
//!    `CapabilityRegistry` under the
//!    `EmbeddingV1` capability.
//! 4. The engine code asks the registry for an
//!    `EmbeddingV1`; the registry hands back the
//!    adapter; the engine calls `embed` /
//!    `embed_batch`.
//!
//! **Phase 2 scope** (per the IMPL §"Phase 2 —
//! Ollama Adapter (HTTP-Based) — PENDING"):
//! - POST to `<base_url>/v1/embeddings` with
//!   `{"model", "input", "keep_alive"}` body.
//! - Parse the `{"data": [{"index", "embedding"}]}`
//!   response, sort by `index`, return.
//! - Retry on 5xx up to 3 times with
//!   1s/2s/4s exponential backoff.
//! - Map 4xx → `InvalidInput` (404 →
//!   `ModelUnavailable`); 5xx → `Internal`;
//!   network errors → `AdapterUnavailable`.
//! - Reject empty input BEFORE any HTTP call
//!   (`InvalidInput`).
//! - Log a structured `info!` event at
//!   registration time.
//!
//! **Out of scope for Phase 2** (later phases):
//! - Auto-pull the model if 404 (Phase 3 will
//!   add the Ollama `/api/pull` integration).
//! - Caching the model in `keep_alive_secs` is
//!   already sent to the server; the adapter
//!   does not need a local cache.
//! - Concurrent request batching (Phase 4).

pub mod adapter;
pub mod client;
pub mod config;

pub use adapter::OllamaEmbeddingAdapter;
pub use client::{
    OllamaEmbedRequest, OllamaEmbedResponse, OllamaEmbedResponseItem, OllamaHttpClient,
};
pub use config::OllamaEmbeddingConfig;
