//! Code Map: Security contract surface
//! - `SecurityV1`: The "I am a security engine" badge. The locked
//!   v1 trait every engine implements. See `SecurityV1` below.
//! - `SecretRef`: The opaque receipt a caller gets back from
//!   `seal`. See `SecretRef` below.
//! - `UnsealedSecret`: The zeroing-on-drop handle to a sealed
//!   secret's plaintext, briefly opened. See `UnsealedSecret`
//!   below.
//! - `SecurityErrorV1`: The eleven "what went wrong with security?"
//!   buckets. See `SecurityErrorV1` below.
//! - `SecretSealed` / `SecretUnsealed` / `SecretRotated`: The three
//!   audit facts the engine publishes on the event bus. See the
//!   three structs at the bottom of the file.
//!
//! Story (plain English): Imagine a bank's safe-deposit box desk.
//! You walk in, give the clerk a small box of papers, ask them to
//! label it "tax returns 2025", and walk out with a paper receipt
//! that says "tax returns 2025, box 7". The receipt
//! (`SecretRef`) is small and never says what's inside — it just
//! says where the box is. The box itself is in a vault you can't
//! see. When you come back with the receipt, the clerk disappears
//! for a moment, opens box 7, and hands you the papers
//! (`UnsealedSecret`) so you can read them at the desk. As soon as
//! you walk away from the desk, the papers are shredded — that's
//! the "zeroize on drop" rule, so a passerby can't see them in
//! the trash. If you ask the clerk to open box 7 again next week,
//! they go to the vault and either find it (and you get fresh
//! papers), find it marked "rotated" (you get told "this receipt
//! is from before the box was swapped"), or don't find it at all
//! (you get told "we have no record of this box"). Every time
//! they do anything, they stamp a small line in the audit log
//! (`SecretSealed`, `SecretUnsealed`, or `SecretRotated`) — never
//! with the actual papers, just with the receipt number and the
//! time.
//!
//! This file is just the contract — the dictionary the engine
//! promises to honour. The actual desk clerk and vault are in
//! the `afa-security` crate (a later phase of this pack). The
//! dictionary is the only thing in `afa-contracts`, because the
//! dictionary is small, never does I/O, and is the same for
//! every deployment (a real bank, a crypto wallet plugin, and a
//! test fixture all use exactly the same words).
//!
//! CID Index:
//! CID:security-001 -> SecurityV1
//! CID:security-002 -> SecretRef
//! CID:security-003 -> UnsealedSecret
//! CID:security-004 -> SecurityErrorV1
//! CID:security-005 -> SecretSealed
//! CID:security-006 -> SecretUnsealed
//! CID:security-007 -> SecretRotated
//!
//! Quick lookup: rg -n "CID:security-" crates/afa-contracts/src/security.rs

use crate::error::AfaErrorKind;
use crate::events::AfaEvent;
use crate::execution_context::ExecutionContext;
use crate::ids::CorrelationId;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::ops::Deref;
use zeroize::Zeroizing;

// CID:security-001 - SecurityV1
// Purpose: The locked v1 contract for any security engine. Three
// methods, all `async`, all returning `Result<_, SecurityErrorV1>`.
// The `Send + Sync` supertrait lets the Capability Registry (a
// later pack) hold `Arc<dyn SecurityV1>` and share it across
// tasks. The dyn-compatibility is provided by `#[async_trait]`
// (the same pattern `afa-contracts` itself uses for
// `ExampleThingV1`); native `async fn` in traits is not
// object-safe.
// Uses: SecretRef, UnsealedSecret, ExecutionContext,
// SecurityErrorV1.
// Used by: the engine implementation in `afa-security` and every
// adapter that needs a secret (the OpenAI adapter for its API
// key, a future webhook plugin for its signing secret, etc.).
#[async_trait]
pub trait SecurityV1: Send + Sync {
    /// Store `plaintext` under `name`. The engine picks the next
    /// version number and returns the receipt. Validation:
    /// `plaintext.len() <= 64 * 1024`, `name.len() <= 256`.
    /// Publishes a `SecretSealed` audit fact.
    async fn seal(&self, plaintext: &[u8], name: &str) -> Result<SecretRef, SecurityErrorV1>;

