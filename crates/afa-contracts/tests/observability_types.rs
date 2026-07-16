//! Code Map: Observability types serde round-trip
//! - `span_record_round_trips_through_json`: `SpanRecord`
//!   -> JSON -> `SpanRecord` yields the same struct (the
//!   primary regression-proof for the spans table wire
//!   format).
//! - `health_report_round_trips_through_json`: `HealthReport`
//!   -> JSON -> `HealthReport` (the per-engine health
//!   envelope's wire format).
//! - `spans_write_failed_round_trips_through_json`: the
//!   `SpansWriteFailed` audit fact (so the dashboard can
//!   read it back from the event bus).
//! - `spans_purged_round_trips_through_json`: the
//!   `SpansPurged` audit fact.
//! - `spans_purge_failed_round_trips_through_json`: the
//!   `SpansPurgeFailed` audit fact.
//!
//! Story (plain English): A wire format is only useful if
//! you can read it back later. These five tests prove the
//! five "small pieces of paper" the observability engine
//! hands out are all legible: you can write one down, hand
//! it to someone else (the dashboard, an event-bus
//! subscriber, a JSON log line), and they can read it back
//! as the same thing you wrote. The trickiest pieces are
//! the chrono timestamps and the BTreeMap attributes —
//! they have to round-trip through JSON without losing the
//! timezone or the ordering.

use afa_contracts::{
    Actor, CorrelationId, HealthReport, HealthStatus, ObservabilityErrorV1, SpanOutcome,
    SpanRecord, SpansPurgeFailed, SpansPurged, SpansWriteFailed, TenantId,
};
use std::collections::BTreeMap;

