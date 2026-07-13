//! Code Map: ResponsesConfig
//! - `ResponsesConfig`: The static, immutable config the
//!   adapter is built from. Carries the model name (the
//!   adapter is hard-wired to one model — there is no
//!   per-request model override), the `key_ref` (the
//!   `SecretRef` the security engine handed back when the
//!   operator pre-sealed the API key at startup), the
//!   `capabilities` (a pre-built `ModelCapabilities` the
//!   adapter returns from `describe_capabilities`), and
//!   the vendor `base_url` (the `https://...` prefix the
//!   adapter appends `/v1/responses` to — overridable
//!   in tests so wiremock-rs can intercept the call).
//!
//! Story (plain English): The config is the small card
//! the Responses-API specialist clips to their lapel. It
//! says "I am a `gpt-4o` specialist, here is the receipt
//! for my API key, my context window is 128,000 tokens,
//! and the URL of the OpenAI Responses front desk I
//! report to." The card is decided when the specialist
//! is hired (at startup) and never changes. A workflow
//! that wants to talk to a different model hires a
//! different specialist.
//!
//! CID Index:
//! CID:afa-plugin-llm-http-config-001 -> ResponsesConfig
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-http-config-" crates/afa-plugin-llm-http/src/config.rs

use afa_contracts::{ModelCapabilities, SecretRef};

/// The production OpenAI Responses API
/// base URL. The adapter appends
/// `/v1/responses` to this when building
/// the request URL.
pub const OPENAI_RESPONSES_BASE_URL: &str = "https://api.openai.com";

// CID:afa-plugin-llm-http-config-001 - ResponsesConfig
// Purpose: The static, immutable config the
// adapter is built from. Carries the model name
// (the adapter is hard-wired to one model —
// there is no per-request model override), the
// `key_ref` (the `SecretRef` the security engine
// handed back when the operator pre-sealed the
// API key at startup), the `capabilities` (a
// pre-built `ModelCapabilities` the adapter
// returns from `describe_capabilities`), and
// the vendor `base_url` (overridable so tests
// can route the call to a wiremock-rs server).
// The struct is `Clone` so a single
// `ResponsesConfig` can be reused across many
// `ResponsesAdapter::new` calls in tests; in
// production, exactly one config is built at
// startup and cloned into one adapter.
// Uses: ModelCapabilities (the engine's
// capabilities card), SecretRef (the receipt for
// the sealed API key).
// Used by: `ResponsesAdapter::new`,
// `ResponsesAdapter::describe_capabilities` (returns
// `self.config.capabilities.clone()`).
#[derive(Debug, Clone)]
pub struct ResponsesConfig {
    /// The model name (e.g. `"gpt-4o"`). The
    /// adapter is hard-wired to one model —
    /// there is no per-request model override.
    pub model: String,
    /// The `SecretRef` for the API key. The
    /// operator pre-seals the key at
    /// startup (calling
    /// `security.seal(plaintext,
    /// "openai-prod-key")`) and passes the
    /// resulting `SecretRef` here. The
    /// adapter uses this ref in
    /// `UnsealedHolder::get_or_unseal`.
    pub key_ref: SecretRef,
    /// The static capabilities card. The
    /// adapter returns this from
    /// `describe_capabilities` without any
    /// computation.
    pub capabilities: ModelCapabilities,
    /// The vendor base URL (the `https://...`
    /// prefix the adapter appends
    /// `/v1/responses` to). The default is
    /// the production OpenAI URL; tests
    /// override it to point at a
    /// wiremock-rs server. The value is
    /// captured per-adapter at construction
    /// time so parallel tests with
    /// different wiremock servers do not
    /// trample each other via a process-
    /// global env var.
    pub base_url: String,
}

impl ResponsesConfig {
    /// Build a config for `gpt-4o` against the
    /// OpenAI Responses API (128k context,
    /// supports vision, supports tools). A common
    /// case; constructor name makes the choice
    /// obvious at the call site. The
    /// `key_ref` must already point to a sealed
    /// API key in the security engine. Uses the
    /// production OpenAI Responses base URL;
    /// tests that need to redirect to a
    /// wiremock-rs server should use
    /// `responses_gpt_4o_with_base_url` (or
    /// mutate the field directly).
    pub fn responses_gpt_4o(key_ref: SecretRef) -> Self {
        Self::responses_gpt_4o_with_base_url(key_ref, OPENAI_RESPONSES_BASE_URL)
    }

    /// Build a config for `gpt-4o` against the
    /// OpenAI Responses API with a custom vendor
    /// base URL. Used by tests that route the
    /// call to a wiremock-rs server; in
    /// production, prefer `responses_gpt_4o`
    /// (which uses the production URL).
    pub fn responses_gpt_4o_with_base_url(key_ref: SecretRef, base_url: &str) -> Self {
        Self {
            model: "gpt-4o".into(),
            key_ref,
            capabilities: ModelCapabilities {
                max_context_tokens: 128_000,
                supports_vision: true,
                supports_tool_use: true,
            },
            base_url: base_url.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use afa_contracts::SecretRef;

    #[test]
    fn responses_gpt_4o_preset_has_the_locked_capabilities() {
        // The preset must produce exactly the
        // 128k / vision+tools card the rest of
        // the test suite expects. A future
        // contributor who changes the cap
        // numbers here is changing the contract
        // for every test in the file.
        let key_ref = SecretRef {
            name: "openai-prod-key".into(),
            version: 1,
        };
        let c = ResponsesConfig::responses_gpt_4o(key_ref.clone());
        assert_eq!(c.model, "gpt-4o");
        assert_eq!(c.key_ref, key_ref);
        assert_eq!(c.capabilities.max_context_tokens, 128_000);
        assert!(c.capabilities.supports_vision);
        assert!(c.capabilities.supports_tool_use);
        // The default base URL is the
        // production OpenAI Responses endpoint.
        assert_eq!(c.base_url, OPENAI_RESPONSES_BASE_URL);
    }

    #[test]
    fn responses_gpt_4o_with_base_url_overrides_the_endpoint() {
        // The `with_base_url` preset lets
        // tests point the adapter at a
        // wiremock-rs server. The custom URL
        // is preserved; the rest of the
        // preset is the same as
        // `responses_gpt_4o`.
        let key_ref = SecretRef {
            name: "x".into(),
            version: 1,
        };
        let c = ResponsesConfig::responses_gpt_4o_with_base_url(
            key_ref.clone(),
            "http://127.0.0.1:9999",
        );
        assert_eq!(c.model, "gpt-4o");
        assert_eq!(c.key_ref, key_ref);
        assert_eq!(c.base_url, "http://127.0.0.1:9999");
    }
}
