//! Code Map: Security contract serde round-trip
//! - `secret_ref_round_trips_through_json`: `SecretRef` ->
//!   JSON -> `SecretRef` yields the same struct (catches
//!   missing fields, wrong field names, lost `version`).
//! - `secret_sealed_round_trips_through_json`: the
//!   `SecretSealed` audit fact round-trips with all its
//!   fields intact, including the chrono timestamp.
//! - `secret_unsealed_round_trips_through_json`: the
//!   `SecretUnsealed` audit fact round-trips with all
//!   four ExecutionContext fields (tenant, correlation,
//!   actor, timestamp) intact.
//! - `secret_rotated_round_trips_through_json`: the
//!   `SecretRotated` audit fact round-trips with all
//!   fields (name, old_version, new_version, ctx fields,
//!   timestamp) intact.
//!
//! Story (plain English): A receipt is only useful if you can
//! read it back later. These four tests prove the four "small
//! pieces of paper" the engine hands out are all legible: you
//! can write one down, hand it to someone else, and they can
//! read it back as the same thing you wrote. The timestamp is
//! the trickiest piece — it has to round-trip through JSON
//! without losing the timezone, and chrono's `DateTime<Utc>`
//! is the only kind the test trusts.

use afa_contracts::{
    Actor, CorrelationId, SecretRef, SecretRotated, SecretSealed, SecretUnsealed, TenantId,
};

#[test]
fn secret_ref_round_trips_through_json() {
    let original = SecretRef {
        name: "openai-api-key".to_string(),
        version: 7,
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: SecretRef = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(original, decoded);
    // The version number is the easy thing to lose in a serde
    // derive typo. Check it explicitly.
    assert_eq!(decoded.version, 7);
}

#[test]
fn secret_sealed_round_trips_through_json() {
    let original = SecretSealed {
        name: "stripe-webhook-secret".to_string(),
        version: 3,
        timestamp: chrono::DateTime::parse_from_rfc3339("2026-07-12T14:23:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: SecretSealed = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded.name, original.name);
    assert_eq!(decoded.version, original.version);
    // The chrono timestamp must round-trip without losing the
    // UTC zone — the audit log's "what time was this?" question
    // is meaningless if the timezone is wrong.
    assert_eq!(decoded.timestamp, original.timestamp);
    assert_eq!(decoded.timestamp.timezone(), chrono::Utc);
}

#[test]
fn secret_unsealed_round_trips_through_json() {
    let original = SecretUnsealed {
        name: "openai-api-key".to_string(),
        version: 7,
        tenant_id: TenantId::new("acme-realty"),
        correlation_id: CorrelationId::new(),
        actor: Actor::Channel {
            name: "http".to_string(),
        },
        timestamp: chrono::Utc::now(),
    };
    let json = serde_json::to_string(&original).expect("serialize");
    let decoded: SecretUnsealed = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded.name, original.name);
    assert_eq!(decoded.version, original.version);
    assert_eq!(decoded.tenant_id, original.tenant_id);
    assert_eq!(decoded.correlation_id, original.correlation_id);
    assert_eq!(decoded.actor, original.actor);
    // The audit fact's timestamp is the wall-clock the engine
    // saw the unseal at; the test only asserts it round-trips,
    // not that it matches anything else.
    assert_eq!(decoded.timestamp, original.timestamp);
}

#[test]
fn secret_rotated_round_trips_through_json() {
    let original = SecretRotated {
        name: "twilio-auth-token".to_string(),
        old_version: 1,
        new_version: 2,
        tenant_id: TenantId::new("acme-realty"),
        correlation_id: CorrelationId::new(),
        actor: Actor::Timer,
        timestamp: chrono::Utc::now(),
    };
    let json = serde_json::to_string(&original).expect("deserialize");
    let decoded: SecretRotated = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(decoded.name, original.name);
    assert_eq!(decoded.old_version, original.old_version);
    assert_eq!(decoded.new_version, original.new_version);
    assert_eq!(decoded.tenant_id, original.tenant_id);
    assert_eq!(decoded.correlation_id, original.correlation_id);
    assert_eq!(decoded.actor, original.actor);
    assert_eq!(decoded.timestamp, original.timestamp);
}
