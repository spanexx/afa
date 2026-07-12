//! Code Map: afa-security (the locked box)
//! - `crypto`: The pure-AEAD layer. `seal` encrypts a plaintext
//!   under a master key with a fresh random nonce; `open` decrypts
//!   and returns a `Zeroizing<Vec<u8>>` so the plaintext is wiped
//!   when the caller drops the buffer. See `crypto.rs`.
//! - `storage`: The SQLite-backed `SealedSecretStore`. Knows how
//!   to open or create the secrets database, run the idempotent
//!   schema, and `insert_active` / `get_active` / `get_any` /
//!   `rotate` rows. See `storage.rs`.
//! - `engine`: The `SecurityEngine` struct. Implements the locked
//!   `SecurityV1` trait by composing `crypto` + `storage` +
//!   the kernel's `EventBus` (Phase 2 publishes audit events).
//!   See `engine.rs`.
//! - `errors`: Re-exports the `SecurityErrorV1` enum from
//!   `afa-contracts` and gives it a canonical alias
//!   (`SecurityError`) for engine-internal code. See `errors.rs`.
//!
//! Story (plain English): This crate is the desk clerk and the
//! vault. The dictionary they speak (`SecurityV1`,
//! `SecretRef`, `UnsealedSecret`, `SecurityErrorV1`, the three
//! audit events) lives in `afa-contracts` — that file is the
//! dictionary, this file is the staff. The `crypto` module is
//! the encryption machine the clerk uses to put papers in the
//! box and take them out. The `storage` module is the vault
//! itself — a SQLite file that lists which box has which
//! version of which secret, in what state, and when it was
//! filed. The `engine` module is the clerk at the desk: it
//! takes a request, looks up the right box in the vault,
//! hands the papers to the caller for a moment, and stamps
//! the audit log. The `errors` module is the list of "sorry,
//! that didn't work" notes the clerk knows how to deliver
//! back to the caller.
//!
//! The kernel is the only crate that constructs an
//! `SecurityEngine`; downstream adapters (future packs) get
//! it through `Arc<dyn SecurityV1>`, never by building one
//! themselves. That is the whole point of the trait: the
//! adapter does not know there is a SQLite file behind the
//! desk, only that it can ask for a secret and get a
//! zeroing-on-drop handle back.
//!
//! CID Index:
//! CID:afa-security-lib-001 -> crypto
//! CID:afa-security-lib-002 -> storage
//! CID:afa-security-lib-003 -> engine
//! CID:afa-security-lib-004 -> errors
//! CID:afa-security-lib-005 -> events
//! CID:afa-security-lib-006 -> crate-root re-exports
//! CID:afa-security-lib-007 -> master_key
//!
//! Quick lookup: rg -n "CID:afa-security-lib-" crates/afa-security/src/lib.rs

#![doc(html_root_url = "https://docs.rs/afa-security/0.1.0")]

// CID:afa-security-lib-001 - crypto
// Purpose: The pure-AEAD layer. `seal` produces a fresh nonce and
// a ciphertext; `open` reverses it. The master key is held by the
// caller (the engine), never stored in this module. The output
// of `open` is `Zeroizing<Vec<u8>>` so the buffer is wiped on
// drop. Used by: `engine::SecurityEngine::seal` and `unseal`.
pub mod crypto;
// CID:afa-security-lib-002 - storage
// Purpose: The SQLite-backed `SealedSecretStore`. Owns the
// connection (behind a `tokio::sync::Mutex`), runs the
// idempotent schema, exposes `open_or_create`, `insert_active`,
// `get_active`, `get_any` (Phase 2), and `rotate` (Phase 2).
// Used by: `engine::SecurityEngine`.
pub mod storage;
// CID:afa-security-lib-003 - engine
// Purpose: The `SecurityEngine` struct. The desk clerk. Composes
// `crypto` + `storage` + the kernel's `EventBus` (Phase 2).
// Implements the locked `SecurityV1` trait. Used by: the kernel
// (constructs one), and every downstream adapter that needs a
// secret (holds an `Arc<dyn SecurityV1>` from the kernel).
pub mod engine;
// CID:afa-security-lib-004 - errors
// Purpose: Re-export `SecurityErrorV1` from `afa-contracts` and
// give it the alias `SecurityError` for engine-internal code.
// No new error variants are introduced here (per the IMPL's
// planning principle #2: "no new `AfaErrorKind` variants").
// Used by: every public function in this crate.
pub mod errors;
// CID:afa-security-lib-005 - events
// Purpose: Re-export the three audit-fact structs
// (`SecretSealed`, `SecretUnsealed`, `SecretRotated`) and
// provide a single `now()` helper for the `timestamp` field
// on every published event. See `events.rs` for the
// per-event Code Map.
// Used by: `engine::SecurityEngine` (every publish site),
// the audit-event shape test
// (`tests/audit_event_shape.rs`).
pub mod events;
// CID:afa-security-lib-007 - master_key
// Purpose: The `MasterKey` newtype. The single, type-safe
// envelope around the 32-byte master key: built by
// `from_hex` (the one and only path an env-var
// reader or a test uses), consumed by
// `SecurityEngine::new` (the one and only path the
// engine sees the key). The newtype is the only
// way the kernel touches the key, which lets the
// wipe-on-drop guarantee be tied to the type (a
// stray `[u8; 32]` would lose it). See `master_key.rs`
// for the per-method Code Map.
// Used by: `SecurityEngine::new` (takes `&MasterKey`),
// `tests/boot_failures.rs` `read_master_key_from_env`.
pub mod master_key;

// CID:afa-security-lib-006 - crate-root re-exports
// Purpose: Re-export the public types downstream code reaches
// for most often. Anything not re-exported here is not part of
// the contract.
pub use crate::engine::SecurityEngine;
pub use crate::errors::SecurityError;
pub use crate::events::{SecretRotated, SecretSealed, SecretUnsealed};
pub use crate::master_key::{MasterKey, MASTER_KEY_LEN};
pub use crate::storage::SealedSecretStore;
