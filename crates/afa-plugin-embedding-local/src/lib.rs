//! Code Map: afa-plugin-embedding-local
//! - `LocalEmbeddingAdapter`: The concrete
//!   candle-based adapter the kernel registers
//!   via `CapabilityRegistry::register_embedding`.
//!   Phase 0 is a skeleton: the struct is built,
//!   the `EmbeddingV1` trait is implemented, but
//!   every `embed` / `embed_batch` call returns
//!   `EmbeddingErrorV1::Internal` (the "not yet
//!   implemented" sentinel). Phase 1 wires the
//!   candle model load, the lazy HuggingFace
//!   download, the batched forward pass, and the
//!   offline-mode logic.
//! - `LocalEmbeddingConfig`: The settings card
//!   the adapter is built with. Phase 0 declares
//!   the fields; Phase 1 reads them.
//!
//! Story (plain English): This is the entry
//! point of the `afa-plugin-embedding-local`
//! crate. It re-exports the two public types
//! the kernel and the conformance suite touch:
//! the adapter (what the kernel hands out) and
//! the config (the settings card). Everything
//! else is module-private.
//!
//! CID Index:
//! CID:afa-plugin-embedding-local-lib-001 -> module re-exports
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-local-lib-" crates/afa-plugin-embedding-local/src/lib.rs

pub mod adapter;
pub mod config;

pub use adapter::LocalEmbeddingAdapter;
pub use config::LocalEmbeddingConfig;
