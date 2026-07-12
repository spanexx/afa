//! Code Map: End-to-end integration test (the "smoke test")
//! - `the_full_pipeline_works_from_a_downstream_consumer_view`:
//!   The one and only integration test in the kernel crate.
//!   Exercises the entire public API of the `Kernel` from the
//!   perspective of a downstream consumer (a future channel
//!   plugin, a future `axum` HTTP handler, a future
//!   integration-test in another pack). Lives in `tests/`
//!   (not in `mod tests` of any source file) so the kernel's
//!   public API is exercised without any `pub(crate)`
//!   privileges.
//!
//! Story (plain English): Imagine opening a brand-new post
//! office for the first time. You hire a teller, you set up
//! the sorting room, you write the rules on the wall. The
//! smoke test is the moment you hand a real letter to the
//! teller, watch it move through the sorting room, watch
//! the right staff member pick it up, watch the response
//! letter get put back on the shelf, and watch the customer
//! at the next window pick it up. If any single step in
//! that chain breaks, the smoke test fails — and the post
//! office is not ready to open.

use afa_contracts::{Actor, AfaEvent, TenantId};
use afa_kernel::runtime::EventReceived;
use afa_kernel::scheduler::WorkflowStepFailed;
use afa_kernel::{Kernel, Step};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::time::timeout;

// Two test-local illustrative event types. These are
// intentionally NOT in `src/` (per the design
// principle: only events named explicitly by the
// architecture docs ship in `src/`).

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Trigger {
    payload: String,
}

impl AfaEvent for Trigger {}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Ack {
    from: String,
    saw_payload: String,
    saw_correlation_id: afa_contracts::CorrelationId,
}

impl AfaEvent for Ack {}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_full_pipeline_works_from_a_downstream_consumer_view() {
    // 1. The downstream consumer (this test)
    //    constructs a `Kernel` with `Kernel::new()`.
    //    No `pub(crate)` accessors are touched
    //    anywhere in this file.
    let kernel = Kernel::new();

    // 2. The consumer subscribes to the audit-trail
    //    fact (`EventReceived`) and to the follow-up
    //    event the registered step will publish
    //    (`Ack`).
    let bus = kernel.event_bus();
    let mut audit = bus.subscribe::<EventReceived>(16);
    let mut acks = bus.subscribe::<Ack>(16);

    // 3. The consumer registers a step on the
    //    shared scheduler. The step is the type
    //    alias `Step<Trigger>` exported from
    //    `afa_kernel` — i.e. the consumer is
    //    using the *public* step shape, not a
    //    private trait.
    let step: Step<Trigger> = Arc::new(|event, ctx, bus_handle| {
        let ctx = ctx.clone();
        let payload = event.payload.clone();
        let cid = ctx.correlation_id;
        Box::pin(async move {
            // The step publishes a follow-up Ack
            // carrying the correlation ID it saw in
            // its ExecutionContext, so the test can
            // confirm the dispatch wired the
            // context through.
            bus_handle
                .publish(
                    Ack {
                        from: "consumer-step".into(),
                        saw_payload: payload,
                        saw_correlation_id: cid,
                    },
                    ctx,
                )
                .await;
            Ok(())
        })
    });
    kernel.scheduler().register::<Trigger>(step);

    // 4. The consumer ingests the trigger event
    //    via the single ingress point
    //    (`Runtime::ingest`). It receives back a
    //    correlation ID.
    let started = Instant::now();
    let returned_id = kernel
        .runtime()
        .ingest(
            Trigger {
                payload: "hello".into(),
            },
            TenantId::new("smoke-tenant"),
            Actor::Timer,
        )
        .await;
    let elapsed = started.elapsed();

    // 5. The audit-trail `EventReceived` was
    //    published, and its correlation ID
    //    matches the one returned to the
    //    caller.
    let audit_event = timeout(Duration::from_secs(2), audit.recv())
        .await
        .expect("audit EventReceived should arrive within 2s")
        .expect("EventReceived should be Some");
    let (audit_payload, _) = audit_event;
    assert_eq!(audit_payload.correlation_id, returned_id);
    assert_eq!(audit_payload.event_type, std::any::type_name::<Trigger>());

    // 6. The follow-up `Ack` arrived, and the
    //    correlation ID the step saw in its
    //    ExecutionContext matches the one
    //    returned to the caller (proves the
    //    dispatch wired the context through).
    let ack_event = timeout(Duration::from_secs(2), acks.recv())
        .await
        .expect("Ack should arrive within 2s")
        .expect("Ack should be Some");
    let (ack_payload, _) = ack_event;
    assert_eq!(ack_payload.from, "consumer-step");
    assert_eq!(ack_payload.saw_payload, "hello");
    assert_eq!(ack_payload.saw_correlation_id, returned_id);

    // 7. The whole round-trip was effectively
    //    instant (paused clock; the real
    //    assertion is that it *did* complete,
    //    not that it was fast).
    assert!(
        elapsed < Duration::from_secs(1),
        "full pipeline should complete well under 1s; got {elapsed:?}"
    );
}

#[tokio::test]
async fn a_panicking_step_does_not_break_the_smoke_test_pipeline() {
    // Companion smoke test: a registered step
    // that panics must not propagate out through
    // `Runtime::ingest` and must result in a
    // `WorkflowStepFailed` fact on the bus.
    let kernel = Kernel::new();
    let bus = kernel.event_bus();
    let mut failed = bus.subscribe::<WorkflowStepFailed>(16);
    let mut audit = bus.subscribe::<EventReceived>(16);

    kernel
        .scheduler()
        .register::<Trigger>(Arc::new(|_event, _ctx, _bus| {
            Box::pin(async move {
                panic!("deliberate panic in smoke test");
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
            TenantId::new("smoke-tenant"),
            Actor::Timer,
        )
        .await;

    // Audit trail still arrived.
    let (event, _) = timeout(Duration::from_secs(2), audit.recv())
        .await
        .expect("audit EventReceived")
        .expect("EventReceived");
    assert_eq!(event.correlation_id, returned_id);

    // WorkflowStepFailed was published on behalf
    // of the panicking step.
    let (failed_event, _) = timeout(Duration::from_secs(2), failed.recv())
        .await
        .expect("WorkflowStepFailed")
        .expect("WorkflowStepFailed");
    assert_eq!(failed_event.event_type, std::any::type_name::<Trigger>());
    assert_eq!(failed_event.kind, afa_contracts::AfaErrorKind::Internal);
}
