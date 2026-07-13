//! Code Map: Conformance suite
//! - `MockAdapter`: A canned-script
//!   `KnowledgeV1` implementation the
//!   conformance suite drives. The
//!   mock is the canonical "what
//!   every adapter must do" test
//!   oracle. It returns scripted
//!   results for `find_information`
//!   and `list_topics`, and
//!   deterministic UUIDs + `Ok(())`
//!   for `store_information`. The
//!   mock also records every call
//!   so the test cases can assert
//!   on the call log.
//! - `run_conformance_suite`: The
//!   contract-conformance entry
//!   point. The suite is
//!   adapter-agnostic — it runs
//!   against any `Arc<dyn
//!   KnowledgeV1>` (the JSON plugin
//!   is exercised in
//!   `afa-plugin-knowledge-json/tests/`,
//!   not here).
//!
//! Story (plain English): The
//! conformance suite is the safety
//! net. A new storage adapter (JSON
//! today, Postgres tomorrow) plugs
//! in by implementing `KnowledgeV1`
//! and passing the suite. The suite
//! answers one question: "does this
//! adapter honor the contract we
//! promised the workflows?" The
//! `MockAdapter` is the simplest
//! possible `KnowledgeV1` impl —
//! canned responses, no file system —
//! so the suite can be run in the
//! `afa-knowledge` crate's own
//! `tests/` dir without depending
//! on the JSON plugin.
//!
//! CID Index:
//! CID:afa-knowledge-conformance-001 -> run_conformance_suite
//! CID:afa-knowledge-conformance-mock-001 -> MockAdapter
//!
//! Quick lookup: rg -n "CID:afa-knowledge-conformance-" crates/afa-knowledge/src/conformance.rs

use std::sync::{Arc, Mutex};

use afa_contracts::{
    ExecutionContext, FindInformationRequest, FindInformationResponse, KnowledgeCapabilities,
    KnowledgeErrorV1, KnowledgeRecordInput, KnowledgeV1, RecordId, Topic,
};
use async_trait::async_trait;

// CID:afa-knowledge-conformance-mock-001 - MockAdapter
// Purpose: A canned-script
// `KnowledgeV1` implementation. The
// mock is the canonical "what every
// adapter must do" test oracle. It
// returns scripted results for
// `find_information` and
// `list_topics`, and deterministic
// UUIDs + `Ok(())` for
// `store_information`. The mock also
// records every call so the test
// cases can assert on the call log
// (e.g., "the adapter called
// `store_information` exactly twice
// after these two
// `store_information` requests").
//
// The mock has a per-method script:
// - `store_script`: a queue of
//   `Result<RecordId,
//   KnowledgeErrorV1>` responses
//   (the mock pops one per call; the
//   last entry is reused if the
//   queue is exhausted).
// - `find_script`: a queue of
//   `Result<FindInformationResponse,
//   KnowledgeErrorV1>` responses.
// - `list_script`: a queue of
//   `Result<Vec<Topic>,
//   KnowledgeErrorV1>` responses.
//
// The defaults (a single-entry
// success script) yield the
// "always succeeds" behavior:
// `store_information` returns
// `Ok(RecordId::new())`;
// `find_information` returns
// `Ok(Vec::new())`; `list_topics`
// returns `Ok(Vec::new())`. The
// tests construct a `MockAdapter`
// with a non-default script to
// exercise the error and result
// paths.
pub struct MockAdapter {
    store_script: Mutex<Vec<Result<RecordId, KnowledgeErrorV1>>>,
    find_script: Mutex<Vec<Result<FindInformationResponse, KnowledgeErrorV1>>>,
    list_script: Mutex<Vec<Result<Vec<Topic>, KnowledgeErrorV1>>>,
    /// The cap the adapter reports
    /// (the conformance suite asserts
    /// on the reported shape).
    capabilities: KnowledgeCapabilities,
    /// The call log (the conformance
    /// suite asserts on the call
    /// shape).
    pub call_log: Mutex<Vec<MockCall>>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum MockCall {
    Store(KnowledgeRecordInput),
    Find(FindInformationRequest),
    List,
}

impl Default for MockAdapter {
    fn default() -> Self {
        Self {
            store_script: Mutex::new(vec![Ok(RecordId::new())]),
            find_script: Mutex::new(vec![Ok(Vec::new())]),
            list_script: Mutex::new(vec![Ok(Vec::new())]),
            capabilities: KnowledgeCapabilities {
                max_record_size_bytes: 1_048_576,
                supports_semantic_search: false,
                supports_hierarchical_topics: false,
            },
            call_log: Mutex::new(Vec::new()),
        }
    }
}

impl MockAdapter {
    /// Build a mock that always
    /// returns the success shape.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a mock with a custom
    /// capability shape (so the
    /// conformance suite can assert
    /// on the reported shape).
    pub fn with_capabilities(capabilities: KnowledgeCapabilities) -> Self {
        Self {
            capabilities,
            ..Self::default()
        }
    }

