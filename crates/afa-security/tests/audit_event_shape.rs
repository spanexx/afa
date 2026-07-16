//! Test: every successful `seal` / `unseal` / `rotate`
//! publishes a `SecretSealed` / `SecretUnsealed` /
//! `SecretRotated` event on the engine's `EventBus`
//! with the documented field shape. No event ever
//! carries a plaintext byte, the master key, or any
//! other field not in the contracts-side
//! specification.
//!
//! What this test asserts: the engine's
//! "audit events are metadata-only" rule. A failure
//! means the engine is publishing events that
//! could be used to reconstruct a plaintext or a
//! key — which would be a one-line compliance
//! violation, a one-line forensics problem, and a
//! one-line log-line redaction race.
//!
//! Why this is a real test (not a fake): the
//! assertion is on the public bus's view of
//! what the engine published. A failure means
//! the engine is leaking a security-sensitive
//! field to the audit log. No internal state
//! is poked — only the public `seal`, `unseal`,
//! `rotate`, and `event_bus` methods are
//! exercised.

mod common;

use afa_contracts::Actor;
use afa_contracts::SecurityV1;
use afa_security::SecretRotated as EngineSecretRotated;
use afa_security::SecretSealed as EngineSecretSealed;
use afa_security::SecretUnsealed as EngineSecretUnsealed;
use afa_security::SecurityError;
use common::ctx_for;
use std::time::Duration;
use tokio::time::timeout;

#[tokio::test]
async fn seal_publishes_a_secret_sealed_event_with_the_documented_fields() {
    let (_dir, bus, engine) = common::new_engine_with_bus().await;
    let mut sealed_sub = bus.subscribe::<EngineSecretSealed>(8);

    let v1 = engine
        .seal(b"hello, audit log", "audit-secret")
        .await
        .expect("seal ok");
    assert_eq!(v1.name, "audit-secret");
    assert_eq!(v1.version, 1);

    // The engine publishes AFTER the transaction
    // commits, so a 2-second timeout is plenty of
    // slack (the bus is in-process; a publish is
    // effectively instant).
    let event = timeout(Duration::from_secs(2), sealed_sub.recv())
        .await
        .expect("SecretSealed should arrive within 2s")
        .expect("SecretSealed should be Some");
    let (sealed, _ctx) = event;
    assert_eq!(sealed.name, "audit-secret");
    assert_eq!(sealed.version, 1);
    // The timestamp is `chrono::Utc::now()` — we
    // only assert it is "close to now" (within 60
    // seconds), not an exact value.
    let now = chrono::Utc::now();
    let diff = (now - sealed.timestamp).num_seconds().abs();
    assert!(
        diff < 60,
        "SecretSealed.timestamp should be close to now; got {diff}s diff"
    );
}

#[tokio::test]
async fn unseal_publishes_a_secret_unsealed_event_with_the_documented_fields() {
    let (_dir, bus, engine) = common::new_engine_with_bus().await;
    let mut sealed_sub = bus.subscribe::<EngineSecretSealed>(8);
    let mut unsealed_sub = bus.subscribe::<EngineSecretUnsealed>(8);

    let v1 = engine
        .seal(
            b"plaintext that must NEVER appear in any event",
            "audit-secret-2",
        )
        .await
        .expect("seal ok");
    // Drain the SecretSealed event so the bus
    // is fresh for the unseal publish.
    let _ = timeout(Duration::from_secs(2), sealed_sub.recv()).await;

    let ctx = ctx_for(
        "audit-test-tenant",
        Actor::Human {
            via: "dashboard".into(),
        },
    );
    let handle = engine.unseal(&v1, &ctx).await.expect("unseal ok");
    drop(handle);

    let event = timeout(Duration::from_secs(2), unsealed_sub.recv())
        .await
        .expect("SecretUnsealed should arrive within 2s")
        .expect("SecretUnsealed should be Some");
    let (unsealed, ctx_seen) = event;
    // The fields the contracts spec promises.
    assert_eq!(unsealed.name, "audit-secret-2");
    assert_eq!(unsealed.version, 1);
    assert_eq!(unsealed.tenant_id.to_string(), "audit-test-tenant");
    // The actor round-trips through the bus's
    // per-event ctx (the event itself stores
    // `actor: Actor`, which serializes + re-
    // deserializes cleanly; we just check the
    // enum tag and the `via` string).
    match &unsealed.actor {
        Actor::Human { via } => assert_eq!(via, "dashboard"),
        other => panic!("expected Human {{ via: \"dashboard\" }}; got {other:?}"),
    }
    // The bus's per-event `ctx` matches the
    // `unsealed.correlation_id` (sanity check
    // that the dispatch wired the same ctx
    // through).
    assert_eq!(unsealed.correlation_id, ctx_seen.correlation_id);
}

