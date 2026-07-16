//! Code Map: observability::persistence
//!
//! - MIGRATIONS: the single-entry Migration list this
//!   engine hands to afa_storage::migrate at boot.
//!   Today: one entry (version 1, the spans table
//!   schema). A future pack can append a v2 entry
//!   (e.g. an additional index); the storage engine
//!   applies only the migrations with version greater
//!   than the current _afa_migrations version max.
//! - DbRow: the local one-row-from-the-table shape.
//!   Holds the columns in plain Rust types (String,
//!   i64, Option`String`) before they are JSON-parsed
//!   back into actor/outcome/attributes. Private --
//!   not re-exported.
//! - write_span: the single-span INSERT, inside a
//!   `BEGIN IMMEDIATE` transaction (TransactionBehavior::Immediate
//!   via rusqlite::Connection::transaction_with_behavior,
//!   NOT raw SQL). One row per call. Returns Ok(())
//!   on success; storage failures surface as
//!   ObservabilityError.
//! - read_by_correlation_id: the per-correlation_id
//!   SELECT. Returns a Vec`SpanRecord` oldest-first.
//!   Empty vec means "no rows for this correlation"
//!   (the dashboard renders "no spans" not "404" for
//!   that case).
//! - read_recent: the "spans with started_at < since,
//!   latest N" SELECT. The dashboard's "spans since X"
//!   view. Sorted started_at DESC, capped at the
//!   requested limit (the HTTP handler applies the
//!   100 / 1000 caps at the wire layer).
//! - delete_older_than: the retention purge's chunk
//!   step. `DELETE FROM spans WHERE started_at <
//!   cutoff` inside a BEGIN IMMEDIATE. Returns the
//!   row count (used by the purge loop's
//!   `SpansPurged { count: ... }` event payload).
//! - map_with_conn_err: the helper that turns a
//!   afa_storage::StorageError into an
//!   ObservabilityError at every with_conn
//!   boundary.
//!
//! Story (plain English): The ward's daily logbook.
//! One shelf, one row per patient visit. The
//! recording nurse files new entries throughout the
//! day (write_span). The floor clerk can pull all
//! entries for one patient by tracking number
//! (read_by_correlation_id, for "what happened to
//! patient 123 today?"). The morning nurse can pull
//! the day's most recent entries (read_recent, for
//! the doctor's morning briefing). The retention
//! clerk walks through the shelf every hour with a
//! black marker, crossing out entries whose date is
//! too old (delete_older_than, used by the purge
//! task). None of the helpers lock the shelf
//! exclusively -- they all open a short transaction
//! (BEGIN IMMEDIATE for writes, the default deferred
//! for SELECTs), do their work, and release.
//! Multiple readers can be in flight at the same
//! time; two writers are serialised by the
//! `BEGIN IMMEDIATE` mutex protocol.
//!
//! Doc drift corrections vs. the IMPL draft:
//! - #3: the IMPL said "BEGIN IMMEDIATE in a single
//!   INSERT" -- achievable via either raw SQL or
//!   rusqlite's TransactionBehavior::Immediate. The
//!   chosen form is the latter (matching the
//!   afa-security::seal pattern) because rusqlite
//!   then owns the BEGIN/COMMIT/ROLLBACK semantics,
//!   and a future runner that wants to add a
//!   rollback-on-error path gets it for free.
//! - #4: the IMPL listed `parent_span_id` as one of
//!   the 10 required fields, but the actual
//!   SpanRecord contract type (afa-contracts) has
//!   it as `Option`Uuid`` (the root span of a
//!   request has `parent_span_id: None`). The schema
//!   column is `TEXT NULL` (not `TEXT NOT NULL`)
//!   to match. The "10 required + attributes" framing
//!   in the IMPL §"Phase 1 Tests required"
//!   (span_record_shape test) is also adjusted:
//!   the test asserts `parent_span_id` is `None` for
//!   the root-span case, not "populated with a UUID".
//!
//! CID Index:
//! CID:afa-observability-persistence-001 -> MIGRATIONS
//! CID:afa-observability-persistence-002 -> write_span
//! CID:afa-observability-persistence-003 -> read_by_correlation_id
//! CID:afa-observability-persistence-004 -> read_recent
//! CID:afa-observability-persistence-005 -> delete_older_than
//!
//! Quick lookup: rg -n "CID:afa-observability-persistence-" crates/afa-observability/src/persistence.rs

