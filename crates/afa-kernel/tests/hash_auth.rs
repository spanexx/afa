use afa_kernel::Kernel;
use afa_security::MasterKey;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use sha2::{Digest, Sha256};
use std::sync::Arc;
use tempfile::TempDir;
use tower::ServiceExt;

async fn fresh_kernel_with_sealed_token(token: &str) -> (TempDir, Kernel) {
    let dir = tempfile::tempdir().expect("tempdir");
    let key = MasterKey::from([0x42u8; 32]);
    let kernel = Kernel::new(&key, dir.path().join("secrets.db"))
        .await
        .expect("kernel::new");
    // Seal a `dashboard-token` so the auth middleware
    // can validate incoming bearers. The kernel is in
    // PreBootstrap mode at boot; the only way to seal
    // is via the POST /pre-bootstrap/seal endpoint,
    // which is why we use it here (the test depends on
    // the existing Phase 4b handler).
    let app = afa_kernel::dashboard::router(Arc::new(kernel.clone()));
    let body = serde_json::to_vec(&serde_json::json!({"value": token, "name": "dashboard-token"}))
        .expect("json");
    app.oneshot(
        Request::builder()
            .method("POST")
            .uri("/pre-bootstrap/seal")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .expect("request"),
    )
    .await
    .expect("seal response");
    (dir, kernel)
}

fn sha256_hex(input: &str) -> String {
    let mut digest = Sha256::new();
    digest.update(input.as_bytes());
    format!("{:x}", digest.finalize())
}

#[tokio::test]
async fn hash_prefix_bearer_with_correct_hash_returns_200() {
    let (_dir, kernel) =
        fresh_kernel_with_sealed_token("super-secret-day-0-token-of-sufficient-length").await;
    let app = afa_kernel::dashboard::router(Arc::new(kernel));
    let hash = sha256_hex("super-secret-day-0-token-of-sufficient-length");
    let response = app
        .oneshot(
            Request::builder()
                .uri("/spans/recent?since=2020-01-01T00:00:00Z")
                .header("authorization", format!("Bearer hash:{hash}"))
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    // Accept either 200 (rows returned) or 204 (rows returned
    // but empty per the handler). The IMPL test only cares
    // that auth let the request through, not that the
    // handler's body shape.
    let s = response.status();
    assert!(s == StatusCode::OK || s == StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn hash_prefix_bearer_with_wrong_hash_returns_401() {
    let (_dir, kernel) =
        fresh_kernel_with_sealed_token("super-secret-day-0-token-of-sufficient-length").await;
    let app = afa_kernel::dashboard::router(Arc::new(kernel));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/spans/recent?since=2020-01-01T00:00:00Z")
                .header(
                    "authorization",
                    "Bearer hash:deadbeef".repeat(8).to_string() + &"0".repeat(64 - 32),
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn hash_prefix_bearer_with_malformed_hash_returns_401() {
    // A "hash:" prefix with a value that isn't
    // 64 hex chars. The middleware should return
    // 401 without leaking whether the prefix
    // matched.
    let (_dir, kernel) =
        fresh_kernel_with_sealed_token("super-secret-day-0-token-of-sufficient-length").await;
    let app = afa_kernel::dashboard::router(Arc::new(kernel));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/spans/recent?since=2020-01-01T00:00:00Z")
                .header("authorization", "Bearer hash:not-hex")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn plaintext_bearer_still_returns_200() {
    // The IMPL TRD OQ-1 says the Bearer <plaintext>
    // path is preserved (the hash: prefix is additive,
    // not a replacement). Verify the existing plaintext
    // path still works.
    let (_dir, kernel) =
        fresh_kernel_with_sealed_token("super-secret-day-0-token-of-sufficient-length").await;
    let app = afa_kernel::dashboard::router(Arc::new(kernel));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/spans/recent?since=2020-01-01T00:00:00Z")
                .header(
                    "authorization",
                    "Bearer super-secret-day-0-token-of-sufficient-length",
                )
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    let s = response.status();
    assert!(s == StatusCode::OK || s == StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn no_authorization_header_returns_401() {
    let (_dir, kernel) =
        fresh_kernel_with_sealed_token("super-secret-day-0-token-of-sufficient-length").await;
    let app = afa_kernel::dashboard::router(Arc::new(kernel));
    let response = app
        .oneshot(
            Request::builder()
                .uri("/spans/recent?since=2020-01-01T00:00:00Z")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}
