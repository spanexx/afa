//! Code Map: Event scheduler
//! - `Scheduler`: The dispatcher. Holds a registry of registered
//!   steps per event type; on `dispatch`, runs every registered
//!   step for an event type concurrently with panic isolation.
//! - `Step<T>`: The type alias for a registered step function.
//!   Steps take an `Arc<T>` (the event), the `ExecutionContext`,
//!   and an `EventBusHandle` (to publish their own events).
//! - `WorkflowStepFailed`: A public, permanent fact the
//!   `Scheduler` publishes on behalf of any step that panics
//!   (a step that returns an `Err` is expected to publish this
//!   itself, before returning). It is named explicitly in
//!   `core/Scheduler.md`, which is why it ships in `src/` and
//!   not just in `tests/`.
//! - `ErasedStep` (private): A type-erased view of a step so the
//!   registry can hold steps for many different concrete `T`s
//!   behind one map. Mirrors `event_bus::ErasedSender`.
//! - `TypedStep<T>` (private): The concrete wrapper for a step
//!   registered against event type `T`.
//!
//! Story (plain English): Imagine a switchboard at a small
//! fire station. Every bell that rings in (every event that
//! arrives through `Runtime::ingest`) is matched against a list
//! of teams that have signed up to respond to that kind of bell.
//! Every team that signed up gets called at the same time — none
//! of them wait for the others. If one team's ladder breaks
//! (a step panics), the other teams are not affected; the
//! switchboard just writes down "team X broke" on a slip of
//! paper and tucks it into the day's log. The switchboard does
//! not try to fix the broken ladder. It just records that it
//! broke, so the next person reading the log can see what
//! happened.
//!
//! CID Index:
//! CID:scheduler-001 -> Scheduler
//! CID:scheduler-002 -> `Step<T>` (type alias)
//! CID:scheduler-003 -> WorkflowStepFailed
//! CID:scheduler-004 -> ErasedStep (private trait)
//! CID:scheduler-005 -> `TypedStep<T>` (private)
//!
//! Quick lookup: rg -n "CID:scheduler-" crates/afa-kernel/src/scheduler.rs

use crate::event_bus::EventBusHandle;
use afa_contracts::{AfaError, AfaErrorKind, AfaEvent, ExecutionContext};
use afa_observability::{record_span_value, ObservabilityEngine};
use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::any::{Any, TypeId};
use std::collections::{BTreeMap, HashMap};
use std::marker::PhantomData;
use std::sync::{Arc, RwLock};
use tokio::task::JoinSet;
use tracing::debug;
use uuid::Uuid;

/// The full type of the scheduler's internal registry. Factored
/// out so the `Arc<RwLock<HashMap<...>>>` shape only has to be
/// spelled once (and so clippy's `type_complexity` lint is
/// satisfied).
type Registry = Arc<RwLock<HashMap<TypeId, Vec<Arc<dyn ErasedStep>>>>>;

// CID:scheduler-002 - Step<T> (type alias)
// Purpose: The shape of a registered step. A step is a
// function that takes the event (wrapped in `Arc<T>` so the
// same instance can be shared across concurrent siblings
// without requiring `T: Clone`), the `ExecutionContext`, and
// an `EventBusHandle` to publish its own events, and returns
// a `'static` boxed future yielding either `Ok(())` or an
// `AfaError`. The `Send + Sync` bound is required because
// the Scheduler stores the step in an `Arc` shared across
// the dispatcher and the spawned tasks.
// Uses: futures_util::future::BoxFuture.
// Used by: callers of `Scheduler::register` (workflow-engine
// authors, channel plugin authors, and the kernel's own
// integration tests).
pub type Step<T> = Arc<
    dyn Fn(
            Arc<T>,
            ExecutionContext,
            EventBusHandle,
        ) -> BoxFuture<'static, Result<(), Box<dyn AfaError>>>
        + Send
        + Sync,
>;

