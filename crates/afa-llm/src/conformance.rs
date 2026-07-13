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
    use super::*;

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
}
