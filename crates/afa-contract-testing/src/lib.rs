//! Code Map: afa-contract-testing (the front door)
//! - `fixtures`: A small helper that builds a fresh
//!   `ExecutionContext` for conformance tests, so the tests do
//!   not all have to repeat the same setup boilerplate. See
//!   `fixtures.rs`.
//! - `harness`: The `run_suite!` macro that runs a named
//!   assertion against every named adapter, and turns a
//!   "something went wrong" failure into a clearly-named
//!   `do_it_fails__adapter_name` test. See `harness.rs`.
//!
//! Story (plain English): This is the front door of the
//! conformance-test crate. The "conformance" idea is simple:
//! every plugin in the kernel promises to follow the same shape
//! as the example types in `afa-contracts`. The harness is the
//! tool that checks that promise — feed it a list of assertions
//! and a list of plugins, and it runs every assertion against
//! every plugin. If a plugin breaks, you see exactly which one
//! and exactly which assertion failed, not a single opaque "the
//! suite panicked" line.
//!
//! CID Index:
//! CID:afa-contract-testing-lib-001 -> fixtures
//! CID:afa-contract-testing-lib-002 -> harness
//! CID:afa-contract-testing-lib-003 -> embedding
//!
//! Quick lookup: rg -n "CID:afa-contract-testing-lib-" crates/afa-contract-testing/src/lib.rs

#![doc(html_root_url = "https://docs.rs/afa-contract-testing/0.1.0")]

// CID:afa-contract-testing-lib-001 - fixtures
// Purpose: Re-export the test-context fixture module.
// Used by: every conformance test in every downstream crate.
pub mod fixtures;
// CID:afa-contract-testing-lib-002 - harness
// Purpose: Re-export the `run_suite!` macro module.
// Used by: every conformance test in every downstream crate.
pub mod harness;
// CID:afa-contract-testing-lib-003 - embedding
// Purpose: Re-export the embedding
// conformance module (mock adapter + 12
// assertions for the `EmbeddingV1`
// contract, run via `run_suite!`).
// Used by: Pack #25 (`afa-plugin-embedding-local`,
// `afa-plugin-embedding-ollama`) to verify
// the adapters conform to the v1 contract;
// `afa-cli embedding status` uses the mock
// to display the v1 card in dev mode.
// See `embedding/mod.rs` for the Code Map.
//
// The `#[cfg(test)]` gate is deliberate:
// the embedding module depends on the
// `paste` crate (used by the `run_suite!`
// macro) and the `async-trait` crate
// (used by the mock adapter), both of
// which are dev-deps of
// `afa-contract-testing` (the lib target
// is just the `fixtures` + `harness`
// exports; the conformance tests run in
// `cargo test`, not in the lib).
#[cfg(test)]
pub mod embedding;
