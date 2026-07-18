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
use serde_json::Value;
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
    let kernel = Arc::new(kernel);

    // First seal the dashboard-token so the kernel
    // transitions from PreBootstrap to Full mode.
    // (Drift #16 / GAP-007: a fresh kernel boots
    // in PreBootstrap and returns 503 on /health.)
    let seal_app = router(Arc::clone(&kernel));
    let seal_resp = seal_app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/pre-bootstrap/seal")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::to_vec(&serde_json::json!({
                        "name": "dashboard-token",
                        "value": "test-token",
                    }))
                    .expect("json"),
                ))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(seal_resp.status(), StatusCode::OK);

    // Now /health should return 200 with a HealthReport.
    // Build a fresh router over the same (now sealed) kernel.
    let health_app = router(kernel);
    let response = health_app
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

#[tokio::test]
async fn health_503_on_pre_bootstrap() {
    let (_dir, kernel) = fresh_kernel().await;

    // CID:dashboard-transport-002 - health_503_on_pre_bootstrap
    // Purpose: A fresh kernel (no `dashboard-token`
    // sealed) boots in `PreBootstrap` mode. The
    // `/health` endpoint must return `503 SERVICE_UNAVAILABLE`
    // with `"pre_bootstrap": true` in the JSON body,
    // so load balancers can detect a non-sealed kernel.
    assert!(
        kernel.mode().is_pre_bootstrap(),
        "fresh kernel must boot in PreBootstrap"
    );

    let response = router(Arc::new(kernel))
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("body");
    let json: Value = serde_json::from_slice(&body).expect("JSON body");
    assert_eq!(
        json.get("pre_bootstrap"),
        Some(&Value::Bool(true)),
        "response must have pre_bootstrap: true"
    );
    assert!(
        json.get("overall").is_some(),
        "response must have overall health status"
    );
}

#[allow(dead_code)]
fn _contract_anchors() {
    let _ = (Actor::Timer, TenantId::new("test"), Probe);
}
