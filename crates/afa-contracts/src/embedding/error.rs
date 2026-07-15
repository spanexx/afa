//! Code Map: Embedding error surface
//! - `EmbeddingErrorV1`: The 4 typed "what went wrong with the
//!   embedding?" buckets. Closed set; adding a new variant is a
//!   deliberate ADR-backed change. The 4 variants are the locked
//!   shape from the TRD §2.2.
//! - `EmbeddingErrorKind`: A human-readable re-export of
//!   `AfaErrorKind` (the 6 coarse buckets the kernel
//!   recognises) — exposed here so callers of the embedding
//!   module can name the kind without reaching into `error`.
//! - `impl AfaError for EmbeddingErrorV1`: The 4-to-6 mapping
//!   from `EmbeddingErrorV1` variants to `AfaErrorKind`s.
//!   Generic code (e.g. the conformance suite) can branch on
//!   the kind without naming the concrete variant.
//!
//! Story (plain English): The error set is the embedding
//! engine's little chart of "why might an embed call fail?"
//! It has 4 rows (one per failure mode the embedding
//! backends can return) and 6 colours (the 6 coarse buckets
//! the kernel recognises — not-found, not-allowed,
//! temporarily-down, too-slow, not-supported, plain broken).
//! The operator picks the row, stamps the colour, and hands
//! the chart to the workflow. The workflow knows the colour
//! tells it what to do (retry, give up, surface a 503)
//! without needing to read the row.
//!
//! CID Index:
//! CID:embedding-error-001 -> EmbeddingErrorV1
//! CID:embedding-error-002 -> EmbeddingErrorKind
//! CID:embedding-error-003 -> impl AfaError for EmbeddingErrorV1
//!
//! Quick lookup: rg -n "CID:embedding-error-" crates/afa-contracts/src/embedding/error.rs

use thiserror::Error;

use crate::error::{AfaError, AfaErrorKind};

// CID:embedding-error-002 - EmbeddingErrorKind
// Purpose: A re-export of `AfaErrorKind` under a more
// specific name, so callers of the embedding module
// can name the kind without reaching into `error`.
// The underlying set is the same 6 coarse buckets;
// the re-export is a naming convenience, not a new
// type. Mirrors the `LlmErrorKind` re-export in
// `llm/error.rs`.
// Uses: `crate::error::AfaErrorKind`.
// Used by: callers of the embedding module that want
// to branch on the kind without naming the concrete
// `EmbeddingErrorV1` variant.
pub use crate::error::AfaErrorKind as EmbeddingErrorKind;

// CID:embedding-error-001 - EmbeddingErrorV1
// Purpose: The 4 typed "what went wrong with the
// embedding?" buckets. The closed set maps onto the
// 6 coarse `AfaErrorKind` buckets the kernel already
// understands, with no new kinds introduced (the
// `#[non_exhaustive]` on `AfaErrorKind` is what makes
// adding a new bucket a deliberate ADR-backed change
// — we are explicitly NOT adding one here). The
// variant names are the locked shape from the TRD
// §2.2.
// Uses: thiserror (for `Display` + `source()` impls
// and the `std::error::Error` derive), `AfaError` (for
// the kernel-wide kind mapping).
// Used by: every `EmbeddingV1` method (and,
// transitively, by every workflow that calls
// `embed` / `embed_batch`).
#[derive(Debug, Clone, Error)]
pub enum EmbeddingErrorV1 {
    /// The adapter is unreachable (DNS,
    /// connection refused, TLS error,
    /// Ollama daemon not running). The
    /// adapter is in principle
    /// configurable but currently not
    /// available. Maps to `Unavailable`.
    #[error("adapter unavailable: {reason}")]
    AdapterUnavailable { reason: String },
    /// The model file is missing or
    /// cannot be loaded (HuggingFace
    /// download failed, safetensors
    /// header corrupted, Ollama model
    /// not pulled). The operator action
    /// is "download the model" or
    /// "switch offline mode to
    /// degraded". Maps to `Unavailable`.
    #[error("model unavailable: {model_name}: {reason}")]
    ModelUnavailable { model_name: String, reason: String },
    /// The input is invalid for this
    /// adapter (empty text, text
    /// exceeding `max_sequence_length`
    /// after truncation, batch larger
    /// than `max_batch_size`). A
    /// developer error or a caller
    /// misuse. Maps to `InvalidInput` —
    /// wait, the closed `AfaErrorKind`
    /// set has no `InvalidInput`. We
    /// map to `CapabilityUnsupported`
    /// (the contract is known but the
    /// input is not supported by this
    /// build) which is the closest
    /// existing bucket. This is a
    /// deliberate, documented choice —
    /// adding a new `AfaErrorKind`
    /// would be an ADR.
    #[error("invalid input: {reason}")]
    InvalidInput { reason: String },
    /// Catch-all for unexpected
    /// internal failures (bugs,
    /// invariant violations, the
    /// Phase 0 default impl returning
    /// this). Maps to `Internal`.
    #[error("embedding internal error: {reason}")]
    Internal { reason: String },
}

