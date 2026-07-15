//! Embedding conformance suite.
//!
//! Phase 3 upgrades this file from a Phase 0
//! skeleton-only score sheet into a
//! behavior-focused contract suite. The mock
//! adapter always runs. The two real adapters are
//! opt-in via features so CI can enable them only
//! when the right fixture/setup exists:
//! - `real-local-conformance`
//! - `real-ollama-conformance`

use std::sync::Arc;

use afa_contracts::error::{AfaError, AfaErrorKind};
use afa_contracts::{EmbeddingErrorV1, EmbeddingV1, EmbeddingV1Version};

use super::mock::MockEmbeddingAdapter;

#[cfg(feature = "real-local-conformance")]
use afa_plugin_embedding_local::{
    DownloadStrategy, LocalEmbeddingAdapter, LocalEmbeddingConfig, OfflineMode,
};
#[cfg(feature = "real-local-conformance")]
use std::{env, path::PathBuf};
#[cfg(feature = "real-local-conformance")]
use tempfile::TempDir;

#[cfg(feature = "real-ollama-conformance")]
use afa_plugin_embedding_ollama::{OllamaEmbeddingAdapter, OllamaEmbeddingConfig};
#[cfg(feature = "real-ollama-conformance")]
use wiremock::matchers::{method, path};
#[cfg(feature = "real-ollama-conformance")]
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

#[cfg(feature = "real-ollama-conformance")]
fn deterministic_embedding(text: &str, dimension: usize) -> Vec<f32> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(dimension);
    let mut state = 0xcbf2_9ce4_8422_2325_u64;

    for i in 0..dimension {
        for &b in bytes {
            state ^= (b as u64) ^ (i as u64);
            state = state.wrapping_mul(0x0000_0100_0000_01b3);
            state = state.rotate_left(5);
        }
        let unit = (state as f64 / u64::MAX as f64) as f32;
        out.push((unit * 2.0) - 1.0);
    }

    let norm = out.iter().map(|v| v * v).sum::<f32>().sqrt();
    if norm > 0.0 {
        for value in &mut out {
            *value /= norm;
        }
    }
    out
}

fn ctx() -> afa_contracts::ExecutionContext {
    crate::fixtures::test_execution_context("acme-realty")
}

async fn assert_embed_returns_correct_dimension(adapter: &dyn EmbeddingV1) {
    let caps = adapter.describe_capabilities();
    let vector = adapter
        .embed("hello world", &ctx())
        .await
        .expect("embed should succeed for non-empty input");
    assert_eq!(
        vector.len(),
        caps.dimension as usize,
        "embed output length must match describe_capabilities().dimension"
    );
}

async fn assert_embed_is_deterministic(adapter: &dyn EmbeddingV1) {
    let v1 = adapter
        .embed("same input", &ctx())
        .await
        .expect("first embed should succeed");
    let v2 = adapter
        .embed("same input", &ctx())
        .await
        .expect("second embed should succeed");
    assert_eq!(v1, v2, "same input must produce same embedding");
}

async fn assert_embed_different_inputs_different_outputs(adapter: &dyn EmbeddingV1) {
    let v1 = adapter
        .embed("alpha input", &ctx())
        .await
        .expect("first embed should succeed");
    let v2 = adapter
        .embed("beta input", &ctx())
        .await
        .expect("second embed should succeed");
    assert_ne!(
        v1, v2,
        "different inputs must not collapse to same embedding"
    );
}

async fn assert_embed_batch_returns_parallel_output(adapter: &dyn EmbeddingV1) {
    let texts = vec!["one".to_string(), "two".to_string(), "three".to_string()];
    let vectors = adapter
        .embed_batch(&texts, &ctx())
        .await
        .expect("embed_batch should succeed");
    assert_eq!(
        vectors.len(),
        texts.len(),
        "embed_batch must return one vector per input"
    );
    let caps = adapter.describe_capabilities();
    for vector in &vectors {
        assert_eq!(
            vector.len(),
            caps.dimension as usize,
            "each batch item must match reported dimension"
        );
    }
}

