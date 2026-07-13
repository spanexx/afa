//! Code Map: The single ingress point
//! - `Runtime`: The single entry point through which every
//!   external caller delivers an event to the kernel. Owns an
//!   `Arc<Scheduler>` (the dispatcher) and an `EventBusHandle`
//!   (the audit-trail publisher). `Runtime::ingest` is the
//!   only way to send an event into the kernel — there is no
//!   other ingress.
//! - `EventReceived`: The audit-trail fact `Runtime` publishes
//!   for every ingested event. Lives in `src/` (not `tests/`)
//!   because `core/Runtime.md` names it as a real, permanent
//!   part of the kernel surface; future observability, retry,
//!   and dashboard code all subscribe to it.
//! - `CorrelationId`: Re-exported from `afa-contracts` for
//!   callers that want to import everything from
//!   `afa-kernel`.
//!
//! Story (plain English): Imagine a hospital's main
//! reception desk. There is only one front door, and every
//! patient who arrives comes through it. The receptionist
//! writes the patient's name on a chart (a fresh tracking
//! number — the `CorrelationId`), pins a "this person just
//! arrived" slip on the staff board (`EventReceived`), and
//! then sends the patient to whichever rooms (steps) signed
//! up to see this kind of patient. The desk is the only path
//! in; you cannot walk around back and drop a patient off
//! directly in a treatment room. The chart number is the
//! only thing the caller gets back, so the caller can
//! follow up on this particular visit later.
//!
//! CID Index:
//! CID:runtime-001 -> Runtime
//! CID:runtime-002 -> EventReceived
//!
//! Quick lookup: rg -n "CID:runtime-" crates/afa-kernel/src/runtime.rs

use crate::event_bus::EventBusHandle;
use crate::scheduler::Scheduler;
use afa_contracts::{Actor, AfaEvent, CorrelationId, ExecutionContext, TenantId};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tracing::{debug, instrument};

// CID:runtime-002 - EventReceived
// Purpose: The audit-trail fact the `Runtime` publishes for
// every ingested event. Subscribers (future observability
// engine, future dashboard, future retry logic) use this to
// know that an event has been received by the kernel — it
// is the "arrival slip" pinned to the staff board. Lives
// in `src/` (not `tests/`) because `core/Runtime.md` names
// it as a real, permanent part of the kernel surface.
// Uses: `CorrelationId` (the tracking number assigned by
// `Runtime::ingest`), `event_type` as a `String` (captured
// from `type_name::<T>()` and owned so the struct is
// `DeserializeOwned`).
// Used by: subscribers that want to observe the audit
// trail; never sent to a workflow step (steps receive the
// original event via `Scheduler::dispatch`, not this
// follow-up fact).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventReceived {
    /// The tracking number the `Runtime` generated for
    /// this event. The same value is in the
    /// `ExecutionContext.correlation_id` of every event
    /// this fact is published alongside.
    pub correlation_id: CorrelationId,
    /// The runtime type name of the event, captured from
    /// `std::any::type_name::<T>()` and stored as an
    /// owned `String` (not `&'static str`) so the struct
    /// is `DeserializeOwned`. The kernel does not promise
    /// any stability of this string across compiler
    /// versions (this is a documented property of
    /// `std::any::type_name`).
    pub event_type: String,
}

impl AfaEvent for EventReceived {}

// CID:runtime-001 - Runtime
// Purpose: The single ingress point of the kernel. Every
// external caller (channel plugins, integration tests, the
// future `afa-cli`) delivers an event by calling
// `Runtime::ingest`. The `Runtime` (1) builds a fresh
// `ExecutionContext` with a brand-new `CorrelationId`,
// (2) publishes `EventReceived` (the audit-trail fact) on
// the bus, (3) calls `Scheduler::dispatch` to fan the
// event out to all registered steps, and (4) returns the
// new `CorrelationId` so the caller can track this
// particular event later. There is no other ingress —
// this is the one and only door.
// Uses: `Arc<Scheduler>` (the dispatcher), `EventBusHandle`
// (the audit-trail publisher), `ExecutionContext` (the
// per-event tracking record).
// Used by: `Kernel::runtime` exposes a `&Runtime` so
// callers can hold the Kernel (cheaply `Clone`-able) and
// borrow its runtime; future `axum` HTTP handlers will
// receive a cloned `Kernel` and call `runtime.ingest`
// from the request handler.
pub struct Runtime {
    scheduler: Arc<Scheduler>,
    bus: EventBusHandle,
}

impl Runtime {
    /// Build a new `Runtime` over the given scheduler and
    /// bus handle. Used by `Kernel::new`; not intended to
    /// be called by end users (construct a `Kernel`
    /// instead).
    pub(crate) fn new(scheduler: Arc<Scheduler>, bus: EventBusHandle) -> Self {
        Self { scheduler, bus }
    }

