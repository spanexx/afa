//! Code Map: LLM request shape
//! - `CompletionRequest`: The full envelope a workflow hands
//!   to `llm.complete` / `llm.stream_complete`. Carries a
//!   top-level `system` prompt, a list of `ConversationItem`s
//!   (which can be text + images + tool results as
//!   first-class content), a list of `ToolDefinition`s, and
//!   `SamplingParams`. The `model` field is intentionally
//!   absent — the model is per-adapter, decided at
//!   construction.
//! - `ConversationItem`: One turn in a multi-turn chat.
//!   Three variants: a `UserMessage` (the human or a
//!   system-injected actor), an `AssistantMessage` (a
//!   previous model reply being replayed for context), and a
//!   `ToolResult` (the output of a tool the assistant called
//!   in a previous turn, fed back in as the next turn).
//! - `ContentBlock`: The body of a message. A `Text(String)`
//!   block, or an `Image` block carrying either a URL or
//!   base64 bytes. Adding a new content kind (audio, video)
//!   is a new variant on this enum — deliberate, ADR-backed.
//! - `ToolDefinition`: A tool the model is allowed to call.
//!   The `name` and `description` are the human-readable
//!   parts; the `parameters_schema` is a JSON Schema object
//!   the model reads to understand what arguments to pass.
//! - `SamplingParams`: The knobs that control the model's
//!   sampling — `temperature`, `max_output_tokens`, `top_p`,
//!   `stop`. The defaults (`SamplingParams::default()`)
//!   match the OpenAI Responses API defaults.
//!
//! Story (plain English): The request is the customer order
//! form at the specialist's desk. It says "here is the system
//! context (e.g. 'you are a helpful real-estate agent'), here
//! is the conversation so far (each turn is a
//! `ConversationItem`, which can be plain text, an image, or a
//! tool result), here are the tools you are allowed to call
//! (each one is a `ToolDefinition` with a JSON Schema
//! describing its arguments), and here are the knobs you
//! should turn while generating (the `SamplingParams`). The
//! specialist does not get to choose the model — that was
//! decided when the customer walked in the door (the adapter
//! is hard-wired to one model).
//!
//! CID Index:
//! CID:llm-request-001 -> CompletionRequest
//! CID:llm-request-002 -> ConversationItem
//! CID:llm-request-003 -> ContentBlock
//! CID:llm-request-004 -> ImageData
//! CID:llm-request-005 -> ToolDefinition
//! CID:llm-request-006 -> SamplingParams
//!
//! Quick lookup: rg -n "CID:llm-request-" crates/afa-contracts/src/llm/request.rs

use serde::{Deserialize, Serialize};

// CID:llm-request-001 - CompletionRequest
// Purpose: The full envelope a workflow hands to
// `llm.complete` / `llm.stream_complete`. The model is NOT
// here — it is per-adapter, decided at construction. The
// `system` field is the top-level instruction (a single
// string, not a list of `ConversationItem`s, by OpenAI
// convention). The `messages` field is the conversation
// history. The `tools` field is empty when the model is
// being used in plain chat mode. The `sampling` field uses
// `SamplingParams::default()` to opt into the vendor's
// defaults (temperature 1.0, max 4096 tokens, no top_p, no
// stop).
// Uses: ConversationItem, ToolDefinition, SamplingParams.
// Used by: every workflow call to `llm.complete` /
// `llm.stream_complete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletionRequest {
    /// Top-level system prompt (the model's "personality").
    /// `None` means "no system prompt; act on the messages
    /// only."
    pub system: Option<String>,
    /// The conversation history. Not a role-string list —
    /// a list of `ConversationItem` so that text, images, and
    /// tool results are first-class content.
    pub messages: Vec<ConversationItem>,
    /// The tools the model is allowed to call. Empty when
    /// the model is being used in plain chat mode.
    pub tools: Vec<ToolDefinition>,
    /// The sampling knobs. `SamplingParams::default()` =
    /// vendor defaults.
    pub sampling: SamplingParams,
}