async fn assert_embed_matches_embed_batch_single(adapter: &dyn EmbeddingV1) {
    let text = "single input";
    let single = adapter
        .embed(text, &ctx())
        .await
        .expect("embed should succeed");
    let batch = adapter
        .embed_batch(&[text.to_string()], &ctx())
        .await
        .expect("single-item batch should succeed");
    assert_eq!(
        batch.len(),
        1,
        "single-item batch must return exactly one vector"
    );
    assert_eq!(
        single, batch[0],
        "embed(text) must equal embed_batch([text])[0]"
    );
}

async fn assert_embed_empty_input_rejected(adapter: &dyn EmbeddingV1) {
    let err = adapter
        .embed("", &ctx())
        .await
        .expect_err("empty input must fail");
    assert!(
        matches!(err, EmbeddingErrorV1::InvalidInput { .. }),
        "empty input must map to InvalidInput, got {err:?}"
    );
}

async fn assert_embed_whitespace_input_rejected(adapter: &dyn EmbeddingV1) {
    let err = adapter
        .embed("   \n\t  ", &ctx())
        .await
        .expect_err("whitespace-only input must fail");
    assert!(
        matches!(err, EmbeddingErrorV1::InvalidInput { .. }),
        "whitespace-only input must map to InvalidInput, got {err:?}"
    );
}

async fn assert_embed_batch_empty_returns_empty(adapter: &dyn EmbeddingV1) {
    let vectors = adapter
        .embed_batch(&[], &ctx())
        .await
        .expect("empty batch should succeed");
    assert!(vectors.is_empty(), "empty batch must return empty output");
}

async fn assert_concurrent_embed_batch(adapter: Arc<dyn EmbeddingV1>) {
    let mut handles = Vec::new();
    for idx in 0..8 {
        let adapter = adapter.clone();
        handles.push(tokio::spawn(async move {
            let batch = vec![format!("text {idx}"), format!("text {idx}b")];
            adapter.embed_batch(&batch, &ctx()).await
        }));
    }

    for handle in handles {
        let vectors = handle
            .await
            .expect("join handle should succeed")
            .expect("embed_batch should succeed");
        assert_eq!(
            vectors.len(),
            2,
            "each concurrent batch should return two vectors"
        );
    }
}

fn assert_describe_capabilities_contract(adapter: &dyn EmbeddingV1) {
    let caps1 = adapter.describe_capabilities();
    let caps2 = adapter.describe_capabilities();
    assert_eq!(
        caps1, caps2,
        "describe_capabilities must be deterministic and have no I/O"
    );
    assert!(caps1.dimension > 0, "dimension must be positive");
    assert!(caps1.max_batch_size > 0, "max_batch_size must be positive");
    assert!(
        caps1.max_sequence_length > 0,
        "max_sequence_length must be positive"
    );
    assert!(
        caps1.supports_batching,
        "v1 adapters should report batching support"
    );
}

fn assert_error_kind_mapping() {
    let cases = [
        (
            EmbeddingErrorV1::AdapterUnavailable {
                reason: "adapter down".to_string(),
            },
            AfaErrorKind::Unavailable,
        ),
        (
            EmbeddingErrorV1::ModelUnavailable {
                model_name: "model".to_string(),
                reason: "missing".to_string(),
            },
            AfaErrorKind::Unavailable,
        ),
        (
            EmbeddingErrorV1::InvalidInput {
                reason: "bad input".to_string(),
            },
            AfaErrorKind::CapabilityUnsupported,
        ),
        (
            EmbeddingErrorV1::Internal {
                reason: "bug".to_string(),
            },
            AfaErrorKind::Internal,
        ),
    ];

    for (error, expected) in cases {
        assert_eq!(
            error.kind(),
            expected,
            "error kind mapping drifted for {error:?}"
        );
    }
}

