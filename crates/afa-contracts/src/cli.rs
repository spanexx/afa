//! Code Map: `afa-cli` shared wire + state types
//!
//! - `PreBootstrapState`: The day-0 vault state as the
//!   CLI sees it from outside the kernel. Three
//!   variants (`Unsealed`, `Sealing`, `Sealed`). The
//!   CLI's `afa status` subcommand reads this from
//!   `KernelMode` via a thin adapter; the CLI never
//!   names `KernelMode` directly (the kernel-internal
//!   name is its concern).
//! - `PreBootstrapSealRequest` / `PreBootstrapSealResponse`:
//!   The wire shapes for the POST /pre-bootstrap/seal
//!   endpoint and for `afa secrets seal` (the CLI is
//!   the operator-driven alternative to the SPA Setup
//!   Wizard). Mirror the IMPL §"Phase 0" exactly:
//!   `value: String` (≥ 32 chars validated at handler
//!   time), `name: Option<String>` (defaults to
//!   `dashboard-token`), `secret_ref: SecretRef`,
//!   `hash: String` (SHA-256 hex).
//! - `SecretListEntry`: The CLI's `afa secrets list`
//!   row shape. Four fields per the IMPL: `name`,
//!   `active_version`, `last_sealed_at`,
//!   `last_rotated_at`.
//!
//! Story (plain English): the operator CLI's
//! vocabulary. The CLI talks to the kernel in terms
//! of `PreBootstrapState` and the three request /
//! response pairs; the kernel maps these to its own
//! `KernelMode` state machine and the existing
//! `SecurityV1` engine. The CLI never reaches into
//! kernel-internal types.
//!
//! CID Index:
//! CID:afa-contracts-cli-001 -> PreBootstrapState
//! CID:afa-contracts-cli-002 -> PreBootstrapSealRequest
//! CID:afa-contracts-cli-003 -> PreBootstrapSealResponse
//! CID:afa-contracts-cli-004 -> SecretListEntry
//!
//! Quick lookup: rg -n "CID:afa-contracts-cli-" crates/afa-contracts/src/cli.rs

use crate::kernel::KernelMode;
use crate::security::SecretRef;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// CID:afa-contracts-cli-005 - From<KernelMode>
// Purpose: Map the kernel-internal `KernelMode`
// (owned by `afa-kernel/src/mode.rs`) into the
// CLI-facing `PreBootstrapState`. The CLI never
// names `KernelMode` directly; the conversion is
// the boundary between the kernel's four-state
// machine (Booting, PreBootstrap, Sealing, Full)
// and the CLI's three-state machine (Unsealed,
// Sealing, Sealed). The contract:
// - `KernelMode::Booting` -> `Unsealed` (a fresh
//   boot has no sealed token)
// - `KernelMode::PreBootstrap` -> `Unsealed`
// - `KernelMode::Sealing` -> `Sealing`
// - `KernelMode::Full` -> `Sealed`
// Used by: the future `afa status` subcommand.
impl From<KernelMode> for PreBootstrapState {
    fn from(mode: KernelMode) -> Self {
        match mode {
            KernelMode::Booting | KernelMode::PreBootstrap { .. } => Self::Unsealed,
            KernelMode::Sealing { .. } => Self::Sealing,
            KernelMode::Full { .. } => Self::Sealed,
        }
    }
}

// CID:afa-contracts-cli-001 - PreBootstrapState
// Purpose: The day-0 vault state as the CLI sees
// it. The CLI's `afa status` subcommand reads this
// from `KernelMode` via a thin adapter. The
// `Default` impl is `Unsealed` (the v1 cold-start
// state for a fresh kernel).
// Used by: `afa-cli` (`afa status`) and the
// `GET /pre-bootstrap/state` endpoint (a future
// pack).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreBootstrapState {
    #[default]
    Unsealed,
    Sealing,
    Sealed,
}

// CID:afa-contracts-cli-002 - PreBootstrapSealRequest
// Purpose: The wire shape for the operator's day-0
// seal request. `value` is the plaintext (≥ 32 chars
// validated at handler / CLI time); `name` defaults
// to `"dashboard-token"` (the only name the v1
// handler accepts).
// Used by: the `POST /pre-bootstrap/seal` HTTP
// endpoint (matches the `dashboard::pre_bootstrap`
// handler's body shape) and `afa secrets seal` CLI
// subcommand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreBootstrapSealRequest {
    pub value: String,
    #[serde(default)]
    pub name: Option<String>,
}

// CID:afa-contracts-cli-003 - PreBootstrapSealResponse
// Purpose: The wire shape for a successful seal.
// `secret_ref` is the engine's `SecretRef`
// (name + version); `hash` is the SHA-256 hex the
// engine persisted (a future pack's `afa-cli trace`
// will print this so the operator can copy it into
// their dashboard).
// Used by: the `POST /pre-bootstrap/seal` HTTP
// endpoint and `afa secrets seal` CLI subcommand.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PreBootstrapSealResponse {
    pub secret_ref: SecretRef,
    pub hash: String,
}

// CID:afa-contracts-cli-004 - SecretListEntry
// Purpose: The CLI's `afa secrets list` row shape.
// `last_rotated_at` is `None` when the secret was
// sealed once and never rotated; `Some(...)` after
// the first rotation.
// Used by: `afa secrets list` CLI subcommand (future
// pack).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretListEntry {
    pub name: String,
    pub active_version: u32,
    pub last_sealed_at: DateTime<Utc>,
    pub last_rotated_at: Option<DateTime<Utc>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    // All four types are serde-de/serializable
    // round-trip. The CLI persists these to disk
    // (via `serde_json`) on `--output json` mode;
    // a regression in the wire shape would break
    // operator scripted workflows.
    #[test]
    fn pre_bootstrap_state_default_is_unsealed() {
        assert_eq!(PreBootstrapState::default(), PreBootstrapState::Unsealed);
        let json = serde_json::to_string(&PreBootstrapState::default()).expect("json");
        assert_eq!(json, "\"unsealed\"");
    }

    #[test]
    fn pre_bootstrap_state_round_trip() {
        for s in [
            PreBootstrapState::Unsealed,
            PreBootstrapState::Sealing,
            PreBootstrapState::Sealed,
        ] {
            let json = serde_json::to_string(&s).expect("json");
            let parsed: PreBootstrapState = serde_json::from_str(&json).expect("json parse");
            assert_eq!(parsed, s);
        }
    }

    #[test]
    fn pre_bootstrap_seal_request_defaults_name_to_none() {
        let json = r#"{"value":"super-secret-day-0-token-of-sufficient-length"}"#;
        let req: PreBootstrapSealRequest = serde_json::from_str(json).expect("json");
        assert!(req.name.is_none());
        assert_eq!(req.value.len(), 45);
    }

    #[test]
    fn secret_list_entry_serializes_datetimes_as_rfc3339() {
        let entry = SecretListEntry {
            name: "dashboard-token".to_string(),
            active_version: 1,
            last_sealed_at: "2026-07-17T12:00:00Z".parse::<DateTime<Utc>>().unwrap(),
            last_rotated_at: None,
        };
        let json = serde_json::to_string(&entry).expect("json");
        assert!(json.contains("2026-07-17T12:00:00Z"));
        assert!(json.contains("\"name\":\"dashboard-token\""));
        assert!(json.contains("\"active_version\":1"));
    }
}
