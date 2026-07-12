//! Code Map: Conformance-test harness
//! - `run_suite!`: A declarative macro. You list the assertions
//!   you want to run, the adapters you want to run them against,
//!   and the macro generates one `#[tokio::test]` per
//!   (assertion, adapter) pair. If an adapter breaks, you see
//!   exactly which one in the test name.
//! - `run_suite_munch!`: An internal helper macro that the
//!   `run_suite!` macro uses to walk one assertion at a time
//!   down the list. (TT-muncher pattern; not for direct use.)
//! - `run_suite_one!`: An internal helper macro that the
//!   `run_suite_munch!` macro uses to emit the actual test
//!   function for a single (assertion, adapter) pair.
//!
//! Story (plain English): Imagine a driving school with a
//! standardised road test. Every student takes the same three
//! challenges (the assertions), and the school wants to see
//! exactly which student failed which challenge. If you just
//! ran "drive the loop" once and reported a single
//! pass/fail, you'd never know *where* the student failed.
//! The `run_suite!` macro is the score sheet: it gives every
//! student their own line for every challenge, with the
//! student's name and the challenge's name printed together
//! (e.g. `do_it_succeeds_correct` or `do_it_succeeds_broken`).
//! That way, a future you looking at the report sees:
//! "ah, the `broken` student failed `do_it_succeeds`" — not
//! "the suite panicked."
//!
//! CID Index:
//! CID:harness-001 -> run_suite!
//! CID:harness-002 -> run_suite_munch! (internal helper)
//! CID:harness-003 -> run_suite_one! (internal helper)
//!
//! Quick lookup: rg -n "CID:harness-" crates/afa-contract-testing/src/harness.rs

// CID:harness-001 - run_suite!
// Purpose: The "score sheet" macro. You give it a list of
// assertions and a list of adapters, and it generates one
// `#[tokio::test]` per (assertion, adapter) pair. A failing
// adapter produces a test whose name encodes *which* assertion
// and *which* adapter failed. Add the keyword `ignored` after
// an adapter entry to mark its generated test as `#[ignore]`
// (used for the validation demo, where the "broken" adapter
// is intentionally wrong but should not turn the default
// `cargo test` run red).
// Uses: tokio (the async runtime for the generated tests),
// paste (to glue assertion and adapter names into one test
// identifier).
// Used by: every conformance test in every downstream crate.
#[macro_export]
macro_rules! run_suite {
    (
        assertions: [ $($assertions:tt)* ],
        adapters:  [ $($adapters:tt)* ],
    ) => {
        $crate::run_suite_munch!(
            @assertions [ $($assertions)* ],
            @adapters  [ $($adapters)* ],
        );
    };
}

// CID:harness-002 - run_suite_munch! (internal helper)
// Purpose: A "one-at-a-time walker." The outer `run_suite!`
// macro has two lists (assertions and adapters), and Rust's
// built-in `macro_rules!` does not let you do an n-by-m
// expansion in one step. The muncher peels one assertion off
// the front of the list, hands it to `run_suite_one!` to
// expand against every adapter, then recurses on the rest of
// the assertions. When the list is empty, the recursion stops.
// Users never call this directly — it is only here to make
// `run_suite!` possible.
// Uses: nothing external — it is pure macro plumbing.
// Used by: `run_suite!` (called once per assertion).
#[macro_export]
#[doc(hidden)]
macro_rules! run_suite_munch {
    (
        @assertions [],
        @adapters $adapters:tt $(,)?
    ) => {};

    (
        @assertions [
            $assertion_name:ident => |$adapter_param:ident : &dyn $trait:path| async { $($body:tt)* }
            , $($rest:tt)*
        ],
        @adapters $adapters:tt $(,)?
    ) => {
        $crate::run_suite_one!(
            @assertion $assertion_name, $adapter_param, $trait,
            body: { $($body)* },
            @adapters $adapters
        );
        $crate::run_suite_munch!(
            @assertions [ $($rest)* ],
            @adapters $adapters,
        );
    };
}

