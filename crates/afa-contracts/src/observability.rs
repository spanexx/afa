//! Code Map: Observability contract surface
//! - `SpanRecord`: The 11-field "one row in the spans table"
//!   shape. Every operation the kernel dispatches gets one row.
//!   See `SpanRecord` below.
//! - `SpanOutcome`: The "did it work?" badge on a `SpanRecord`.
//!   `Ok` or `Err { kind, reason }` so the dashboard can colour
//!   the row green or red without having to parse free-form
//!   error strings. See `SpanOutcome` below.
//! - `HealthStatus`: The "is this engine OK?" badge. Three
//!   states — `Healthy`, `Degraded { reason }`, `Unhealthy {
//!   reason }` — and the `reason` field is capped at 200 chars
//!   so a runaway plugin cannot blow out the dashboard with a
//!   megabyte of log text. See `HealthStatus` below.
//! - `HealthReport`: The "how is the whole kernel?" envelope.
//!   An `overall` HealthStatus + a per-engine `BTreeMap` so
//!   the dashboard can show "afa-storage: Healthy, afa-llm:
//!   Degraded, afa-knowledge: Healthy" at a glance. See
//!   `HealthReport` below.
//! - `SpansWriteFailed`: The audit fact the observability
//!   engine publishes when a span write to the spans table
//!   fails. Carries the failure `reason` + the `correlation_id`
//!   so a follow-up query can find the request. See
//!   `SpansWriteFailed` below.
//! - `SpansPurged` / `SpansPurgeFailed`: The two audit facts
//!   the retention purge task publishes when it runs
//!   (success or failure). See the structs below.
//! - `StorageError`: The "what went wrong opening / migrating
//!   the SQLite file?" enum used by the `afa-storage` crate.
//!   Re-exported from `afa-storage` so callers see one error
//!   type, not two. See `StorageError` below.
//! - `ObservabilityErrorV1`: The four-bucket
//!   "what went wrong with observability?" enum. Maps to
//!   `AfaErrorKind` via the standard `kind()` impl.
//!   See `ObservabilityErrorV1` below.
//! - `HealthCheck`: The "I can answer a health-check" trait
//!   every engine implements. The kernel holds
//!   `Arc<dyn HealthCheck>` for every registered engine and
//!   aggregates their `health_check()` results in
//!   `Kernel::aggregate_health()`. See `HealthCheck` below.
//!
//! Story (plain English): Imagine a hospital's ward round.
//! The doctors walk from patient to patient; for each one
//! they write a one-line note on the chart (the
//! `SpanRecord`). The note has the patient's tracking number
//! (`correlation_id`), which ward they're in (`tenant_id`),
//! who paged the doctor (`actor`), what the doctor did
//! (`engine` + `operation`), when the round started
//! (`started_at`), how long it took (`duration_ms`),
//! whether the patient got better (`outcome`), and any
//! notable side-effects (`attributes`). Some patients get a
//! second chart note from a sub-specialist who was called in
//! (a nested span — the `parent_span_id` points back at the
//! first note). At the end of every round, the head doctor
//! asks every engine "are you OK?" — the answers go on a
//! big board (the `HealthReport`) so the ward clerk can see
//! "ICU: Healthy, Lab: Degraded (the analyser is backed up),
//! Pharmacy: Healthy" without having to call each engine.
//! If the chart room runs out of paper (a `SpansWriteFailed`
//! event) or the retention clerk throws out old charts (a
//! `SpansPurged` event), those are also stamped into the
//! audit log so the next morning's review can ask "why did
//! the chart room drop 200 notes last Tuesday?".
//!
//! This file is just the contract — the dictionary every
//! observability consumer agrees on. The actual engine
//! (`afa-observability`) and the storage crate
//! (`afa-storage`) are in later phases of this pack. The
//! dictionary is in `afa-contracts` because every engine
//! needs the `HealthCheck` trait and the kernel needs the
//! `HealthReport` envelope.
//!
//! CID Index:
//! CID:observability-001 -> SpanRecord
//! CID:observability-002 -> SpanOutcome
//! CID:observability-003 -> HealthStatus
//! CID:observability-004 -> HealthReport
//! CID:observability-005 -> SpansWriteFailed
//! CID:observability-006 -> SpansPurged
//! CID:observability-007 -> SpansPurgeFailed
//! CID:observability-008 -> StorageError
//! CID:observability-009 -> ObservabilityErrorV1
//! CID:observability-010 -> HealthCheck
//!
//! Quick lookup: rg -n "CID:observability-" crates/afa-contracts/src/observability.rs

