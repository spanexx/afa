//! Code Map: LLM error surface
//! - `LlmErrorV1`: The 13 typed "what went wrong with the
//!   LLM?" buckets. Closed set; adding a new variant is a
//!   deliberate ADR-backed change. The 13 variants are
//!   the locked shape from the TRD §2.2.10.
//! - `LlmErrorKind`: A human-readable re-export of
//!   `AfaErrorKind` (the 6 coarse buckets the kernel
//!   recognises) — exposed here so callers of the LLM
//!   module can name the kind without reaching into
//!   `error`.
//! - `impl AfaError for LlmErrorV1`: The 13-to-6 mapping
//!   from `LlmErrorV1` variants to `AfaErrorKind`s.
//!   Generic code (e.g. the conformance suite) can
//!   branch on the kind without naming the concrete
//!   variant.
//!
//! Story (plain English): The error set is the switchboard
//! operator's little chart of "why might a call fail?"
//! It has 13 rows (one per failure mode the OpenAI
//! Responses API can return) and 6 colours (the 6 coarse
//! buckets the kernel recognises — not-found,
//! not-allowed, temporarily-down, too-slow, not-supported,
//! plain broken). The operator picks the row, stamps the
//! colour, and hands the chart to the workflow. The
//! workflow knows the colour tells it what to do (retry,
//! give up, surface a 404) without needing to read the
//! row.
//!
//! CID Index:
//! CID:llm-error-001 -> LlmErrorV1
//! CID:llm-error-002 -> LlmErrorKind
//! CID:llm-error-003 -> impl AfaError for LlmErrorV1
//!
//! Quick lookup: rg -n "CID:llm-error-" crates/afa-contracts/src/llm/error.rs

use std::time::Duration;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::error::{AfaError, AfaErrorKind};

// CID:llm-error-002 - LlmErrorKind
// Purpose: A re-export of `AfaErrorKind` under a more
// specific name, so callers of the LLM module can
// name the kind without reaching into `error`. The
// underlying set is the same 6 coarse buckets; the
// re-export is a naming convenience, not a new
// type.
// Uses: `crate::error::AfaErrorKind`.
// Used by: callers of the LLM module that want to
// branch on the kind without naming the concrete
// `LlmErrorV1` variant.
pub use crate::error::AfaErrorKind as LlmErrorKind;

// CID:llm-error-001 - LlmErrorV1
// Purpose: The 13 typed "what went wrong with the LLM?"
// buckets. The closed set maps onto the 6 coarse
// `AfaErrorKind` buckets the kernel already understands,
// with no new kinds introduced (the `#[non_exhaustive]`
// on `AfaErrorKind` is what makes adding a new bucket a
// deliberate ADR-backed change — we are explicitly NOT
// adding one here). The variant names are the locked
// shape from the TRD §2.2.10. The fields on each
// variant carry the minimum information an operator
// needs to diagnose the failure (e.g.
// `ContextLengthExceeded { actual_tokens, max_tokens }`
// so they can see how close the prompt was to the
// cap).
// Uses: thiserror (for `Display` + `source()` impls and
// the `std::error::Error` derive), `AfaError` (for the
// kernel-wide kind mapping), `std::time::Duration` (for
// the `retry_after` and `elapsed` fields).
// Used by: every `LlmV1` method (and, transitively, by
// every workflow that calls `llm.complete` /
// `llm.stream_complete`).
#[derive(Debug, Clone, Serialize, Deserialize, Error)]
pub enum LlmErrorV1 {
    /// HTTP 401 (or a vendor-equivalent: "the bearer
    /// token is invalid"). The adapter's key-wiring
    /// pattern re-unseals the key and retries once
    /// before surfacing this. Maps to `Unauthorized`.
    #[error("authentication failed: {reason}")]
    AuthenticationFailed { reason: String },
    /// HTTP 429 (the vendor is throttling us). The
    /// `retry_after` is parsed from the
    /// `Retry-After` header if present. Maps to
    /// `Unavailable`.
    #[error("rate limited (retry after {retry_after:?})")]
    RateLimited { retry_after: Option<Duration> },
    /// HTTP 429 with a body indicating the quota is
    /// exhausted (not just throttled — a billing
    /// problem). Maps to `Unavailable`.
    #[error("quota exhausted: {reason}")]
    QuotaExhausted { reason: String },
    /// The prompt was too long for the model's
    /// `max_context_tokens` (HTTP 400 with
    /// `code: "context_length_exceeded"`). Maps to
    /// `CapabilityUnsupported` (the contract is
    /// known but not supported by this build — the
    /// prompt is too big).
    #[error("context length exceeded: {actual_tokens} tokens (max {max_tokens})")]
    ContextLengthExceeded { actual_tokens: u32, max_tokens: u32 },
    /// The vendor's safety policy refused (e.g. the
    /// prompt or the model's draft contained
    /// disallowed content). Maps to
    /// `CapabilityUnsupported`.
    #[error("content policy violation: {reason}")]
    ContentPolicyViolation { reason: String },
    /// The model's name was not recognised by the
    /// vendor (HTTP 404 with `code: "model_not_found"`).
    /// Maps to `NotFound`.
    #[error("model not found: {model}")]
    ModelNotFound { model: String },
    /// A `ToolDefinition::name` in the request was
    /// not recognised by the vendor. Maps to
    /// `NotFound`.
    #[error("tool not found: {tool_name}")]
    ToolNotFound { tool_name: String },
    /// The request was malformed (e.g. a required
    /// field was missing). A developer error, not a
    /// runtime condition. Maps to `Internal`.
    #[error("invalid request: {reason}")]
    InvalidRequest { reason: String },
    /// The vendor is unreachable (DNS, connection
    /// refused, TLS error). `http_status` is `None`
    /// because the request never reached the HTTP
    /// layer. Maps to `Unavailable`.
    #[error("upstream unavailable (http_status {http_status:?})")]
    UpstreamUnavailable { http_status: Option<u16> },
    /// The call exceeded its deadline. `elapsed` is
    /// the actual elapsed time. Maps to `Timeout`.
    #[error("timeout after {elapsed:?}")]
    Timeout { elapsed: Duration },
    /// The vendor sent unparseable JSON or a
    /// response shape we did not expect. Maps to
    /// `Internal`.
    #[error("malformed response: {reason}")]
    MalformedResponse { reason: String },
    /// The vendor's connection died mid-stream.
    /// Distinct from `UpstreamUnavailable` (which is
    /// a pre-call failure) — this one is a
    /// mid-stream failure. Maps to `Unavailable`.
    #[error("stream interrupted: {reason}")]
    StreamInterrupted { reason: String },
    /// Catch-all for unexpected internal failures
    /// (bugs, invariant violations). Maps to
    /// `Internal`.
    #[error("llm internal error: {reason}")]
    Internal { reason: String },
}

