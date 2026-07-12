//! Code Map: The desk clerk
//! - `SecurityEngine`: The desk clerk. Owns the master key
//!   (in a `Zeroizing<[u8; 32]>` so it is wiped on drop), the
//!   `SealedSecretStore` (the vault), and (in Phase 2) the
//!   `EventBus` (the audit log). Implements the locked
//!   `SecurityV1` trait. See the `impl SecurityV1 for
//!   SecurityEngine` block below.
//! - `seal`: Validate the size caps, compute the next version
//!   number inside a `BEGIN IMMEDIATE` transaction (so two
//!   `seal` calls cannot pick the same version), encrypt
//!   under the master key with `format!("{}:{}", name,
//!   version)` as the AEAD AAD, insert the row, (Phase 2:
//!   publish `SecretSealed` on the bus), return the
//!   `SecretRef`.
//! - `unseal`: Look up the row, decrypt under the master key
//!   with the same AAD, wrap the plaintext in an
//!   `UnsealedSecret` (so it is wiped on drop), (Phase 2:
//!   publish `SecretUnsealed` on the bus), return the
//!   handle.
//!
//! Story (plain English): The clerk sits at the desk with a
//! copy of the master key (in a sealed envelope so the key
//! itself is wiped if the envelope is dropped) and a list of
//! every box that has ever been filed (the SQLite file).
//! When a caller hands the clerk a new sheet of paper and
//! says "file this under 'openai-api-key'", the clerk:
//!
//!   1. checks the paper is not too big and the label is not
//!      too long (DoS protection — the same rule every
//!      bank-vault clerk enforces),
//!   2. looks at the index card file to find the next box
//!      number for that label (1 if this is the first time,
//!      2 if it was filed once before and rotated, etc.),
//!   3. seals the paper in a tamper-evident envelope, writing
//!      the label and the box number on the outside in pen
//!      so a row-swap attack cannot succeed,
//!   4. files the new card in the index,
//!   5. hands the caller a receipt (a `SecretRef`) that says
//!      "openai-api-key, box 1".
//!
//! When the same caller comes back later with the receipt
//! and says "give me box 1 for one second", the clerk:
//!
//!   1. looks up the card for `("openai-api-key", 1)`,
//!   2. opens the sealed envelope,
//!   3. hands the caller the paper in a special tray
//!      (`UnsealedSecret`) that shreds the paper the moment
//!      the caller lets go,
//!   4. stamps the audit log (Phase 2).
//!
//! The clerk never copies the paper to a notebook, never
//! pastes it into a chat message, never reads it aloud.
//! The whole point of the desk is to keep the paper
//! invisible to everyone but the caller, for as short a
//! time as possible.
//!
//! CID Index:
//! CID:afa-security-engine-001 -> SecurityEngine struct
//! CID:afa-security-engine-002 -> seal
//! CID:afa-security-engine-003 -> unseal
//! CID:afa-security-engine-004 -> rotate (Phase 2 stub)
//!
//! Quick lookup: rg -n "CID:afa-security-engine-" crates/afa-security/src/engine.rs

use crate::crypto;
use crate::storage::SealedSecretStore;
use crate::SecurityError;
use afa_contracts::{ExecutionContext, SecretRef, SecurityV1, UnsealedSecret};
use async_trait::async_trait;
use std::sync::Arc;
use zeroize::Zeroizing;

/// The plaintext payload size cap, in bytes. Larger payloads
/// are rejected with `SecurityError::PayloadTooLarge` (DoS
/// protection — the engine never wants to encrypt a 1 GiB
/// blob).
pub const PAYLOAD_CAP: usize = 64 * 1024;
/// The secret name length cap, in bytes. Longer names are
/// rejected with `SecurityError::NameTooLong`.
pub const NAME_CAP: usize = 256;

// CID:afa-security-engine-001 - SecurityEngine struct
// Purpose: The desk clerk. Cheap to clone (`Arc` on the store;
// the key is `Clone` via its inner `[u8; 32]` and is wrapped
// in a `Zeroizing` so the clone does not double the
// plaintext-key exposure surface beyond the kernel's
// process scope). Phase 2 adds an `event_bus: Arc<EventBus>`
// field; Phase 1 omits it (no events are published yet).
// Used by: the kernel (constructs one in `Kernel::new`),
// downstream adapters (hold an `Arc<dyn SecurityV1>` from
// the kernel).
#[derive(Clone)]
pub struct SecurityEngine {
    key: Arc<Zeroizing<[u8; 32]>>,
    store: SealedSecretStore,
}

impl SecurityEngine {
    /// Construct a new `SecurityEngine`. Caller supplies the
    /// master key (already hex-decoded and wrapped in
    /// `Zeroizing`) and a `SealedSecretStore` (already opened
    /// or created). The kernel's `Kernel::new` is the only
    /// caller in the v1 codebase; downstream adapters receive
    /// an `Arc<dyn SecurityV1>` and never call this directly.
    pub fn new(key: Zeroizing<[u8; 32]>, store: SealedSecretStore) -> Self {
        Self {
            key: Arc::new(key),
            store,
        }
    }
}