use crate::error::{AfaError, AfaErrorKind};
use crate::events::AfaEvent;
use crate::execution_context::Actor;
use crate::ids::{CorrelationId, TenantId};
use crate::security::SecurityErrorV1;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use uuid::Uuid;

// CID:observability-001 - SpanRecord
// Purpose: The 11-field "one row in the spans table" shape.
// Every operation the kernel dispatches gets exactly one row,
// written by the `afa-observability::record_span` wrapper
// helper. The shape is locked: 11 fields, 10 required + 1
// optional (`parent_span_id` is the only optional field —
// the root span of a request has no parent). The
// `attributes` map is a `BTreeMap` (not `HashMap`) so the
// JSON serialisation is deterministic — a regression-proof
// for the dashboard's "same span twice should serialise to
// the same bytes" expectation. The cap on
// `attributes.len()` (64 entries / 4 KiB per value) is
// enforced by the wrapper helper, not the type — the type
// is the wire shape, the helper is the validator.
// Uses: Uuid, DateTime<Utc>, BTreeMap.
// Used by: the `afa-observability` engine (writes one per
// dispatched operation), the dashboard's `/spans/recent` and
// `/spans/{correlation_id}` query handlers, and the
// `SpansWriteFailed` audit event (which carries the
// `correlation_id` back to the operator).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpanRecord {
    /// A fresh UUID the wrapper helper generated for this
    /// span. The only field the wrapper helper is allowed to
    /// pick itself (every other field comes from the
    /// `ExecutionContext` or the operation site).
    pub span_id: Uuid,
    /// The parent span's `span_id`, if any. The root span of
    /// a request has `parent_span_id: None`; nested spans
    /// (a sub-call from inside a span) have
    /// `parent_span_id: Some(<parent's span_id>)`. The
    /// dashboard uses this to draw the "spans nested under
    /// this one" view.
    pub parent_span_id: Option<Uuid>,
    /// The tracking number from the `ExecutionContext`.
    /// Every span on a single request shares this id; the
    /// dashboard's `GET /spans/{correlation_id}` query
    /// groups by it.
    pub correlation_id: CorrelationId,
    /// The agency this span belongs to (from the
    /// `ExecutionContext`). Used for tenant isolation in
    /// multi-agency deployments.
    pub tenant_id: TenantId,
    /// The actor that started the request (from the
    /// `ExecutionContext`).
    pub actor: Actor,
    /// The engine that produced the span (e.g. `"afa-kernel"`,
    /// `"afa-llm"`, `"afa-knowledge"`). The dashboard groups
    /// by this field.
    pub engine: String,
    /// The operation within the engine (e.g.
    /// `"runtime.dispatch"`, `"llm.complete"`,
    /// `"security.unseal"`). The dashboard groups by this
    /// field.
    pub operation: String,
    /// The wall-clock time the span started (the moment
    /// the wrapper helper entered the `tracing::span!`
    /// scope).
    pub started_at: chrono::DateTime<chrono::Utc>,
    /// How long the wrapped work took, in milliseconds.
    /// `u32` is enough for ~50 days; the retention purge
    /// deletes spans older than `retention_days` (default
    /// 7) so the field never overflows in practice.
    pub duration_ms: u32,
    /// `Ok` if the wrapped work returned `Ok(_)`, or
    /// `Err { kind, reason }` if it returned `Err(_)`. The
    /// dashboard colours spans red on `Err`.
    pub outcome: SpanOutcome,
    /// Free-form key/value side-effect data (e.g.
    /// `{"model": "gpt-4o", "tokens": 1234}` for an LLM
    /// span, or `{"name": "openai-api-key", "version": 7}`
    /// for a security span). The wrapper helper caps this
    /// at 64 entries / 4 KiB per value — see
    /// `afa-observability::record_span` for the validator.
    pub attributes: BTreeMap<String, String>,
}