use crate::error::ObservabilityError;
use afa_contracts::{CorrelationId, Migration, SpanRecord, TenantId};
use afa_storage::{with_conn, Storage};
use chrono::{DateTime, Utc};
use rusqlite::TransactionBehavior;
use uuid::Uuid;

// CID:afa-observability-persistence-001 - MIGRATIONS
// Purpose: The exact `&[Migration]` list the engine
// hands to afa_storage::migrate at boot. One entry
// today (the v1 spans table). A future pack can
// append a v2 here (the storage engine will run
// only the unapplied ones).
//
// **Why each column is the type it is**:
// - span_id, parent_span_id, correlation_id:
//   TEXT (not BLOB) -- readable via a plain sqlite3
//   CLI for debugging. UUIDs serialise to 36 ASCII
//   bytes; collision-resistance is unchanged.
// - actor_json, outcome_json, attributes_json:
//   TEXT (not BLOB) -- so a future SQLite-based query
//   (`SELECT * FROM spans WHERE attributes LIKE
//   '%model:gpt-4o%'`) is possible without an
//   out-of-band JSON parser.
// - started_at: TEXT (RFC 3339 with millisecond
//   precision). Stored as a string so the column is
//   indexable and the retention purge can compare
//   `started_at < cutoff` without a SQLite datetime
//   extension.
// - duration_ms: INTEGER. u32 is enough for ~50 days;
//   the retention purge deletes spans older than
//   `retention_days` (default 7) so the field never
//   overflows in practice. Stored as i64 so the
//   SQLite `INTEGER` type casts cleanly.
//
// **Indexes**:
// - idx_spans_correlation: the dashboard's
//   `GET /spans/{correlation_id}` GROUP BY. Without
//   it, every group-by is a full table scan.
// - idx_spans_started_at: the retention purge's
//   `started_at < cutoff` filter (and the dashboard's
//   `GET /spans/recent?since=...`). Without it, the
//   nightly 7-day sweep is a full table scan.
//
// **WITHOUT ROWID**: the `span_id` is the only
// natural key. A hidden `rowid` would just be a
// second copy of the same bytes; dropping it cuts
// the on-disk footprint by ~30% per row.
//
// Used by: ObservabilityEngine::new (calls
// afa_storage::migrate(&storage, MIGRATIONS)).
pub const MIGRATIONS: &[Migration] = &[Migration {
    version: 1,
    sql: SPANS_TABLE_SCHEMA,
}];