    /// Replace the store script. The
    /// mock will pop one entry per
    /// `store_information` call (the
    /// last entry is reused if the
    /// queue is exhausted).
    pub fn set_store_script(&self, script: Vec<Result<RecordId, KnowledgeErrorV1>>) {
        *self.store_script.lock().expect("lock") = script;
    }

    /// Replace the find script.
    pub fn set_find_script(&self, script: Vec<Result<FindInformationResponse, KnowledgeErrorV1>>) {
        *self.find_script.lock().expect("lock") = script;
    }

    /// Replace the list script.
    pub fn set_list_script(&self, script: Vec<Result<Vec<Topic>, KnowledgeErrorV1>>) {
        *self.list_script.lock().expect("lock") = script;
    }

    /// Returns a snapshot of the call
    /// log. The conformance suite
    /// asserts on the call log to
    /// catch "the adapter is silently
    /// no-op'ing" regressions.
    pub fn call_log(&self) -> Vec<MockCall> {
        self.call_log.lock().expect("lock").clone()
    }
}

/// Helper: pop the next script
/// entry, cloning the last entry so
/// subsequent calls get the same
/// shape (the script is a "queue
/// with sticky tail" — pop one,
/// refill with the same value).
/// The script is
/// `Vec<Result<T, E>>`; the helper
/// returns the `Result` (the caller
/// `?`-unwraps or pattern-matches
/// as needed).
fn pop_script_entry<T: Clone, E: Clone>(script: &mut Vec<Result<T, E>>) -> Result<T, E> {
    if script.is_empty() {
        panic!("script is empty (test bug: caller should populate the script first)");
    }
    let entry = script.remove(0);
    if script.is_empty() {
        script.push(entry.clone());
    }
    entry
}

#[async_trait]
impl KnowledgeV1 for MockAdapter {
    async fn find_information(
        &self,
        request: FindInformationRequest,
        _ctx: &ExecutionContext,
    ) -> Result<FindInformationResponse, KnowledgeErrorV1> {
        self.call_log
            .lock()
            .expect("lock")
            .push(MockCall::Find(request));
        let mut script = self.find_script.lock().expect("lock");
        pop_script_entry(&mut script)
    }

    async fn store_information(
        &self,
        record: KnowledgeRecordInput,
        _ctx: &ExecutionContext,
    ) -> Result<RecordId, KnowledgeErrorV1> {
        self.call_log
            .lock()
            .expect("lock")
            .push(MockCall::Store(record));
        let mut script = self.store_script.lock().expect("lock");
        pop_script_entry(&mut script)
    }

    async fn list_topics(&self, _ctx: &ExecutionContext) -> Result<Vec<Topic>, KnowledgeErrorV1> {
        self.call_log.lock().expect("lock").push(MockCall::List);
        let mut script = self.list_script.lock().expect("lock");
        pop_script_entry(&mut script)
    }

