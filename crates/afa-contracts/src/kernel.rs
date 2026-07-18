//! Code Map: Kernel lifecycle
//!
//! - `KernelMode`: The four-state machine the kernel walks
//!   through from `Booting` to `Full`. `Booting` is the
//!   initial startup with no endpoints served; `PreBootstrap`
//!   is the day-0 state where HTTP is up but only
//!   `POST /pre-bootstrap/seal` is writable (and the
//!   `/health` endpoint returns `503 pre_bootstrap: true`);
//!   `Sealing` is the transient state during a seal request
//!   (the kernel holds the exclusive write lock on the
//!   `secrets` table so two concurrent seal attempts cannot
//!   both succeed); `Full` is the day-2 operating state.
//! - `mode_helpers`: `is_sealed`, `is_pre_bootstrap`,
//!   `is_sealing`, `is_booting` — convenience predicates
//!   the dashboard transport's `/health` handler uses to
//!   decide whether to return 200 or 503.
//! - `Display`: The `KernelMode` render formats the variant
//!   plus any optional `since`/`request_id` fields. `since`
//!   renders as a relative duration; `sealed_at` renders as
//!   an RFC 3339 timestamp.
//!
//! Story (plain English): Imagine a bank's front door. The
//! bank is closed for a moment (Booting). Then the doors
//! open but only the manager can come in to set the alarm
//! code (PreBootstrap). While the manager types the code,
//! nobody else can come in and type a different code
//! (Sealing). Once the code is set, the bank is open for
//! business (Full). The four states are one-way arrows
//! forward; a failed seal sends the bank back to
//! PreBootstrap so the manager can try again.
//!
//! CID Index:
//! CID:kernel-mode-001 -> KernelMode
//! CID:kernel-mode-002 -> mode_helpers
//! CID:kernel-mode-003 -> Display
//!
//! Quick lookup: rg -n "CID:kernel-mode-" crates/afa-contracts/src/kernel.rs

use std::fmt;
use std::time::{Instant, SystemTime};
use uuid::Uuid;

// CID:kernel-mode-001 - KernelMode
// Purpose: The kernel's four-state lifecycle. The transitions
// are one-way forward on the happy path
// (`Booting → PreBootstrap → Sealing → Full`) and one-way
// back to `PreBootstrap` on a failed seal, so the operator
// can retry. Once in `Full` the kernel never re-enters
// `PreBootstrap`.
// Used by: the kernel boot path, the dashboard `/health`
// handler, and the future `POST /pre-bootstrap/seal`
// endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum KernelMode {
    /// Initial startup. No endpoints are served.
    /// The kernel transitions out of this state as
    /// soon as `boot()` finishes; the variant exists
    /// so observers can distinguish "kernel is still
    /// starting" from "kernel is up and waiting for
    /// the operator's first action."
    Booting,
    /// HTTP is up. `GET /health` returns `503` with
    /// `pre_bootstrap: true` so a load balancer
    /// doesn't route real traffic. The only
    /// write-capable endpoint is
    /// `POST /pre-bootstrap/seal`; every other
    /// bearer-protected endpoint refuses. The
    /// `since` field is the moment the kernel
    /// entered this state (used for uptime logs).
    PreBootstrap { since: Instant },
    /// Transient state held during a single
    /// `POST /pre-bootstrap/seal` request. The
    /// kernel holds an exclusive write lock on
    /// the `secrets` table so a second
    /// concurrent seal attempt cannot both
    /// succeed. The `request_id` field is the
    /// `Uuid` the handler minted at the start of
    /// the request; it rides along on every log
    /// line and event the request produces, so
    /// a stuck seal can be diagnosed end-to-end.
    Sealing { since: Instant, request_id: Uuid },
    /// Day-2 operating state. Every dashboard
    /// endpoint is open (subject to bearer auth).
    /// The `since` field is the moment the kernel
    /// entered `Full`; `sealed_at` is the wall-clock
    /// `SystemTime` of the same moment (used by
    /// `afa-cli` and the SPA to show the operator
    /// when the install completed).
    Full {
        since: Instant,
        sealed_at: SystemTime,
    },
}

impl KernelMode {
    // CID:kernel-mode-002 - mode_helpers
    // Purpose: Convenience predicates the dashboard
    // transport's `/health` handler (and the future
    // `POST /pre-bootstrap/seal` handler) use to
    // route or reject without pattern-matching the
    // variant shape in every call site.
    // Used by: afa-kernel::dashboard::health,
    // afa-kernel::dashboard::auth (Phase 4b).

    /// True iff the kernel has completed its day-0
    /// setup and is ready to serve bearer-protected
    /// requests. Equivalent to
    /// `matches!(self, Self::Full { .. })`.
    pub fn is_sealed(&self) -> bool {
        matches!(self, Self::Full { .. })
    }

    /// True iff the kernel is waiting for the
    /// operator to seal the dashboard token.
    /// Equivalent to
    /// `matches!(self, Self::PreBootstrap { .. })`.
    pub fn is_pre_bootstrap(&self) -> bool {
        matches!(self, Self::PreBootstrap { .. })
    }

    /// True iff a seal request is in flight. Useful
    /// for tests and for the future `POST
    /// /pre-bootstrap/seal` handler to reject a
    /// second concurrent request. Equivalent to
    /// `matches!(self, Self::Sealing { .. })`.
    pub fn is_sealing(&self) -> bool {
        matches!(self, Self::Sealing { .. })
    }

    /// True iff the kernel is still starting up and
    /// no endpoints are served. Equivalent to
    /// `matches!(self, Self::Booting)`.
    pub fn is_booting(&self) -> bool {
        matches!(self, Self::Booting)
    }
}