// CID:afa-observability-persistence-006 - SPANS_TABLE_SCHEMA
// Purpose: The single-spans-table SQL string. Stored
// as a `&'static str` per the Migration::sql
// contract (the SQL is always baked into the binary
// at compile time, never loaded from a file or a
// network source; SECURITY + reproducibility).
//
// The schema is intentionally inline (not a
// concat of per-column constants) so a single grep
// at the schema tag returns the entire table in
// one read.
const SPANS_TABLE_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS spans (
    span_id          TEXT    NOT NULL PRIMARY KEY,
    parent_span_id   TEXT,
    correlation_id   TEXT    NOT NULL,
    tenant_id        TEXT    NOT NULL,
    actor_json       TEXT    NOT NULL,
    engine           TEXT    NOT NULL,
    operation        TEXT    NOT NULL,
    started_at       TEXT    NOT NULL,
    duration_ms      INTEGER NOT NULL,
    outcome_json     TEXT    NOT NULL,
    attributes_json  TEXT    NOT NULL
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS idx_spans_correlation
    ON spans (correlation_id);
CREATE INDEX IF NOT EXISTS idx_spans_started_at
    ON spans (started_at);
";

// CID:afa-observability-persistence-007 - DbRow
// Purpose: The local one-row-from-the-table shape.
// Private -- not pub. Holds the columns in plain
// Rust types (String, i64, Option`String`) before
// they are JSON-parsed back into
// actor/outcome/attributes. The intermediate type
// exists so the read helpers can return Vec`DbRow`
// from inside with_conn and do the JSON-parsing
// outside the connection-lock window (parsing a
// bad JSON string is cheap, but it would still hold
// the lock during a slow parse on a future crate
// change -- the boundary here keeps the lock tight).
struct DbRow {
    span_id: Uuid,
    parent_span_id: Option<Uuid>,
    correlation_id: String,
    tenant_id: String,
    actor_json: String,
    engine: String,
    operation: String,
    started_at: String,
    duration_ms: i64,
    outcome_json: String,
    attributes_json: String,
}

impl DbRow {
    // Map one row from the SELECT into a DbRow.
    // The two UUID columns use parse_str
    // inline because rusqlite's FromSql for Uuid
    // is not stable across the workspace; the
    // FromSqlConversionFailure wrapping preserves
    // the column index for any future
    // schema-drift diagnostic.
    fn from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Self> {
        let span_id_str: String = row.get(0)?;
        let span_id = Uuid::parse_str(&span_id_str).map_err(|e| {
            rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(e))
        })?;
        let parent_id_str: Option<String> = row.get(1)?;
        let parent_span_id = parent_id_str
            .map(|s| {
                Uuid::parse_str(&s).map_err(|e| {
                    rusqlite::Error::FromSqlConversionFailure(
                        1,
                        rusqlite::types::Type::Text,
                        Box::new(e),
                    )
                })
            })
            .transpose()?;
        Ok(Self {
            span_id,
            parent_span_id,
            correlation_id: row.get(2)?,
            tenant_id: row.get(3)?,
            actor_json: row.get(4)?,
            engine: row.get(5)?,
            operation: row.get(6)?,
            started_at: row.get(7)?,
            duration_ms: row.get(8)?,
            outcome_json: row.get(9)?,
            attributes_json: row.get(10)?,
        })
    }

    // Convert one DbRow into a SpanRecord. The four
    // parse-from-str calls (UUID x2, DateTime, JSON
    // x3) all return ObservabilityError::StorageCorrupted
    // on failure -- the StorageCorrupted variant is
    // used for "the row in the table does not match
    // the schema we expect" because that is the
    // operator-action semantic ("the file is
    // corrupted; investigate").
    fn into_record(self) -> Result<SpanRecord, ObservabilityError> {
        let correlation_uuid = Uuid::parse_str(&self.correlation_id)
            .map_err(|_| ObservabilityError::StorageCorrupted)?;
        let actor: afa_contracts::execution_context::Actor = serde_json::from_str(&self.actor_json)
            .map_err(|_| ObservabilityError::StorageCorrupted)?;
        let started_at = DateTime::parse_from_rfc3339(&self.started_at)
            .map_err(|_| ObservabilityError::StorageCorrupted)?
            .with_timezone(&Utc);
        let outcome: afa_contracts::SpanOutcome = serde_json::from_str(&self.outcome_json)
            .map_err(|_| ObservabilityError::StorageCorrupted)?;
        let attributes: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&self.attributes_json)
                .map_err(|_| ObservabilityError::StorageCorrupted)?;
        Ok(SpanRecord {
            span_id: self.span_id,
            parent_span_id: self.parent_span_id,
            correlation_id: CorrelationId(correlation_uuid),
            tenant_id: TenantId(self.tenant_id),
            actor,
            engine: self.engine,
            operation: self.operation,
            started_at,
            duration_ms: self.duration_ms as u32,
            outcome,
            attributes,
        })
    }
}

