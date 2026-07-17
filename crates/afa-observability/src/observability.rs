//! Code Map: observability::observability (the engine)
//!
//! - ObservabilityConfig: the boot-time configuration
//!   the kernel hands to ObservabilityEngine::new.
//!   Holds the spans DB path, the retention window
//!   (None = never purge), the purge tick interval
//!   in hours (0 = disable the loop), and the row
//!   chunk size for the (future) chunked DELETE.
//! - ObservabilityEngine: the engine struct
//!   itself. Owns the Arc`Storage` for the spans
//!   DB, the EventBusHandle for publishing the
//!   three audit facts, the AtomicU64 drop counter
//!   (shared with the purge loop's hourly
//!   reset), the config snapshot, and an optional
//!   JoinHandle for the background purge loop.
//! - impl ObservabilityEngine { new, record_span,
//!   drops_in_last_hour, storage, config,
//!   abort_purge_for_test }: the public + crate-
//!   internal API. record_span is the method form
//!   the wrapper helper (record.rs) calls into on
//!   every wrapped future's close.
//! - run_purge_loop: the background task spawned by
//!   new() when retention_days.is_some() and
//!   purge_interval_hours > 0. Fires every
//!   purge_interval_hours, runs run_one_purge
//!   (from purge.rs), resets the drop counter.
//! - run_one_purge_for_loop: the per-tick body of
//!   the loop. Private (not exported; tests call
//!   purge::run_one_purge directly with explicit
//!   retention_days).
//! - current_span_id / set_parent_span_id: the
//!   thread-local helpers that make nested span
//!   linkage work. The free-function record_span
//!   pushes the parent span_id on entry and clears
//!   on exit; the engine's record_span method
//!   reads it to populate SpanRecord::parent_span_id
//!   when called inside a wrapper scope.
//! - truncate_reason: the 1 KiB cap helper for the
//!   SpansWriteFailed event payload (the contract
//!   type's reason field is bounded at 1 KiB by the
//!   IMPL §Phase 0).
//! - timer_ctx / empty_correlation_id: helpers used
//!   by purge.rs to synthesise a timer-driven
//!   ExecutionContext and a fresh correlation_id for
//!   each purge task.
//!
//! Story (plain English): The recording nurse's
//! workbench. When the hospital opens for the day
//! (boot), she walks in, opens her logbook on the
//! shelf (Storage::open), files any index cards
//! that came in overnight (MIGRATIONS), and starts
//! waiting for the doctor. Every time the doctor
//! finishes a patient visit (record_span is
//! called), she writes a one-line summary on the
//! chart and files it in the logbook (the
//! `write_span` INSERT). If the page is too full to
//! write a new line (the storage write fails), she
//! drops the entry, stamps "I dropped this one" on
//! her own pad (the SpansWriteFailed event), and
//! ticks the "drops this hour" tally (the
//! AtomicU64). Her self-check at shift-change
//! (the HealthCheck impl in health.rs) reports the
//! tally to the supervisor. The retention clerk
//! (`run_purge_loop`) shows up every hour, asks
//! "what's the retention rule?", crosses out
//! anything older, stamps "purged N entries" on
//! her own pad (the SpansPurged event), and zeroes
//! the drops tally for the next hour.
//!
//! Doc drift corrections vs. the IMPL draft:
//! - #5: HealthCheck is sync (see health.rs).
//! - #6: SpanOutcome::Err uses e.to_string() (no
//!   e.reason() method on AfaError).
//! - #7: event_bus.publish returns (), not Result.
//! - #8: SpanRecord is NOT published on the bus (it
//!   is not an AfaEvent -- only the three audit
//!   facts are).
//! - #11: The free-fn record_span signature is
//!   `F: Future<Output = Result<T, E>>` where
//!   `E: AfaError`, not `Result<T, Box<dyn
//!   AfaError>>` (the IMPL's signature would force
//!   AsyncFnOnce-style unstable bounds).
//!
//! CID Index:
//! CID:afa-observability-observability-001 -> ObservabilityConfig
//! CID:afa-observability-observability-002 -> ObservabilityEngine
//! CID:afa-observability-observability-003 -> new
//! CID:afa-observability-observability-004 -> record_span
//! CID:afa-observability-observability-005 -> run_purge_loop
//!
//! Quick lookup: rg -n "CID:afa-observability-observability-" crates/afa-observability/src/observability.rs

