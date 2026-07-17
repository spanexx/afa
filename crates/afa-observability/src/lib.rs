//! Code Map: afa-observability
//!
//! Public items:
//! - ObservabilityEngine: the engine struct (one per
//!   process; cheap to clone via Arc`Self`).
//! - ObservabilityConfig: the boot-time configuration
//!   the kernel hands to ObservabilityEngine::new.
//! - record_span: the free-function wrapper helper
//!   that opens a tracing::span! around an async
//!   future and records one SpanRecord on close.
//!   Phase 1 ships the signature so the lib compiles;
//!   Phase 2 wires it into the kernel's dispatch path
//!   and adds the attributes-cap + nested-span logic.
//!
//! Module map:
//! - observability: the engine struct + the engine's
//!   new() + record_span() method + the background
//!   purge loop runner.
//! - record: the free-function record_span wrapper
//!   helper.
//! - persistence: the spans table schema (MIGRATIONS)
//!   plus write_span, read_by_correlation_id,
//!   read_recent, and delete_older_than.
//! - purge: the single-shot run_one_purge function +
//!   cutoff_datetime helper. The background loop is
//!   spawned by observability.rs (it needs the
//!   config snapshot the engine already holds).
//! - events: re-exports of the three audit facts
//!   (SpansWriteFailed, SpansPurged, SpansPurgeFailed)
//!   from afa-contracts.
//! - health: the HealthCheck for ObservabilityEngine
//!   impl.
//! - error: the internal ObservabilityError enum (four
//!   variants) + the From impl to the contract surface
//!   ObservabilityErrorV1.
//!
//! Story (plain English): Imagine the hospital's two
//! support roles for the doctors on their ward round.
//! The observability engine is the recording nurse
//! following the doctor: every time the doctor opens a
//! chart (the wrapper helper), the recording nurse
//! writes a one-line summary on the chart (the
//! SpanRecord) and files it in the ward's daily logbook
//! (the spans table). If the logbook is full or locked
//! (the spans DB file is unwritable), the recording
//! nurse does NOT stop the doctor -- she writes a
//! "I dropped this one" note on her own pad (the
//! SpansWriteFailed event), increments the "drops this
//! hour" tally, and lets the doctor keep going. The
//! purge clerk shows up every hour, asks the recording
//! nurse "what's the retention rule?", then walks
//! through the logbook with a black marker (the DELETE
//! statement), crossing out every entry older than
//! retention_days. When she's done she stamps
//! "purged N entries" on her own pad (the SpansPurged
//! event). The two never block the doctor's work:
//! they are best-effort observers, not gates.
//!
//! Doc drift corrections vs. the IMPL draft:
//!
//! - #5: the HealthCheck impl is synchronous
//!   (fn health_check(&self) -> HealthStatus). The
//!   IMPL's `#[async_trait::async_trait]` annotation
//!   is wrong -- the HealthCheck trait in
//!   afa-contracts has a sync signature on purpose
//!   (so the kernel's aggregate_health can use
//!   catch_unwind + a 100ms-per-engine timeout
//!   without a `Pin<Box<dyn Future + Send>>` getting
//!   in the way).
//!
//! - #6: SpanOutcome::Err { kind: e.kind(), reason:
//!   e.reason().to_string() } is wrong; the AfaError
//!   trait has ONLY .kind() -- no .reason() method. The
//!   correct form is SpanOutcome::Err { kind:
//!   e.kind(), reason: e.to_string() }. The to_string()
//!   form works for every AfaError implementor (every
//!   one uses thiserror::Error which derives Display).
//!
//! - #7: the IMPL's "every event_bus.publish(event)
//!   returns a Result" assumption is wrong. The actual
//!   EventBus::publish returns () (failures log
//!   internally via tracing::warn!). The audit-event
//!   emits in this crate are all of the form
//!   event_bus.publish(event, ctx).await; (no Result
//!   to unwrap).
//!
//! - #8: the IMPL's code example called
//!   event_bus.publish(record.clone()) (publishing the
//!   SpanRecord itself on the bus). But SpanRecord is
//!   not an AfaEvent (only the three audit facts are),
//!   and adding it as one would force a no-consumer
//!   subscription on the bus. The SpanRecord lives in
//!   the SQL table; the bus only carries the three
//!   audit facts.
//!
//! - #9: the ObservabilityError enum has four variants
//!   (one more than the IMPL lists). The fourth is
//!   StorageCorrupted, needed to distinguish "spans DB
//!   file missing or unwritable" (operator may be
//!   running a fresh deploy) from "spans DB file
//!   exists but its _afa_migrations table does not
//!   match the engine's expected version" (operator
//!   must investigate). Collapsing both into a single
//!   StorageUnreachable would have made the
//!   /spans/storage diagnostic endpoint uninformative.
//!
//! - #10: the IMPL's health_check example string uses
//!   "spans write failing: 1 drops in last hour"
//!   literally -- grammatically wrong for N > 1. The
//!   implementation uses format!("... {n} drops ...")
//!   so the wire form is correct for any count.
//!
//! - #11: the IMPL's record_span free-fn signature has
//!   the closure returning
//!   `Result<T, Box<dyn AfaError>>`, which forces
//!   AsyncFnOnce-style trait bounds that are
//!   unstable in Rust as of 2026-07. The Phase-1
//!   signature accepts F: Future<Output =
//!   Result<T, E>> where E: AfaError -- a regular
//!   future returning a concrete error type that
//!   satisfies the trait. The full Phase-2 helper
//!   (with attributes-cap enforcement + nested-span
//!   linking) lives in this file once Phase 2
//!   lands.
//!
//! CID Index:
//! CID:afa-observability-lib-001 -> ObservabilityEngine
//! CID:afa-observability-lib-002 -> ObservabilityConfig
//! CID:afa-observability-lib-003 -> record_span (free fn)
//! CID:afa-observability-lib-004 -> ObservabilityError
//!
//! Quick lookup: rg -n "CID:afa-observability-" crates/afa-observability/src/

pub mod error;
pub mod events;
pub mod health;
pub mod observability;
pub mod persistence;
pub mod purge;
pub mod record;

pub use error::ObservabilityError;
pub use observability::{ObservabilityConfig, ObservabilityEngine};
pub use record::{record_span, record_span_value};