    fn describe_capabilities(&self) -> KnowledgeCapabilities {
        self.capabilities.clone()
    }
}

// CID:afa-knowledge-conformance-001 - run_conformance_suite
// Purpose: The contract-conformance
// entry point. The signature is
// locked: one `Arc<dyn KnowledgeV1>`
// in, no return value. The Phase 2
// body exercises the happy path on
// every method (a no-op
// `store_information`, a
// `find_information` that returns
// the canned result, a
// `list_topics` that returns the
// canned result). The full
// per-method cases (oversized
// content, empty topic, etc.) land
// in Phase 3 when the
// `MockAdapter`'s call-log
// assertion path is fully wired.
pub async fn run_conformance_suite(adapter: Arc<dyn KnowledgeV1>) {
    let ctx = ExecutionContext::new(
        afa_contracts::ids::TenantId::new("c"),
        afa_contracts::execution_context::Actor::Timer,
    );
    // store_information happy path.
    let _ = adapter
        .store_information(
            KnowledgeRecordInput {
                topic: "FAQ".to_string(),
                tags: vec![],
                content: "conformance hello".to_string(),
                source: None,
            },
            &ctx,
        )
        .await
        .expect("conformance: store_information happy path");
    // find_information happy path.
    let _ = adapter
        .find_information(FindInformationRequest::default(), &ctx)
        .await
        .expect("conformance: find_information happy path");
    // list_topics happy path.
    let _ = adapter
        .list_topics(&ctx)
        .await
        .expect("conformance: list_topics happy path");
    // describe_capabilities.
    let _ = adapter.describe_capabilities();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_store_information_default_returns_ok() {
        // The default mock
        // returns
        // `Ok(RecordId::new())`
        // for every
        // `store_information`
        // call.
        let m: Arc<dyn KnowledgeV1> = Arc::new(MockAdapter::new());
        let ctx = ExecutionContext::new(
            afa_contracts::ids::TenantId::new("c"),
            afa_contracts::execution_context::Actor::Timer,
        );
        let r = m
            .store_information(
                KnowledgeRecordInput {
                    topic: "FAQ".to_string(),
                    tags: vec![],
                    content: "x".to_string(),
                    source: None,
                },
                &ctx,
            )
            .await;
        assert!(r.is_ok());
    }

    #[tokio::test]
    async fn mock_store_information_script_returns_scripted_error() {
        // A mock with a
        // scripted
        // `InvalidInput` error
        // returns that error
        // on the first call.
        let mock = MockAdapter::new();
        mock.set_store_script(vec![Err(KnowledgeErrorV1::InvalidInput {
            topic: Some("FAQ".to_string()),
            record_id: None,
            reason: "scripted".to_string(),
        })]);
        let m: Arc<dyn KnowledgeV1> = Arc::new(mock);
        let ctx = ExecutionContext::new(
            afa_contracts::ids::TenantId::new("c"),
            afa_contracts::execution_context::Actor::Timer,
        );
        let r = m
            .store_information(
                KnowledgeRecordInput {
                    topic: "FAQ".to_string(),
                    tags: vec![],
                    content: "x".to_string(),
                    source: None,
                },
                &ctx,
            )
            .await;
        assert!(matches!(r, Err(KnowledgeErrorV1::InvalidInput { .. })));
    }

    #[tokio::test]
    async fn mock_call_log_records_every_call() {
        // The call log
        // records every
        // method call so
        // the conformance
        // suite can
        // assert on
        // "exactly N
        // calls".
        let m = MockAdapter::new();
        let ctx = ExecutionContext::new(
            afa_contracts::ids::TenantId::new("c"),
            afa_contracts::execution_context::Actor::Timer,
        );
        m.store_information(
            KnowledgeRecordInput {
                topic: "FAQ".to_string(),
                tags: vec![],
                content: "x".to_string(),
                source: None,
            },
            &ctx,
        )
        .await
        .unwrap();
        m.list_topics(&ctx).await.unwrap();
        let log = m.call_log();
        assert_eq!(log.len(), 2);
        assert!(matches!(log[0], MockCall::Store(_)));
        assert!(matches!(log[1], MockCall::List));
    }

    #[tokio::test]
    async fn conformance_suite_runs_against_mock() {
        // The Phase 2
        // happy-path
        // conformance
        // suite: it must
        // run against the
        // `MockAdapter`
        // without
        // panicking.
        let m: Arc<dyn KnowledgeV1> = Arc::new(MockAdapter::new());
        run_conformance_suite(m).await;
    }
}
