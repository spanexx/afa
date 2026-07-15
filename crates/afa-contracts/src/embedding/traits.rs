//! Code Map: Embedding V1 trait
//!
//! CID Index:
//! CID:embedding-traits-001 -> EmbeddingV1Version
//! CID:embedding-traits-002 -> EmbeddingV1
//! CID:embedding-traits-003 -> default embed_batch impl
//!
//! Quick lookup: rg -n "CID:embedding-traits-" crates/afa-contracts/src/embedding/traits.rs

use async_trait::async_trait;

use crate::execution_context::ExecutionContext;

use super::capabilities::EmbeddingCapabilitiesV1;
use super::error::EmbeddingErrorV1;

// CID:embedding-traits-001 - EmbeddingV1Version
// Purpose: The locked version string for the
// EmbeddingV1 contract. Bumped only via an ADR.
// Uses: nothing (just a `&str`).
// Used by: tests (to assert the contract
// version is what they expect), `afa-cli
// embedding status` (to print the contract
// version), downstream consumers (to log the
// version they were built against).
// The `#[allow(non_upper_case_globals)]` is
// deliberate: the "V1" is part of the name
// (matching the `EmbeddingV1` trait) and the
// Rust convention of `SCREAMING_SNAKE_CASE` for
// `const` items would produce the awkward
// `EMBEDDING_V1_VERSION`. The allow keeps the
// IMPL/PRD/TRD naming convention.
#[allow(non_upper_case_globals)]
pub const EmbeddingV1Version: &str = "1.0.0";

// CID:embedding-traits-002 - EmbeddingV1
// Purpose: The locked v1 contract for any
// embedding engine adapter. Three methods:
// `embed` (a single text → a single
// `Vec<f32>`), `embed_batch` (a slice of
// texts → a parallel `Vec<Vec<f32>>`), and
// `describe_capabilities` (sync, returns the
// static `EmbeddingCapabilitiesV1` of the
// underlying model). All async methods are
// `#[async_trait]`-decorated so adapters can
// be held behind `Arc<dyn EmbeddingV1>` in
// the CapabilityRegistry. The `Send + Sync`
// supertrait lets the registry share one
// adapter across many concurrent callers.
//
// The default `embed_batch` implementation
// (CID:embedding-traits-003) loops over
// `embed` and concatenates the results.
// Adapters with a true batched forward pass
// (e.g. `LocalEmbeddingAdapter` using
// candle's batched matmul) override it for
// throughput.
// Uses: ExecutionContext,
// EmbeddingCapabilitiesV1, EmbeddingErrorV1.
// Used by: the CapabilityRegistry (holds
// `Arc<dyn EmbeddingV1>`), Pack #24
// (ingestion, which calls `embed_batch` to
// embed chunks), and the conformance suite
// in `afa-contract-testing`.
#[async_trait]
pub trait EmbeddingV1: Send + Sync + 'static {
    /// Embed a single text into a
    /// fixed-dimension `Vec<f32>`. The
    /// returned vector is L2-normalized
    /// (unit length) so downstream cosine
    /// similarity is just a dot product.
    /// On error returns one of the 4 typed
    /// `EmbeddingErrorV1` variants.
    /// Publishes `EmbeddingRequested`
    /// before the vendor call, then either
    /// `EmbeddingCompleted` on success or
    /// `EmbeddingFailed` on failure.
    async fn embed(&self, text: &str, ctx: &ExecutionContext)
        -> Result<Vec<f32>, EmbeddingErrorV1>;

    /// Embed a batch of texts into a
    /// parallel `Vec<Vec<f32>>`. Each
    /// output vector corresponds 1:1 with
    /// the input text at the same index.
    /// On error returns one of the 4 typed
    /// `EmbeddingErrorV1` variants (the
    /// whole batch fails; partial results
    /// are not returned). Publishes
    /// `EmbeddingRequested` first, then
    /// either `EmbeddingCompleted` (with
    /// the count of texts) on success or
    /// `EmbeddingFailed` on failure.
    ///
    /// The default implementation loops
    /// over `embed`; adapters with a real
    /// batched forward pass override it.
    async fn embed_batch(
        &self,
        texts: &[String],
        ctx: &ExecutionContext,
    ) -> Result<Vec<Vec<f32>>, EmbeddingErrorV1> {
        let mut out = Vec::with_capacity(texts.len());
        for t in texts {
            // The default impl is per-chunk;
            // it cannot be parallelised
            // without making the trait
            // surface more complex (e.g.
            // returning a `Stream`). The
            // adapters that want throughput
            // override this method.
            let v = self.embed(t, ctx).await?;
            out.push(v);
        }
        Ok(out)
    }

    /// Synchronously return the static
    /// capabilities of the underlying
    /// model (`model_name`, `dimension`,
    /// `max_batch_size`,
    /// `max_sequence_length`,
    /// `supports_batching`). The values
    /// are decided at adapter construction
    /// and never change for the process
    /// lifetime. No `async`, no `ctx`, no
    /// I/O.
    fn describe_capabilities(&self) -> EmbeddingCapabilitiesV1;
}