fn assert_arc_dyn_safety<T>(adapter: T)
where
    T: EmbeddingV1,
{
    let erased: Arc<dyn EmbeddingV1> = Arc::new(adapter);
    let caps = erased.describe_capabilities();
    assert!(
        caps.dimension > 0,
        "trait object must remain usable behind Arc<dyn EmbeddingV1>"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn mock_embed_returns_correct_dimension() {
    let adapter = MockEmbeddingAdapter::default();
    assert_embed_returns_correct_dimension(&adapter).await;
}

#[tokio::test(flavor = "current_thread")]
async fn mock_embed_is_deterministic() {
    let adapter = MockEmbeddingAdapter::default();
    assert_embed_is_deterministic(&adapter).await;
}

#[tokio::test(flavor = "current_thread")]
async fn mock_embed_different_inputs_different_outputs() {
    let adapter = MockEmbeddingAdapter::default();
    assert_embed_different_inputs_different_outputs(&adapter).await;
}

#[tokio::test(flavor = "current_thread")]
async fn mock_embed_batch_returns_parallel_output() {
    let adapter = MockEmbeddingAdapter::default();
    assert_embed_batch_returns_parallel_output(&adapter).await;
}

#[tokio::test(flavor = "current_thread")]
async fn mock_embed_matches_embed_batch_single() {
    let adapter = MockEmbeddingAdapter::default();
    assert_embed_matches_embed_batch_single(&adapter).await;
}

#[tokio::test(flavor = "current_thread")]
async fn mock_embed_empty_input_rejected() {
    let adapter = MockEmbeddingAdapter::default();
    assert_embed_empty_input_rejected(&adapter).await;
}

#[tokio::test(flavor = "current_thread")]
async fn mock_embed_whitespace_input_rejected() {
    let adapter = MockEmbeddingAdapter::default();
    assert_embed_whitespace_input_rejected(&adapter).await;
}

#[tokio::test(flavor = "current_thread")]
async fn mock_embed_batch_empty_returns_empty() {
    let adapter = MockEmbeddingAdapter::default();
    assert_embed_batch_empty_returns_empty(&adapter).await;
}

#[tokio::test(flavor = "current_thread")]
async fn mock_concurrent_embed_batch_calls_succeed() {
    let adapter: Arc<dyn EmbeddingV1> = Arc::new(MockEmbeddingAdapter::default());
    assert_concurrent_embed_batch(adapter).await;
}

#[test]
fn mock_describe_capabilities_contract() {
    let adapter = MockEmbeddingAdapter::default();
    assert_describe_capabilities_contract(&adapter);
}

#[test]
fn embedding_error_kind_mapping_is_locked() {
    assert_error_kind_mapping();
}

#[test]
fn embedding_trait_is_object_safe() {
    assert_arc_dyn_safety(MockEmbeddingAdapter::default());
}

#[test]
fn embedding_v1_version_is_locked() {
    assert_eq!(
        EmbeddingV1Version, "1.0.0",
        "contract version is ADR-locked"
    );
}

#[cfg(feature = "real-local-conformance")]
fn make_local_fixture_dir() -> PathBuf {
    env::var("AFA_EMBEDDING_LOCAL_FIXTURE_DIR")
        .map(PathBuf::from)
        .expect(
            "real-local-conformance requires AFA_EMBEDDING_LOCAL_FIXTURE_DIR pointing at the base dir that contains all-MiniLM-L6-v2/",
        )
}

#[cfg(feature = "real-local-conformance")]
fn make_local_adapter() -> LocalEmbeddingAdapter {
    LocalEmbeddingAdapter::new(LocalEmbeddingConfig {
        model_name: "all-MiniLM-L6-v2".to_string(),
        model_dir: make_local_fixture_dir(),
        offline_mode: OfflineMode::Strict,
        download_strategy: DownloadStrategy::Never,
    })
    .expect("real local fixture should construct adapter")
}

#[cfg(feature = "real-local-conformance")]
fn make_missing_local_dir() -> TempDir {
    tempfile::tempdir().expect("tempdir")
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_strict_missing_model_returns_model_unavailable() {
    let dir = make_missing_local_dir();
    let result = LocalEmbeddingAdapter::new(LocalEmbeddingConfig {
        model_name: "all-MiniLM-L6-v2".to_string(),
        model_dir: dir.path().to_path_buf(),
        offline_mode: OfflineMode::Strict,
        download_strategy: DownloadStrategy::Never,
    });

    // `LocalEmbeddingAdapter` does not implement `Debug`,
    // so we match on the result instead of using
    // `expect_err` (which would require `T: Debug`).
    let err = match result {
        Ok(_) => panic!("strict missing model must fail"),
        Err(e) => e,
    };

    assert!(
        matches!(err, EmbeddingErrorV1::ModelUnavailable { .. }),
        "strict missing model must return ModelUnavailable, got {err:?}"
    );
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_degraded_missing_model_returns_zero_vector() {
    let dir = make_missing_local_dir();
    let adapter = LocalEmbeddingAdapter::new(LocalEmbeddingConfig {
        model_name: "all-MiniLM-L6-v2".to_string(),
        model_dir: dir.path().to_path_buf(),
        offline_mode: OfflineMode::Degraded,
        download_strategy: DownloadStrategy::Never,
    })
    .expect("degraded missing model should still construct");

    let vector = adapter
        .embed("hello world", &ctx())
        .await
        .expect("degraded mode should return sentinel vector");
    assert_eq!(
        vector.len(),
        384,
        "degraded vector must still match advertised dimension"
    );
    assert!(
        vector.iter().all(|value| *value == 0.0),
        "degraded vector must be all zeros"
    );
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_embed_returns_correct_dimension() {
    let adapter = make_local_adapter();
    assert_embed_returns_correct_dimension(&adapter).await;
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_embed_is_deterministic() {
    let adapter = make_local_adapter();
    assert_embed_is_deterministic(&adapter).await;
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_embed_different_inputs_different_outputs() {
    let adapter = make_local_adapter();
    assert_embed_different_inputs_different_outputs(&adapter).await;
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_embed_batch_returns_parallel_output() {
    let adapter = make_local_adapter();
    assert_embed_batch_returns_parallel_output(&adapter).await;
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_embed_matches_embed_batch_single() {
    let adapter = make_local_adapter();
    assert_embed_matches_embed_batch_single(&adapter).await;
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_embed_empty_input_rejected() {
    let adapter = make_local_adapter();
    assert_embed_empty_input_rejected(&adapter).await;
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_embed_whitespace_input_rejected() {
    let adapter = make_local_adapter();
    assert_embed_whitespace_input_rejected(&adapter).await;
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_embed_batch_empty_returns_empty() {
    let adapter = make_local_adapter();
    assert_embed_batch_empty_returns_empty(&adapter).await;
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn local_concurrent_embed_batch_calls_succeed() {
    let adapter: Arc<dyn EmbeddingV1> = Arc::new(make_local_adapter());
    assert_concurrent_embed_batch(adapter).await;
}

#[cfg(feature = "real-local-conformance")]
#[test]
fn local_describe_capabilities_contract() {
    let adapter = make_local_adapter();
    assert_describe_capabilities_contract(&adapter);
}

#[cfg(feature = "real-local-conformance")]
#[test]
fn local_trait_object_is_safe() {
    assert_arc_dyn_safety(make_local_adapter());
}

#[cfg(feature = "real-local-conformance")]
#[tokio::test(flavor = "current_thread")]
#[ignore = "performance smoke: requires real local fixture and stable CI CPU lane"]
async fn local_embed_batch_smoke_faster_than_loop() {
    let adapter = make_local_adapter();
    let texts: Vec<String> = (0..32)
        .map(|idx| format!("performance sample {idx}"))
        .collect();

    let loop_start = std::time::Instant::now();
    for text in &texts {
        adapter
            .embed(text, &ctx())
            .await
            .expect("single embed should succeed");
    }
    let loop_elapsed = loop_start.elapsed();

    let batch_start = std::time::Instant::now();
    adapter
        .embed_batch(&texts, &ctx())
        .await
        .expect("batch embed should succeed");
    let batch_elapsed = batch_start.elapsed();

    assert!(
        batch_elapsed < loop_elapsed,
        "batch embed ({batch_elapsed:?}) should beat per-item loop ({loop_elapsed:?})"
    );
}

#[cfg(feature = "real-ollama-conformance")]
fn make_ollama_config(base_url: String) -> OllamaEmbeddingConfig {
    OllamaEmbeddingConfig {
        name: "test-ollama".to_string(),
        base_url,
        model: "nomic-embed-text".to_string(),
        timeout_secs: 2,
        max_batch_size: 100,
        keep_alive_secs: 60,
    }
}

#[cfg(feature = "real-ollama-conformance")]
struct EchoResponder;

#[cfg(feature = "real-ollama-conformance")]
impl Respond for EchoResponder {
    fn respond(&self, request: &Request) -> ResponseTemplate {
        let body: serde_json::Value =
            serde_json::from_slice(&request.body).expect("request body should be valid json");
        let inputs = body["input"]
            .as_array()
            .expect("input should be array")
            .iter()
            .map(|value| value.as_str().expect("input item should be string"))
            .collect::<Vec<_>>();

        let data = inputs
            .iter()
            .enumerate()
            .map(|(idx, text)| {
                serde_json::json!({
                    "object": "embedding",
                    "index": idx,
                    "embedding": deterministic_embedding(text, 768),
                })
            })
            .collect::<Vec<_>>();

        ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "object": "list",
            "data": data,
            "model": "nomic-embed-text",
        }))
    }
}

#[cfg(feature = "real-ollama-conformance")]
async fn make_ollama_adapter() -> (OllamaEmbeddingAdapter, MockServer) {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(EchoResponder)
        .mount(&server)
        .await;

    (
        OllamaEmbeddingAdapter::new(make_ollama_config(server.uri()))
            .expect("wiremock-backed ollama adapter should construct"),
        server,
    )
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_embed_returns_correct_dimension() {
    let (adapter, _server) = make_ollama_adapter().await;
    assert_embed_returns_correct_dimension(&adapter).await;
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_embed_is_deterministic() {
    let (adapter, _server) = make_ollama_adapter().await;
    assert_embed_is_deterministic(&adapter).await;
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_embed_different_inputs_different_outputs() {
    let (adapter, _server) = make_ollama_adapter().await;
    assert_embed_different_inputs_different_outputs(&adapter).await;
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_embed_batch_returns_parallel_output() {
    let (adapter, _server) = make_ollama_adapter().await;
    assert_embed_batch_returns_parallel_output(&adapter).await;
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_embed_matches_embed_batch_single() {
    let (adapter, _server) = make_ollama_adapter().await;
    assert_embed_matches_embed_batch_single(&adapter).await;
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_embed_empty_input_rejected() {
    let (adapter, _server) = make_ollama_adapter().await;
    assert_embed_empty_input_rejected(&adapter).await;
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_embed_whitespace_input_rejected() {
    let (adapter, _server) = make_ollama_adapter().await;
    assert_embed_whitespace_input_rejected(&adapter).await;
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_embed_batch_empty_returns_empty() {
    let (adapter, _server) = make_ollama_adapter().await;
    assert_embed_batch_empty_returns_empty(&adapter).await;
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_concurrent_embed_batch_calls_succeed() {
    let (adapter, _server) = make_ollama_adapter().await;
    let adapter: Arc<dyn EmbeddingV1> = Arc::new(adapter);
    assert_concurrent_embed_batch(adapter).await;
}

#[cfg(feature = "real-ollama-conformance")]
#[test]
fn ollama_describe_capabilities_contract() {
    let adapter = OllamaEmbeddingAdapter::new(make_ollama_config("http://127.0.0.1:1".to_string()))
        .expect("adapter should construct");
    assert_describe_capabilities_contract(&adapter);
}

#[cfg(feature = "real-ollama-conformance")]
#[test]
fn ollama_trait_object_is_safe() {
    let adapter = OllamaEmbeddingAdapter::new(make_ollama_config("http://127.0.0.1:1".to_string()))
        .expect("adapter should construct");
    assert_arc_dyn_safety(adapter);
}

#[cfg(feature = "real-ollama-conformance")]
#[tokio::test(flavor = "current_thread")]
async fn ollama_retries_500_then_succeeds() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(500).set_body_string("internal"))
        .up_to_n_times(3)
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(EchoResponder)
        .mount(&server)
        .await;

    let adapter = OllamaEmbeddingAdapter::new(make_ollama_config(server.uri()))
        .expect("adapter should construct");
    let vectors = adapter
        .embed_batch(&["hello world".to_string()], &ctx())
        .await
        .expect("adapter should eventually recover after retryable 500s");
    assert_eq!(
        vectors.len(),
        1,
        "successful retry path should still return one vector"
    );
    let requests = server.received_requests().await.expect("request log");
    assert_eq!(
        requests.len(),
        4,
        "expected 1 initial call + 3 retries before success"
    );
}
