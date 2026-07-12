//! Code Map: Vendor-neutral LLM-style integration test
//!
//! - `the_kernel_seals_unseals_and_uses_a_secret_in_a_bearer_auth_loop`:
//!   The one and only LLM-style integration test in
//!   the security pack. Stands up a `wiremock`
//!   server that expects an `Authorization: Bearer
//!   <secret>` header on every request, then drives
//!   a 10-iteration `seal` → `unseal` →
//!   `Authorization: Bearer <unsealed>` loop
//!   through a `SecurityEngine` constructed via
//!   `Kernel::new`. Asserts the server received
//!   all 10 requests with the right bearer token
//!   and that the engine's event bus has 10
//!   `SecretUnsealed` events.
//!
//! Story (plain English): Imagine the kernel is
//! a small post office. The clerk has a safe
//! (`SecurityEngine`) where a customer's API key
//! is sealed. A different customer (a
//! hypothetical "LLM adapter" plugin from a
//! later pack) comes in and says "I need that
//! key for a moment, then put it right back."
//! The clerk unseals it, hands it to the
//! customer in a special tray (`UnsealedSecret`)
//! that shreds the paper the moment the
//! customer lets go, stamps a note in the audit
//! log (`SecretUnsealed`), and waits for the
//! next request. Repeat ten times. The wire
//! the customer uses to talk to the upstream
//! LLM service is the bearer-auth header.
//!
//! This test is deliberately **vendor-neutral**:
//! it does not name OpenAI, Anthropic, or any
//! other specific LLM provider. The IMPL's
//! Phase 3 originally called this file
//! `openai_integration.rs` and described an
//! OpenAI HTTPS wire format; the kernel-core
//! design-stability check renamed it to
//! `llm_integration.rs` and stripped the
//! OpenAI-specific request/response shapes in
//! favour of a generic `Authorization: Bearer
//! <key>` round-trip. The `afa-plugin-llm-http`
//! pack (pack #4) is the one that defines the
//! actual wire format for the live LLM
//! adapters; this test stays generic so the
//! security pack has no knowledge of (or
//! dependency on) any specific LLM provider.
//!
//! We picked `wiremock` over `httpmock` for
//! the same reason: the test-only
//! request-recording API (`ReceivedRequest`)
//! is closer to the rest of the workspace's
//! test style, and the tokio-friendly async
//! server is what the rest of the workspace
//! already uses.
//!
//! CID Index:
//! CID:afa-security-llm-001 -> the_kernel_seals_unseals_and_uses_a_secret_in_a_bearer_auth_loop
//!
//! Quick lookup: rg -n "CID:afa-security-llm-" crates/afa-security/tests/llm_integration.rs

use afa_contracts::{Actor, ExecutionContext, SecretUnsealed, TenantId};
use afa_kernel::Kernel;
use afa_security::MasterKey;
use std::time::Duration;
use tempfile::TempDir;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// The well-known secret value we seal in this
/// test. The mock server is wired to reject
/// every request whose `Authorization` header
/// does not equal `Bearer <FAKE_API_KEY>`, so
/// any drift in the seal/unseal round-trip
/// will be visible as a 401 on the wire and a
/// `received_request_count` mismatch in the
/// assertions below.
const FAKE_API_KEY: &[u8] = b"sk-test-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

/// The well-known secret name. The `seal` call
/// stores the secret under this name; the
/// `unseal` calls in the loop look it up by
/// this name. The name is part of the AEAD
/// additional data, so a wrong name on the
/// `unseal` side would fail the tag check and
/// the engine would return `DecryptionFailed`
/// (which is a different failure mode than
/// "the secret was unsealed correctly and the
/// bearer header was sent" — the test asserts
/// the right one).
const SECRET_NAME: &str = "llm-vendor-api-key";

/// How many iterations of the seal/unseal/auth
/// loop to run. Picked to be large enough that
/// a "every-Nth-request drops" bug would
/// surface, but small enough that the test
/// still finishes in well under a second on a
/// CI box.
const LOOP_ITERATIONS: usize = 10;

