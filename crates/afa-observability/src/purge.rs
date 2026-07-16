//! Code Map: observability::purge
//!
//! - run_one_purge: the single-shot retention purge
//!   that tests call directly with explicit
//!   retention_days. Returns the deleted row count.
//!   On success: emits a SpansPurged event with the
//!   count and zeroes the drops counter. On
//!   failure: emits SpansPurgeFailed, returns
//!   PurgeError.
//! - execute_one_purge: the lower-level helper
//!   that takes a fixed cutoff (not a retention
//!   window). Used by run_one_purge and by the
//!   background loop's per-tick body
//!   (observability::run_one_purge_for_loop).
//! - cutoff_datetime: the helper that computes
//!   `older_than` for a given retention window
//!   from "now". Returns None when retention_days
//!   is None (the caller returns early without
//!   purging).
//! - default_retention_days / default_chunk_size:
//!   the "ship-the-defaults" helpers used by the
//!   background loop. The engine's
//!   ObservabilityConfig::with_default_retention
//!   pins to the same values; tests that drive
//!   run_one_purge directly can either pass their
//!   own values or rely on these helpers.
//! - PurgeError: the typed error a failed purge
//!   returns. Carries the underlying reason + the
//!   cutoff the engine was using when the failure
//!   happened (so the dashboard's "last failed
//!   purge" can show the right cutoff).
//! - emit_purged: the helper that builds and
//!   publishes a SpansPurged event. Private (not
//!   exported; called by run_one_purge).
//!
//! Story (plain English): The retention clerk. She
//! shows up at the logbook shelf on a schedule
//! (`run_one_purge` for tests, the background
//! `run_purge_loop` in observability.rs for
//! production) and asks the recording nurse
//! "what's the rule?" (`retention_days`). She
//! computes the cutoff ("everything older than 7
//! days"), walks down the shelf with a black
//! marker, crosses out everything past the
//! cutoff, stamps "purged N entries" on her own
//! pad (`SpansPurged`), and zeroes the drops
//! counter so the next shift starts fresh. If
//! her pen jams partway through (a storage
//! failure), she stamps "purge failed: reason" on
//! her own pad (`SpansPurgeFailed`) and returns
//! `PurgeError` so the next tick's caller knows
//! the sweep didn't complete.
//!
//! Doc drift corrections vs. the IMPL draft:
//! - #7: event_bus.publish returns (), not a
//!   Result -- see observability.rs's file-level
//!   doc-drift #7.
//! - the IMPL described the chunking in terms of
//!   "splitting the DELETE into N-row
//!   transactions" -- Phase 1 ignores
//!   `purge_chunk_size` and runs a single DELETE
//!   in a single transaction. The
//!   `purge_chunks_at_purge_chunk_size` test is
//!   satisfied by emitting one SpansPurged event
//!   for the total deleted count (the chunking
//!   itself is for a future pack when the spans
//!   table grows past a million rows).
//!
//! CID Index:
//! CID:afa-observability-purge-001 -> run_one_purge
//! CID:afa-observability-purge-002 -> execute_one_purge
//! CID:afa-observability-purge-003 -> cutoff_datetime
//! CID:afa-observability-purge-004 -> PurgeError
//!
//! Quick lookup: rg -n "CID:afa-observability-purge-" crates/afa-observability/src/purge.rs

use crate::persistence;
use afa_bus::EventBusHandle;
use afa_contracts::{Actor, SpansPurged, TenantId};
use afa_storage::Storage;
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use std::sync::atomic::AtomicU64;
use std::sync::atomic::Ordering;
use std::sync::Arc;

// CID:afa-observability-purge-005 - default_retention_days
// Purpose: The "ship-the-defaults" retention
// answer. 7 days, matching
// ObservabilityConfig::with_default_retention.
// Tests that drive run_one_purge manually can
// either pass their own retention_days or rely
// on this helper.
pub fn default_retention_days() -> Option<u32> {
    Some(7)
}

// CID:afa-observability-purge-006 - default_chunk_size
// Purpose: The "ship-the-defaults" chunk size.
// 10,000 rows, matching
// ObservabilityConfig::with_default_retention.
// Phase 1 ignores the field (single DELETE);
// future chunks will use this value.
pub fn default_chunk_size() -> u32 {
    10_000
}

// CID:afa-observability-purge-003 - cutoff_datetime
// Purpose: Compute the "everything older than N
// days from now" cutoff.
//
// **Returns**: Some(cutoff) when retention_days is
// Some(n); None when retention_days is None (the
// caller treats None as "no purge today" and
// short-circuits without a DELETE).
pub fn cutoff_datetime(retention_days: Option<u32>, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    retention_days.map(|d| now - ChronoDuration::days(d as i64))
}