// CID:afa-security-engine-002 - seal
// Purpose: Validate caps, compute the next version number
// inside a `BEGIN IMMEDIATE` transaction (so two `seal`
// calls cannot pick the same version), encrypt under the
// master key with `format!("{}:{}", name, version)` as the
// AEAD AAD, insert the row, return the `SecretRef`. No
// `SecretSealed` event is published yet (Phase 2 adds it).
// Used by: the deployment's one-time setup routine, then
// every future "add a new secret" command from the CLI.
// Errors: `PayloadTooLarge`, `NameTooLong`, `Internal` on
// the version computation race (the `BEGIN IMMEDIATE` makes
// this unreachable in practice).
#[async_trait]
impl SecurityV1 for SecurityEngine {
    async fn seal(&self, plaintext: &[u8], name: &str) -> Result<SecretRef, SecurityError> {
        // Validate caps first. We do this BEFORE touching
        // the store so a bad-input caller cannot race for a
        // `BEGIN IMMEDIATE` lock against a good caller.
        if plaintext.len() > PAYLOAD_CAP {
            return Err(SecurityError::PayloadTooLarge {
                size: plaintext.len(),
                cap: PAYLOAD_CAP,
            });
        }
        if name.len() > NAME_CAP {
            return Err(SecurityError::NameTooLong {
                length: name.len(),
                cap: NAME_CAP,
            });
        }

        // Compute the next version number inside a write
        // transaction. The `BEGIN IMMEDIATE` prevents two
        // parallel `seal` calls from reading the same
        // `MAX(version)` and picking the same new number.
        let conn = self.store.conn().lock().await;
        let tx = conn
            .unchecked_transaction()
            .map_err(|_| SecurityError::StorageCorrupted)?;
        tx.execute_batch("BEGIN IMMEDIATE")
            .map_err(|_| SecurityError::StorageCorrupted)?;
        let next_version: u32 = tx
            .query_row(
                "SELECT COALESCE(MAX(version), 0) + 1 FROM sealed_secrets WHERE name = ?1",
                rusqlite::params![name],
                |row| row.get(0),
            )
            .map_err(|_| SecurityError::StorageCorrupted)?;

        // Encrypt under the master key with the AAD bound
        // to (name, version). A row-swap attack (replacing
        // one secret's ciphertext with another's) cannot
        // succeed because the AAD changes.
        let aad = format!("{}:{}", name, next_version);
        let (ciphertext, nonce) = crypto::seal(plaintext, &self.key, &aad)?;

        // Insert the row inside the same transaction.
        let timestamp = chrono::Utc::now();
        tx.execute(
            "INSERT INTO sealed_secrets \
             (name, version, status, nonce, ciphertext, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            rusqlite::params![
                name,
                next_version,
                crate::storage::STATUS_ACTIVE,
                &nonce[..],
                &ciphertext,
                timestamp.to_rfc3339(),
            ],
        )
        .map_err(|_| SecurityError::StorageCorrupted)?;
        tx.execute_batch("COMMIT")
            .map_err(|_| SecurityError::StorageCorrupted)?;
        // `conn` and `tx` are dropped at end of scope,
        // releasing the `Mutex` lock and the
        // `Transaction`'s borrow on the connection.
        Ok(SecretRef {
            name: name.to_string(),
            version: next_version,
        })
    }

    // CID:afa-security-engine-003 - unseal
    // Purpose: Look up the `(name, version)` row, decrypt
    // under the master key with the same AAD, wrap the
    // plaintext in an `UnsealedSecret` so it is wiped on
    // drop, return the handle. No `SecretUnsealed` event is
    // published yet (Phase 2 adds it).
    // Used by: every adapter that needs a secret for one
    // operation (an HTTPS request, a one-shot signature).
    // Errors: `SecretNotFound` on no row, `DecryptionFailed`
    // on AEAD tag mismatch, `StorageCorrupted` on SQL
    // failures.
    async fn unseal(
        &self,
        secret_ref: &SecretRef,
        _ctx: &ExecutionContext,
    ) -> Result<UnsealedSecret, SecurityError> {
        // Read the row. Phase 1 only checks for the active
        // row; Phase 2's `get_any` adds the
        // `SecretRotated` distinction.
        let (ciphertext, nonce) = self
            .store
            .get_active(&secret_ref.name, secret_ref.version)
            .await?
            .ok_or_else(|| SecurityError::SecretNotFound {
                name: secret_ref.name.clone(),
                version: secret_ref.version,
            })?;

        // Decrypt with the same AAD. A wrong key, a tampered
        // ciphertext, or a row-swap attack all surface as
        // `DecryptionFailed` (the three cases are
        // indistinguishable by design — the operator does
        // not need to know which one happened, and the
        // caller should not be told).
        let aad = format!("{}:{}", secret_ref.name, secret_ref.version);
        let plaintext = crypto::open(&ciphertext, &nonce, &self.key, &aad).map_err(|_| {
            SecurityError::DecryptionFailed {
                name: secret_ref.name.clone(),
                version: secret_ref.version,
            }
        })?;

        Ok(UnsealedSecret::new(plaintext.to_vec()))
    }

    // CID:afa-security-engine-004 - rotate (Phase 2 stub)
    // Purpose: Phase 1 returns `Internal` so the trait
    // is implementable. Phase 2 replaces the body with
    // the real `BEGIN IMMEDIATE` (mark old row rotated +
    // insert new row) and the `SecretRotated` event
    // publish.
    async fn rotate(
        &self,
        _secret_ref: &SecretRef,
        _new_plaintext: &[u8],
        _ctx: &ExecutionContext,
    ) -> Result<SecretRef, SecurityError> {
        Err(SecurityError::Internal {
            reason: "rotate is not implemented in Phase 1; lands in Phase 2 of the IMPL"
                .to_string(),
        })
    }
}