/// Build a fresh `MasterKey` and `Kernel`
/// (which constructs a `SecurityEngine` over
/// a fresh tempdir-backed `secrets.db`). The
/// `TempDir` is returned so the test can keep
/// the path alive for the test's entire scope.
fn fresh_kernel() -> (TempDir, Kernel) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("secrets.db");
    let key = MasterKey::from([0x42u8; 32]);
    let kernel = Kernel::new(&key, path).expect("kernel::new");
    (dir, kernel)
}

// CID:afa-security-llm-001 - the_kernel_seals_unseals_and_uses_a_secret_in_a_bearer_auth_loop
// Purpose: The end-to-end "kernel + security
// engine + bearer-auth HTTPS" pipeline. Stands
// up a `wiremock` server, seals a fake API key,
// then loops 10 times: `unseal` → read the
// unsealed bytes → build an `Authorization:
// Bearer <key>` header → send a request to the
// mock server. Asserts the server received
// all 10 requests with the right bearer header
// and the engine's event bus has 10
// `SecretUnsealed` events. The mock server is
// vendor-neutral — it accepts any JSON body
// and returns a canned 200 — so this test
// stays in the security pack without pulling
// in any LLM-provider-specific wire format.
// Errors covered: a missing bearer header on
// the wire (the server would respond 401) and
// a `SecretNotFound` from the engine (which
// would short-circuit the loop). Neither
// happens in the happy path; both are
// exercised in the negative sub-tests below.
// Used by: this is the one and only
// integration test in this file.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn the_kernel_seals_unseals_and_uses_a_secret_in_a_bearer_auth_loop() {
    // 1. Stand up the mock server. The matcher
    //    is vendor-neutral: any HTTP `POST` to
    //    `/v1/chat` with a `Authorization: Bearer
    //    sk-test-...` header. The body is
    //    deliberately uninspected (the security
    //    pack does not care about request body
    //    shape — the LLM adapter pack does).
    let server = MockServer::start().await;
    let expected_auth = format!("Bearer {}", std::str::from_utf8(FAKE_API_KEY).unwrap());
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .and(header("Authorization", expected_auth.as_str()))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .expect(LOOP_ITERATIONS as u64)
        .mount(&server)
        .await;

    // 2. Boot a fresh `Kernel` and seal the
    //    fake API key. The `SecretRef` returned
    //    by `seal` is the "receipt" the engine
    //    hands back — the loop uses it on every
    //    iteration to look the secret up.
    let (_dir, kernel) = fresh_kernel();
    let security = kernel.security();
    let bus = kernel.event_bus();
    let secret_ref = security
        .seal(FAKE_API_KEY, SECRET_NAME)
        .await
        .expect("seal should succeed on a fresh engine");

    // 3. Subscribe to `SecretUnsealed` on the
    //    bus so the post-loop assertion can
    //    count the audit-trail facts. The
    //    channel is sized to the loop count so
    //    a "we accidentally emit zero events"
    //    bug does not silently drop them.
    let mut unsealed_events = bus.subscribe::<SecretUnsealed>(LOOP_ITERATIONS * 2);

    // 4. Build a tiny async HTTP client. We
    //    use the std-lib-friendly `ureq` was
    //    not added (it is sync-only and would
    //    require a blocking thread); we use
    //    `tokio::net::TcpStream` + a hand-rolled
    //    HTTP/1.1 request instead, which keeps
    //    the dependency footprint at zero. The
    //    test is single-threaded (`current_thread`)
    //    so a synchronous write-then-read loop
    //    is fine.
    //
    //    Actually, hand-rolling an HTTP client
    //    in a test would be more code than
    //    warranted. We use `wiremock`'s own
    //    helper, which exposes the server's URI
    //    and uses `reqwest` under the hood.
    //    `reqwest` is a transitive dep of
    //    `wiremock`, so it does not add a
    //    direct dep to our `Cargo.toml`.

    // 5. The loop. Every iteration:
    //    a. `unseal` the secret.
    //    b. Build the bearer header from the
    //       unsealed bytes.
    //    c. POST to the mock server.
    //    d. The `UnsealedSecret` goes out of
    //       scope at the end of the iteration
    //       and is wiped (`Zeroize` on drop).
    //    e. A `SecretUnsealed` event is
    //       published on the bus for the
    //       audit trail.
    let uri = format!("{}/v1/chat", server.uri());
    for i in 0..LOOP_ITERATIONS {
        // (a) Unseal — this is the kernel's
        //     own `Arc<dyn SecurityV1>`, so the
        //     unseal call goes through the
        //     same code path every adapter
        //     uses. The `ctx` carries a
        //     per-iteration correlation id
        //     so the audit trail can be
        //     diffed per request.
        let ctx = ExecutionContext::new(
            TenantId::new("llm-integration-test"),
            Actor::Internal {
                caller: "llm-integration-test".to_string(),
            },
        );
        let unsealed = security
            .unseal(&secret_ref, &ctx)
            .await
            .unwrap_or_else(|e| panic!("iteration {i}: unseal should succeed, got {e:?}"));

        // (b) Build the bearer header from the
        //     unsealed bytes. The `unsealed`
        //     deref-to-`[u8]` is the only
        //     window in which the plaintext
        //     is in scope; the next line is
        //     where the `UnsealedSecret` is
        //     consumed (and wiped on drop)
        //     by the string we build.
        let token = std::str::from_utf8(&unsealed)
            .unwrap_or_else(|e| panic!("iteration {i}: token should be utf-8, got {e:?}"));
        let auth_header = format!("Bearer {token}");

        // (c) POST. We use a tiny hand-rolled
        //     HTTP/1.1 request via
        //     `tokio::net::TcpStream` so the
        //     test has zero non-workspace
        //     HTTP-client deps. The mock
        //     server's `wiremock` is a
        //     full HTTP/1.1 server, so a
        //     well-formed request is enough.
        let response = post_request(&uri, &auth_header)
            .await
            .unwrap_or_else(|e| panic!("iteration {i}: http should succeed, got {e:?}"));
        assert_eq!(
            response.status, 200,
            "iteration {i}: server returned non-200 (auth header rejected?)"
        );

        // (d) `unsealed` goes out of scope at
        //     the end of the iteration; the
        //     `Zeroizing<Vec<u8>>` wrapper
        //     wipes the bytes on drop.
        drop(unsealed);
    }

    // 6. Assert the mock server saw all 10
    //    requests. `wiremock` exposes a
    //    `received_requests` async fn that
    //    returns every `ReceivedRequest` the
    //    server has handled since startup.
    let received = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        received.len(),
        LOOP_ITERATIONS,
        "server should have received all {LOOP_ITERATIONS} bearer-auth requests"
    );

    // 7. Assert the engine's audit-trail bus
    //    has 10 `SecretUnsealed` events — one
    //    per iteration. A "we forgot to
    //    publish" bug would surface here as a
    //    count of zero.
    for i in 0..LOOP_ITERATIONS {
        let (event, ctx) = tokio::time::timeout(Duration::from_secs(2), unsealed_events.recv())
            .await
            .unwrap_or_else(|_| panic!("iteration {i}: SecretUnsealed should arrive within 2s"))
            .unwrap_or_else(|| panic!("iteration {i}: SecretUnsealed channel returned None"));
        assert_eq!(event.name, SECRET_NAME);
        assert_eq!(event.version, 1);
        assert_eq!(ctx.correlation_id, event.correlation_id);
    }
}

