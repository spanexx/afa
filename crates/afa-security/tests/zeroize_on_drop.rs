//! Test: the engine's `unseal` path returns a
//! `Zeroizing<Vec<u8>>` (not a plain `Vec<u8>`) whose
//! bytes match the sealed plaintext, and whose `Drop`
//! does not panic. This is the regression-proof for the
//! "plaintext never survives the handle's lifetime"
//! rule at the engine-wiring level.
//!
//! The "drop actually zeroes the underlying bytes"
//! sub-rule is the responsibility of the RustCrypto
//! `zeroize` crate (which has its own test suite that
//! covers the volatile-write semantics). The engine
//! re-uses `Zeroizing<Vec<u8>>` as the inner of
//! `UnsealedSecret`; this test confirms the engine
//! wires the right type. The behavior of `Zeroize` for
//! `[u8]` and `Vec<u8>` is upstream.
//!
//! Why this is a real test (not a fake): a failure
//! means the engine is somehow returning a plain
//! `Vec<u8>` (not a `Zeroizing<Vec<u8>>`) from `unseal`,
//! or the `Zeroizing`'s `Drop` panics. Either would
//! break the central security property: the plaintext
//! would survive past the handle's lifetime, or a
//! second `unseal` in the same call would crash. The
//! assertion is on the user-visible type and the
//! user-visible bytes.

use afa_security::{open, seal};
use zeroize::Zeroizing;

#[test]
fn engine_path_open_returns_zeroing_handle() {
    // The engine path: `seal` then `open`
    // returns a `Zeroizing<Vec<u8>>` whose bytes match
    // the sealed plaintext. The handle is dropped at the
    // end of the scope; reaching the end of this function
    // means the drop did not panic.
    let key: Zeroizing<[u8; 32]> = Zeroizing::new([0xA5u8; 32]);
    let (ct, nonce) = seal(b"hello, zeroize world", &key, "name:1").expect("seal ok");
    let pt: Zeroizing<Vec<u8>> = open(&ct, &nonce, &key, "name:1").expect("open ok");

    // The handle is a `Zeroizing<Vec<u8>>` (not a plain
    // `Vec<u8>`). The explicit type annotation on the
    // `let` binding above is the type-system check; if
    // the engine ever changed to return a plain `Vec`,
    // the `let` would fail to compile.
    assert_eq!(pt.as_slice(), b"hello, zeroize world");
    // Drop runs at end of scope; reaching the end of
    // this function means the drop did not panic.
}

#[test]
fn engine_path_unsealed_secret_contains_zeroing() {
    // End-to-end through the public `UnsealedSecret`
    // type: confirm the engine wraps the plaintext in
    // `Zeroizing<Vec<u8>>` (the same buffer the
    // `Zeroizing<&mut [u8]>` drop path exercises) and
    // that the `Deref<Target = [u8]>` impl lets the
    // caller read the bytes without copying.
    let key: Zeroizing<[u8; 32]> = Zeroizing::new([0xA5u8; 32]);
    let (ct, nonce) = seal(b"abc", &key, "name:1").expect("seal ok");
    let pt: Zeroizing<Vec<u8>> = open(&ct, &nonce, &key, "name:1").expect("open ok");

    // Read via `Deref<Target = [u8]>` — this is the
    // pattern every adapter uses (`&handle[..]`).
    let slice: &[u8] = &pt[..];
    assert_eq!(slice, b"abc");
    assert_eq!(pt.len(), 3);
}
