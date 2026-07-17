//! Code Map: observability::record (the wrapper helpers)
//!
//! - record_span: Result-returning wrapper helper
//!   (parent_span_id explicit). Awaits a future,
//!   classifies its Ok/Err outcome, enforces the
//!   64-entries / 4 KiB-per-value attributes cap,
//!   and best-effort-records one SpanRecord to
//!   the engine.
//! - record_span_value: non-Result sibling
//!   (parent_span_id explicit). Awaits a
//!   non-Result future, forces outcome=Ok,
//!   enforces the caps, and best-effort-records
//!   one SpanRecord.
//! - caps_enforce_attributes: private helper
//!   shared by both wrappers — drops
//!   attributes when count > 64 OR any entry
//!   exceeds the 4 KiB-per-value / 64-byte-key
//!   cap, and emits a tracing::error!.
//!
//! Story (plain English): the doctor's chart
//! wrapper. The doctor (caller code) hands the
//! chart-helper a future. The chart-helper runs
//! the future, watches what comes out, and
//! records one paper logbook entry. The
//! chart-helper does NOT decide what the doctor
//! did (that's the engine's span_record); the
//! chart-helper just times the visit and notes
//! "the patient is fine" or "the patient
//! coughed". If the paper logbook is locked, the
//! chart-helper silently keeps going — the
//! doctor's work is never blocked by
//! bookkeeping. The parent_id is explicit
//! (passed by the caller), not hidden in a
//! thread_local — tokio::spawn puts work on
//! different tasks, and a thread_local value
//! from one task does not appear on another.
//!
//! Doc-drift corrections vs. the IMPL draft:
//! - #6: HealthCheck is sync, not async (IMPL's
//!   example code uses `#[async_trait]` but the
//!   trait in afa-contracts is sync).
//! - #8: SpanOutcome's unit variant emits the
//!   bare string "Ok" (serde's default for a
//!   unit variant of an externally-tagged
//!   enum), NOT the IMPL's `{"Ok": null}`
//!   shape.
//! - #9: afa_bus::EventBus::publish returns (),
//!   not Result (the IMPL's draft said Result
//!   but the contract is best-effort publish,
//!   swallow errors).
//! - #11: the free-fn wrapper signature has
//!   `parent_span_id: Option<Uuid>` as an
//!   explicit parameter (the IMPL's draft used
//!   `Box<dyn AfaError>` and AsyncFnOnce-style
//!   trait bounds that don't compile on stable
//!   Rust today).
//! - #13: SpanOutcome wire form is "Ok" (test
//!   caught on first red→green cycle).
//! - #14: parent linkage was attempted via
//!   `PARENT_SPAN_ID thread_local!`, but the
//!   tokio scheduler's JoinSet::spawn puts each
//!   spawned task on a worker thread that has
//!   its OWN thread_local — the parent linkage
//!   was lost across spawn boundaries. Fixed by
//!   making parent_span_id explicit on both
//!   `record_span` (engine) and the two wrapper
//!   helpers; the wrapper no longer mints a
//!   parent UUID, the caller passes it.
//! - #20: doc-comments in earlier versions of
//!   this file advertised Phase 2 features
//!   (elapsed timing, full Phase 2 nested-span
//!   linking) as "current". This rewrite drops
//!   those bullets; elapsed timing is still 0
//!   ms (the wrapper doesn't measure it; the
//!   engine accepts a duration_ms arg for
//!   future use).
//!
//! CID Index:
//! CID:afa-observability-record-001 -> record_span
//! CID:afa-observability-record-002 -> record_span_value
//! CID:afa-observability-record-003 -> caps_enforce_attributes
//!
//! Quick lookup:
//!   "wrap a Result future, record outcome" -> record_span
//!   "wrap a non-Result future, record Ok" -> record_span_value
//!   "drop attrs that exceed caps" -> caps_enforce_attributes

use crate::error::ObservabilityError;
use crate::observability::ObservabilityEngine;
use afa_contracts::{ExecutionContext, SpanOutcome};
use chrono::{DateTime, Utc};
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use uuid::Uuid;

