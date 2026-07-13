//! Code Map: LLM non-streaming response shape
//! - `CompletionResponse`: The two-variant reply envelope.
//!   `TextReply` (the model wrote a text answer) or
//!   `ToolCalls` (the model wants to call one or more
//!   tools). Both variants carry the `Usage` block so the
//!   caller knows how many tokens were burned.
//! - `ToolCallRequest`: One tool the model wants to call.
//!   Carries the vendor's `id` (so a future turn can match
//!   the `ToolResult` back to the call), the tool `name`,
//!   and the parsed `arguments` (the vendor returns a JSON
//!   string; the adapter parses it into a
//!   `serde_json::Value`).
//! - `Usage`: Exactly two fields — `prompt_tokens` and
//!   `completion_tokens`. No `total_tokens` (derivable) and
//!   no `cached_tokens` / `reasoning_tokens` (vendor-
//!   specific, deferred until a workflow needs them).
//!
//! Story (plain English): The non-streaming reply is the
//! sealed envelope the specialist hands back. It is either
//! a single sheet of paper with the answer (`TextReply`) or
//! a stack of "please call these tools" cards (`ToolCalls`).
//! In both cases, the specialist stamps a small receipt on
//! the envelope (`Usage`) so the operator knows how much it
//! cost. The receipt only has two numbers — the question
//! size and the answer size — because that is all the
//! operator needs to know; anything more specific (e.g.
//! "reasoning tokens") is deferred until a workflow asks
//! for it.
//!
//! CID Index:
//! CID:llm-response-001 -> CompletionResponse
//! CID:llm-response-002 -> ToolCallRequest
//! CID:llm-response-003 -> Usage
//!
//! Quick lookup: rg -n "CID:llm-response-" crates/afa-contracts/src/llm/response.rs

use serde::{Deserialize, Serialize};

// CID:llm-response-001 - CompletionResponse
// Purpose: The two-variant reply envelope. `TextReply`
// when the model wrote a text answer; `ToolCalls` when
// the model wants to call one or more tools. Both
// variants carry the `Usage` block so the caller knows
// how many tokens were burned. The choice between the
// two variants is the model's, not the engine's — the
// engine reports whichever the vendor returned.
// Uses: Usage, ToolCallRequest.
// Used by: every workflow that calls `llm.complete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompletionResponse {
    /// The model wrote a text answer. The `content` is
    /// the full answer (not a stream — for the streaming
    /// variant, see `CompletionChunk::TextDelta`).
    /// The `usage` is the token-burn receipt.
    TextReply { content: String, usage: Usage },
    /// The model wants to call one or more tools. The
    /// `calls` is the list of tool calls the model
    /// produced (each one is a `ToolCallRequest`). The
    /// `usage` is the token-burn receipt (the model
    /// still spent tokens "thinking" about which tool
    /// to call, even if it produced no text).
    ToolCalls {
        calls: Vec<ToolCallRequest>,
        usage: Usage,
    },
}

// CID:llm-response-002 - ToolCallRequest
// Purpose: One tool the model wants to call. The
// vendor's `id` is preserved so a future turn (the
// `ConversationItem::ToolResult` variant) can match the
// result back to the call — the OpenAI Responses API
// uses the `id` to glue a `ToolResult` to its
// originating `ResponseFunctionToolCall`. The `name`
// is the tool name (e.g. `"search_listings"`) the
// workflow dispatches on. The `arguments` is the
// vendor's JSON string, already parsed into a
// `serde_json::Value` by the adapter's response
// mapper — the workflow never sees the raw JSON
// string.
// Uses: serde_json::Value.
// Used by: `CompletionResponse::ToolCalls.calls`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallRequest {
    /// The vendor's tool-call id (used to match a
    /// `ToolResult` to the originating call in a
    /// multi-turn flow).
    pub id: String,
    /// The tool name (e.g. `"search_listings"`).
    pub name: String,
    /// The parsed arguments. A `serde_json::Value` so
    /// the workflow can `as_object().and_then(|o|
    /// o.get("query"))` without re-parsing.
    pub arguments: serde_json::Value,
}

// CID:llm-response-003 - Usage
// Purpose: The token-burn receipt. Exactly two fields:
// `prompt_tokens` (the question size, including the
// system prompt + messages + tool definitions) and
// `completion_tokens` (the answer size). No
// `total_tokens` because it is `prompt_tokens +
// completion_tokens` and the consumer can add them.
// No `cached_tokens` or `reasoning_tokens` because they
// are vendor-specific (Anthropic, OpenAI, Google all
// name them differently) and deferred until a
// workflow asks for them.
// Uses: nothing.
// Used by: `CompletionResponse::TextReply.usage`,
// `CompletionResponse::ToolCalls.usage`,
// `CompletionChunk::Finished.usage`,
// `CompletionCompleted.prompt_tokens` /
// `completion_tokens`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    /// The number of tokens in the prompt (system +
    /// messages + tools).
    pub prompt_tokens: u32,
    /// The number of tokens in the completion (text
    /// + tool-call JSON).
    pub completion_tokens: u32,
}

impl Usage {
    /// The total tokens burned (`prompt_tokens +
    /// completion_tokens`). Convenience for callers
    /// that want the sum.
    pub fn total(&self) -> u32 {
        self.prompt_tokens + self.completion_tokens
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_total_is_prompt_plus_completion() {
        // The convenience method is a deliberate
        // contract: callers can compute the total
        // without adding the two fields themselves.
        let u = Usage {
            prompt_tokens: 10,
            completion_tokens: 5,
        };
        assert_eq!(u.total(), 15);
    }

    #[test]
    fn completion_response_text_reply_round_trips() {
        // A `TextReply` is the common case. The
        // round-trip must preserve the content and
        // the usage exactly.
        let r = CompletionResponse::TextReply {
            content: "hello".into(),
            usage: Usage {
                prompt_tokens: 3,
                completion_tokens: 2,
            },
        };
        let json = serde_json::to_string(&r).expect("serialize");
        let back: CompletionResponse = serde_json::from_str(&json).expect("deserialize");
        match back {
            CompletionResponse::TextReply { content, usage } => {
                assert_eq!(content, "hello");
                assert_eq!(usage.total(), 5);
            }
            _ => panic!("expected TextReply"),
        }
    }

    #[test]
    fn completion_response_tool_calls_round_trip_with_parsed_arguments() {
        // A `ToolCalls` round-trip must preserve
        // the parsed `arguments` (not a string —
        // the adapter has already parsed the
        // vendor's JSON into a `serde_json::Value`).
        let r = CompletionResponse::ToolCalls {
            calls: vec![ToolCallRequest {
                id: "call_abc".into(),
                name: "search_listings".into(),
                arguments: serde_json::json!({"query": "Warsaw"}),
            }],
            usage: Usage {
                prompt_tokens: 20,
                completion_tokens: 8,
            },
        };
        let json = serde_json::to_string(&r).expect("serialize");
        let back: CompletionResponse = serde_json::from_str(&json).expect("deserialize");
        match back {
            CompletionResponse::ToolCalls { calls, usage } => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].name, "search_listings");
                assert_eq!(calls[0].arguments["query"], "Warsaw");
                assert_eq!(usage.total(), 28);
            }
            _ => panic!("expected ToolCalls"),
        }
    }
}
