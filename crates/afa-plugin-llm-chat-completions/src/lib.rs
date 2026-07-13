//! Code Map: afa-plugin-llm-chat-completions (the OpenAI Chat Completions adapter)
//! - `ChatCompletionsAdapter`: The concrete `LlmV1` adapter for
//!   any service that speaks the OpenAI Chat Completions wire
//!   format (`POST {base_url}/chat/completions`). This is
//!   intentionally separate from `ResponsesAdapter` in
//!   `afa-plugin-llm-http` (which targets the new OpenAI
//!   Responses API at `/v1/responses`). The two are sibling
//!   adapters; the difference is the wire format. A future
//!   contributor who wants to add another wire shape (Claude
//!   Messages, Gemini, etc.) creates a new plugin under
//!   `crates/afa-plugin-*`, not a new method on this one.
//! - `ChatCompletionsConfig`: The static, immutable config the
//!   adapter is built from. Carries the model name, the
//!   `key_ref` for the sealed API key, the `capabilities`
//!   card, the `base_url` (the adapter appends
//!   `/chat/completions` to it), and a human-readable
//!   `provider_name` that lands on every audit event so a
//!   dashboard can group calls by provider (e.g.
//!   "freellmapi", "groq", "ollama-local").
//! - `key_wiring`: The same 3-step pattern as
//!   `afa-plugin-llm-http`: cache, retry on 401, zeroize on
//!   drop. Duplicated here (rather than DRY'd into
//!   `afa-llm`) because the holder's config type is
//!   different and a single shared `UnsealedHolder` would
//!   force the http and chat-completions plugins to depend on
//!   each other's config types. The pattern is small
//!   (~150 lines) and the duplication is a deliberate
//!   trade-off; a future "DRY key wiring" pack can extract
//!   it without behavior change.
//!
//! Story (plain English): The `ChatCompletionsAdapter` is
//! the "Lend Your Voice" specialist that talks the older
//! OpenAI standard (`/chat/completions`). OpenAI itself
//! still supports it, and a *lot* of services do too:
//! Groq, Cerebras, SambaNova, NVIDIA NIM, Mistral,
//! OpenRouter, GitHub Models, Fireworks, Ollama (when
//! called via its `/v1` shim), LM Studio, llama.cpp's
//! server, vLLM, and the user's own `freellmapi` proxy.
//! When a workflow asks for an LLM, the switchboard
//! (`CapabilityRegistry`) hands the request to whichever
//! specialist was hired for that model — this one for
//! "any Chat-Completions-speaking service," the
//! Responses-API specialist in `afa-plugin-llm-http` for
//! "OpenAI's new API." The specialist has one permanent
//! job: talk to the vendor on the workflow's behalf, using
//! the sealed API key the security engine hands it. If the
//! vendor says "your key is bad" (HTTP 401), the specialist
//! re-unseals the key (the operator may have rotated it)
//! and tries once more, then gives up. Every request
//! stamps three small tickets on the log so an auditor can
//! later reconstruct "who asked for what from which
//! provider, did it work, and how long did it take?" —
//! without reading the question or the answer.
//!
//! CID Index:
//! CID:afa-plugin-llm-chat-completions-001 -> ChatCompletionsAdapter
//! CID:afa-plugin-llm-chat-completions-002 -> ChatCompletionsConfig
//! CID:afa-plugin-llm-chat-completions-003 -> key_wiring
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-chat-completions-" crates/afa-plugin-llm-chat-completions/src/

#![doc(html_root_url = "https://docs.rs/afa-plugin-llm-chat-completions/0.1.0")]

pub mod adapter;
pub mod config;
pub mod key_wiring;
pub mod streaming;