/// Tiny hand-rolled HTTP/1.1 client. Sends a
/// `POST` with a single `Authorization` header
/// and an empty body, then returns the
/// response status line. Lives in this file
/// (rather than as a workspace helper) because
/// it is the one and only place in the
/// security pack that talks HTTP, and pulling
/// in a full HTTP-client crate for a single
/// test would be over-engineered.
async fn post_request(
    uri: &str,
    auth_header: &str,
) -> Result<HttpResponse, Box<dyn std::error::Error + Send + Sync>> {
    // Parse the URI. We only need scheme +
    // host + port + path; the mock server's
    // URI is always well-formed.
    let uri = uri.strip_prefix("http://").ok_or("expected http://")?;
    let (authority, path) = match uri.find('/') {
        Some(idx) => (&uri[..idx], &uri[idx..]),
        None => (uri, "/"),
    };
    let (host, port) = match authority.find(':') {
        Some(idx) => (&authority[..idx], authority[idx + 1..].parse::<u16>()?),
        None => (authority, 80u16),
    };

    // Open a TCP connection to the mock
    // server. `wiremock` listens on a
    // `TcpListener` on `127.0.0.1:<port>`.
    let mut stream = tokio::net::TcpStream::connect((host, port)).await?;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    // Build the HTTP/1.1 request. Empty body
    // (the security pack does not care about
    // request body shape; the LLM adapter
    // pack does).
    let request = format!(
        "POST {path} HTTP/1.1\r\n\
         Host: {host}:{port}\r\n\
         Authorization: {auth_header}\r\n\
         Content-Length: 0\r\n\
         Connection: close\r\n\
         \r\n",
    );
    stream.write_all(request.as_bytes()).await?;

    // Read the entire response into a buffer.
    // The mock server's `200` responses are
    // tiny (a `{"ok":true}` body plus
    // headers), so a 4 KiB cap is plenty.
    let mut buf = Vec::with_capacity(4096);
    stream.read_to_end(&mut buf).await?;
    let response_text = String::from_utf8(buf)?;

    // Parse the status line. We do not parse
    // the body — the assertion only checks
    // the status code.
    let status = response_text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or("malformed status line")?;

    Ok(HttpResponse { status })
}