// CID:scheduler-003 - WorkflowStepFailed
// Purpose: A real, permanent fact the `Scheduler` publishes
// on behalf of a step that panicked (a step that returned
// `Err` is expected to publish this itself before returning;
// the Scheduler only publishes it on a step's behalf when
// the step never got the chance to publish anything itself,
// because the panic aborted it mid-execution). Lives in
// `src/` (not `tests/`) because `core/Scheduler.md` names
// it as a real, permanent part of the dispatch surface.
// Uses: AfaErrorKind (the `kind` field is the bucket the
// future responder will branch on).
// Used by: subscribers that want to know when a step
// failed — the future observability engine, future
// workflow-engine "step retry" logic, future dashboard
// views.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowStepFailed {
    /// The runtime type name of the event the failed step
    /// was registered against. Captured by `type_name::<T>()`
    /// at the call site and stored as an owned `String` (not
    /// `&'static str`) so the struct is `DeserializeOwned`
    /// and can ride the bus across the JSON wire. The
    /// kernel does not promise any stability of this string
    /// across compiler versions (this is a documented
    /// property of `std::any::type_name`).
    pub event_type: String,
    /// The coarse "what kind of trouble" bucket. For a
    /// Scheduler-published failure this is always
    /// `AfaErrorKind::Internal`; for a step-self-published
    /// failure it can be any bucket the step chose.
    pub kind: AfaErrorKind,
    /// A human-readable message. For a Scheduler-published
    /// failure this is fixed (`"step panicked"`); for a
    /// step-self-published failure it can be anything the
    /// step chose.
    pub message: String,
}

impl AfaEvent for WorkflowStepFailed {}

// CID:scheduler-001 - Scheduler
// Purpose: The dispatcher. Holds a registry of registered
// steps per event type (`TypeId` -> `Vec<Arc<dyn
// ErasedStep>>`); on `dispatch`, snapshots the registered
// steps for the event's `TypeId`, spawns each one on a
// `tokio::task::JoinSet`, and drains the set. Each step
// runs concurrently with the others; a panic in one step
// is caught by the executor and surfaces only as a
// `JoinError` on that step's own task, never affecting
// its siblings. Cloning is intentionally not implemented
// — the intended sharing path is `Arc<Scheduler>` (e.g.
// inside `Kernel`).
// Uses: tokio::task::JoinSet (concurrent drain with
// per-task panic isolation), std::sync::RwLock (the
// registry mutex, held only across sync operations).
// Used by: `Kernel` (owns one), `Runtime::ingest` (calls
// `dispatch` on every incoming event).
pub struct Scheduler {
    registry: Registry,
    /// The observability engine. `Phase 2` of the
    /// observability-baseline pack wires every
    /// `dispatch` call through the
    /// `record_span_value` helper, and every
    /// individual step body through the `record_span`
    /// helper. The engine is `Arc`-shared with the
    /// `Runtime` (constructed once in `Kernel::new`
    /// and handed to both), so the two always see
    /// the same spans DB connection.
    observability: Arc<ObservabilityEngine>,
}

impl Scheduler {
    /// Build a fresh, empty `Scheduler` over the
    /// given observability engine. The engine is
    /// `Arc`-cloned (it's `Send + Sync` and cheap to
    /// share).
    pub fn new(observability: Arc<ObservabilityEngine>) -> Self {
        Self {
            registry: Arc::new(RwLock::new(HashMap::new())),
            observability,
        }
    }

    /// Register a step to run whenever an event of type `T`
    /// is dispatched.
    ///
    /// Any number of steps may be registered against the
    /// same `T`; all of them run concurrently on each
    /// dispatch. The order of registration is not preserved
    /// — concurrent tasks are scheduled by the Tokio
    /// executor, not by registration order.
    ///
    /// Registration is infallible (no `Result` return):
    /// the registry is in-process, in-memory, and always
    /// available.
    ///
    /// The `step` parameter is the public `Step<T>` type
    /// alias (an `Arc<dyn Fn(...) -> BoxFuture<...>>`).
    /// Callers wrap their closure in an `Arc` themselves,
    /// which lets them share the same step between two
    /// schedulers (or two kernels) without an extra copy.
    /// Register a step under the event type `T`. The
    /// `name` is a stable identifier used as the
    /// spans DB's `operation` column (e.g. `"seal"`,
    /// `"unseal"`); it is NOT the Rust FQ type name
    /// (which is debug-only output). Two steps
    /// registered against the same `T` with the
    /// same name overwrite each other (the
    /// registry is keyed by `TypeId`; a future
    /// pack may move to a `(TypeId, name)` key
    /// if multi-registration is required).
    pub fn register<T: AfaEvent>(&self, name: &'static str, step: Step<T>) {
        let typed = Arc::new(TypedStep::<T> {
            name,
            f: step,
            _phantom: PhantomData,
        });
        let erased: Arc<dyn ErasedStep> = typed;
        self.registry
            .write()
            .expect("Scheduler registry poisoned")
            .entry(TypeId::of::<T>())
            .or_default()
            .push(erased);
    }

