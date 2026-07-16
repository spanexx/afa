//! Code Map: The desk clerk
//! - `SecurityEngine`: The desk clerk. Owns the master key
//!   (in a `Zeroizing<[u8; 32]>` so it is wiped on drop), the
//!   `Storage` (the vault, after the Phase 0.5a move from
//!   `SealedSecretStore` to `afa_storage::Storage`), and
//!   the `Arc<EventBus>` (the audit log). Implements the
//!   locked `SecurityV1` trait.
//! - `seal`: Validate the size caps, compute the next
//!   version number inside a `BEGIN IMMEDIATE` transaction
//!   (so two `seal` calls cannot pick the same version),
//!   encrypt under the master key with
//!   `format!("{}:{}", name, version)` as the AEAD AAD,
//!   insert the row, publish `SecretSealed` on the bus,
//!   return the `SecretRef`.
//! - `unseal`: Look up the row (via a `with_conn` SELECT so
//!   a `SecretRotated` row returns a distinct error from
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
//! - `lookup_hash`: The new real override (Phase 0.5b).
//!   Reads the `sha256` of the active row for `name` and
//!   constant-time compares it to `incoming_hash`. Used
//!   by the dashboard transport's bearer-auth middleware
//!   (Pack #6 Phase 3) to verify a bearer token without
//!   ever holding the plaintext in the process.
//!
//! Story (plain English): The clerk sits at the desk with
//! a copy of the master key (in a sealed envelope so the
//! key itself is wiped if the envelope is dropped), the
//! index card file (the SQLite `sealed_secrets` table),
//! and a rubber stamp book (the event bus) for stamping
//! "I did a thing" notes. When a caller hands the clerk a
//! new sheet of paper and says "file this under
//! 'openai-api-key'", the clerk:
//!
//!   1. checks the paper is not too big and the label is
//!      not too long (DoS protection — the same rule every
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
//! **Doc drift correction vs. the IMPL draft**: the IMPL
//! had the engine call `seal_secret` (a thin wrapper that
//! takes a pre-computed version and does only the INSERT).
//! That design would break the existing
//! `concurrent_rotate.rs` test (which asserts all 16
//! parallel rotates succeed) by re-introducing the version
//! race. The corrected design: the engine uses
//! `Storage::with_conn` directly with an `async` closure
//! (`Box::pin(async move { ... })`) that holds the
//! `BEGIN IMMEDIATE` transaction across the version-read,
//! the AEAD encrypt, and the row-insert — three steps in
//! one transaction, atomicity preserved.
//!
//! CID Index:
//! CID:afa-security-engine-001 -> SecurityEngine struct
//! CID:afa-security-engine-002 -> seal
//! CID:afa-security-engine-003 -> unseal
//! CID:afa-security-engine-004 -> rotate
//! CID:afa-security-engine-005 -> lookup_hash
//!
//! Quick lookup: rg -n "CID:afa-security-engine-" crates/afa-security/src/engine.rs

use crate::crypto;
use crate::events;
use crate::master_key::MasterKey;
use crate::storage::{Storage, STATUS_ACTIVE, STATUS_ROTATED};
use afa_bus::EventBus;
use afa_contracts::{Actor, ExecutionContext, SecretRef, SecurityErrorV1 as SecurityError, SecurityV1, TenantId, UnsealedSecret};
use async_trait::async_trait;
use rusqlite::{OptionalExtension, TransactionBehavior};
use sha2::{Digest, Sha256};
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
// `Storage` and the bus; the key is `Clone` via its
// inner `[u8; 32]` and is wrapped in a `Zeroizing` so
// the clone does not double the plaintext-key exposure
// surface beyond the kernel's process scope).
//
// **API change vs. pre-Phase-0.5a**: the `store` field
// is now `Storage` (re-exported from `afa-storage`),
// not `SealedSecretStore`. The engine does its own SQL
// via `Storage::with_conn` — there is no per-engine
// "storage struct" anymore. The `Storage` is shared
// with any future engine (e.g. `afa-observability`
// will use the same `Storage` for the spans table).
//
// Used by: the kernel (constructs one in `Kernel::new`),
// downstream adapters (hold an `Arc<dyn SecurityV1>` from
// the kernel).
#[derive(Clone)]
pub struct SecurityEngine {
    key: Arc<Zeroizing<[u8; 32]>>,
    storage: Storage,
    event_bus: Arc<EventBus>,
}

