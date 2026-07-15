//! Code Map: Embedding Engine contract surface
//! - `EmbeddingV1`: The "I am an embedding engine" badge. The
//!   locked v1 trait every adapter implements. See
//!   `traits.rs`.
//! - `EmbeddingErrorV1`: The 4 typed "what went wrong with the
//!   embedding?" buckets and their mapping to the 6 coarse
//!   `AfaErrorKind`s. See `error.rs`.
//! - `EmbeddingCapabilitiesV1`: The "what can this embedding
//!   adapter do?" description (model name, dimension, max batch
//!   size, max sequence length, batching support). See
//!   `capabilities.rs`.
//! - `EmbeddingRequested`, `EmbeddingCompleted`,
//!   `EmbeddingFailed`: The three audit facts the adapter
//!   publishes on the event bus. See `events.rs`.
//!
//! Story (plain English): The Embedding Engine is the kernel's
//! "vector translator" — it turns a chunk of text into a
//! fixed-dimension `Vec<f32>` that downstream engines
//! (currently Pack #24 ingestion; future semantic search
//! packs) can compare with cosine similarity. The trait is the
//! dictionary: three methods (embed, embed_batch,
//! describe_capabilities) plus a static version constant. The
//! dictionary lives in `afa-contracts` because it is small,
//! never does I/O, and is the same for every deployment (a
//! real local adapter, a real Ollama adapter, a test mock, a
//! future OpenAI adapter all use exactly the same words).
//!
//! CID Index:
//! CID:embedding-001 -> EmbeddingV1
//! CID:embedding-002 -> EmbeddingErrorV1
//! CID:embedding-003 -> EmbeddingCapabilitiesV1
//! CID:embedding-004 -> EmbeddingRequested/Completed/Failed events
//!
//! Quick lookup: rg -n "CID:embedding-" crates/afa-contracts/src/embedding/

pub mod capabilities;
pub mod error;
pub mod events;
pub mod traits;

pub use capabilities::EmbeddingCapabilitiesV1;
pub use error::{EmbeddingErrorKind, EmbeddingErrorV1};
pub use events::{EmbeddingCompleted, EmbeddingFailed, EmbeddingRequested};
pub use traits::{EmbeddingV1, EmbeddingV1Version};
