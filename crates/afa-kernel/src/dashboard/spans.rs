//! Code Map: Dashboard span handlers
//!
//! - `by_correlation_id`: Returns spans for one request.
//! - `recent`: Returns spans after a UUID cursor.
//! - `stream`: Upgrades to WebSocket and forwards persisted spans.
//!
//! Story (plain English): private patient charts and the live
//! monitor. Queries read the logbook; the monitor receives copies
//! of new notes without slowing the recording nurse.
//!
//! CID Index:
//! CID:dashboard-spans-001 -> by_correlation_id
//! CID:dashboard-spans-002 -> recent
//! CID:dashboard-spans-003 -> stream
//!
//! Quick lookup: rg -n "CID:dashboard-spans-" crates/afa-kernel/src/dashboard/spans.rs
//!
//! Doc drift corrections vs. the IMPL draft:
//! - **#10**: KernelConfig deferral — the IMPL draft proposed a
//!   `KernelConfig` struct for `Kernel::new`. Shipped preserves
//!   the `(master_key, secrets_db_path)` signature and passes
//!   `Arc<Kernel>` via `DashboardState` to the handlers instead.
//!   See dashboard.rs for the full correction.
//! - **#11**: The `/spans/stream` route is registered with
//!   `any(spans::stream)` (see dashboard.rs line 45), not with
//!   `get(stream_handler)` as the IMPL draft stated. axum 0.8
//!   requires `any()` for WebSocket upgrade handshakes.

use super::DashboardState;
use afa_contracts::{CorrelationId, SpanRecord};
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;
use uuid::Uuid;

fn bad_request(message: &'static str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        Json(serde_json::json!({ "error": message })),
    )
        .into_response()
}

fn empty() -> Response {
    StatusCode::NO_CONTENT.into_response()
}

// CID:dashboard-spans-001 - by_correlation_id
// Purpose: Parse a correlation UUID, query the spans database,
// and return 200 JSON or 204 when no rows exist.
// Used by: GET /spans/{correlation_id}.
pub(crate) async fn by_correlation_id(
    State(state): State<DashboardState>,
    Path(raw): Path<String>,
) -> Response {
    let Ok(uuid) = Uuid::parse_str(&raw) else {
        return bad_request("invalid correlation_id");
    };
    let correlation_id = CorrelationId(uuid);
    match afa_observability::persistence::read_by_correlation_id(
        state.kernel.observability().storage(),
        &correlation_id,
    )
    .await
    {
        Ok(rows) if rows.is_empty() => empty(),
        Ok(rows) => Json(rows).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct RecentQuery {
    since: Option<String>,
    limit: Option<String>,
}

// CID:dashboard-spans-002 - recent
// Purpose: Parse the RFC-3339 timestamp cursor and bounded
// limit, then return 200 JSON or 204 when no rows are newer
// than the cursor.
// Used by: GET /spans/recent.
pub(crate) async fn recent(
    State(state): State<DashboardState>,
    Query(query): Query<RecentQuery>,
) -> Response {
    let Some(since) = query.since.as_deref() else {
        return bad_request("invalid since or limit");
    };
    let Ok(since) = chrono::DateTime::parse_from_rfc3339(since) else {
        return bad_request("invalid since or limit");
    };
    let since = since.with_timezone(&chrono::Utc);
    let limit = match query.limit.as_deref() {
        None => 100,
        Some(raw) => match raw.parse::<u32>() {
            Ok(limit) => limit.min(1000),
            Err(_) => return bad_request("invalid since or limit"),
        },
    };
    match afa_observability::persistence::read_after_started_at(
        state.kernel.observability().storage(),
        since,
        limit,
    )
    .await
    {
        Ok(rows) if rows.is_empty() => empty(),
        Ok(rows) => Json(rows).into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

// CID:dashboard-spans-003 - stream
// Purpose: Upgrade an authenticated request and forward one JSON
// text message for every successfully persisted span until the
// client disconnects.
// Used by: GET /spans/stream.
pub(crate) async fn stream(
    State(state): State<DashboardState>,
    upgrade: WebSocketUpgrade,
) -> Response {
    let observability = state.kernel.observability();
    upgrade.on_upgrade(move |mut socket| async move {
        let mut receiver = observability.subscribe_spans();
        while let Ok(record) = receiver.recv().await {
            let Ok(json) = serde_json::to_string::<SpanRecord>(&record) else {
                continue;
            };
            if socket
                .send(axum::extract::ws::Message::Text(json.into()))
                .await
                .is_err()
            {
                break;
            }
        }
    })
}