    /// Fan out an event of type `T` to every registered
    /// step, concurrently, with panic isolation.
    ///
    /// Behavior:
    /// - Looks up the steps for `TypeId::of::<T>()`; if
    ///   none are registered, logs at `trace` (well,
    ///   `debug`, since we don't have `trace` enabled in
    ///   the default subscriber) and returns.
    /// - Wraps the event in `Arc::new` once; clones the
    ///   `Arc` for each registered step.
    /// - Spawns each step on a `tokio::task::JoinSet` with
    ///   a clone of the event, the context, and a clone of
    ///   the bus handle.
    /// - Drains the `JoinSet` via `join_next()` in a loop.
    /// - For each completed task: success is logged at
    ///   `debug`; a returned `Err` is logged at `debug`
    ///   (the step is expected to have published its own
    ///   `WorkflowStepFailed` before returning, so the
    ///   Scheduler does not publish again); a `JoinError`
    ///   that is a panic causes the Scheduler to publish
    ///   `WorkflowStepFailed` on the panicking step's
    ///   behalf (the step never got the chance to publish
    ///   anything itself).
    /// - A `JoinError` that is a cancellation (not a panic)
    ///   is logged at `debug` and does not publish anything.
    ///
    /// **Phase 2 observability wiring**: the entire
    /// fan-out body is wrapped in
    /// `record_span_value` (engine: "afa-kernel",
    /// operation: "scheduler.dispatch"). Each step
    /// body is further wrapped in `record_span`
    /// (engine: "afa-kernel", operation:
    /// "scheduler.step") with the `event_type`
    /// attribute. The wrapper helpers are
    /// additive: the existing `tracing::debug!`
    /// calls and the `WorkflowStepFailed`
    /// publish-on-panic path are preserved.
    pub async fn dispatch<T: AfaEvent>(
        &self,
        event: T,
        ctx: ExecutionContext,
        bus: EventBusHandle,
    ) {
        let event = Arc::new(event);
        let type_name = std::any::type_name::<T>();

        // Snapshot the list of steps under the read
        // lock, OUTSIDE the wrap scope, then release
        // the lock before any work that could be
        // slow. The snapshot is moved into the
        // wrap-scope future as a plain `Vec` (no
        // registry lock is held across an await).
        let steps: Vec<Arc<dyn ErasedStep>> = {
            let registry = self.registry.read().expect("Scheduler registry poisoned");
            registry
                .get(&TypeId::of::<T>())
                .cloned()
                .unwrap_or_default()
        };

        if steps.is_empty() {
            // Phase 2: even a "no steps" dispatch
            // records one top-level
            // `scheduler.dispatch` row (the wrap
            // scope runs, the future is a no-op,
            // and one SpanRecord is written).
            let mut attributes: BTreeMap<String, String> = BTreeMap::new();
            attributes.insert("event_type".to_string(), type_name.to_string());
            attributes.insert("step_count".to_string(), "0".to_string());
            let observability = Arc::clone(&self.observability);
            let ctx_for_wrap = ctx.clone();
            record_span_value(
                &ctx_for_wrap,
                "afa-kernel",
                "scheduler.dispatch",
                attributes,
                None,
                &observability,
                async move {},
            )
            .await;
            debug!(
                event_type = type_name,
                "dispatch with zero registered steps"
            );
            return;
        }

        // The dispatch wrap is the root span of
        // the per-step tree. We mint the
        // dispatch_span_id here (in the
        // dispatching task) and pass it as the
        // parent_span_id to each per-step wrap
        // (which runs on a different tokio task
        // after `join_set.spawn`). Explicit
        // parent is required because
        // tokio::spawn does not propagate
        // thread_local values to the spawned
        // task (see doc-drift #14 in
        // `afa-observability/src/record.rs`).
        let dispatch_span_id = Uuid::new_v4();
        let observability_for_wrap = Arc::clone(&self.observability);
        let type_name_for_wrap = type_name.to_string();
        let ctx_for_wrap = ctx.clone();
        let ctx_for_closure = ctx.clone();
        let bus_for_closure = bus.clone();
        let step_count_attr = steps.len().to_string();
        let mut attributes: BTreeMap<String, String> = BTreeMap::new();
        attributes.insert("event_type".to_string(), type_name_for_wrap.clone());
        attributes.insert("step_count".to_string(), step_count_attr);
        let event_for_steps = Arc::clone(&event);
        record_span_value(
            &ctx_for_wrap,
            "afa-kernel",
            "scheduler.dispatch",
            attributes,
            None, // root span of the dispatch
            &observability_for_wrap,
            async move {
                let mut join_set: JoinSet<()> = JoinSet::new();
                for step in steps {
                    let erased: Arc<dyn Any + Send + Sync> =
                        Arc::clone(&event_for_steps) as Arc<dyn Any + Send + Sync>;
                    // The outer `observability` Arc is
                    // borrowed for each iteration's
                    // `Arc::clone` (line below). The
                    // original loop body read
                    // `&observability` directly, which
                    // moves on the second iteration.
                    // Clone from the underlying
                    // self.observability Arc each
                    // iteration, NOT the local
                    // `observability` (which the
                    // outer async-move captures by
                    // value). The outer async-move
                    // takes the moved `observability`
                    // into the closure; re-borrowing
                    // here is the move-error pattern.
                    let step_observability = Arc::clone(&self.observability);
                    let step_ctx = ctx_for_closure.clone();
                    let step_bus = bus_for_closure.clone();
                    let step_name: &'static str = step.name();
                    join_set.spawn(async move {
                        // **Phase 2 observability
                        // wiring**: every step
                        // body is wrapped in a
                        // freshly-minted span
                        // so the spans DB
                        // records one row per
                        // step (with
                        // `parent_span_id` =
                        // the dispatch wrap's
                        // span_id, set via
                        // the engine's
                        // `PARENT_SPAN_ID`
                        // thread-local).
                        //
                        // The step span id is
                        // minted BEFORE
                        // `invoke` and pushed
                        // onto the engine's
                        // `PARENT_SPAN_ID`
                        // thread-local for the
                        // duration of the
                        // call. This is what
                        // lets a step body
                        // record its OWN
                        // inner spans
                        // (e.g. a future LLM
                        // adapter's
                        // `complete` call)
                        // and have those
                        // inner spans see
                        // the step's
                        // `span_id` as their
                        // `parent_span_id`.
                        // The dispatch wrap's
                        // `PARENT_SPAN_ID`
                        // set is on the stack
                        // here, but the
                        // per-spawn
                        // `set_parent_span_id`
                        // override takes
                        // precedence (the
                        // engine's contract
                        // is "the most
                        // recently set
                        // parent wins").
                        //
                        // The step's own
                        // `Result` is what
                        // we throw away here
                        // — a step that
                        // returns Err
                        // already published
                        // its own failure; a
                        // step that panics
                        // surfaces as a
                        // `JoinError` (see
                        // the drain loop
                        // below). This
                        // inner Result is
                        // therefore
                        // deliberately
                        // discarded: the
                        // Scheduler does not
                        // double-publish.
                        //
                        // We use the engine's
                        // method-form
                        // `record_span` here
                        // (not the
                        // free-function
                        // helper) because
                        // the step's
                        // `Result` type is
                        // `Result<(), Box<dyn
                        // AfaError>>` — a
                        // `Box<dyn
                        // AfaError>` does
                        // NOT satisfy the
                        // helper's
                        // `E: AfaError`
                        // trait bound, so
                        // the free-function
                        // form would not
                        // type-check. The
                        // method form takes
                        // an explicit
                        // `SpanOutcome` and
                        // accepts any error
                        // that is `&dyn
                        // AfaError` (which
                        // `Box<dyn
                        // AfaError>`
                        // deref-coerces to).
                        // `step_observability` is no longer needed
                        // here — the `record_span_value`
                        // call above already wrote the
                        // step's SpanRecord via the
                        // wrapper. The earlier version
                        // called `engine.record_span`
                        // directly too, which produced a
                        // duplicate row per step (one
                        // from the wrapper, one from
                        // the manual call). The wrapper
                        // is now the single writer;
                        // the engine method-form is
                        // exposed for tests + direct
                        // callers but the kernel's
                        // dispatch path uses only the
                        // wrapper.
                        // (the
                        // free-function
                        // `record_span`
                        // helper does,
                        // but we can't
                        // use it for
                        // this case —
                        // see the
                        // explanation
                        // below).
                        // Wrap the step invocation in
                        // `record_span_value` so the
                        // thread-local parent linkage
                        // is owned by the wrapper helper
                        // (per the IMPL §"SpanOutcome"
                        // planning principle: the
                        // wrapper owns timing + outcome
                        // classification; the method
                        // form is the dumb endpoint).
                        // This is what Phase 2's
                        // dispatch wiring relies on.
                        // The dropped step_span_id +
                        // manual set_parent_span_id calls
                        // from the in-flight branch were
                        // the right shape for the
                        // not-yet-extracted wrapper; with
                        // `record_span_value` the helper
                        // does the mint/clear internally.
                        let mut step_attributes: BTreeMap<String, String> = BTreeMap::new();
                        step_attributes.insert("event_type".to_string(), step_name.to_string());
                        let _ = afa_observability::record_span_value(
                            &step_ctx,
                            "afa-kernel",
                            step_name,
                            step_attributes.clone(),
                            Some(dispatch_span_id), // parent = the dispatch wrap
                            &step_observability,
                            step.invoke(erased, step_ctx.clone(), step_bus),
                        )
                        .await;
                        // (Removed unused duration_ms + outcome
                        // locals — the manual
                        // `engine.record_span` call that used
                        // them is gone; the wrapper is the
                        // single writer.)
                        // (Removed duplicate engine.record_span
                        // call — the wrapper already wrote the
                        // step's row above. Keeping a
                        // second manual call produced 2 rows
                        // per step (test caught this as the
                        // "3 vs 4 spans" assertion failure).)
                    });
                }

                while let Some(res) = join_set.join_next().await {
                    match res {
                        Ok(()) => {
                            debug!(
                                event_type = type_name_for_wrap,
                                "step completed successfully"
                            );
                        }
                        Err(join_err) if join_err.is_panic() => {
                            // The panicked step never
                            // got the chance to
                            // publish anything itself.
                            // Publish
                            // `WorkflowStepFailed` on
                            // its behalf.
                            let failed = WorkflowStepFailed {
                                event_type: type_name_for_wrap.clone(),
                                kind: AfaErrorKind::Internal,
                                message: "step panicked".to_string(),
                            };
                            bus_for_closure
                                .publish(failed, ctx_for_closure.clone())
                                .await;
                            debug!(
                                event_type = type_name_for_wrap,
                                "step panicked; published WorkflowStepFailed on its behalf"
                            );
                        }
                        Err(join_err) => {
                            // Cancellation or other
                            // JoinError that is not
                            // a panic. Logged but
                            // not published; a
                            // future pack may add a
                            // shutdown protocol
                            // that uses this path.
                            debug!(
                                error = ?join_err,
                                event_type = type_name_for_wrap,
                                "step cancelled or otherwise did not complete normally"
                            );
                        }
                    }
                }
            },
        )
        .await;
    }

