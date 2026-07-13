//! Code Map: ChatCompletionsConfig
//! - `ChatCompletionsConfig`: The static, immutable config
//!   the adapter is built from. Carries the model name
//!   (e.g. `"gpt-4o-mini"`, `"llama-3.1-70b"`, or whatever
//!   the upstream service exposes), the `key_ref` (the
//!   `SecretRef` the security engine handed back when the
//!   operator pre-sealed the API key at startup), the
//!   `capabilities` (a pre-built `ModelCapabilities` the
//!   adapter returns from `describe_capabilities`), the
//!   vendor `base_url` (the `https://...` prefix the
//!   adapter appends `/chat/completions` to — overridable
//!   in tests so wiremock-rs can intercept the call), and
//!   a human-readable `provider_name` that lands on every
//!   audit event (so a dashboard can group calls by
//!   provider, not just by model).
//!
//! Story (plain English): The config is the small card the
//! Chat-Completions specialist clips to their lapel. It
//! says "I am a `gpt-4o-mini` specialist on the `freellmapi`
//! proxy, here is the receipt for my API key, my context
//! window is 128,000 tokens, and the URL of the front desk I
//! report to." The card is decided when the specialist is
//! hired (at startup) and never changes. A workflow that
//! wants to talk to a different model hires a different
//! specialist with a different card. The
//! `provider_name` is the piece of the card that an auditor
//! reads to answer "which vendor answered this customer?".
//!
//! CID Index:
//! CID:afa-plugin-llm-openai-compat-config-001 -> ChatCompletionsConfig
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-openai-compat-config-" crates/afa-plugin-llm-openai-compat/src/config.rs

use afa_contracts::{ModelCapabilities, SecretRef};

/// The production OpenAI Chat Completions API
/// base URL. The adapter appends `/chat/completions`
/// to this when building the request URL. The
/// `https://api.openai.com` host is the canonical
/// OpenAI Chat Completions endpoint; the same path
/// shape works for every other OpenAI-compatible
/// service (Groq, Cerebras, Ollama's `/v1` shim,
/// llama.cpp's server, LM Studio, vLLM, etc.).
pub const OPENAI_CHAT_PROD_BASE_URL: &str = "https://api.openai.com/v1";

// CID:afa-plugin-llm-openai-compat-config-001 - ChatCompletionsConfig
// Purpose: The static, immutable config the
// adapter is built from. Carries the model name
// (e.g. `"gpt-4o-mini"`, `"llama-3.1-70b"`,
// `"mixtral-8x7b"`, or whatever the upstream service
// exposes), the `key_ref` (the receipt for the
// sealed API key), the `capabilities` (a pre-built
// `ModelCapabilities` the adapter returns from
// `describe_capabilities`), the vendor `base_url`
// (overridable so tests can route the call to a
// wiremock-rs server), and a human-readable
// `provider_name` that lands on every audit event.
// The struct is `Clone` so a single config can be
// reused across many `ChatCompletionsAdapter::new`
// calls in tests; in production, exactly one config
// is built at startup and cloned into one adapter.
// Uses: ModelCapabilities (the engine's
// capabilities card), SecretRef (the receipt for
// the sealed API key).
// Used by: `ChatCompletionsAdapter::new`,
// `ChatCompletionsAdapter::describe_capabilities`
// (returns `self.config.capabilities.clone()`).
#[derive(Debug, Clone)]
pub struct ChatCompletionsConfig {
    /// The model name (e.g. `"gpt-4o-mini"`,
    /// `"llama-3.1-70b"`). The adapter is
    /// hard-wired to one model — there is no
    /// per-request model override. A workflow
    /// that wants a different model hires a
    /// different adapter.
    pub model: String,
    /// The `SecretRef` for the API key. The
    /// operator pre-seals the key at startup
    /// (calling `security.seal(plaintext,
    /// "freellmapi-key")`) and passes the
    /// resulting `SecretRef` here. The adapter
    /// uses this ref in
    /// `UnsealedHolder::get_or_unseal`. A
    /// keyless provider (a service that needs
    /// no `Authorization` header, e.g. Ollama
    /// in `--keyless` mode or a local
    /// llama.cpp) can still pass a dummy
    /// `SecretRef` — the adapter will send the
    /// header anyway. Keyless support is a
    /// future-pack concern.
    pub key_ref: SecretRef,
    /// The static capabilities card. The
    /// adapter returns this from
    /// `describe_capabilities` without any
    /// computation.
    pub capabilities: ModelCapabilities,
    /// The vendor base URL (the `https://...`
    /// prefix the adapter appends
    /// `/chat/completions` to). The default
    /// is the production OpenAI URL; tests
    /// override it to point at a wiremock-rs
    /// server, and a user pointing the
    /// adapter at `freellmapi` passes
    /// `"http://localhost:3000/v1"`. The
    /// value is captured per-adapter at
    /// construction time so parallel tests
    /// with different wiremock servers do
    /// not trample each other via a
    /// process-global env var.
    pub base_url: String,
    /// A human-readable name for the
    /// provider (e.g. `"freellmapi"`,
    /// `"groq"`, `"ollama-local"`,
    /// `"openai-prod"`). Lands on every
    /// audit event so a dashboard can
    /// group calls by provider, not just
    /// by model. Two different providers
    /// might serve the same model name
    /// (e.g. `llama-3.1-70b` from both
    /// Groq and a self-hosted vLLM); the
    /// `provider_name` disambiguates.
    pub provider_name: String,
}