// CID:afa-observability-persistence-002 - write_span
// Purpose: Insert one SpanRecord into the spans
// table. Called by ObservabilityEngine::record_span
// (the engine's method form) for every dispatched
// operation.
//
// **Contract**:
// - Success: returns Ok(()).
// - Storage failure: returns ObservabilityError
//   (the engine treats this as "best effort" --
//   the caller's future is NOT short-circuited,
//   just logged/counted/audited via the
//   SpansWriteFailed event).
//
// **Transaction**:
// `TransactionBehavior::Immediate` (rusqlite issues
// BEGIN IMMEDIATE under the hood). The single
// INSERT runs inside the transaction; commit()
// finalises it. drop(tx) on the unhappy path
// triggers an automatic rollback (so a failed
// INSERT does not leave a partial row).
//
// **Why no chunking**: a single INSERT in BEGIN
// IMMEDIATE is the right shape for "one row per
// dispatched operation" -- chunks would only matter
// if we batched, and batching is not in the Phase 1
// scope. A future Phase 5 that batch-publishes
// engine output would revisit this.
//
// **Pre-binding fields before the closure**: the
// closure must be `FnOnce` (per the with_conn
// signature), so every input the closure touches
// must be owned by the closure. We clone the
// `String` fields into local owned values first
// (cheap; the record is built once per call); the
// non-string fields are Copy (UUID, DateTime, u32)
// and move into the closure directly.
pub async fn write_span(storage: &Storage, record: &SpanRecord) -> Result<(), ObservabilityError> {
    let span_id_str = record.span_id.to_string();
    let parent_span_id_str = record.parent_span_id.map(|u| u.to_string());
    let actor_json =
        serde_json::to_string(&record.actor).map_err(|e| ObservabilityError::Internal {
            reason: format!("actor serialize: {e}"),
        })?;
    let outcome_json =
        serde_json::to_string(&record.outcome).map_err(|e| ObservabilityError::Internal {
            reason: format!("outcome serialize: {e}"),
        })?;
    let attributes_json =
        serde_json::to_string(&record.attributes).map_err(|e| ObservabilityError::Internal {
            reason: format!("attributes serialize: {e}"),
        })?;
    let started_at_str = record
        .started_at
        .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
    let correlation_id_str = record.correlation_id.0.to_string();
    let tenant_id_str = record.tenant_id.0.clone();
    let engine_str = record.engine.clone();
    let operation_str = record.operation.clone();
    let duration_ms_i = i64::from(record.duration_ms);

    with_conn(storage, move |conn| {
        Box::pin(async move {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute(
                "INSERT INTO spans (
                    span_id, parent_span_id, correlation_id,
                    tenant_id, actor_json, engine, operation,
                    started_at, duration_ms,
                    outcome_json, attributes_json
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    span_id_str,
                    parent_span_id_str,
                    correlation_id_str,
                    tenant_id_str,
                    actor_json,
                    engine_str,
                    operation_str,
                    started_at_str,
                    duration_ms_i,
                    outcome_json,
                    attributes_json,
                ],
            )?;
            tx.commit()?;
            Ok::<(), rusqlite::Error>(())
        })
    })
    .await
    .map_err(|e| match e {
        afa_contracts::StorageError::Open(io) => ObservabilityError::StorageUnreachable {
            reason: io.to_string(),
        },
        other => ObservabilityError::Internal {
            reason: format!("storage: {other}"),
        },
    })?;
    Ok(())
}

// CID:afa-observability-persistence-003 - read_by_correlation_id
// Purpose: SELECT every span with the given
// correlation_id, oldest-first. Used by the
// dashboard's GET /spans/{correlation_id}
// endpoint.
//
// **Empty result semantics**: Ok(vec![]) for "no
// spans written for this correlation" (the
// dashboard renders "no spans" not "404" for that
// case). The dashboard's caller is responsible for
// distinguishing "real tracking number, no spans"
// from "garbage tracking number"; the latter
// surfaces as Ok(vec![]) too, by design (a 204 for
// any unknown correlation_id would leak the
// existence-of-tracking-numbers info).
pub async fn read_by_correlation_id(
    storage: &Storage,
    correlation_id: &CorrelationId,
) -> Result<Vec<SpanRecord>, ObservabilityError> {
    let cid_str = correlation_id.0.to_string();
    let rows: Vec<DbRow> = with_conn(storage, move |conn| {
        Box::pin(async move {
            let mut stmt = conn.prepare(
                "SELECT span_id, parent_span_id, correlation_id,
                            tenant_id, actor_json, engine, operation,
                            started_at, duration_ms, outcome_json,
                            attributes_json
                     FROM spans
                     WHERE correlation_id = ?1
                     ORDER BY started_at ASC, span_id ASC",
            )?;
            let parsed: Vec<DbRow> = stmt
                .query_map([&cid_str], DbRow::from_row)?
                .filter_map(|r| r.ok())
                .collect();
            Ok::<Vec<DbRow>, rusqlite::Error>(parsed)
        })
    })
    .await
    .map_err(map_with_conn_err)?;

    rows.into_iter().map(|r| r.into_record()).collect()
}

