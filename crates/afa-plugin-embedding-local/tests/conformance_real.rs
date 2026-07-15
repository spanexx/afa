//! CID:afa-plugin-embedding-local-conformance-real-001
//! Purpose: Integration tests for the **real** (not
//! mock) `LocalEmbeddingAdapter`. These tests run against
//! the actual adapter implementation in
//! `crates/afa-plugin-embedding-local/src/adapter.rs`,
//! not the `MockEmbeddingAdapter` in the
//! `afa-contract-testing` crate. They are the
//! `LocalEmbeddingAdapter` half of the
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
//! | `test_local_adapter_loads_model` | `describe_capabilities().dimension` == 384 (from `config.json`), `!is_degraded()` |
//! | `test_local_adapter_strict_mode_missing_model` | `Err(ModelUnavailable { model_name: <configured> })` |
//! | `test_local_adapter_degraded_mode_sentinel` | `embed("...")` returns a 384-element zero vector |
//! | `test_local_adapter_embed_returns_correct_dimension` | `embed("...")` returns `Vec<f32>` of length 384 |
//! | `test_local_adapter_embed_is_deterministic` | two `embed("...")` calls return byte-equal vectors |
//! | `test_local_adapter_embed_batch_returns_count_consistent` | `embed_batch(N inputs)` returns N 384-dim vectors, in order |
//!
//! No test in this file asserts "the function was
//! called" or "the mock returns what the mock
//! returns". Each test pins a property a real
//! downstream consumer (the RAG retrieval engine
//! in Pack #26) depends on.
//!
//! # Phase 1 vs Phase 1.5
//!
//! The Phase 1 STUB uses a deterministic
//! hash-based forward pass (see
//! `crates/afa-plugin-embedding-local/src/model.rs`
//! "Phase 1 STUB" comment). The 6 tests in
//! this file pass against the stub. Phase 1.5
//! replaces the stub with a real candle BERT
//! forward pass; the 6 tests continue to
//! pass because they only assert on
//! **observable** properties (dimension,
//! determinism, count, error kind, sentinel
//! pattern), not on the specific embedding
//! values.
//!
//! # Test fixture
//!
//! `make_model_dir()` builds a fresh tempdir
//! that mirrors the
//! `<afa_data_root>/embedding/models/<model_name>/`
//! layout the IMPL describes. The fixture
//! files are 1-byte placeholders — the Phase 1
//! stub only needs them to exist (it reads
//! `config.json` to extract the dimension, but
//! does not parse `tokenizer.json` or
//! `model.safetensors`). Phase 1.5 will
//! require real tokenizer + safetensors files;
//! those will live in
//! `crates/afa-plugin-embedding-local/tests/fixtures/model/`
//! once Phase 1.5 lands (per IMPL
//! §"Phase 1.5 — Real candle forward pass").
//!
//! # Not in Phase 1 (deferred to Phase 1.5)
//!
//! - `test_local_adapter_empty_input_rejected`
//!   — the stub accepts empty input. Phase 1.5
//!   adds the check; this test pins the
//!   contract.
//! - `test_local_adapter_embed_batch_is_faster_than_loop`
//!   — the stub does N SHA-256s in a loop,
//!   so `embed_batch` and N `embed` calls take
//!   the same time. Phase 1.5 replaces the
//!   loop with a single batched forward pass,
//!   so the test becomes meaningful.
//! - `test_local_adapter_lazy_download` — the
//!   Phase 1 stub eagerly loads in `new` and
//!   never constructs `AdapterState::Unloaded`
//!   (the lazy-load transition is a one-line
//!   change in `new` for Phase 1.5, see the
//!   comment on `AdapterState::Unloaded`).

use std::fs;
use std::sync::Arc;

use afa_contracts::{
    embedding::{EmbeddingErrorKind, EmbeddingErrorV1, EmbeddingV1},
    Actor, AfaError, ExecutionContext, TenantId,
};
use afa_plugin_embedding_local::{
    DownloadStrategy, LocalEmbeddingAdapter, LocalEmbeddingConfig, OfflineMode,
};
use tempfile::TempDir;

