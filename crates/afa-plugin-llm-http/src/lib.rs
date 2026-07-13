//! Code Map: afa-plugin-llm-http (the OpenAI adapter)
//! - `OpenAiAdapter`: The concrete `LlmV1` adapter for the
//!   OpenAI Responses API. Hard-wired to one model at
//!   construction (the model is in `OpenAiConfig`). All
//!   audit events (`CompletionRequested`,
//!   `CompletionCompleted`, `CompletionFailed`) are
//!   published on the event bus the constructor was given.
//! - `OpenAiConfig`: The static config (model name,
//!   capabilities, the name of the sealed key to look up
//!   via the security engine). Built once at startup;
//!   the model does not change for the process lifetime.
//! - `key_wiring`: The 3-step pattern that the adapter
//!   uses to fetch + unseal + (on 401) re-unseal the API
//!   key. See `key_wiring.rs`.
//!
//! Story (plain English): This crate is the OpenAI
//! specialist on the switchboard. When a workflow asks
//! for an LLM, the switchboard (`CapabilityRegistry`)
//! hands the request to this specialist. The specialist
//! has one permanent job: talk to the OpenAI Responses
//! API on the model's behalf, using the sealed API key
//! the security engine hands it. If the OpenAI service
//! says "your key is bad" (HTTP 401), the specialist
//! re-unseals the key (the operator may have rotated
//! it) and tries once more, then gives up. Every
//! request stamps three small tickets on the log so an
//! auditor can later reconstruct "who asked for what
//! from the OpenAI specialist, did it work, and how
//! long did it take?" — without reading the question or
//! the answer.
//!
//! CID Index:
//! CID:afa-plugin-llm-http-001 -> OpenAiAdapter
//! CID:afa-plugin-llm-http-002 -> OpenAiConfig
//! CID:afa-plugin-llm-http-003 -> key_wiring
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-http-" crates/afa-plugin-llm-http/src/

#![doc(html_root_url = "https://docs.rs/afa-plugin-llm-http/0.1.0")]

pub mod config;
pub mod key_wiring;
pub mod openai_adapter;