#[test]
fn span_record_round_trips_through_json() {
    let mut attributes = BTreeMap::new();
    attributes.insert("model".to_string(), "gpt-4o".to_string());
    attributes.insert("tokens".to_string(), "1234".to_string());

    let original = SpanRecord {
        span_id: uuid::Uuid::new_v4(),
        parent_span_id: Some(uuid::Uuid::new_v4()),
        correlation_id: CorrelationId::new(),
        tenant_id: TenantId::new("acme-realty"),
        actor: Actor::Channel {
            name: "http".to_string(),
        },
        engine: "afa-llm".to_string(),
        operation: "llm.complete".to_string(),
        started_at: chrono::DateTime::parse_from_rfc3339("2026-07-12T14:23:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        duration_ms: 142,
        outcome: SpanOutcome::Ok,
        attributes: attributes.clone(),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: SpanRecord = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, decoded);
    // The 11-field shape is the regression target — check
    // each field is preserved (the easy ones to lose in a
    // serde derive typo are the `parent_span_id` and the
    // `attributes` BTreeMap).
    assert_eq!(decoded.parent_span_id, original.parent_span_id);
    assert_eq!(decoded.attributes, original.attributes);
    assert_eq!(decoded.engine, "afa-llm");
    assert_eq!(decoded.operation, "llm.complete");
    assert_eq!(decoded.duration_ms, 142);
    assert_eq!(decoded.outcome, SpanOutcome::Ok);
    // The timestamp must round-trip without losing the
    // UTC zone — the dashboard's "when did this happen?"
    // question is meaningless if the timezone is wrong.
    assert_eq!(decoded.started_at, original.started_at);
    assert_eq!(decoded.started_at.timezone(), chrono::Utc);
}

#[test]
fn span_record_with_no_parent_round_trips() {
    // The root span of a request has `parent_span_id: None`.
    // This is a common case (one per request), so test it
    // explicitly — a `#[serde(skip_serializing_if = ...)]` on
    // a future change would break it.
    let original = SpanRecord {
        span_id: uuid::Uuid::new_v4(),
        parent_span_id: None,
        correlation_id: CorrelationId::new(),
        tenant_id: TenantId::new("acme-realty"),
        actor: Actor::Timer,
        engine: "afa-kernel".to_string(),
        operation: "runtime.dispatch".to_string(),
        started_at: chrono::Utc::now(),
        duration_ms: 5,
        outcome: SpanOutcome::Ok,
        attributes: BTreeMap::new(),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: SpanRecord = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded.parent_span_id, None);
    assert_eq!(original, decoded);
}

#[test]
fn span_outcome_err_round_trips() {
    let original = SpanRecord {
        span_id: uuid::Uuid::new_v4(),
        parent_span_id: None,
        correlation_id: CorrelationId::new(),
        tenant_id: TenantId::new("acme-realty"),
        actor: Actor::Internal {
            caller: "scheduler".to_string(),
        },
        engine: "afa-knowledge".to_string(),
        operation: "knowledge.find_information".to_string(),
        started_at: chrono::Utc::now(),
        duration_ms: 88,
        outcome: SpanOutcome::Err {
            kind: afa_contracts::AfaErrorKind::Unavailable,
            reason: "knowledge index load failed".to_string(),
        },
        attributes: BTreeMap::new(),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: SpanRecord = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, decoded);
    if let SpanOutcome::Err { kind, reason } = &decoded.outcome {
        assert_eq!(*kind, afa_contracts::AfaErrorKind::Unavailable);
        assert_eq!(reason, "knowledge index load failed");
    } else {
        panic!("expected Err outcome, got Ok");
    }
}

#[test]
fn health_report_round_trips_through_json() {
    let mut engines = BTreeMap::new();
    engines.insert("afa-llm".to_string(), HealthStatus::Healthy);
    engines.insert(
        "afa-knowledge".to_string(),
        HealthStatus::Degraded {
            reason: "3 drops in last hour".to_string(),
        },
    );
    engines.insert(
        "afa-security".to_string(),
        HealthStatus::Unhealthy {
            reason: "secrets storage unreachable".to_string(),
        },
    );
    let original = HealthReport {
        overall: HealthStatus::Unhealthy {
            reason: "1 of 3 engines unhealthy".to_string(),
        },
        engines: engines.clone(),
        checked_at: chrono::DateTime::parse_from_rfc3339("2026-07-15T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: HealthReport = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, decoded);
    // The BTreeMap ordering must be preserved (the dashboard
    // expects the JSON output to be deterministic — same
    // engine set twice should serialise to the same bytes).
    assert_eq!(decoded.engines.get("afa-llm"), Some(&HealthStatus::Healthy));
    assert_eq!(
        decoded.engines.get("afa-knowledge"),
        Some(&HealthStatus::Degraded {
            reason: "3 drops in last hour".to_string()
        })
    );
    assert_eq!(decoded.checked_at, original.checked_at);
}

#[test]
fn spans_write_failed_round_trips_through_json() {
    let original = SpansWriteFailed {
        count: 1,
        reason: "spans DB chmod 000".to_string(),
        occurred_at: chrono::DateTime::parse_from_rfc3339("2026-07-15T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        correlation_id: CorrelationId::new(),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: SpansWriteFailed = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, decoded);
    // The `correlation_id` is the forensic link back to the
    // dropped span — it must round-trip exactly.
    assert_eq!(decoded.correlation_id, original.correlation_id);
    assert_eq!(decoded.count, 1);
    assert_eq!(decoded.reason, "spans DB chmod 000");
}

#[test]
fn spans_purged_round_trips_through_json() {
    let original = SpansPurged {
        count: 12_400,
        older_than: chrono::DateTime::parse_from_rfc3339("2026-07-08T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        occurred_at: chrono::DateTime::parse_from_rfc3339("2026-07-15T03:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        correlation_id: CorrelationId::new(),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: SpansPurged = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, decoded);
    assert_eq!(decoded.count, 12_400);
    // The `older_than` cutoff must round-trip — a
    // misaligned timezone would silently purge the wrong
    // rows.
    assert_eq!(decoded.older_than, original.older_than);
    assert_eq!(decoded.older_than.timezone(), chrono::Utc);
}

#[test]
fn spans_purge_failed_round_trips_through_json() {
    let original = SpansPurgeFailed {
        count: 0,
        reason: "spans DB locked by another holder".to_string(),
        older_than: chrono::DateTime::parse_from_rfc3339("2026-07-08T00:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
        occurred_at: chrono::Utc::now(),
        correlation_id: CorrelationId::new(),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: SpansPurgeFailed = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, decoded);
    assert_eq!(decoded.count, 0);
    assert_eq!(decoded.reason, "spans DB locked by another holder");
}

#[test]
fn observability_error_v1_serializes_via_display() {
    // The audit log shows `Display` of the error. The
    // `thiserror::Error` derive wires `Display` so the
    // assertion below catches any future change that
    // breaks the format strings.
    let e = ObservabilityErrorV1::SchemaVersionMismatch {
        found: 2,
        expected: 1,
    };
    assert_eq!(
        format!("{e}"),
        "spans storage schema version mismatch (found 2, expected 1)"
    );
}