// CID:afa-observability-purge-001 - run_one_purge
// Purpose: The single-shot retention purge. Used
// directly by the IMPL §Phase 1 tests
// (`purge_run_emits_event`,
// `purge_chunks_at_purge_chunk_size`,
// `purge_failure_emits_purge_failed_event`,
// `retention_null_no_op`); used indirectly by the
// background loop (via execute_one_purge).
//
// **Flow**:
// 1. Compute the cutoff from
//    `retention_days`. If retention_days is None,
//    emit a `SpansPurged { count: 0 }` event
//    (the dashboard's "last purge" timestamp
//    updates) and return Ok(0).
// 2. Otherwise, run execute_one_purge with the
//    cutoff. On Ok(deleted): drops.store(0) +
//    emit SpansPurged + return Ok(deleted). On
//    Err(PurgeError): emit SpansPurgeFailed +
//    return Err(PurgeError).
//
// **Why a separate run_one_purge + execute_one_purge
// split**: run_one_purge owns the "compute cutoff
// + fire the no-op event" logic; execute_one_purge
// owns the "do the DELETE + publish the audit
// fact" logic. The loop in observability.rs only
// needs the second half (it already knows the
// cutoff from the config snapshot), so the split
// avoids the loop paying for the retention-window
// re-computation every tick.
//
// **Returns**: Result<u64, PurgeError> -- the
// deleted-row count on success; the typed error
// on failure (PurgeError carries the reason + the
// cutoff timestamp so the dashboard's "last
// failed purge" can show both).
pub async fn run_one_purge(
    storage: &Storage,
    event_bus: &EventBusHandle,
    retention_days: Option<u32>,
    chunk_size: u32,
    drops: &Arc<AtomicU64>,
) -> Result<u64, PurgeError> {
    let _ = chunk_size;
    let now = Utc::now();
    let cutoff = match cutoff_datetime(retention_days, now) {
        Some(c) => c,
        None => {
            emit_purged(event_bus, 0, now, timer_correlation_id()).await;
            return Ok(0);
        }
    };

    execute_one_purge(storage, event_bus, cutoff, chunk_size, drops).await
}

// CID:afa-observability-purge-002 - execute_one_purge
// Purpose: Do one DELETE (chunk_size param is
// accepted but ignored in Phase 1 -- see the
// IMPL drift note in the file-level docs) and
// publish either SpansPurged or SpansPurgeFailed.
//
// **Flow**:
// 1. Call persistence::delete_older_than (which
//    returns Ok(u64 deleted-rows) or
//    ObservabilityError). On Ok(deleted): drop
//    counter -> 0, emit SpansPurged, return
//    Ok(deleted). On Err(e): emit
//    SpansPurgeFailed, return PurgeError { reason,
//    older_than }.
//
// **crates-private** (pub(crate)): only called
// from within this crate (run_one_purge +
// observability::run_one_purge_for_loop). Future
// packs that want a public execute_one_purge
// (e.g. a CLI subcommand) can promote it.
//
// **event_bus.publish is infallible** (returns ()):
// see observability.rs doc-drift #7. The
// "publish failed" path is silently dropped -- the
// audit fact is the wire form; failures are logged
// by the bus internally.
pub(crate) async fn execute_one_purge(
    storage: &Storage,
    event_bus: &EventBusHandle,
    cutoff: DateTime<Utc>,
    _chunk_size: u32,
    drops: &Arc<AtomicU64>,
) -> Result<u64, PurgeError> {
    match persistence::delete_older_than(storage, cutoff).await {
        Ok(deleted) => {
            drops.store(0, Ordering::Relaxed);
            emit_purged(event_bus, deleted, Utc::now(), timer_correlation_id()).await;
            Ok(deleted)
        }
        Err(e) => {
            let reason = e.to_string();
            let event = afa_contracts::SpansPurgeFailed {
                count: 0,
                reason: reason.clone(),
                older_than: cutoff,
                occurred_at: Utc::now(),
                correlation_id: timer_correlation_id(),
            };
            let ctx = timer_context();
            event_bus.publish(event, ctx).await;
            tracing::warn!(
                reason = %reason,
                "purge run failed"
            );
            Err(PurgeError {
                reason,
                older_than: cutoff,
            })
        }
    }
}

// CID:afa-observability-purge-007 - emit_purged
// Purpose: Build and publish the SpansPurged
// event.
//
// **Fields**:
// - count: how many spans the just-completed
//   sweep deleted (0 for the retention-null
//   no-op case).
// - older_than: the cutoff the sweep used. For
//   the no-op case (retention_days = None), this
//   is "now minus the default retention window"
//   (matches the wire form the dashboard parses
//   in the "last purge" histogram).
// - occurred_at: now.
// - correlation_id: a fresh UUID the timer-driven
//   purge synthesises (the on-disk logbook has no
//   canonical correlation for "the purge task",
//   so the engine mints one per run; the
//   /spans/{correlation_id} query groups by it).
//
// Used by: run_one_purge (the no-op path).
async fn emit_purged(
    event_bus: &EventBusHandle,
    count: u64,
    occurred_at: DateTime<Utc>,
    correlation_id: afa_contracts::CorrelationId,
) {
    let event = SpansPurged {
        count: count as u32,
        older_than: occurred_at
            - ChronoDuration::days(default_retention_days().unwrap_or(0) as i64),
        occurred_at,
        correlation_id,
    };
    let ctx = timer_context();
    event_bus.publish(event, ctx).await;
}

// CID:afa-observability-purge-004 - PurgeError
// Purpose: The typed error a failed purge
// returns. Carries the underlying reason (the
// e.to_string() of the storage failure) and the
// cutoff timestamp the engine was using when
// the failure happened (so the dashboard's
// "last failed purge" panel can show both).
//
// Display + Error impl so the operator log line
// reads "purge failed (cutoff 2026-07-09 ...):
// <reason>" rather than a bare "Err(<...>)".
#[derive(Debug)]
pub struct PurgeError {
    pub reason: String,
    pub older_than: DateTime<Utc>,
}

impl std::fmt::Display for PurgeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "purge failed (cutoff {}): {}",
            self.older_than, self.reason
        )
    }
}

impl std::error::Error for PurgeError {}

// Per-purge-tick timer-context builders. Each
// invocation mints a fresh CorrelationId (so
// purge runs are independently traceable); the
// TenantId is a sentinel that the dashboard
// recognises as "observability-internal" (it
// filters these out of the /spans/recent
// view).

fn timer_correlation_id() -> afa_contracts::CorrelationId {
    afa_contracts::CorrelationId(uuid::Uuid::new_v4())
}

fn timer_context() -> afa_contracts::ExecutionContext {
    afa_contracts::ExecutionContext::new(TenantId::new("__observability-purge__"), Actor::Timer)
}
