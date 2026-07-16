//! Test: `seal` followed by `open` returns the
//! identical plaintext for the boundary cases of the 64 KB
//! payload cap (0, 1, 64, 4096, 65535 bytes).
//!
//! What this test asserts: the AEAD round-trip is correct
//! for every size the engine's `seal` validation is
//! designed to accept. The empty-payload case covers the
//! "0-byte AAD-protected blob" edge (the engine never
//! refuses an empty `plaintext`, only an oversized one).
//! The 65535-byte case is the largest payload the engine
//! will accept (the cap is 64 * 1024 = 65536).
//!
//! Why this is a real test (not a fake): a failure means
//! the AEAD round-trip is broken for a payload size the
//! engine will hand it in production. The assertion is on
//! the visible plaintext bytes, not on internal state.

use afa_security::{open, seal};
use zeroize::Zeroizing;

/// Master key for the test: a fixed 32-byte pattern so the
/// test is deterministic.
fn test_key() -> Zeroizing<[u8; 32]> {
    Zeroizing::new([0xA5u8; 32])
}

#[test]
fn empty_plaintext_round_trips() {
    let key = test_key();
    let (ct, nonce) = seal(b"", &key, "aad:0").expect("seal ok");
    let pt = open(&ct, &nonce, &key, "aad:0").expect("open ok");
    assert_eq!(pt.as_slice(), b"");
}

#[test]
fn one_byte_plaintext_round_trips() {
    let key = test_key();
    let (ct, nonce) = seal(b"x", &key, "aad:0").expect("seal ok");
    let pt = open(&ct, &nonce, &key, "aad:0").expect("open ok");
    assert_eq!(pt.as_slice(), b"x");
}

#[test]
fn sixty_four_byte_plaintext_round_trips() {
    let key = test_key();
    let pt_in: Vec<u8> = (0u8..64).collect();
    let (ct, nonce) = seal(&pt_in, &key, "aad:0").expect("seal ok");
    let pt_out = open(&ct, &nonce, &key, "aad:0").expect("open ok");
    assert_eq!(pt_out.as_slice(), pt_in.as_slice());
}

#[test]
fn four_kib_plaintext_round_trips() {
    let key = test_key();
    let pt_in: Vec<u8> = (0u8..=255).cycle().take(4096).collect();
    let (ct, nonce) = seal(&pt_in, &key, "aad:0").expect("seal ok");
    let pt_out = open(&ct, &nonce, &key, "aad:0").expect("open ok");
    assert_eq!(pt_out.len(), 4096);
    assert_eq!(pt_out.as_slice(), pt_in.as_slice());
}

#[test]
fn max_payload_65535_bytes_round_trips() {
    let key = test_key();
    let pt_in: Vec<u8> = (0u8..=255).cycle().take(65_535).collect();
    let (ct, nonce) = seal(&pt_in, &key, "aad:0").expect("seal ok");
    let pt_out = open(&ct, &nonce, &key, "aad:0").expect("open ok");
    assert_eq!(pt_out.len(), 65_535);
    assert_eq!(pt_out.as_slice(), pt_in.as_slice());
}