    /// Open the sealed payload behind `secret_ref` (using the
    /// context's tenant/correlation/actor for audit purposes) and
    /// hand back a zeroing-on-drop handle to the plaintext. If
    /// the version is unknown returns `SecretNotFound`; if the
    /// version is known but already rotated, `SecretRotated`; if
    /// the AEAD tag fails, `DecryptionFailed`. Publishes a
    /// `SecretUnsealed` audit fact.
    async fn unseal(
        &self,
        secret_ref: &SecretRef,
        ctx: &ExecutionContext,
    ) -> Result<UnsealedSecret, SecurityErrorV1>;

    /// Replace the secret behind `secret_ref` with a new
    /// plaintext (using the context for audit purposes). The old
    /// `SecretRef` becomes `SecretRotated` from this call onward.
    /// Returns the new `SecretRef`. Publishes a `SecretRotated`
    /// audit fact.
    async fn rotate(
        &self,
        secret_ref: &SecretRef,
        new_plaintext: &[u8],
        ctx: &ExecutionContext,
    ) -> Result<SecretRef, SecurityErrorV1>;
}

// CID:security-002 - SecretRef
// Purpose: The opaque receipt a caller gets back from `seal` and
// `rotate`. It is small, `Clone`able, `Serialize`/`Deserialize`,
// and `Hash` so it can ride a `HashSet` or a `HashMap` key (the
// Capability Registry uses that). It has no `Display` impl on
// purpose: the receipt is opaque to humans, and a stray
// `format!("{}", ref)` would invite `Display` to leak the name
// into a log line in a way the next maintainer didn't expect.
// The name is the human-meaningful label the caller passed to
// `seal`; the version is the integer counter the engine picked.
// Uses: serde, the standard derives.
// Used by: every caller that holds a secret across more than
// one async call (e.g. an HTTP adapter that wants to reuse the
// same key for many requests).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SecretRef {
    /// The human-meaningful label the caller passed to `seal`.
    pub name: String,
    /// The integer counter the engine picked for this version.
    pub version: u32,
}

// CID:security-003 - UnsealedSecret
// Purpose: The zeroing-on-drop handle to a sealed secret's
// plaintext, briefly opened. Wraps `Zeroizing<Vec<u8>>` from the
// RustCrypto `zeroize` crate. When the handle goes out of scope,
// the underlying buffer is overwritten with zeros via a volatile
// write (the optimizer is not allowed to elide the zeroing as
// "dead store"). The `Deref<Target = [u8]>` impl lets a caller
// pass `&handle[..]` to an HTTPS client without copying the
// bytes. It deliberately does NOT implement `Clone` (a clone
// would double the plaintext exposure surface), `Display`, or
// `Debug` (so a stray `format!("{:?}", handle)` cannot leak the
// plaintext into a log line by accident).
// Uses: `zeroize::Zeroizing`.
// Used by: every caller that needs the plaintext bytes for the
// duration of one operation (an HTTPS request, a one-shot
// signature, etc.).
pub struct UnsealedSecret(Zeroizing<Vec<u8>>);

