//! Code Map: Model capabilities
//! - `ModelCapabilities`: The "what can this model do?"
//!   description. Three fields: `max_context_tokens` (the
//!   hard cap on prompt + completion tokens, used by
//!   workflows to size their context window), and two
//!   feature flags (`supports_vision`, `supports_tool_use`).
//!   The values are decided at adapter construction
//!   (e.g. `ResponsesConfig` sets them for the model it is
//!   hard-wired to) and never change for the process
//!   lifetime.
//!
//! Story (plain English): The capabilities card is the
//! small name tag the specialist wears on their lapel.
//! It says "I can read up to 128,000 words at a time,"
//! "I can see images," and "I can call tools." A
//! workflow that wants to know "will this model fit my
//! 200,000-word document?" checks the card before
//! sending. The card is decided when the specialist is
//! hired (the adapter is constructed) and never
//! changes — a gpt-4o adapter is always a gpt-4o
//! adapter, not a chameleon that flips between models.
//!
//! CID Index:
//! CID:llm-capabilities-001 -> ModelCapabilities
//!
//! Quick lookup: rg -n "CID:llm-capabilities-" crates/afa-contracts/src/llm/capabilities.rs

use serde::{Deserialize, Serialize};

// CID:llm-capabilities-001 - ModelCapabilities
// Purpose: The "what can this model do?" description.
// Three fields: `max_context_tokens` (the hard cap on
// prompt + completion tokens, used by workflows to
// size their context window — e.g. 128,000 for gpt-4o,
// 200,000 for claude-sonnet), and two feature flags
// (`supports_vision`, `supports_tool_use`). The values
// are decided at adapter construction and never change
// for the process lifetime. There is no per-request
// negotiation: the model is the model.
// Uses: nothing.
// Used by: `LlmV1::describe_capabilities`,
// `ResponsesConfig` (the source of the values).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelCapabilities {
    /// The hard cap on prompt + completion tokens.
    /// E.g. `128_000` for gpt-4o, `200_000` for
    /// claude-sonnet. Used by workflows to size
    /// their context window.
    pub max_context_tokens: u32,
    /// Whether the model accepts image content
    /// blocks. `false` for gpt-3.5-turbo, `true` for
    /// gpt-4o.
    pub supports_vision: bool,
    /// Whether the model accepts tool definitions
    /// and emits `ToolCall`s. `false` for the old
    /// completions API, `true` for gpt-4o.
    pub supports_tool_use: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_capabilities_carries_all_three_fields() {
        // The three fields are the locked shape
        // from the TRD §2.2.9: a workflow that
        // wants to check any of the three must
        // find them on this struct.
        let caps = ModelCapabilities {
            max_context_tokens: 128_000,
            supports_vision: true,
            supports_tool_use: true,
        };
        assert_eq!(caps.max_context_tokens, 128_000);
        assert!(caps.supports_vision);
        assert!(caps.supports_tool_use);
    }

    #[test]
    fn model_capabilities_round_trips_through_serde() {
        // A workflow may persist the capabilities
        // (e.g. to log which model handled a
        // request). The round-trip must preserve
        // every field exactly.
        let caps = ModelCapabilities {
            max_context_tokens: 200_000,
            supports_vision: false,
            supports_tool_use: true,
        };
        let json = serde_json::to_string(&caps).expect("serialize");
        let back: ModelCapabilities = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, caps);
    }
}
