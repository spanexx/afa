#![allow(clippy::doc_lazy_continuation)]
//! Code Map: afa-plugin-embedding-local
//! - `LocalEmbeddingAdapter`: The concrete
//!   candle-based adapter the kernel registers
//!   via `CapabilityRegistry::register_embedding`.
//!   Phase 1 wires the candle model load, the
//!   lazy HuggingFace download, and the strict /
//!   degraded mode logic.
//! - `LocalEmbeddingConfig`: The settings card
//!   the adapter is built with.
//! - `OfflineMode`, `DownloadStrategy`: The
//!   config enums.
//! - `BertEmbedder` (re-exported from `model`):
//!   The wrapped candle `BertModel` + tokenizer.
//! - `Downloader` (re-exported from `download`):
//!   The HuggingFace download helper with
//!   SHA-256 verification.
//!
//! Story (plain English): This is the entry
//! point of the `afa-plugin-embedding-local`
//! crate. It re-exports the 6 public types the
//! kernel and the conformance suite touch.
//! Everything else is module-private.
//!
//! CID Index:
//! CID:afa-plugin-embedding-local-lib-001 -> module re-exports
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-local-lib-" crates/afa-plugin-embedding-local/src/lib.rs

pub mod adapter;
pub mod config;
pub mod download;
pub mod model;
pub mod offline;

pub use adapter::LocalEmbeddingAdapter;
pub use config::{DownloadStrategy, LocalEmbeddingConfig, OfflineMode};
pub use download::Downloader;
pub use model::BertEmbedder;
