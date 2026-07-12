//! Code Map: Audit-event re-exports + timestamp helper
//! - `SecretSealed`, `SecretUnsealed`, `SecretRotated`:
//!   Re-exports of the three audit-fact structs from
//!   `afa-contracts::security`. The engine publishes one of
//!   these after every successful `seal`, `unseal`, or
//!   `rotate`. See `crates/afa-contracts/src/security.rs`
//!   for the per-event Code Map; this file is just the
//!   "lift the dictionary entries to desk height" re-export
//!   so engine-internal code does not have to reach into
//!   the `afa_contracts` crate on every publish call.
//! - `now`: A `chrono::Utc::now()` helper. Centralizing
//!   timestamp construction in one function makes it easy
//!   to swap the clock source in a future pack (e.g. a
//!   frozen-clock test helper, or a hardware RTC path)
//!   without touching every publish site. Every event the
//!   engine publishes uses this helper for its
//!   `timestamp` field.
//!
//! Story (plain English): Imagine a single rubber-stamp
//! the desk clerk uses on every "I did a thing" note. The
//! stamp's body (the words "SecretSealed", "SecretUnsealed",
//! "SecretRotated") was ordered from the dictionary
//! publisher; this file is the holder that keeps the stamp
//! at desk height. The stamp's clock (the timestamp) is a
//! single timepiece bolted to the desk so every stamped
//! note has the same source of time, instead of three
//! different wristwatches.
//!
//! CID Index:
//! CID:afa-security-events-001 -> SecretSealed
//! CID:afa-security-events-002 -> SecretUnsealed
//! CID:afa-security-events-003 -> SecretRotated
//! CID:afa-security-events-004 -> now
//!
//! Quick lookup: rg -n "CID:afa-security-events-" crates/afa-security/src/events.rs

// CID:afa-security-events-001 - SecretSealed
// Purpose: Re-export the `SecretSealed` audit-fact type
// from `afa_contracts::security`. The engine's `seal` path
// constructs one of these after a successful row insert
// and publishes it on the bus.
// Uses: AfaEvent (the badge), serde (so the audit log can
// serialize it), chrono (for the `timestamp` field).
// Used by: `engine::SecurityEngine::seal` (publishes
// after commit), the audit-event shape test
// (`tests/audit_event_shape.rs`).
pub use afa_contracts::SecretSealed;

// CID:afa-security-events-002 - SecretUnsealed
// Purpose: Re-export the `SecretUnsealed` audit-fact type.
// Carries the full `ExecutionContext` metadata (tenant,
// correlation, actor) so the audit trail can be tied
// back to the request that asked for the secret. Does
// NOT carry any field that could carry the plaintext —
// the field set is metadata only, per the
// "audit events publish metadata, never secrets" rule.
// Used by: `engine::SecurityEngine::unseal` (publishes
// after successful decrypt), the audit-event shape test.
pub use afa_contracts::SecretUnsealed;

// CID:afa-security-events-003 - SecretRotated
// Purpose: Re-export the `SecretRotated` audit-fact type.
// Carries the full `ExecutionContext` metadata plus the
// old and new version numbers, so a compliance tool can
// answer "who replaced secret X v3, and when, and from
// which request?"
// Used by: `engine::SecurityEngine::rotate` (publishes
// after commit), the audit-event shape test.
pub use afa_contracts::SecretRotated;

// CID:afa-security-events-004 - now
// Purpose: A single source of "what time is it right now?"
// for every event the engine publishes. Centralized so
// the future frozen-clock test helper (or a hardware
// RTC path) can swap the implementation in one place.
// Returns `chrono::DateTime<chrono::Utc>` (the
// `DateTime<Utc>` type alias from chrono, not the
// `DateTime` struct directly, so callers do not have to
// import chrono's path).
// Used by: every `publish(...)` call site in
// `engine::SecurityEngine`.
pub fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}