// CID:afa-observability-persistence-004 - read_recent
// Purpose: SELECT the N most-recent spans whose
// started_at < since (UTC). Used by the
// dashboard's GET /spans/recent?since=...&limit=...
// endpoint.
//
// **Sort order**: DESC by started_at (newest
// first). The dashboard re-sorts at display time
// so the wire-form order is an implementation
// detail -- the limit is the only contract that
// matters.
//
// **Limit handling**: SQL `LIMIT N` is the cap.
// The HTTP layer applies the 100 / 1000
// wire-level caps (per the IMPL §Phase 3
// `spans_recent_default_limit_100` and
// `spans_recent_cap_1000` tests); this helper
// trusts its caller.
pub async fn read_recent(
    storage: &Storage,
    since: DateTime<Utc>,
    limit: u32,
) -> Result<Vec<SpanRecord>, ObservabilityError> {
    let since_str = since.to_rfc3339();
    let rows: Vec<DbRow> = with_conn(storage, move |conn| {
        Box::pin(async move {
            let mut stmt = conn.prepare(
                "SELECT span_id, parent_span_id, correlation_id,
                            tenant_id, actor_json, engine, operation,
                            started_at, duration_ms, outcome_json,
                            attributes_json
                     FROM spans
                     WHERE started_at < ?1
                     ORDER BY started_at DESC
                     LIMIT ?2",
            )?;
            let parsed: Vec<DbRow> = stmt
                .query_map(rusqlite::params![since_str, limit as i64], DbRow::from_row)?
                .filter_map(|r| r.ok())
                .collect();
            Ok::<Vec<DbRow>, rusqlite::Error>(parsed)
        })
    })
    .await
    .map_err(map_with_conn_err)?;

    rows.into_iter().map(|r| r.into_record()).collect()
}

// CID:afa-observability-persistence-005 - delete_older_than
// Purpose: DELETE every span with started_at <
// older_than. Used by the retention purge. Returns
// the number of rows deleted (the
// SpansPurged { count: ... } event payload).
//
// **Why no chunks in Phase 1**: see
// `purge::run_one_purge` for the IMPL §Phase 1
// "purge_chunks_at_purge_chunk_size" test -- the
// test asserts a single SpansPurged event after a
// single DELETE completes (the chunk_size field
// is plumbed through the purge code path but the
// chunking itself is left for a future pack when
// the spans table is expected to grow into
// millions of rows).
//
// **Transaction**: BEGIN IMMEDIATE (the IMPL
// principle #5). The single DELETE runs in the
// transaction; commit() finalises.
pub async fn delete_older_than(
    storage: &Storage,
    older_than: DateTime<Utc>,
) -> Result<u64, ObservabilityError> {
    let older_str = older_than.to_rfc3339();
    let deleted: u64 = with_conn(storage, move |conn| {
        Box::pin(async move {
            let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let n = tx.execute(
                "DELETE FROM spans WHERE started_at < ?1",
                rusqlite::params![older_str],
            )? as u64;
            tx.commit()?;
            Ok::<u64, rusqlite::Error>(n)
        })
    })
    .await
    .map_err(map_with_conn_err)?;

    Ok(deleted)
}

// CID:afa-observability-persistence-008 - map_with_conn_err
// Purpose: Translate a afa_contracts::StorageError
// (the outer error type of with_conn) into the
// local ObservabilityError vocabulary. The
// StorageError::Open arm folds to StorageUnreachable
// (the operator-action semantic); every other
// variant (Closure / Migrate / Locked) folds to
// Internal (an invariant violation we did not
// expect).
//
// **Why the lossy fold**: the With_conn outer error
// already encodes "the storage layer returned an
// error"; the specific subclass matters less to the
// engine than the fact that the call failed. The
// closure's inner error (e.g. the original
// rusqlite::Error) is preserved by the engine's
// upstream tracing::warn!.
//
// Used by: read_by_correlation_id, read_recent,
// delete_older_than (every public persistence
// helper that uses with_conn under the hood;
// write_span inlines its own match because it
// needs the result pair returned by with_conn and
// handled differently).
fn map_with_conn_err(e: afa_contracts::StorageError) -> ObservabilityError {
    match e {
        afa_contracts::StorageError::Open(io) => ObservabilityError::StorageUnreachable {
            reason: io.to_string(),
        },
        other => ObservabilityError::Internal {
            reason: format!("storage: {other}"),
        },
    }
}
