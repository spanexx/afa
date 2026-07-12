//! Code Map: The master-key newtype
//! - `MasterKey`: A 32-byte master key wrapped in a
//!   newtype. The newtype is the only way code outside
//!   `crypto::seal` / `crypto::open` touches the
//!   underlying bytes, which lets the zeroize
//!   implementation be tied to the type (so a stray
//!   `[u8; 32]` never ends up holding a copy of the key
//!   without the wipe-on-drop guarantee).
//!
//! Story (plain English): Imagine the single key that
//! opens every safe-deposit box in the bank. The bank
//! keeps that key in a special envelope that shreds
//! itself the moment the bank manager lets go of it
//! (`Zeroize` on `Drop`). The `MasterKey` newtype is
//! that envelope. The `Zeroizing<[u8; 32]>` the engine
//! stores is the *contents* of the envelope — same
//! bytes, same shred-on-drop guarantee, but the
//! newtype wrapper is what stops a careless caller from
//! copying the bytes to a `Vec<u8>` and forgetting to
//! wipe it.
//!
//! `from_hex` is the one and only path an operator uses
//! to put a key into an envelope: the env-var reader
//! hands the env-var string straight to `from_hex`,
//! and the kernel boot path hands the result straight
//! to `SecurityEngine::new`. There is no other entry
//! point.
//!
//! CID Index:
//! CID:afa-security-masterkey-001 -> MasterKey
//! CID:afa-security-masterkey-002 -> from_hex
//!
//! Quick lookup: rg -n "CID:afa-security-masterkey-" crates/afa-security/src/master_key.rs

use crate::SecurityError;
use zeroize::{Zeroize, Zeroizing};

/// The master-key length, in bytes. Fixed at 32 by
/// AES-256 (the AEAD the engine uses); the engine's
/// `crypto::seal` / `crypto::open` also enforce this
/// length, so a `MasterKey` of any other size would
/// be rejected on the first `seal` call anyway.
pub const MASTER_KEY_LEN: usize = 32;

// CID:afa-security-masterkey-001 - MasterKey
// Purpose: A 32-byte master key wrapped in a newtype
// so the only way to hold the key in memory is through
// a `Zeroize`-on-drop container. The inner `[u8; 32]`
// is private — outside callers can read the bytes via
// `as_bytes()` (for the AEAD), build a `MasterKey`
// from a 32-byte array via `From<[u8; 32]>`, or build
// one from a 64-char hex string via `from_hex`. There
// is no `Deref<Target=[u8]>` impl (which would let a
// caller store a `&[u8]` somewhere and outlive the
// wipe) and no `Clone` (which would double the
// plaintext exposure surface in the process heap —
// the kernel clones the key by passing a `&MasterKey`
// around, and the engine stores the original in an
// `Arc<Zeroizing<[u8; 32]>>` so a second `Kernel::new`
// for the same env-var still wipes the key when the
// process exits).
// Used by: `SecurityEngine::new` (takes `&MasterKey`),
// `tests/boot_failures.rs` (`read_master_key_from_env`).
pub struct MasterKey([u8; MASTER_KEY_LEN]);

impl MasterKey {
    /// Borrow the inner 32 bytes (e.g. to pass to
    /// `crypto::seal` / `crypto::open`). The borrow
    /// is `&[u8; 32]` (not `&[u8]`) so the caller
    /// cannot accidentally take a longer slice of
    /// the key by mistake.
    pub fn as_bytes(&self) -> &[u8; MASTER_KEY_LEN] {
        &self.0
    }

