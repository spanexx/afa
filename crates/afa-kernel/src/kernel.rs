//! Code Map: The top-level composition
//! - `Kernel`: The top-level composition that owns the
//!   `Runtime`, the `Arc<Scheduler>`, the `Arc<EventBus>`,
//!   and the `Arc<dyn SecurityV1>` (the security engine
//!   constructed in Phase 3 from a `MasterKey` and a
//!   secrets-DB path), all wired together. Cloning is
//!   cheap (every field is `Arc`-backed). Constructed via
//!   `Kernel::new(master_key, secrets_db_path)`; the
//!   constructor is the **only** path through which a
//!   `SecurityEngine` is built in the v1 codebase.
//!
//! Story (plain English): Imagine the front desk of a
//! small post office. The desk is the `Runtime` (the only
//! place a letter can be dropped off). Behind the desk is
//! the sorting room (the `Scheduler`), the mail
//! shelves (`EventBus`), and the safe (`SecurityEngine`)
//! where the manager keeps the day's deposit-box keys.
//! The post office as a whole (the `Kernel`) is just a
//! clean way to say "all four of those, wired together."
//! Several tellers at different counters can each have
//! their own copy of the post office — but they all share
//! the same mail shelves, the same sorting room, and the
//! same safe, so a letter dropped at one counter lands in
//! exactly the same boxes as a letter dropped at any
//! other, and any teller can hand a key out of the safe
//! to a customer who needs to open a deposit box.
//!
//! CID Index:
//! CID:kernel-001 -> Kernel
//!
//! Quick lookup: rg -n "CID:kernel-" crates/afa-kernel/src/kernel.rs

use crate::event_bus::{EventBus, EventBusHandle};
use crate::runtime::Runtime;
use crate::scheduler::Scheduler;
use afa_contracts::{SecurityErrorV1, SecurityV1};
use afa_security::{MasterKey, SealedSecretStore, SecurityEngine};
use std::path::PathBuf;
use std::sync::Arc;

// CID:kernel-001 - Kernel
// Purpose: The top-level composition. Owns the
// `Runtime`, the `Arc<Scheduler>`, the
// `Arc<EventBus>`, and the `Arc<dyn SecurityV1>` (the
// security engine), all wired together so a single
// `Kernel::new(master_key, secrets_db_path)` call
// gives you a working kernel. Cloning a `Kernel` is
// cheap because every field is `Arc`-backed; this is
// the intended sharing pattern (e.g. one `Kernel`
// per `axum` request handler, each of which calls
// `runtime.ingest` or `security().seal(...)`).
// Uses: `Arc<Scheduler>`, `Arc<EventBus>`, `Runtime`,
// `Arc<dyn SecurityV1>`. The `SecurityV1` trait object
// (rather than the concrete `SecurityEngine`) is what
// downstream adapters depend on — they never know
// there is a SQLite file behind the desk.
// Used by: every consumer of the kernel; this is the
// type most callers will hold and pass around.
pub struct Kernel {
    runtime: Runtime,
    scheduler: Arc<Scheduler>,
    event_bus: Arc<EventBus>,
    security: Arc<dyn SecurityV1>,
}

impl Kernel {
    /// Build a fresh `Kernel`, including a freshly
    /// constructed `SecurityEngine` that owns the
    /// `secrets.db` SQLite file at `secrets_db_path` and
    /// the master key in an `Arc<Zeroizing<[u8; 32]>>`.
    ///
    /// Steps:
    /// 1. Open or create the `secrets.db` SQLite file
    ///    at `secrets_db_path` (via
    ///    `SealedSecretStore::open_or_create`, which runs
    ///    the idempotent schema on first boot).
    /// 2. Build the `SecurityEngine` over the store and
    ///    the kernel's `Arc<EventBus>`.
    /// 3. Wire the `Runtime` over the `Scheduler` and
    ///    the `EventBusHandle`.
    /// 4. Store the `SecurityEngine` behind the
    ///    `Arc<dyn SecurityV1>` trait object so
    ///    downstream adapters cannot bypass the trait.
    ///
    /// Errors: propagates `SecurityErrorV1` from the
    /// store / engine construction (the typical case
    /// is `StorageUnreachable` for an unwritable
    /// parent dir or `StorageCorrupted` for a truncated
    /// SQLite file). The caller (an `axum` bootstrap
    /// handler or a CLI `afa kernel start` command) is
    /// expected to log the error and refuse to start.
    pub fn new(master_key: &MasterKey, secrets_db_path: PathBuf) -> Result<Self, SecurityErrorV1> {
        // Step 1: open or create the SQLite file. The
        // `open_or_create` call runs the idempotent
        // schema on first boot (a fresh file gets the
        // `sealed_secrets` table; an existing file is
        // left untouched).
        let store = SealedSecretStore::open_or_create(&secrets_db_path)?;

        // Step 2: build the shared bus (every adapter
        // sees the same one), and the `Runtime` /
        // `Scheduler` over it.
        let scheduler = Arc::new(Scheduler::new());
        let event_bus = Arc::new(EventBus::new());

        // Step 3: build the `SecurityEngine`. The
        // engine gets a fresh `Arc` clone of the bus
        // so the kernel's own bus handle and the
        // engine's bus handle point at the same
        // underlying bus.
        let engine = SecurityEngine::new(master_key, store, Arc::clone(&event_bus));
        // Upcast to the trait object so the kernel's
        // public `security()` accessor hands out the
        // locked `SecurityV1` view, not the concrete
        // engine. Downstream adapters never see the
        // SQLite file.
        let security: Arc<dyn SecurityV1> = Arc::new(engine);

        // Step 4: build the `Runtime` over the
        // scheduler and the bus handle.
        let runtime = Runtime::new(Arc::clone(&scheduler), event_bus.handle());

        Ok(Self {
            runtime,
            scheduler,
            event_bus,
            security,
        })
    }