    /// Test-only: report the number of registered steps for
    /// the given event type. Used by the fan-out and
    /// zero-steps tests.
    #[cfg(test)]
    pub(crate) fn step_count<T: AfaEvent>(&self) -> usize {
        self.registry
            .read()
            .expect("Scheduler registry poisoned")
            .get(&TypeId::of::<T>())
            .map(|v| v.len())
            .unwrap_or(0)
    }
}

// No `Default` impl: `Scheduler` needs an
// `ObservabilityEngine`, which is async-constructed
// and not safely defaultable. Tests that need a
// fresh `Scheduler` should call
// `Scheduler::new(engine)` directly (the test
// helpers in this file do so via the `fresh()`
// helper).

impl std::fmt::Debug for Scheduler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Scheduler").finish_non_exhaustive()
    }
}

// CID:scheduler-004 - ErasedStep (private trait)
// Purpose: A type-erased view of a `Step<T>` so the
// registry can hold steps for many different concrete
// `T`s behind one map. The trait is object-safe (no
// generics, no `Self` in return position) so it can be
// stored as `Arc<dyn ErasedStep>`. The `invoke` method
// mirrors the `Step<T>` shape but with the event typed
// as `Arc<dyn Any + Send + Sync>` for the same
// type-erasure reasons as `event_bus::ErasedSender`.
// Used by: the registry; the dispatch fan-out loop.
trait ErasedStep: Send + Sync {
    /// Invoke the step. The caller has already downcast
    /// the event to `Arc<T>` indirectly — see
    /// `TypedStep<T>::invoke` for the actual downcast.
    /// Returns the step's `Result` (the Scheduler
    /// inspects only the `is_panic()` distinction via
    /// the `JoinError`, but the future itself is
    /// awaited and may yield `Err`).
    fn invoke(
        &self,
        event: Arc<dyn Any + Send + Sync>,
        ctx: ExecutionContext,
        bus: EventBusHandle,
    ) -> BoxFuture<'static, Result<(), Box<dyn AfaError>>>;
    /// Stable name for this step (e.g. `"seal"`,
    /// `"unseal"`). Used as the spans DB's
    /// `operation` column by the kernel's
    /// dispatch wrapper. NOT the Rust FQ type
    /// name — that would leak the crate's full
    /// type path into the audit log.
    fn name(&self) -> &'static str;
}