// CID:afa-observability-record-003 - caps_enforce_attributes
// Purpose: Drop attributes that exceed the
// IMPL §"attributes cap" planning principle:
// > 64 entries OR any key > 64 bytes OR any
// value > 4096 bytes (4 KiB). When the cap
// fires, the function emits a tracing::error!
// log so the operator sees the over-cap call in
// the audit trail, and returns an empty
// BTreeMap so the engine records the row
// without attributes (rather than rejecting
// the row entirely — the doctor still gets
// recorded, just without the over-cap
// detail).
//
// Used by: record_span + record_span_value
// (both call this before forwarding to the
// engine).
fn caps_enforce_attributes(
    engine_str: &str,
    operation: &str,
    attributes: BTreeMap<String, String>,
) -> BTreeMap<String, String> {
    const MAX_ENTRIES: usize = 64;
    const MAX_KEY_BYTES: usize = 64;
    const MAX_VALUE_BYTES: usize = 4096;
    if attributes.len() > MAX_ENTRIES {
        tracing::error!(
            engine = %engine_str,
            operation = %operation,
            count = attributes.len(),
            "attributes count > 64, recording without attributes"
        );
        BTreeMap::new()
    } else if attributes
        .iter()
        .any(|(k, v)| k.len() > MAX_KEY_BYTES || v.len() > MAX_VALUE_BYTES)
    {
        tracing::error!(
            engine = %engine_str,
            operation = %operation,
            "attributes entry exceeds cap (key > 64 or value > 4096), recording without attributes"
        );
        BTreeMap::new()
    } else {
        attributes
    }
}