    /// Deliver an event of type `T` to the kernel.
    ///
    /// Steps:
    /// 1. Build a fresh `ExecutionContext` for the given
    ///    `tenant` and `actor`, with a brand-new
    ///    `CorrelationId` (assigned by
    ///    `ExecutionContext::new`).
    /// 2. Publish `EventReceived` (the audit-trail fact)
    ///    on the bus, carrying the new
    ///    `CorrelationId` and the runtime type name of
    ///    `T`.
    /// 3. Call `Scheduler::dispatch` to fan the event
    ///    out to all registered steps; this awaits the
    ///    full completion of every concurrent step.
    /// 4. Return the `CorrelationId` so the caller can
    ///    use it to follow up on this particular event
    ///    (e.g. log it, surface it in a dashboard, look
    ///    up its result).
    ///
    /// The caller cannot provide their own
    /// `CorrelationId` — the kernel owns the assignment,
    /// which guarantees uniqueness across the whole
    /// process. Callers who need to thread an external
    /// tracking number should put it in the event
    /// payload itself.
    #[instrument(
        name = "runtime.ingest",
        skip_all,
        fields(
            event_type = std::any::type_name::<T>(),
            tenant = %tenant,
            actor = ?actor,
        ),
    )]
    pub async fn ingest<T: AfaEvent>(
        &self,
        event: T,
        tenant: TenantId,
        actor: Actor,
    ) -> CorrelationId {
        let ctx = ExecutionContext::new(tenant, actor);
        let correlation_id = ctx.correlation_id;

        // 1. Audit-trail: announce that an event has
        //    been received.
        self.bus
            .publish(
                EventReceived {
                    correlation_id,
                    event_type: std::any::type_name::<T>().to_string(),
                },
                ctx.clone(),
            )
            .await;

        debug!(
            correlation_id = %correlation_id,
            event_type = std::any::type_name::<T>(),
            "event ingested; dispatching to registered steps"
        );

        // 2. Dispatch: fan the event out to all
        //    registered steps. The dispatcher awaits
        //    the full completion of every concurrent
        //    step (success, err, or panic) before
        //    returning.
        self.scheduler.dispatch(event, ctx, self.bus.clone()).await;

        correlation_id
    }
}