// CID:observability-002 - SpanOutcome
// Purpose: The "did it work?" badge on a `SpanRecord`.
// `Ok` for a successful operation, `Err { kind, reason }`
// for a failed one. The two fields on `Err` carry enough
// for the dashboard to colour-code and label the row
// without having to parse free-form error strings:
// `kind` is one of the six coarse `AfaErrorKind` buckets
// (the same one the rest of the kernel uses), and `reason`
// is the human-readable `Display` of the underlying error
// (truncated to 1 KiB so a runaway plugin cannot blow out
// the spans table).
// Uses: AfaErrorKind.
// Used by: SpanRecord, the dashboard's "red/green" colouring.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum SpanOutcome {
    /// The wrapped work returned `Ok(_)`.
    Ok,
    /// The wrapped work returned `Err(e)`. The `kind` is
    /// the same `AfaErrorKind` bucket the kernel's generic
    /// error handler would have classified `e` into; the
    /// `reason` is `e`'s `Display` output (truncated to 1
    /// KiB by the wrapper helper).
    ///
    /// Note: the default externally-tagged serde
    /// representation is used (the `#[serde(tag = "kind")]`
    /// shortcut would conflict with the `kind` field on
    /// the `Err` variant — serde forbids a field name that
    /// matches the tag). The wire form is therefore
    /// `{"Ok": null}` for the `Ok` variant and
    /// `{"Err": {"kind": "Unavailable", "reason": "..."}}`
    /// for the `Err` variant. This is the
    /// regression-proof target — a future change to the
    /// serde attributes is caught by the
    /// `observability_types` round-trip test.
    Err { kind: AfaErrorKind, reason: String },
}

// CID:observability-003 - HealthStatus
// Purpose: The "is this engine OK?" badge. Three states —
// `Healthy` (no problems), `Degraded { reason }` (working
// but with a known issue, e.g. "spans write failing: 3
// drops in last hour"), and `Unhealthy { reason }` (not
// working at all, e.g. "secrets storage unreachable"). The
// `reason` field is capped at 200 chars in the `Display`
// impl (not in the type — the type is the wire shape) so
// a runaway plugin cannot blow out the dashboard with a
// megabyte of log text. The cap is enforced by truncating
// in `Display`; a regression-proof in
// `tests/health_check_shape.rs` asserts the truncation
// behavior.
// Uses: nothing — it is just a label.
// Used by: every engine's `HealthCheck` impl, the kernel's
// `aggregate_health()` worst-wins aggregator, and the
// dashboard's `GET /health` handler.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum HealthStatus {
    /// The engine is fully operational.
    Healthy,
    /// The engine is working but with a known issue. The
    /// `reason` field is a short human-readable label
    /// (capped at 200 chars in `Display`).
    Degraded { reason: String },
    /// The engine is not operational. The `reason` field
    /// is a short human-readable label (capped at 200
    /// chars in `Display`).
    Unhealthy { reason: String },
}

impl HealthStatus {
    /// Maximum length of the `reason` field in
    /// `Display` output. Longer reasons are truncated and
    /// suffixed with `"..."` (so the truncation is
    /// observable in the wire format).
    pub const REASON_DISPLAY_CAP: usize = 200;
}

