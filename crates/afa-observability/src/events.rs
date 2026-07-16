//! Code Map: observability::events
//!
//! - SpansWriteFailed: re-export of
//!   afa_contracts::SpansWriteFailed. Audit fact
//!   published on the bus when a span write to the
//!   spans table fails. Carries the failure count (1
//!   today; the field exists so a future batched-write
//!   failure can publish one event with a higher
//!   count), the reason string (truncated to 1 KiB),
//!   the wall-clock occurred_at, and the
//!   correlation_id of the dropped span's request.
//! - SpansPurged: re-export of
//!   afa_contracts::SpansPurged. Audit fact
//!   published on the bus when a retention purge run
//!   completes. Carries the count of spans deleted,
//!   the older_than cutoff (so a reviewer can confirm
//!   the right rows were targeted), the occurred_at
//!   time, and the correlation_id of the
//!   timer-driven purge task.
//! - SpansPurgeFailed: re-export of
//!   afa_contracts::SpansPurgeFailed. Audit fact
//!   published on the bus when a retention purge run
//!   fails (e.g. the spans DB file became unreadable
//!   mid-purge). Carries the count the engine tried to
//!   delete (often 0 if the failure happened on the
//!   first chunk), the reason, the older_than cutoff,
//!   the occurred_at time, and the correlation_id of
//!   the timer-driven task.
//!
//! Story (plain English): Imagine the recording
//! nurse's personal pad. Every time the central
//! logbook (the spans table) does something
//! noteworthy, she stamps a fact on her pad so the
//! next shift has a paper trail. Three stamps:
//! SpansWriteFailed (a single chart note could not
//! be filed -- "doctor's chart went missing at
//! 14:23, reason: logbook locked"), SpansPurged (the
//! retention clerk's morning sweep crossed out 240
//! entries -- "cleaned out anything older than 7
//! days"), and SpansPurgeFailed (the retention
//! clerk's sweep hit a snag -- "could not read page
//! 47 of the logbook"). The pad never has to be
//! asked -- it just stamps as events happen, and the
//! next nurse on shift can read it back.
//!
//! Doc drift correction vs. the IMPL draft:
//! - #1: the IMPL suggested a separate
//!   EventBusHandle trait in this crate. The trait is
//!   unnecessary -- afa_bus::EventBusHandle is
//!   already the publish-only handle the engine
//!   holds, and re-declaring it here would either be
//!   a duplicate trait definition (compile error in
//!   the workspace) or a re-export (no behaviour
//!   change). The re-export in this file is the
//!   cheapest correct answer.
//!
//! CID Index:
//! CID:afa-observability-events-001 -> SpansWriteFailed
//! CID:afa-observability-events-002 -> SpansPurged
//! CID:afa-observability-events-003 -> SpansPurgeFailed
//!
//! Quick lookup: rg -n "CID:afa-observability-events-" crates/afa-observability/src/events.rs

// The three audit facts. Re-exported from
// afa-contracts because every consumer agrees on
// the wire shape (the dictionary is the contract
// crate); this module is the convenience re-export
// so callers write
// `use afa_observability::SpansWriteFailed;` instead
// of having to know the type's contract-crate
// address.
//
// The re-exports are pub use (not pub mod + types
// + path re-exports inside) because we never add
// any behaviour here -- the types are the contract
// types verbatim. A future pack that needs to add
// engine-specific helper methods to any of these
// would do it by introducing a new `impl` block on
// the type from this crate (the contract crate is
// dictionary-only).
pub use afa_contracts::SpansPurgeFailed;
pub use afa_contracts::SpansPurged;
pub use afa_contracts::SpansWriteFailed;
