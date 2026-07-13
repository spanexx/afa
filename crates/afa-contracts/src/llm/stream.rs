//! Code Map: LLM streaming shape
//! - `CompletionChunk`: One item yielded by a streaming
//!   completion. Four variants: `TextDelta` (a piece of
//!   text the model just produced), `ToolCallDelta` (a
//!   piece of a tool call — the consumer reassembles the
//!   full call from the deltas), `Finished` (the terminal
//!   chunk, carrying the `FinishReason` and the final
//!   `Usage`), and `Error` (a mid-stream failure, carrying
//!   the typed `LlmErrorV1`).
//! - `CompletionStream`: A type alias for
//!   `tokio::sync::mpsc::Receiver<CompletionChunk>` with
//!   capacity 64. Bounded capacity gives free
//!   backpressure: if the consumer is slow, the adapter's
//!   `send().await` awaits, which backpressures the
//!   vendor's HTTP/2 stream. The consumer can also wrap
//!   the receiver in `tokio_stream::wrappers::ReceiverStream`
//!   to compose with `timeout`, `take`, etc.
//! - `FinishReason`: Why the model stopped generating.
//!   Five variants: `Stop` (natural end), `ToolCalls`
//!   (the model wants to call tools), `MaxTokens` (the
//!   `max_output_tokens` cap was hit), `ContentFilter`
//!   (the vendor's safety policy refused), `Cancelled`
//!   (the client dropped the receiver or `ctx.deadline`
//!   hit — this is an engine-level reason, not a
//!   vendor-level one).
//!
//! Story (plain English): The streaming reply is a roll
//! of stamps the specialist peels off one at a time. Each
//! stamp is a `CompletionChunk` — usually a small piece
//! of text (`TextDelta`), but sometimes a piece of a
//! "please call this tool" card (`ToolCallDelta`). When
//! the roll runs out, the last stamp is a special one
//! (`Finished`) that says "I'm done" and carries the
//! reason: the model finished naturally, hit the length
//! cap, or got cut off. If the specialist's connection
//! to the service dies mid-roll, the consumer gets one
//! last stamp marked `Error` (carrying the typed reason)
//! and the roll ends.
//!
//! CID Index:
//! CID:llm-stream-001 -> CompletionChunk
//! CID:llm-stream-002 -> CompletionStream
//! CID:llm-stream-003 -> FinishReason
//!
//! Quick lookup: rg -n "CID:llm-stream-" crates/afa-contracts/src/llm/stream.rs

use serde::{Deserialize, Serialize};

use super::error::LlmErrorV1;
use super::response::Usage;

// CID:llm-stream-001 - CompletionChunk
// Purpose: One item yielded by a streaming completion.
// Four variants: `TextDelta` (a piece of text the model
// just produced — the consumer concatenates them to
// reconstruct the full answer), `ToolCallDelta` (a piece
// of a tool call — the consumer reassembles the full
// call from the deltas by id), `Finished` (the terminal
// chunk, carrying the `FinishReason` and the final
// `Usage`), and `Error` (a mid-stream failure, carrying
// the typed `LlmErrorV1`). The choice to make deltas
// the unit (rather than a `Stream<String>` and a separate
// `Stream<ToolCall>`) is deliberate: a single stream
// preserves ordering, and `tokio::sync::mpsc` has
// built-in backpressure that a custom `Stream` would
// have to re-implement.
// Uses: FinishReason, Usage, LlmErrorV1.
// Used by: every workflow that calls
// `llm.stream_complete`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum CompletionChunk {
    /// A piece of text the model just produced. The
    /// consumer concatenates deltas to reconstruct the
    /// full answer (e.g. `"Hel"` + `"lo, "` + `"world"`).
    TextDelta(String),
    /// A piece of a tool call. The `id` identifies the
    /// call (so the consumer can accumulate deltas
    /// across many chunks into one `ToolCallRequest`),
    /// the `name_delta` is the (usually single-chunk)
    /// tool name, and `arguments_delta` is a piece of
    /// the JSON arguments string.
    ToolCallDelta {
        /// The vendor's call id (matches
        /// `ToolCallRequest::id` once the call is fully
        /// assembled).
        id: String,
        /// A piece of the tool name (usually the full
        /// name in one chunk).
        name_delta: String,
        /// A piece of the arguments JSON (the consumer
        /// concatenates across chunks and parses once
        /// the call is `Finished`).
        arguments_delta: String,
    },
    /// The terminal chunk. Carries the `reason` and
    /// the final `usage`. The consumer's
    /// `while let Some(chunk) = stream.recv().await`
    /// loop sees one of these and then `None` (the
    /// adapter drops the sender, closing the channel).
    Finished {
        /// Why the model stopped.
        reason: FinishReason,
        /// The final `Usage` (the sum of all tokens
        /// burned across the whole stream).
        usage: Usage,
    },
    /// A mid-stream failure. Carries the typed
    /// `LlmErrorV1` (e.g. `StreamInterrupted` if the
    /// vendor's connection died). The consumer's loop
    /// sees one of these and then `None`.
    Error(LlmErrorV1),
}

