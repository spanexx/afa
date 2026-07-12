//! Code Map: SecurityErrorV1 -> AfaErrorKind mapping
//! - 11 tests, one per `SecurityErrorV1` variant, each
//!   asserts the variant maps to the correct
//!   `AfaErrorKind`. The mapping is the contract every
//!   caller depends on (a `match` on `kind()` in a generic
//!   error handler must produce the right HTTP status /
//!   retry behaviour / log severity for each variant).
//!
//! Story (plain English): The security engine's "what
//! went wrong?" list has eleven entries, but the rest of
//! the kernel only has six coarse buckets ("not found",
//! "not allowed", "service down", "too slow", "not
//! supported", "weird internal error"). The mapping from
//! the eleven to the six is the dictionary the rest of
//! the kernel relies on. This file's eleven tiny tests
//! are the regression-proof for that dictionary — if a
//! future pack renames a variant or moves it to a
//! different bucket, the right test fails first.

use afa_contracts::{AfaError, AfaErrorKind, SecurityErrorV1};

#[test]
fn master_key_missing_maps_to_unavailable() {
    let e = SecurityErrorV1::MasterKeyMissing;
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn master_key_malformed_maps_to_unavailable() {
    let e = SecurityErrorV1::MasterKeyMalformed {
        reason: "not 64 hex chars",
    };
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn storage_unreachable_maps_to_unavailable() {
    let e = SecurityErrorV1::StorageUnreachable {
        reason: "permission denied".into(),
    };
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn storage_corrupted_maps_to_unavailable() {
    let e = SecurityErrorV1::StorageCorrupted;
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn schema_version_mismatch_maps_to_unavailable() {
    let e = SecurityErrorV1::SchemaVersionMismatch {
        found: 2,
        expected: 1,
    };
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn payload_too_large_maps_to_unavailable() {
    // 64 KiB + 1 is the smallest size that fails the
    // payload cap (the engine rejects >= 64 KiB + 1).
    let e = SecurityErrorV1::PayloadTooLarge {
        size: 65_537,
        cap: 65_536,
    };
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn name_too_long_maps_to_unavailable() {
    let e = SecurityErrorV1::NameTooLong {
        length: 257,
        cap: 256,
    };
    assert_eq!(e.kind(), AfaErrorKind::Unavailable);
}

#[test]
fn secret_not_found_maps_to_not_found() {
    let e = SecurityErrorV1::SecretNotFound {
        name: "openai-api-key".into(),
        version: 7,
    };
    assert_eq!(e.kind(), AfaErrorKind::NotFound);
}

#[test]
fn secret_rotated_maps_to_not_found() {
    // Rotated is intentionally `NotFound`, not a separate
    // kind: the contract on the old `SecretRef` is "this
    // receipt is no longer valid for an unseal", which is
    // the same coarse "not here" signal the rest of the
    // kernel uses for any missing resource.
    let e = SecurityErrorV1::SecretRotated {
        name: "openai-api-key".into(),
        version: 6,
    };
    assert_eq!(e.kind(), AfaErrorKind::NotFound);
}

#[test]
fn decryption_failed_maps_to_unauthorized() {
    // AEAD tag mismatch = "you are not allowed to read
    // this." The variant is intentionally collapsed into
    // one name (KeyMismatch and TamperedCiphertext are
    // NOT separate variants) so a caller cannot use the
    // error type as an oracle to figure out which
    // scenario is happening.
    let e = SecurityErrorV1::DecryptionFailed {
        name: "openai-api-key".into(),
        version: 7,
    };
    assert_eq!(e.kind(), AfaErrorKind::Unauthorized);
}

#[test]
fn internal_maps_to_internal() {
    let e = SecurityErrorV1::Internal {
        reason: "invariant broken".into(),
    };
    assert_eq!(e.kind(), AfaErrorKind::Internal);
}

#[test]
fn no_new_afa_error_kind_variants_are_introduced() {
    // The contract is: every SecurityErrorV1 variant maps
    // to one of the 6 existing AfaErrorKind values. No
    // new kind. This test asserts that the closed set is
    // intact by enumerating the bucket each variant goes
    // to and confirming the result is one of the 6.
    let variants: Vec<AfaErrorKind> = vec![
        SecurityErrorV1::MasterKeyMissing.kind(),
        SecurityErrorV1::MasterKeyMalformed { reason: "x" }.kind(),
        SecurityErrorV1::StorageUnreachable { reason: "x".into() }.kind(),
        SecurityErrorV1::StorageCorrupted.kind(),
        SecurityErrorV1::SchemaVersionMismatch {
            found: 1,
            expected: 1,
        }
        .kind(),
        SecurityErrorV1::PayloadTooLarge { size: 1, cap: 1 }.kind(),
        SecurityErrorV1::NameTooLong { length: 1, cap: 1 }.kind(),
        SecurityErrorV1::SecretNotFound {
            name: "x".into(),
            version: 1,
        }
        .kind(),
        SecurityErrorV1::SecretRotated {
            name: "x".into(),
            version: 1,
        }
        .kind(),
        SecurityErrorV1::DecryptionFailed {
            name: "x".into(),
            version: 1,
        }
        .kind(),
        SecurityErrorV1::Internal { reason: "x".into() }.kind(),
    ];
    for k in &variants {
        let ok = matches!(
            k,
            AfaErrorKind::NotFound
                | AfaErrorKind::Unauthorized
                | AfaErrorKind::Unavailable
                | AfaErrorKind::Timeout
                | AfaErrorKind::CapabilityUnsupported
                | AfaErrorKind::Internal
        );
        assert!(
            ok,
            "SecurityErrorV1 mapped to a non-AfaErrorKind value: {k:?}"
        );
    }
}
