//! Code Map: afa-kernel (the front door)
//! - `capability_registry`: The small lookup table the
//!   `Kernel` holds for plugin capabilities (currently
//!   one slot: LLM). See `capability_registry.rs`.
//! - `event_bus`: Re-export of the in-process pub/sub broker
//!   from `afa-bus`. A publisher hands it an event, and every
//!   subscriber that cares about that event type gets a copy.
//!   The bus was extracted from this crate in Phase 3 of the
//!   security pack (see `crates/afa-bus/src/lib.rs`'s
//!   opening comment for the full rationale). The re-export
//!   here keeps the public path
//!   `afa_kernel::event_bus::EventBus` working for
//!   downstream code that was written before the split.
//! - `kernel`: The top-level composition that owns the Runtime,
//!   the Scheduler, the Event Bus, the Security Engine, and
//!   the CapabilityRegistry (all wrapped in `Arc`s so cloning
//!   is cheap). See `kernel.rs`.
//! - `runtime`: The single entry point (`ingest`) that turns a
//!   raw event into a fully-dispatched unit of work. See
//!   `runtime.rs`.
//! - `scheduler`: The dispatcher that finds every registered
//!   step for an event type, runs them concurrently, and
//!   isolates panics. See `scheduler.rs`.
//! - `Kernel`: Re-exported at the crate root so downstream
//!   consumers (future channel plugins, future `axum` HTTP
//!   handlers, future integration tests in other packs) can
//!   `use afa_kernel::Kernel` without reaching into a submodule.
//! - `Step`: Re-exported at the crate root as the public shape of
//!   a workflow step.
//! - `LlmV1`: Re-export of the LLM-adapter trait so
//!   `register_llm` callers can name the trait without
//!   importing `afa-contracts` directly.
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
//! CID:afa-kernel-lib-001 -> capability_registry
//! CID:afa-kernel-lib-002 -> event_bus (re-export of afa-bus)
//! CID:afa-kernel-lib-003 -> kernel
//! CID:afa-kernel-lib-004 -> runtime
//! CID:afa-kernel-lib-005 -> scheduler
//! CID:afa-kernel-lib-006 -> crate-root re-exports
//!
//! Quick lookup: rg -n "CID:afa-kernel-lib-" crates/afa-kernel/src/lib.rs

#![doc(html_root_url = "https://docs.rs/afa-kernel/0.1.0")]

// CID:afa-kernel-lib-001 - capability_registry
// Purpose: The small lookup table the `Kernel`
// holds for plugin capabilities (currently one
// slot: LLM). The `register_llm` method inserts
// the adapter; `Kernel::capabilities` hands a
// read-only view to workflows.
// Used by: `Kernel` (composes the registry),
// workflows that call `llm.complete` /
// `llm.stream_complete`.
pub mod capability_registry;
// CID:afa-kernel-lib-002 - event_bus (re-export of afa-bus)
// Purpose: Re-export the in-process pub/sub broker module
// from `afa-bus` so downstream code that was written
// before the Phase 3 split (which extracted the bus
// into its own crate to break the
// afa-kernel → afa-security → afa-kernel cycle) can
// keep using `afa_kernel::event_bus::EventBus` as
// before. New code should prefer
// `use afa_bus::EventBus;` directly. The contents
// of this module are the exact same types as
// `afa_bus`; the re-export is purely a path alias.
// Used by: `Kernel` (composes an `Arc<EventBus>`),
// `Runtime` (publishes `EventReceived`), `Scheduler`
// (hands steps an `EventBusHandle`).
pub use afa_bus as event_bus;
pub mod dashboard;
// CID:afa-kernel-lib-003 - kernel
// Purpose: Re-export the top-level composition (Runtime +
// Scheduler + Event Bus + Security Engine +
// CapabilityRegistry, all `Arc`-backed, cheaply
// `Clone`-able).
// Used by: every consumer of the kernel; this is the type most
// callers will hold and pass around.
pub mod kernel;
// CID:afa-kernel-lib-004 - runtime
// Purpose: Re-export the single ingress point (`Runtime::ingest`)
// that turns a raw event into a dispatched unit of work and
// returns the new correlation ID.
// Used by: every external caller (channel plugins, tests).
pub mod runtime;
// CID:afa-kernel-lib-005 - scheduler
// Purpose: Re-export the dispatcher that finds every registered
// step for an event type and runs them concurrently with panic
// isolation.
// Used by: `Runtime` (calls `dispatch` on every ingest),
// workflow-engine authors (register steps at startup).
pub mod scheduler;

// CID:afa-kernel-lib-006 - crate-root re-exports
// Purpose: Re-export the three types downstream
// consumers reach for most often (a `Kernel` to
// construct and pass around, a `Step` to register
// on a scheduler, and the two trait-object views
// for the `CapabilityRegistry` slots) at the crate
// root, so callers can `use afa_kernel::Kernel`
// without reaching into a submodule. This is the
// public-API boundary: anything not re-exported
// here is not part of the contract. `LlmV1` and
// `KnowledgeV1` are the trait objects the
// `register_llm` / `register_knowledge` callers
// name when they hand an adapter to the kernel —
// re-exporting them at the crate root lets
// downstream code stay on the
// `afa_kernel::KnowledgeV1` path rather than
// reaching into `afa_contracts` directly.
pub use afa_contracts::{EmbeddingV1, KnowledgeV1, LlmV1};
pub use capability_registry::{CapabilityRegistry, RegisterError};
pub use kernel::Kernel;
pub use scheduler::Step;
