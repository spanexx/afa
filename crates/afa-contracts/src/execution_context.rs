//! Code Map: Per-request context
//! - `Actor`: A small label that says *who* kicked off the current
//!   request. Was it a web visitor? A timer? An operator at a
//!   keyboard? An internal call from the kernel itself?
//! - `ExecutionContext`: A small envelope that travels with every
//!   request. It carries the tracking number, the tenant, the actor,
//!   an optional deadline, and a tracing span — so anything the
//!   kernel does on behalf of that request can be tied back to it.
//!
//! Story (plain English): Imagine a single customer service call.
//! When the call starts, the operator opens a folder (the
//! `ExecutionContext`). Inside the folder: the caller's tracking
//! number (`CorrelationId`), which line called in (`TenantId`),
//! how they reached us (`Actor`), how long they're willing to wait
//! (`deadline`), and a sticky note saying "this is the open call"
//! (`tracing::Span`). Every person who helps the caller — the
//! security guard, the database clerk, the notifier — gets a copy
//! of that folder so they can do their work *and* write back into
//! the same call's record.
//!
//! CID Index:
//! CID:execution-context-001 -> Actor
//! CID:execution-context-002 -> ExecutionContext
//!
//! Quick lookup: rg -n "CID:execution-context-" crates/afa-contracts/src/execution_context.rs

use crate::ids::{CorrelationId, TenantId};
use serde::{Deserialize, Serialize};
use std::time::Instant;
use tracing::Span;

// CID:execution-context-001 - Actor
// Purpose: A small label that says *who* started the current
// request. The kernel uses it for audit logs and for routing rules
// like "timed jobs cannot call the secrets API."
// Uses: serde (to ride along on JSON), String (for the names of
// channels and surfaces).
// Used by: ExecutionContext (every context has exactly one Actor),
// and the security engine (the actor controls which permissions
// apply to the request).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Actor {
    /// Inbound from a named channel (e.g. `"http"`, `"grpc"`,
    /// `"scheduler"`).
    Channel(String),
    /// A timer-driven job.
    Timer,
    /// A human operator, reached via the named surface.
    Human { via: String },
    /// An internal call from the kernel or another engine. The string
    /// identifies the caller.
    Internal(String),
}

// CID:execution-context-002 - ExecutionContext
// Purpose: The envelope that travels with every request, so any
// piece of code the kernel runs on behalf of that request knows
// the request's tracking number, the agency it belongs to, the
// actor that started it, when it must finish, and the tracing
// span it should add its log lines to. Clone is required because
// the event bus fans the context out to many subscribers without
// requiring the event itself to be cloneable.
// Uses: CorrelationId, TenantId, Actor, tracing::Span
// (current_thread-style span attached to the request).
// Used by: every engine, plugin, and adapter call in the kernel.
// It is the first argument of nearly every public function.
#[derive(Debug, Clone)]
pub struct ExecutionContext {
    pub correlation_id: CorrelationId,
    pub tenant_id: TenantId,
    pub actor: Actor,
    pub deadline: Option<Instant>,
    pub span: Span,
}

impl ExecutionContext {
    /// Build a fresh `ExecutionContext` for the given tenant + actor.
    ///
    /// A new `CorrelationId` is generated and a new root `tracing::Span`
    /// is opened. The deadline defaults to `None`.
    pub fn new(tenant_id: TenantId, actor: Actor) -> Self {
        let correlation_id = CorrelationId::new();
        let span = tracing::info_span!(
            "execution_context",
            correlation_id = %correlation_id,
            tenant_id = %tenant_id,
        );
        Self {
            correlation_id,
            tenant_id,
            actor,
            deadline: None,
            span,
        }
    }

    /// Attach a deadline to this context.
    #[must_use]
    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn execution_context_new_populates_required_fields() {
        let tenant = TenantId::new("acme-realty");
        let actor = Actor::Channel("http".into());
        let ctx = ExecutionContext::new(tenant.clone(), actor.clone());

        assert_eq!(ctx.tenant_id, tenant);
        assert_eq!(ctx.actor, actor);
        assert!(ctx.deadline.is_none(), "deadline must default to None");
    }

    #[test]
    fn execution_context_new_assigns_a_correlation_id() {
        let a = ExecutionContext::new(TenantId::new("a"), Actor::Timer);
        let b = ExecutionContext::new(TenantId::new("a"), Actor::Timer);
        assert_ne!(
            a.correlation_id, b.correlation_id,
            "each new context must get its own CorrelationId"
        );
    }

    #[test]
    fn with_deadline_attaches_deadline() {
        let ctx = ExecutionContext::new(TenantId::new("a"), Actor::Timer);
        let deadline = Instant::now();
        let ctx = ctx.with_deadline(deadline);
        assert_eq!(ctx.deadline, Some(deadline));
    }
}
