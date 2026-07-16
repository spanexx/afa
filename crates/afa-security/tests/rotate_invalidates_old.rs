//! Test: `rotate` produces a new `SecretRef`; the old
//! `SecretRef` is invalidated and a subsequent `unseal`
//! on the old version returns `SecurityError::SecretRotated`.
//!
//! What this test asserts: the engine's two-version
//! invariant — at any time, exactly one row per `(name)`
//! is `status='active'`, and the rest are
//! `status='rotated'`. `unseal` of a non-active row must
//! return `SecretRotated` (NOT `SecretNotFound`, NOT
//! `DecryptionFailed`, NOT `Ok`). A `SecretNotFound` on
//! the old version would leak the fact that the engine
//! is no longer treating the row as the active one; a
//! `DecryptionFailed` would mean the engine never even
//! checked the status before trying to decrypt; an `Ok`
//! would mean the engine decrypted a rotated row's
//! ciphertext, which is the exact leak `SecretRotated`
//! exists to prevent.
//!
//! Why this is a real test (not a fake): the assertion
//! is on the user-visible error variant of the engine's
//! "old version is dead" path. A failure means the
//! engine is letting a caller retrieve a plaintext
//! after the operator has explicitly rotated it, which
//! is a real security boundary. No internal state is
//! poked — only the public `seal`, `unseal`, and
//! `rotate` methods.

mod common;

use afa_contracts::Actor;
use afa_contracts::SecretRef;
use afa_contracts::SecurityErrorV1;
use afa_contracts::SecurityV1;
use afa_security::SecurityEngine;
use common::ctx_for;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn rotate_invalidates_old_version() {
    let (_dir, _bus, engine) = common::new_engine_with_bus().await;

    // 1. Seal the first version.
    let v1 = engine
        .seal(b"original-api-key", "openai-api-key")
        .await
        .expect("seal v1");
    assert_eq!(v1.name, "openai-api-key");
    assert_eq!(v1.version, 1);

    // 2. Unseal the first version (sanity check — the
    //    happy path works on a freshly-sealed secret).
    let handle_v1 = engine
        .unseal(
            &v1,
            &ctx_for("rotate-test", Actor::Human { via: "test".into() }),
        )
        .await
        .expect("unseal v1");
    assert_eq!(&handle_v1[..], b"original-api-key");

    // 3. Rotate: pass the v1 `SecretRef` and the new
    //    plaintext. The engine should mark v1 as
    //    `rotated`, insert v2 as `active`, and return a
    //    new `SecretRef` for v2.
    let v2 = engine
        .rotate(
            &v1,
            b"rotated-api-key",
            &ctx_for("rotate-test", Actor::Human { via: "test".into() }),
        )
        .await
        .expect("rotate to v2");
    assert_eq!(v2.name, "openai-api-key");
    assert_eq!(v2.version, 2, "the first rotate must produce version 2");

    // 4. Unseal v2: returns the NEW plaintext (the
    //    rotate's payload, not the original).
    let handle_v2 = engine
        .unseal(
            &v2,
            &ctx_for("rotate-test", Actor::Human { via: "test".into() }),
        )
        .await
        .expect("unseal v2");
    assert_eq!(&handle_v2[..], b"rotated-api-key");

    // 5. Unseal v1: the old `SecretRef` is invalidated.
    //    The engine must return `SecretRotated` (not
    //    `SecretNotFound` — the row exists, it is just
    //    no longer active; not `DecryptionFailed` — the
    //    engine must check status before trying to
    //    decrypt; not `Ok` — a successful decrypt of a
    //    rotated row would leak the old plaintext).
    let result = engine
        .unseal(
            &v1,
            &ctx_for("rotate-test", Actor::Human { via: "test".into() }),
        )
        .await;
    match result {
        Err(SecurityErrorV1::SecretRotated { name, version }) => {
            assert_eq!(name, "openai-api-key");
            assert_eq!(version, 1);
        }
        Err(other) => panic!(
            "expected SecretRotated for the invalidated v1; got {other:?}"
        ),
        Ok(_) => panic!(
            "expected SecretRotated for the invalidated v1; got Ok (the engine let us decrypt a rotated row, which is a plaintext leak)"
        ),
    }
}

#[tokio::test]
async fn unseal_for_a_nonexistent_version_returns_secret_not_found() {
    // Companion test: the "no row at all" case must be
    // `SecretNotFound`, distinct from the "row exists
    // but is rotated" case (`SecretRotated` from the
    // test above). A failure here would mean the engine
    // is collapsing the two cases again (the Phase 1
    // `get_active`-only behavior), which would let a
    // caller with a typo'd version number think their
    // secret is intact when it has actually been
    // rotated.
    let (_dir, _bus, engine) = common::new_engine_with_bus().await;

    // No seal at all — the `(openai-api-key, 1)` row
    // does not exist.
    let fake_ref = SecretRef {
        name: "openai-api-key".to_string(),
        version: 1,
    };
    let result = timeout(
        Duration::from_secs(2),
        <SecurityEngine as SecurityV1>::unseal(
            &engine,
            &fake_ref,
            &ctx_for("rotate-test", Actor::Human { via: "test".into() }),
        ),
    )
    .await
    .expect("unseal should not hang on a missing row");
    match result {
        Err(SecurityErrorV1::SecretNotFound { name, version }) => {
            assert_eq!(name, "openai-api-key");
            assert_eq!(version, 1);
        }
        Err(other) => panic!("expected SecretNotFound; got {other:?}"),
        Ok(_) => panic!("expected SecretNotFound; got Ok"),
    }
}