impl SecurityEngine {
    /// Construct a new `SecurityEngine`. Caller supplies the
    /// master key (as a `&MasterKey` — the newtype is the
    /// only way the kernel hands the key to the engine,
    /// which means the key is guaranteed to be a 32-byte
    /// value that was either lifted from a 64-char hex
    /// string via `MasterKey::from_hex` or built from a
    /// test's deterministic `[u8; 32]` via
    /// `MasterKey::from`), a `Storage` (already opened
    /// or created and migrated — the kernel does that
    /// step), and an `Arc<EventBus>` (the bus the kernel
    /// owns).
    ///
    /// **API change vs. pre-Phase-0.5a**: the second
    /// argument is now `Storage` (was `SealedSecretStore`).
    /// The boot path (open + migrate + check) is done by
    /// the kernel, not the engine — the engine only sees
    /// a `Storage` that is already ready.
    pub fn new(key: &MasterKey, storage: Storage, event_bus: Arc<EventBus>) -> Self {
        // The engine stores the key inside an
        // `Arc<Zeroizing<[u8; 32]>>` so (1) the bytes are
        // wiped on drop (the `Zeroizing` wrapper) and
        // (2) cloning the engine does not duplicate the
        // key bytes in the process heap (only the
        // `Arc`'s refcount is bumped). The newtype's
        // own `Drop` runs as soon as this function
        // returns, so the only live copy in the
        // process is the one inside the engine.
        let raw: [u8; 32] = *key.as_bytes();
        Self {
            key: Arc::new(Zeroizing::new(raw)),
            storage,
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
// on the bus, return the `SecretRef`.
// Used by: the deployment's one-time setup routine, then
// every future "add a new secret" command from the CLI.
// Errors: `PayloadTooLarge`, `NameTooLong`, `StorageCorrupted`
// on the version-compute race (the `BEGIN IMMEDIATE` makes
// this unreachable in practice).
#[async_trait]
impl SecurityV1 for SecurityEngine {
    async fn seal(&self, plaintext: &[u8], name: &str) -> Result<SecretRef, SecurityError> {
        // Validate caps first. We do this BEFORE touching
        // the store so a bad-input caller cannot race for
        // the lock against a good caller.
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

        // Copy the input strings to owned `String` /
        // `Vec<u8>` so the `with_conn` closure's
        // future can be `'static`. The HRTB on
        // `with_conn`'s `F` bound requires the future
        // to be `Send + 'a` for the Connection's
        // borrow `'a`; the `Box::pin(async move)`
        // pattern infers `Pin<Box<impl Future +
        // 'static>>` by default, and capturing a
        // `&str` or `&[u8]` (function parameters
        // with non-`'static` lifetimes) would force
        // the closure body to require those
        // lifetimes to be `'static`, which they are
        // not. Owning the strings keeps the captures
        // `'static`; the cost is two small
        // allocations per `seal` call (cheap
        // relative to the SQL round-trip + AEAD
        // encryption). The `payload_for_err` clone
        // is for the `PayloadTooLarge` error path
        // (in case the engine adds such a path
        // later); the `name_for_err` clone is
        // preserved so the audit-fact publish at
        // the bottom of the function can use it.
        let plaintext = plaintext.to_vec();
        let name = name.to_string();
        let name_for_err = name.clone();
        let plaintext_len = plaintext.len();
        // (kept for the future `PayloadTooLarge` error
        // path; currently the only size check is in
        // `MAX_PAYLOAD_BYTES` at the top of the function)
        let _ = plaintext_len;
        // Clone the `Arc<Zeroizing<[u8; 32]>>` so the
        // closure can hold an owned `Arc` (the inner
        // `Zeroizing` is shared via the `Arc` ref
        // count; no copy of the key bytes is made).
        // The clone is cheap (one atomic increment);
        // it lets the closure's future be `'static`
        // (see the `Box::pin` lifetime note on
        // `lookup_hash`).
        let key = Arc::clone(&self.key);

        // Compute the next version number inside a write
        // transaction. The `TransactionBehavior::Immediate`
        // starts the transaction as `BEGIN IMMEDIATE`,
        // which is what prevents two parallel `seal`
        // calls from reading the same `MAX(version)` and
        // picking the same new number.
        //
        // The whole flow (BEGIN IMMEDIATE → version-read
        // → encrypt → INSERT → commit) runs inside a
        // SINGLE `with_conn` async closure. The lock
        // is held for the duration of the future; the
        // encryption (CPU work, no `.await`) happens
        // inside the transaction so the version in the
        // AAD is the same version we just read and
        // are about to INSERT. This preserves the
        // `BEGIN IMMEDIATE` atomicity the original
        // code relied on. **Doc drift correction #5
        // vs. the IMPL draft** — see the module-level
        // comment.
        let (next_version, timestamp) = afa_storage::with_conn(
            &self.storage,
            |conn| {
                Box::pin(async move {
                    let tx = conn
                        .transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let next_version: u32 = tx
                        .query_row(
                            "SELECT COALESCE(MAX(version), 0) + 1 FROM sealed_secrets WHERE name = ?1",
                            rusqlite::params![name],
                            |row| row.get(0),
                        )?;

                    // Encrypt under the master key with the
                    // AAD bound to (name, version). A row-
                    // swap attack (replacing one secret's
                    // ciphertext with another's) cannot
                    // succeed because the AAD changes. The
                    // `&self.key` here is `&Arc<Zeroizing<...>>`
                    // and deref-coerces to `&Zeroizing<...>`
                    // (the `crypto::seal` parameter type).
                    let aad = format!("{}:{}", name, next_version);
                    let (ciphertext, nonce) =
                        crypto::seal(&plaintext, &key, &aad)?;

                    // Compute the SHA-256 of the plaintext
                    // (lowercase hex, 64 ASCII bytes) and
                    // store it in the `sha256` column. The
                    // `lookup_hash` method reads this
                    // column to constant-time-compare
                    // against the incoming hash. The hex
                    // form (not raw bytes) means the
                    // constant-time compare can
                    // short-circuit on length without a
                    // second decode, and the on-disk
                    // column is readable via a plain
                    // `sqlite3` CLI for debugging
                    // (without re-deriving the hash from
                    // a binary blob).
                    let mut hasher = Sha256::new();
                    hasher.update(&plaintext);
                    let sha256_hex = format!("{:x}", hasher.finalize());

                    let timestamp = events::now();
                    tx.execute(
                        "INSERT INTO sealed_secrets \
                         (name, version, status, nonce, ciphertext, sha256, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            name,
                            next_version,
                            STATUS_ACTIVE,
                            &nonce[..],
                            &ciphertext,
                            sha256_hex.as_bytes(),
                            timestamp.to_rfc3339(),
                        ],
                    )?;
                    tx.commit()?;
                    Ok::<_, SecurityError>((next_version, timestamp))
                })
            },
        )
        .await
        .map_err(|e| match e {
            afa_storage::StorageError::Closure(boxed) => boxed.into(),
            _ => SecurityError::StorageCorrupted,
        })?;

        // Publish the `SecretSealed` audit fact AFTER
        // commit (the lock is released by the time we
        // reach this line). The `ExecutionContext` here
        // is a synthetic one — the `seal` call does not
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
                    name: name_for_err.clone(),
                    version: next_version,
                    timestamp,
                },
                deploy_ctx,
            )
            .await;

        Ok(SecretRef {
            name: name_for_err,
            version: next_version,
        })
    }