use crate::error::ObservabilityError;
use crate::persistence;
use afa_bus::EventBusHandle;
use afa_contracts::{SpanOutcome, SpanRecord};
use afa_storage::{migrate, open, Storage};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

// CID:afa-observability-observability-001 - ObservabilityConfig
// Purpose: The boot-time configuration the kernel
// hands to ObservabilityEngine::new. Cloned
// cheaply; mostly read by run_one_purge (which
// reads `retention_days`, `purge_interval_hours`,
// and `purge_chunk_size`).
//
// **Field semantics**:
// - spans_db_path: the on-disk path for the spans
//   SQLite file. Every engine has its own Storage
//   instance pointing at its own file (Phase 7a
//   may unify the security + observability DBs on
//   a single file, but in Phase 1 every engine
//   ships its own file).
// - retention_days: None means "never purge"
//   (the IMPL §Phase 1 `retention_null_no_op`
//   test). Some(n) means delete any row whose
//   started_at is more than n days old. The
//   background loop is spawned only when
//   retention_days.is_some() AND
//   purge_interval_hours > 0 (both conditions
//   needed; see new()).
// - purge_interval_hours: 0 disables the loop
//   (used by tests that want to drive
//   run_one_purge manually). Default is 1 (every
//   hour). The loop's first tick fires after one
//   interval, NOT on boot (the IMPL does not
//   specify boot behaviour; not-purging-on-boot is
//   the safe default).
// - purge_chunk_size: rows per DELETE transaction.
//   Phase 1 ignores this field (the DELETE is a
//   single transaction). A future pack can split
//   the DELETE into chunks of `purge_chunk_size`
//   when the spans table grows into the
//   millions.
//
// Used by: ObservabilityEngine::new (every field),
// run_purge_loop / run_one_purge_for_loop
// (retention_days + purge_interval_hours).
#[derive(Clone, Debug)]
pub struct ObservabilityConfig {
    pub spans_db_path: PathBuf,
    pub retention_days: Option<u32>,
    pub purge_interval_hours: u64,
    pub purge_chunk_size: u32,
}

impl ObservabilityConfig {
    // CID:afa-observability-observability-006 - with_default_retention
    // Purpose: The "ship the defaults" constructor.
    // 7-day retention, hourly purge, 10,000 rows
    // per chunk.
    //
    // Used by: Phase 2's KernelConfig -> ObservabilityConfig
    // conversion (the kernel's KernelConfig is the
    // source of every field; this helper is a
    // convenience for tests + future minimal-config
    // callers).
    pub fn with_default_retention(spans_db_path: PathBuf) -> Self {
        Self {
            spans_db_path,
            retention_days: Some(7),
            purge_interval_hours: 1,
            purge_chunk_size: 10_000,
        }
    }
}

// CID:afa-observability-observability-002 - ObservabilityEngine
// Purpose: The engine struct. Cheap to clone (every
// field is Arc-wrapped or atomic).
//
// **Field-by-field**:
// - storage: Arc`Storage` -- the wrapped
//   SQLite connection. Arc'd so the engine AND
//   the background loop can hold a reference (the
//   loop needs the Storage to call
//   delete_older_than every hour).
// - event_bus: EventBusHandle -- the publish-only
//   bus handle (the kernel holds the full
//   EventBus; the engine only needs the
//   publish side).
// - drops_in_last_hour: Arc`AtomicU64` -- the
//   drop counter shared with the HealthCheck
//   impl (which reads it) and the purge loop
//   (which resets it on every tick).
// - config: ObservabilityConfig -- the config
//   snapshot, owned by the engine. Read by
//   run_purge_loop when it needs the retention
//   rule and the chunk size.
// - purge_handle: Option`JoinHandle<()>` -- the
//   handle to the background purge task. None
//   when the loop is disabled (retention_days =
//   None or purge_interval_hours = 0).
//
// **Clone semantics**: a manual Clone impl is
// omitted because Arc / AtomicU64 / EventBusHandle
// are all Clone themselves; the derive(Clone)
// would auto-implement the same. Kept derive-less
// so the type's clone-cost is obvious from a read
// of the field declarations.
pub struct ObservabilityEngine {
    storage: Arc<Storage>,
    event_bus: EventBusHandle,
    drops_in_last_hour: Arc<AtomicU64>,
    config: ObservabilityConfig,
    purge_handle: Option<tokio::task::JoinHandle<()>>,
}

