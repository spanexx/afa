//! Code Map: Dashboard bearer authentication
//!
//! - `bearer_auth`: Validates the HTTP Authorization bearer
//!   without logging or returning token contents.
//!
//! Story (plain English): the nurse's badge reader. It reports
//! only accepted or rejected; it never prints the badge number or
//! explains which part was wrong.
//!
//! CID Index:
//! CID:dashboard-auth-001 -> bearer_auth
//!
//! Quick lookup: rg -n "CID:dashboard-auth-" crates/afa-kernel/src/dashboard/auth.rs
//!
//! Doc drift corrections vs. the IMPL draft:
//! - **#9**: the IMPL §Phase 3 dependencies listed the `subtle`
//!   crate for constant-time bearer-token comparison. Shipped
//!   omits `subtle` entirely: the engine (`SecurityV1::lookup_hash`)
//!   performs a constant-time comparison internally, and the
//!   dashboard only receives the boolean result. The `sha2` crate
//!   (0.10) is used to hash the raw token before calling
//!   `lookup_hash("dashboard-token", sha256_hex)`, but the
//!   constant-time step lives in the engine, not here.
//!   The `clap` crate (also listed in the IMPL) is omitted because
//!   no `main.rs` or CLI binary exists in this pack; CLI wiring
//!   is deferred to a future `afa-cli` pack.

use super::DashboardState;
use axum::extract::State;
use axum::http::{header, Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};

fn unauthorized() -> Response {
    let mut response = StatusCode::UNAUTHORIZED.into_response();
    response.headers_mut().insert(
        header::WWW_AUTHENTICATE,
        "Bearer realm=\"afa\"".parse().expect("static header"),
    );
    response
}

// CID:dashboard-auth-001 - bearer_auth
// Purpose: Validate a bearer token for protected dashboard
// routes. Accepts `Bearer token` and `Bearer hash:<sha256>`;
// both paths call SecurityV1::lookup_hash and return the same
// empty 401 on every failure.
// Used by: the `/spans/*` router layer.
pub(crate) async fn bearer_auth(
    State(state): State<DashboardState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let bearer = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .or_else(|| {
            request.uri().query().and_then(|query| {
                query.split('&').find_map(|pair| {
                    let (key, value) = pair.split_once('=')?;
                    (key == "token").then_some(value)
                })
            })
        });
    let Some(bearer) = bearer else {
        return unauthorized();
    };
    if bearer.is_empty() {
        return unauthorized();
    }

    let incoming_hash = if let Some(hash) = bearer.strip_prefix("hash:") {
        hash.to_string()
    } else {
        let mut digest = Sha256::new();
        digest.update(bearer.as_bytes());
        format!("{:x}", digest.finalize())
    };

    let valid = state
        .kernel
        .security()
        .lookup_hash("dashboard-token", &incoming_hash)
        .await
        .unwrap_or(false);
    if valid {
        next.run(request).await
    } else {
        unauthorized()
    }
}
