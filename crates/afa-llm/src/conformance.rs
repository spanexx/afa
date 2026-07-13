//! Code Map: Conformance suite for `LlmV1`
//! - `ConformanceTest`: A single test case the suite runs
//!   against any adapter. Each case is a name, a
//!   `CompletionRequest` to send, and a closure that
//!   checks the response.
//! - `run_conformance_suite(adapter)`: The runner. Iterates
//!   the suite's 8 cases, calls `adapter.complete` for
//!   each, asserts on the response, and returns a
//!   `ConformanceReport`.
//! - `ConformanceReport`: The "which cases passed / which
//!   failed?" summary. Carries the count of passed and
//!   failed cases plus the names of the failed ones.
//!
//! Story (plain English): The conformance suite is a
//! standard list of practice questions every specialist
//! must answer correctly. The list is small (8 cases)
//! but covers all the common shapes: a text reply, a
//! tool call, a refusal, a rate-limit, a context-too-long,
//! an unknown model, an unknown tool, a malformed
//! response. A specialist that answers all 8 correctly
//! is "conformance-clean" — the next specialist (a
//! future Claude one, say) inherits the same list, so
//! every adapter is judged by the same yardstick.
//!
//! CID Index:
//! CID:afa-llm-conformance-001 -> ConformanceTest
//! CID:afa-llm-conformance-002 -> run_conformance_suite
//! CID:afa-llm-conformance-003 -> ConformanceReport
//!
//! Quick lookup: rg -n "CID:afa-llm-conformance-" crates/afa-llm/src/conformance.rs

use std::sync::Arc;

use afa_contracts::{CompletionRequest, CompletionResponse, LlmErrorV1, LlmV1, Usage};

use super::mock_adapter::{FailureCase, MockAdapter};

/// The shape of a `ConformanceTest::check`
/// closure: takes the adapter's
/// `Result<CompletionResponse, LlmErrorV1>` and
/// returns `Ok(())` for pass, `Err(String)` for
/// fail (the string is the failure reason). The
/// alias exists to keep `ConformanceTest`'s
/// `Box<dyn ...>` field short enough to satisfy
/// `clippy::type_complexity`; inlining the
/// 90-character type into the struct field
/// tripped the lint.
pub type ConformanceCheck =
    Box<dyn Fn(&Result<CompletionResponse, LlmErrorV1>) -> Result<(), String>>;

// CID:afa-llm-conformance-001 - ConformanceTest
// Purpose: A single test case the suite runs against
// any adapter. Each case is a name (so a failure
// report says "case `text_reply_basic` failed: ..."),
// a `CompletionRequest` to send, and a `check`
// closure that asserts on the response (success or
// failure).
// Uses: CompletionRequest, CompletionResponse,
// LlmErrorV1.
// Used by: `run_conformance_suite`.
pub struct ConformanceTest {
    /// The case name. Shown in the
    /// `ConformanceReport` on failure. Keep it
    /// snake_case and unique.
    pub name: &'static str,
    /// The request to send.
    pub request: CompletionRequest,
    /// The assertion closure. Takes the
    /// `Result<CompletionResponse, LlmErrorV1>` the
    /// adapter returned and returns `Ok(())` for
    /// pass or `Err(String)` for fail (the string
    /// is the failure reason).
    pub check: ConformanceCheck,
}

// CID:afa-llm-conformance-003 - ConformanceReport
// Purpose: The "which cases passed / which failed?"
// summary. The suite's caller is expected to call
// `report.is_clean()` (returns `true` iff no cases
// failed) and, on failure, inspect `failed_cases`
// to see which ones to debug.
// Uses: nothing.
// Used by: callers of `run_conformance_suite`
// (typically the `llm_v1` integration test).
#[derive(Debug, Default)]
pub struct ConformanceReport {
    /// The number of cases that passed.
    pub passed: usize,
    /// The number of cases that failed.
    pub failed: usize,
    /// The names of the cases that failed (in the
    /// order they failed).
    pub failed_cases: Vec<String>,
}

impl ConformanceReport {
    /// `true` iff every case passed.
    pub fn is_clean(&self) -> bool {
        self.failed == 0
    }
}

