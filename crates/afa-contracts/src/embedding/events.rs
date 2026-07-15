//! Code Map: Embedding audit events
//! - `EmbeddingRequested`: The "an embed was requested" audit
//!   fact. Published before the vendor call. Carries the
//!   number of texts (1 for `embed`, N for `embed_batch`).
//! - `EmbeddingCompleted`: The "an embed succeeded" audit
//!   fact. Published on success. Carries the count of
//!   vectors produced.
//! - `EmbeddingFailed`: The "an embed failed" audit fact.
//!   Published on any of the 4 typed errors. Carries the
//!   error message.
//!
//! Story (plain English): The three tickets the embedding
//! engine stamps on the audit log. A reader can later
//! reconstruct "who asked for what, and did it work?"
//! without ever reading the text content of the embedded
//! chunks.
//!
//! CID Index:
//! CID:embedding-events-001 -> EmbeddingRequested
//! CID:embedding-events-002 -> EmbeddingCompleted
//! CID:embedding-events-003 -> EmbeddingFailed
//!
//! Quick lookup: rg -n "CID:embedding-events-" crates/afa-contracts/src/embedding/events.rs

use serde::{Deserialize, Serialize};

use crate::ids::{CorrelationId, TenantId};

// CID:embedding-events-001 - EmbeddingRequested
// Purpose: The "an embed was requested" audit
// fact. Published before the vendor call so a
// reader can correlate the request with the
// completion or failure event that follows. The
// `text_count` field distinguishes `embed` (1)
// from `embed_batch` (N). The `correlation_id`
// and `tenant_id` are propagated from the
// `ExecutionContext` so an operator can
// reconstruct "who asked for what" from the
// audit log alone.
// Uses: CorrelationId, TenantId.
// Used by: every `EmbeddingV1` method (via the
// event bus).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingRequested {
    pub correlation_id: CorrelationId,
    pub tenant_id: TenantId,
    pub model_name: String,
    pub text_count: u32,
}

// CID:embedding-events-002 - EmbeddingCompleted
// Purpose: The "an embed succeeded" audit fact.
// Published after the vendor returns vectors.
// The `vector_count` is the number of vectors
// produced (1 for `embed`, N for
// `embed_batch`). The `duration_ms` lets an
// operator spot a regression in adapter
// performance over time.
// Uses: CorrelationId, TenantId.
// Used by: every successful `EmbeddingV1`
// method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingCompleted {
    pub correlation_id: CorrelationId,
    pub tenant_id: TenantId,
    pub model_name: String,
    pub vector_count: u32,
    pub duration_ms: u64,
}

// CID:embedding-events-003 - EmbeddingFailed
// Purpose: The "an embed failed" audit fact.
// Published when any of the 4 typed
// `EmbeddingErrorV1` variants is returned.
// The `error` field is the `Display`-formatted
// error (e.g. "adapter unavailable: ollama not
// running"). The `duration_ms` is the time
// spent before the error (useful for spotting
// a slow-failing backend).
// Uses: CorrelationId, TenantId.
// Used by: every failing `EmbeddingV1` method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingFailed {
    pub correlation_id: CorrelationId,
    pub tenant_id: TenantId,
    pub model_name: String,
    pub error: String,
    pub duration_ms: u64,
}