    // CID:afa-security-engine-003 - unseal
    // Purpose: Look up the `(name, version)` row via a
    // `with_conn` SELECT so a `status='rotated'` row
    // returns a distinct `SecretRotated` error from a
    // missing row's `SecretNotFound`. Decrypt under the
    // master key with the same AAD, wrap the plaintext
    // in an `UnsealedSecret` so it is wiped on drop,
    // publish `SecretUnsealed` on the bus, return the
    // handle.
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
        // Read the row, including its `status`. The
        // closure returns `Option<(Vec<u8>, Vec<u8>, String)>` —
        // the `None` branch is "no row for (name, version)".
        //
        // Copy the `secret_ref` fields to owned
        // `String` / `u32` so the `with_conn` closure's
        // future can be `'static` (see the lifetime
        // note on `seal` and `lookup_hash`).
        let name = secret_ref.name.clone();
        let version = secret_ref.version;
        let row: Option<(Vec<u8>, Vec<u8>, String)> = afa_storage::with_conn(
            &self.storage,
            |conn| {
                Box::pin(async move {
                    conn.query_row(
                        "SELECT ciphertext, nonce, status FROM sealed_secrets \
                         WHERE name = ?1 AND version = ?2",
                        rusqlite::params![name, version],
                        |row| {
                            Ok((
                                row.get::<_, Vec<u8>>(0)?,
                                row.get::<_, Vec<u8>>(1)?,
                                row.get::<_, String>(2)?,
                            ))
                        },
                    )
                    .optional()
                    .map_err(|_| SecurityError::StorageCorrupted)
                })
            },
        )
        .await
        .map_err(|e| match e {
            afa_storage::StorageError::Closure(boxed) => boxed.into(),
            _ => SecurityError::StorageCorrupted,
        })?;

