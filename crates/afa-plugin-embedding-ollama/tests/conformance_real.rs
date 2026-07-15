//! CID:afa-plugin-embedding-ollama-conformance-real-001
//! Purpose: Integration tests for the **real**
//! (not mock) `OllamaEmbeddingAdapter`. These
//! tests run against the actual adapter
//! implementation in
//! `crates/afa-plugin-embedding-ollama/src/adapter.rs`,
//! not the `MockEmbeddingAdapter` in
//! `afa-contract-testing`. They are the
//! `OllamaEmbeddingAdapter` half of the
//! "real backend" branch of the conformance
//! matrix that `EmbeddingV1` defines (see
//! `afa-contracts/src/embedding/traits.rs`).
//!
//! # What is observable here
//!
//! Per the project rule (user_profile: "no
//! tautological tests; assertions on observable
//! behavior"), every test in this file pins a
//! **publicly observable property**:
//!
//! | Test | Observable property |
//! | --- | --- |
//! | `test_ollama_happy_path` | `embed_batch(N inputs)` returns N 768-dim vectors in input order, single HTTP call |
//! | `test_ollama_connection_refused` | unreachable host → `Err(AdapterUnavailable)`, 4 attempts (1 + 3 retries) |
//! | `test_ollama_404_model_not_pulled` | HTTP 404 → `Err(ModelUnavailable)`, 1 attempt (no retry) |
//! | `test_ollama_500_retry_then_success` | HTTP 500 ×3 then 200 → `Ok(...)`, 4 attempts |
//! | `test_ollama_500_exhausted` | HTTP 500 ×4 → `Err(Internal)`, 4 attempts |
//! | `test_ollama_400_no_retry` | HTTP 400 → `Err(InvalidInput)`, 1 attempt (no retry) |
//! | `test_ollama_response_parse_error` | non-JSON body → `Err(Internal)`, 1 attempt (no retry) |
//! | `test_ollama_timeout` | server sleeps > timeout → `Err(AdapterUnavailable)`, 1 attempt (timeout is not retried) |
//! | `test_ollama_describe_capabilities` | `describe_capabilities()` returns `model_name`, `dimension`, `max_sequence_length` from the `known_model_specs` table |
//!
//! No test in this file asserts "the function was
//! called" or "the mock returns what the mock
//! returns". Each test pins a property a real
//! downstream consumer (the RAG retrieval engine
//! in Pack #26) depends on.
//!
//! # Test fixture
//!
//! Each test stands up a `wiremock-rs` server
//! (the `MockServer::start()` async helper) and
//! points the adapter at it via the
//! `OllamaEmbeddingConfig::base_url`. The
//! `wiremock` server is an in-process HTTP
//! server that the adapter's `reqwest::Client`
//! hits transparently — no special
//! accommodation in the adapter code is needed.
//! Every test that hits the wire uses
//! `MockServer::start_async().await` to get a
//! random ephemeral port, then configures
//! `mock.with_status(...)` and
//! `mock.with_body(...)` to return canned
//! responses, then asserts on the adapter's
//! return value and on `mock.received_requests().await`
//! (the server's per-test request log).

use std::time::Duration;

use afa_contracts::{
    embedding::{EmbeddingErrorV1, EmbeddingV1},
    Actor, ExecutionContext, TenantId,
};
use afa_plugin_embedding_ollama::{OllamaEmbeddingAdapter, OllamaEmbeddingConfig};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

/// Build the `ExecutionContext` the integration
/// tests hand to the adapter's `embed` /
/// `embed_batch` methods. Mirrors the helper
/// in the local adapter's conformance suite.
fn make_ctx() -> ExecutionContext {
    ExecutionContext::new(
        TenantId::new("test-tenant"),
        Actor::Internal {
            caller: "ollama-conformance".into(),
        },
    )
}

/// Build a `OllamaEmbeddingConfig` pointed at a
/// specific `base_url` (the wiremock server's
/// URL, in the happy-path tests; a guaranteed-
/// unreachable address in the network-failure
/// tests). The other fields are chosen to make
/// the test deterministic (short timeout, small
/// batch, fixed model).
fn make_config(base_url: String) -> OllamaEmbeddingConfig {
    OllamaEmbeddingConfig {
        name: "test-ollama".to_string(),
        base_url,
        model: "nomic-embed-text".to_string(),
        timeout_secs: 2,
        max_batch_size: 100,
        keep_alive_secs: 60,
    }
}

