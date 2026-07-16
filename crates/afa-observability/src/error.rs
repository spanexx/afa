//! Code Map: observability::error
//!
//! - ObservabilityError: the local four-variant enum
//!   used inside the crate. Maps to the contract
//!   surface ObservabilityErrorV1 (afa-contracts) via
//!   the From impl below.
//! - impl AfaError for ObservabilityError: the six
//!   bucket-kinds mapping (storage-class failures all
//!   collapse to Unavailable; bugs + invariant
//!   violations collapse to Internal). Mirrors the
//!   StorageError::kind impl in afa-contracts for the
//!   same reason.
//! - impl From`ObservabilityError` for
//!   ObservabilityErrorV1: the contract-boundary
//!   conversion. The crate's public callers see the
//!   ObservabilityErrorV1 vocabulary; this impl is
//!   the bridge.
//! - from_storage_open: the helper that wraps a
//!   afa_contracts::StorageError::Open into the
//!   ObservabilityError::StorageUnreachable variant.
//!   The "Open" semantic is "the file is not there and
//!   can't be created" -- the operator may be running
//!   a fresh deploy, so this is its own failure mode.
//! - from_rusqlite: the helper that wraps a bare
//!   rusqlite::Error into the StorageOp variant. Used
//!   at the with_conn boundaries where a closure
//!   returns rusqlite::Error directly.
//!
//! Story (plain English): The recording nurse has
//! three different ways her pen can fail: she can't
//! find the logbook (file unwritable -- the
//! StorageUnreachable case, which usually means
//! "wrong path, check the config"); she opens the
//! logbook and the index is missing or looks wrong
//! (the StorageCorrupted case, which usually means
//! "the file was edited by hand or a newer version of
//! the engine wrote to it -- investigate"); or the pen
//! itself misbehaves mid-write (the StorageOp case,
//! which the SQL layer surfaces as a generic
//! rusqlite::Error). All three collapse to the
//! AfaErrorKind::Unavailable bucket at the contract
//! layer because the doctor's response in every case
//! is "the operator needs to look at the logbook",
//! not "the client should retry".
//!
//! CID Index:
//! CID:afa-observability-error-001 -> ObservabilityError
//! CID:afa-observability-error-002 -> impl AfaError
//! CID:afa-observability-error-003 -> From`ObservabilityError`
//!
//! Quick lookup: rg -n "CID:afa-observability-error-" crates/afa-observability/src/error.rs

use afa_contracts::{AfaErrorKind, ObservabilityErrorV1};

// CID:afa-observability-error-001 - ObservabilityError
// Purpose: The internal error vocabulary for the
// afa-observability crate. Maps to
// ObservabilityErrorV1 (afa-contracts) for the
// public surface. The four variants cover every
// failure mode the engine can hit at the I/O
// boundary: file unwritable, file corrupted, file
// I/O failed (the underlying rusqlite::Error
// wrapped), or an invariant violation (a bug).
//
// **Doc drift correction vs. the IMPL draft**:
// the IMPL listed three variants (Open / Migrate /
// Internal). Phase 1's implementation adds a fourth
// `StorageCorrupted` variant for "the file is there
// but its contents do not look right" -- needed to
// distinguish a fresh-deploy failure (StorageUnreachable)
// from a corrupted-file failure (StorageCorrupted) at
// the contract boundary; otherwise the dashboard's
// /spans/storage diagnostic endpoint would surface
// the same string for both conditions.
//
// **Variant count vs. afa-contracts::StorageError**:
// the four-variant here is a strict refinement of the
// four-variant StorageError (Open / Migrate / Locked /
// Closure); the Local mapping preserves the same
// "StorageError::Closure" -> "StorageOp" semantic
// because the storage crate already maps every
// rusqlite::Error into StorageError::Migrate
// { version: 0, source } before it surfaces, so
// from_storage_open is the only translation needed
// at the engine boundary.
//
// Used by: observability.rs (the engine constructs
// this on every persistence / bus-publish failure),
// persistence.rs (the read/write/delete helpers
// return this).
#[derive(Debug, thiserror::Error)]
pub enum ObservabilityError {
    #[error("spans storage is unreachable: {reason}")]
    StorageUnreachable { reason: String },
    #[error("spans storage is corrupted")]
    StorageCorrupted,
    #[error("spans storage operation failed: {source}")]
    StorageOp {
        #[source]
        source: rusqlite::Error,
    },
    #[error("observability engine internal error: {reason}")]
    Internal { reason: String },
}

