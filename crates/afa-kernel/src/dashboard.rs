//! Code Map: Dashboard Transport
//!
//! - `router`: Builds the read-only Phase 3 HTTP + WebSocket
//!   surface. `/health` is open; span queries require bearer auth.
//! - `DashboardState`: Shared kernel state passed to handlers.
//!
//! Story (plain English): the hospital's front desk. The public
//! health board is visible to everyone; patient charts require a
//! nurse's badge; the live monitor receives new chart notes as they
//! are filed.
//!
//! CID Index:
//! CID:dashboard-001 -> router
//! CID:dashboard-002 -> DashboardState
//!
//! Quick lookup: rg -n "CID:dashboard-" crates/afa-kernel/src/dashboard.rs

// **Doc drift correction #8 vs. the IMPL draft**:
// - Source: ~/Shared/agents/docs/features/observability-baseline/IMPL-observability-baseline.md
//   (Phase 3 backend tasks — dependencies table)
// - Shipped: axum 0.8 (the Cargo.toml pins axum = "0.8")
// - Why: The IMPL draft listed axum = "0.7.9". The shipped
//   code uses 0.8 because the WebSocket upgrade handshake in
//   axum 0.8 requires the `any()` route method, which does
//   not exist in the 0.7.x series. The features `ws`, `json`,
//   and `http1` are the minimal set needed for the Phase 3
//   Dashboard Transport surface.
//
// **Doc drift correction #10 vs. the IMPL draft**:
// - Source: ~/Shared/agents/docs/features/observability-baseline/IMPL-observability-baseline.md
//   (Phase 3 — KernelConfig)
// - Shipped: Kernel::new(master_key, secrets_db_path) preserved.
// - Why: The IMPL draft proposed a `KernelConfig` struct parameter
//   for `Kernel::new`. The shipped code keeps the two-arg
//   `(master_key, secrets_db_path)` signature for backward
//   compatibility with existing tests and callers. `KernelConfig`
//   is deferred to a future pack.
//
// **Doc drift correction #11 vs. the IMPL draft**:
// - Source: ~/Shared/agents/docs/features/observability-baseline/IMPL-observability-baseline.md
//   (Phase 3 — /spans/stream route method)
// - Shipped: `.route("/stream", any(spans::stream))` (see line 45 below).
// - Why: The IMPL draft said `.route("/stream", get(stream_handler))`.
//   axum 0.8 requires `any()` for WebSocket upgrades — the internal
//   upgrade handshake is not a plain GET response, so `get()` would
//   reject the connection. `any()` accepts the GET + upgrade without
//   requiring the handler to be a GET-only endpoint.

mod auth;
mod health;
mod spans;

use crate::Kernel;
use axum::routing::{any, get};
use axum::Router;
use std::sync::Arc;

// CID:dashboard-002 - DashboardState
// Purpose: Shared state for all dashboard handlers.
// Used by: router and the health/span handlers.
#[derive(Clone)]
pub(crate) struct DashboardState {
    pub(crate) kernel: Arc<Kernel>,
}

// CID:dashboard-001 - router
// Purpose: Build the Phase 3 Dashboard Transport router.
// `/health` is intentionally outside the bearer middleware;
// every `/spans/*` route is protected before its handler runs.
// Used by: kernel bootstrap and integration tests.
pub fn router(kernel: Arc<Kernel>) -> Router {
    let state = DashboardState { kernel };
    let protected = Router::new()
        .route("/{correlation_id}", get(spans::by_correlation_id))
        .route("/recent", get(spans::recent))
        .route("/stream", any(spans::stream))
        .layer(axum::middleware::from_fn_with_state(
            state.clone(),
            auth::bearer_auth,
        ))
        .with_state(state.clone());

    Router::new()
        .route("/health", get(health::handler))
        .nest("/spans", protected)
        .with_state(state)
}