// CID:llm-request-002 - ConversationItem
// Purpose: One turn in a multi-turn chat. Three variants:
// `UserMessage` (the human, or a system-injected actor
// pretending to be the human), `AssistantMessage` (a
// previous model reply being replayed for context — needed
// for the "stateless multi-turn" story where the workflow
// reconstructs the conversation on every call), and
// `ToolResult` (the output of a tool the assistant called in
// a previous turn, fed back in as the next turn so the
// model can use it).
// Uses: ContentBlock.
// Used by: `CompletionRequest::messages`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ConversationItem {
    /// A turn from the human (or a system-injected actor
    /// pretending to be the human). The `content` is a list
    /// of `ContentBlock`s so the turn can mix text and
    /// images.
    UserMessage { content: Vec<ContentBlock> },
    /// A turn from the assistant (a previous model reply).
    /// Replayed for context. The `content` is a list of
    /// `ContentBlock`s so the turn can mix text and
    /// images (e.g. a model that outputs an image).
    AssistantMessage { content: Vec<ContentBlock> },
    /// The result of a tool call from a previous turn. The
    /// `tool_call_id` is the vendor's id (so the model can
    /// match the result to the call), and `content` is the
    /// tool's output (text or images).
    ToolResult {
        tool_call_id: String,
        content: Vec<ContentBlock>,
    },
}

// CID:llm-request-003 - ContentBlock
// Purpose: The body of a single message. A `Text(String)`
// block (the common case), or an `Image` block carrying
// either a URL or base64 bytes. Adding a new content kind
// (audio, video, a future file attachment) is a new
// variant on this enum — deliberate, ADR-backed.
// Uses: ImageData.
// Used by: `ConversationItem::UserMessage.content`,
// `ConversationItem::AssistantMessage.content`,
// `ConversationItem::ToolResult.content`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ContentBlock {
    /// Plain text.
    Text(String),
    /// An image. The `mime_type` is the standard
    /// `image/png` / `image/jpeg` / `image/webp` string.
    /// The `data` is either a URL (the common case for
    /// publicly-hosted images) or base64-encoded bytes
    /// (for local images the workflow embeds directly).
    Image { mime_type: String, data: ImageData },
}

// CID:llm-request-004 - ImageData
// Purpose: How an image is carried — either as a URL
// (the common case) or as base64-encoded bytes (for local
// images the workflow embeds directly). The two-variant
// choice keeps the type self-documenting: a reader of an
// `ImageData` value knows immediately whether the bytes
// are inline or fetched separately.
// Uses: nothing.
// Used by: `ContentBlock::Image.data`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ImageData {
    /// A `https://...` URL the model fetches itself.
    Url(String),
    /// Base64-encoded image bytes the model decodes
    /// directly. The caller is responsible for the base64
    /// encoding (the engine never re-encodes).
    Base64(String),
}

// CID:llm-request-005 - ToolDefinition
// Purpose: A tool the model is allowed to call. The `name`
// is the function name the model will pass back in a
// `ToolCallRequest` (e.g. `"search_listings"`). The
// `description` is the human-readable prompt the model
// reads to decide when to call the tool. The
// `parameters_schema` is a JSON Schema object (passed
// through unchanged to the vendor) describing the
// arguments the tool expects.
// Uses: serde_json::Value (for the free-form JSON Schema).
// Used by: `CompletionRequest::tools`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    /// The function name (e.g. `"search_listings"`).
    pub name: String,
    /// The human-readable description the model reads to
    /// decide when to call the tool.
    pub description: String,
    /// A JSON Schema object describing the arguments. The
    /// engine never inspects this; it is passed through
    /// unchanged to the vendor.
    pub parameters_schema: serde_json::Value,
}

