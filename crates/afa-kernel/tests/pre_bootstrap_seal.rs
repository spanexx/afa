//! Code Map: Pre-Bootstrap seal end-to-end conformance
//!
//! - `pre_bootstrap_seal_transitions_to_full`: a fresh
//!   `Kernel` boots in `PreBootstrap`; a `POST
//!   /pre-bootstrap/seal` request with `name =
//!   "dashboard-token"` and a chosen value transitions
//!   the kernel to `Full`; a second request on the same
//!   kernel returns `409 Conflict`.
//! - `pre_bootstrap_seal_rejects_non_dashboard_token_name`:
//!   the v1 handler refuses any `name` other than
//!   `"dashboard-token"` (the IMPL's name-allowlist).
//!
//! Story (plain English): the kernel starts locked; the
//! operator (or the SPA Setup Wizard) uses
//! `POST /pre-bootstrap/seal` to drop the first key into
//! the vault. The handler is the only door that opens
//! during day-0 setup; every other endpoint stays closed
//! until the seal succeeds.
//!
//! CID Index:
//! CID:pre-bootstrap-001 -> pre_bootstrap_seal_transitions_to_full
//! CID:pre-bootstrap-002 -> pre_bootstrap_seal_rejects_non_dashboard_token_name
//!
//! Quick lookup: rg -n "CID:pre-bootstrap-" crates/afa-kernel/tests/pre_bootstrap_seal.rs

use afa_contracts::KernelMode;
use afa_kernel::Kernel;
use afa_security::MasterKey;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use serde_json::json;
use std::sync::Arc;
use tempfile::TempDir;
use tokio::sync::oneshot;
use tower::ServiceExt;

async fn fresh_kernel() -> (TempDir, Kernel) {
    let dir = tempfile::tempdir().expect("tempdir");
    let kernel = Kernel::new(
        &MasterKey::from([0x42u8; 32]),
        dir.path().join("secrets.db"),
    )
    .await
    .expect("kernel::new");
    (dir, kernel)
}

fn seal_request_body(name: &str, value: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({ "name": name, "value": value })).expect("json")
}

