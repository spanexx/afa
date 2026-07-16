//! Code Map: observability::record (the wrapper helper)
//!
//! - record_span: the free-function wrapper helper
//!   that opens a tracing::span! around an async
//!   future, measures the duration with
//!   tokio::time::Instant, and on close hands one
//!   SpanRecord to the engine via the method form
//!   (ObservabilityEngine::record_span). In Phase 1
//!   the helper exists so the lib's surface compiles
//!   and is testable; in Phase 2 the kernel's
//!   dispatch path wraps every dispatched
//!   operation in record_span, and the helper gains
//!   the attributes-cap enforcement + nested-span
//!   linkage (see observability.rs's parent-span
//!   thread-local + the IMPL §Phase 1
//!   `attributes_cap_64_entries` test for the
//!   intended Phase-2 surface).
//!
//! Story (plain English): The doctor's chart
//! wrapper. Before the doctor opens a chart (the
//! async future), she wraps it in a blank cover
//! sheet (the tracing::span). While the doctor
//! works, the cover sheet stamps the start time.
//! When the doctor closes the cover sheet (the
//! future returns), the recording nurse (the
//! engine) walks over, reads the cover sheet's
//! stamps + the doctor's notes, and files a
//! one-line summary in the central logbook (the
//! spans table). If the cover sheet's notes say
//! "everything went well" (Ok), the recording
//! nurse stamps SpanOutcome::Ok on her summary;
//! if they say "this patient needed an X-ray I
//! couldn't get" (Err), she stamps SpanOutcome::Err
//! with the kind + reason from the doctor's notes.
//! In every case the doctor's work proceeds
//! unchanged (best-effort -- the logbook is for
//! the nurse, not for the patient).
//!
//! Doc drift corrections vs. the IMPL draft:
//! - #6: SpanOutcome::Err uses e.to_string() for
//!   the reason (not e.reason() -- that method does
//!   not exist on the AfaError trait).
//! - #7: event_bus.publish returns (), not Result,
//!   but the helper does not call publish directly
//!   (it routes through the engine's record_span,
//!   which calls publish internally).
//! - #11: the IMPL's `record_span<F, T>(..., future
//!   F) -> Result<T, Box<dyn AfaError>>` signature
//!   forces AsyncFnOnce-style bounds that are
//!   unstable in Rust as of 2026-07. Phase 1's
//!   signature accepts `F: Future<Output = Result<T,
//!   E>> where E: AfaError + ...` -- a regular
//!   future returning a concrete error type that
//!   satisfies the trait. The full Phase-2 helper
//!   (attributes-cap enforcement + nested-span
//!   linking) lives in this file once Phase 2
//!   lands.
//!
//! CID Index:
//! CID:afa-observability-record-001 -> record_span
//!
//! Quick lookup: rg -n "CID:afa-observability-record-" crates/afa-observability/src/record.rs

use crate::error::ObservabilityError;
use crate::observability::ObservabilityEngine;
use afa_contracts::{ExecutionContext, SpanOutcome};
use chrono::Utc;
use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use uuid::Uuid;