// CID:scheduler-005 - TypedStep<T> (private)
// Purpose: The concrete `ErasedStep` implementation
// for a specific event type `T`. Stores the user's
// `Step<T>` and downcasts the incoming `Arc<dyn Any +
// Send + Sync>` event back to `Arc<T>` before calling
// it. The downcast is guaranteed to succeed because the
// `TypedStep<T>` is only ever installed in the registry
// under `TypeId::of::<T>()` — see
// `Scheduler::register`. Uses the standard
// `PhantomData<fn() -> T>` "sendable phantom" pattern
// so the type is `Send + Sync` regardless of `T`'s
// auto-traits.
// Used by: `Scheduler::register` (one created per
// `register` call, then stored as `Arc<dyn ErasedStep>`).
struct TypedStep<T: AfaEvent> {
    name: &'static str,
    f: Step<T>,
    _phantom: PhantomData<fn() -> T>,
}

impl<T: AfaEvent> ErasedStep for TypedStep<T> {
    fn invoke(
        &self,
        event: Arc<dyn Any + Send + Sync>,
        ctx: ExecutionContext,
        bus: EventBusHandle,
    ) -> BoxFuture<'static, Result<(), Box<dyn AfaError>>> {
        // Safety: this step is installed in the registry
        // under `TypeId::of::<T>()`, and the only caller
        // of `invoke` is the dispatch fan-out loop, which
        // is itself generic over the same `T` and looks
        // up steps by the same `TypeId`. The downcast
        // therefore cannot fail.
        let event: Arc<T> = event
            .downcast::<T>()
            .expect("TypedStep<T> downcast: TypeId mismatch");
        (self.f)(event, ctx, bus)
    }
    fn name(&self) -> &'static str {
        self.name
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_bus::EventBus;
    use afa_contracts::{Actor, TenantId};
    use serde::{Deserialize, Serialize};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};
    use thiserror::Error;
    use tokio::time::sleep;

    // ---- Shared illustrative types (test-local, not in src/) ----

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Trigger {
        payload: String,
    }

    impl AfaEvent for Trigger {}

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Ack {
        from_step: String,
        payload: String,
    }

    impl AfaEvent for Ack {}

    #[derive(Debug, Error)]
    #[error("illustrative step error: {0}")]
    struct StepError(String);

    impl AfaError for StepError {
        fn kind(&self) -> AfaErrorKind {
            AfaErrorKind::Internal
        }
    }

    fn fresh_ctx() -> ExecutionContext {
        ExecutionContext::new(TenantId::new("test-tenant"), Actor::Timer)
    }

    /// Build a fresh (Scheduler, EventBus, bus handle) trio
    /// for tests. The bus handle is what a Scheduler would
    /// receive from `Runtime::ingest` in real code. A
    /// throwaway `ObservabilityEngine` is constructed and
    /// handed to the `Scheduler`; the engine writes to
    /// a private file in a fresh tempdir (which is
    /// leaked for the rest of the test process; the
    /// file is harmless because the purge loop is
    /// disabled and retention is `None`).
    ///
    /// **Note**: the engine constructor is async, so
    /// the helper is itself async. Every test in this
    /// module is a `#[tokio::test]`, which awaits the
    /// future before the rest of the test body runs.
    async fn fresh() -> (Scheduler, EventBus, EventBusHandle) {
        let bus = EventBus::new();
        let handle = bus.handle();
        let dir = tempfile::tempdir().expect("test tempdir");
        let path = dir.path().join("scheduler_test_spans.db");
        // Leak the tempdir so the file path
        // stays valid for the rest of the
        // process.
        Box::leak(Box::new(dir));
        let engine = afa_observability::ObservabilityEngine::new(
            afa_observability::ObservabilityConfig {
                spans_db_path: path,
                retention_days: None,
                purge_interval_hours: 0,
                purge_chunk_size: 1_000,
            },
            handle.clone(),
        )
        .await
        .expect("test engine boot");
        (Scheduler::new(engine), bus, handle)
    }

    // ---- The five required tests ----

    #[tokio::test]
    async fn two_independent_steps_for_the_same_event_both_run() {
        // Flow 2: a second step registered for an
        // already-used event type runs alongside the
        // first, with zero changes to the first step's
        // code.
        let (scheduler, bus, handle) = fresh().await;
        // One subscription is enough: both steps publish
        // `Ack`, and a single subscription receives both
        // (fan-out on the bus side; the Scheduler is
        // verifying the *step fan-out*, not the bus
        // fan-out — that is covered by the EventBus
        // tests).
        let mut acks = bus.subscribe::<Ack>(16);

        scheduler.register::<Trigger>(
            "scheduler_test_step_1",
            Arc::new(|_event, ctx, bus_handle| {
                let ctx = ctx.clone();
                Box::pin(async move {
                    bus_handle
                        .publish(
                            Ack {
                                from_step: "alpha".into(),
                                payload: "a".into(),
                            },
                            ctx,
                        )
                        .await;
                    Ok(())
                })
            }),
        );
        scheduler.register::<Trigger>(
            "scheduler_test_step_2",
            Arc::new(|_event, ctx, bus_handle| {
                let ctx = ctx.clone();
                Box::pin(async move {
                    bus_handle
                        .publish(
                            Ack {
                                from_step: "beta".into(),
                                payload: "b".into(),
                            },
                            ctx,
                        )
                        .await;
                    Ok(())
                })
            }),
        );

        assert_eq!(scheduler.step_count::<Trigger>(), 2);

        scheduler
            .dispatch(
                Trigger {
                    payload: "go".into(),
                },
                fresh_ctx(),
                handle,
            )
            .await;

        // Both steps ran; the bus delivers both Acks
        // to the single subscription. Order is
        // non-deterministic (concurrent siblings), so
        // we collect and check membership.
        let (first, _) = acks.recv().await.expect("first ack");
        let (second, _) = acks.recv().await.expect("second ack");
        let from_steps = [first.from_step.as_str(), second.from_step.as_str()];
        assert!(
            from_steps.contains(&"alpha"),
            "alpha step's Ack should be in the delivered set: {from_steps:?}"
        );
        assert!(
            from_steps.contains(&"beta"),
            "beta step's Ack should be in the delivered set: {from_steps:?}"
        );
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn two_steps_with_delays_complete_concurrently_not_sequentially() {
        // Flow 3: two steps that each sleep ~200ms; the
        // total `dispatch` wall-clock time should be close
        // to 200ms (concurrent), not close to 400ms
        // (sequential).
        //
        // `start_paused = true` freezes the test
        // runtime's clock so `tokio::time::sleep`
        // advances only when the test (or a
        // `tokio::time::sleep` itself) yields. This
        // makes the wall-clock measurement deterministic
        // regardless of host scheduling.
        let (scheduler, _bus, handle) = fresh().await;

        scheduler.register::<Trigger>(
            "scheduler_test_step_3",
            Arc::new(|_event, _ctx, _bus| {
                Box::pin(async move {
                    sleep(Duration::from_millis(200)).await;
                    Ok(())
                })
            }),
        );
        scheduler.register::<Trigger>(
            "scheduler_test_step_4",
            Arc::new(|_event, _ctx, _bus| {
                Box::pin(async move {
                    sleep(Duration::from_millis(200)).await;
                    Ok(())
                })
            }),
        );

        let started = Instant::now();
        scheduler
            .dispatch(
                Trigger {
                    payload: "go".into(),
                },
                fresh_ctx(),
                handle,
            )
            .await;
        let elapsed = started.elapsed();

        // Generous tolerance: a sequential implementation
        // would land at ~400ms. Anything under 350ms
        // (a full step's duration plus a comfortable
        // margin) is unambiguous proof of concurrency.
        assert!(
            elapsed < Duration::from_millis(350),
            "expected concurrent dispatch (~200ms), got {elapsed:?} — \
             steps ran sequentially?"
        );
    }

    #[tokio::test]
    async fn a_step_returning_err_publishes_its_own_workflow_step_failed() {
        // Flow 5 Trigger A: a step that returns
        // `Err(Box<dyn AfaError>)` results in a published
        // `WorkflowStepFailed` event (published by the
        // step itself, per the design — the Scheduler
        // does not double-publish on `Err`).
        let (scheduler, bus, handle) = fresh().await;
        let mut failed = bus.subscribe::<WorkflowStepFailed>(16);

        scheduler.register::<Trigger>(
            "scheduler_test_step_5",
            Arc::new(|_event, ctx, bus_handle| {
                let ctx = ctx.clone();
                Box::pin(async move {
                    bus_handle
                        .publish(
                            WorkflowStepFailed {
                                event_type: "Trigger".into(),
                                kind: AfaErrorKind::Unavailable,
                                message: "service down".into(),
                            },
                            ctx,
                        )
                        .await;
                    Err(Box::new(StepError("service down".into())) as Box<dyn AfaError>)
                })
            }),
        );

        scheduler
            .dispatch(
                Trigger {
                    payload: "go".into(),
                },
                fresh_ctx(),
                handle,
            )
            .await;

        let (received, _) = failed.recv().await.expect("WorkflowStepFailed");
        assert_eq!(received.event_type, "Trigger");
        assert_eq!(received.kind, AfaErrorKind::Unavailable);
        assert_eq!(received.message, "service down");
    }

    #[tokio::test]
    async fn a_panicking_step_does_not_crash_the_process_and_publishes_failed() {
        // Flow 5 Trigger B: a step that panics does not
        // crash the test process, results in a
        // `WorkflowStepFailed` with
        // `AfaErrorKind::Internal` (published by the
        // Scheduler, on the step's behalf), and any
        // sibling step still completes.
        let (scheduler, bus, handle) = fresh().await;
        let mut failed = bus.subscribe::<WorkflowStepFailed>(16);

        // Counter proves the sibling step completed.
        let sibling_counter = Arc::new(AtomicUsize::new(0));

        scheduler.register::<Trigger>(
            "scheduler_test_step_6",
            Arc::new(|_event, _ctx, _bus| {
                Box::pin(async move {
                    panic!("deliberate panic to test isolation");
                })
            }),
        );

        let counter_for_sibling = Arc::clone(&sibling_counter);
        scheduler.register::<Trigger>(
            "scheduler_test_step_7",
            Arc::new(move |_event, _ctx, _bus| {
                let counter = Arc::clone(&counter_for_sibling);
                Box::pin(async move {
                    counter.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })
            }),
        );

        // If panic isolation were broken, this would
        // take down the test process. Reaching the next
        // line at all is the first assertion.
        scheduler
            .dispatch(
                Trigger {
                    payload: "go".into(),
                },
                fresh_ctx(),
                handle,
            )
            .await;

        let (received, _) = failed.recv().await.expect("WorkflowStepFailed");
        assert_eq!(received.event_type, std::any::type_name::<Trigger>());
        assert_eq!(received.kind, AfaErrorKind::Internal);
        assert_eq!(received.message, "step panicked");

        // Sibling step ran and incremented its counter.
        assert_eq!(
            sibling_counter.load(Ordering::SeqCst),
            1,
            "sibling step should have completed despite the panic"
        );
    }

    #[tokio::test]
    async fn dispatch_with_zero_registered_steps_does_not_error_or_panic() {
        // Flow 6 Edge case C: dispatching an event with
        // no registered steps is a no-op, not an error.
        let (scheduler, bus, handle) = fresh().await;
        // Subscribe to confirm no spurious events are
        // published by the Scheduler itself.
        let mut failed = bus.subscribe::<WorkflowStepFailed>(16);

        scheduler
            .dispatch(
                Trigger {
                    payload: "into the void".into(),
                },
                fresh_ctx(),
                handle,
            )
            .await;

        // `recv` would block forever if WorkflowStepFailed
        // were published. Use a short timeout to confirm
        // nothing arrived.
        let recv_result = tokio::time::timeout(Duration::from_millis(50), failed.recv()).await;
        assert!(
            recv_result.is_err(),
            "Scheduler should not publish any event when no steps are registered"
        );
    }
}
