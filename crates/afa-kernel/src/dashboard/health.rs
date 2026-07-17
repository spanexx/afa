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

// CID:dashboard-health-001 - handler
// Purpose: Return `HealthReport` as JSON. Healthy and Degraded
// reports use 200; Unhealthy reports use 503 so a load balancer
// can react without parsing the body.
// Used by: GET /health.
pub(crate) async fn handler(State(state): State<DashboardState>) -> impl IntoResponse {
    let report: HealthReport = state.kernel.aggregate_health();
    let status = if matches!(report.overall, HealthStatus::Unhealthy { .. }) {
        StatusCode::SERVICE_UNAVAILABLE
    } else {
        StatusCode::OK
    };
    (status, Json(report))
}