#[tokio::test]
async fn pre_bootstrap_seal_transitions_to_full() {
    let (_dir, kernel) = fresh_kernel().await;

    // CID:pre-bootstrap-001 - pre_bootstrap_seal_transitions_to_full
    // Purpose: A fresh kernel boots in `PreBootstrap`;
    // a `POST /pre-bootstrap/seal` with the canonical
    // name and a chosen value transitions the
    // kernel to `Full`. A second request on the
    // same kernel returns `409 Conflict` because
    // the day-0 endpoint closes once the operator
    // has sealed the first secret.
    assert!(
        kernel.mode().is_pre_bootstrap(),
        "fresh kernel must boot in PreBootstrap; got {:?}",
        kernel.mode()
    );

    let app = afa_kernel::dashboard::router(std::sync::Arc::new(kernel.clone()));
    let first = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/pre-bootstrap/seal")
                .header("content-type", "application/json")
                .body(Body::from(seal_request_body(
                    "dashboard-token",
                    "super-secret-day-0-token",
                )))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(first.status(), StatusCode::OK);
    let body = first.into_body().collect().await.unwrap().to_bytes();
    let parsed: serde_json::Value = serde_json::from_slice(&body).expect("json");
    assert!(parsed.get("sealed_at").is_some());
    assert!(parsed.get("version").is_some());

    // The mode transition is observable on every
    // kernel clone (the ModeController is Arc-shared).
    assert!(
        kernel.mode().is_sealed(),
        "kernel must transition to Full after a successful seal; got {:?}",
        kernel.mode()
    );

    // Second request must be rejected (the day-0
    // endpoint is closed once `dashboard-token`
    // is sealed).
    let second = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/pre-bootstrap/seal")
                .header("content-type", "application/json")
                .body(Body::from(seal_request_body(
                    "dashboard-token",
                    "second-attempt",
                )))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(second.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn pre_bootstrap_seal_rejects_non_dashboard_token_name() {
    let (_dir, kernel) = fresh_kernel().await;
    let app = afa_kernel::dashboard::router(std::sync::Arc::new(kernel));

    // CID:pre-bootstrap-002 - pre_bootstrap_seal_rejects_non_dashboard_token_name
    // Purpose: The v1 handler is hard-coded to accept
    // `name = "dashboard-token"` only. Any other
    // name returns `400 Bad Request` and the kernel
    // stays in `PreBootstrap` (no state change).
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/pre-bootstrap/seal")
                .header("content-type", "application/json")
                .body(Body::from(seal_request_body("llm-api-key", "x")))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pre_bootstrap_boots_in_pre_bootstrap_for_fresh_secrets_db() {
    let (_dir, kernel) = fresh_kernel().await;

    // CID:pre-bootstrap-003 - pre_bootstrap_boots_in_pre_bootstrap_for_fresh_secrets_db
    // Purpose: A kernel booted against a fresh
    // (empty) `secrets.db` must be in
    // `PreBootstrap`. The day-0 check
    // (`security.lookup_hash("dashboard-token", ...)`)
    // returns `Err(SecretNotFound)`, so the
    // `ModeController` is built via
    // `new_pre_bootstrap()`. This test pins
    // the day-0 detection logic so a future
    // change to the engine's error response
    // does not silently re-classify a fresh
    // boot as `Full`.
    assert!(matches!(kernel.mode(), KernelMode::PreBootstrap { .. }));
}

#[tokio::test]
async fn pre_bootstrap_seal_malformed_body_400() {
    let (_dir, kernel) = fresh_kernel().await;
    let app = afa_kernel::dashboard::router(std::sync::Arc::new(kernel));

    // CID:pre-bootstrap-004 - pre_bootstrap_seal_malformed_body_400
    // Purpose: A malformed JSON body (e.g. invalid JSON)
    // must return `400 Bad Request` without triggering
    // any mode transition.
    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/pre-bootstrap/seal")
                .header("content-type", "application/json")
                .body(Body::from("not-json"))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn pre_bootstrap_seal_concurrent_returns_409() {
    let (_dir, kernel) = fresh_kernel().await;
    let kernel = Arc::new(kernel);
    let app1 = afa_kernel::dashboard::router(Arc::clone(&kernel));
    let app2 = afa_kernel::dashboard::router(Arc::clone(&kernel));

    // CID:pre-bootstrap-005 - pre_bootstrap_seal_concurrent_returns_409
    // Purpose: Two concurrent `POST /pre-bootstrap/seal`
    // requests on a fresh kernel. Exactly one must
    // return `200` and the other `409 Conflict`.
    let body = serde_json::to_vec(&json!({
        "name": "dashboard-token",
        "value": "my-secret-token",
    }))
    .expect("json");
    let body1 = body.clone();
    let body2 = body;

    let (tx1, rx1) = oneshot::channel();
    let (tx2, rx2) = oneshot::channel();

    let j1 = tokio::spawn(async move {
        let resp = app1
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/pre-bootstrap/seal")
                    .header("content-type", "application/json")
                    .body(Body::from(body1))
                    .expect("request"),
            )
            .await
            .expect("response");
        let _ = tx1.send(resp.status());
    });

    let j2 = tokio::spawn(async move {
        let resp = app2
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/pre-bootstrap/seal")
                    .header("content-type", "application/json")
                    .body(Body::from(body2))
                    .expect("request"),
            )
            .await
            .expect("response");
        let _ = tx2.send(resp.status());
    });

    let (s1, s2) = tokio::join!(j1, j2);
    s1.expect("join 1");
    s2.expect("join 2");

    let status1 = rx1.await.expect("rx1");
    let status2 = rx2.await.expect("rx2");

    // Exactly one 200 and one 409
    let two_hundred = [status1, status2]
        .iter()
        .filter(|&&s| s == StatusCode::OK)
        .count();
    let four_nine = [status1, status2]
        .iter()
        .filter(|&&s| s == StatusCode::CONFLICT)
        .count();
    assert_eq!(two_hundred, 1, "exactly one request must succeed");
    assert_eq!(four_nine, 1, "exactly one request must be 409");
}

#[tokio::test]
async fn pre_bootstrap_seal_storage_failure_500() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let (dir, kernel) = fresh_kernel().await;
    let app = afa_kernel::dashboard::router(std::sync::Arc::new(kernel));

    // CID:pre-bootstrap-006 - pre_bootstrap_seal_storage_failure_500
    // Purpose: When the secrets DB directory is made
    // non-writable after the kernel boots, the seal
    // handler should return `500 Internal Server Error`.
    // (chmod on the directory prevents SQLite from
    // creating journal files; the open connection
    // cannot complete a write transaction.)
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o000))
        .expect("chmod 000 on secrets.db parent dir");

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/pre-bootstrap/seal")
                .header("content-type", "application/json")
                .body(Body::from(seal_request_body("dashboard-token", "my-token")))
                .expect("request"),
        )
        .await
        .expect("response");
    // The storage layer should return an error that
    // maps to 500.
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR,);

    // Restore permissions so the TempDir can clean up.
    fs::set_permissions(dir.path(), fs::Permissions::from_mode(0o755)).ok();
}

