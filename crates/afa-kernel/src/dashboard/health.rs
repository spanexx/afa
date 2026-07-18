//! Code Map: Dashboard health endpoint
//!
//! - `handler`: Returns the kernel's current health report.
//!
//! Story (plain English): the public status board at the ward
//! entrance. It never asks for a badge; it shows whether the
//! whole ward is healthy enough to receive traffic.
//!
//! CID Index:
//! CID:dashboard-health-001 -> handler
//!
//! Quick lookup: rg -n "CID:dashboard-health-" crates/afa-kernel/src/dashboard/health.rs
//!
//! Doc drift corrections vs. the IMPL draft:
//! - **#8**: axum version — this crate uses axum 0.8 (not 0.7.9
//!   as the IMPL draft specified). The `axum::http::StatusCode`
//!   import is the same between versions, but the router layer
//!   (dashboard.rs) that invokes this handler uses axum 0.8's
//!   `any()` / `get()` routing API. See dashboard.rs for the
//!   full correction.
//! - **#10**: KernelConfig deferral — the handler accesses the
//!   kernel through `DashboardState::kernel` (an `Arc<Kernel>`),
//!   not through a `KernelConfig` struct as the IMPL draft
//!   proposed. The two-arg `Kernel::new` signature is preserved.

use super::DashboardState;
use afa_contracts::{HealthReport, HealthStatus};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use serde_json::json;

// CID:dashboard-health-001 - handler
// Purpose: Return `HealthReport` as JSON. Healthy and Degraded
// reports use 200; Unhealthy reports use 503 so a load balancer
// can react without parsing the body.
// Drift #16 (GAP-007): when the kernel is in PreBootstrap mode,
// the response is 503 with `"pre_bootstrap": true` in the body
// so load balancers can detect a non-sealed kernel.
// Used by: GET /health.
pub(crate) async fn handler(State(state): State<DashboardState>) -> impl IntoResponse {
    let kernel = &state.kernel;

    // Drift #16 (GAP-007): return 503 when the kernel is in
    // PreBootstrap mode (dashboard-token has not been sealed yet).
    if kernel.mode().is_pre_bootstrap() {
        let report = kernel.aggregate_health();
        let body = json!({
            "pre_bootstrap": true,
            "overall": report.overall,
            "engines": report.engines,
            "checked_at": report.checked_at,
        });
        return (StatusCode::SERVICE_UNAVAILABLE, Json(body));
    }

    let report: HealthReport = kernel.aggregate_health();
    let status = if matches!(report.overall, HealthStatus::Unhealthy { .. }) {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (status, Json(json!(report)))
}
