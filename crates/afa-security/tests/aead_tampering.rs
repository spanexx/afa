//! Test: a single-bit flip in the ciphertext (or in the
//! AAD) causes `open` to return `SecurityError::DecryptionFailed`.
//! This is the regression-proof for the AEAD tag check: the
//! only way the engine can ever be "given a tampered
//! ciphertext" is in this test, and the only acceptable
//! answer is the closed-set `DecryptionFailed` (not a
//! panic, not a different variant, not "the tag check
//! passed anyway").
//!
//! Why this is a real test (not a fake): a failure means
//! the AEAD tag check is not actually catching tampering,
//! which would let an attacker who can write to the
//! secrets database (e.g. via a backup restore or a
//! malicious admin) silently swap one secret's
//! ciphertext for another. The assertion is on the
//! error variant the caller will see, which is the
//! user-visible security boundary.

use afa_contracts::SecurityErrorV1;
use afa_security::crypto;
use zeroize::Zeroizing;

fn test_key() -> Zeroizing<[u8; 32]> {
    Zeroizing::new([0xA5u8; 32])
}

#[test]
fn flipping_a_bit_in_ciphertext_fails_open() {
    let key = test_key();
    let (mut ct, nonce) = crypto::seal(b"hello world", &key, "name:1").expect("seal ok");

    // Flip a bit in the middle of the ciphertext.
    let mid = ct.len() / 2;
    ct[mid] ^= 0x01;

    let result = crypto::open(&ct, &nonce, &key, "name:1");
    match result {
        Err(SecurityErrorV1::DecryptionFailed { .. }) => { /* expected */ }
        Err(other) => panic!("expected DecryptionFailed, got {other:?}"),
        Ok(_) => panic!("expected DecryptionFailed, got Ok (tag check did not fire)"),
    }
}

#[test]
fn flipping_a_bit_in_aad_fails_open() {
    let key = test_key();
    let (ct, nonce) = crypto::seal(b"hello world", &key, "name:1").expect("seal ok");

    // Open with a different AAD. A row-swap attack (where
    // an attacker substitutes one (name, version) pair's
    // ciphertext for another's) is the threat this
    // covers; the AAD is the `(name, version)` string the
    // engine binds to the seal.
    let result = crypto::open(&ct, &nonce, &key, "name:2");
    match result {
        Err(SecurityErrorV1::DecryptionFailed { .. }) => { /* expected */ }
        Err(other) => panic!("expected DecryptionFailed, got {other:?}"),
        Ok(_) => panic!("expected DecryptionFailed, got Ok (AAD check did not fire)"),
    }
}

#[test]
fn wrong_key_fails_open() {
    let key = test_key();
    let (ct, nonce) = crypto::seal(b"hello world", &key, "name:1").expect("seal ok");

    let wrong_key = Zeroizing::new([0x5Au8; 32]);
    let result = crypto::open(&ct, &nonce, &wrong_key, "name:1");
    match result {
        Err(SecurityErrorV1::DecryptionFailed { .. }) => { /* expected */ }
        Err(other) => panic!("expected DecryptionFailed, got {other:?}"),
        Ok(_) => panic!("expected DecryptionFailed, got Ok (wrong key opened the envelope)"),
    }
}