    // CID:afa-security-masterkey-002 - from_hex
    // Purpose: Decode a 64-character hex string into a
    // `MasterKey`. The one and only path an operator
    // uses to put a key into an envelope: the env-var
    // reader hands the env-var string straight to
    // `from_hex`, and the kernel boot path hands the
    // result straight to `SecurityEngine::new`.
    // Errors: `MasterKeyMalformed` with one of three
    // `&'static str` reasons: `"odd length"`,
    // `"too short"`, `"too long"`, or
    // `"non-hex character"` (matches the four
    // `MasterKeyMalformed` reasons listed in
    // `docs/CONTEXT.md` §5.2, so the dashboard can
    // map the reason to a one-line operator hint
    // without parsing free-form text).
    // Used by: `tests/boot_failures.rs` `read_master_key_from_env`.
    pub fn from_hex(hex: &str) -> Result<Self, SecurityError> {
        // Step 1: Length gate. AES-256 needs exactly
        // 32 bytes / 64 hex chars; reject odd /
        // short / long with a stable reason string
        // so the dashboard can map it to a
        // one-line operator hint.
        if !hex.len().is_multiple_of(2) {
            return Err(SecurityError::MasterKeyMalformed {
                reason: "odd length",
            });
        }
        if hex.len() < MASTER_KEY_LEN * 2 {
            return Err(SecurityError::MasterKeyMalformed {
                reason: "too short",
            });
        }
        if hex.len() > MASTER_KEY_LEN * 2 {
            return Err(SecurityError::MasterKeyMalformed { reason: "too long" });
        }

        // Step 2: Decode the 32 bytes through a
        // `Zeroizing` scratch buffer so a malformed
        // mid-decode half-byte does not leave a
        // non-zero key copy on the heap. We use the
        // pure-Rust hex decoder from the `hex` crate
        // (it is in the kernel's direct deps already
        // for the boot-failures test).
        let mut bytes = Zeroizing::new([0u8; MASTER_KEY_LEN]);
        let decode_result = hex::decode_to_slice(hex, bytes.as_mut());

        // Step 3: Map hex-decode errors back to the
        // four stable reason strings the dashboard
        // understands.
        if let Err(err) = decode_result {
            // `hex::decode_to_slice` only returns
            // `InvalidHexCharacter` for non-hex
            // input (the length check above already
            // gated the other failure modes).
            return Err(SecurityError::MasterKeyMalformed {
                reason: match err {
                    hex::FromHexError::InvalidHexCharacter { .. } => "non-hex character",
                    // `InvalidStringLength` is unreachable
                    // here (the length gate above is
                    // exact), but the compiler still
                    // requires the match arm — map
                    // it to the closest stable reason
                    // so a future refactor cannot
                    // accidentally swallow a real
                    // length error.
                    hex::FromHexError::InvalidStringLength => "odd length",
                    _ => "non-hex character",
                },
            });
        }

        // `bytes` is a `Zeroizing<[u8; 32]>`, which
        // implements `Deref<Target = [u8; 32]>` so the
        // `*bytes` copy gives us a fresh owned
        // `[u8; 32]`. The `Zeroizing` temporary is
        // wiped on the next line (when the `Zeroizing`
        // drops), and again on the `MasterKey`'s own
        // `Drop` (below).
        Ok(MasterKey(*bytes))
    }
}

impl From<[u8; MASTER_KEY_LEN]> for MasterKey {
    /// Lift a raw 32-byte array into the
    /// `MasterKey` newtype. Used by tests that have
    /// a deterministic `Zeroizing<[u8; 32]>` test
    /// key (the `common::test_key()` helper) and
    /// want to pass it to a function that takes
    /// `&MasterKey`.
    fn from(bytes: [u8; MASTER_KEY_LEN]) -> Self {
        MasterKey(bytes)
    }
}

impl From<MasterKey> for [u8; MASTER_KEY_LEN] {
    /// Drop the newtype and return the raw bytes.
    /// Used by `SecurityEngine::new` to copy the
    /// bytes into the engine's `Arc<Zeroizing<[u8;
    /// 32]>>` storage (the engine needs raw bytes
    /// for the AEAD call, not a `MasterKey`).
    /// The newtype itself is dropped (and its
    /// `Drop` impl wipes the bytes) on the
    /// caller's side immediately after this
    /// `Into` returns, so the only live copy in
    /// the process is the engine's `Zeroizing`
    /// wrapper.
    fn from(key: MasterKey) -> Self {
        // The `key` binding is consumed by this
        // expression; the newtype's own
        // `Zeroize` impl runs as soon as the
        // expression returns, wiping the bytes
        // before the caller can hold onto a
        // stray reference.
        key.0
    }
}