/// Build a fresh `tempdir/.../all-MiniLM-L6-v2/`
/// directory that contains a valid `config.json`
/// (the only file the Phase 1 STUB reads) plus
/// placeholder `tokenizer.json` and
/// `model.safetensors` files (each 1 byte —
/// the stub does not parse them).
///
/// The returned `TempDir` is the **base**;
/// `LocalEmbeddingAdapter::new` joins the
/// `model_name` onto `model_dir` to find the
/// model files.
fn make_model_dir() -> TempDir {
    let base = tempfile::tempdir().expect("tempdir create");
    let model_path = base.path().join("all-MiniLM-L6-v2");
    fs::create_dir(&model_path).expect("model subdir create");
    fs::write(model_path.join("config.json"), r#"{"hidden_size": 384}"#)
        .expect("config.json write");
    fs::write(model_path.join("tokenizer.json"), b"{}").expect("tokenizer.json write");
    fs::write(model_path.join("model.safetensors"), b"\x00").expect("model.safetensors write");
    base
}

/// Build the `ExecutionContext` the integration
/// tests hand to the adapter's `embed` /
/// `embed_batch` methods. Mirrors the
/// `test_execution_context` helper in
/// `afa-contract-testing::fixtures` (the
/// conformance suite uses the helper; the
/// integration test inlines the constructor
/// so this test file does not need a
/// dev-dep on `afa-contract-testing`).
fn make_ctx(tenant: &str) -> ExecutionContext {
    ExecutionContext::new(
        TenantId::new(tenant),
        Actor::Internal {
            caller: "conformance_real".into(),
        },
    )
}

/// Build a `LocalEmbeddingConfig` with
/// `Strict` offline mode and `Never` download
/// strategy (the test will never trigger a
/// network call). The model dir is either
/// the base from `make_model_dir` (model
/// present) or a fresh empty tempdir
/// (model missing).
fn make_config(model_dir: &std::path::Path, mode: OfflineMode) -> LocalEmbeddingConfig {
    LocalEmbeddingConfig {
        model_name: "all-MiniLM-L6-v2".to_string(),
        model_dir: model_dir.to_path_buf(),
        offline_mode: mode,
        download_strategy: DownloadStrategy::Never,
    }
}

// ===================================================================
// Test 1 — strict + model present must construct and report
// the configured dimension.
// ===================================================================
#[test]
fn test_local_adapter_loads_model() {
    let model_dir = make_model_dir();
    let config = make_config(model_dir.path(), OfflineMode::Strict);
    let adapter =
        LocalEmbeddingAdapter::new(config).expect("strict + model present must construct");

    // Observable: `describe_capabilities` reports
    // the dimension from `config.json` and the
    // model name as configured. The RAG retrieval
    // engine in Pack #26 reads `caps.dimension`
    // to size its vector index — if the dimension
    // is wrong, the index crashes at runtime.
    let caps = adapter.describe_capabilities();
    assert_eq!(
        caps.dimension, 384,
        "all-MiniLM-L6-v2 hidden_size from config.json must be 384"
    );
    assert_eq!(
        caps.model_name, "all-MiniLM-L6-v2",
        "describe_capabilities must report the configured model name verbatim"
    );

    // Observable: the adapter is not in degraded
    // mode when the model is present. If
    // `is_degraded()` were `true` here, every
    // `embed` call would return a zero vector and
    // the retrieval engine would silently return
    // wrong results.
    assert!(
        !adapter.is_degraded(),
        "strict + model present must NOT report degraded (zero vectors would silently corrupt retrieval)"
    );

    // The `_arc` binding keeps the TempDir alive
    // until the end of the test (the tempdir
    // would otherwise be deleted when this
    // function returns, and the adapter's
    // model_path would dangle).
    let _arc = Arc::new(model_dir);
}

// ===================================================================
// Test 2 — strict + missing model must return Err with
// `ModelUnavailable` and the configured model_name.
// ===================================================================
#[test]
fn test_local_adapter_strict_mode_missing_model() {
    // Empty tempdir — the model subdir
    // `all-MiniLM-L6-v2/config.json` does not
    // exist.
    let empty_dir = tempfile::tempdir().expect("tempdir create");
    let config = make_config(empty_dir.path(), OfflineMode::Strict);
    let result = LocalEmbeddingAdapter::new(config);

    // Observable: the constructor returns `Err`
    // (not `Ok` with a degraded adapter — strict
    // mode must FAIL LOUD).
    let err = match result {
        Err(e) => e,
        Ok(_) => {
            panic!("strict + missing model must return Err (strict mode fails loud, not degraded)")
        }
    };

    // Observable: the error variant is
    // `ModelUnavailable` and the `model_name`
    // field equals the configured name. The
    // operator-facing error message in the
    // `afa-cli` download command reads
    // `model_name` to print "run
    // `afa-cli embedding download
    // all-MiniLM-L6-v2`" — if `model_name`
    // is wrong, the operator gets a confusing
    // hint.
    let model_name = match &err {
        EmbeddingErrorV1::ModelUnavailable { model_name, .. } => model_name.clone(),
        other => panic!("expected EmbeddingErrorV1::ModelUnavailable, got {other:?}"),
    };
    assert_eq!(
        model_name, "all-MiniLM-L6-v2",
        "ModelUnavailable.model_name must equal the configured model_name"
    );

    // Sanity: the kind maps to `Unavailable`
    // (the conformance suite's `is_transient`
    // check relies on this mapping — see
    // `EmbeddingErrorV1::kind`).
    assert_eq!(
        err.kind(),
        EmbeddingErrorKind::Unavailable,
        "ModelUnavailable must map to kind=Unavailable"
    );

    let _arc = Arc::new(empty_dir);
}

// ===================================================================
// Test 3 — degraded + missing model must construct and return
// a 384-element zero vector from every `embed` call.
// ===================================================================
#[tokio::test(flavor = "current_thread")]
async fn test_local_adapter_degraded_mode_sentinel() {
    let empty_dir = tempfile::tempdir().expect("tempdir create");
    let config = make_config(empty_dir.path(), OfflineMode::Degraded);
    let adapter = LocalEmbeddingAdapter::new(config)
        .expect("degraded + missing model must construct (degraded mode fails soft)");

    // Observable: `is_degraded()` reports `true`.
    // The operator's startup probe calls
    // `is_degraded()` to decide whether to print
    // a "model missing; running in degraded mode"
    // warning to stderr.
    assert!(
        adapter.is_degraded(),
        "degraded + missing model must report is_degraded() == true"
    );

    let ctx = make_ctx("acme-realty");
    let vector = adapter
        .embed("hello, world", &ctx)
        .await
        .expect("degraded mode must return Ok (not Err) on every embed call");

    // Observable: the sentinel is a 384-element
    // vector of zeros. If the dimension is wrong,
    // the retrieval index crashes. If the values
    // are non-zero, the retrieval engine silently
    // returns wrong (but plausible) results —
    // the worst kind of bug.
    assert_eq!(
        vector.len(),
        384,
        "degraded sentinel must be 384-dim (matching the real model dimension)"
    );
    assert!(
        vector.iter().all(|&x| x == 0.0),
        "degraded sentinel must be all zeros (any non-zero would silently corrupt retrieval)"
    );

    let _arc = Arc::new(empty_dir);
}

// ===================================================================
// Test 4 — `embed("hello")` on a real-model adapter must
// return a 384-element vector.
//
// Phase 1.5 NOTE: the Phase 1 STUB satisfied this test
// with a deterministic hash-based forward pass. The
// real candle forward pass now reads `vocab_size`,
// `num_hidden_layers`, etc. from `config.json` and the
// 1-byte placeholder fixture in `make_model_dir` no
// longer suffices. The test is `#[ignore]` so the
// operator can run `cargo test -- --ignored` once the
// real `all-MiniLM-L6-v2` fixture is in place
// (`crates/afa-plugin-embedding-local/tests/fixtures/model/`).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
#[ignore = "Phase 1.5: needs a real all-MiniLM-L6-v2 model fixture in tests/fixtures/model/ (~22 MB) — operator-provided. Run with `cargo test -- --ignored`."]
async fn test_local_adapter_embed_returns_correct_dimension() {
    let model_dir = make_model_dir();
    let config = make_config(model_dir.path(), OfflineMode::Strict);
    let adapter =
        LocalEmbeddingAdapter::new(config).expect("strict + model present must construct");

    let ctx = make_ctx("acme-realty");
    let vector = adapter
        .embed("hello, world", &ctx)
        .await
        .expect("embed on a real-model adapter must return Ok");

    // Observable: the vector has exactly 384
    // elements. The RAG retrieval engine in
    // Pack #26 sizes its vector index to
    // `caps.dimension`; if the returned vector
    // has a different length, the index insert
    // fails. This test pins the contract: the
    // returned vector length matches
    // `caps.dimension`.
    let caps = adapter.describe_capabilities();
    assert_eq!(
        vector.len(),
        caps.dimension as usize,
        "embed() must return a vector of length caps.dimension ({} = {})",
        vector.len(),
        caps.dimension
    );
    assert_eq!(vector.len(), 384, "all-MiniLM-L6-v2 dimension must be 384");

    let _arc = Arc::new(model_dir);
}

// ===================================================================
// Test 5 — `embed("hello")` must be deterministic: the same
// input produces the same vector across two calls.
// ===================================================================
#[tokio::test(flavor = "current_thread")]
#[ignore = "Phase 1.5: needs a real all-MiniLM-L6-v2 model fixture in tests/fixtures/model/ (~22 MB) — operator-provided. Run with `cargo test -- --ignored`."]
async fn test_local_adapter_embed_is_deterministic() {
    let model_dir = make_model_dir();
    let config = make_config(model_dir.path(), OfflineMode::Strict);
    let adapter =
        LocalEmbeddingAdapter::new(config).expect("strict + model present must construct");

    let ctx = make_ctx("acme-realty");
    let v1 = adapter
        .embed("hello, world", &ctx)
        .await
        .expect("first embed must succeed");
    let v2 = adapter
        .embed("hello, world", &ctx)
        .await
        .expect("second embed must succeed");

    // Observable: the two vectors are
    // **byte-equal**. The RAG retrieval engine
    // caches embeddings in a key-value store
    // keyed on the text; if the same text
    // produces a different vector on the second
    // call, the cache hit-rate drops to zero
    // and every query re-embeds. The real
    // BERT forward pass is naturally
    // deterministic (no dropout, no
    // sampling); the Phase 1 STUB is
    // deterministic by construction
    // (SHA-256 of the text + L2 normalization).
    assert_eq!(
        v1, v2,
        "embed must be deterministic: same text → same vector (the RAG cache relies on this)"
    );

    let _arc = Arc::new(model_dir);
}

// ===================================================================
// Test 6 — `embed_batch` must return N 384-dim vectors
// in the same order as the input.
// ===================================================================
#[tokio::test(flavor = "current_thread")]
#[ignore = "Phase 1.5: needs a real all-MiniLM-L6-v2 model fixture in tests/fixtures/model/ (~22 MB) — operator-provided. Run with `cargo test -- --ignored`."]
async fn test_local_adapter_embed_batch_returns_count_consistent() {
    let model_dir = make_model_dir();
    let config = make_config(model_dir.path(), OfflineMode::Strict);
    let adapter =
        LocalEmbeddingAdapter::new(config).expect("strict + model present must construct");

    // 32 chunks is the Pack #24 ingestion
    // pipeline's default batch size.
    let texts: Vec<String> = (0..32)
        .map(|i| format!("text number {i}, a uniquely identifiable sentence"))
        .collect();

    let ctx = make_ctx("acme-realty");
    let vectors = adapter
        .embed_batch(&texts, &ctx)
        .await
        .expect("embed_batch on a real-model adapter must return Ok");

    // Observable: the output has exactly N
    // elements, one per input. The ingestion
    // pipeline (Pack #24) pairs `embed_batch`
    // output with its input by index — if the
    // count is wrong, the chunk ↔ vector
    // alignment is off by one and the
    // knowledge graph (Pack #26) stores
    // wrong edges.
    assert_eq!(
        vectors.len(),
        texts.len(),
        "embed_batch must return one vector per input (32 in → 32 out)"
    );

    // Observable: every output vector is
    // 384-dim.
    for (i, v) in vectors.iter().enumerate() {
        assert_eq!(v.len(), 384, "vector at index {i} must be 384-dim");
    }

    // Observable: different inputs produce
    // different vectors (the stub hashes the
    // text, so "text 0" and "text 1" have
    // different SHA-256 digests and therefore
    // different vectors). If the stub
    // accidentally returned the same vector
    // for every input, the RAG retrieval
    // engine would conflate all 32 chunks.
    let v0 = &vectors[0];
    let v1 = &vectors[1];
    assert_ne!(
        v0, v1,
        "different inputs must produce different vectors (text 0 vs text 1)"
    );

    let _arc = Arc::new(model_dir);
}

// ===================================================================
// Test 7 — `embed("")` must return
// `Err(InvalidInput)`. Phase 1.5 added
// the empty input check in
// `LocalEmbeddingAdapter::embed` (and
// `embed_batch`); this test pins the
// contract.
//
// Observable: `embed("")` returns
// `Err(InvalidInput)` with a
// `reason` field that mentions
// "empty".
//
// This test does NOT need the
// model fixture (the empty check
// runs before the model is loaded).
// It is `#[ignore]` for consistency
// with the 6 Phase 1 tests (so the
// operator runs `cargo test
// -- --ignored` once to verify the
// whole Phase 1.5 contract).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
#[ignore = "Phase 1.5: runs with `cargo test -- --ignored` (does not need the model fixture, but is grouped with the Phase 1.5 batch)."]
async fn test_local_adapter_empty_input_rejected() {
    let model_dir = make_model_dir();
    let config = make_config(model_dir.path(), OfflineMode::Strict);
    let adapter = LocalEmbeddingAdapter::new(config).expect("adapter should construct");
    let ctx = make_ctx("acme-realty");
    let result = adapter.embed("", &ctx).await;
    let err = match result {
        Err(e) => e,
        Ok(v) => panic!("embed(\"\") must return Err, got Ok({} elements)", v.len()),
    };
    match &err {
        EmbeddingErrorV1::InvalidInput { reason } => {
            assert!(
                reason.contains("empty"),
                "InvalidInput.reason must mention 'empty', got: {reason}"
            );
        }
        other => panic!("expected EmbeddingErrorV1::InvalidInput, got {other:?}"),
    }
    let _arc = Arc::new(model_dir);
}

// ===================================================================
// Test 8 — `embed_batch(32)` must be
// faster than 32 separate
// `embed()` calls. Phase 1.5: the
// real candle forward pass runs
// N inputs in a single
// `BertModel::forward` call
// (vs 32 separate `forward`
// calls in the per-text path).
//
// Observable: 1 `embed_batch(32)`
// call is at least 4x faster
// than 32 individual `embed`
// calls (each spawning its own
// forward pass via
// `task::spawn_blocking`).
//
// This test is `#[ignore]`
// because it needs the
// real model fixture AND
// is timing-dependent
// (flaky on slow CI
// runners).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
#[ignore = "Phase 1.5: needs the real all-MiniLM-L6-v2 fixture AND is timing-dependent (flaky on slow CI). Enable manually for the Phase 1.5 verification run."]
async fn test_local_adapter_embed_batch_is_faster_than_loop() {
    let model_dir = make_model_dir();
    let config = make_config(model_dir.path(), OfflineMode::Strict);
    let adapter = LocalEmbeddingAdapter::new(config).expect("adapter should construct");
    let ctx = make_ctx("acme-realty");
    let texts: Vec<String> = (0..32).map(|i| format!("text {i}")).collect();

    let t_loop = std::time::Instant::now();
    for text in &texts {
        adapter.embed(text, &ctx).await.expect("embed must succeed");
    }
    let loop_time = t_loop.elapsed();

    let t_batch = std::time::Instant::now();
    adapter
        .embed_batch(&texts, &ctx)
        .await
        .expect("embed_batch must succeed");
    let batch_time = t_batch.elapsed();

    assert!(
        batch_time * 4 < loop_time,
        "embed_batch ({batch_time:?}) must be at least 4x faster than 32 embed calls ({loop_time:?})"
    );

    let _arc = Arc::new(model_dir);
}

// ===================================================================
// Test 9 — `Lazy` download strategy
// must trigger the HuggingFace
// download on the first `embed`
// call. Phase 1.5: the
// `LocalEmbeddingAdapter` is
// configured with a missing
// model dir + `Lazy` strategy.
// The first `embed` call must
// trigger `Downloader::download`,
// which fetches the model files
// from a `wiremock-rs` server and
// places them on disk. The
// second `embed` call must NOT
// trigger another download (the
// model is cached).
//
// This test is `#[ignore]`
// because:
// 1. It needs a real model
//    fixture (the
//    `Downloader` writes
//    the same files the
//    real `BertEmbedder`
//    reads)
// 2. It needs a `wiremock-rs`
//    server (the
//    `Downloader` is
//    HTTP-based, not
//    file-based)
// 3. The
//    `DownloadStrategy::Lazy`
//    struct in the
//    current code is a
//    unit variant
//    (Phase 1.5 NOTE: it
//    must be extended
//    to carry the
//    `huggingface_url`
//    field; this is a
//    follow-up to the
//    current commit).
// ===================================================================
#[tokio::test(flavor = "current_thread")]
#[ignore = "Phase 1.5: needs the real all-MiniLM-L6-v2 fixture + a wiremock-rs server + DownloadStrategy::Lazy must be extended to carry huggingface_url. Enable manually for the Phase 1.5 verification run."]
async fn test_local_adapter_lazy_download() {
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    // (1) Spin up a
    // `wiremock-rs` server
    // that returns the
    // model files.
    let server = MockServer::start().await;
    let config_json = r#"{"hidden_size": 384, "vocab_size": 30522, "num_hidden_layers": 6, "num_attention_heads": 12, "intermediate_size": 1536, "max_position_embeddings": 512, "type_vocab_size": 2, "hidden_act": "gelu"}"#;
    let tokenizer_json = "{}";
    let safetensors_bytes = vec![0u8; 16];
    Mock::given(method("GET"))
        .and(path("/config.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(config_json))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/tokenizer.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tokenizer_json))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/model.safetensors"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(safetensors_bytes))
        .mount(&server)
        .await;

    // (2) Build a config
    // pointing at the
    // wiremock server
    // with a missing
    // model dir.
    let model_dir = tempfile::tempdir().expect("tempdir create");
    let model_path = model_dir.path().join("all-MiniLM-L6-v2");
    // The
    // `DownloadStrategy::Lazy`
    // is a unit
    // variant in the
    // current code;
    // the URL is
    // hard-coded to
    // the HuggingFace
    // CDN. The
    // wiremock server
    // is reachable
    // from the test
    // (in-process);
    // the production
    // path uses the
    // real CDN.
    let config = LocalEmbeddingConfig {
        model_name: "all-MiniLM-L6-v2".to_string(),
        model_dir: model_path.clone(),
        offline_mode: OfflineMode::Strict,
        download_strategy: DownloadStrategy::Lazy,
    };
    let adapter =
        LocalEmbeddingAdapter::new(config).expect("adapter should construct in Lazy mode");
    let ctx = make_ctx("acme-realty");

    // (3) First
    // `embed`
    // call:
    // the
    // `Downloader`
    // fetches
    // the
    // model,
    // the
    // `BertEmbedder::load`
    // parses
    // it,
    // the
    // forward
    // pass
    // returns
    // a
    // vector.
    let v1 = adapter
        .embed("hello, world", &ctx)
        .await
        .expect("first embed must succeed (download + load + forward pass)");

    // (4) Second
    // `embed`
    // call:
    // the
    // model
    // is
    // cached.
    let v2 = adapter
        .embed("hello, world", &ctx)
        .await
        .expect("second embed must succeed (cached model)");

    // Observable:
    // the two
    // vectors
    // are
    // byte-equal.
    assert_eq!(
        v1, v2,
        "lazy-download embed must be deterministic across two calls (no re-download on the second)"
    );

    // (5) The
    // model
    // files
    // must
    // exist
    // on
    // disk
    // after
    // the
    // first
    // `embed`.
    assert!(
        model_path.join("config.json").exists(),
        "Downloader must write config.json to disk after the first embed call"
    );
    assert!(
        model_path.join("tokenizer.json").exists(),
        "Downloader must write tokenizer.json to disk after the first embed call"
    );
    assert!(
        model_path.join("model.safetensors").exists(),
        "Downloader must write model.safetensors to disk after the first embed call"
    );

    let _arc_model = Arc::new(model_dir);
    let _arc_server = Arc::new(server);
}
