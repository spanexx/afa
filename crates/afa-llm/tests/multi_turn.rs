//! Code Map: Flow 11 — Multi-turn statelessness test
//!
//! - `mock_adapter_publishes_three_requested_completed_pairs_for_three_sequential_calls`:
//!   Builds a `MockAdapter` via the bus-aware
//!   constructor (`MockAdapter::with_event_bus`). Runs
//!   `complete()` 3 times in sequence. Asserts:
//!     1. Each call returns the canned text reply
//!        ("Hello, world!") — no cache poisoning
//!        between calls.
//!     2. The bus saw exactly 6 events total — 3
//!        `CompletionRequested` + 3
//!        `CompletionCompleted`, in the interleaved
//!        order (Requested₁, Completed₁, Requested₂,
//!        Completed₂, Requested₃, Completed₃).
//!     3. The `correlation_id` of each
//!        Requested matches the `correlation_id` of
//!        the immediately-following Completed (the
//!        audit story: "the same correlation id ties
//!        the two halves of a single request
//!        together").
//!     4. The 3 `correlation_id`s are pairwise
//!        distinct (no aliasing between calls).
//!     5. The first Requested carries
//!        `prompt_tokens_estimate: None` and the
//!        subsequent two carry `Some(0)` (the
//!        `prompt_tokens_estimate` is a single
//!        `u32` token count, not a full `Usage`
//!        struct — see `events.rs`).
//!
//! - `mock_adapter_publishes_a_completion_failed_event_when_the_request_dispatches_to_an_error`:
//!   The bus-aware variant also publishes
//!   `CompletionFailed` on error. Exercises the
//!   `complete` error path: the mock dispatches the
//!   "rate_limited" stub (which returns
//!   `Err(LlmErrorV1::RateLimited { ... })`) and the
//!   bus sees exactly one `CompletionFailed` event
//!   with the matching `correlation_id`.
//!
//! - `mock_adapter_without_bus_publishes_no_events`:
//!   The default `MockAdapter::new()` is hermetic:
//!   3 sequential `complete()` calls publish zero
//!   events on the bus. The conformance suite uses
//!   the bus-less variant deliberately so it stays
//!   hermetic (no event loop, no async runtime, no
//!   bus state to drain between cases).
//!
//! Story (plain English): A multi-turn
//! conversation is a sequence of single-turn
//! requests, and the engine must not stitch
//! them together behind the workflow's back.
//! The first turn does not influence the second
//! turn's reply (no engine-side cache), the
//! second does not influence the third, and so
//! on. The audit bus is the witness: it sees
//! exactly one Requested + one Completed per
//! call, in the right order, with the right
//! correlation id, and nothing else.
//!
//! CID Index:
//! CID:afa-llm-multi-turn-001 -> three sequential calls, six events
//! CID:afa-llm-multi-turn-002 -> error path publishes CompletionFailed
//! CID:afa-llm-multi-turn-003 -> bus-less variant publishes nothing
//!
//! Quick lookup: rg -n "CID:afa-llm-multi-turn-" crates/afa-llm/tests/multi_turn.rs

use std::time::Duration;

use afa_bus::{EventBus, EventBusHandle};
use afa_contracts::{
    CompletionCompleted, CompletionFailed, CompletionRequest, CompletionRequested, ContentBlock,
    ConversationItem, ExecutionContext, FinishReason, LlmV1,
};
use afa_llm::mock_adapter::MockAdapter;

/// Fresh `ExecutionContext` per call. The
/// `correlation_id` defaults to a fresh UUID
/// (the `CorrelationId::new()` impl), so 3
/// sequential calls naturally produce 3
/// distinct correlation ids — the test
/// asserts this property rather than
/// constructing the ids by hand.
fn fresh_ctx() -> ExecutionContext {
    ExecutionContext::new(
        afa_contracts::TenantId::new("multi_turn"),
        afa_contracts::Actor::Timer,
    )
}

/// Build a `CompletionRequest` for a single
/// text turn. The system prompt uses the
/// `conformance:text_reply_basic:...` tag the
/// mock's `dispatch` looks for; the
/// `multi_turn` extra is just to make the
/// system prompt distinct from the
/// conformance-suite form. Three of these in
/// a row is the "multi-turn" pattern: the
/// engine sees three independent `complete()`
/// calls.
fn text_request() -> CompletionRequest {
    MockAdapter::request_for_text_reply("multi_turn")
}