    /// Borrow the `Runtime` (the single ingress point).
    /// `Runtime` is the only way to send an event into
    /// the kernel; there is no other path.
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Hand out a fresh `Arc<Scheduler>` (the
    /// dispatcher). The Scheduler is what workflow
    /// authors call `register` on to install steps
    /// for their event types.
    pub fn scheduler(&self) -> Arc<Scheduler> {
        Arc::clone(&self.scheduler)
    }

    /// Hand out a fresh `Arc<EventBus>` (the
    /// pub/sub broker). Use this when you want to
    /// `subscribe` to events; use the `EventBusHandle`
    /// returned by `Runtime::ingest` (or this method's
    /// sibling) when you want to `publish`.
    pub fn event_bus(&self) -> Arc<EventBus> {
        Arc::clone(&self.event_bus)
    }

    /// Hand out a fresh `EventBusHandle` (a
    /// publish-only view of the bus). Steps receive a
    /// handle to publish their own events; this method
    /// is for code that wants the same publish-only
    /// view without going through a step.
    #[allow(dead_code)] // Used by future packs (afa-cli, etc.).
    pub fn event_bus_handle(&self) -> EventBusHandle {
        self.event_bus.handle()
    }

    /// Hand out a fresh `Arc<dyn SecurityV1>` (the
    /// security engine's trait-object view). Every
    /// downstream adapter that needs to `seal` /
    /// `unseal` / `rotate` a secret goes through this
    /// method, so the kernel is the only place that
    /// holds a concrete `SecurityEngine` (and the
    /// only place that holds the `secrets.db` file
    /// handle).
    #[allow(dead_code)] // Used by future packs (afa-cli, axum handlers, etc.).
    pub fn security(&self) -> Arc<dyn SecurityV1> {
        Arc::clone(&self.security)
    }
}

impl Clone for Kernel {
    /// Cheaply clone the kernel. Every field is
    /// `Arc`-backed, so this is just a few refcount
    /// bumps — no registry copy, no bus copy, no
    /// runtime copy. The two clones share the exact
    /// same underlying `Scheduler`, `EventBus`, and
    /// `SecurityEngine`; steps registered on one are
    /// immediately visible to the other, and a secret
    /// sealed on one is immediately unsealable on the
    /// other.
    fn clone(&self) -> Self {
        Self {
            runtime: Runtime::new(Arc::clone(&self.scheduler), self.event_bus.handle()),
            scheduler: Arc::clone(&self.scheduler),
            event_bus: Arc::clone(&self.event_bus),
            security: Arc::clone(&self.security),
        }
    }
}

impl std::fmt::Debug for Kernel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Kernel").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::EventReceived;
    use afa_contracts::{Actor, AfaEvent, TenantId};
    use afa_security::MasterKey;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    /// Build a fresh `MasterKey` (a deterministic
    /// `0x42` pattern) and a fresh tempdir-backed
    /// `secrets.db` path. The `TempDir` is returned
    /// so the test can keep the path alive for the
    /// test's entire scope (dropping the `TempDir`
    /// would delete the file, which would race with
    /// the engine's open connection on slow
    /// filesystems).
    fn fresh_kernel() -> (TempDir, Kernel) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.db");
        let key = MasterKey::from([0x42u8; 32]);
        let kernel = Kernel::new(&key, path).expect("kernel::new");
        (dir, kernel)
    }

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Probe {
        payload: String,
    }

    impl AfaEvent for Probe {}

    #[tokio::test]
    async fn kernel_new_returns_a_working_kernel() {
        // Flow: a freshly-constructed Kernel can
        // accept an ingest and produce an
        // `EventReceived` audit-trail fact. If
        // `Kernel::new` wired the components
        // incorrectly, this would fail.
        let (_dir, kernel) = fresh_kernel();
        let bus = kernel.event_bus();
        let mut received = bus.subscribe::<EventReceived>(16);

        kernel
            .runtime()
            .ingest(
                Probe {
                    payload: "ping".into(),
                },
                TenantId::new("test-tenant"),
                Actor::Timer,
            )
            .await;

        let (event, _) = received.recv().await.expect("EventReceived");
        assert_eq!(event.event_type, std::any::type_name::<Probe>());
    }

    #[tokio::test]
    async fn kernel_accessors_return_the_same_underlying_components() {
        // Flow: `kernel.scheduler()` and
        // `kernel.event_bus()` each hand out a fresh
        // `Arc`, but the underlying instances are
        // shared. We check this by pointing the
        // `Arc`s at the same registry entry and
        // confirming both see the same steps.
        let (_dir, kernel) = fresh_kernel();
        let scheduler_a = kernel.scheduler();
        let scheduler_b = kernel.scheduler();
        let bus_a = kernel.event_bus();
        let bus_b = kernel.event_bus();

        // Two `Arc<Scheduler>` clones point to the
        // same instance: `Arc::ptr_eq` is true.
        assert!(
            Arc::ptr_eq(&scheduler_a, &scheduler_b),
            "kernel.scheduler() should hand out Arcs to the same underlying Scheduler"
        );
        assert!(
            Arc::ptr_eq(&bus_a, &bus_b),
            "kernel.event_bus() should hand out Arcs to the same underlying EventBus"
        );
    }

    #[tokio::test]
    async fn kernel_clone_shares_underlying_state() {
        // Flow: a cloned `Kernel` is backed by the
        // same Scheduler and EventBus as the
        // original. Steps registered on the original
        // are visible to the clone, and events
        // published on one side land in subscriptions
        // made on the other.
        let (_dir, original) = fresh_kernel();
        let clone = original.clone();

        // Register a step on the original's
        // scheduler (the shared one).
        original
            .scheduler()
            .register::<Probe>(Arc::new(|_event, ctx, bus_handle| {
                let ctx = ctx.clone();
                Box::pin(async move {
                    // Publish a follow-up event with
                    // a known payload so the clone's
                    // subscriber can confirm it ran.
                    bus_handle
                        .publish(
                            super::event_bus_test_marker::ProbeAck {
                                from: "shared-step".into(),
                            },
                            ctx,
                        )
                        .await;
                    Ok(())
                })
            }));

        // Subscribe to the ProbeAck on the clone's
        // bus (the shared one).
        let mut acks = clone
            .event_bus()
            .subscribe::<super::event_bus_test_marker::ProbeAck>(16);

        // Ingest on the clone. Because the
        // Scheduler and EventBus are shared, the
        // step registered via the original's
        // scheduler will run.
        clone
            .runtime()
            .ingest(
                Probe {
                    payload: "go".into(),
                },
                TenantId::new("test-tenant"),
                Actor::Timer,
            )
            .await;

        // And the subscription on the clone's bus
        // receives the step's follow-up event.
        let (ack, _) = acks.recv().await.expect("ProbeAck");
        assert_eq!(ack.from, "shared-step");
    }

    #[tokio::test]
    async fn kernel_security_accessor_returns_a_shared_security_engine() {
        // Flow: `kernel.security()` hands out an
        // `Arc<dyn SecurityV1>`. A sealed secret on
        // the original is unsealable from the
        // clone, which proves the engine is shared
        // (not re-built per call).
        let (_dir, kernel) = fresh_kernel();
        let clone = kernel.clone();

        // Seal a secret on the original's engine.
        let secret_ref = kernel
            .security()
            .seal(b"hello-engine", "test-secret")
            .await
            .expect("seal should succeed on a fresh engine");

        // Unseal it on the clone's engine.
        let ctx = afa_contracts::ExecutionContext::new(TenantId::new("test-tenant"), Actor::Timer);
        let unsealed = clone
            .security()
            .unseal(&secret_ref, &ctx)
            .await
            .expect("unseal should succeed on a clone");

        assert_eq!(&*unsealed, b"hello-engine");
    }
}

/// Tiny test-only marker module so the Kernel clone
/// test above can name a follow-up event type without
/// putting a test-only `pub` item in `event_bus.rs`.
#[cfg(test)]
mod event_bus_test_marker {
    use afa_contracts::AfaEvent;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    pub struct ProbeAck {
        pub from: String,
    }

    impl AfaEvent for ProbeAck {}
}