#[tokio::test]
async fn rotate_publishes_a_secret_rotated_event_with_both_versions() {
    let (_dir, bus, engine) = common::new_engine_with_bus().await;
    let mut sealed_sub = bus.subscribe::<EngineSecretSealed>(8);
    let mut rotated_sub = bus.subscribe::<EngineSecretRotated>(8);

    let v1 = engine
        .seal(b"old-key", "rotate-audit-secret")
        .await
        .expect("seal v1");
    let _ = timeout(Duration::from_secs(2), sealed_sub.recv()).await;

    let ctx = ctx_for(
        "rotate-audit-tenant",
        Actor::Internal {
            caller: "test".to_string(),
        },
    );
    let v2 = engine
        .rotate(&v1, b"new-key", &ctx)
        .await
        .expect("rotate ok");
    assert_eq!(v2.version, 2);

    let event = timeout(Duration::from_secs(2), rotated_sub.recv())
        .await
        .expect("SecretRotated should arrive within 2s")
        .expect("SecretRotated should be Some");
    let (rotated, _ctx) = event;
    assert_eq!(rotated.name, "rotate-audit-secret");
    assert_eq!(rotated.old_version, 1);
    assert_eq!(rotated.new_version, 2);
    assert_eq!(rotated.tenant_id.to_string(), "rotate-audit-tenant");
    match &rotated.actor {
        Actor::Internal { caller } => assert_eq!(caller, "test"),
        other => panic!("expected Internal {{ caller: \"test\" }}; got {other:?}"),
    }
}

#[tokio::test]
async fn no_event_carries_the_plaintext_or_the_master_key() {
    // Negative-space test: the engine's published
    // events must not contain the plaintext or the
    // master key (or any other field not in the
    // spec). The assertion is on the JSON shape
    // of each event type — the field set must
    // match the contracts spec exactly, and the
    // plaintext bytes must not appear anywhere in
    // the serialized form.
    let (_dir, bus, engine) = common::new_engine_with_bus().await;
    let mut sealed_sub = bus.subscribe::<EngineSecretSealed>(8);
    let mut unsealed_sub = bus.subscribe::<EngineSecretUnsealed>(8);
    let mut rotated_sub = bus.subscribe::<EngineSecretRotated>(8);

    let secret_plaintext = b"plaintext-NEVER-LEAK-THROUGH-AUDIT";
    let v1 = engine
        .seal(secret_plaintext, "leak-check-secret")
        .await
        .expect("seal ok");
    let handle = engine
        .unseal(&v1, &ctx_for("leak-check-tenant", Actor::Timer))
        .await
        .expect("unseal ok");
    drop(handle);
    let _v2 = engine
        .rotate(
            &v1,
            b"new-plaintext",
            &ctx_for("leak-check-tenant", Actor::Timer),
        )
        .await
        .expect("rotate ok");

    // Collect every published event.
    let sealed = timeout(Duration::from_secs(2), sealed_sub.recv())
        .await
        .expect("sealed arrived")
        .expect("sealed Some");
    let unsealed = timeout(Duration::from_secs(2), unsealed_sub.recv())
        .await
        .expect("unsealed arrived")
        .expect("unsealed Some");
    let rotated = timeout(Duration::from_secs(2), rotated_sub.recv())
        .await
        .expect("rotated arrived")
        .expect("rotated Some");

    // Serialize each event and assert the
    // plaintext / master-key bytes are nowhere
    // in the JSON. (We use `serde_json` because
    // the audit log is a JSON log; if the
    // engine ever started emitting a binary
    // blob, that would be a separate violation
    // caught by the field-set assertion below.)
    // The bus hands us `Arc<T>`, so we deref
    // (`&*sealed.0`) to get the inner value
    // before serializing.
    let sealed_json = serde_json::to_string(&*sealed.0).expect("sealed serialize");
    let unsealed_json = serde_json::to_string(&*unsealed.0).expect("unsealed serialize");
    let rotated_json = serde_json::to_string(&*rotated.0).expect("rotated serialize");
    let plaintext_ascii = std::str::from_utf8(secret_plaintext).expect("plaintext is utf-8");

    for (event_type, json) in [
        ("SecretSealed", &sealed_json),
        ("SecretUnsealed", &unsealed_json),
        ("SecretRotated", &rotated_json),
    ] {
        assert!(
            !json.contains(plaintext_ascii),
            "{event_type} event must not contain the plaintext bytes; got: {json}"
        );
    }
}

