//! Code Map: Versioning convention + dyn-compatibility
//! - `ExampleThingV1`: A sample "do a thing" interface, first
//!   version. The "V1" in the name is on purpose — see the story
//!   below.
//! - `ExampleThingV2`: A sample "do a thing" interface, second
//!   version. It adds a new parameter (`hint`). It is a *new*
//!   trait, not a modified V1.
//! - `ExampleThingImpl`: A reference implementation that satisfies
//!   both V1 and V2. Used by the conformance tests as the
//!   "correct adapter."
//!
//! Story (plain English): Imagine a power socket on the wall.
//! The plug shape is the *interface*. If you change the plug
//! shape, every device that uses the old shape breaks at once.
//! The locked decision here is: when you need to change the
//! shape, you *add a new socket next to the old one* instead of
//! replacing it. V1 and V2 are two sockets side by side. V1
//! never changes — old devices keep working forever. V2 is a
//! new shape for new devices. The numbering ("V1", "V2") in the
//! type names is the rule that says "this is a different socket,
//! not a changed one." The other part of the file proves the
//! socket can be used through a borrowed handle (`Arc<dyn
//! ExampleThingV1>`), which is how the kernel actually hands
//! plugins around.
//!
//! CID Index:
//! CID:versioning-example-001 -> ExampleThingV1
//! CID:versioning-example-002 -> ExampleThingV2
//! CID:versioning-example-003 -> ExampleThingImpl
//!
//! Quick lookup: rg -n "CID:versioning-example-" crates/afa-contracts/src/versioning_example.rs

use crate::error::ExampleStoreErrorV1;
use async_trait::async_trait;

// CID:versioning-example-001 - ExampleThingV1
// Purpose: A sample "do a thing" interface, version 1. It is the
// first socket on the wall. It will never change — new behaviour
// goes into V2, not into V1. `Send + Sync` is required so the
// kernel can hand a borrowed handle to plugins.
// Uses: async-trait (so the trait can have async methods and
// still be usable through `Arc<dyn ExampleThingV1>`).
// Used by: the conformance test (the "correct" adapter is a V1
// implementor), and as a teaching example of the suffix-in-name
// versioning rule.
#[async_trait]
pub trait ExampleThingV1: Send + Sync {
    async fn do_it(&self) -> Result<(), ExampleStoreErrorV1>;
}

// CID:versioning-example-002 - ExampleThingV2
// Purpose: A sample "do a thing" interface, version 2. It is a
// *new* socket added next to V1, with one extra parameter
// (`hint`). V1 still works exactly as before; V2 is for new
// callers that want to pass a hint.
// Uses: async-trait, Send + Sync (same reasons as V1).
// Used by: the conformance test (the "correct" adapter also
// implements V2), and as the proof that adding V2 did not break
// any V1 test.
#[async_trait]
pub trait ExampleThingV2: Send + Sync {
    async fn do_it(&self, hint: &str) -> Result<(), ExampleStoreErrorV1>;
}

// CID:versioning-example-003 - ExampleThingImpl
// Purpose: The reference implementation that satisfies both V1
// and V2. It is the "good citizen" adapter the conformance
// tests use to prove the harness works. Both V1 and V2 just
// return `Ok(())` because the point of the example is the
// interface shape, not the body.
// Uses: nothing — it is a self-contained demo.
// Used by: the conformance test in `afa-contract-testing` (the
// "correct" adapter).
#[derive(Debug, Default)]
pub struct ExampleThingImpl;

#[async_trait]
impl ExampleThingV1 for ExampleThingImpl {
    async fn do_it(&self) -> Result<(), ExampleStoreErrorV1> {
        Ok(())
    }
}

#[async_trait]
impl ExampleThingV2 for ExampleThingImpl {
    async fn do_it(&self, _hint: &str) -> Result<(), ExampleStoreErrorV1> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test(flavor = "current_thread")]
    async fn v1_is_dyn_compatible_and_still_works_after_v2_introduced() {
        // Two assertions in one body, no duplication:
        //   1. The `Arc<dyn ExampleThingV1>` cast compiles (the
        //      dyn-compat contract).
        //   2. `do_it()` returns `Ok(())` end-to-end.
        // This is also the regression-proof for FLOW Flow 4: if
        // V2's addition ever broke V1 (e.g. by re-introducing a
        // non-object-safe method into V1, or by removing the V1
        // trait entirely), this test would fail to compile.
        let handle: Arc<dyn ExampleThingV1> = Arc::new(ExampleThingImpl);
        handle.do_it().await.expect("do_it should succeed");
    }
}
