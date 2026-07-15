//! Code Map: afa-contract-testing — embedding mock
//! - `MockEmbeddingAdapter`: The reference
//!   adapter the conformance suite runs
//!   against. Conforms to the `EmbeddingV1`
//!   contract with a deterministic,
//!   pure-Rust vector function and a pinned
//!   capabilities card. The mock exists so
//!   the conformance suite can always run
//!   without touching the network or the
//!   file system.
//!
//! Story (plain English): The mock is the
//! "perfect student" the driving school
//! uses to demo the score sheet before any
//! real student has been enrolled. It
//! passes the same observable contract the
//! real adapters must pass: same input →
//! same vector, different input → different
//! vector, empty input → `InvalidInput`,
//! stable capabilities, no I/O. The real adapters
//! (`LocalEmbeddingAdapter` in Phase 1,
//! `OllamaEmbeddingAdapter` in Phase 2)
//! are the "real students" that must
//! pass the same challenges with real
//! behavior.
//!
//! CID Index:
//! CID:embedding-mock-001 -> MockEmbeddingAdapter
//! CID:embedding-mock-002 -> impl EmbeddingV1
//!
//! Quick lookup: rg -n "CID:embedding-mock-" crates/afa-contract-testing/src/embedding/mock.rs

use async_trait::async_trait;

use afa_contracts::{EmbeddingCapabilitiesV1, EmbeddingErrorV1, EmbeddingV1, ExecutionContext};

// CID:embedding-mock-001 - MockEmbeddingAdapter
// Purpose: Reference adapter the
// conformance suite runs against.
// Pure-Rust, no I/O, deterministic.
// The fields are the pinned
// capabilities card so the suite can
// compare observable output against
// advertised shape.
#[derive(Debug, Clone)]
pub struct MockEmbeddingAdapter {
    caps: EmbeddingCapabilitiesV1,
}

impl Default for MockEmbeddingAdapter {
    fn default() -> Self {
        Self {
            caps: EmbeddingCapabilitiesV1 {
                model_name: "all-MiniLM-L6-v2".to_string(),
                dimension: 384,
                max_batch_size: 64,
                max_sequence_length: 512,
                supports_batching: true,
            },
        }
    }
}

fn deterministic_embedding(text: &str, dimension: usize) -> Vec<f32> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(dimension);
    let mut state = 0xcbf2_9ce4_8422_2325_u64;

    for i in 0..dimension {
        for &b in bytes {
            state ^= (b as u64) ^ (i as u64);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
            state = state.rotate_left(5);
        }
        let unit = (state as f64 / u64::MAX as f64) as f32;
        out.push((unit * 2.0) - 1.0);
    }

    let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut out {
            *value /= norm;
        }
    }
    out
}

// CID:embedding-mock-002 - impl EmbeddingV1
// Purpose: Trait impl. The mock returns
// deterministic unit-length vectors for
// non-empty input, rejects empty input,
// and uses the default `embed_batch`
// impl so the conformance suite still
// exercises the trait-level batching
// path.
#[async_trait]
impl EmbeddingV1 for MockEmbeddingAdapter {
    async fn embed(
        &self,
        text: &str,
        _ctx: &ExecutionContext,
    ) -> Result<Vec<f32>, EmbeddingErrorV1> {
        if text.trim().is_empty() {
            return Err(EmbeddingErrorV1::InvalidInput {
                reason: "mock adapter: text must be non-empty and not whitespace-only".to_string(),
            });
        }
        Ok(deterministic_embedding(text, self.caps.dimension as usize))
    }

    fn describe_capabilities(&self) -> EmbeddingCapabilitiesV1 {
        self.caps.clone()
    }
}