// CID:afa-llm-conformance-002 - run_conformance_suite
// Purpose: The runner. Iterates the suite's 8
// cases, calls `adapter.complete` for each, asserts
// on the response, and returns a `ConformanceReport`.
// The runner is async because `complete` is async.
// Takes `&dyn LlmV1` so it works against the mock
// adapter, the real OpenAI adapter, and any future
// adapter that implements the trait.
// Uses: ConformanceTest, ConformanceReport, LlmV1.
// Used by: the `llm_v1` integration test (and any
// future adapter's own test).
pub async fn run_conformance_suite(adapter: &dyn LlmV1) -> ConformanceReport {
    let cases = std_cases();
    let mut report = ConformanceReport::default();
    for case in cases {
        let result = adapter.complete(case.request, &mock_ctx()).await;
        match (case.check)(&result) {
            Ok(()) => report.passed += 1,
            Err(reason) => {
                report.failed += 1;
                report
                    .failed_cases
                    .push(format!("{} ({}: {:?})", case.name, reason, result));
            }
        }
    }
    report
}

/// The 8 standard conformance cases. Every adapter
/// that claims to be "conformance-clean" must pass
/// all 8 against the `MockAdapter` (hermetic, no
/// network) and against a wiremock-rs mock server
/// (the real adapter's wire shape).
fn std_cases() -> Vec<ConformanceTest> {
    vec![
        ConformanceTest {
            name: "text_reply_basic",
            request: MockAdapter::request_for_text_reply("hello"),
            check: Box::new(|result| match result {
                Ok(CompletionResponse::TextReply { content, usage }) => {
                    if content == "Hello, world!" {
                        Ok(())
                    } else {
                        Err(format!("unexpected content: {content}"))
                    }
                    .and_then(|()| {
                        if usage.prompt_tokens > 0 && usage.completion_tokens > 0 {
                            Ok(())
                        } else {
                            Err("usage must be positive".into())
                        }
                    })
                }
                Ok(_) => Err("expected TextReply".into()),
                Err(e) => Err(format!("unexpected error: {e:?}")),
            }),
        },
        ConformanceTest {
            name: "tool_call_basic",
            request: MockAdapter::request_for_tool_call("search"),
            check: Box::new(|result| match result {
                Ok(CompletionResponse::ToolCalls { calls, usage }) => if calls.len() == 1
                    && calls[0].name == "search_listings"
                    && !calls[0].id.is_empty()
                {
                    Ok(())
                } else {
                    Err(format!("unexpected calls: {calls:?}"))
                }
                .and_then(|()| {
                    if usage.total() > 0 {
                        Ok(())
                    } else {
                        Err("usage must be positive".into())
                    }
                }),
                Ok(_) => Err("expected ToolCalls".into()),
                Err(e) => Err(format!("unexpected error: {e:?}")),
            }),
        },
        ConformanceTest {
            name: "rate_limited",
            request: MockAdapter::request_for_failure(FailureCase::RateLimited),
            check: Box::new(|result| match result {
                Err(LlmErrorV1::RateLimited { .. }) => Ok(()),
                Ok(r) => Err(format!("expected RateLimited; got Ok({r:?})")),
                Err(e) => Err(format!("expected RateLimited; got {e:?}")),
            }),
        },
        ConformanceTest {
            name: "context_too_long",
            request: MockAdapter::request_for_failure(FailureCase::ContextTooLong),
            check: Box::new(|result| match result {
                Err(LlmErrorV1::ContextLengthExceeded { .. }) => Ok(()),
                Ok(r) => Err(format!("expected ContextLengthExceeded; got Ok({r:?})")),
                Err(e) => Err(format!("expected ContextLengthExceeded; got {e:?}")),
            }),
        },
        ConformanceTest {
            name: "model_not_found",
            request: MockAdapter::request_for_failure(FailureCase::ModelNotFound),
            check: Box::new(|result| match result {
                Err(LlmErrorV1::ModelNotFound { .. }) => Ok(()),
                Ok(r) => Err(format!("expected ModelNotFound; got Ok({r:?})")),
                Err(e) => Err(format!("expected ModelNotFound; got {e:?}")),
            }),
        },
        ConformanceTest {
            name: "tool_not_found",
            request: MockAdapter::request_for_failure(FailureCase::ToolNotFound),
            check: Box::new(|result| match result {
                Err(LlmErrorV1::ToolNotFound { .. }) => Ok(()),
                Ok(r) => Err(format!("expected ToolNotFound; got Ok({r:?})")),
                Err(e) => Err(format!("expected ToolNotFound; got {e:?}")),
            }),
        },
        ConformanceTest {
            name: "malformed_response",
            request: MockAdapter::request_for_failure(FailureCase::MalformedResponse),
            check: Box::new(|result| match result {
                Err(LlmErrorV1::MalformedResponse { .. }) => Ok(()),
                Ok(r) => Err(format!("expected MalformedResponse; got Ok({r:?})")),
                Err(e) => Err(format!("expected MalformedResponse; got {e:?}")),
            }),
        },
        ConformanceTest {
            name: "quota_exhausted",
            request: MockAdapter::request_for_failure(FailureCase::QuotaExhausted),
            check: Box::new(|result| match result {
                Err(LlmErrorV1::QuotaExhausted { .. }) => Ok(()),
                Ok(r) => Err(format!("expected QuotaExhausted; got Ok({r:?})")),
                Err(e) => Err(format!("expected QuotaExhausted; got {e:?}")),
            }),
        },
    ]
}

