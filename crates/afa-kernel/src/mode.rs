//! Code Map: Kernel lifecycle state machine
//!
//! - `Mode`: The four-state lifecycle. `Booting` is
//!   transient (only present during the very first
//!   instructions of `Kernel::new`); `PreBootstrap` is
//!   the day-0 state where the operator has not yet
//!   sealed `dashboard-token`; `Sealing` is held during
//!   a single `POST /pre-bootstrap/seal` request; `Full`
//!   is the day-2 operating state.
//! - `Discriminant`: The `u8` value stored in the
//!   `AtomicU8`. The four values are stable so a future
//!   repr change cannot break persisted logs or external
//!   observers (the wire shape `Mode` enum is the
//!   public contract; the discriminant is internal).
//! - `ModePayload`: The `since` / `request_id` /
//!   `sealed_at` payload that each variant carries.
//!   Stored behind a `Mutex<ModePayload>` so the
//!   transition helpers can write the new payload
//!   after the discriminant CAS succeeds.
//! - `ModeController`: The runtime handle. Holds the
//!   `Arc<AtomicU8>` and the `Arc<Mutex<ModePayload>>`
//!   so cloning it (via `Arc::clone`) is cheap and all
//!   clones see the same transitions.
//!
//! Story (plain English): imagine a bank vault. The
//! vault is closed (Booting). The bank opens, but the
//! manager is the only one allowed in until they set
//! the alarm code (PreBootstrap). While the manager
//! types the code, nobody else can come in (Sealing).
//! Once the code is set, the bank is open for business
//! (Full). The four states are one-way arrows forward
//! on the happy path and one-way back to PreBootstrap
//! on a failed seal.
//!
//! CID Index:
//! CID:kernel-mode-010 -> Mode
//! CID:kernel-mode-011 -> Discriminant
//! CID:kernel-mode-012 -> ModePayload
//! CID:kernel-mode-013 -> ModeController
//! CID:kernel-mode-014 -> try_transition_to_sealing
//! CID:kernel-mode-015 -> transition_to_full
//! CID:kernel-mode-016 -> transition_to_prebootstrap
//!
//! Quick lookup: rg -n "CID:kernel-mode-0" crates/afa-kernel/src/mode.rs

use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use std::time::{Instant, SystemTime};
use uuid::Uuid;

// CID:kernel-mode-011 - Discriminant
// Purpose: The four stable `u8` values the
// `ModeController`'s `AtomicU8` uses. The values
// are stable so a future repr change cannot break
// persisted logs or external observers (the wire
// shape `Mode` enum is the public contract; the
// discriminant is internal).
// Used by: the `ModeController` CAS and
// `current()` accessor.
pub(crate) const PRE_BOOTSTRAP: u8 = 1;
pub(crate) const SEALING: u8 = 2;
pub(crate) const FULL: u8 = 3;

// CID:kernel-mode-010 - Mode
// Purpose: Re-export `KernelMode` as the public
// type for kernel state. The day-0 check +
// transition logic live on the controller; this
// type is the read-only view the dashboard's
// `/health` and bearer middleware use to decide
// what to do.
pub use afa_contracts::KernelMode as Mode;

// CID:kernel-mode-012 - ModePayload
// Purpose: The `since` / `request_id` / `sealed_at`
// payload each variant carries. Stored behind a
// `Mutex<ModePayload>` so the transition helpers
// can write the new payload after the discriminant
// CAS succeeds.
// Used by: `ModeController` (write) and
// `ModeController::current()` (read).
pub enum ModePayload {
    PreBootstrap {
        since: Instant,
    },
    Sealing {
        since: Instant,
        request_id: Uuid,
    },
    Full {
        since: Instant,
        sealed_at: SystemTime,
    },
}

// CID:kernel-mode-013 - ModeController
// Purpose: The runtime handle. Owns the
// `Arc<AtomicU8>` (the discriminant) and the
// `Arc<Mutex<ModePayload>>` (the variant-specific
// fields). `Arc::clone` is cheap (two atomic
// refcount bumps), and every clone observes the
// same transitions (the atomic is the source of
// truth; the mutex is read after the atomic).
// Used by: `Kernel` (held as `Arc<ModeController>`).
#[derive(Clone)]
pub struct ModeController {
    discriminant: Arc<AtomicU8>,
    payload: Arc<Mutex<ModePayload>>,
}

impl ModeController {
    /// Build a fresh `ModeController` in `PreBootstrap`
    /// (the day-0 state for a fresh kernel where
    /// `dashboard-token` has never been sealed).
    /// Called from `Kernel::new` when the engine
    /// returns `Err(SecretNotFound)` on the day-0
    /// `lookup_hash` check.
    pub(crate) fn new_pre_bootstrap() -> Self {
        Self {
            discriminant: Arc::new(AtomicU8::new(PRE_BOOTSTRAP)),
            payload: Arc::new(Mutex::new(ModePayload::PreBootstrap {
                since: Instant::now(),
            })),
        }
    }

    /// Build a fresh `ModeController` in `Full` (the
    /// day-2 state for a kernel that boots with
    /// `dashboard-token` already sealed). Called from
    /// `Kernel::new` when the engine returns
    /// `Ok(_)` on the day-0 `lookup_hash` check
    /// (the secret exists, regardless of whether the
    /// placeholder hash matched).
    pub(crate) fn new_full() -> Self {
        let now_instant = Instant::now();
        let now_system = SystemTime::now();
        Self {
            discriminant: Arc::new(AtomicU8::new(FULL)),
            payload: Arc::new(Mutex::new(ModePayload::Full {
                since: now_instant,
                sealed_at: now_system,
            })),
        }
    }