impl fmt::Display for HealthStatus {
    /// Renders the status for log lines and the dashboard
    /// health board. The `reason` field is truncated to
    /// `REASON_DISPLAY_CAP` (200) chars so a runaway plugin
    /// cannot blow out the dashboard with a megabyte of
    /// log text.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Healthy => f.write_str("healthy"),
            Self::Degraded { reason } => {
                f.write_str("degraded:")?;
                truncate_for_display(f, reason, Self::REASON_DISPLAY_CAP)
            }
            Self::Unhealthy { reason } => {
                f.write_str("unhealthy:")?;
                truncate_for_display(f, reason, Self::REASON_DISPLAY_CAP)
            }
        }
    }
}

fn truncate_for_display(f: &mut fmt::Formatter<'_>, s: &str, cap: usize) -> fmt::Result {
    if s.len() <= cap {
        f.write_str(s)
    } else {
        // Truncate on a char boundary to keep the output
        // valid UTF-8. The `take` + `chars` round-trip
        // ensures we never split a multi-byte code point.
        let truncated: String = s.chars().take(cap).collect();
        write!(f, "{truncated}...")
    }
}

// CID:observability-004 - HealthReport
// Purpose: The "how is the whole kernel?" envelope. An
// `overall` HealthStatus (the worst-wins aggregation of
// every engine's status) + a per-engine `BTreeMap<String,
// HealthStatus>` so the dashboard can show
// "afa-storage: Healthy, afa-llm: Degraded, afa-knowledge:
// Healthy" at a glance. The `BTreeMap` (not `HashMap`) keeps
// the JSON serialisation deterministic — a regression-proof
// for the dashboard's "same engine set twice should
// serialise to the same bytes" expectation.
// `checked_at` is the wall-clock time the kernel built the
// report (NOT the time each engine was queried — that is
// its own per-engine `last_checked_at` field, which is a
// future pack).
// Uses: HealthStatus, BTreeMap, DateTime<Utc>.
// Used by: the kernel's `aggregate_health()` method, the
// dashboard's `GET /health` handler.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HealthReport {
    /// The worst-wins aggregation of every engine's status.
    /// `Unhealthy` beats `Degraded` beats `Healthy`.
    pub overall: HealthStatus,
    /// Per-engine status, keyed by the engine's name
    /// (e.g. `"afa-llm"`, `"afa-knowledge"`).
    pub engines: BTreeMap<String, HealthStatus>,
    /// The wall-clock time the kernel built the report.
    pub checked_at: chrono::DateTime<chrono::Utc>,
}

// CID:observability-005 - SpansWriteFailed
// Purpose: The audit fact the observability engine publishes
// on the event bus when a span write to the spans table
// fails. Carries the failure `count` (usually 1, but
// could be batched), `reason` (the underlying error's
// `Display`), `occurred_at` (the wall-clock time the
// failure was observed), and the `correlation_id` so a
// follow-up `GET /spans/{correlation_id}` can find the
// request that produced the dropped span.
// Uses: AfaEvent (so it can ride the bus), serde, chrono.
// Used by: the observability engine's `record_span` failure
// path, and any dashboard or anomaly detector subscribed
// to observability events.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpansWriteFailed {
    /// How many span writes failed in this batch (usually
    /// 1; the field exists so a future batched-write
    /// failure can publish one event with a higher count).
    pub count: u32,
    /// The underlying error's `Display` output (truncated
    /// to 1 KiB by the engine to bound the event size).
    pub reason: String,
    /// The wall-clock time the engine saw the failure.
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    /// The tracking number from the request that produced
    /// the dropped span. A follow-up
    /// `GET /spans/{correlation_id}` will return 204
    /// (no spans written), so this event is the only
    /// forensic trace of the dropped span.
    pub correlation_id: CorrelationId,
}

impl AfaEvent for SpansWriteFailed {}