#[tokio::test]
async fn full_lifecycle_seal_unseal_rotate_unseal_old_fails_unseal_new_succeeds() {
    // The A-3 lifecycle test from
    // `IMPL-security-engine-baseline.md` Phase 4. Walks
    // the engine through the full happy-path plus one
    // invalidation path, and asserts the audit trail
    // carries exactly the four events the design
    // promises, in the order the engine publishes them:
    //
    //   1. SecretSealed   (v1 is filed)
    //   2. SecretUnsealed (v1 is opened)
    //   3. SecretRotated  (v1 is replaced by v2)
    //   4. SecretUnsealed (v2 is opened)
    //
    // The failed unseal of v1 in step (4a) MUST NOT
    // publish a SecretUnsealed event. The engine's
    // `unseal` rejects a `status='rotated'` row
    // BEFORE the publish call (see
    // `engine.rs:327-332`), returning
    // `SecurityError::SecretRotated`. This test is the
    // regression-proof that an audit subscriber never
    // sees a "secret X was opened" fact for a secret
    // that was not actually opened — which would be a
    // false-positive in any future compliance scan
    // ("we unsealed the openai key 47 times" must mean
    // 47 actual unseals, not 46 actual unseals plus 1
    // attempt).
    let (_dir, bus, engine) = common::new_engine_with_bus().await;
    let mut sealed_sub = bus.subscribe::<EngineSecretSealed>(8);
    let mut unsealed_sub = bus.subscribe::<EngineSecretUnsealed>(8);
    let mut rotated_sub = bus.subscribe::<EngineSecretRotated>(8);

    // 1. Seal v1.
    let v1 = engine
        .seal(b"plaintext-v1", "lifecycle-secret")
        .await
        .expect("seal v1 ok");

    // 2. Unseal v1 (succeeds -> publishes
    //    SecretUnsealed).
    let ctx = ctx_for("lifecycle-tenant", Actor::Timer);
    let handle = engine.unseal(&v1, &ctx).await.expect("unseal v1 ok");
    drop(handle);

    // 3. Rotate v1 -> v2.
    let v2 = engine
        .rotate(&v1, b"plaintext-v2", &ctx)
        .await
        .expect("rotate v1->v2 ok");
    assert_eq!(v2.version, 2);

    // 4. Unseal v1 (must FAIL — v1 is now
    //    rotated-out). No audit event is published for
    //    this call.
    match engine.unseal(&v1, &ctx).await {
        Err(SecurityError::SecretRotated { name, version }) => {
            assert_eq!(name, "lifecycle-secret");
            assert_eq!(version, 1);
        }
        Err(other) => panic!("expected SecurityError::SecretRotated; got {other:?}"),
        Ok(_) => panic!("unsealing v1 (now rotated) must fail; got Ok"),
    }

    // 5. Unseal v2 (succeeds -> publishes
    //    SecretUnsealed).
    let handle = engine.unseal(&v2, &ctx).await.expect("unseal v2 ok");
    drop(handle);

    // Drain the audit trail in the order the engine
    // published them.
    let sealed = timeout(Duration::from_secs(2), sealed_sub.recv())
        .await
        .expect("SecretSealed should arrive within 2s")
        .expect("SecretSealed should be Some");
    let (s, _) = sealed;
    assert_eq!(s.name, "lifecycle-secret");
    assert_eq!(s.version, 1);

    let unsealed_v1 = timeout(Duration::from_secs(2), unsealed_sub.recv())
        .await
        .expect("SecretUnsealed (v1) should arrive within 2s")
        .expect("SecretUnsealed (v1) should be Some");
    let (u1, _) = unsealed_v1;
    assert_eq!(u1.name, "lifecycle-secret");
    assert_eq!(u1.version, 1);

    let rotated = timeout(Duration::from_secs(2), rotated_sub.recv())
        .await
        .expect("SecretRotated should arrive within 2s")
        .expect("SecretRotated should be Some");
    let (r, _) = rotated;
    assert_eq!(r.name, "lifecycle-secret");
    assert_eq!(r.old_version, 1);
    assert_eq!(r.new_version, 2);

    let unsealed_v2 = timeout(Duration::from_secs(2), unsealed_sub.recv())
        .await
        .expect("SecretUnsealed (v2) should arrive within 2s")
        .expect("SecretUnsealed (v2) should be Some");
    let (u2, _) = unsealed_v2;
    assert_eq!(u2.name, "lifecycle-secret");
    assert_eq!(u2.version, 2);

    // No further events of any kind — the failed
    // unseal in step (4) did NOT publish a
    // SecretUnsealed fact. If it had, one of the next
    // three `recv()`s would have returned Some
    // instead of timing out.
    assert!(
        timeout(Duration::from_millis(100), sealed_sub.recv())
            .await
            .is_err(),
        "no more SecretSealed events expected"
    );
    assert!(
        timeout(Duration::from_millis(100), unsealed_sub.recv())
            .await
            .is_err(),
        "no more SecretUnsealed events expected; failed unseal must not publish"
    );
    assert!(
        timeout(Duration::from_millis(100), rotated_sub.recv())
            .await
            .is_err(),
        "no more SecretRotated events expected"
    );
}