impl ObservabilityEngine {
    // CID:afa-observability-observability-003 - new
    // Purpose: Boot the engine. Open the spans DB,
    // apply the crate's MIGRATIONS, spawn the
    // background purge loop (if enabled).
    //
    // **Steps**:
    // 1. Open the spans DB at config.spans_db_path
    //    (sync -- per the afa-storage contract;
    //    async would require spawn_blocking for no
    //    benefit).
    // 2. Apply this crate's MIGRATIONS list (async
    //    -- the storage layer's migrate acquires
    //    the same tokio::sync::Mutex the rest of
    //    the engine uses).
    // 3. Build the engine struct with purge_handle
    //    = None (the field is set in-place via
    //    Arc::get_mut, see below).
    // 4. If config.purge_interval_hours > 0 AND
    //    config.retention_days.is_some(), spawn
    //    the background loop and assign the
    //    JoinHandle through Arc::get_mut (sound
    //    because no other thread holds a clone
    //    yet).
    // 5. Return the Arc`Self`.
    //
    // **Why Arc::get_mut and not Mutex`Option<...>`**:
    // the purge_handle field is read only on test
    // teardown (`abort_purge_for_test`); making it
    // a Mutex would add lock acquisition on every
    // future accessor. Arc::get_mut is sound in
    // the new() block because we hold the only
    // strong reference until the function returns.
    //
    // **Error contract**: returns Err on the open
    // or migrate path only. The purge-loop spawn
    // uses tokio::spawn which is infallible (the
    // runtime panic's if a tokio::spawn fails, not
    // our problem here).
    pub async fn new(
        config: ObservabilityConfig,
        event_bus: EventBusHandle,
    ) -> Result<Arc<Self>, ObservabilityError> {
        let storage_raw =
            open(&config.spans_db_path).map_err(|e| ObservabilityError::StorageUnreachable {
                reason: format!("open: {e}"),
            })?;
        let storage = Arc::new(storage_raw);

        migrate(&storage, persistence::MIGRATIONS)
            .await
            .map_err(|e| ObservabilityError::StorageUnreachable {
                reason: format!("migrate: {e}"),
            })?;

        let drops_in_last_hour = Arc::new(AtomicU64::new(0));

        let mut engine = Arc::new(Self {
            storage: Arc::clone(&storage),
            event_bus: event_bus.clone(),
            drops_in_last_hour: Arc::clone(&drops_in_last_hour),
            config,
            purge_handle: None,
        });

        if engine.config.purge_interval_hours > 0 && engine.config.retention_days.is_some() {
            // SAFETY: `engine` is the only strong
            // reference (we just made it; no clones
            // exist yet because Arc::get_mut would
            // otherwise return None). Mutating the
            // field through get_mut is sound for the
            // same reason the borrow checker trusts
            // it: no other thread can observe the
            // field until we return the Arc.
            let engine_mut =
                Arc::get_mut(&mut engine).expect("engine Arc must be unique before spawn");
            let bus = engine_mut.event_bus.clone();
            let drops = Arc::clone(&engine_mut.drops_in_last_hour);
            let interval_hours = engine_mut.config.purge_interval_hours;
            let storage_for_loop = Arc::clone(&storage);
            let handle = tokio::spawn(async move {
                run_purge_loop(storage_for_loop, bus, drops, interval_hours).await;
            });
            engine_mut.purge_handle = Some(handle);
        }

        Ok(engine)
    }

