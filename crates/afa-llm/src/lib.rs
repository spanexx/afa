//! Code Map: afa-llm (the vendor-neutral LLM engine capability)
//! - `LlmV1` is the locked contract from
//!   `afa_contracts::LlmV1`. This crate re-exports it as
//!   `afa_llm::LlmV1` so adapters that depend on `afa-llm`
//!   do not have to import `afa-contracts` directly.
//! - The conformance suite and the `MockAdapter` live
//!   here. Every real adapter (the OpenAI one in
//!   `afa-plugin-llm-http`, a future Claude one, a future
//!   Ollama one) inherits all conformance tests by
//!   depending on this crate and running the suite.
//!
//! Story (plain English): This crate is the "Lend Your
//! Voice" suite for the switchboard. It is not a
//! specialist itself — it is a practice room where every
//! specialist (OpenAI, Claude, Ollama, the local mock
//! one) tries out the same set of test customers. The
//! suite's two halves are: a fake specialist
//! (`MockAdapter`) that runs the tests hermetically (no
//! network, no API key), and a folder of test cases
//! (the `conformance` module) that any specialist can
//! opt into. The OpenAI specialist in
//! `afa-plugin-llm-http` runs the same folder of test
//! customers against a wiremock-rs mock server, so the
//! exact same questions that passed the fake specialist
//! also pass the real one.
//!
//! CID Index:
//! CID:afa-llm-001 -> the LlmV1 re-export
//! CID:afa-llm-002 -> MockAdapter (the test fixture)
//! CID:afa-llm-003 -> the conformance suite
//!
//! Quick lookup: rg -n "CID:afa-llm-" crates/afa-llm/src/

#![doc(html_root_url = "https://docs.rs/afa-llm/0.1.0")]

// CID:afa-llm-001 - LlmV1 re-export
// Purpose: Re-export the locked v1 contract from
// `afa-contracts` so adapter crates do not have to
// import `afa-contracts` directly. The re-export
// keeps the surface narrow: "I am an LLM engine
// adapter" is a one-import fact for the consumer.
pub use afa_contracts::LlmV1;

pub mod conformance;
pub mod mock_adapter;
