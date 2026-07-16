//! Code Map: afa-security
//! - The "desk clerk" crate. Owns the master key (in a
//!   `Zeroizing<[u8; 32]>` so it is wiped on drop), the
//!   `Storage` (the vault, after the Phase 0.5a move from
//!   `SealedSecretStore` to `afa_storage::Storage`), and the
//!   `Arc<EventBus>` (the audit log). Implements the
//!   `SecurityV1` trait from `afa-contracts`.
//!
//! The crate is split into four modules:
//! - `crypto`: The pure-AEAD layer (`seal`, `open`,
//!   `NONCE_LEN`, `KEY_LEN`).
//! - `events`: Audit-event re-exports + the `now()` clock
//!   helper.
//! - `master_key`: The `MasterKey` newtype + the
//!   `from_hex` constructor.
//! - `storage`: The `Storage` re-export (was
//!   `SealedSecretStore` in pre-Phase-0.5a). Also
//!   includes `SCHEMA_MIGRATIONS`, the three
//!   constants (`SCHEMA_VERSION`, `STATUS_ACTIVE`,
//!   `STATUS_ROTATED`), and the `check_schema_version`
//!   and `open_storage` helpers.
//! - `engine`: The `SecurityEngine` struct and the
//!   `impl SecurityV1` block (seal, unseal, rotate,
//!   lookup_hash).
//!
//! The public surface is the `SecurityEngine` + the
//! `Storage` re-export. Every downstream adapter holds an
//! `Arc<dyn SecurityV1>` from the kernel; the engine is the
//! only `SecurityV1` impl in the v1 codebase.
//!
//! Story (plain English): This is the room in the bank where
//! the safe-deposit boxes live. The room has one clerk
//! (`SecurityEngine`) at one desk. The desk has three
//! things on it: a copy of the bank's master key (in a
//! sealed envelope), the index card file (the SQLite
//! `sealed_secrets` table), and a rubber stamp book (the
//! event bus) for stamping "I did a thing" notes. Any
//! customer who walks in has to talk to the clerk — there
//! is no self-service kiosk.
//!
//! **Doc drift corrections vs. the IMPL draft**:
//! - **#5**: the engine uses `Storage::with_conn` with
//!   `Box::pin(async move { ... })` (an async closure),
//!   not the IMPL's "sync `seal_secret` helper" — the
//!   IMPL's design would re-introduce the version race
//!   the `BEGIN IMMEDIATE` pattern was here to prevent.
//! - **#6**: the `rusqlite` dep stays (the engine writes
//!   its own SQL via `Storage::with_conn`); the IMPL said
//!   to remove it but the engine still needs the
//!   `rusqlite` types for `query_row`, `execute`,
//!   `Transaction`, etc.
//!
//! Quick lookup: rg -n "CID:afa-security-" crates/afa-security/src/

mod crypto;
mod engine;
mod events;
mod master_key;
mod storage;

// Public surface: the `SecurityEngine` (the only
// `SecurityV1` impl in v1) + the `Storage` re-export
// (downstream crates that need to read the
// `sealed_secrets` table directly — e.g. the
// future `afa-observability` engine — import
// `afa_security::Storage` to get the same `Storage`
// type the engine uses, without reaching into
// `afa-storage` directly).
pub use engine::SecurityEngine;
pub use storage::Storage;
// Re-export the schema version the kernel
// wraps into `SecurityErrorV1::SchemaVersionMismatch`
// (so the panic message shows the right
// "expected" number, not a hardcoded `1`).
pub use storage::SCHEMA_VERSION;
// Re-export the `open_storage` helper from the
// `storage` module so the kernel (the only caller
// that doesn't have direct access to the private
// module) can boot the SQLite file. Same pattern
// as the `Storage` re-export above — the
// `storage` module stays private to the crate;
// only the items the kernel needs are re-exported.
pub use storage::open_storage;
// Re-export the `MasterKey` newtype so the
// kernel (and any future crate that needs to
// accept a master key from the environment) can
// name the type without reaching into the
// private `master_key` module.
pub use master_key::MasterKey;

pub use afa_contracts::SecurityErrorV1;
// Type alias so internal modules (crypto, master_key,
// engine) can use the shorter name `SecurityError` in
// their function signatures and tests, matching the
// pre-Phase-0.5a style. The public re-export above
// (the contract type) is the canonical name; the
// alias is for internal ergonomics only.
pub use SecurityErrorV1 as SecurityError;
// Re-export the pure-AEAD primitives so the
// `crypto_roundtrip` integration test can exercise
// the seal/open boundary cases (0, 1, 64, 4096,
// 65535 bytes) directly, without going through
// the engine. The engine's `seal` / `unseal` are
// tested separately in `engine.rs`; this test
// is the "is the AES-256-GCM plumbing correct at
// the byte level?" check. The two constants
// (`NONCE_LEN`, `KEY_LEN`) are re-exported alongside
// so the test can name them without re-deriving
// them. The `crypto` module itself stays private
// (the engine is the only public-side caller;
// downstream adapters go through `SecurityV1`).
pub use crypto::{open, seal, KEY_LEN, NONCE_LEN};
// Re-export the audit-fact types so downstream
// adapters that subscribe to the bus do not have
// to reach into `afa-contracts` for them. The
// engine's `publish` sites also use these names
// (in `engine.rs`).
pub use events::{SecretRotated, SecretSealed, SecretUnsealed};