impl ObservabilityError {
    // CID:afa-observability-error-004 - from_storage_open
    // Purpose: Translate afa_contracts::StorageError
    // -> ObservabilityError. The "Open" case is the
    // only StorageError variant the engine treats
    // specially (every other variant folds to
    // StorageOp via the Internal stub).
    //
    // Used by: persistence.rs (the write_span / read /
    // delete helpers call this at their with_conn
    // .map_err sites).
    pub fn from_storage_open(e: afa_contracts::StorageError) -> Self {
        Self::StorageUnreachable {
            reason: e.to_string(),
        }
    }

    // CID:afa-observability-error-005 - from_rusqlite
    // Purpose: Translate a bare rusqlite::Error (the
    // common-within-closure return type) into the
    // local StorageOp variant. Used inside the
    // with_conn closures where the closure signature
    // is Result<T, rusqlite::Error>.
    pub fn from_rusqlite(e: rusqlite::Error) -> Self {
        Self::StorageOp { source: e }
    }
}

// CID:afa-observability-error-003 - From`ObservabilityError` for ObservabilityErrorV1
// Purpose: The contract-boundary conversion. Every
// crate-public function that returns
// ObservabilityError gets a ?-convertible path to
// ObservabilityErrorV1 (the dictionary type).
//
// The mapping is deliberately lossy on StorageOp (it
// collapses to StorageUnreachable with a
// reason=e.to_string() so the wire form carries the
// underlying rusqlite::Error message). The lossy
// collapse is fine because the four-bucket public
// vocabulary (StorageUnreachable / StorageCorrupted /
// SchemaVersionMismatch / Internal) is exactly the
// operator-action vocabulary -- the operator does
// not need to distinguish "StorageOp from a busy
// table" from "StorageOp from a disk full": both
// are "check the spans DB file".
impl From<ObservabilityError> for ObservabilityErrorV1 {
    fn from(e: ObservabilityError) -> Self {
        match &e {
            ObservabilityError::StorageUnreachable { reason } => {
                ObservabilityErrorV1::StorageUnreachable {
                    reason: reason.clone(),
                }
            }
            ObservabilityError::StorageCorrupted => ObservabilityErrorV1::StorageCorrupted,
            ObservabilityError::StorageOp { .. } => ObservabilityErrorV1::StorageUnreachable {
                reason: e.to_string(),
            },
            ObservabilityError::Internal { reason } => ObservabilityErrorV1::Internal {
                reason: reason.clone(),
            },
        }
    }
}

// CID:afa-observability-error-002 - impl AfaError for ObservabilityError
// Purpose: Map the four-variant local enum to the
// six-bucket AfaErrorKind vocabulary. Storage-class
// failures (the first three variants) all collapse to
// AfaErrorKind::Unavailable because the fix is
// operator action (restore the file, kill the
// competing process), not a client retry. Bugs and
// invariant violations (the fourth variant) collapse
// to AfaErrorKind::Internal because the fix is
// "file an issue against the kernel".
//
// **Reason for the StorageCorrupted -> Unavailable
// mapping**: a corrupted spans DB file usually means
// "the file was edited by hand" or "a newer engine
// wrote a forward-compatible schema we can't read";
// both are operator-action -- the client should NOT
// retry, they should escalate.
//
// Used by: every caller that needs to branch on "is
// this error retryable?" without knowing the concrete
// type (the kernel's dispatch adapter, the
// `/spans/recent` HTTP handler).
impl afa_contracts::AfaError for ObservabilityError {
    fn kind(&self) -> AfaErrorKind {
        match self {
            ObservabilityError::StorageUnreachable { .. }
            | ObservabilityError::StorageCorrupted
            | ObservabilityError::StorageOp { .. } => AfaErrorKind::Unavailable,
            ObservabilityError::Internal { .. } => AfaErrorKind::Internal,
        }
    }
}