impl ChatCompletionsConfig {
    /// Build a config for OpenAI's `gpt-4o-mini`
    /// (128k context, no vision, supports tools).
    /// Uses the production OpenAI Chat Completions
    /// base URL. A common case; constructor name
    /// makes the choice obvious at the call site.
    /// The `key_ref` must already point to a
    /// sealed API key in the security engine.
    /// `provider_name` defaults to
    /// `"openai-prod"` for dashboards.
    pub fn gpt_4o_mini(key_ref: SecretRef) -> Self {
        Self::with_provider(
            "gpt-4o-mini",
            key_ref,
            ModelCapabilities {
                max_context_tokens: 128_000,
                supports_vision: false,
                supports_tool_use: true,
            },
            OPENAI_CHAT_PROD_BASE_URL,
            "openai-prod",
        )
    }

    /// Build a config for a custom OpenAI-compatible
    /// provider (e.g. `freellmapi` running on
    /// `localhost:3000`, Groq, Cerebras, Ollama's
    /// `/v1` shim, LM Studio, llama.cpp's server,
    /// vLLM). Takes the model name, the
    /// `key_ref`, the capabilities card, the
    /// `base_url` (including the `/v1` suffix for
    /// most services — see the doc on the field),
    /// and the `provider_name` for audit
    /// grouping. Use this when the preset above
    /// does not match.
    pub fn with_provider(
        model: &str,
        key_ref: SecretRef,
        capabilities: ModelCapabilities,
        base_url: &str,
        provider_name: &str,
    ) -> Self {
        Self {
            model: model.into(),
            key_ref,
            capabilities,
            base_url: base_url.into(),
            provider_name: provider_name.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gpt_4o_mini_preset_has_the_locked_capabilities() {
        // The preset must produce exactly the
        // 128k / no-vision / tools card the
        // rest of the test suite expects. A
        // future contributor who changes the
        // cap numbers here is changing the
        // contract for every test in the file.
        let key_ref = SecretRef {
            name: "openai-prod-key".into(),
            version: 1,
        };
        let c = ChatCompletionsConfig::gpt_4o_mini(key_ref.clone());
        assert_eq!(c.model, "gpt-4o-mini");
        assert_eq!(c.key_ref, key_ref);
        assert_eq!(c.capabilities.max_context_tokens, 128_000);
        assert!(!c.capabilities.supports_vision);
        assert!(c.capabilities.supports_tool_use);
        // The default base URL is the
        // production OpenAI Chat Completions
        // endpoint (note the `/v1` suffix —
        // the adapter appends
        // `/chat/completions`).
        assert_eq!(c.base_url, OPENAI_CHAT_PROD_BASE_URL);
        // The default `provider_name` is
        // `openai-prod` for dashboards.
        assert_eq!(c.provider_name, "openai-prod");
    }

    #[test]
    fn with_provider_preset_is_fully_user_controllable() {
        // The `with_provider` preset lets
        // the user point the adapter at
        // any OpenAI-compatible service.
        // The custom fields are preserved;
        // the rest is up to the caller.
        let key_ref = SecretRef {
            name: "freellmapi-key".into(),
            version: 1,
        };
        let c = ChatCompletionsConfig::with_provider(
            "llama-3.1-70b",
            key_ref.clone(),
            ModelCapabilities {
                max_context_tokens: 128_000,
                supports_vision: false,
                supports_tool_use: true,
            },
            "http://localhost:3000/v1",
            "freellmapi",
        );
        assert_eq!(c.model, "llama-3.1-70b");
        assert_eq!(c.key_ref, key_ref);
        assert_eq!(c.base_url, "http://localhost:3000/v1");
        assert_eq!(c.provider_name, "freellmapi");
    }
}