// CID:afa-observability-record-001 - record_span
// Purpose: The Result-returning wrapper helper.
// Wraps a future that produces a `Result<T, E>`
// where `E: AfaError`. The helper:
//   1. records `started_at = Utc::now()`,
//   2. awaits the future,
//   3. classifies the outcome (Ok -> Ok, Err ->
//      Err { kind: e.kind(), reason:
//      e.to_string() }),
//   4. enforces the attributes cap,
//   5. best-effort-records one SpanRecord to the
//      engine with `parent_span_id = parent`
//      (caller-supplied; the wrapper does NOT
//      mint a parent UUID because tokio::spawn
//      does not propagate thread_local values
//      to the spawned task — see doc-drift #14
//      in the file header),
//   6. returns the future's Result unchanged
//      (best-effort: a persistence failure in
//      the engine does NOT change what the
//      caller sees).
//
// **Caller picks parent_span_id**: `None` for
// the root span of a request (e.g. `Runtime::
// ingest`'s wrapper), `Some(dispatch_uuid)` for
// a span nested under a prior `record_span`
// (e.g. `Scheduler::dispatch`'s per-step
// wrapper). The kernel mints
// `dispatch_uuid` once and passes it to both
// the outer wrap and each per-step wrap.
//
// **Why explicit parent_span_id and not a
// hidden thread_local**: `tokio::task::JoinSet::
// spawn` puts each task on a worker thread
// chosen by the runtime. A `std::thread::
// LocalKey` value lives on the worker's
// per-thread storage; the spawned task's
// worker thread has its own (empty) value.
// Same issue for `tokio::task::LocalKey`
// (per-task, not propagated across spawn).
// Explicit parameters work across both.
// The prior session's implementation
// tried `thread_local!` first; the Phase 2
// `kernel_dispatch_records_spans.rs` tests
// caught the broken linkage and this
// rewrite is the fix.
//
// **Where the wrapper differs from the
// engine's record_span method**: the helper
// owns the timing + outcome classification
// (the method form is the dumb "build a
// SpanRecord + INSERT" endpoint, useful for
// callers that already have a SpanRecord).
//
// **Best-effort propagation**: the helper
// ignores the engine's outcome (the engine's
// record_span is also best-effort and returns
// Ok(()) on persistence failure). The helper
// returns the *future's* outcome to the caller
// (not the engine's), so the doctor sees what
// the doctor saw, not what the nurse's
// logbook saw.
pub async fn record_span<E, T, F>(
    ctx: &ExecutionContext,
    engine_str: &str,
    operation: &str,
    attributes: BTreeMap<String, String>,
    parent_span_id: Option<Uuid>,
    engine_arc: &Arc<ObservabilityEngine>,
    future: F,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: afa_contracts::AfaError + Send + Sync + 'static,
{
    record_inner(
        ctx,
        engine_str,
        operation,
        attributes,
        parent_span_id,
        engine_arc,
        outcome_from_result::<E, T>,
        future,
    )
    .await
}

// CID:afa-observability-record-002 - record_span_value
// Purpose: The non-`Result` sibling of
// `record_span`. Used by callers whose future
// does not return a `Result`
// (e.g. `Runtime::ingest -> CorrelationId`,
// `Scheduler::dispatch -> ()`). Mirrors
// `record_span` in every other way: enforces
// the caps, classifies the outcome (always
// `SpanOutcome::Ok` since the future cannot
// fail), best-effort-records one SpanRecord,
// and returns the future's value unchanged.
// The `parent_span_id` is explicit (see the
// doc-drift #14 note on `record_span`).
//
// **Why a sibling helper instead of forcing
// `Result` everywhere**: `Runtime::ingest` and
// `Scheduler::dispatch` have stable, non-
// `Result` return types in the v1 contract.
// Wrapping them to return `Result<T, Box<dyn
// AfaError>>` would either change the contract
// (breaking the `Runtime::ingest ->
// CorrelationId` callers) or force every call
// site into an `Ok(value)` ceremony. The
// sibling helper is the cheaper option and is
// what Phase 2's Kernel wiring uses.
pub async fn record_span_value<T, F>(
    ctx: &ExecutionContext,
    engine_str: &str,
    operation: &str,
    attributes: BTreeMap<String, String>,
    parent_span_id: Option<Uuid>,
    engine_arc: &Arc<ObservabilityEngine>,
    future: F,
) -> T
where
    F: Future<Output = T>,
{
    record_inner(
        ctx,
        engine_str,
        operation,
        attributes,
        parent_span_id,
        engine_arc,
        outcome_from_value::<T>,
        future,
    )
    .await
}

// Private helper shared by record_span +
// record_span_value. Runs the future, captures
// the outcome, enforces the caps, fires the
// engine.record_span call, and returns the
// future's value (T, which is Result<T, E>
// for record_span and T for record_span_value).
//
// **Why a private helper, not two near-identical
// bodies**: AGENTS.md §5 ("files stay under
// 250 lines; small pieces compose"). The two
// public wrappers differ only in how the
// outcome is classified (Ok/Err vs always-Ok);
// the rest (timing, caps, engine call,
// best-effort swallow) is identical. The
// classify function is supplied by the caller
// so the helper can serve both signatures.
#[allow(clippy::too_many_arguments)]
async fn record_inner<T, F>(
    ctx: &ExecutionContext,
    engine_str: &str,
    operation: &str,
    attributes: BTreeMap<String, String>,
    parent_span_id: Option<Uuid>,
    engine_arc: &Arc<ObservabilityEngine>,
    classify: fn(&T) -> SpanOutcome,
    future: F,
) -> T
where
    F: Future<Output = T>,
{
    let started_at: DateTime<Utc> = Utc::now();
    let result = future.await;
    let attributes = caps_enforce_attributes(engine_str, operation, attributes);
    let outcome = classify(&result);
    let _ = engine_arc
        .record_span(
            ctx,
            engine_str,
            operation,
            attributes,
            parent_span_id,
            0,
            outcome,
            started_at,
        )
        .await
        .map_err(|e: ObservabilityError| {
            tracing::warn!(error = %e, "record_span helper: engine write failed");
        });
    result
}

// Classify the outcome of a Result-returning
// future. `Ok(_) -> Ok`; `Err(e) -> Err { kind,
// reason: e.to_string() }`. Used by record_span.
fn outcome_from_result<E, T>(result: &Result<T, E>) -> SpanOutcome
where
    E: afa_contracts::AfaError,
{
    match result {
        Ok(_) => SpanOutcome::Ok,
        Err(e) => SpanOutcome::Err {
            kind: e.kind(),
            reason: e.to_string(),
        },
    }
}

// Classify the outcome of a non-Result
// future: always Ok. Used by record_span_value.
fn outcome_from_value<T>(_result: &T) -> SpanOutcome {
    SpanOutcome::Ok
}