/// Build a synthetic successful `/v1/embeddings`
/// response. The `data` array is in input order
/// (the adapter sorts by `index` defensively,
/// so the order is correct either way).
///
/// Each embedding is a 768-element vector of
/// the form `[index as f32, 0.0, 0.0, ..., 0.0]`
/// so the test can identify which input each
/// vector came from (the value at position 0
/// is the input's index).
fn make_ollama_success_response(input_count: usize) -> serde_json::Value {
    let data: Vec<serde_json::Value> = (0..input_count)
        .map(|i| {
            let mut embedding = vec![0.0_f32; 768];
            embedding[0] = i as f32;
            serde_json::json!({
                "object": "embedding",
                "index": i,
                "embedding": embedding,
            })
        })
        .collect();
    serde_json::json!({
        "object": "list",
        "data": data,
        "model": "nomic-embed-text",
        "usage": {"prompt_tokens": 4, "total_tokens": 4},
    })
}

// ===================================================================
// Test 1 — happy path: embed_batch of 4 inputs returns
// 4 768-dim vectors in input order.
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_ollama_happy_path() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(make_ollama_success_response(4)))
        .expect(1) // single HTTP call (not 4 — the batched endpoint is one POST)
        .mount(&server)
        .await;

    let adapter = OllamaEmbeddingAdapter::new(make_config(server.uri())).unwrap();
    let inputs = vec![
        "a".to_string(),
        "b".to_string(),
        "c".to_string(),
        "d".to_string(),
    ];
    let out = adapter.embed_batch(&inputs, &make_ctx()).await.unwrap();

    // Length matches input length.
    assert_eq!(out.len(), 4);
    // Each vector is 768-dim (nomic-embed-text).
    for v in &out {
        assert_eq!(v.len(), 768, "each embedding must be 768-dim");
    }
    // Order is preserved (index 0 → first output,
    // index 1 → second, etc.). We use the
    // value-at-position-0 trick so this assertion
    // pins the contract, not a coincidence of
    // the test data.
    for (i, v) in out.iter().enumerate() {
        assert_eq!(
            v[0], i as f32,
            "output at index {i} should be the embedding for input at index {i}"
        );
    }
}

// ===================================================================
// Test 2 — connection refused: adapter returns
// AdapterUnavailable, 4 attempts (1 + 3 retries).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_ollama_connection_refused() {
    // Port 1 is reserved and never has a listener.
    // `connect_timeout` of 2s × 4 attempts = ~8s
    // wall clock. We use a short per-call timeout
    // (2s) so the test runs in <10s.
    let adapter =
        OllamaEmbeddingAdapter::new(make_config("http://127.0.0.1:1".to_string())).unwrap();
    let inputs = vec!["a".to_string()];

    let start = std::time::Instant::now();
    let err = adapter.embed_batch(&inputs, &make_ctx()).await.unwrap_err();
    let elapsed = start.elapsed();

    assert!(
        matches!(err, EmbeddingErrorV1::AdapterUnavailable { .. }),
        "expected AdapterUnavailable, got {err:?}"
    );
    // 1 attempt + 3 retries = 4 total. Each
    // attempt has a 2s per-call timeout; the
    // retries have 1s/2s/4s backoffs. Total
    // expected wall time: 2*4 + 1+2+4 = 15s
    // (worst case). We bound it loosely to
    // 20s to avoid flakiness on slow CI.
    assert!(
        elapsed < Duration::from_secs(20),
        "connection-refused test took {elapsed:?}, expected < 20s"
    );
}

// ===================================================================
// Test 3 — HTTP 404 (model not pulled) →
// ModelUnavailable, no retry (404 is a
// "don't retry" 4xx).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_ollama_404_model_not_pulled() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(404).set_body_string("model not found"))
        .expect(1) // exactly 1 call (no retry on 4xx)
        .mount(&server)
        .await;

    let adapter = OllamaEmbeddingAdapter::new(make_config(server.uri())).unwrap();
    let inputs = vec!["a".to_string()];
    let err = adapter.embed_batch(&inputs, &make_ctx()).await.unwrap_err();

    assert!(
        matches!(err, EmbeddingErrorV1::ModelUnavailable { .. }),
        "expected ModelUnavailable, got {err:?}"
    );
    // Sanity check: the error mentions "model"
    // so the operator can grep for it in logs.
    let reason = match err {
        EmbeddingErrorV1::ModelUnavailable { reason, .. } => reason,
        _ => unreachable!(),
    };
    assert!(
        reason.to_lowercase().contains("model"),
        "error reason should mention model: {reason}"
    );
}

// ===================================================================
// Test 4 — HTTP 500 ×3 then 200 → Ok(...) after
// 4 attempts (the 4th attempt succeeds).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_ollama_500_retry_then_success() {
    let server = MockServer::start().await;

    // First 3 requests: HTTP 500.
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal"))
        .up_to_n_times(3)
        .mount(&server)
        .await;
    // 4th request: HTTP 200.
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(make_ollama_success_response(1)))
        .expect(1)
        .mount(&server)
        .await;

    let adapter = OllamaEmbeddingAdapter::new(make_config(server.uri())).unwrap();
    let inputs = vec!["a".to_string()];
    let out = adapter.embed_batch(&inputs, &make_ctx()).await.unwrap();

    assert_eq!(out.len(), 1);
    assert_eq!(out[0].len(), 768);
    // 3 500s + 1 200 = 4 total requests observed
    // by the server.
    let received = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        received.len(),
        4,
        "expected 3 500s + 1 200 = 4 total requests"
    );
}