// CID:harness-003 - run_suite_one! (internal helper)
// Purpose: The "emit one test" macro. Called by
// `run_suite_munch!` once per (assertion, adapter) pair, it
// generates a `#[tokio::test]` function whose name is
// `<assertion>_<adapter>`, then walks the rest of the
// adapter list to generate the next test. Two arms exist:
// one for normal adapters, one for adapters marked
// `ignored` (which get an `#[ignore]` attribute so they only
// run on demand with `cargo test -- --ignored`).
// Uses: paste (to glue names), tokio (the async runtime),
// and the body block the user supplied.
// Used by: `run_suite_munch!` (called once per pair).
#[macro_export]
#[doc(hidden)]
macro_rules! run_suite_one {
    (
        @assertion $assertion_name:ident, $adapter_param:ident, $trait:path,
        body: { $($body:tt)* },
        @adapters []
    ) => {};

    (
        @assertion $assertion_name:ident, $adapter_param:ident, $trait:path,
        body: { $($body:tt)* },
        @adapters [ $adapter_name:literal => $adapter_ty:ty, ignored $(, $($rest:tt)*)? ]
    ) => {
        ::paste::paste! {
            #[::tokio::test(flavor = "current_thread")]
            #[ignore = "validation demo: see IMPL Phase 5"]
            #[allow(non_snake_case)]
            async fn [<$assertion_name _ $adapter_name>]() {
                let adapter: $adapter_ty =
                    <$adapter_ty as ::core::default::Default>::default();
                let $adapter_param: &dyn $trait = &adapter;
                async { $($body)* }.await;
            }
        }
        $crate::run_suite_one!(
            @assertion $assertion_name, $adapter_param, $trait,
            body: { $($body)* },
            @adapters [ $( $($rest)* )? ]
        );
    };

    (
        @assertion $assertion_name:ident, $adapter_param:ident, $trait:path,
        body: { $($body:tt)* },
        @adapters [ $adapter_name:literal => $adapter_ty:ty, $($rest_adapters:tt)* ]
    ) => {
        ::paste::paste! {
            #[::tokio::test(flavor = "current_thread")]
            #[allow(non_snake_case)]
            async fn [<$assertion_name _ $adapter_name>]() {
                let adapter: $adapter_ty =
                    <$adapter_ty as ::core::default::Default>::default();
                let $adapter_param: &dyn $trait = &adapter;
                async { $($body)* }.await;
            }
        }
        $crate::run_suite_one!(
            @assertion $assertion_name, $adapter_param, $trait,
            body: { $($body)* },
            @adapters [ $($rest_adapters)* ]
        );
    };
}

#[cfg(test)]
mod tests {
    use afa_contracts::error::ExampleStoreErrorV1;
    use afa_contracts::versioning_example::ExampleThingImpl;
    use afa_contracts::ExampleThingV1;
    use async_trait::async_trait;

    /// A deliberately-broken adapter: `do_it` always returns
    /// `ExampleStoreErrorV1::Internal`.
    #[derive(Default)]
    struct BrokenExampleThingImpl;

    #[async_trait]
    impl ExampleThingV1 for BrokenExampleThingImpl {
        async fn do_it(&self) -> Result<(), ExampleStoreErrorV1> {
            Err(ExampleStoreErrorV1::Internal("deliberately wrong".into()))
        }
    }

    // The "correct" adapter is in the default test run. The "broken"
    // adapter is `ignored` so the default run stays green; run with
    // `cargo test -p afa-contract-testing -- --ignored` to see the
    // validation split (the `broken` test fails with a clearly-named
    // assertion error).
    run_suite!(
        assertions: [
            do_it_succeeds => |a: &dyn ExampleThingV1| async {
                a.do_it().await.expect("ok");
            },
        ],
        adapters: [
            "correct" => ExampleThingImpl,
            "broken"  => BrokenExampleThingImpl, ignored,
        ],
    );
}