    // CID:afa-observability-observability-004 - record_span
    // Purpose: The method form of record_span.
    // Called by the free-function record_span
    // wrapper helper in record.rs; also exposed
    // for direct callers (tests, future engine
    // paths) that already have the SpanRecord.
    //
    // **Best-effort contract**: a write failure
    // does NOT return Err to the caller. The
    // caller sees Ok(()) (their request may
    // proceed); the dropped span is logged +
    // counted + audited. The engine's HealthCheck
    // reflects the drops.
    //
    // **Why best-effort**: the spans table is an
    // observability surface, not a source of
    // truth. A failed span write degrades the
    // observability (the doctor cannot see what
    // happened in the chart room) but never
    // blocks the patient's treatment (the doctor's
    // work continues). The IMPL §Phase 1
    // planning principle #7 makes this contract
    // explicit: "Best-effort is the contract, not
    // a fallback."
    //
    // **Failure handling**:
    // 1. drops_in_last_hour.fetch_add(1)
    // 2. Build a SpansWriteFailed event with the
    //    failure reason (truncated to 1 KiB).
    // 3. Publish the event on the bus
    //    (event_bus.publish returns () -- see
    //    doc-drift #7).
    // 4. tracing::warn! the reason at WARN level.
    // 5. Return Ok(()) (do NOT propagate the error).
    // The 8-argument signature (self + ctx +
    // engine_str + operation + attributes +
    // duration_ms + outcome + started_at) is the
    // IMPL §Phase 1 contract shape; cannot
    // reasonably be refactored into a builder
    // without changing the call-site shape.
    #[allow(clippy::too_many_arguments)]
    pub async fn record_span(
        &self,
        ctx: &afa_contracts::ExecutionContext,
        engine_str: &str,
        operation: &str,
        attributes: BTreeMap<String, String>,
        parent_span_id: Option<Uuid>,
        duration_ms: u32,
        outcome: SpanOutcome,
        started_at: DateTime<Utc>,
    ) -> Result<(), ObservabilityError> {
        let correlation_id = ctx.correlation_id;
        let record = SpanRecord {
            span_id: Uuid::new_v4(),
            parent_span_id,
            correlation_id,
            tenant_id: ctx.tenant_id.clone(),
            actor: ctx.actor.clone(),
            engine: engine_str.to_string(),
            operation: operation.to_string(),
            started_at,
            duration_ms,
            outcome,
            attributes,
        };

        match persistence::write_span(&self.storage, &record).await {
            Ok(()) => Ok(()),
            Err(e) => {
                self.drops_in_last_hour.fetch_add(1, Ordering::Relaxed);
                let event = afa_contracts::SpansWriteFailed {
                    count: 1,
                    reason: truncate_reason(&e.to_string()),
                    occurred_at: Utc::now(),
                    correlation_id: record.correlation_id,
                };
                self.event_bus.publish(event, ctx.clone()).await;
                tracing::warn!(
                    correlation_id = %record.correlation_id,
                    reason = %e,
                    "span write failed, dropped"
                );
                Ok(())
            }
        }
    }

    // CID:afa-observability-observability-007 - drops_in_last_hour
    // Purpose: Hand a clone of the drops counter
    // Arc to the caller. Used by the HealthCheck
    // impl and (via the storage field) by the
    // purge loop. Cheap to call (one atomic
    // increment on the Arc's ref count).
    pub fn drops_in_last_hour(&self) -> Arc<AtomicU64> {
        Arc::clone(&self.drops_in_last_hour)
    }

    // CID:afa-observability-observability-008 - storage
    // Purpose: Borrow the engine's storage handle.
    // Used by tests + by purge::run_one_purge when
    // it needs to drive the purge without going
    // through the engine.
    pub fn storage(&self) -> &Storage {
        &self.storage
    }

    // CID:afa-observability-observability-009 - config
    // Purpose: Borrow the engine's config snapshot.
    // Used by tests + future engine paths that
    // need to read the retention rule without
    // re-reading the kernel's KernelConfig.
    pub fn config(&self) -> &ObservabilityConfig {
        &self.config
    }

    // CID:afa-observability-observability-010 - abort_purge_for_test
    // Purpose: Abort the background purge task, if
    // one is running. Test-only helper (not part of
    // the Phase-1 public API).
    pub fn abort_purge_for_test(&self) {
        if let Some(handle) = &self.purge_handle {
            handle.abort();
        }
    }
}

// Thread-local parent span link. The free-fn
// record_span wrapper helper pushes its span_id on
// entry and clears on exit; the method form
// (ObservabilityEngine::record_span) reads it to
// populate SpanRecord::parent_span_id when called
// inside a wrapper scope.
//
// **Why thread_local and not a field on the
// engine**: the parent span_id is logically a
// property of the current stack frame, not a
// property of the engine. A thread_local survives
// the yield across an .await within the same
// thread (Rust's async runtimes use per-thread
// task queues by default; the multi-threaded
// executor would break this -- Phase 5 can
// migrate to a `tracing::Span` link instead).
//
// **Doc drift correction (folded here too)**: the
// IMPL said "use the tracing::Span's id() to derive
// parent_span_id". tracing::Id and uuid::Uuid are
// unrelated types (tracing::Id is an opaque u64;
// Uuid is a 128-bit value); translating one to the
// other deterministically would require a separate
// registry. The thread_local above is simpler and
// (no thread_local — the engine's `record_span`
// takes `parent_span_id: Option<Uuid>` directly
// from the caller. The wrapper helper passes
// the UUID it minted on the same task, which
// preserves nested-span linkage across tokio
// task boundaries. See `record.rs`.)

