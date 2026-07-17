//! Code Map: Phase 3 Dashboard Transport conformance
//!
//! - `health_open_no_bearer`: `/health` is open and returns a
//!   serialized `HealthReport` without an Authorization header.
//!
//! Story (plain English): the hospital's public front-door
//! status board. Anyone may ask whether the ward is operating;
//! private patient charts remain behind the nurse's badge check.
//!
//! CID Index:
//! CID:dashboard-transport-001 -> health_open_no_bearer
//!
//! Quick lookup: rg -n "CID:dashboard-transport-" crates/afa-kernel/tests/dashboard_transport.rs

use afa_contracts::{Actor, AfaEvent, HealthReport, TenantId};
use afa_kernel::dashboard::router;
use afa_kernel::Kernel;
use afa_security::MasterKey;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Probe;

impl AfaEvent for Probe {}

async fn fresh_kernel() -> (TempDir, Kernel) {
    let dir = tempfile::tempdir().expect("tempdir");
    let kernel = Kernel::new(&MasterKey::from([0x42; 32]), dir.path().join("secrets.db"))
        .await
        .expect("kernel::new");
    (dir, kernel)
}

#[tokio::test]
async fn health_open_no_bearer() {
    let (_dir, kernel) = fresh_kernel().await;
    let response = router(Arc::new(kernel))
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::OK);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let report: HealthReport = serde_json::from_slice(&body).expect("HealthReport JSON");
    assert!(matches!(
        report.overall,
        afa_contracts::HealthStatus::Healthy
    ));
}

#[allow(dead_code)]
fn _contract_anchors() {
    let _ = (Actor::Timer, TenantId::new("test"), Probe);
}
