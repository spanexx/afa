//! Code Map: `POST /pre-bootstrap/seal` handler
//!
//! - `handler`: The day-0 endpoint that moves the
//!   kernel from `PreBootstrap` to `Full`. Mounted
//!   at `POST /pre-bootstrap/seal` with **no
//!   bearer middleware** (the handler itself
//!   enforces the mode check). On a 200, the
//!   kernel transitions to `Full` and the
//!   response body is `{sealed_at, version}`. On
//!   a 409, the kernel was already in `Sealing`
//!   (concurrent first request) or `Full`
//!   (operator already sealed; the day-0 endpoint
//!   is closed). On a 400, the `name` field is
//!   not `"dashboard-token"`.
//! - `PreBootstrapSealRequest` / `PreBootstrapSealResponse`:
//!   The wire shapes (per the IMPL draft).
//! - `PreBootstrapError`: The error enum mapped
//!   to HTTP status by the handler.
//!
//! Story (plain English): Imagine a hotel with a
//! keycard lock. The first time the hotel opens,
//! the lock is set to "accept the manager's first
//! key." The manager inserts the master key
//! (this handler) and the lock arms itself. The
//! second manager to walk in gets rejected (409) —
//! the day-0 setup window has closed.
//!
//! CID Index:
//! CID:dashboard-pre-bootstrap-001 -> PreBootstrapSealRequest
//! CID:dashboard-pre-bootstrap-002 -> PreBootstrapSealResponse
//! CID:dashboard-pre-bootstrap-003 -> PreBootstrapError
//! CID:dashboard-pre-bootstrap-004 -> handler
//!
//! Quick lookup: rg -n "CID:dashboard-pre-bootstrap-" crates/afa-kernel/src/dashboard/pre_bootstrap.rs

use super::DashboardState;
use crate::mode::TryTransitionError;
use afa_contracts::SecurityErrorV1;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::{Deserialize, Serialize};
use std::time::SystemTime;
use uuid::Uuid;

// CID:dashboard-pre-bootstrap-001 - PreBootstrapSealRequest
// Purpose: The `POST /pre-bootstrap/seal` request
// body. The v1 handler accepts `name =
// "dashboard-token"` and any non-empty `value`
// (the operator-generated day-0 token).
// Used by: the SPA Setup Wizard and `afa-cli`.
#[derive(Debug, Deserialize)]
pub(crate) struct PreBootstrapSealRequest {
    pub name: String,
    pub value: String,
}

// CID:dashboard-pre-bootstrap-002 - PreBootstrapSealResponse
// Purpose: The success response body. `sealed_at`
// is the wall-clock `SystemTime` the kernel
// transitioned to `Full` (in RFC 3339 format);
// `version` is the engine's version number
// (always 1 for a fresh seal — the engine
// increments on `rotate`).
// Used by: the SPA Setup Wizard and `afa-cli`.
#[derive(Debug, Serialize)]
pub(crate) struct PreBootstrapSealResponse {
    pub sealed_at: String,
    pub version: u32,
}

// CID:dashboard-pre-bootstrap-003 - PreBootstrapError
// Purpose: The error enum for the handler. The
// variants map 1:1 to HTTP status codes:
// `BadName` → 400, `NotInMode` / `AlreadySealing`
// / `AlreadySealed` → 409, `Engine` → 500.
// Used by: the handler's `IntoResponse` impl.
#[derive(Debug)]
#[allow(dead_code)] // Engine field is set on the error path; not yet read by any handler
pub(crate) enum PreBootstrapError {
    BadName,
    NotInMode,
    AlreadySealing,
    AlreadySealed,
    Engine(SecurityErrorV1),
}

impl From<TryTransitionError> for PreBootstrapError {
    fn from(e: TryTransitionError) -> Self {
        match e {
            TryTransitionError::NotInMode => PreBootstrapError::NotInMode,
            TryTransitionError::AlreadySealing => PreBootstrapError::AlreadySealing,
            TryTransitionError::AlreadySealed => PreBootstrapError::AlreadySealed,
        }
    }
}

impl IntoResponse for PreBootstrapError {
    fn into_response(self) -> Response {
        let (status, body) = match &self {
            PreBootstrapError::BadName => (
                StatusCode::BAD_REQUEST,
                serde_json::json!({"error": "name must be 'dashboard-token'"}),
            ),
            PreBootstrapError::NotInMode => (
                StatusCode::CONFLICT,
                serde_json::json!({"error": "kernel is not in PreBootstrap"}),
            ),
            PreBootstrapError::AlreadySealing => (
                StatusCode::CONFLICT,
                serde_json::json!({"error": "another seal is in flight"}),
            ),
            PreBootstrapError::AlreadySealed => (
                StatusCode::CONFLICT,
                serde_json::json!({"error": "dashboard-token already sealed"}),
            ),
            PreBootstrapError::Engine(_) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::json!({"error": "engine error"}),
            ),
        };
        (status, Json(body)).into_response()
    }
}

// CID:dashboard-pre-bootstrap-004 - handler
// Purpose: The `POST /pre-bootstrap/seal` handler.
// Steps:
// 1. Validate the `name` (must be
//    `"dashboard-token"`) and the `value`
//    (non-empty).
// 2. Try the `PreBootstrap → Sealing` CAS via
//    `ModeController::try_transition_to_sealing`.
//    On conflict, return 409.
// 3. Call `security.seal(value, name)`. On
//    engine error, ROLL BACK to `PreBootstrap`
//    so the operator can retry, then return
//    500.
// 4. On success, transition to `Full` and
//    return 200 with `{sealed_at, version}`.
// Used by: the SPA Setup Wizard and `afa-cli`.
pub(crate) async fn handler(
    State(state): State<DashboardState>,
    Json(req): Json<PreBootstrapSealRequest>,
) -> Result<Response, PreBootstrapError> {
    // 1. Validate the request.
    if req.name != "dashboard-token" || req.value.is_empty() {
        return Err(PreBootstrapError::BadName);
    }

    // 2. CAS into `Sealing`. The CAS guarantees
    // only one concurrent request gets past
    // this point; the second receives
    // `AlreadySealing`.
    let request_id = Uuid::new_v4();
    state
        .kernel
        .mode_controller()
        .try_transition_to_sealing(request_id)?;

    // 3. Call the security engine. The seal
    // is the operator's day-0 setup, not a
    // tenant-driven action, so we use the
    // "system" tenant + "timer" actor.
    let result = state
        .kernel
        .security()
        .seal(req.value.as_bytes(), &req.name)
        .await;

    match result {
        Ok(secret_ref) => {
            let sealed_at = SystemTime::now();
            state.kernel.mode_controller().transition_to_full(sealed_at);
            let body = PreBootstrapSealResponse {
                sealed_at: chrono::DateTime::<chrono::Utc>::from(sealed_at)
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
                version: secret_ref.version,
            };
            Ok((StatusCode::OK, Json(body)).into_response())
        }
        Err(e) => {
            // ROLL BACK. The CAS won, so the
            // kernel is in `Sealing`. Without the
            // rollback, the operator would be
            // locked out (no second request can
            // win the CAS — `Sealing` is not
            // `PreBootstrap`).
            state.kernel.mode_controller().transition_to_prebootstrap();
            Err(PreBootstrapError::Engine(e))
        }
    }
}