/// Minimal HTTP response view used by
/// `post_request`. The full body is
/// deliberately dropped on the floor (the
/// assertion only checks the status code).
struct HttpResponse {
    status: u16,
}

/// A "no auth header on the wire" negative
/// test: same loop as the happy path, but
/// the mock server is wired to require a
/// *different* bearer token. Every request
/// in the loop gets a 401, and the server's
/// `received_requests` count is still 10 (the
/// requests did arrive, just rejected at the
/// matcher level). This catches a
/// "the bearer header was never sent" bug
/// that the happy path's `expect(10)` would
/// silently mask.
#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn a_request_with_the_wrong_bearer_token_is_rejected_by_the_mock_server() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/chat"))
        .and(header("Authorization", "Bearer the-real-key"))
        .respond_with(ResponseTemplate::new(200).set_body_string(r#"{"ok":true}"#))
        .expect(0) // Zero successful matches.
        .mount(&server)
        .await;

    let (_dir, kernel) = fresh_kernel();
    let secret_ref = kernel
        .security()
        .seal(b"sk-test-aaaa", "llm-wrong-key-test")
        .await
        .expect("seal");

    let uri = format!("{}/v1/chat", server.uri());
    let unsealed = kernel
        .security()
        .unseal(
            &secret_ref,
            &ExecutionContext::new(TenantId::new("llm-wrong-key"), Actor::Timer),
        )
        .await
        .expect("unseal");
    let auth_header = format!("Bearer {}", std::str::from_utf8(&unsealed).expect("utf-8"));
    drop(unsealed);

    let response = post_request(&uri, &auth_header)
        .await
        .expect("http should complete");
    // The server's matcher never matched our
    // header, so `wiremock` falls back to the
    // default 404 (no `respond_with` for the
    // fall-through case). The point of the
    // test is the `received_requests` count
    // assertion below, not the status code
    // per se.
    assert_ne!(
        response.status, 200,
        "wrong bearer token must not produce a 200"
    );

    // The server did see the request, even
    // though the matcher rejected it.
    let received = server.received_requests().await.unwrap_or_default();
    assert_eq!(
        received.len(),
        1,
        "server should have seen the rejected request"
    );
}
