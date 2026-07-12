//! Code Map: Test fixtures
//! - `test_execution_context`: A one-liner that builds a fresh
//!   `ExecutionContext` for a conformance test, tagged with
//!   `Actor::Internal("test-fixture")` so the origin is obvious
//!   in logs.
//!
//! Story (plain English): Imagine a movie set. Every scene needs
//! a fake phone, a fake address, a fake ID card — the same
//! props, every time, so the actor can focus on the scene. The
//! `test_execution_context` function is the prop department for
//! the conformance tests. It hands every test a brand-new,
//! boring, predictable `ExecutionContext` so the test can focus
//! on the assertion, not on the setup.
//!
//! CID Index:
//! CID:fixtures-001 -> test_execution_context
//!
//! Quick lookup: rg -n "CID:fixtures-" crates/afa-contract-testing/src/fixtures.rs

use afa_contracts::execution_context::ExecutionContext;
use afa_contracts::ids::TenantId;
use afa_contracts::Actor;

// CID:fixtures-001 - test_execution_context
// Purpose: The "prop department" — hand every conformance test a
// fresh, predictable `ExecutionContext` so the test can focus on
// the assertion, not the setup. The actor is always
// `Internal("test-fixture")`, so any log line produced by a
// conformance test is clearly traceable back to the harness.
// Uses: afa_contracts::ExecutionContext, TenantId, Actor.
// Used by: every conformance test that needs a context
// (downstream crates, and the harness's own self-tests).
pub fn test_execution_context(tenant: &str) -> ExecutionContext {
    ExecutionContext::new(
        TenantId::new(tenant),
        Actor::Internal("test-fixture".into()),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_carries_the_full_expected_context() {
        // The fixture's contract is more than just the tenant:
        // it must use `Actor::Internal("test-fixture")` (so log
        // lines produced by conformance tests are clearly
        // distinguishable from real-traffic log lines) and must
        // leave the deadline `None` (so the fixture does not
        // accidentally time out a test). The tenant round-trip
        // is the easy check; the actor and deadline are the
        // ones most likely to drift if someone "tidies up" the
        // fixture.
        let ctx = test_execution_context("acme-realty");
        assert_eq!(ctx.tenant_id.as_ref(), "acme-realty");
        assert_eq!(ctx.actor, Actor::Internal("test-fixture".into()));
        assert!(
            ctx.deadline.is_none(),
            "fixture must not impose a deadline on tests"
        );
    }
}
