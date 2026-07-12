//! Code Map: Identity newtypes
//! - `CorrelationId`: A small wrapper around a random ID. Every
//!   request gets one, and the same ID rides along on every log line,
//!   event, and span that the request produces — so you can search
//!   for one ID and see the whole story of that request.
//! - `TenantId`: A name tag for the agency using the kernel. It
//!   identifies *which* customer owns a given request. The kernel
//!   stores it as a plain string; no cleaning, no validation.
//!
//! Story (plain English): Imagine a busy post office. Every parcel
//! that comes in gets a tracking number (`CorrelationId`) glued to
//! every form and receipt it touches. The post office also has a
//! return address stamp (`TenantId`) saying which business sent
//! it. The numbers and the stamps are small, simple wrappers — but
//! they let you trace anything that happened to anything.
//!
//! CID Index:
//! CID:ids-001 -> CorrelationId
//! CID:ids-002 -> TenantId
//!
//! Quick lookup: rg -n "CID:ids-" crates/afa-contracts/src/ids.rs

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

// CID:ids-001 - CorrelationId
// Purpose: Hands out a unique tracking number for one request, and
// lets that number travel with every log line and event the request
// produces. Two of these are equal only when their inner IDs match.
// Uses: uuid::Uuid (the actual random-number source), serde (to
// ride along on JSON, logs, and events).
// Used by: ExecutionContext (every request carries one), the event
// bus (every event carries one), and every log span the kernel
// opens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CorrelationId(pub Uuid);

impl CorrelationId {
    /// Generate a fresh random correlation ID.
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl From<Uuid> for CorrelationId {
    fn from(id: Uuid) -> Self {
        Self(id)
    }
}

impl From<CorrelationId> for Uuid {
    fn from(id: CorrelationId) -> Self {
        id.0
    }
}

// CID:ids-002 - TenantId
// Purpose: A plain name tag for the agency that owns a request.
// Like a return address stamp. The kernel keeps the string exactly
// as it was given — no trimming, no lowercasing — so a typo in one
// place is a typo everywhere, which is what we want for audits.
// Uses: serde (to ride along on JSON).
// Used by: ExecutionContext (every request is scoped to one
// TenantId), the security engine (every secret is bound to a
// TenantId), and the observability layer.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TenantId(pub String);

impl TenantId {
    /// Wrap a string as a `TenantId`. No normalization.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

impl fmt::Display for TenantId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl AsRef<str> for TenantId {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correlation_id_new_produces_distinct_values() {
        let a = CorrelationId::new();
        let b = CorrelationId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn correlation_id_debug_contains_valid_uuid_string() {
        let id = CorrelationId::new();
        let dbg = format!("{id:?}");
        // `Debug` prints the newtype wrapper: `CorrelationId(<uuid>)`.
        // Strip the wrapper and parse the inner UUID.
        let inner = dbg
            .strip_prefix("CorrelationId(")
            .and_then(|s| s.strip_suffix(")"))
            .unwrap_or_else(|| panic!("unexpected Debug shape: {dbg}"));
        assert!(
            Uuid::parse_str(inner).is_ok(),
            "expected inner Debug output to be a valid UUID, got: {inner}"
        );
    }

    #[test]
    fn tenant_id_debug_includes_inner_string() {
        let id = TenantId::new("acme-realty");
        let dbg = format!("{id:?}");
        // The Debug output must include the inner string verbatim.
        assert!(
            dbg.contains("acme-realty"),
            "Debug should include the inner string, got: {dbg}"
        );
    }

    #[test]
    fn tenant_id_no_normalization() {
        // Per the locked decision: no trimming, no lowercasing, no
        // validation — the inner string is the inner string.
        let id = TenantId::new("  Acme  ");
        assert_eq!(id.0, "  Acme  ");
    }
}
