//! Code Map: Embedding capabilities
//! - `EmbeddingCapabilitiesV1`: The "what can this embedding
//!   adapter do?" description. Five fields: `model_name`
//!   (e.g. "all-MiniLM-L6-v2"), `dimension` (e.g. 384 for
//!   MiniLM, 768 for nomic-embed-text), `max_batch_size`
//!   (how many texts a single `embed_batch` call accepts),
//!   `max_sequence_length` (how many tokens a single text
//!   can contain before truncation), `supports_batching`
//!   (whether the adapter has a real batched forward pass,
//!   not just the default per-chunk loop).
//!
//! Story (plain English): The capabilities card is the
//! adapter's business card. It says "I am MiniLM, I emit
//! 384-dim vectors, I can handle 64 texts at once, I cap
//! each text at 512 tokens, and yes I have a real batched
//! forward pass." A workflow that needs to embed 10 000
//! chunks can glance at the card and say "I should split
//! into 156 batches of 64" before making the first call.
//!
//! CID Index:
//! CID:embedding-capabilities-001 -> EmbeddingCapabilitiesV1
//!
//! Quick lookup: rg -n "CID:embedding-capabilities-" crates/afa-contracts/src/embedding/capabilities.rs

use serde::{Deserialize, Serialize};

// CID:embedding-capabilities-001 - EmbeddingCapabilitiesV1
// Purpose: The static, immutable description of
// what the underlying model can do. The values
// are decided at adapter construction and never
// change for the process lifetime. The struct
// is `Clone` + `PartialEq` + `Eq` so a
// conformance test can assert "the mock
// adapter and the real adapter report the same
// capabilities" (the test pattern).
// Uses: serde (for the `afa-cli embedding
// status` JSON output).
// Used by: every workflow that calls
// `embed` / `embed_batch` (to size batches
// and to know the dimension for downstream
// vector storage), and `afa-cli embedding
// status` (to print the card).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingCapabilitiesV1 {
    /// The model identifier (e.g.
    /// "all-MiniLM-L6-v2",
    /// "nomic-embed-text",
    /// "text-embedding-3-small").
    pub model_name: String,
    /// The fixed dimension of the
    /// output vectors (e.g. 384 for
    /// MiniLM, 768 for nomic-embed).
    /// A workflow that builds an
    /// in-memory index allocates a
    /// `[f32; dimension]` per chunk
    /// based on this number.
    pub dimension: u32,
    /// The maximum number of texts
    /// a single `embed_batch` call
    /// accepts. The caller is
    /// responsible for chunking its
    /// input; an over-sized batch
    /// returns `InvalidInput`.
    pub max_batch_size: u32,
    /// The maximum number of
    /// tokens (after tokenization)
    /// a single text can contain
    /// before truncation. The
    /// adapter truncates at this
    /// boundary (no error).
    pub max_sequence_length: u32,
    /// Whether the adapter has a
    /// real batched forward pass
    /// (true for the local candle
    /// adapter and the Ollama
    /// adapter) or only the
    /// default per-chunk loop
    /// (true for the mock).
    pub supports_batching: bool,
}
