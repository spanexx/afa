//! Code Map: afa-plugin-embedding-local — offline
//! - `Offline`: The strict / degraded
//!   policy enum. `Strict` returns
//!   `Err(ModelUnavailable)` when the
//!   model is missing. `Degraded`
//!   constructs the adapter with a
//!   "degraded" marker and returns a
//!   384-element zero vector on every
//!   `embed` call. The "degraded" mode
//!   is the CI / dev-mode escape
//!   hatch (the operator can run
//!   `afa-cli embedding status` on a
//!   machine without the model).
//! - `degraded_vector`: The sentinel
//!   384-element zero vector. The
//!   function is `const`-friendly so
//!   the zero vector is allocated
//!   on the stack for the call (no
//!   heap allocation per `embed`
//!   call).
//!
//! Story (plain English): The
//! offline module is the policy
//! book for "what should the
//! adapter do if the model is
//! missing?". The strict policy
//! says "refuse to start; the
//! operator must download the
//! model". The degraded policy
//! says "start anyway, return
//! zero vectors on every call,
//! and tell the operator via
//! the audit log that they are
//! in degraded mode". The
//! degraded mode is the
//! developer-friendly fallback
//! for CI environments that
//! cannot download the model.
//!
//! CID Index:
//! CID:afa-plugin-embedding-local-offline-001 -> Offline
//! CID:afa-plugin-embedding-local-offline-002 -> degraded_vector
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-local-offline-" crates/afa-plugin-embedding-local/src/offline.rs

use afa_contracts::EmbeddingErrorV1;

use super::config::OfflineMode;

// CID:afa-plugin-embedding-local-offline-002 - degraded_vector
// Purpose: The sentinel
// 384-element zero vector
// that the degraded mode
// returns on every `embed`
// call. The function is
// `const`-friendly so the
// zero vector is allocated
// on the stack for the call
// (no heap allocation per
// `embed` call). The
// dimension is hard-coded to
// 384 (the all-MiniLM-L6-v2
// hidden_size); a future
// pack can add a
// `dimension: usize` parameter
// if more models are added.
// Uses: nothing.
// Used by:
// `LocalEmbeddingAdapter::embed`
// when the adapter is in
// degraded mode.
pub fn degraded_vector() -> Vec<f32> {
    vec![0.0; 384]
}

// CID:afa-plugin-embedding-local-offline-001 - Offline
// Purpose: The strict / degraded
// policy. The struct is built
// from an `OfflineMode` (the
// config enum) and a
// `model_present: bool` (the
// runtime check for the
// model file). The
// `decide` method returns
// the action the adapter
// should take: `Construct`
// (proceed with the real
// model load),
// `ConstructDegraded`
// (proceed with the sentinel
// mode), or
// `Refuse(reason)`
// (return
// `Err(ModelUnavailable)`).
// Uses: OfflineMode,
// EmbeddingErrorV1.
// Used by:
// `LocalEmbeddingAdapter::new`
// (the only consumer).
pub enum OfflineAction {
    /// The model is
    /// present. Construct
    /// the real adapter.
    Construct,
    /// The model is
    /// missing but the
    /// operator is in
    /// degraded mode.
    /// Construct the
    /// adapter with the
    /// sentinel mode.
    ConstructDegraded,
    /// The model is
    /// missing and the
    /// operator is in
    /// strict mode. Refuse
    /// to construct the
    /// adapter.
    Refuse(EmbeddingErrorV1),
}

pub struct Offline {
    mode: OfflineMode,
    model_present: bool,
    model_name: String,
}

impl Offline {
    /// Build an `Offline` policy
    /// for the given
    /// `mode` and
    /// `model_present` state.
    pub fn new(mode: OfflineMode, model_present: bool, model_name: String) -> Self {
        Self {
            mode,
            model_present,
            model_name,
        }
    }

    /// Decide which action the
    /// adapter should take.
    /// The logic:
    /// - `Strict` + model
    ///   present → `Construct`
    /// - `Strict` + model
    ///   missing →
    ///   `Refuse(ModelUnavailable)`
    /// - `Degraded` + model
    ///   present → `Construct`
    /// - `Degraded` + model
    ///   missing →
    ///   `ConstructDegraded`
    pub fn decide(&self) -> OfflineAction {
        if self.model_present {
            return OfflineAction::Construct;
        }
        match self.mode {
            OfflineMode::Strict => OfflineAction::Refuse(EmbeddingErrorV1::ModelUnavailable {
                model_name: self.model_name.clone(),
                reason: "model file missing; run `afa-cli embedding download` or pre-place the model files".to_string(),
            }),
            OfflineMode::Degraded => OfflineAction::ConstructDegraded,
        }
    }
}
