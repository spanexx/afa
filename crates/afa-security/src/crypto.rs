//! Code Map: The pure-AEAD layer
//! - `seal`: Encrypt `plaintext` under `key` with a fresh random
//!   12-byte nonce from the OS RNG, authenticated with `aad` as
//!   additional data. Returns `(ciphertext, nonce)`.
//! - `open`: Decrypt `ciphertext` under `key` and `nonce`,
//!   re-checking the AEAD tag against `aad`. On tag mismatch
//!   returns `SecurityError::DecryptionFailed`; on success
//!   returns a `Zeroizing<Vec<u8>>` so the plaintext buffer is
//!   wiped on drop.
//!
//! Story (plain English): Imagine a tamper-evident envelope
//! machine on the desk. The clerk slides a sheet of paper in
//! (`plaintext`), the machine prints a fresh, unique serial
//! number on the outside (`nonce`), seals the envelope so any
//! tampering with the paper or the serial number is obvious
//! on the next read, and hands the envelope back. To open the
//! envelope later, the clerk types the same serial number into
//! a matching reader; the reader checks the seal, opens the
//! envelope, and hands the paper back — wiping the paper's
//! ink the moment the caller lets go of it.
//!
//! The seal binds the envelope to a label written on the
//! outside in pen (`aad`). If anyone swaps the label, the
//! seal check fails on the next open. The engine writes the
//! label as `format!("{}:{}", name, version)` so a row-swap
//! attack (replacing one secret's envelope with another's
//! under the same nonce) cannot succeed.
//!
//! The serial number is never reused. A 12-byte random nonce
//! is 96 bits, so the chance of two `seal` calls (over the
//! whole lifetime of the deployment) producing the same
//! nonce is cryptographically negligible.
//!
//! CID Index:
//! CID:afa-security-crypto-001 -> seal
//! CID:afa-security-crypto-002 -> open
//!
//! Quick lookup: rg -n "CID:afa-security-crypto-" crates/afa-security/src/crypto.rs

use crate::SecurityError;
use aes_gcm::aead::{Aead, KeyInit, Payload};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use zeroize::Zeroizing;

/// The AEAD nonce length, in bytes. Fixed by the AES-256-GCM
/// standard at 12 bytes; the compiler does not enforce this
/// but the spec does.
pub const NONCE_LEN: usize = 12;

/// The AEAD key length, in bytes. Fixed by AES-256 at 32 bytes.
pub const KEY_LEN: usize = 32;

// CID:afa-security-crypto-001 - seal
// Purpose: Encrypt `plaintext` under `key` with a fresh random
// nonce and `aad` as additional authenticated data. Returns
// `(ciphertext, nonce)`. The caller stores both in the
// `sealed_secrets` SQLite row; the nonce is also used as the
// row's `nonce` column so the next `open` can re-derive the
// AAD-protected tag check.
// Caller pattern: `let (ct, nonce) = crypto::seal(pt, &key, aad)?;`
// Errors: only `SecurityError::Internal` (encrypt failure is
// unreachable in practice — only OOM triggers `aead::Error`
// on the encrypt path).
// Used by: `engine::SecurityEngine::seal`.
pub fn seal(
    plaintext: &[u8],
    key: &Zeroizing<[u8; KEY_LEN]>,
    aad: &str,
) -> Result<(Vec<u8>, [u8; NONCE_LEN]), SecurityError> {
    // Build a 12-byte nonce from the OS RNG. `rand::rngs::OsRng`
    // is a zero-cost zero-sized type that draws from
    // `getrandom` (the kernel's CSPRNG), so there is no global
    // state and no way to seed it wrong.
    let mut nonce_bytes = [0u8; NONCE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);

    // Construct the cipher from the master key slice. The
    // `KeyInit::from_slice` constructor validates the length
    // (it would panic on the wrong size, but the engine
    // already type-checked the key at boot).
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| SecurityError::Internal {
        reason: "invalid AES-256-GCM key length".to_string(),
    })?;
    let nonce = Nonce::from_slice(&nonce_bytes);

    // Encrypt. The `Payload { msg, aad }` form is the
    // additional-data path: any change to `aad` between
    // `seal` and `open` will fail the tag check on the next
    // `open` and surface as `DecryptionFailed`. We bind
    // `format!("{}:{}", name, version)` so a row-swap attack
    // (replacing one secret's ciphertext with another's
    // under the same nonce) cannot succeed.
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| SecurityError::Internal {
            reason: "AES-256-GCM seal failed (unreachable in practice)".to_string(),
        })?;

    Ok((ciphertext, nonce_bytes))
}

// CID:afa-security-crypto-002 - open
// Purpose: Decrypt `ciphertext` under `key` and `nonce`,
// re-checking the AEAD tag against `aad`. On tag mismatch
// returns `SecurityError::DecryptionFailed`; on success
// returns a `Zeroizing<Vec<u8>>` so the plaintext buffer
// is wiped when the caller drops it.
// Caller pattern: `let pt = crypto::open(&ct, &nonce, &key, aad)?;`
// Errors: `SecurityError::DecryptionFailed` on tag mismatch
// (covers the three real cases: tampered ciphertext, wrong
// master key, AAD row-swap). `SecurityError::Internal` on
// the unreachable key-length panic.
// Used by: `engine::SecurityEngine::unseal`.
pub fn open(
    ciphertext: &[u8],
    nonce: &[u8; NONCE_LEN],
    key: &Zeroizing<[u8; KEY_LEN]>,
    aad: &str,
) -> Result<Zeroizing<Vec<u8>>, SecurityError> {
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|_| SecurityError::Internal {
        reason: "invalid AES-256-GCM key length".to_string(),
    })?;
    let nonce_ref = Nonce::from_slice(nonce);

    // The `Vec<u8>` return is the only path `aes-gcm` gives
    // us; we wrap it in `Zeroizing` so the wrapper's `Drop`
    // calls `volatile_write(Vec::default())` (per the
    // RustCrypto `zeroize` crate's source). The volatile
    // semantics prevent the optimizer from eliding the
    // zeroing as a "dead store."
    let plaintext = cipher
        .decrypt(
            nonce_ref,
            Payload {
                msg: ciphertext,
                aad: aad.as_bytes(),
            },
        )
        .map_err(|_| SecurityError::DecryptionFailed {
            name: String::new(), // filled in by the engine, which knows the name
            version: 0,
        })?;

    Ok(Zeroizing::new(plaintext))
}
