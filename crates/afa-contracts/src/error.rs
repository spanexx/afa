//! Code Map: Error convention
//! - `AfaErrorKind`: The six coarse "what kind of trouble was
//!   this?" buckets the kernel recognises — not-found,
//!   not-allowed, temporarily-down, too-slow, not-supported, or
//!   plain broken.
//! - `AfaError`: A "I'm an error" badge. Every error type in the
//!   kernel wears it, so generic code can ask any error "what
//!   bucket are you?" without naming the concrete type.
//! - `ExampleStoreErrorV1`: A sample error type used by the
//!   conformance tests to prove the badge works. Not a real
//!   production error — the first real one comes with
//!   `kernel-core`.
//!
//! Story (plain English): Imagine a hospital's intake desk. Every
//! person who arrives gets a chart. The chart is a different
//! shape depending on why they came in (a fever chart, a broken
//! bone chart, an X-ray chart), but they all share one big
//! coloured sticker: "this is an intake problem." That sticker
//! (`AfaError`) and the small set of possible colours
//! (`AfaErrorKind`) are what we agree on. The actual chart
//! (`ExampleStoreErrorV1`, and later the real production errors)
//! can have as many boxes and lines as it needs. Adding a brand
//! new colour to the sticker set is a deliberate decision (a new
//! ADR), not a mistake — which is why the colour set is marked
//! `#[non_exhaustive]`.
//!
//! CID Index:
//! CID:error-001 -> AfaErrorKind
//! CID:error-002 -> AfaError
//! CID:error-003 -> ExampleStoreErrorV1
//!
//! Quick lookup: rg -n "CID:error-" crates/afa-contracts/src/error.rs

use serde::{Deserialize, Serialize};

// CID:error-001 - AfaErrorKind
// Purpose: The closed set of "what kind of trouble" buckets the
// kernel recognises. Generic code branches on these (e.g. "if
// NotFound, return 404; if Unauthorized, return 401") without
// naming the concrete error type. Marked `#[non_exhaustive]` so
// adding a new bucket is always a deliberate ADR-backed change.
// Uses: nothing — it is just a label.
// Used by: every `AfaError` implementor (each one maps itself to
// a bucket), and every generic caller that wants to react to the
// kind of trouble without knowing the exact error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AfaErrorKind {
    /// The requested resource does not exist.
    NotFound,
    /// The caller is not authorised to perform the operation.
    Unauthorized,
    /// The dependency is reachable in principle but currently
    /// unavailable (network down, service restarting, etc.).
    Unavailable,
    /// The operation exceeded its deadline.
    Timeout,
    /// The contract is known but not supported by this build (e.g. a
    /// plugin's interface variant is missing or disabled).
    CapabilityUnsupported,
    /// Catch-all for unexpected internal failures (bugs, invariant
    /// violations).
    Internal,
}

// CID:error-002 - AfaError
// Purpose: The "I'm an error" badge. Anything that wants the
// kernel to handle it as an error wears this. The one rule every
// implementor must follow: tell me which `AfaErrorKind` bucket
// you belong in.
// Uses: std::error::Error (the standard Rust error trait).
// Used by: every concrete error type in the kernel
// (ExampleStoreErrorV1 below, plus all the real ones that arrive
// in later packs), and every generic caller that needs to react
// to errors without naming the type.
pub trait AfaError: std::error::Error + Send + Sync + 'static {
    fn kind(&self) -> AfaErrorKind;
}

// CID:error-003 - ExampleStoreErrorV1
// Purpose: A sample error type used only by the conformance
// tests, to prove the `AfaError` badge and the
// `AfaErrorKind` mapping work end-to-end. It is not a real
// production error — the first real one arrives with
// `kernel-core`.
// Uses: thiserror (a tiny helper that makes writing error types
// less noisy).
// Used by: the conformance test in `afa-contract-testing`, and
// by every `AfaError` example in the docs.
#[derive(Debug, thiserror::Error)]
pub enum ExampleStoreErrorV1 {
    #[error("lesson not found: {0}")]
    NotFound(String),
    #[error("storage is temporarily unavailable")]
    Unavailable,
    #[error("internal invariant broken: {0}")]
    Internal(String),
    #[error("deadline exceeded after {0:?}")]
    Timeout(Option<std::time::Duration>),
}

impl AfaError for ExampleStoreErrorV1 {
    fn kind(&self) -> AfaErrorKind {
        match self {
            Self::NotFound(_) => AfaErrorKind::NotFound,
            Self::Unavailable => AfaErrorKind::Unavailable,
            Self::Internal(_) => AfaErrorKind::Internal,
            Self::Timeout(_) => AfaErrorKind::Timeout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn not_found_variant_classifies_as_not_found() {
        let err = ExampleStoreErrorV1::NotFound("lesson-42".into());
        assert_eq!(err.kind(), AfaErrorKind::NotFound);
    }

    #[test]
    fn internal_variant_classifies_as_internal() {
        let err = ExampleStoreErrorV1::Internal("bad state".into());
        assert_eq!(err.kind(), AfaErrorKind::Internal);
    }

    #[test]
    fn timeout_variant_classifies_as_timeout() {
        let err = ExampleStoreErrorV1::Timeout(Some(std::time::Duration::from_millis(50)));
        assert_eq!(err.kind(), AfaErrorKind::Timeout);
    }

    #[test]
    fn unavailable_variant_classifies_as_unavailable() {
        let err = ExampleStoreErrorV1::Unavailable;
        assert_eq!(err.kind(), AfaErrorKind::Unavailable);
    }

    #[test]
    fn example_store_error_implements_std_error() {
        fn assert_std_error<E: std::error::Error>(_: &E) {}
        let err = ExampleStoreErrorV1::NotFound("x".into());
        assert_std_error(&err);
    }

    #[test]
    fn generic_classification_without_concrete_type() {
        // A function bounded only on `impl AfaError` can branch on
        // `.kind()` without naming `ExampleStoreErrorV1`.
        fn classify(err: &dyn AfaError) -> &'static str {
            match err.kind() {
                AfaErrorKind::NotFound => "missing",
                AfaErrorKind::Internal => "broken",
                _ => "other",
            }
        }
        let err = ExampleStoreErrorV1::NotFound("x".into());
        assert_eq!(classify(&err), "missing");
        let err = ExampleStoreErrorV1::Internal("x".into());
        assert_eq!(classify(&err), "broken");
    }
}