impl UnsealedSecret {
    /// Wrap a `Vec<u8>` as an `UnsealedSecret` (zeroing on drop).
    /// Intended for use by engine implementations, not by
    /// ordinary callers.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

impl Deref for UnsealedSecret {
    type Target = [u8];

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

// CID:security-004 - SecurityErrorV1
// Purpose: The eleven "what went wrong with security?" buckets.
// The closed set maps cleanly onto the six coarse `AfaErrorKind`
// buckets the kernel already understands, with no new kinds
// introduced (the `#[non_exhaustive]` on `AfaErrorKind` is what
// makes adding a new bucket a deliberate ADR-backed change — we
// are explicitly NOT adding one here). The variant names are the
// locked shape from the TRD §2.2 table. The fields on each
// variant carry the minimum information an operator needs to
// diagnose the failure (e.g. `PayloadTooLarge { size, cap }` so
// they can see the actual size vs. the cap).
// Uses: thiserror (for the `Display` + `source()` impls and the
// `std::error::Error` derive), `AfaError` (for the kernel-wide
// kind mapping).
// Used by: every method on `SecurityV1` (and, transitively, by
// every adapter that calls those methods).
#[derive(Debug, thiserror::Error)]
pub enum SecurityErrorV1 {
    /// Boot-time: the `AFA_MASTER_KEY` env var was not set.
    #[error("master key missing from the environment")]
    MasterKeyMissing,
    /// Boot-time: the `AFA_MASTER_KEY` env var was not valid
    /// 64-hex-char / 32-byte form.
    #[error("master key malformed: {reason}")]
    MasterKeyMalformed { reason: &'static str },
    /// The SQLite file at the configured path is not reachable
    /// (path doesn't exist and can't be created, or the directory
    /// is not writable).
    #[error("secrets storage is unreachable: {reason}")]
    StorageUnreachable { reason: String },
    /// The SQLite file is reachable but its contents are not
    /// readable as expected (e.g. truncated, or the magic bytes
    /// are wrong).
    #[error("secrets storage is corrupted")]
    StorageCorrupted,
    /// The SQLite file's `schema_version` is not the one this
    /// engine version supports. The admin must run the
    /// migration tool from a later pack.
    #[error("secrets storage schema version mismatch (found {found}, expected {expected})")]
    SchemaVersionMismatch { found: u32, expected: u32 },
    /// The `plaintext` argument was bigger than the 64 KiB cap.
    #[error("secrets payload too large ({size} bytes; cap is {cap})")]
    PayloadTooLarge { size: usize, cap: usize },
    /// The `name` argument was longer than the 256-byte cap.
    #[error("secrets name too long ({length} bytes; cap is {cap})")]
    NameTooLong { length: usize, cap: usize },
    /// No row exists for `(name, version)`. Either the secret
    /// was never sealed under that name, or the version number
    /// is wrong.
    #[error("secret not found: {name} v{version}")]
    SecretNotFound { name: String, version: u32 },
    /// The row exists for `(name, version)` but its `status`
    /// is `rotated` — a newer version has taken over.
    #[error("secret already rotated: {name} v{version}")]
    SecretRotated { name: String, version: u32 },
    /// The AEAD tag check failed. The row's ciphertext was
    /// tampered with, OR the wrong master key is in use, OR the
    /// AAD mismatch suggests a row-swap attack.
    #[error("decryption failed: {name} v{version}")]
    DecryptionFailed { name: String, version: u32 },
    /// Catch-all for unexpected internal failures.
    #[error("security engine internal error: {reason}")]
    Internal { reason: String },
}

impl crate::error::AfaError for SecurityErrorV1 {
    fn kind(&self) -> AfaErrorKind {
        match self {
            // Boot-time and storage-class failures all map to
            // `Unavailable`: the engine is "temporarily down" in
            // the same way a database outage is "temporarily
            // down" — the fix is operator action (set the env
            // var, restore the file, run the migration), not a
            // client retry.
            Self::MasterKeyMissing
            | Self::MasterKeyMalformed { .. }
            | Self::StorageUnreachable { .. }
            | Self::StorageCorrupted
            | Self::SchemaVersionMismatch { .. }
            | Self::PayloadTooLarge { .. }
            | Self::NameTooLong { .. } => AfaErrorKind::Unavailable,
            // The (name, version) was not in the table.
            Self::SecretNotFound { .. } | Self::SecretRotated { .. } => AfaErrorKind::NotFound,
            // Wrong key or tampered ciphertext: this is not
            // "the server is down" — it is "you are not allowed
            // to read this." The caller should NOT be told which
            // (this is the same reason the variant is collapsed
            // into one name, not split into KeyMismatch vs
            // TamperedCiphertext).
            Self::DecryptionFailed { .. } => AfaErrorKind::Unauthorized,
            // Bugs and invariant violations.
            Self::Internal { .. } => AfaErrorKind::Internal,
        }
    }
}

// CID:security-005 - SecretSealed
// Purpose: The audit fact the engine publishes on the event bus
// when `seal` commits a new (name, version) row. Note the
// absence of a `tenant_id` or `correlation_id` field: the
// `seal` call does not take an `ExecutionContext` (the engine
// boot is the one place a secret is created, and there is no
// "request" to attribute the seal to). The `name` and `version`
// are sufficient to find the row in the store; `timestamp` is
// the wall-clock time the engine saw the commit.
// Uses: AfaEvent (so it can ride the bus), serde, chrono for
// the timestamp type.
// Used by: dashboards and observability tools subscribed to
// security events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretSealed {
    /// The human-meaningful label the caller passed to `seal`.
    pub name: String,
    /// The integer counter the engine picked for this version.
    pub version: u32,
    /// The wall-clock time the engine saw the commit.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl AfaEvent for SecretSealed {}

// CID:security-006 - SecretUnsealed
// Purpose: The audit fact the engine publishes on the event bus
// when `unseal` returns a plaintext handle. Carries the full
// `ExecutionContext` metadata (tenant, correlation, actor) so
// the audit trail can be tied back to the request that asked
// for the secret. Does NOT carry any field that could carry the
// plaintext itself — the field set is metadata only, per the
// "audit events publish metadata, never secrets" rule.
// Uses: AfaEvent, serde, chrono, ExecutionContext types
// (TenantId, CorrelationId, Actor).
// Used by: dashboards, anomaly detectors, and any compliance
// tool subscribed to security events.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretUnsealed {
    /// The human-meaningful label the caller passed to `seal`.
    pub name: String,
    /// The integer counter the engine picked for this version.
    pub version: u32,
    /// The tenant from the `ExecutionContext` passed to `unseal`.
    pub tenant_id: crate::ids::TenantId,
    /// The tracking number from the `ExecutionContext`.
    pub correlation_id: CorrelationId,
    /// The actor from the `ExecutionContext` (the `Actor` enum
    /// — channel/timer/human/internal — not the full context).
    pub actor: crate::execution_context::Actor,
    /// The wall-clock time the engine saw the `unseal` call.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl AfaEvent for SecretUnsealed {}

// CID:security-007 - SecretRotated
// Purpose: The audit fact the engine publishes on the event bus
// when `rotate` swaps an old version for a new one. Carries the
// full `ExecutionContext` metadata plus the old and new version
// numbers, so a compliance tool can answer "who replaced secret
// X v3, and when, and from which request?"
// Uses: AfaEvent, serde, chrono, ExecutionContext types.
// Used by: dashboards, anomaly detectors, and compliance tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecretRotated {
    /// The human-meaningful label the caller passed to `seal`.
    pub name: String,
    /// The version number the old `SecretRef` referred to.
    pub old_version: u32,
    /// The version number the new `SecretRef` refers to.
    pub new_version: u32,
    /// The tenant from the `ExecutionContext` passed to `rotate`.
    pub tenant_id: crate::ids::TenantId,
    /// The tracking number from the `ExecutionContext`.
    pub correlation_id: CorrelationId,
    /// The actor from the `ExecutionContext` (the `Actor` enum,
    /// not the full context).
    pub actor: crate::execution_context::Actor,
    /// The wall-clock time the engine saw the `rotate` call.
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl AfaEvent for SecretRotated {}

/// Compile-fail check: `UnsealedSecret` does NOT implement
/// `Clone`. This is the regression-proof for the
/// "one zeroing-on-drop handle per seal" rule. If a future
/// change re-adds `Clone`, the doctest below stops compiling
/// and `cargo test` fails.
///
/// ```compile_fail
/// use afa_contracts::UnsealedSecret;
/// let handle = UnsealedSecret::new(vec![1, 2, 3]);
/// let _copy = handle.clone();
/// ```
#[allow(dead_code)]
fn _unsealed_secret_does_not_implement_clone() {}

/// Compile-fail check: `UnsealedSecret` does NOT implement
/// `Display`. This is the regression-proof for the
/// "no `format!("{}", ...)` plaintext leak" rule.
///
/// ```compile_fail
/// use afa_contracts::UnsealedSecret;
/// let handle = UnsealedSecret::new(vec![1, 2, 3]);
/// let _s = format!("{}", handle);
/// ```
#[allow(dead_code)]
fn _unsealed_secret_does_not_implement_display() {}

/// Compile-fail check: `UnsealedSecret` does NOT implement
/// `Debug`. This is the regression-proof for the
/// "no `format!("{:?}", ...)` plaintext leak" rule — the
/// most common accidental leak path (`tracing::error!(?secret)`,
/// a panic message, a `dbg!` macro call).
///
/// ```compile_fail
/// use afa_contracts::UnsealedSecret;
/// let handle = UnsealedSecret::new(vec![1, 2, 3]);
/// let _s = format!("{:?}", handle);
/// ```
#[allow(dead_code)]
fn _unsealed_secret_does_not_implement_debug() {}
