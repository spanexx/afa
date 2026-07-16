//! Code Map: observability::health
//!
//! - impl HealthCheck for ObservabilityEngine: the
//!   sync trait impl that the kernel's
//!   aggregate_health() walks every engine on to
//!   build the dashboard's GET /health response.
//!   Returns Healthy when the engine has had zero
//!   span-write failures in the last hour;
//!   otherwise Degraded { reason: "spans write
//!   failing: N drops in last hour" } where N is the
//!   current count from the AtomicU64. The counter
//!   is reset to 0 by the purge loop's hourly tick
//!   (see purge::run_purge_loop -- the "and zeroes
//!   the counter" effect).
//!
//! Story (plain English): The recording nurse's
//! self-check at the start of every shift. She
//! checks her notepad (the drops counter). If the
//! notepad is empty (zero dropped entries), she
//! reports "all good" (`Healthy`). If the notepad
//! has entries (any dropped entries), she reports
//! "I'm OK but I've had trouble; here's how many
//! chart notes I lost this hour" (`Degraded`). She
//! never reports `Unhealthy` because being
//! best-effort means she cannot block the doctor's
//! work -- the worst she can do is lose chart
//! notes, and the operator can recover by
//! inspecting the spans DB later.
//!
//! Doc drift corrections vs. the IMPL draft:
//! - #5: the IMPL put `#[async_trait::async_trait]`
//!   on the impl. Dropped -- the HealthCheck trait
//!   in afa-contracts is sync for the kernel's
//!   catch_unwind + 100ms-per-engine timeout
//!   pattern to work without a
//!   `Pin<Box<dyn Future + Send>>` wrapper.
//! - #10: the IMPL's example string was
//!   hard-coded to "spans write failing: 1 drops in
//!   last hour". The implementation interpolates
//!   the actual count -- the wire form is correct
//!   for any N, not just N = 1.
//!
//! CID Index:
//! CID:afa-observability-health-001 -> impl HealthCheck
//!
//! Quick lookup: rg -n "CID:afa-observability-health-" crates/afa-observability/src/health.rs

use crate::observability::ObservabilityEngine;
use afa_contracts::{HealthCheck, HealthStatus};
use std::sync::atomic::Ordering;

// CID:afa-observability-health-001 - impl HealthCheck for ObservabilityEngine
// Purpose: The "is the observability engine OK?"
// answer the kernel's aggregate_health() polls.
//
// **Sync on purpose**: the kernel's aggregator uses
// catch_unwind + a 100ms-per-engine timeout, and
// the HealthCheck trait in afa-contracts has a
// sync signature specifically to avoid a
// Pin<Box<dyn Future>> that would break the
// panic-isolation pattern. The atomic load is one
// cheap read; no I/O, no locks.
//
// **HealthStatus mapping**:
// - 0 drops -> Healthy
// - 1+ drops -> Degraded { reason: "spans write
//   failing: N drops in last hour" }
// - Unhealthy: never. The engine cannot enter a
//   state where ALL span writes fail indefinitely
//   (the recording nurse never gives up -- she
//   falls back to "the central logbook is full but
//   the doctor can still continue" semantics). An
//   operator who wants the "catastrophic" semantic
//   can layer it on top by checking the same
//   `drops_in_last_hour` field from their own code.
//
// **Counter reset**: the purge loop's hourly tick
// fires `drops_in_last_hour.store(0)` (see
// purge.rs). The window is therefore exactly 1
// hour long; a 30-minute window could be added by
// introducing a second AtomicU64 + alternating
// resets, but the 1-hour window matches the kernel's
// dashboard "last hour" cadence.

impl HealthCheck for ObservabilityEngine {
    fn health_check(&self) -> HealthStatus {
        let drops = self.drops_in_last_hour();
        let n = drops.load(Ordering::Relaxed);
        if n == 0 {
            HealthStatus::Healthy
        } else {
            HealthStatus::Degraded {
                reason: format!("spans write failing: {n} drops in last hour"),
            }
        }
    }
}