// CID:observability-006 - SpansPurged
// Purpose: The audit fact the retention purge task publishes
// on the event bus when a purge run succeeds. Carries the
// `count` of spans deleted, the `older_than` threshold
// (so an operator can confirm the right rows were
// targeted), the `occurred_at` time, and the
// `correlation_id` of the timer-driven purge task (every
// task gets its own `ExecutionContext` with a fresh id).
// Uses: AfaEvent, serde, chrono, CorrelationId.
// Used by: dashboards (to show "purged 12,400 spans at
// 03:00"), anomaly detectors (to flag a sudden spike in
// purge volume), and capacity planners.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpansPurged {
    /// How many spans were deleted in this purge run.
    pub count: u32,
    /// The cutoff timestamp — every span with
    /// `started_at < older_than` was deleted. Stored as a
    /// wall-clock `DateTime<Utc>` so a reviewer can
    /// compare against the configured
    /// `retention_days` directly.
    pub older_than: chrono::DateTime<chrono::Utc>,
    /// The wall-clock time the purge completed.
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    /// The tracking number of the timer-driven purge task.
    pub correlation_id: CorrelationId,
}

impl AfaEvent for SpansPurged {}

// CID:observability-007 - SpansPurgeFailed
// Purpose: The audit fact the retention purge task publishes
// on the event bus when a purge run fails (e.g. the spans
// DB file became unreadable mid-purge). Carries the
// `count` of spans the engine TRIED to delete before the
// failure (often 0 if the failure happened on the first
// chunk), the `reason` (the underlying error's `Display`),
// the `older_than` threshold, the `occurred_at` time, and
// the `correlation_id` of the timer-driven task.
// Uses: AfaEvent, serde, chrono, CorrelationId.
// Used by: dashboards (to show "purge run failed at 03:00
// — see logs"), the engine's `HealthCheck` impl (a
// `SpansPurgeFailed` event in the last hour flags the
// engine as `Degraded`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SpansPurgeFailed {
    /// How many spans the engine tried to delete before
    /// the failure. Often 0 (the failure usually happens
    /// on the first chunk).
    pub count: u32,
    /// The underlying error's `Display` output (truncated
    /// to 1 KiB by the engine).
    pub reason: String,
    /// The cutoff timestamp — the same `older_than` value
    /// the engine was using when the failure happened.
    pub older_than: chrono::DateTime<chrono::Utc>,
    /// The wall-clock time the engine saw the failure.
    pub occurred_at: chrono::DateTime<chrono::Utc>,
    /// The tracking number of the timer-driven purge task.
    pub correlation_id: CorrelationId,
}

impl AfaEvent for SpansPurgeFailed {}

// CID:observability-008 - StorageError
// Purpose: The "what went wrong opening / migrating the
// SQLite file?" enum used by the `afa-storage` crate.
// Re-exported from `afa-storage` so callers see one error
// type, not two. The four-variant closed set covers the
// only four things that can go wrong at boot-time
// (file is unreachable, schema migration failed, the
// file is locked by another holder — e.g. a second
// `afa-kernel` process pointing at the same file, and a
// generic "the closure handed me an error" for engine-
// level failures that surface through `with_conn`).
// `thiserror::Error` is the derive so the type is
// `std::error::Error`-compatible and the `Display` impl
// is the one shown in the kernel's panic messages.
// Uses: thiserror.
// Used by: the `afa-storage` crate (the only producer) and
// every caller that opens a `Storage` (afa-observability,
// afa-security, future packs).
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The SQLite file at the configured path is not
    /// reachable (path doesn't exist and can't be created,
    /// or the directory is not writable). The `io::Error`
    /// is the underlying cause.
    #[error("failed to open SQLite file: {0}")]
    Open(std::io::Error),
    /// A migration script failed. The `version` is the
    /// migration that failed; the `source` is the
    /// underlying `rusqlite::Error`.
    #[error("migration {version} failed: {source}")]
    Migrate {
        version: u32,
        #[source]
        source: rusqlite::Error,
    },
    /// The SQLite file is locked by another holder (e.g. a
    /// second `afa-kernel` process pointing at the same
    /// file).
    #[error("storage is locked by another holder")]
    Locked,
    /// An engine's `with_conn` closure returned a non-SQL
    /// error (typically the engine's own `SecurityErrorV1`
    /// or `KnowledgeErrorV1`). The boxed `dyn Error` holds
    /// the engine error so the downstream caller can
    /// downcast to the engine's concrete type if it wants
    /// to (e.g. the security engine's `seal` call wraps
    /// this back into `SecurityError::StorageCorrupted`).
    /// This is **doc drift correction #7 vs. the IMPL
    /// draft** — the IMPL's "the closure returns
    /// `Result<T, E>` for any `E: Into<StorageError>`"
    /// framing required a `From<SecurityErrorV1>` impl that
    /// tried to wrap a `String` as a `Box<dyn Error>`,
    /// which does not compile (`String` does not implement
    /// `Error`). The Closure variant is the corrected
    /// surface: any engine error is boxed in here.
    #[error("storage closure failed: {0}")]
    Closure(Box<dyn std::error::Error + Send + Sync>),
}