// CID:llm-stream-002 - CompletionStream
// Purpose: A type alias for
// `tokio::sync::mpsc::Receiver<CompletionChunk>` with
// capacity 64. Bounded capacity gives free
// backpressure: if the consumer is slow, the adapter's
// `send().await` awaits, which backpressures the
// vendor's HTTP/2 stream. The consumer can also wrap
// the receiver in
// `tokio_stream::wrappers::ReceiverStream` to compose
// with `timeout`, `take`, `chunks_timeout`, etc.
// Uses: `tokio::sync::mpsc`.
// Used by: every workflow that calls
// `llm.stream_complete`. The alias is `pub` so
// downstream code can name the type without reaching
// into tokio.
pub type CompletionStream = tokio::sync::mpsc::Receiver<CompletionChunk>;

// CID:llm-stream-003 - FinishReason
// Purpose: Why the model stopped generating. Five
// variants: `Stop` (natural end), `ToolCalls` (the
// model wants to call tools — the consumer reads the
// accumulated `ToolCallDelta`s), `MaxTokens` (the
// `max_output_tokens` cap was hit), `ContentFilter`
// (the vendor's safety policy refused), `Cancelled`
// (the client dropped the receiver or `ctx.deadline`
// hit — this is an engine-level reason, not a
// vendor-level one). The `Cancelled` variant is
// deliberately distinct from `Stop`: a
// `FinishReason::Cancelled` in the audit event tells
// the operator "the user gave up," not "the model
// finished."
// Uses: nothing.
// Used by: `CompletionChunk::Finished.reason`,
// `CompletionCompleted.finish_reason`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FinishReason {
    /// The model finished naturally (it produced an
    /// end-of-sequence token).
    Stop,
    /// The model wants to call one or more tools.
    /// The consumer reads the accumulated
    /// `ToolCallDelta`s.
    ToolCalls,
    /// The `max_output_tokens` cap was hit.
    MaxTokens,
    /// The vendor's safety policy refused (e.g. the
    /// model started producing disallowed content).
    ContentFilter,
    /// The client dropped the receiver or
    /// `ctx.deadline` hit. This is an engine-level
    /// reason (not a vendor-level one); it is
    /// surfaced via `CompletionChunk::Finished`
    /// (NOT `CompletionChunk::Error`) so the
    /// consumer can distinguish "we gave up" from
    /// "something went wrong."
    Cancelled,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn completion_chunk_text_delta_carries_just_a_string() {
        // The `TextDelta` variant is the common
        // case — small pieces of text the consumer
        // concatenates. The variant must carry
        // exactly one `String`, not a struct, to
        // keep the `mpsc::send` call as cheap as
        // possible.
        let c = CompletionChunk::TextDelta("Hel".into());
        let json = serde_json::to_string(&c).expect("serialize");
        let back: CompletionChunk = serde_json::from_str(&json).expect("deserialize");
        match back {
            CompletionChunk::TextDelta(s) => assert_eq!(s, "Hel"),
            _ => panic!("expected TextDelta"),
        }
    }

    #[test]
    fn completion_chunk_finished_carries_reason_and_usage() {
        // The terminal chunk must carry both the
        // `FinishReason` and the final `Usage` — a
        // consumer that discards one of them is
        // dropping audit data.
        let c = CompletionChunk::Finished {
            reason: FinishReason::Stop,
            usage: Usage {
                prompt_tokens: 10,
                completion_tokens: 5,
            },
        };
        let json = serde_json::to_string(&c).expect("serialize");
        let back: CompletionChunk = serde_json::from_str(&json).expect("deserialize");
        match back {
            CompletionChunk::Finished { reason, usage } => {
                assert_eq!(reason, FinishReason::Stop);
                assert_eq!(usage.total(), 15);
            }
            _ => panic!("expected Finished"),
        }
    }

    #[test]
    fn finish_reason_cancelled_is_distinct_from_stop() {
        // `Cancelled` and `Stop` are deliberately
        // separate variants: a
        // `FinishReason::Cancelled` in the audit
        // event tells the operator "the user gave
        // up," not "the model finished." If a
        // future refactor merges them, the audit
        // story is broken (compliance would no
        // longer be able to distinguish
        // user-cancellation from natural end).
        assert_ne!(FinishReason::Cancelled, FinishReason::Stop);
    }
}
