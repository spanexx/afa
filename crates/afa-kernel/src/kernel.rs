//! Code Map: The top-level composition
//! - `Kernel`: The top-level composition that owns the
//!   `Runtime`, the `Arc<Scheduler>`, and the `Arc<EventBus>`,
//!   all wired together. Cloning is cheap (every field is
//!   `Arc`-backed). Constructed via `Kernel::new()` with
//!   sensible defaults; future constructors (e.g.
//!   `Kernel::with_observability`) will layer in extra
//!   behavior without breaking the cheap-clone contract.
//!
//! Story (plain English): Imagine the front desk of a
//! small post office. The desk is the `Runtime` (the only
//! place a letter can be dropped off). Behind the desk is
//! the sorting room (the `Scheduler`) and the mail
//! shelves (`EventBus`). The post office as a whole
//! (the `Kernel`) is just a clean way to say "all three
//! of those, wired together." Several tellers at
//! different counters can each have their own copy of
//! the post office — but they all share the same mail
//! shelves and the same sorting room, so a letter
//! dropped at one counter lands in exactly the same
//! boxes as a letter dropped at any other.
//!
//! CID Index:
//! CID:kernel-001 -> Kernel
//!
//! Quick lookup: rg -n "CID:kernel-" crates/afa-kernel/src/kernel.rs

use crate::event_bus::{EventBus, EventBusHandle};
use crate::runtime::Runtime;
use crate::scheduler::Scheduler;
use std::sync::Arc;

// CID:kernel-001 - Kernel
// Purpose: The top-level composition. Owns the
// `Runtime`, the `Arc<Scheduler>`, and the
// `Arc<EventBus>`, all wired together so a single
// `Kernel::new()` call gives you a working kernel.
// Cloning a `Kernel` is cheap because every field is
// `Arc`-backed; this is the intended sharing pattern
// (e.g. one `Kernel` per `axum` request handler, each
// of which calls `runtime.ingest`).
// Uses: `Arc<Scheduler>`, `Arc<EventBus>`, `Runtime`.
// Used by: every consumer of the kernel; this is the
// type most callers will hold and pass around.
pub struct Kernel {
    runtime: Runtime,
    scheduler: Arc<Scheduler>,
    event_bus: Arc<EventBus>,
}

impl Kernel {
    /// Build a fresh, empty `Kernel`. Wires together a
    /// new `Scheduler` and a new `EventBus`; the
    /// `Runtime` is built over a `Clone` of each so
    /// every accessor (`runtime`, `scheduler`,
    /// `event_bus`) sees the same shared instances.
    pub fn new() -> Self {
        let scheduler = Arc::new(Scheduler::new());
        let event_bus = Arc::new(EventBus::new());
        let runtime = Runtime::new(Arc::clone(&scheduler), event_bus.handle());
        Self {
            runtime,
            scheduler,
            event_bus,
        }
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
}

impl Default for Kernel {
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for Kernel {
    /// Cheaply clone the kernel. Every field is
    /// `Arc`-backed, so this is just a few refcount
    /// bumps — no registry copy, no bus copy, no
    /// runtime copy. The two clones share the exact
    /// same underlying `Scheduler` and `EventBus`;
    /// steps registered on one are immediately
    /// visible to the other.
    fn clone(&self) -> Self {
        Self {
            runtime: Runtime::new(Arc::clone(&self.scheduler), self.event_bus.handle()),
            scheduler: Arc::clone(&self.scheduler),
            event_bus: Arc::clone(&self.event_bus),
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
    use serde::{Deserialize, Serialize};

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
        let kernel = Kernel::new();
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
        let kernel = Kernel::new();
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
        let original = Kernel::new();
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