#[tokio::test]
async fn bearer_auth_hash_only_path() {
    let (_dir, kernel) = fresh_kernel().await;
    let kernel = Arc::new(kernel);
    let app = afa_kernel::dashboard::router(Arc::clone(&kernel));

    // First, seal the dashboard-token so the kernel
    // transitions to Full mode.
    let seal_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/pre-bootstrap/seal")
                .header("content-type", "application/json")
                .body(Body::from(seal_request_body(
                    "dashboard-token",
                    "my-secret-token",
                )))
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(seal_resp.status(), StatusCode::OK);

    // CID:pre-bootstrap-007 - bearer_auth_hash_only_path
    // Purpose: After sealing `dashboard-token`, the
    // bearer auth middleware must accept the `hash:`
    // prefix bearer: `Authorization: Bearer hash:<sha256(token)>`.
    // The `hash:` prefix means the middleware skips the
    // SHA-256 computation and delegates the compare
    // directly to `lookup_hash`.
    use sha2::{Digest, Sha256};
    let token = "my-secret-token";
    let hash = hex::encode(Sha256::digest(token.as_bytes()));

    // Hash-only path: `Authorization: Bearer hash:<hex>`
    // Note: `/spans/recent` requires a `since` RFC 3339 param.
    // Use a fixed RFC 3339 timestamp to avoid serialization
    // issues with `to_rfc3339()`.
    let since = "2026-01-01T00:00:00Z";
    let uri = format!("/spans/recent?since={}&limit=1", since);
    let hash_only_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(&uri)
                .header("Authorization", format!("Bearer hash:{}", hash))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        hash_only_resp.status(),
        StatusCode::NO_CONTENT,
        "hash-only bearer should be accepted; expected 204 No Content",
    );

    // Wrong hash must be rejected.
    let wrong_hash_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/spans/recent?since={}&limit=1", since))
                .header(
                    "Authorization",
                    "Bearer hash:0000000000000000000000000000000000000000000000000000000000000000",
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        wrong_hash_resp.status(),
        StatusCode::UNAUTHORIZED,
        "wrong hash should be rejected",
    );

    // Plaintext path still works.
    let plaintext_resp = app
        .clone()
        .oneshot(
            Request::builder()
                .method("GET")
                .uri(format!("/spans/recent?since={}&limit=1", since))
                .header("Authorization", format!("Bearer {}", token))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(
        plaintext_resp.status(),
        StatusCode::NO_CONTENT,
        "plaintext bearer should still be accepted",
    );
}