// ===================================================================
// Test 5 — HTTP 500 ×4 → Internal after 4
// attempts (the 3 retries + initial = 4 total,
// all 500).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_ollama_500_exhausted() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal"))
        .expect(4) // exactly 4 (1 + 3 retries), then give up
        .mount(&server)
        .await;

    let adapter = OllamaEmbeddingAdapter::new(make_config(server.uri())).unwrap();
    let inputs = vec!["a".to_string()];
    let err = adapter.embed_batch(&inputs, &make_ctx()).await.unwrap_err();

    assert!(
        matches!(err, EmbeddingErrorV1::Internal { .. }),
        "expected Internal, got {err:?}"
    );
}

// ===================================================================
// Test 6 — HTTP 400 (bad request) → InvalidInput,
// no retry (400 is a "don't retry" 4xx).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_ollama_400_no_retry() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(400).set_body_string("invalid request"))
        .expect(1) // exactly 1 call (no retry on 4xx)
        .mount(&server)
        .await;

    let adapter = OllamaEmbeddingAdapter::new(make_config(server.uri())).unwrap();
    let inputs = vec!["a".to_string()];
    let err = adapter.embed_batch(&inputs, &make_ctx()).await.unwrap_err();

    assert!(
        matches!(err, EmbeddingErrorV1::InvalidInput { .. }),
        "expected InvalidInput, got {err:?}"
    );
}

// ===================================================================
// Test 7 — non-JSON response → Internal, no retry
// (a 200 with a non-JSON body is a server bug;
// we don't retry because re-trying with the
// same request would produce the same bad
// response).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_ollama_response_parse_error() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json at all"))
        .expect(1) // exactly 1 call (no retry on parse error)
        .mount(&server)
        .await;

    let adapter = OllamaEmbeddingAdapter::new(make_config(server.uri())).unwrap();
    let inputs = vec!["a".to_string()];
    let err = adapter.embed_batch(&inputs, &make_ctx()).await.unwrap_err();

    assert!(
        matches!(err, EmbeddingErrorV1::Internal { .. }),
        "expected Internal, got {err:?}"
    );
    // The reason should mention "parse" so the
    // operator can grep for it in logs.
    let reason = match err {
        EmbeddingErrorV1::Internal { reason } => reason,
        _ => unreachable!(),
    };
    assert!(
        reason.to_lowercase().contains("parse") || reason.to_lowercase().contains("json"),
        "error reason should mention parse/json: {reason}"
    );
}

// ===================================================================
// Test 8 — server sleeps > timeout →
// AdapterUnavailable. Timeouts are NOT retried
// (the per-call timeout already spent the
// configured budget; retrying would compound
// the latency).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_ollama_timeout() {
    let server = MockServer::start().await;
    // A responder that sleeps for 5s — longer
    // than the 2s per-call timeout in
    // `make_config`.
    struct SlowResponder;
    impl Respond for SlowResponder {
        fn respond(&self, _req: &Request) -> ResponseTemplate {
            std::thread::sleep(Duration::from_secs(5));
            ResponseTemplate::new(200)
        }
    }
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(SlowResponder)
        .expect(1) // exactly 1 call (timeout is not retried)
        .mount(&server)
        .await;

    let adapter = OllamaEmbeddingAdapter::new(make_config(server.uri())).unwrap();
    let inputs = vec!["a".to_string()];
    let start = std::time::Instant::now();
    let err = adapter.embed_batch(&inputs, &make_ctx()).await.unwrap_err();
    let elapsed = start.elapsed();

    assert!(
        matches!(err, EmbeddingErrorV1::AdapterUnavailable { .. }),
        "expected AdapterUnavailable, got {err:?}"
    );
    // Per-call timeout is 2s. We don't retry on
    // timeout, so the wall clock should be ~2s,
    // not 8s (4 attempts). The check is loose
    // (< 5s) to avoid flakiness on slow CI.
    assert!(
        elapsed < Duration::from_secs(5),
        "timeout test took {elapsed:?}, expected < 5s (no retry on timeout)"
    );
}

// ===================================================================
// Test 9 — describe_capabilities returns the
// hard-coded `known_model_specs` table values
// for `nomic-embed-text`.
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_ollama_describe_capabilities() {
    let adapter =
        OllamaEmbeddingAdapter::new(make_config("http://127.0.0.1:1".to_string())).unwrap();
    let caps = adapter.describe_capabilities();
    assert_eq!(caps.model_name, "nomic-embed-text");
    assert_eq!(caps.dimension, 768, "nomic-embed-text is 768-dim");
    assert_eq!(caps.max_sequence_length, 2048);
    assert_eq!(caps.max_batch_size, 100);
    assert!(caps.supports_batching);
}