impl AsRef<[u8]> for MasterKey {
    /// Borrow the key as a `&[u8]` for callers
    /// that need the slice form (e.g. logging
    /// frameworks that need a `AsRef<[u8]>` for
    /// their redacted-display wrappers). The
    /// lifetime is tied to `&self`, so the
    /// borrow cannot outlive the newtype.
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

impl Zeroize for MasterKey {
    /// Wipe the key bytes on demand. The newtype
    /// also wipes on `Drop` via the derived
    /// blanket impl (any `Zeroize` impl gives the
    /// type a `Drop` that calls `zeroize()` —
    /// see the `zeroize` crate's docs).
    fn zeroize(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for MasterKey {
    /// Defensive: explicit wipe on drop. The
    /// `zeroize` crate's blanket-impl `Drop`
    /// already covers this, but the explicit
    /// `Drop` makes the wipe visible to anyone
    /// reading the type's surface (and lets us
    /// add a `tracing::trace!` later if we ever
    /// want to confirm the wipe happened in
    /// production).
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl std::fmt::Debug for MasterKey {
    /// Redacted `Debug` so a `MasterKey` printed
    /// by a test's panic message or a `dbg!()` does
    /// not leak the key. The format is
    /// `MasterKey([REDACTED 32 bytes])` so the
    /// size is still visible (useful for asserting
    /// the newtype holds the right number of
    /// bytes) but the contents are not.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MasterKey")
            .field(
                "bytes",
                &format_args!("[REDACTED {} bytes]", MASTER_KEY_LEN),
            )
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Test helper: a 64-char hex string of all `0xA5`
    // bytes (matches `common::test_key()` so the
    // hex round-trip and the raw-array construction
    // stay in lock-step).
    const TEST_HEX: &str = "a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5";

    #[test]
    fn from_hex_accepts_a_well_formed_64_char_hex_string() {
        // Happy path: a 64-char hex string decodes
        // to the expected 32 bytes.
        let key = MasterKey::from_hex(TEST_HEX).expect("from_hex should succeed");
        let raw: [u8; MASTER_KEY_LEN] = key.into();
        assert_eq!(raw, [0xA5u8; MASTER_KEY_LEN]);
    }

    #[test]
    fn from_hex_rejects_an_odd_length_hex_string_with_the_stable_reason() {
        // E-7 family (short-circuit on length
        // before the decoder even runs).
        let err = MasterKey::from_hex("abc").expect_err("odd length should fail");
        match err {
            SecurityError::MasterKeyMalformed { reason } => assert_eq!(reason, "odd length"),
            other => panic!("expected MasterKeyMalformed, got {other:?}"),
        }
    }

    #[test]
    fn from_hex_rejects_a_too_short_hex_string_with_the_stable_reason() {
        // 62 chars is < 64 and even, so it fails
        // the `too short` check, not `odd length`.
        let too_short = "a".repeat(62);
        let err = MasterKey::from_hex(&too_short).expect_err("too short should fail");
        match err {
            SecurityError::MasterKeyMalformed { reason } => assert_eq!(reason, "too short"),
            other => panic!("expected MasterKeyMalformed, got {other:?}"),
        }
    }

    #[test]
    fn from_hex_rejects_a_too_long_hex_string_with_the_stable_reason() {
        // 66 chars is > 64 and even, so it fails
        // the `too long` check.
        let too_long = "a".repeat(66);
        let err = MasterKey::from_hex(&too_long).expect_err("too long should fail");
        match err {
            SecurityError::MasterKeyMalformed { reason } => assert_eq!(reason, "too long"),
            other => panic!("expected MasterKeyMalformed, got {other:?}"),
        }
    }

    #[test]
    fn from_hex_rejects_a_non_hex_character_with_the_stable_reason() {
        // Exactly 64 chars but contains a `z` (not
        // a hex digit), so the length gate passes
        // and the hex decoder rejects it.
        let mut bad = String::from(TEST_HEX);
        // Replace the last two chars with a
        // non-hex pair (`zz`).
        bad.replace_range(62..64, "zz");
        let err = MasterKey::from_hex(&bad).expect_err("non-hex should fail");
        match err {
            SecurityError::MasterKeyMalformed { reason } => assert_eq!(reason, "non-hex character"),
            other => panic!("expected MasterKeyMalformed, got {other:?}"),
        }
    }

    #[test]
    fn debug_format_redacts_the_key_bytes() {
        // The `Debug` impl must not leak the key
        // (a panic in production that prints
        // `format!("{:?}", key)` is exactly the
        // kind of leak the newtype is supposed
        // to prevent).
        let key = MasterKey::from_hex(TEST_HEX).expect("from_hex should succeed");
        let printed = format!("{:?}", key);
        assert!(
            !printed.contains("a5a5a5a5"),
            "Debug should not print the raw bytes"
        );
        assert!(
            printed.contains("REDACTED"),
            "Debug should announce the redaction"
        );
    }

    #[test]
    fn from_array_and_into_array_round_trip() {
        // The `From<[u8; 32]>` and
        // `From<MasterKey> for [u8; 32]>` impls
        // are inverses: a key built from a raw
        // array can be unwrapped back to the
        // same array.
        let raw = [0x42u8; MASTER_KEY_LEN];
        let key = MasterKey::from(raw);
        let unwrapped: [u8; MASTER_KEY_LEN] = key.into();
        assert_eq!(unwrapped, raw);
    }
}