// CID:afa-observability-observability-013 - truncate_reason
// Purpose: Cap a SpansWriteFailed event's reason
// string at 1 KiB. The contract type's reason
// field is bounded at 1 KiB by the IMPL §Phase 0
// ("the on-disk size matches"); the cap is
// enforced at the publish boundary, not in the
// type itself (the type is the wire shape, the
// helper is the validator).
//
// The String slicing on is_char_boundary protects
// against splitting a multi-byte UTF-8 code
// point; we back up the slice end until we hit a
// valid boundary.
fn truncate_reason(s: &str) -> String {
    const ONE_KIB: usize = 1024;
    if s.len() <= ONE_KIB {
        s.to_string()
    } else {
        let mut end = ONE_KIB;
        while !s.is_char_boundary(end) && end > 0 {
            end -= 1;
        }
        format!("{}...", &s[..end])
    }
}

// CID:afa-observability-observability-005 - run_purge_loop
// Purpose: The background purge task. Fires every
// `interval_hours` hours, runs run_one_purge
// (from purge.rs), resets the drop counter.
//
// **First-tick skip**: tokio::time::interval fires
// its first tick immediately; we consume it
// without doing anything so the loop does NOT
// purge on boot (the operator can run run_one_purge
// manually if they want an immediate sweep). The
// IMPL does not specify boot behaviour; "no
// boot-time purge" is the safe default.
//
// **Cancellation policy**: the JoinHandle returned
// by tokio::spawn is held in
// ObservabilityEngine::purge_handle. A future
// Phase 4 may replace this with a clean shutdown
// signal (a CancellationToken the engine watches
// via tokio::select!). Phase 1's abort path is
// `abort_purge_for_test` (test-only).
//
// **Missed-tick behaviour**: Skip -- if the
// executor was busy for two intervals, the next
// tick fires once and then the loop is back in
// cadence. Burst-tick (the default) would queue
// every missed tick and try to catch up, which
// would run N purges back-to-back -- a bad idea
// when each purge is a single-transaction DELETE
// that may take seconds.
async fn run_purge_loop(
    storage: Arc<Storage>,
    event_bus: EventBusHandle,
    drops: Arc<AtomicU64>,
    interval_hours: u64,
) {
    let interval = Duration::from_secs(interval_hours * 3600);
    let mut ticker = tokio::time::interval(interval);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;

    loop {
        ticker.tick().await;
        let _ = run_one_purge_for_loop(&storage, &event_bus, &drops).await;
    }
}

// CID:afa-observability-observability-014 - run_one_purge_for_loop
// Purpose: The per-tick body of the background
// loop. Reads the default retention + chunk-size
// from purge.rs and delegates.
//
// **Private** -- tests call purge::run_one_purge
// directly with explicit retention_days, so this
// helper stays out of the public crate surface.
//
// **Result type**: Result<(), ()> -- the loop
// ignores failures (the next tick will try again;
// logging is the only feedback). The Result type
// is only kept because the body is one
// expression short of being elided.
async fn run_one_purge_for_loop(
    storage: &Storage,
    event_bus: &EventBusHandle,
    drops: &Arc<AtomicU64>,
) -> Result<(), ()> {
    let retention_days = crate::purge::default_retention_days();
    let chunk_size = crate::purge::default_chunk_size();
    let now = Utc::now();
    let cutoff = match crate::purge::cutoff_datetime(retention_days, now) {
        Some(c) => c,
        None => return Ok(()),
    };
    let _ = crate::purge::execute_one_purge(storage, event_bus, cutoff, chunk_size, drops).await;
    Ok(())
}

// CID:afa-observability-observability-015 - timer_ctx
// Purpose: Synthesise a timer-driven
// ExecutionContext (used by purge.rs as the
// publisher ctx for SpansPurged / SpansPurgeFailed
// events).
pub fn timer_ctx() -> afa_contracts::ExecutionContext {
    afa_contracts::ExecutionContext::new(
        afa_contracts::TenantId::new("__observability-purge__"),
        afa_contracts::Actor::Timer,
    )
}

#[allow(dead_code)]
pub fn empty_correlation_id() -> afa_contracts::CorrelationId {
    afa_contracts::CorrelationId(Uuid::nil())
}
