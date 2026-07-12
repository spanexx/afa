//! Code Map: afa-kernel (the front door)
//! - `event_bus`: The in-process pub/sub broker. A publisher hands
//!   it an event, and every subscriber that cares about that event
//!   type gets a copy. See `event_bus.rs`.
//! - `kernel`: The top-level composition that owns the Runtime, the
//!   Scheduler, and the Event Bus, all wrapped in `Arc`s so cloning
//!   it is cheap. See `kernel.rs`.
//! - `runtime`: The single entry point (`ingest`) that turns a raw
//!   event into a fully-dispatched unit of work. See `runtime.rs`.
//! - `scheduler`: The dispatcher that finds every registered step
//!   for an event type, runs them concurrently, and isolates
//!   panics. See `scheduler.rs`.
//! - `Kernel`: Re-exported at the crate root so downstream
//!   consumers (future channel plugins, future `axum` HTTP
//!   handlers, future integration tests in other packs) can
//!   `use afa_kernel::Kernel` without reaching into a submodule.
//! - `Step`: Re-exported at the crate root as the public shape of
//!   a workflow step.
//!
//! Story (plain English): This file is the front door of the
//! kernel crate. The kernel is the heartbeat of AFA: it takes
//! events in, finds the workflow steps that care about each event,
//! runs those steps all at the same time, lets the steps publish
//! their own events, and traces what happened along the way. The
//! four files in this folder each handle one of those jobs. The
//! kernel is intentionally small and does not know about any
//! specific engine, vendor, or domain — it is a generic
//! dispatcher. A dictionary would not run a post office; this
//! kernel is not a workflow engine, not a message broker, not a
//! database. It is the post office: it routes, it does not decide.
//!
//! CID Index:
//! CID:afa-kernel-lib-001 -> event_bus
//! CID:afa-kernel-lib-002 -> kernel
//! CID:afa-kernel-lib-003 -> runtime
//! CID:afa-kernel-lib-004 -> scheduler
//! CID:afa-kernel-lib-005 -> crate-root re-exports
//!
//! Quick lookup: rg -n "CID:afa-kernel-lib-" crates/afa-kernel/src/lib.rs

#![doc(html_root_url = "https://docs.rs/afa-kernel/0.1.0")]

// CID:afa-kernel-lib-001 - event_bus
// Purpose: Re-export the in-process pub/sub broker module.
// Used by: `Kernel` (composes an `Arc<EventBus>`),
// `Runtime` (publishes `EventReceived`), `Scheduler` (hands
// steps an `EventBusHandle`).
pub mod event_bus;
// CID:afa-kernel-lib-002 - kernel
// Purpose: Re-export the top-level composition (Runtime +
// Scheduler + Event Bus, all `Arc`-backed, cheaply `Clone`-able).
// Used by: every consumer of the kernel; this is the type most
// callers will hold and pass around.
pub mod kernel;
// CID:afa-kernel-lib-003 - runtime
// Purpose: Re-export the single ingress point (`Runtime::ingest`)
// that turns a raw event into a dispatched unit of work and
// returns the new correlation ID.
// Used by: every external caller (channel plugins, tests).
pub mod runtime;
// CID:afa-kernel-lib-004 - scheduler
// Purpose: Re-export the dispatcher that finds every registered
// step for an event type and runs them concurrently with panic
// isolation.
// Used by: `Runtime` (calls `dispatch` on every ingest),
// workflow-engine authors (register steps at startup).
pub mod scheduler;

// CID:afa-kernel-lib-005 - crate-root re-exports
// Purpose: Re-export the two types downstream consumers
// reach for most often (a `Kernel` to construct and pass
// around, and a `Step` to register on a scheduler) at
// the crate root, so callers can `use afa_kernel::Kernel`
// without reaching into a submodule. This is the
// public-API boundary: anything not re-exported here is
// not part of the contract.
pub use kernel::Kernel;
pub use scheduler::Step;
