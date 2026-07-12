//! Code Map: Security engine error re-exports
//! - `SecurityError`: The local alias for the locked
//!   `SecurityErrorV1` enum from `afa-contracts`. Engine-internal
//!   code uses the short name; the `SecurityV1` trait method
//!   signatures use the long name (to match the trait).
//!
//! Story (plain English): Imagine a sign on the desk that lists
//! the eleven "sorry, that didn't work" notes the clerk knows
//! how to deliver. The sign itself is owned by the dictionary
//! (the `afa-contracts` crate), not by the desk. This file is
//! just a smaller copy of the same sign, hung at desk height so
//! the clerk can read it without leaning over to the dictionary
//! shelf. The contents are identical; only the name on the
//! clipboard is shorter.
//!
//! We deliberately do NOT add new error variants here. Every
//! thing that can go wrong in this crate is one of the eleven
//! variants the dictionary already lists. If a new failure
//! mode appears (e.g. "the SQLite file is on a read-only
//! filesystem"), it goes into the dictionary first as an
//! ADR-backed change to `SecurityErrorV1`; this file picks up
//! the new variant on the next rebuild. The IMPL's planning
//! principle #2 says this is a hard rule.
//!
//! CID Index:
//! CID:afa-security-errors-001 -> SecurityError alias
//!
//! Quick lookup: rg -n "CID:afa-security-errors-" crates/afa-security/src/errors.rs

// CID:afa-security-errors-001 - SecurityError alias
// Purpose: Engine-internal short name for the contract-level
// `SecurityErrorV1` enum. The trait method signatures on
// `SecurityV1` use the long name (so the trait and the
// dictionary stay visually identical); the engine's own
// internal code uses the short name (so the function bodies
// stay readable). Re-exported at the crate root as
// `afa_security::SecurityError`.
// Used by: every public function in this crate.
pub use afa_contracts::SecurityErrorV1 as SecurityError;