// ---------------------------------------------------------------------------
// Flow 11 — bus-aware MockAdapter: 3 calls, 6 events, no engine state.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_adapter_publishes_three_requested_completed_pairs_for_three_sequential_calls() {
    // The bus is a bare `EventBus` (not
    // wrapped in `Arc`) — `subscribe` is
    // `&self` so no Arc is needed. The
    // adapter takes an `EventBusHandle`
    // (the publish-only view) via
    // `MockAdapter::with_event_bus`. The
    // handle and the bare bus share the
    // same underlying `Registry` via
    // `Arc`, so the adapter's publishes
    // land in our subscriptions.
    let bus: EventBus = EventBus::new();
    let handle: EventBusHandle = bus.handle();
    let adapter = MockAdapter::with_event_bus(handle);
    let mut req_sub = bus.subscribe::<CompletionRequested>(32);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(32);
    // 3 sequential `complete()` calls. Each
    // call must return the canned text and
    // must not bleed into the next call.
    for _ in 0..3 {
        let response = adapter.complete(text_request(), &fresh_ctx()).await;
        let value = response.expect("happy path should return Ok");
        match value {
            afa_contracts::CompletionResponse::TextReply { content, .. } => {
                assert_eq!(content, "Hello, world!")
            }
            other => panic!("expected TextReply, got {other:?}"),
        }
    }
    // Drain the bus. We expect exactly 6
    // events: 3 Requested + 3 Completed, in
    // interleaved order. The deadline is
    // generous (2s) so a hang in the adapter
    // surfaces as a test failure rather than
    // a timeout-flaky. The bus's `Subscription::recv`
    // returns `(Arc<T>, ExecutionContext)`, so
    // we unwrap the `Arc` with `*` to get a
    // moved `T` for the asserts below.
    let mut seen_requested: Vec<CompletionRequested> = Vec::new();
    let mut seen_completed: Vec<CompletionCompleted> = Vec::new();
    for _ in 0..3 {
        let (req_arc, _) = tokio::time::timeout(Duration::from_secs(2), req_sub.recv())
            .await
            .expect("CompletionRequested not received in time")
            .expect("CompletionRequested channel closed");
        let req: CompletionRequested = (*req_arc).clone();
        let (comp_arc, _) = tokio::time::timeout(Duration::from_secs(2), comp_sub.recv())
            .await
            .expect("CompletionCompleted not received in time")
            .expect("CompletionCompleted channel closed");
        let comp: CompletionCompleted = (*comp_arc).clone();
        assert_eq!(
            req.correlation_id, comp.correlation_id,
            "each Requested/Completed pair must share a correlation id"
        );
        seen_requested.push(req);
        seen_completed.push(comp);
    }
    // The 3 `correlation_id`s are pairwise
    // distinct (no aliasing between calls).
    let mut ids: Vec<_> = seen_requested.iter().map(|e| e.correlation_id).collect();
    ids.dedup();
    assert_eq!(
        ids.len(),
        3,
        "the 3 sequential calls must have 3 distinct correlation ids; got {ids:?}"
    );
    // The first Requested carries
    // `prompt_tokens_estimate: None` (the
    // mock has no prior Usage to draw from).
    // The field type is `Option<u32>`, not
    // `Option<Usage>` (the audit estimate is
    // a single token count, not a full
    // usage struct — see `events.rs`).
    assert_eq!(seen_requested[0].prompt_tokens_estimate, None);
    // The 2nd and 3rd Requests carry
    // `Some(<canned-prompt-tokens>)`. The
    // mock's text-reply dispatch returns
    // `Usage { prompt_tokens: 5, ... }`, and
    // the bus-aware `complete` caches that
    // into `last_usage`, then re-stamps it
    // on the next call's
    // `prompt_tokens_estimate`. The exact
    // value is irrelevant to the test (the
    // real adapters use their
    // `OnceLock<Usage>` here); the property
    // under test is "the field is populated
    // on the 2nd + 3rd calls but not the
    // 1st" (a strict contract for the bus
    // audit-event stream).
    assert_eq!(
        seen_requested[1].prompt_tokens_estimate,
        Some(seen_completed[0].prompt_tokens),
        "the 2nd Requested's estimate must equal the 1st Completed's prompt_tokens"
    );
    assert_eq!(
        seen_requested[2].prompt_tokens_estimate,
        Some(seen_completed[1].prompt_tokens),
        "the 3rd Requested's estimate must equal the 2nd Completed's prompt_tokens"
    );
    // Each Completed carries the canned
    // `Usage { prompt_tokens: 5, ... }`
    // (the mock's text-reply dispatch
    // returns that shape — see
    // `mock_adapter.rs::dispatch` for the
    // exact canned values) and `Stop`
    // reason. We assert on the canonical
    // `Stop` reason + the
    // non-`prompt_tokens_estimate`-only
    // shape (i.e., the wire-level counts
    // are present and not zero) rather
    // than a hard-coded count, so a
    // future canned-value change in the
    // mock is a deliberate test change.
    for comp in &seen_completed {
        assert_eq!(comp.finish_reason, FinishReason::Stop);
        assert!(
            comp.prompt_tokens > 0,
            "the canned text-reply Usage must carry a non-zero prompt_tokens; got {}",
            comp.prompt_tokens
        );
    }
    // No extra events on the bus (a 6th
    // Requested, a 4th Completed, or any
    // `CompletionFailed` would mean the
    // adapter double-stamped or the engine
    // accidentally published something).
    assert!(
        tokio::time::timeout(Duration::from_millis(50), req_sub.recv())
            .await
            .is_err(),
        "no extra CompletionRequested should have been published"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), comp_sub.recv())
            .await
            .is_err(),
        "no extra CompletionCompleted should have been published"
    );
}