impl AfaError for StorageError {
    fn kind(&self) -> AfaErrorKind {
        match self {
            // Boot-time and storage-class failures all
            // map to `Unavailable`: the engine is
            // "temporarily down" in the same way a
            // database outage is "temporarily down" — the
            // fix is operator action (restore the file,
            // kill the other process), not a client
            // retry.
            Self::Open(_) | Self::Migrate { .. } | Self::Locked => AfaErrorKind::Unavailable,
            // The engine-closure error is whatever kind
            // the engine mapped it to. The engine is
            // responsible for calling `.map_err` to
            // preserve the specific kind (e.g. the
            // security engine's `SecretRotated` is a
            // `NotFound` kind). Mapping it to `Internal`
            // here would silently hide the engine's
            // semantic; mapping it to the engine's kind
            // requires the engine to set the kind before
            // crossing the `with_conn` boundary, which
            // is what the engine's
            // `.map_err(SecurityError::StorageCorrupted)`
            // call site does — so the Closure variant
            // here carries the engine's mapped error and
            // the `with_conn` caller unwraps it back into
            // the engine's error type.
            Self::Closure(_) => AfaErrorKind::Internal,
        }
    }
}

// `From<rusqlite::Error>` so an engine's `with_conn`
// closure that returns `Result<T, rusqlite::Error>`
// (the common case for SQL-only closures) can `?`
// the error without an explicit `.map_err`. The
// `version: 0` in the `Migrate` arm is a placeholder —
// `with_conn` is not a migration, it's a runtime
// read/write; the migration version is unknown at the
// `with_conn` level. The downstream caller (the engine)
// maps the `StorageError` to its own error type with
// the version it knows about.
//
// **Doc drift correction #7 vs. the IMPL draft**:
// the IMPL promised `From<SecurityErrorV1>` and
// `From<rusqlite::Error>` `From` impls; the
// `From<SecurityErrorV1>` impl was the broken one
// (see the Closure variant above). The
// `From<rusqlite::Error>` impl is the one that
// actually compiles and is the one used by every
// `with_conn` closure that returns `rusqlite::Error`.
impl From<rusqlite::Error> for StorageError {
    fn from(e: rusqlite::Error) -> Self {
        StorageError::Migrate {
            version: 0,
            source: e,
        }
    }
}

// `From<SecurityErrorV1>` so an engine's `with_conn`
// closure that returns `Result<T, SecurityErrorV1>`
// (the security engine pattern — the closure can
// return business-logic errors like `SecretRotated`
// that the SQL `?` operator does not produce) can
// cross the `with_conn` boundary. The engine error
// is boxed into the `Closure` variant; the downstream
// caller (the engine's `with_conn(...).await?` site)
// maps the boxed error back to `SecurityError` via
// the `From<Box<dyn Error + Send + Sync>>` impl on
// `SecurityErrorV1` (the kernel and the engine
// share the same `Storage` so the engine is the
// only `with_conn` caller that needs to round-trip
// a `SecurityErrorV1`).
impl From<SecurityErrorV1> for StorageError {
    fn from(e: SecurityErrorV1) -> Self {
        StorageError::Closure(Box::new(e))
    }
}