    /// Read the current mode. Re-reads the
    /// discriminant and dispatches to the right
    /// payload variant. The discriminant is the
    /// source of truth; the payload is read after,
    /// so a stale payload (one of the very few
    /// transient states the CAS can leave behind)
    /// cannot be observed as the wrong variant —
    /// at worst the payload is one transition
    /// behind, and the `match _` arms substitute
    /// fresh values.
    pub(crate) fn current(&self) -> Mode {
        let d = self.discriminant.load(Ordering::Acquire);
        let guard = self.payload.lock().expect("mode payload lock");
        match d {
            PRE_BOOTSTRAP => match &*guard {
                ModePayload::PreBootstrap { since } => Mode::PreBootstrap { since: *since },
                _ => Mode::PreBootstrap {
                    since: Instant::now(),
                },
            },
            SEALING => match &*guard {
                ModePayload::Sealing { since, request_id } => Mode::Sealing {
                    since: *since,
                    request_id: *request_id,
                },
                _ => Mode::Sealing {
                    since: Instant::now(),
                    request_id: Uuid::nil(),
                },
            },
            FULL => match &*guard {
                ModePayload::Full { since, sealed_at } => Mode::Full {
                    since: *since,
                    sealed_at: *sealed_at,
                },
                _ => Mode::Full {
                    since: Instant::now(),
                    sealed_at: SystemTime::now(),
                },
            },
            // Catch-all for any unrecognised
            // discriminant (a future variant
            // added without updating this
            // match). The default is
            // `PreBootstrap` because the next
            // request from the operator is
            // the most likely "safe" default
            // (the kernel will accept a fresh
            // seal request rather than refusing
            // all traffic).
            _ => Mode::PreBootstrap {
                since: Instant::now(),
            },
        }
    }

    // CID:kernel-mode-014 - try_transition_to_sealing
    // Purpose: Lock-free CAS that moves the kernel
    // from `PreBootstrap` to `Sealing`. The
    // `compare_exchange` ensures only one concurrent
    // request gets `Ok(())`; the second receives
    // `Err(TryTransitionError::AlreadySealing)`. The
    // payload is updated inside the same critical
    // section that the CAS wins (the discriminant
    // and the payload transition together).
    // Used by: `dashboard::pre_bootstrap::handler`.
    pub(crate) fn try_transition_to_sealing(
        &self,
        request_id: Uuid,
    ) -> Result<(), TryTransitionError> {
        let now = Instant::now();
        self.discriminant
            .compare_exchange(PRE_BOOTSTRAP, SEALING, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|current| match current {
                SEALING => TryTransitionError::AlreadySealing,
                FULL => TryTransitionError::AlreadySealed,
                PRE_BOOTSTRAP => TryTransitionError::AlreadySealing,
                _ => TryTransitionError::NotInMode,
            })?;
        let mut guard = self.payload.lock().expect("mode payload lock");
        *guard = ModePayload::Sealing {
            since: now,
            request_id,
        };
        Ok(())
    }

    // CID:kernel-mode-015 - transition_to_full
    // Purpose: Move the kernel from `Sealing` to
    // `Full` on a successful seal. The discriminant
    // transition and the payload update happen as
    // a pair (the discriminant transition is the
    // source of truth; the payload update is the
    // readable value).
    // Used by: `dashboard::pre_bootstrap::handler`
    // (success arm only).
    pub(crate) fn transition_to_full(&self, sealed_at: SystemTime) {
        self.discriminant.store(FULL, Ordering::Release);
        let mut guard = self.payload.lock().expect("mode payload lock");
        *guard = ModePayload::Full {
            since: Instant::now(),
            sealed_at,
        };
    }

    // CID:kernel-mode-016 - transition_to_prebootstrap
    // Purpose: Roll the kernel back from `Sealing`
    // to `PreBootstrap` on a seal error. Without
    // this rollback the kernel would be stuck in
    // `Sealing` forever (no other request could
    // get the CAS to win), and the operator
    // would be locked out.
    // Used by: `dashboard::pre_bootstrap::handler`
    // (error arm only).
    pub(crate) fn transition_to_prebootstrap(&self) {
        self.discriminant.store(PRE_BOOTSTRAP, Ordering::Release);
        let mut guard = self.payload.lock().expect("mode payload lock");
        *guard = ModePayload::PreBootstrap {
            since: Instant::now(),
        };
    }
}

// CID:kernel-mode-014 - TryTransitionError
// Purpose: The error enum for
// `ModeController::try_transition_to_sealing`. The
// three variants cover the three failure modes of
// the CAS: not-in-PreBootstrap (the kernel already
// transitioned to `Full` or `Sealing`), already
// sealing (a second concurrent request lost the
// race), and a catch-all for any other state.
// Used by: `dashboard::pre_bootstrap::handler`'s
// error mapping (`AlreadySealing` → 409, the
// others → 409 with a different reason).
pub enum TryTransitionError {
    NotInMode,
    AlreadySealing,
    AlreadySealed,
}