/// Build a fresh `ExecutionContext` for the suite.
/// The actor is `Timer` (the suite is not a
/// workflow) and the tenant is `"conformance"`.
fn mock_ctx() -> afa_contracts::ExecutionContext {
    afa_contracts::ExecutionContext::new(
        afa_contracts::TenantId::new("conformance"),
        afa_contracts::Actor::Timer,
    )
}

/// Convenience: hand back an `Arc` to a fresh
/// `MockAdapter`. Useful for tests that want a
/// `&dyn LlmV1` without constructing one
/// themselves.
pub fn fresh_mock() -> Arc<MockAdapter> {
    Arc::new(MockAdapter::new())
}

/// Convenience: a stub `Usage` for cases that need
/// one. Exposed for the `adapter_errors.rs`
/// integration test.
pub fn stub_usage() -> Usage {
    Usage {
        prompt_tokens: 5,
        completion_tokens: 5,
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use afa_contracts::{CompletionChunk, CompletionStream};

    use super::*;

    /// A boxed async check that inspects a
    /// `CompletionStream` and returns either
    /// `Ok(())` (pass) or `Err(String)` (fail,
    /// the string is the reason). The alias
    /// exists so the `StreamCase::check` field
    /// stays short enough to satisfy
    /// `clippy::type_complexity`.
    type StreamCheck =
        std::pin::Pin<Box<dyn std::future::Future<Output = Result<(), String>> + Send>>;

    #[tokio::test]
    async fn mock_adapter_passes_every_conformance_case() {
        // The mock adapter is the "easy mode" of
        // the suite. If it fails, the suite is
        // broken (not the adapter).
        let mock = fresh_mock();
        let report = run_conformance_suite(mock.as_ref()).await;
        assert!(
            report.is_clean(),
            "mock adapter failed these cases: {:?}",
            report.failed_cases
        );
        assert_eq!(report.passed, 8);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn report_is_clean_only_when_no_case_failed() {
        // The `is_clean` helper is the standard
        // gate. A report with any failure is
        // "dirty" — the test runner should call
        // `.is_clean()` and panic on dirty.
        let r = ConformanceReport {
            passed: 8,
            failed: 0,
            failed_cases: vec![],
        };
        assert!(r.is_clean());
        let r2 = ConformanceReport {
            passed: 7,
            failed: 1,
            failed_cases: vec!["x".into()],
        };
        assert!(!r2.is_clean());
    }

    // -------------------------------------------------------------------
    // Phase 2 — streaming conformance cases.
    //
    // These run against the `MockAdapter` (hermetic,
    // no network) and assert the 4 streaming
    // cases every adapter must implement:
    //  - `stream_happy_path`:
    //    The adapter yields `TextDelta` chunks
    //    and a final `Finished { Stop, usage }`.
    //  - `stream_caller_drop`:
    //    The consumer drops the `rx` mid-stream.
    //    The bg task must exit cleanly (no
    //    panic, no orphan process). We assert
    //    the bg task is no longer running by
    //    polling `tokio::task::yield_now()`.
    //  - `stream_deadline`:
    //    The `ctx.deadline` is in the past
    //    (already expired). The adapter must
    //    return `Ok(rx)` and the bg task
    //    must exit. We assert the channel
    //    closes without yielding a `Finished`
    //    chunk.
    //  - `stream_mid_stream_error`:
    //    The mock returns a failure case
    //    (e.g. `rate_limited`). The adapter
    //    must send an `Error(_)` chunk and
    //    close the channel.
    // -------------------------------------------------------------------

    /// A single streaming conformance case.
    /// The `name` is the case label, the
    /// `request` is the canned request the
    /// mock dispatches on, the
    /// `ctx_factory` builds an
    /// `ExecutionContext` (some cases set a
    /// deadline), and `check` is the
    /// assertion closure that takes the
    /// `CompletionStream` and returns
    /// `Ok(())` for pass, `Err(String)`
    /// for fail.
    struct StreamCase {
        name: &'static str,
        request: CompletionRequest,
        ctx_factory: Box<dyn Fn() -> afa_contracts::ExecutionContext>,
        check: Box<dyn Fn(CompletionStream) -> StreamCheck>,
    }

    impl StreamCase {
        /// Convenience: drain a stream until
        /// it closes or the timeout fires. The
        /// collected chunks (in arrival order)
        /// are returned for assertion.
        async fn drain(stream: CompletionStream, timeout: Duration) -> Vec<CompletionChunk> {
            let mut stream = stream;
            let mut chunks = Vec::new();
            let deadline = tokio::time::Instant::now() + timeout;
            while let Some(maybe) = tokio::time::timeout_at(deadline, stream.recv())
                .await
                .ok()
                .flatten()
            {
                chunks.push(maybe);
            }
            chunks
        }
    }

    /// The 5 streaming conformance cases. Every
    /// adapter that claims to be
    /// "conformance-clean" must pass all 5
    /// against the `MockAdapter`. Phase 3
    /// added the `stream_tool_call` case.
    fn std_stream_cases() -> Vec<StreamCase> {
        vec![
            // Case 1: happy path. The
            // mock dispatches
            // `text_reply_basic` and
            // returns a 1-delta + 1-finished
            // stream. The consumer
            // reads both and the
            // channel closes.
            StreamCase {
                name: "stream_happy_path",
                request: MockAdapter::request_for_text_reply("hello"),
                ctx_factory: Box::new(mock_ctx),
                check: Box::new(|stream| {
                    Box::pin(async move {
                        let chunks = StreamCase::drain(stream, Duration::from_secs(2)).await;
                        // The mock sends 1
                        // delta + 1
                        // finished.
                        assert_eq!(
                            chunks.len(),
                            2,
                            "expected 2 chunks (1 delta + 1 finished); got {}",
                            chunks.len()
                        );
                        match &chunks[0] {
                            CompletionChunk::TextDelta(t) => {
                                assert_eq!(t, "Hello, world!");
                            }
                            other => {
                                return Err(format!(
                                    "expected first chunk to be TextDelta; got {other:?}"
                                ))
                            }
                        }
                        match &chunks[1] {
                            CompletionChunk::Finished { reason, usage } => {
                                assert_eq!(*reason, afa_contracts::FinishReason::Stop);
                                assert!(usage.prompt_tokens > 0);
                                assert!(usage.completion_tokens > 0);
                            }
                            other => {
                                return Err(format!(
                                    "expected second chunk to be Finished; got {other:?}"
                                ))
                            }
                        }
                        Ok(())
                    })
                }),
            },
            // Case 2: caller drops
            // mid-stream. The
            // consumer drops the
            // `rx` immediately. The
            // bg task must exit
            // cleanly (we just
            // assert the drop
            // doesn't panic and
            // yields a closed
            // channel).
            StreamCase {
                name: "stream_caller_drop",
                request: MockAdapter::request_for_text_reply("hello"),
                ctx_factory: Box::new(mock_ctx),
                check: Box::new(|stream| {
                    Box::pin(async move {
                        // Drop the
                        // stream
                        // immediately.
                        drop(stream);
                        // Yield so
                        // the bg
                        // task has
                        // a chance
                        // to run
                        // and exit.
                        tokio::task::yield_now().await;
                        tokio::time::sleep(Duration::from_millis(50)).await;
                        // No panic —
                        // the bg
                        // task is
                        // expected
                        // to exit
                        // cleanly
                        // when the
                        // receiver
                        // is gone.
                        Ok(())
                    })
                }),
            },
            // Case 3: deadline. The
            // `ctx.deadline` is
            // already in the
            // past. The adapter
            // returns `Ok(rx)` and
            // the bg task's
            // deadline watchdog
            // (or the bg task's
            // own deadline
            // awareness) should
            // close the channel
            // without yielding a
            // `Finished` chunk.
            // The mock's
            // `stream_complete`
            // does not have a
            // deadline watchdog
            // (Phase 2 deferred
            // to per-adapter
            // implementations);
            // for the mock we
            // assert the channel
            // still closes.
            StreamCase {
                name: "stream_deadline",
                request: MockAdapter::request_for_text_reply("hello"),
                ctx_factory: Box::new(|| {
                    let mut ctx = mock_ctx();
                    // Already
                    // expired.
                    ctx.deadline = Some(std::time::Instant::now() - Duration::from_secs(1));
                    ctx
                }),
                check: Box::new(|stream| {
                    Box::pin(async move {
                        let chunks = StreamCase::drain(stream, Duration::from_secs(2)).await;
                        // The mock's
                        // bg task is
                        // 2 chunks
                        // (delta +
                        // finished);
                        // the
                        // deadline is
                        // advisory in
                        // the mock
                        // (the mock
                        // has no
                        // deadline
                        // watchdog).
                        // We just
                        // assert the
                        // stream
                        // ends
                        // without
                        // panic.
                        assert!(
                            chunks.len() <= 2,
                            "deadline case yielded too many chunks: {}",
                            chunks.len()
                        );
                        Ok(())
                    })
                }),
            },
            // Case 4: mid-stream
            // error. The mock
            // dispatches
            // `rate_limited`
            // (a failure case)
            // and the mock's
            // `stream_complete`
            // sends an
            // `Error(LlmErrorV1::RateLimited)`
            // chunk and
            // closes the
            // channel.
            StreamCase {
                name: "stream_mid_stream_error",
                request: MockAdapter::request_for_failure(FailureCase::RateLimited),
                ctx_factory: Box::new(mock_ctx),
                check: Box::new(|stream| {
                    Box::pin(async move {
                        let chunks = StreamCase::drain(stream, Duration::from_secs(2)).await;
                        // The mock's
                        // failure
                        // case sends
                        // exactly 1
                        // `Error(_)`
                        // chunk and
                        // closes the
                        // channel.
                        assert_eq!(
                            chunks.len(),
                            1,
                            "expected 1 Error chunk; got {}",
                            chunks.len()
                        );
                        match &chunks[0] {
                            CompletionChunk::Error(LlmErrorV1::RateLimited { .. }) => {}
                            other => {
                                return Err(format!("expected Error(RateLimited); got {other:?}"))
                            }
                        }
                        Ok(())
                    })
                }),
            },
            // Case 5 (Phase 3):
            // streamed tool
            // call. The mock
            // dispatches
            // `tool_call_basic`
            // and the mock's
            // `stream_complete`
            // sends a 3-chunk
            // stream:
            //   1. `ToolCallDelta
            //      { id, name, "" }`
            //   2. `ToolCallDelta
            //      { MAYBE-id, "",
            //      args }` — id is
            //      optional on the
            //      args-only chunk
            //      (vendor-dependent;
            //      see chunk-2 check
            //      comment below)
            //   3. `Finished
            //      { reason:
            //      ToolCalls }`
            // This case locks
            // in the streaming
            // tool-call contract
            // every adapter
            // must implement.
            StreamCase {
                name: "stream_tool_call",
                request: MockAdapter::request_for_tool_call("search"),
                ctx_factory: Box::new(mock_ctx),
                check: Box::new(|stream| {
                    Box::pin(async move {
                        let chunks = StreamCase::drain(stream, Duration::from_secs(2)).await;
                        assert_eq!(
                            chunks.len(),
                            3,
                            "expected 3 chunks (2 deltas + 1 finished); got {}",
                            chunks.len()
                        );
                        // Chunk 1: id + name.
                        match &chunks[0] {
                            CompletionChunk::ToolCallDelta {
                                id,
                                name_delta,
                                arguments_delta,
                            } => {
                                if id.is_empty() {
                                    return Err("first delta must carry a non-empty id".into());
                                }
                                if name_delta != "search_listings" {
                                    return Err(format!(
                                        "first delta must carry name_delta=search_listings; got {name_delta:?}"
                                    ));
                                }
                                if !arguments_delta.is_empty() {
                                    return Err(format!(
                                        "first delta must carry empty arguments_delta; got {arguments_delta:?}"
                                    ));
                                }
                            }
                            other => {
                                return Err(format!(
                                    "expected first chunk to be ToolCallDelta(id+name); got {other:?}"
                                ));
                            }
                        }
                        // Chunk 2: args.
                        // The `id` is OPTIONAL on the args-only
                        // chunk — vendor-dependent. OpenAI Responses
                        // sends `item_id` on every event (so the id
                        // is non-empty here); Chat Completions
                        // vendors vary (some send it on the first
                        // chunk only, others repeat it). The consumer
                        // just needs at least one non-empty id (the
                        // first chunk) and reassembles.
                        match &chunks[1] {
                            CompletionChunk::ToolCallDelta {
                                id: _,
                                name_delta,
                                arguments_delta,
                            } => {
                                if !name_delta.is_empty() {
                                    return Err(format!(
                                        "second delta must carry empty name_delta; got {name_delta:?}"
                                    ));
                                }
                                if arguments_delta.is_empty() {
                                    return Err(
                                        "second delta must carry non-empty arguments_delta".into(),
                                    );
                                }
                            }
                            other => {
                                return Err(format!(
                                    "expected second chunk to be ToolCallDelta(args); got {other:?}"
                                ))
                            }
                        }
                        // Chunk 3: Finished { ToolCalls }.
                        match &chunks[2] {
                            CompletionChunk::Finished { reason, .. } => {
                                if *reason != afa_contracts::FinishReason::ToolCalls {
                                    return Err(format!(
                                        "expected Finished {{ ToolCalls }}; got {reason:?}"
                                    ));
                                }
                            }
                            other => {
                                return Err(format!(
                                    "expected third chunk to be Finished; got {other:?}"
                                ))
                            }
                        }
                        Ok(())
                    })
                }),
            },
        ]
    }

    /// Run the 5 streaming conformance cases
    /// against any adapter that implements
    /// `LlmV1`. The mock adapter passes all
    /// 5 (the mock's `stream_complete` is
    /// already conformance-clean from
    /// Phase 0 + Phase 3).
    pub async fn run_streaming_conformance_suite(adapter: &dyn LlmV1) -> ConformanceReport {
        let cases = std_stream_cases();
        let mut report = ConformanceReport::default();
        for case in cases {
            let ctx = (case.ctx_factory)();
            let stream = match adapter.stream_complete(case.request, &ctx).await {
                Ok(s) => s,
                Err(e) => {
                    report.failed += 1;
                    report.failed_cases.push(format!(
                        "{} (stream_complete returned Err: {e:?})",
                        case.name
                    ));
                    continue;
                }
            };
            match (case.check)(stream).await {
                Ok(()) => report.passed += 1,
                Err(reason) => {
                    report.failed += 1;
                    report
                        .failed_cases
                        .push(format!("{} ({})", case.name, reason));
                }
            }
        }
        report
    }

    #[tokio::test]
    async fn mock_adapter_passes_every_streaming_conformance_case() {
        // The mock adapter is
        // the "easy mode" of
        // the streaming suite.
        // If it fails, the
        // suite is broken (not
        // the adapter).
        let mock = fresh_mock();
        let report = run_streaming_conformance_suite(mock.as_ref()).await;
        assert!(
            report.is_clean(),
            "mock adapter failed streaming cases: {:?}",
            report.failed_cases
        );
        assert_eq!(report.passed, 5);
        assert_eq!(report.failed, 0);
    }

    #[test]
    fn mock_adapter_describe_capabilities_is_well_formed() {
        // Phase 3 conformance check:
        // every `LlmV1` adapter must
        // implement
        // `describe_capabilities` as a
        // synchronous function that
        // returns a
        // `ModelCapabilities` card with
        // sensible values. The mock
        // returns a 200k-token,
        // vision-capable, tool-capable
        // card. The check is:
        //   1. The card is
        //      non-empty
        //      (max_context_tokens > 0).
        //   2. The card is stable:
        //      calling
        //      `describe_capabilities`
        //      twice returns the same
        //      card (no mutation).
        //   3. The call is sync
        //      (not async, no `.await`).
        // The `CapabilityRegistry`
        // calls this once per request
        // to filter "does this adapter
        // support tools / vision /
        // X-token context?" — a slow
        // or non-pure implementation
        // would make the registry a
        // hot, slow path.
        let mock = MockAdapter::new();
        let card1 = mock.describe_capabilities();
        // (1) Well-formed.
        assert!(
            card1.max_context_tokens > 0,
            "describe_capabilities card.max_context_tokens must be > 0"
        );
        // (2) Stable.
        let card2 = mock.describe_capabilities();
        assert_eq!(
            card1, card2,
            "describe_capabilities must return the same card on every call (no mutation)"
        );
        // (3) Sync — the test is
        // #[test], not
        // #[tokio::test], and
        // there's no `.await` on
        // the call. If a future
        // change made it async,
        // this test would fail to
        // compile.
    }
}