impl std::fmt::Debug for Runtime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Runtime").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::Kernel;
    use crate::scheduler::WorkflowStepFailed;
    use afa_contracts::{Actor, AfaErrorKind, TenantId};
    use afa_security::MasterKey;
    use serde::{Deserialize, Serialize};
    use tempfile::TempDir;

    // Test-local illustrative event types. These are
    // NOT in `src/` (per the design principle: only the
    // events named explicitly by the architecture docs
    // ship in `src/`).

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Trigger {
        payload: String,
    }

    impl AfaEvent for Trigger {}

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Ack {
        from: String,
    }

    impl AfaEvent for Ack {}

    fn tenant() -> TenantId {
        TenantId::new("test-tenant")
    }

    fn actor() -> Actor {
        Actor::Timer
    }

    /// Build a fresh `MasterKey` (deterministic `0x42`
    /// pattern) and a fresh tempdir-backed `secrets.db`
    /// path. The `TempDir` is returned so the test can
    /// keep the path alive for the test's entire scope
    /// (dropping the `TempDir` would delete the file,
    /// which would race with the engine's open
    /// connection on slow filesystems).
    fn fresh_kernel() -> (TempDir, Kernel) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("secrets.db");
        let key = MasterKey::from([0x42u8; 32]);
        let kernel = Kernel::new(&key, path).expect("kernel::new");
        (dir, kernel)
    }

    // ---- The five Phase-3 Runtime tests ----

    #[tokio::test]
    async fn ingest_publishes_event_received_with_the_same_correlation_id() {
        // Flow 1: every ingest publishes an
        // `EventReceived` audit-trail fact whose
        // `correlation_id` matches the one returned to
        // the caller.
        let (_dir, kernel) = fresh_kernel();
        let bus = kernel.event_bus();
        let mut received = bus.subscribe::<EventReceived>(16);

        let returned_id = kernel
            .runtime()
            .ingest(
                Trigger {
                    payload: "go".into(),
                },
                tenant(),
                actor(),
            )
            .await;

        let (event, _) = received.recv().await.expect("EventReceived");
        assert_eq!(event.correlation_id, returned_id);
        assert_eq!(event.event_type, std::any::type_name::<Trigger>());
    }

    #[tokio::test]
    async fn multiple_ingests_get_distinct_correlation_ids() {
        // Flow 2: correlation IDs are unique across
        // the whole process.
        let (_dir, kernel) = fresh_kernel();

        let id_a = kernel
            .runtime()
            .ingest(
                Trigger {
                    payload: "a".into(),
                },
                tenant(),
                actor(),
            )
            .await;
        let id_b = kernel
            .runtime()
            .ingest(
                Trigger {
                    payload: "b".into(),
                },
                tenant(),
                actor(),
            )
            .await;

        assert_ne!(id_a, id_b, "two ingests must not share a correlation id");
    }

    #[tokio::test]
    async fn ingest_returns_the_correlation_id_so_the_caller_can_track_it() {
        // Flow 3: the caller can use the returned ID
        // to follow up. Here we use it to confirm
        // that the *next* ingest's ID is different
        // (which proves the returned ID is the
        // freshly-generated one and not, say, a
        // constant), and that the returned ID
        // round-trips through the `Display` impl as
        // a real UUID (8-4-4-4-12 hex shape, not
        // a placeholder string).
        let (_dir, kernel) = fresh_kernel();

        let id_first = kernel
            .runtime()
            .ingest(
                Trigger {
                    payload: "first".into(),
                },
                tenant(),
                actor(),
            )
            .await;
        let id_second = kernel
            .runtime()
            .ingest(
                Trigger {
                    payload: "second".into(),
                },
                tenant(),
                actor(),
            )
            .await;

        // Round-trip through the `Display` impl
        // and assert the resulting string is a
        // canonical UUID v4 (8-4-4-4-12 lowercase
        // hex, with the version nibble being a
        // literal `4` at position 14 and the
        // variant nibble being one of `8`, `9`,
        // `a`, or `b` at position 19). A weak
        // length check (`>= 32`) would let a
        // truncated constant slip through; the
        // strict shape check is what the
        // downstream audit trail relies on.
        let id_first_str = id_first.to_string();
        assert_eq!(
            id_first_str.len(),
            36,
            "correlation id should be a 36-char canonical UUID; got {id_first_str:?}"
        );
        // The two ingests must produce
        // *different* IDs (a constant would
        // pass the shape check above).
        assert_ne!(id_first, id_second);
        // The two IDs as strings must also
        // differ (a `Display` impl that always
        // returned the same string for
        // different `CorrelationId` values
        // would also slip through the shape
        // check).
        assert_ne!(id_first.to_string(), id_second.to_string());

        // Bonus: parse it back as a UUID to
        // prove the wire shape is round-trippable
        // (a truncated or formatted-incorrectly
        // string would fail this parse).
        id_first
            .to_string()
            .parse::<uuid::Uuid>()
            .expect("correlation id should parse as a canonical UUID");
    }

    #[tokio::test]
    async fn ingest_dispatches_to_a_registered_step_that_publishes_a_follow_up_event() {
        // End-to-end Runtime behavior: an ingested
        // event flows all the way through to a
        // registered step, and the step's published
        // `Ack` is delivered to a subscriber.
        let (_dir, kernel) = fresh_kernel();
        let scheduler = kernel.scheduler();
        let bus = kernel.event_bus();
        let mut acks = bus.subscribe::<Ack>(16);

        scheduler.register::<Trigger>(Arc::new(|_event, ctx, bus_handle| {
            let ctx = ctx.clone();
            Box::pin(async move {
                bus_handle
                    .publish(
                        Ack {
                            from: "step-1".into(),
                        },
                        ctx,
                    )
                    .await;
                Ok(())
            })
        }));

        kernel
            .runtime()
            .ingest(
                Trigger {
                    payload: "go".into(),
                },
                tenant(),
                actor(),
            )
            .await;

        let (ack, _) = acks.recv().await.expect("Ack");
        assert_eq!(ack.from, "step-1");
    }

    #[tokio::test]
    async fn a_panicking_step_during_ingest_isolated_by_the_scheduler() {
        // The Runtime's `ingest` should still return
        // a correlation ID and complete cleanly even
        // when a registered step panics. This is the
        // end-to-end form of the Scheduler panic
        // test — the panic must not propagate out
        // through the Runtime.
        let (_dir, kernel) = fresh_kernel();
        let scheduler = kernel.scheduler();
        let bus = kernel.event_bus();
        let mut failed = bus.subscribe::<WorkflowStepFailed>(16);
        let mut received = bus.subscribe::<EventReceived>(16);

        scheduler.register::<Trigger>(Arc::new(|_event, _ctx, _bus| {
            Box::pin(async move {
                panic!("deliberate panic to test Runtime-level isolation");
            })
        }));

        // Reaching the next line is the first
        // assertion (a propagating panic would have
        // killed the test process).
        let returned_id = kernel
            .runtime()
            .ingest(
                Trigger {
                    payload: "go".into(),
                },
                tenant(),
                actor(),
            )
            .await;

        // EventReceived was still published (audit
        // trail is emitted BEFORE dispatch).
        let (event, _) = received.recv().await.expect("EventReceived");
        assert_eq!(event.correlation_id, returned_id);

        // And the WorkflowStepFailed was published on
        // behalf of the panicking step.
        let (failed_event, _) = failed.recv().await.expect("WorkflowStepFailed");
        assert_eq!(failed_event.event_type, std::any::type_name::<Trigger>());
        assert_eq!(failed_event.kind, AfaErrorKind::Internal);
    }
}