// CID:llm-request-006 - SamplingParams
// Purpose: The knobs that control the model's sampling.
// `temperature` (0.0 = deterministic, 1.0 = balanced,
// 2.0 = chaotic), `max_output_tokens` (the hard cap on
// the reply length, surfaced as
// `FinishReason::MaxTokens` when hit), `top_p` (nucleus
// sampling; `None` = vendor default), and `stop` (a list
// of strings that, when produced, end the generation).
// The defaults match the OpenAI Responses API defaults:
// temperature 1.0, max 4096 tokens, no top_p, no stop.
// Uses: nothing.
// Used by: `CompletionRequest::sampling`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SamplingParams {
    /// Sampling temperature. `1.0` = vendor default.
    pub temperature: f32,
    /// Hard cap on the reply length. `4096` = vendor
    /// default.
    pub max_output_tokens: u32,
    /// Nucleus sampling threshold. `None` = vendor
    /// default.
    pub top_p: Option<f32>,
    /// A list of strings that, when produced, end the
    /// generation. Empty = no early stop.
    pub stop: Vec<String>,
}

impl Default for SamplingParams {
    /// The OpenAI Responses API defaults: temperature
    /// 1.0, max 4096 tokens, no top_p, no stop.
    fn default() -> Self {
        Self {
            temperature: 1.0,
            max_output_tokens: 4096,
            top_p: None,
            stop: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sampling_params_defaults_match_openai_responses_api() {
        // The defaults are an explicit, locked
        // design choice: they match the OpenAI Responses
        // API defaults so a workflow that passes
        // `SamplingParams::default()` gets the same
        // behaviour as a workflow that calls the API
        // directly with no overrides.
        let s = SamplingParams::default();
        assert_eq!(s.temperature, 1.0);
        assert_eq!(s.max_output_tokens, 4096);
        assert_eq!(s.top_p, None);
        assert!(s.stop.is_empty());
    }

    #[test]
    fn completion_request_carries_every_field_needed() {
        // A workflow that builds a request should
        // be able to set all four fields (system,
        // messages, tools, sampling) and have them
        // round-trip through serde. If a field were
        // missing or hidden behind a builder, this
        // test would fail.
        let r = CompletionRequest {
            system: Some("you are a helpful agent".into()),
            messages: vec![ConversationItem::UserMessage {
                content: vec![ContentBlock::Text("hi".into())],
            }],
            tools: vec![],
            sampling: SamplingParams::default(),
        };
        let json = serde_json::to_string(&r).expect("serialize");
        let back: CompletionRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.system, r.system);
        assert_eq!(back.messages.len(), 1);
        assert!(back.tools.is_empty());
    }

    #[test]
    fn conversation_item_user_message_carries_text_and_image() {
        // A user message must be able to mix text
        // and image content. The variants are
        // enforced by the type system, not by a
        // string-typed field.
        let item = ConversationItem::UserMessage {
            content: vec![
                ContentBlock::Text("look at this:".into()),
                ContentBlock::Image {
                    mime_type: "image/png".into(),
                    data: ImageData::Url("https://example.com/x.png".into()),
                },
            ],
        };
        let json = serde_json::to_string(&item).expect("serialize");
        let back: ConversationItem = serde_json::from_str(&json).expect("deserialize");
        match back {
            ConversationItem::UserMessage { content } => {
                assert_eq!(content.len(), 2);
                assert!(matches!(content[0], ContentBlock::Text(_)));
                assert!(matches!(content[1], ContentBlock::Image { .. }));
            }
            _ => panic!("expected UserMessage"),
        }
    }

    #[test]
    fn tool_definition_round_trips_through_serde_with_a_real_json_schema() {
        // A `ToolDefinition`'s `parameters_schema` is
        // a `serde_json::Value` so any JSON Schema
        // passes through unchanged. We use a real
        // (tiny) schema as a regression-proof.
        let td = ToolDefinition {
            name: "search".into(),
            description: "search the catalog".into(),
            parameters_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "query": {"type": "string"}
                },
                "required": ["query"]
            }),
        };
        let json = serde_json::to_string(&td).expect("serialize");
        let back: ToolDefinition = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.name, "search");
        assert_eq!(
            back.parameters_schema["properties"]["query"]["type"],
            "string"
        );
    }
}