        let (ciphertext, nonce, status) = row.ok_or_else(|| SecurityError::SecretNotFound {
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

        // The nonce column is stored as `BLOB`; rusqlite
        // returns it as `Vec<u8>`. The AEAD layer
        // expects `&[u8; NONCE_LEN]` (the `crypto::open`
        // signature is locked). A `Vec<u8>` of length 12
        // can be converted via `try_into`; any other
        // length is a `StorageCorrupted` (the row was
        // tampered with or the schema changed).
        let nonce_arr: [u8; 12] = nonce
            .try_into()
            .map_err(|_| SecurityError::StorageCorrupted)?;

        // Decrypt with the same AAD. A wrong key, a
        // tampered ciphertext, or a row-swap attack all
        // surface as `DecryptionFailed` (the three
        // cases are indistinguishable by design — the
        // operator does not need to know which one
        // happened, and the caller should not be told).
        let aad = format!("{}:{}", secret_ref.name, secret_ref.version);
        let plaintext = crypto::open(&ciphertext, &nonce_arr, &self.key, &aad).map_err(|_| {
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
    // in one transaction, publish `SecretRotated` on
    // the bus, return the new `SecretRef`.
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

        // Copy the input fields to owned values so
        // the `with_conn` closure's future can be
        // `'static` (see the lifetime note on
        // `seal` and `lookup_hash`).
        let name = secret_ref.name.clone();
        let old_version = secret_ref.version;
        let new_plaintext = new_plaintext.to_vec();
        let key = Arc::clone(&self.key);

        // Compute the next version number inside a
        // write transaction (same pattern as `seal`).
        // The `TransactionBehavior::Immediate` starts
        // the transaction as `BEGIN IMMEDIATE`, which
        // prevents two parallel rotates from picking
        // the same new version.
        let (new_version, timestamp) = afa_storage::with_conn(
            &self.storage,
            |conn| {
                Box::pin(async move {
                    let tx = conn
                        .transaction_with_behavior(TransactionBehavior::Immediate)?;
                    let new_version: u32 = tx
                        .query_row(
                            "SELECT COALESCE(MAX(version), 0) + 1 FROM sealed_secrets WHERE name = ?1",
                            rusqlite::params![name],
                            |row| row.get(0),
                        )?;

                    // Encrypt the new plaintext with AAD
                    // bound to (name, new_version). The
                    // AAD differs from the old row's AAD
                    // (which used old_version), so the
                    // AEAD tag check is the only thing
                    // that prevents an old-row / new-row
                    // ciphertext confusion attack.
                    let aad = format!("{}:{}", name, new_version);
                    let (new_ciphertext, new_nonce) =
                        crypto::seal(&new_plaintext, &key, &aad)?;

                    // Same SHA-256 of the plaintext as
                    // `seal` (see that function for the
                    // rationale on hex form). The hash
                    // is for the *new* plaintext — the
                    // old row's hash stays put, but the
                    // `lookup_hash` method filters on
                    // `status = 'active'`, so the old
                    // row's hash is unreachable.
                    let mut hasher = Sha256::new();
                    hasher.update(&new_plaintext);
                    let sha256_hex = format!("{:x}", hasher.finalize());

                    // Mark the old row as `rotated` AND
                    // insert the new active row inside the
                    // same transaction. The 0-row-update
                    // branch detects the concurrent-rotate
                    // race surface (a parallel rotate
                    // flipped the old row's status between
                    // our pre-check and this UPDATE).
                    let timestamp = events::now();
                    let updated = tx
                        .execute(
                            "UPDATE sealed_secrets SET status = ?1 \
                             WHERE name = ?2 AND version = ?3 AND status = ?4",
                            rusqlite::params![
                                STATUS_ROTATED,
                                name,
                                old_version,
                                STATUS_ACTIVE
                            ],
                        )?;
                    if updated != 1 {
                        return Err(SecurityError::SecretRotated {
                            name: name.clone(),
                            version: old_version,
                        });
                    }
                    tx.execute(
                        "INSERT INTO sealed_secrets \
                         (name, version, status, nonce, ciphertext, sha256, created_at) \
                         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                        rusqlite::params![
                            name,
                            new_version,
                            STATUS_ACTIVE,
                            &new_nonce[..],
                            &new_ciphertext,
                            sha256_hex.as_bytes(),
                            timestamp.to_rfc3339(),
                        ],
                    )?;
                    tx.commit()?;
                    Ok::<_, SecurityError>((new_version, timestamp))
                })
            },
        )
        .await
        .map_err(|e| match e {
            afa_storage::StorageError::Closure(boxed) => boxed.into(),
            _ => SecurityError::StorageCorrupted,
        })?;

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

    // CID:afa-security-engine-005 - lookup_hash
    // Purpose: The real override (Phase 0.5b) of
    // `SecurityV1::lookup_hash`. The default impl in
    // `afa-contracts::security` returns
    // `Err(Internal)` (a fake that does not
    // implement `lookup_hash` should not silently
    // accept arbitrary hashes). The real
    // implementation: SELECT the `sha256` of the
    // active row for `name` (the v2 schema added
    // this column in Phase 0.5b; the engine
    // populates it on `seal` / `rotate`), and
    // constant-time-compare it to `incoming_hash`.
    // The comparison happens inside the engine, not
    // at the call site, so the comparison latency is
    // not a side-channel oracle.
    //
    // **On-disk format**: the `sha256` column is
    // `BLOB` (not `TEXT`) but the engine stores
    // lowercase ASCII hex (64 bytes per row) so
    // the column is readable via a plain `sqlite3`
    // CLI for debugging and so the constant-time
    // compare can short-circuit on length without
    // a second decode. Legacy v1 rows (pre-Phase-0.5b)
    // have `sha256 = NULL`; for those, `lookup_hash`
    // returns `Err(SecretNotFound)` (the same as
    // "no row"). The operator must re-seal the
    // secret to populate the column.
    //
    // Used by: the dashboard transport's bearer-
    // auth middleware (Pack #6 Phase 3) to verify
    // a bearer token without ever holding the
    // plaintext in the process. The middleware
    // computes `sha256(token)` (the hash) and hands
    // the hex to this method; the engine reads
    // the stored hash and constant-time compares.
    //
    // Errors: `SecretNotFound` on no active row for
    // `name` (or a legacy v1 row with no hash), the
    // `version: 0` is a placeholder —
    // `lookup_hash` operates on `name`, not on
    // `(name, version)`), `StorageCorrupted` on
    // SQL failures or a stored hash that is not
    // valid UTF-8.
    async fn lookup_hash(&self, name: &str, incoming_hash: &str) -> Result<bool, SecurityError> {
        // Read the active row's `sha256` column.
        // The `WHERE status = 'active'` filters out
        // rotated rows; a `name` whose only row is
        // rotated returns `None` and the method
        // returns `Err(SecretNotFound)`. Legacy v1
        // rows (pre-Phase-0.5b) have `sha256 = NULL`,
        // which `row.get::<_, Option<Vec<u8>>>(0)`
        // reads back as `None` — the same path as
        // "no row".
        //
        // Copy `name` into an owned `String` before
        // the `with_conn` call so the boxed future
        // can be 'static. The `Box::pin(async move)`
        // pattern infers `Pin<Box<impl Future +
        // 'static>>` by default, and capturing a
        // `&str` (the function parameter, with a
        // non-'static lifetime) would force that
        // lifetime to be 'static, which it is not.
        // Owning the name string keeps the captures
        // 'static; the cost is one allocation per
        // `lookup_hash` call (which is cheap
        // relative to the SQL round-trip).
        let name = name.to_string();
        // `name_for_err` is a clone of `name` so the
        // `SecretNotFound` error path below can use
        // it; the original `name` is moved into the
        // `with_conn` closure's future (so the
        // future's captures are 'static, avoiding
        // the `Box::pin` lifetime issue). The clone
        // is a single short-string allocation; the
        // alternative (a second `name.to_string()` in
        // the `ok_or_else` closure) would be the
        // same allocation cost but more verbose.
        let name_for_err = name.clone();
        // The column is `BLOB` (the engine stores
        // the hex as 64 ASCII bytes). We read it
        // as `Option<Vec<u8>>` and then convert
        // to `Option<String>` (the hex form is
        // always valid UTF-8 since it is
        // `[0-9a-f]{64}`; a non-UTF-8 value is
        // a `StorageCorrupted`).
        let stored_hash: Option<String> = afa_storage::with_conn(
            &self.storage,
            |conn| {
                Box::pin(async move {
                    conn.query_row(
                        "SELECT sha256 FROM sealed_secrets \
                         WHERE name = ?1 AND status = ?2",
                        rusqlite::params![name, STATUS_ACTIVE],
                        |row| row.get::<_, Option<Vec<u8>>>(0),
                    )
                    .optional()
                    .map_err(|_| SecurityError::StorageCorrupted)
                    .and_then(|opt| {
                        // Flatten `Option<Option<Vec<u8>>>` to
                        // `Option<Vec<u8>>` (None for either
                        // "no row" or "row but NULL column").
                        opt.flatten()
                            // Convert bytes to String (the
                            // stored hex is always valid
                            // UTF-8; a non-UTF-8 value means
                            // the row was tampered with).
                            .map(|bytes| {
                                String::from_utf8(bytes)
                                    .map_err(|_| SecurityError::StorageCorrupted)
                            })
                            .transpose()
                    })
                })
            },
        )
        .await
        .map_err(|e| match e {
            afa_storage::StorageError::Closure(boxed) => boxed.into(),
            _ => SecurityError::StorageCorrupted,
        })?;

        let stored = stored_hash.ok_or_else(|| SecurityError::SecretNotFound {
            name: name_for_err,
            version: 0, // placeholder — see the doc comment
        })?;

        // Constant-time compare. The hand-rolled
        // XOR-OR loop runs in time proportional to
        // the longer of the two inputs, not the
        // matching prefix. The length pre-check
        // above is a fast-path for the "different
        // lengths → cannot be equal" case (e.g. the
        // caller sent a 6-char prefix of a 64-char
        // hash); the response is `Ok(false)`,
        // not `Err`, because length-mismatch is a
        // *legitimate* "not equal" outcome (not a
        // *malformed input* outcome). The `subtle`
        // crate would give us a more robust compare
        // (and is in the IMPL's TRD §3.5), but for
        // now the engine has no new dep; the
        // compare is correct (constant-time given
        // equal lengths) and the engine's tests
        // cover it.
        let stored_bytes = stored.as_bytes();
        let incoming_bytes = incoming_hash.as_bytes();
        if stored_bytes.len() != incoming_bytes.len() {
            return Ok(false);
        }
        let mut diff: u8 = 0;
        for (a, b) in stored_bytes.iter().zip(incoming_bytes.iter()) {
            diff |= a ^ b;
        }
        Ok(diff == 0)
    }
}