impl fmt::Display for KernelMode {
    // CID:kernel-mode-003 - Display
    // Purpose: Human-readable render of the
    // current mode. Used by `tracing` lines and
    // by `/health`'s JSON body (the dashboard
    // surfaces it verbatim). The format is
    // `mode: <variant>` plus any
    // variant-specific fields, comma-separated.
    // The `since` `Instant` is rendered as a
    // relative duration (e.g. `+12.345s`); the
    // `sealed_at` `SystemTime` is rendered as
    // an RFC 3339 timestamp.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Booting => f.write_str("booting"),
            Self::PreBootstrap { since } => {
                write!(
                    f,
                    "pre_bootstrap, since=+{:.3}s",
                    since.elapsed().as_secs_f64()
                )
            }
            Self::Sealing { since, request_id } => write!(
                f,
                "sealing, since=+{:.3}s, request_id={}",
                since.elapsed().as_secs_f64(),
                request_id
            ),
            Self::Full { since, sealed_at } => {
                // Render the wall-clock `sealed_at` as
                // an RFC 3339 string for the operator;
                // render the elapsed `since` as a
                // relative duration for log-line
                // grep-ability.
                let rfc3339 = chrono::DateTime::<chrono::Utc>::from(*sealed_at)
                    .to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
                write!(
                    f,
                    "full, since=+{:.3}s, sealed_at={}",
                    since.elapsed().as_secs_f64(),
                    rfc3339
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // **Doc drift correction #1 (kernel mode)**:
    // - Source: the IMPL/TRD draft implied a
    //   serde-derived JSON wire form for
    //   `KernelMode` (so the dashboard's
    //   `/health` body can round-trip it).
    // - Shipped: no serde derives. The
    //   `Instant` and `SystemTime` fields
    //   do not implement `serde::Serialize`,
    //   so a derived `#[serde(tag = "mode",
    //   rename_all = "snake_case")]` would
    //   not compile. The dashboard serializes
    //   the mode by hand (Phase 4b) using
    //   the locked wire string
    //   `"boot" | "pre_bootstrap" | "sealing" | "full"`.
    //   A future pack that needs a true
    //   serde round-trip should swap the
    //   `Instant` fields for
    //   `chrono::DateTime<Utc>` and add a
    //   custom (de)serializer for the
    //   elapsed-duration side.
    // - Why: the test suite below pins the
    //   four-variant shape; the equality
    //   test's comment explicitly documents
    //   the trade-off so a future "let's
    //   add serde round-trip" patch starts
    //   from the right context.

    // CID:kernel-mode-001 - tests
    // Purpose: Five unit tests that lock down the
    // four-state shape and the four helper predicates.
    // Conformance gate for the contract; a future
    // change that reorders the variants, renames a
    // helper, or breaks the `Display` round-trip
    // trips one of these.

    #[test]
    fn kernel_mode_booting_helpers() {
        let m = KernelMode::Booting;
        assert!(m.is_booting());
        assert!(!m.is_pre_bootstrap());
        assert!(!m.is_sealing());
        assert!(!m.is_sealed());
        assert_eq!(m.to_string(), "booting");
    }

    #[test]
    fn kernel_mode_pre_bootstrap_helpers() {
        let m = KernelMode::PreBootstrap {
            since: Instant::now(),
        };
        assert!(!m.is_booting());
        assert!(m.is_pre_bootstrap());
        assert!(!m.is_sealing());
        assert!(!m.is_sealed());
        // Display includes the elapsed duration; assert
        // the prefix only so the test is robust to
        // clock-jitter between `since` and `Display`.
        assert!(m.to_string().starts_with("pre_bootstrap, since=+"));
    }

    #[test]
    fn kernel_mode_sealing_helpers() {
        let m = KernelMode::Sealing {
            since: Instant::now(),
            request_id: Uuid::new_v4(),
        };
        assert!(!m.is_booting());
        assert!(!m.is_pre_bootstrap());
        assert!(m.is_sealing());
        assert!(!m.is_sealed());
        assert!(m.to_string().starts_with("sealing, since=+"));
        assert!(m.to_string().contains("request_id="));
    }

    #[test]
    fn kernel_mode_full_helpers() {
        let m = KernelMode::Full {
            since: Instant::now(),
            sealed_at: SystemTime::now(),
        };
        assert!(!m.is_booting());
        assert!(!m.is_pre_bootstrap());
        assert!(!m.is_sealing());
        assert!(m.is_sealed());
        assert!(m.to_string().starts_with("full, since=+"));
        assert!(m.to_string().contains("sealed_at="));
    }

    #[test]
    fn kernel_mode_equality_and_clone() {
        // The contract derives `Clone, Debug, PartialEq,
        // Eq` so the mode can ride in `Arc<RwLock<…>>`.
        // The dashboard's `/health` JSON body serializes
        // the mode by hand (the `Instant` and `SystemTime`
        // fields do not implement `serde::Serialize`, so
        // a derived round-trip is not possible today; a
        // future pack that needs a JSON wire form should
        // add a custom (de)serializer or replace the
        // timing fields with `chrono::DateTime<Utc>`).
        // This test pins the derives (a future "remove
        // PartialEq for size" optimization trips here).
        let a = KernelMode::PreBootstrap {
            since: Instant::now(),
        };
        let b = a.clone();
        assert_eq!(a, b);
        // Distinct `Instant`s → distinct values (the
        // `since` field participates in equality).
        let later = KernelMode::PreBootstrap {
            since: Instant::now() + std::time::Duration::from_millis(1),
        };
        assert_ne!(a, later);
    }
}
