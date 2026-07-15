//! Code Map: afa-plugin-embedding-ollama
//! - `OllamaEmbeddingAdapter`: The concrete
//!   HTTP adapter the kernel registers
//!   via `CapabilityRegistry::register_embedding`.
//!   Phase 0 is a skeleton: the struct is built,
//!   the `EmbeddingV1` trait is implemented, but
//!   every `embed` / `embed_batch` call returns
//!   `EmbeddingErrorV1::Internal` (the "not yet
//!   implemented" sentinel). Phase 2 wires the
//!   HTTP client, the request building, the
//!   response parsing, and the offline-mode
//!   logic.
//! - `OllamaEmbeddingConfig`: The settings card
//!   the adapter is built with. Phase 0 declares
//!   the fields; Phase 2 reads them (Ollama URL,
//!   model name, request timeout).
//!
//! Story (plain English): This is the entry
//! point of the `afa-plugin-embedding-ollama`
//! crate. It re-exports the two public types
//! the kernel and the conformance suite touch:
//! the adapter (what the kernel hands out) and
//! the config (the settings card). Everything
//! else is module-private.
//!
//! CID Index:
//! CID:afa-plugin-embedding-ollama-lib-001 -> module re-exports
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-ollama-lib-" crates/afa-plugin-embedding-ollama/src/lib.rs

pub mod adapter;
pub mod config;

pub use adapter::OllamaEmbeddingAdapter;
pub use config::OllamaEmbeddingConfig;
