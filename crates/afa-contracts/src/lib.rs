//! Code Map: afa-contracts (the front door)
//! - `error`: The "what kind of trouble?" buckets and the
//!   "I'm an error" badge. See `error.rs`.
//! - `events`: The "I'm an event" badge and the sample event used
//!   by the conformance tests. See `events.rs`.
//! - `execution_context`: The envelope that travels with every
//!   request, plus the "who started this?" label. See
//!   `execution_context.rs`.
//! - `ids`: The tracking number and the tenant name tag. See
//!   `ids.rs`.
//! - `security`: The v1 security engine contract: the trait
//!   `SecurityV1`, the receipt `SecretRef`, the zeroing-on-drop
//!   handle `UnsealedSecret`, the eleven error buckets, and
//!   three audit events. See `security.rs`.
//! - `versioning_example`: A worked example of the "add a new
//!   socket, never change V1" versioning rule. See
//!   `versioning_example.rs`.
//!
//! Story (plain English): This file is the front door of the
//! shared types library. The other files in this folder hold the
//! actual types; this one just re-exports the most-used ones at
//! the top level so callers can write `use afa_contracts::Foo`
//! instead of `use afa_contracts::some_module::Foo`. The
//! `afa-contracts` crate is intentionally tiny: no I/O, no
//! async runtimes, no databases. It is the dictionary the rest
//! of the kernel agrees on, and a dictionary does not run a
//! post office.
//!
//! CID Index:
//! CID:afa-contracts-lib-001 -> error
//! CID:afa-contracts-lib-002 -> events
//! CID:afa-contracts-lib-003 -> execution_context
//! CID:afa-contracts-lib-004 -> ids
//! CID:afa-contracts-lib-005 -> security
//! CID:afa-contracts-lib-006 -> versioning_example
//!
//! Quick lookup: rg -n "CID:afa-contracts-lib-" crates/afa-contracts/src/lib.rs

#![doc(html_root_url = "https://docs.rs/afa-contracts/0.1.0")]

// CID:afa-contracts-lib-001 - error
// Purpose: Re-export the error-convention module so callers can
// reach it as `afa_contracts::error` (and the most-used items as
// `afa_contracts::AfaError`, `afa_contracts::AfaErrorKind`,
// `afa_contracts::ExampleStoreErrorV1`).
// Used by: every other AFA crate.
pub mod error;
// CID:afa-contracts-lib-002 - events
// Purpose: Re-export the event-convention module.
// Used by: every publisher and subscriber of events.
pub mod events;
// CID:afa-contracts-lib-003 - execution_context
// Purpose: Re-export the per-request context and actor label.
// Used by: every engine and plugin call signature.
pub mod execution_context;
// CID:afa-contracts-lib-004 - ids
// Purpose: Re-export the tracking number and tenant name tag.
// Used by: every request, every event, every log line.
pub mod ids;
// CID:afa-contracts-lib-005 - security
// Purpose: Re-export the v1 security engine contract: the trait,
// the receipt, the zeroing handle, the error type, and the
// three audit events. See `security.rs` for the Code Map.
// Used by: the `afa-security` engine (which implements the
// trait) and every adapter that needs a secret.
pub mod security;
// CID:afa-contracts-lib-006 - versioning_example
// Purpose: Re-export the worked example of the V1/V2 versioning
// rule and the dyn-compatibility pattern.
// Used by: the conformance test in `afa-contract-testing` and
// every real plugin that follows the pattern.
pub mod versioning_example;

pub use error::{AfaError, AfaErrorKind, ExampleStoreErrorV1};
pub use events::{AfaEvent, ExampleLessonCreated};
pub use execution_context::{Actor, ExecutionContext};
pub use ids::{CorrelationId, TenantId};
pub use security::{
    SecretRef, SecretRotated, SecretSealed, SecretUnsealed, SecurityErrorV1, SecurityV1,
    UnsealedSecret,
};
pub use versioning_example::{ExampleThingImpl, ExampleThingV1, ExampleThingV2};
