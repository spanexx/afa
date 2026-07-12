//! Code Map: The desk clerk
//! - `SecurityEngine`: The desk clerk. Owns the master key
//!   (in a `Zeroizing<[u8; 32]>` so it is wiped on drop), the
//!   `SealedSecretStore` (the vault), and the `Arc<EventBus>`
//!   (the audit log). Implements the locked `SecurityV1`
//!   trait. See the `impl SecurityV1 for SecurityEngine`
//!   block below.
//! - `seal`: Validate the size caps, compute the next
//!   version number inside a `BEGIN IMMEDIATE` transaction
//!   (so two `seal` calls cannot pick the same version),
//!   encrypt under the master key with
//!   `format!("{}:{}", name, version)` as the AEAD AAD,
//!   insert the row, publish `SecretSealed` on the bus,
//!   return the `SecretRef`.
//! - `unseal`: Look up the row (via `get_any` so a
//!   `SecretRotated` row returns a distinct error from
//!   a missing row), decrypt under the master key with
//!   the same AAD, wrap the plaintext in an
//!   `UnsealedSecret` (so it is wiped on drop), publish
//!   `SecretUnsealed` on the bus, return the handle.
//! - `rotate`: Look up the old row, validate it is
//!   `status='active'`, compute the next version inside
//!   a `BEGIN IMMEDIATE`, mark the old row
//!   `status='rotated'` and insert the new active row in
//!   one transaction, publish `SecretRotated` on the bus,
//!   return the new `SecretRef`.
//!
//! Story (plain English): The clerk sits at the desk with a
//! copy of the master key (in a sealed envelope so the key
//! itself is wiped if the envelope is dropped), the index
//! card file (the SQLite `sealed_secrets` table), and a
//! rubber stamp book (the event bus) for stamping
//! "I did a thing" notes. When a caller hands the clerk a
//! new sheet of paper and says "file this under
//! 'openai-api-key'", the clerk:
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
//!   5. stamps a `SecretSealed` note in the audit log,
//!   6. hands the caller a receipt (a `SecretRef`) that says
//!      "openai-api-key, box 1".
//!
//! When the same caller comes back later with the receipt
//! and says "give me box 1 for one second", the clerk:
//!
//!   1. looks up the card for `("openai-api-key", 1)`,
//!   2. if the card is missing entirely, says
//!      "SecretNotFound"; if the card exists but is stamped
//!      "rotated", says "SecretRotated"; if the card is
//!      stamped "active", continues,
//!   3. opens the sealed envelope,
//!   4. hands the caller the paper in a special tray
//!      (`UnsealedSecret`) that shreds the paper the moment
//!      the caller lets go,
//!   5. stamps a `SecretUnsealed` note in the audit log.
//!
//! When the caller says "swap the paper in box 1 for this
//! new sheet", the clerk:
//!
//!   1. looks up box 1 — refuses if it is missing or already
//!      rotated,
//!   2. computes the next box number (2 for the first
//!      rotate),
//!   3. inside one `BEGIN IMMEDIATE` transaction, flips box
//!      1's card to "rotated" AND files box 2's card as
//!      "active" (so a crash between the two writes cannot
//!      leave the system in a half-rotated state),
//!   4. stamps a `SecretRotated` note in the audit log,
//!   5. hands the caller a new receipt for box 2.
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
//! CID:afa-security-engine-004 -> rotate
//!
//! Quick lookup: rg -n "CID:afa-security-engine-" crates/afa-security/src/engine.rs

use crate::crypto;
use crate::events;
use crate::storage::{SealedSecretStore, STATUS_ACTIVE, STATUS_ROTATED};
use crate::SecurityError;
use afa_contracts::{Actor, ExecutionContext, SecretRef, SecurityV1, TenantId, UnsealedSecret};
use afa_kernel::event_bus::EventBus;
use async_trait::async_trait;
use rusqlite::TransactionBehavior;
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
// Purpose: The desk clerk. Cheap to clone (`Arc` on the
// store and the bus; the key is `Clone` via its inner
// `[u8; 32]` and is wrapped in a `Zeroizing` so the clone
// does not double the plaintext-key exposure surface
// beyond the kernel's process scope).
// Used by: the kernel (constructs one in `Kernel::new`),
// downstream adapters (hold an `Arc<dyn SecurityV1>` from
// the kernel).
#[derive(Clone)]
pub struct SecurityEngine {
    key: Arc<Zeroizing<[u8; 32]>>,
    store: SealedSecretStore,
    event_bus: Arc<EventBus>,
}