// CID:observability-009 - ObservabilityErrorV1
// Purpose: The four-bucket "what went wrong with
// observability?" enum. Maps to `AfaErrorKind` via the
// standard `kind()` impl. The four variants are the only
// things that can go wrong with the observability engine
// at the contract surface (a richer set of failures is
// surfaced via the `SpansWriteFailed` / `SpansPurgeFailed`
// audit events, not via this error type). The `From` impl
// to `AfaError` is in `error.rs` of the
// `afa-observability` crate (so the conversion is one
// place, not duplicated here).
// Uses: thiserror.
// Used by: the `afa-observability` engine (the only
// producer) and the kernel's `aggregate_health()` worst-
// wins aggregator.
#[derive(Debug, thiserror::Error)]
pub enum ObservabilityErrorV1 {
    /// The spans DB file is unreachable (e.g. the
    /// configured `spans_db_path` is in a directory that
    /// doesn't exist and can't be created).
    #[error("spans storage is unreachable: {reason}")]
    StorageUnreachable { reason: String },
    /// The spans DB file is reachable but its contents
    /// are not readable as expected (e.g. truncated, or
    /// the magic bytes are wrong).
    #[error("spans storage is corrupted")]
    StorageCorrupted,
    /// The spans DB file's `schema_version` is not the one
    /// this engine version supports. The admin must run
    /// the migration tool from a later pack.
    #[error("spans storage schema version mismatch (found {found}, expected {expected})")]
    SchemaVersionMismatch { found: u32, expected: u32 },
    /// Catch-all for unexpected internal failures (bugs,
    /// invariant violations, an `.await` panic, etc.).
    #[error("observability engine internal error: {reason}")]
    Internal { reason: String },
}

impl AfaError for ObservabilityErrorV1 {
    fn kind(&self) -> AfaErrorKind {
        match self {
            // All three storage-class failures map to
            // `Unavailable`: the engine is "temporarily
            // down" in the same way a database outage is
            // "temporarily down" — the fix is operator
            // action, not a client retry.
            Self::StorageUnreachable { .. }
            | Self::StorageCorrupted
            | Self::SchemaVersionMismatch { .. } => AfaErrorKind::Unavailable,
            // Bugs and invariant violations.
            Self::Internal { .. } => AfaErrorKind::Internal,
        }
    }
}