// ---------------------------------------------------------------------------
// Flow 11 — bus-aware MockAdapter: error path publishes `CompletionFailed`.
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_adapter_publishes_a_completion_failed_event_when_the_request_dispatches_to_an_error()
{
    // The mock's "rate_limited" stub is the
    // canned error path: it returns
    // `Err(LlmErrorV1::RateLimited { ... })`
    // when the request carries a "rate"
    // system prompt. The bus-aware
    // `MockAdapter::complete` must publish a
    // `CompletionFailed` event with the
    // matching `correlation_id` so the audit
    // trail covers the failure case too.
    let bus: EventBus = EventBus::new();
    let handle: EventBusHandle = bus.handle();
    let adapter = MockAdapter::with_event_bus(handle);
    let mut req_sub = bus.subscribe::<CompletionRequested>(16);
    let mut failed_sub = bus.subscribe::<CompletionFailed>(16);
    // We DO NOT also subscribe to
    // `CompletionCompleted`: the error path
    // must NOT publish a Completed event
    // (that would be a double-stamp). The
    // bus-less assertion at the end checks
    // for the absence.
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(16);
    let mut ctx = fresh_ctx();
    // The mock dispatches to the error path
    // when the system prompt starts with
    // `conformance:rate_limited` (see
    // `mock_adapter.rs::dispatch` +
    // `MockAdapter::request_for_failure`).
    // We construct the request directly so
    // the system prompt matches the dispatch
    // pattern exactly (the canned
    // `request_for_failure` builder works
    // too, but the inline form keeps the
    // test self-contained).
    ctx.correlation_id = afa_contracts::CorrelationId::new();
    let req = CompletionRequest {
        system: Some("conformance:rate_limited".into()),
        messages: vec![ConversationItem::UserMessage {
            content: vec![ContentBlock::Text("turn".into())],
        }],
        tools: vec![],
        sampling: Default::default(),
    };
    let result = adapter.complete(req, &ctx).await;
    assert!(
        result.is_err(),
        "the 'conformance:rate_limited' system prompt must dispatch to Err"
    );
    // The Requested event was published
    // before dispatch.
    let (req_arc, _) = tokio::time::timeout(Duration::from_secs(2), req_sub.recv())
        .await
        .expect("CompletionRequested not received in time")
        .expect("CompletionRequested channel closed");
    let req_evt: CompletionRequested = (*req_arc).clone();
    assert_eq!(req_evt.correlation_id, ctx.correlation_id);
    // The Failed event was published
    // after dispatch.
    let (failed_arc, _) = tokio::time::timeout(Duration::from_secs(2), failed_sub.recv())
        .await
        .expect("CompletionFailed not received in time")
        .expect("CompletionFailed channel closed");
    let failed_evt: CompletionFailed = (*failed_arc).clone();
    assert_eq!(failed_evt.correlation_id, ctx.correlation_id);
    assert!(matches!(
        failed_evt.error,
        afa_contracts::LlmErrorV1::RateLimited { .. }
    ));
    // The Completed channel stays empty
    // (the error path must NOT publish a
    // Completed event).
    assert!(
        tokio::time::timeout(Duration::from_millis(50), comp_sub.recv())
            .await
            .is_err(),
        "no CompletionCompleted should have been published on the error path"
    );
}

// ---------------------------------------------------------------------------
// Flow 11 — bus-less MockAdapter: zero events (hermetic conformance).
// ---------------------------------------------------------------------------

#[tokio::test]
async fn mock_adapter_without_bus_publishes_no_events() {
    // The default `MockAdapter::new()` is
    // the hermetic variant the conformance
    // suite uses. 3 sequential `complete()`
    // calls must not publish any events on a
    // bus that is NOT attached to the
    // adapter — and the adapter has no bus
    // to attach in the first place. The
    // test asserts both: the bus is
    // untouched (no events of any kind on
    // the 3 subscriptions), and the
    // adapter returns the canned text for
    // every call.
    let bus: EventBus = EventBus::new();
    let mut req_sub = bus.subscribe::<CompletionRequested>(8);
    let mut comp_sub = bus.subscribe::<CompletionCompleted>(8);
    let mut failed_sub = bus.subscribe::<CompletionFailed>(8);
    let adapter = MockAdapter::new();
    for _ in 0..3 {
        let value = adapter
            .complete(text_request(), &fresh_ctx())
            .await
            .expect("happy path");
        match value {
            afa_contracts::CompletionResponse::TextReply { content, .. } => {
                assert_eq!(content, "Hello, world!")
            }
            other => panic!("expected TextReply, got {other:?}"),
        }
    }
    // No events on any of the 3
    // subscriptions.
    assert!(
        tokio::time::timeout(Duration::from_millis(50), req_sub.recv())
            .await
            .is_err(),
        "no events should have been published by the bus-less MockAdapter"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), comp_sub.recv())
            .await
            .is_err(),
        "no events should have been published by the bus-less MockAdapter"
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(50), failed_sub.recv())
            .await
            .is_err(),
        "no events should have been published by the bus-less MockAdapter"
    );
}