impl SecurityEngine {
    /// Construct a new `SecurityEngine`. Caller supplies the
    /// master key (already hex-decoded and wrapped in
    /// `Zeroizing`), a `SealedSecretStore` (already opened
    /// or created), and an `Arc<EventBus>` (the bus the
    /// kernel owns). The kernel's `Kernel::new` is the
    /// only caller in the v1 codebase; downstream
    /// adapters receive an `Arc<dyn SecurityV1>` and
    /// never call this directly.
    pub fn new(
        key: Zeroizing<[u8; 32]>,
        store: SealedSecretStore,
        event_bus: Arc<EventBus>,
    ) -> Self {
        Self {
            key: Arc::new(key),
            store,
            event_bus,
        }
    }
}

// CID:afa-security-engine-002 - seal
// Purpose: Validate caps, compute the next version number
// inside a `BEGIN IMMEDIATE` transaction (so two `seal`
// calls cannot pick the same version), encrypt under the
// master key with `format!("{}:{}", name, version)` as
// the AEAD AAD, insert the row, publish `SecretSealed`
// on the bus, return the `SecretRef`. The `SecretSealed`
// publish uses a synthetic `ExecutionContext` (the
// deployment-time seal has no per-request tenant /
// correlation / actor — see the contracts-side doc
// comment on the event for the rationale).
// Used by: the deployment's one-time setup routine, then
// every future "add a new secret" command from the CLI.
// Errors: `PayloadTooLarge`, `NameTooLong`, `StorageCorrupted`
// on the version-compute race (the `BEGIN IMMEDIATE` makes
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
        // transaction. The `TransactionBehavior::Immediate`
        // starts the transaction as `BEGIN IMMEDIATE`, which
        // is what prevents two parallel `seal` calls from
        // reading the same `MAX(version)` and picking the
        // same new number. (Phase 1's code used
        // `unchecked_transaction()` + `execute_batch("BEGIN
        // IMMEDIATE")`, which double-starts a transaction
        // and SQL-errors out — see IMPL §7 Drift #7.)
        // Wrapped in a block so `conn` and `tx` are
        // dropped (releasing the `Mutex` lock and the
        // `Transaction`'s borrow on the connection)
        // BEFORE the `event_bus.publish().await` below —
        // `Transaction<'a>` is not `Send` and a held
        // `tx` across an `.await` would be a compile
        // error.
        let (next_version, timestamp) = {
            let mut conn = self.store.conn().lock().await;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| SecurityError::StorageCorrupted)?;
            let next_version: u32 = tx
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) + 1 FROM sealed_secrets WHERE name = ?1",
                    rusqlite::params![name],
                    |row| row.get(0),
                )
                .map_err(|_| SecurityError::StorageCorrupted)?;

            // Encrypt under the master key with the AAD
            // bound to (name, version). A row-swap
            // attack (replacing one secret's ciphertext
            // with another's) cannot succeed because
            // the AAD changes.
            let aad = format!("{}:{}", name, next_version);
            let (ciphertext, nonce) = crypto::seal(plaintext, &self.key, &aad)?;

            // Insert the row inside the same transaction.
            let timestamp = events::now();
            tx.execute(
                "INSERT INTO sealed_secrets \
                 (name, version, status, nonce, ciphertext, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    name,
                    next_version,
                    STATUS_ACTIVE,
                    &nonce[..],
                    &ciphertext,
                    timestamp.to_rfc3339(),
                ],
            )
            .map_err(|_| SecurityError::StorageCorrupted)?;
            tx.commit().map_err(|_| SecurityError::StorageCorrupted)?;
            (next_version, timestamp)
        };

        // Publish the `SecretSealed` audit fact AFTER
        // commit. The `ExecutionContext` here is a
        // synthetic one — the `seal` call does not
        // take a per-request context (it is a
        // deployment-time operation), and the
        // `SecretSealed` event does not carry the ctx
        // fields (per the contracts-side doc comment
        // on `SecretSealed`). The ctx is just a
        // vehicle for the bus's per-event routing.
        let deploy_ctx = ExecutionContext::new(
            TenantId::new("afa-deployment"),
            Actor::Internal {
                caller: "security-engine.seal".to_string(),
            },
        );
        self.event_bus
            .publish(
                events::SecretSealed {
                    name: name.to_string(),
                    version: next_version,
                    timestamp,
                },
                deploy_ctx,
            )
            .await;

        Ok(SecretRef {
            name: name.to_string(),
            version: next_version,
        })
    }

    // CID:afa-security-engine-003 - unseal
    // Purpose: Look up the `(name, version)` row via
    // `get_any` so a `status='rotated'` row returns a
    // distinct `SecretRotated` error from a missing row's
    // `SecretNotFound`. Decrypt under the master key with
    // the same AAD, wrap the plaintext in an
    // `UnsealedSecret` so it is wiped on drop, publish
    // `SecretUnsealed` on the bus, return the handle.
    // Used by: every adapter that needs a secret for one
    // operation (an HTTPS request, a one-shot signature).
    // Errors: `SecretNotFound` on no row, `SecretRotated`
    // on a row with `status='rotated'`, `DecryptionFailed`
    // on AEAD tag mismatch, `StorageCorrupted` on SQL
    // failures.
    async fn unseal(
        &self,
        secret_ref: &SecretRef,
        ctx: &ExecutionContext,
    ) -> Result<UnsealedSecret, SecurityError> {
        // Read the row, including its `status`. This
        // is the Phase 2 switch from `get_active` (which
        // collapsed the three "row missing", "row
        // rotated", "row active" cases into one `None`)
        // to `get_any` (which returns the status so the
        // engine can distinguish them).
        let (ciphertext, nonce, status) = self
            .store
            .get_any(&secret_ref.name, secret_ref.version)
            .await?
            .ok_or_else(|| SecurityError::SecretNotFound {
                name: secret_ref.name.clone(),
                version: secret_ref.version,
            })?;

        // Distinguish `SecretRotated` from the happy
        // path. The order matters: we check status
        // BEFORE attempting decryption (a successful
        // decrypt of a rotated row would leak the old
        // plaintext, which is the leak the
        // `SecretRotated` error is here to prevent).
        if status == STATUS_ROTATED {
            return Err(SecurityError::SecretRotated {
                name: secret_ref.name.clone(),
                version: secret_ref.version,
            });
        }

        // Decrypt with the same AAD. A wrong key, a
        // tampered ciphertext, or a row-swap attack all
        // surface as `DecryptionFailed` (the three
        // cases are indistinguishable by design — the
        // operator does not need to know which one
        // happened, and the caller should not be told).
        let aad = format!("{}:{}", secret_ref.name, secret_ref.version);
        let plaintext = crypto::open(&ciphertext, &nonce, &self.key, &aad).map_err(|_| {
            SecurityError::DecryptionFailed {
                name: secret_ref.name.clone(),
                version: secret_ref.version,
            }
        })?;

        // Publish the `SecretUnsealed` audit fact AFTER
        // successful decrypt. The `ctx` parameter
        // carries the request's tenant / correlation /
        // actor, which the event's `tenant_id` /
        // `correlation_id` / `actor` fields expose to
        // compliance / observability subscribers.
        let timestamp = events::now();
        self.event_bus
            .publish(
                events::SecretUnsealed {
                    name: secret_ref.name.clone(),
                    version: secret_ref.version,
                    tenant_id: ctx.tenant_id.clone(),
                    correlation_id: ctx.correlation_id,
                    actor: ctx.actor.clone(),
                    timestamp,
                },
                ctx.clone(),
            )
            .await;

        Ok(UnsealedSecret::new(plaintext.to_vec()))
    }

    // CID:afa-security-engine-004 - rotate
    // Purpose: Validate the new plaintext cap, look up
    // the old row (refuse if missing or already
    // rotated), compute the next version inside a
    // `BEGIN IMMEDIATE`, mark the old row
    // `status='rotated'` and insert the new active row
    // in one transaction (the storage layer's `rotate`
    // does this), publish `SecretRotated` on the bus,
    // return the new `SecretRef`.
    // Used by: every adapter that wants to swap a
    // secret for a new value (e.g. an API key refresh).
    // Errors: `PayloadTooLarge`, `NameTooLong` (for the
    // *new* plaintext + the *old* name), `SecretNotFound`
    // on no row for the old `SecretRef`, `SecretRotated`
    // if the old row is already rotated (covers the
    // concurrent-rotate race — see test
    // `tests/concurrent_rotate.rs`).
    async fn rotate(
        &self,
        secret_ref: &SecretRef,
        new_plaintext: &[u8],
        ctx: &ExecutionContext,
    ) -> Result<SecretRef, SecurityError> {
        // Validate the NEW plaintext cap and the OLD
        // name cap. The new plaintext gets the same
        // cap as `seal` (DoS protection is the same on
        // both entry points). The OLD name is
        // already in the store, so its length check
        // is theoretically redundant — but it is cheap
        // and keeps the engine's input-validation
        // rules identical at every entry point.
        if new_plaintext.len() > PAYLOAD_CAP {
            return Err(SecurityError::PayloadTooLarge {
                size: new_plaintext.len(),
                cap: PAYLOAD_CAP,
            });
        }
        if secret_ref.name.len() > NAME_CAP {
            return Err(SecurityError::NameTooLong {
                length: secret_ref.name.len(),
                cap: NAME_CAP,
            });
        }

        // Verify the old row exists and is still
        // `status='active'` BEFORE we start the write
        // transaction. The check is best-effort: a
        // concurrent rotate could flip the row's
        // status between this read and the write, and
        // the write transaction's `UPDATE ... WHERE
        // status = 'active'` would then affect 0 rows
        // and the storage layer's `rotate` would return
        // `StorageCorrupted`. The test
        // `tests/concurrent_rotate.rs` is the
        // regression-proof that this race is detected
        // — the caller re-reads the latest
        // `SecretRef` and retries.
        let (_, _, status) = self
            .store
            .get_any(&secret_ref.name, secret_ref.version)
            .await?
            .ok_or_else(|| SecurityError::SecretNotFound {
                name: secret_ref.name.clone(),
                version: secret_ref.version,
            })?;
        if status == STATUS_ROTATED {
            return Err(SecurityError::SecretRotated {
                name: secret_ref.name.clone(),
                version: secret_ref.version,
            });
        }

        // Compute the next version number inside a
        // write transaction (same pattern as `seal`).
        // The `TransactionBehavior::Immediate` starts
        // the transaction as `BEGIN IMMEDIATE`, which
        // prevents two parallel rotates from picking
        // the same new version. (Phase 1's code used
        // the broken `unchecked_transaction()` +
        // `execute_batch("BEGIN IMMEDIATE")` pattern
        // — see IMPL §7 Drift #7.)
        // Wrapped in a block so `conn` and `tx` are
        // dropped BEFORE the `event_bus.publish().await`
        // below — `Transaction<'a>` is not `Send` and
        // a held `tx` across an `.await` would be a
        // compile error.
        let (new_version, timestamp) = {
            let mut conn = self.store.conn().lock().await;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| SecurityError::StorageCorrupted)?;
            let new_version: u32 = tx
                .query_row(
                    "SELECT COALESCE(MAX(version), 0) + 1 FROM sealed_secrets WHERE name = ?1",
                    rusqlite::params![secret_ref.name],
                    |row| row.get(0),
                )
                .map_err(|_| SecurityError::StorageCorrupted)?;

            // Encrypt the new plaintext with AAD bound
            // to (name, new_version). The AAD differs
            // from the old row's AAD (which used
            // old_version), so the AEAD tag check is
            // the only thing that prevents an
            // old-row / new-row ciphertext confusion
            // attack.
            let aad = format!("{}:{}", secret_ref.name, new_version);
            let (new_ciphertext, new_nonce) = crypto::seal(new_plaintext, &self.key, &aad)?;

            // Mark the old row as `rotated` AND insert
            // the new active row inside the same
            // transaction. The 0-row-update branch
            // detects the concurrent-rotate race
            // surface (a parallel rotate flipped the
            // old row's status between our pre-check
            // and this UPDATE).
            let timestamp = events::now();
            let updated = tx
                .execute(
                    "UPDATE sealed_secrets SET status = ?1 \
                     WHERE name = ?2 AND version = ?3 AND status = ?4",
                    rusqlite::params![
                        STATUS_ROTATED,
                        secret_ref.name,
                        secret_ref.version,
                        STATUS_ACTIVE
                    ],
                )
                .map_err(|_| SecurityError::StorageCorrupted)?;
            if updated != 1 {
                return Err(SecurityError::SecretRotated {
                    name: secret_ref.name.clone(),
                    version: secret_ref.version,
                });
            }
            tx.execute(
                "INSERT INTO sealed_secrets \
                 (name, version, status, nonce, ciphertext, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                rusqlite::params![
                    secret_ref.name,
                    new_version,
                    STATUS_ACTIVE,
                    &new_nonce[..],
                    &new_ciphertext,
                    timestamp.to_rfc3339(),
                ],
            )
            .map_err(|_| SecurityError::StorageCorrupted)?;
            tx.commit().map_err(|_| SecurityError::StorageCorrupted)?;
            (new_version, timestamp)
        };

        // Publish the `SecretRotated` audit fact AFTER
        // commit. The event carries the old and new
        // version numbers so a compliance tool can
        // answer "who replaced secret X v3, and when,
        // and from which request?"
        self.event_bus
            .publish(
                events::SecretRotated {
                    name: secret_ref.name.clone(),
                    old_version: secret_ref.version,
                    new_version,
                    tenant_id: ctx.tenant_id.clone(),
                    correlation_id: ctx.correlation_id,
                    actor: ctx.actor.clone(),
                    timestamp,
                },
                ctx.clone(),
            )
            .await;

        Ok(SecretRef {
            name: secret_ref.name.clone(),
            version: new_version,
        })
    }
}