// CID:llm-error-003 - impl AfaError for LlmErrorV1
// Purpose: The 13-to-6 mapping from `LlmErrorV1`
// variants to `AfaErrorKind` buckets. The mapping is
// the locked shape from the TRD §2.2.10 table:
// `AuthenticationFailed` → `Unauthorized`,
// `RateLimited` / `QuotaExhausted` /
// `UpstreamUnavailable` / `StreamInterrupted` →
// `Unavailable`, `Timeout` → `Timeout`,
// `ContextLengthExceeded` /
// `ContentPolicyViolation` →
// `CapabilityUnsupported`, `ModelNotFound` /
// `ToolNotFound` → `NotFound`, everything else →
// `Internal`. Generic code (e.g. the conformance
// suite) can branch on the kind without naming the
// concrete variant.
// Uses: `AfaError`, `AfaErrorKind`.
// Used by: every generic caller that wants to react
// to the kind of trouble without knowing the exact
// error type.
impl AfaError for LlmErrorV1 {
    fn kind(&self) -> AfaErrorKind {
        match self {
            // Not allowed: the bearer token was rejected.
            Self::AuthenticationFailed { .. } => AfaErrorKind::Unauthorized,
            // Temporarily down: the vendor is throttling
            // us, has exhausted our quota, is unreachable,
            // or dropped the connection mid-stream.
            // (A quota-exhausted condition is "temporarily
            // down" in the same way a database outage is —
            // the fix is operator action (top up the
            // quota), not a client retry.)
            Self::RateLimited { .. }
            | Self::QuotaExhausted { .. }
            | Self::UpstreamUnavailable { .. }
            | Self::StreamInterrupted { .. } => AfaErrorKind::Unavailable,
            // Too slow.
            Self::Timeout { .. } => AfaErrorKind::Timeout,
            // The contract is known but not supported by
            // this build (the prompt is too big, or the
            // model refused for safety).
            Self::ContextLengthExceeded { .. } | Self::ContentPolicyViolation { .. } => {
                AfaErrorKind::CapabilityUnsupported
            }
            // The (model, tool) was not recognised.
            Self::ModelNotFound { .. } | Self::ToolNotFound { .. } => AfaErrorKind::NotFound,
            // Developer errors and unexpected internal
            // failures. A workflow should NOT silently
            // retry these — they are bugs.
            Self::InvalidRequest { .. }
            | Self::MalformedResponse { .. }
            | Self::Internal { .. } => AfaErrorKind::Internal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn authentication_failed_classifies_as_unauthorized() {
        let e = LlmErrorV1::AuthenticationFailed {
            reason: "bad key".into(),
        };
        assert_eq!(e.kind(), AfaErrorKind::Unauthorized);
    }

    #[test]
    fn rate_limited_classifies_as_unavailable_and_carries_retry_after() {
        // The `retry_after` field is the whole point
        // of this variant — a workflow that wants to
        // back off needs to know how long to wait.
        let e = LlmErrorV1::RateLimited {
            retry_after: Some(Duration::from_secs(2)),
        };
        assert_eq!(e.kind(), AfaErrorKind::Unavailable);
        // The Display impl must mention the retry
        // value so a log line tells the operator
        // how long the throttle is.
        assert!(format!("{e}").contains("retry after"));
    }

    #[test]
    fn context_length_exceeded_carries_both_token_counts() {
        // The fields `actual_tokens` and
        // `max_tokens` are what an operator needs
        // to see "we were 10 tokens over the cap" —
        // not a generic "too long" message.
        let e = LlmErrorV1::ContextLengthExceeded {
            actual_tokens: 128_010,
            max_tokens: 128_000,
        };
        assert_eq!(e.kind(), AfaErrorKind::CapabilityUnsupported);
        assert!(format!("{e}").contains("128010"));
        assert!(format!("{e}").contains("128000"));
    }

    #[test]
    fn not_found_variants_classify_as_not_found() {
        // The two "name was not recognised" variants
        // both map to `NotFound`. A workflow that
        // branches on the kind can handle them
        // together.
        assert_eq!(
            LlmErrorV1::ModelNotFound {
                model: "gpt-99".into()
            }
            .kind(),
            AfaErrorKind::NotFound
        );
        assert_eq!(
            LlmErrorV1::ToolNotFound {
                tool_name: "x".into()
            }
            .kind(),
            AfaErrorKind::NotFound
        );
    }

    #[test]
    fn internal_variants_classify_as_internal() {
        // The three "developer error / unexpected"
        // variants all map to `Internal` — a
        // workflow should NOT silently retry these
        // (they are bugs).
        assert_eq!(
            LlmErrorV1::InvalidRequest { reason: "x".into() }.kind(),
            AfaErrorKind::Internal
        );
        assert_eq!(
            LlmErrorV1::MalformedResponse { reason: "x".into() }.kind(),
            AfaErrorKind::Internal
        );
        assert_eq!(
            LlmErrorV1::Internal { reason: "x".into() }.kind(),
            AfaErrorKind::Internal
        );
    }

    #[test]
    fn all_thirteen_variants_have_a_kind_mapping() {
        // Regression-proof that the 13-to-6 mapping
        // is complete. If a future contributor
        // adds a 14th variant, the compiler will
        // force them to add a kind mapping (the
        // match is exhaustive on `Self`).
        let samples: Vec<LlmErrorV1> = vec![
            LlmErrorV1::AuthenticationFailed { reason: "x".into() },
            LlmErrorV1::RateLimited { retry_after: None },
            LlmErrorV1::QuotaExhausted { reason: "x".into() },
            LlmErrorV1::ContextLengthExceeded {
                actual_tokens: 1,
                max_tokens: 1,
            },
            LlmErrorV1::ContentPolicyViolation { reason: "x".into() },
            LlmErrorV1::ModelNotFound { model: "x".into() },
            LlmErrorV1::ToolNotFound {
                tool_name: "x".into(),
            },
            LlmErrorV1::InvalidRequest { reason: "x".into() },
            LlmErrorV1::UpstreamUnavailable { http_status: None },
            LlmErrorV1::Timeout {
                elapsed: Duration::from_secs(1),
            },
            LlmErrorV1::MalformedResponse { reason: "x".into() },
            LlmErrorV1::StreamInterrupted { reason: "x".into() },
            LlmErrorV1::Internal { reason: "x".into() },
        ];
        assert_eq!(samples.len(), 13, "the pack is 13 variants");
        for e in samples {
            // Every variant classifies into one of
            // the 6 known kinds. If a future
            // contributor adds a 7th kind without
            // updating this test, the assert below
            // will fail.
            let kind = e.kind();
            assert!(
                matches!(
                    kind,
                    AfaErrorKind::NotFound
                        | AfaErrorKind::Unauthorized
                        | AfaErrorKind::Unavailable
                        | AfaErrorKind::Timeout
                        | AfaErrorKind::CapabilityUnsupported
                        | AfaErrorKind::Internal
                ),
                "variant {e:?} mapped to unknown kind {kind:?}"
            );
        }
    }
}