// CID:embedding-error-003 - impl AfaError for EmbeddingErrorV1
// Purpose: The 4-to-6 mapping from
// `EmbeddingErrorV1` variants to `AfaErrorKind`
// buckets. The mapping is the locked shape from
// the TRD §2.2 table: `AdapterUnavailable` /
// `ModelUnavailable` → `Unavailable`,
// `InvalidInput` → `CapabilityUnsupported` (no
// `InvalidInput` kind in the closed set; closest
// existing bucket), `Internal` → `Internal`.
// Generic code (e.g. the conformance suite) can
// branch on the kind without naming the concrete
// variant.
// Uses: `AfaError`, `AfaErrorKind`.
// Used by: every generic caller that wants to
// react to the kind of trouble without knowing
// the exact error type.
impl AfaError for EmbeddingErrorV1 {
    fn kind(&self) -> AfaErrorKind {
        match self {
            // Temporarily down: the
            // adapter or the model file
            // is not currently
            // reachable. The fix is
            // operator action (start
            // the daemon, download
            // the model) or a retry.
            Self::AdapterUnavailable { .. } | Self::ModelUnavailable { .. } => {
                AfaErrorKind::Unavailable
            }
            // The contract is known
            // but the input is not
            // supported by this
            // build (empty text, text
            // too long, batch too
            // large). The closest
            // existing `AfaErrorKind`
            // is `CapabilityUnsupported`.
            Self::InvalidInput { .. } => AfaErrorKind::CapabilityUnsupported,
            // Developer errors and
            // unexpected internal
            // failures. A workflow
            // should NOT silently
            // retry these — they
            // are bugs.
            Self::Internal { .. } => AfaErrorKind::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_unavailable_classifies_as_unavailable() {
        let e = EmbeddingErrorV1::AdapterUnavailable {
            reason: "ollama not running".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::Unavailable);
    }

    #[test]
    fn model_unavailable_classifies_as_unavailable() {
        let e = EmbeddingErrorV1::ModelUnavailable {
            model_name: "all-MiniLM-L6-v2".into(),
            reason: "model file missing".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::Unavailable);
    }

    #[test]
    fn invalid_input_classifies_as_capability_unsupported() {
        // The closed `AfaErrorKind` set has
        // no `InvalidInput`; the closest
        // existing bucket is
        // `CapabilityUnsupported`. This test
        // pins that mapping so a future
        // contributor does not accidentally
        // try to "fix" it to a new kind
        // (which would be an ADR).
        let e = EmbeddingErrorV1::InvalidInput {
            reason: "empty text".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::CapabilityUnsupported);
    }

    #[test]
    fn internal_classifies_as_internal() {
        let e = EmbeddingErrorV1::Internal {
            reason: "phase 0 default impl".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::Internal);
    }

    #[test]
    fn embedding_error_implements_std_error() {
        fn assert_std_error<E: std::error::Error>(_: &E) {}
        let e = EmbeddingErrorV1::Internal { reason: "x".into() };
        assert_std_error(&e);
    }

    #[test]
    fn all_four_variants_have_a_kind_mapping() {
        // Regression-proof that the 4-to-6
        // mapping is complete. If a future
        // contributor adds a 5th variant,
        // the compiler will force them to
        // add a kind mapping (the match is
        // exhaustive on `Self`).
        let samples: Vec<EmbeddingErrorV1> = vec![
            EmbeddingErrorV1::AdapterUnavailable { reason: "x".into() },
            EmbeddingErrorV1::ModelUnavailable {
                model_name: "x".into(),
                reason: "x".into(),
            },
            EmbeddingErrorV1::InvalidInput { reason: "x".into() },
            EmbeddingErrorV1::Internal { reason: "x".into() },
        ];
        assert_eq!(samples.len(), 4, "the pack is 4 variants");
        for e in samples {
            let kind = e.kind();
            assert!(
                matches!(
                    kind,
                    AfaErrorKind::Unavailable
                        | AfaErrorKind::CapabilityUnsupported
                        | AfaErrorKind::Internal
                ),
                "variant {e:?} mapped to unknown kind {kind:?}"
            );
        }
    }
}