// CID:observability-010 - HealthCheck
// Purpose: The "I can answer a health-check" trait every
// engine implements. The kernel holds
// `Arc<dyn HealthCheck>` for every registered engine and
// aggregates their `health_check()` results in
// `Kernel::aggregate_health()`. The `Send + Sync` supertrait
// lets the kernel share the trait object across tasks (the
// same pattern `SecurityV1` uses); the `'static` bound
// is the standard one for trait objects that outlive a
// single request.
// The method is `&self` (not `&mut self`) and synchronous
// (not `async`) on purpose: the kernel's aggregator
// uses `catch_unwind` + a 100ms timeout per engine, and
// an `async` method would force the trait object behind
// a `Pin<Box<dyn Future>>` and break the panic-isolation
// pattern. An engine that needs to do async I/O to answer
// the health check is expected to maintain a cached
// `HealthStatus` field (updated by its background task)
// and return the cached value from `health_check()`.
// Uses: HealthStatus.
// Used by: the observability engine (implements), the
// security engine (implements), the LLM engine
// (implements), the knowledge engine (implements), and the
// kernel's `aggregate_health()` method.
pub trait HealthCheck: Send + Sync + 'static {
    /// Return the engine's current health status. Must be
    /// cheap (no I/O, no locks held across calls) — the
    /// kernel's aggregator calls this for every engine
    /// inside a 100ms-per-engine timeout.
    fn health_check(&self) -> HealthStatus;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_status_display_healthy() {
        let s = HealthStatus::Healthy;
        assert_eq!(format!("{s}"), "healthy");
    }

    #[test]
    fn health_status_display_degraded_short_reason() {
        let s = HealthStatus::Degraded {
            reason: "3 drops in last hour".into(),
        };
        assert_eq!(format!("{s}"), "degraded:3 drops in last hour");
    }

    #[test]
    fn health_status_display_unhealthy_short_reason() {
        let s = HealthStatus::Unhealthy {
            reason: "secrets storage unreachable".into(),
        };
        assert_eq!(format!("{s}"), "unhealthy:secrets storage unreachable");
    }

    #[test]
    fn health_status_display_truncates_long_reason() {
        // 250 'x' chars > 200 cap. The Display impl
        // truncates to 200 and suffixes "...".
        let long = "x".repeat(250);
        let s = HealthStatus::Degraded { reason: long };
        let out = format!("{s}");
        assert!(
            out.starts_with("degraded:"),
            "prefix must be intact: got {out:?}"
        );
        // The prefix + the 200 'x' + the "..." = 9 + 200 + 3 = 212.
        assert_eq!(out.len(), 9 + 200 + 3, "got {out:?}");
        assert!(out.ends_with("..."), "must end with ellipsis");
    }

    #[test]
    fn health_status_display_200_char_reason_not_truncated() {
        // Exactly 200 chars: not truncated (no ellipsis).
        let exact = "y".repeat(200);
        let s = HealthStatus::Unhealthy { reason: exact };
        let out = format!("{s}");
        // prefix "unhealthy:" is 10 chars + 200 'y' = 210.
        assert_eq!(out.len(), 10 + 200, "got {out:?}");
        assert!(!out.ends_with("..."), "must not be truncated");
    }

    #[test]
    fn span_outcome_ok_round_trips_through_serde_json() {
        let o = SpanOutcome::Ok;
        let json = serde_json::to_string(&o).unwrap();
        let back: SpanOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(o, back);
    }

    #[test]
    fn span_outcome_err_round_trips_through_serde_json() {
        let o = SpanOutcome::Err {
            kind: AfaErrorKind::Unavailable,
            reason: "spans DB chmod 000".into(),
        };
        let json = serde_json::to_string(&o).unwrap();
        let back: SpanOutcome = serde_json::from_str(&json).unwrap();
        assert_eq!(o, back);
    }

    #[test]
    fn health_status_round_trips_through_serde_json() {
        let statuses = vec![
            HealthStatus::Healthy,
            HealthStatus::Degraded {
                reason: "drops".into(),
            },
            HealthStatus::Unhealthy {
                reason: "down".into(),
            },
        ];
        for s in statuses {
            let json = serde_json::to_string(&s).unwrap();
            let back: HealthStatus = serde_json::from_str(&json).unwrap();
            assert_eq!(s, back);
        }
    }

    #[test]
    fn storage_error_maps_to_unavailable() {
        // The `Open` variant's inner `io::Error` is the
        // "permission denied" style — wrap one just for
        // the mapping test.
        let e = StorageError::Open(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "no write",
        ));
        assert_eq!(e.kind(), AfaErrorKind::Unavailable);

        let e = StorageError::Migrate {
            version: 1,
            source: rusqlite::Error::QueryReturnedNoRows,
        };
        assert_eq!(e.kind(), AfaErrorKind::Unavailable);

        let e = StorageError::Locked;
        assert_eq!(e.kind(), AfaErrorKind::Unavailable);
    }

    #[test]
    fn observability_error_maps_to_unavailable_or_internal() {
        assert_eq!(
            ObservabilityErrorV1::StorageUnreachable { reason: "x".into() }.kind(),
            AfaErrorKind::Unavailable
        );
        assert_eq!(
            ObservabilityErrorV1::StorageCorrupted.kind(),
            AfaErrorKind::Unavailable
        );
        assert_eq!(
            ObservabilityErrorV1::SchemaVersionMismatch {
                found: 1,
                expected: 2
            }
            .kind(),
            AfaErrorKind::Unavailable
        );
        assert_eq!(
            ObservabilityErrorV1::Internal { reason: "x".into() }.kind(),
            AfaErrorKind::Internal
        );
    }
}