// CID:afa-observability-record-001 - record_span
// Purpose: The free-function wrapper helper.
//
// **Phase 1 surface**: takes a future, awaits it,
// times the duration via tokio::time::Instant,
// classifies the outcome (Ok / Err { kind,
// reason: e.to_string() }), and hands one
// SpanRecord (duration_ms = 0 in Phase 1 -- the
// elapsed measurement is on the wrapper scope but
// is plumbed through the method form's duration_ms
// arg as a separate call) to the engine.
//
// **Phase 2 additions (NOT in this file yet)**:
// - Attributes cap enforcement (>64 entries OR
//   any key>64 OR any value>4096 -> drop
//   attributes, log a tracing::error!, record
//   with empty attributes). The IMPL §Phase 1
//   `attributes_cap_64_entries` and
//   `attributes_cap_4kb_per_value` tests pin this.
// - Nested-span linking: push the new span_id on
//   PARENT_SPAN_ID via set_parent_span_id on
//   entry, clear on exit, so any inner record_span
//   call (or any direct ObservabilityEngine::
//   record_span call inside the scope) sees
//   `parent_span_id = Some(this_id)`.
// - Per-call elapsed timing: capture
//   `tokio::time::Instant::now()` at span open,
//   `elapsed().as_millis() as u32` at span close,
//   and pass the value to the engine's
//   `record_span` method (the engine's
//   `duration_ms` parameter is wired through
//   here, not synthesised at zero).
//
// **Where the wrapper helper differs from the
// engine's record_span method**: the helper owns
// the timing + outcome classification (the method
// is the dumb "build a SpanRecord + INSERT"
// endpoint). The method form is also exposed for
// direct callers (tests + future engine paths)
// that already have the SpanRecord.
//
// **Best-effort propagation**: the helper ignores
// the engine's outcome (the engine's record_span
// is also best-effort and returns Ok(()) on
// persistence failure). The helper returns the
// *future's* outcome to the caller (not the
// engine's), so the doctor sees what the doctor
// saw, not what the nurse's logbook saw.
pub async fn record_span<E, T, F>(
    ctx: &ExecutionContext,
    engine_str: &str,
    operation: &str,
    attributes: BTreeMap<String, String>,
    engine_arc: &Arc<ObservabilityEngine>,
    future: F,
) -> Result<T, E>
where
    F: Future<Output = Result<T, E>>,
    E: afa_contracts::AfaError + Send + Sync + 'static,
{
    let started_at = Utc::now();
    // **Nested-span wiring** (GREEN step for test
    // `span_with_parent`): the wrapper helper mints
    // a fresh span_id on entry, pushes it onto the
    // engine's PARENT_SPAN_ID thread-local, and
    // clears it on exit. Any ObservabilityEngine::
    // record_span call inside the future (or any
    // direct record_span call) sees
    // parent_span_id = Some(this_id). This is the
    // minimum code needed to make the
    // `span_with_parent` test green -- Phase 2
    // adds attributes-cap enforcement + per-call
    // elapsed timing on top of this.
    let outer_span_id = Uuid::new_v4();
    eprintln!(
        "[record_span helper] enter, minted outer_span_id={}",
        outer_span_id
    );
    crate::observability::set_parent_span_id(Some(outer_span_id));
    eprintln!("[record_span helper] PARENT set to Some({})", outer_span_id);
    let result = future.await;
    eprintln!("[record_span helper] post-await, clearing PARENT");
    crate::observability::set_parent_span_id(None);
    eprintln!(
        "[record_span helper] PARENT cleared, outer_uuid was {}",
        outer_span_id
    );

    // **Attributes-cap enforcement** (GREEN step for
    // tests `attributes_cap_64_entries` and
    // `attributes_cap_4kb_per_value`): the
    // wrapper enforces the 64-entries / 4 KiB-
    // per-value caps before forwarding to the
    // engine (per the IMPL §"attributes cap"
    // planning principle). The over-cap call
    // gets a tracing::error! log + an empty
    // attributes map passed downstream.
    let attributes: BTreeMap<String, String> = if attributes.len() > 64 {
        tracing::error!(
            engine = %engine_str,
            operation = %operation,
            count = attributes.len(),
            "attributes count > 64, recording without attributes"
        );
        BTreeMap::new()
    } else if attributes
        .iter()
        .any(|(k, v)| k.len() > 64 || v.len() > 4096)
    {
        tracing::error!(
            engine = %engine_str,
            operation = %operation,
            "attributes entry exceeds cap (key > 64 or value > 4096), recording without attributes"
        );
        BTreeMap::new()
    } else {
        attributes
    };

    let outcome = match &result {
        Ok(_) => SpanOutcome::Ok,
        Err(e) => SpanOutcome::Err {
            kind: e.kind(),
            reason: e.to_string(),
        },
    };
    // Best-effort: swallow the engine's outcome
    // (a persistence failure should not change
    // what the future returned).
    let _ = engine_arc
        .record_span(
            ctx, engine_str, operation, attributes, 0, outcome, started_at,
        )
        .await
        .map_err(|e: ObservabilityError| {
            tracing::warn!(error = %e, "record_span helper: engine write failed");
        });
    result
}
