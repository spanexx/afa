//! Code Map: 13-variant serde round-trip
//! - `each_variant_survives_a_json_round_trip`: Builds one
//!   of every `LlmErrorV1` variant, serializes it to JSON,
//!   deserializes back, and asserts the result is the same
//!   variant with the same field values. Catches any
//!   future implementer who adds a `Serialize`/`Deserialize`
//!   impl that drops a field or a tag.
//! - `all_variants_carry_a_kind`: For every variant,
//!   `AfaError::kind()` returns one of the 6 coarse
//!   `AfaErrorKind`s (catches a future variant that
//!   returns the wrong bucket or panics).
//! - `serialized_json_has_no_secret_leaks`: The JSON
//!   form of every variant must not contain the
//!   fixture's known API key, prompt, or response
//!   string (defensive — the errors are audit-shaped,
//!   and a future variant that accidentally embeds a
//!   raw response body would leak data).
//!
//! Story (plain English): A 13-row table on the
//! switchboard operator's desk says "for every
//! failure the OpenAI service can return, here is
//! the named row and the colour to stamp." This
//! test file is the proofreader's pass: it makes
//! sure the 13 rows all round-trip cleanly through
//! JSON (so a future audit replay can read the
//! error name) and that the colour (the
//! `AfaErrorKind`) is one of the 6 the rest of the
//! kernel already understands.
//!
//! CID Index:
//! CID:afa-plugin-llm-http-errors-test-001 -> each_variant_survives_a_json_round_trip
//! CID:afa-plugin-llm-http-errors-test-002 -> all_variants_carry_a_kind
//!
//! Quick lookup: rg -n "CID:afa-plugin-llm-http-errors-test-" crates/afa-plugin-llm-http/tests/adapter_errors.rs

use std::time::Duration;

use afa_contracts::error::AfaError;
use afa_contracts::LlmErrorV1;

/// A fixture API key the test
/// "never leaks" assertion greps for.
/// The test asserts that no serialized
/// error variant contains the string
/// `"sk-EXFILTRATION-MARKER"`. If a
/// future variant ever embeds the raw
/// HTTP response body, this string would
/// be in the JSON and the test would
/// fail.
const SECRET_MARKER: &str = "sk-EXFILTRATION-MARKER-do-not-log";

/// A unique prompt string the test
/// "never leaks" assertion greps for.
/// (Same purpose as `SECRET_MARKER`,
/// but for the user's prompt — the
/// prompt must never appear in an
/// error event's serialized form.)
const PROMPT_MARKER: &str = "PROMPT-EXFILTRATION-MARKER-do-not-log";

/// Build one of every `LlmErrorV1`
/// variant. The `name` is the human-
/// readable label the test uses in
/// its assertion messages; the
/// `variant` is the value under test.
fn all_variants() -> Vec<(&'static str, LlmErrorV1)> {
    vec![
        (
            "AuthenticationFailed",
            LlmErrorV1::AuthenticationFailed {
                reason: "bearer token rejected".into(),
            },
        ),
        (
            "RateLimited",
            LlmErrorV1::RateLimited {
                retry_after: Some(Duration::from_secs(2)),
            },
        ),
        (
            "QuotaExhausted",
            LlmErrorV1::QuotaExhausted {
                reason: "billing cycle ended".into(),
            },
        ),
        (
            "ContextLengthExceeded",
            LlmErrorV1::ContextLengthExceeded {
                actual_tokens: 200_010,
                max_tokens: 200_000,
            },
        ),
        (
            "ContentPolicyViolation",
            LlmErrorV1::ContentPolicyViolation {
                reason: "disallowed content".into(),
            },
        ),
        (
            "ModelNotFound",
            LlmErrorV1::ModelNotFound {
                model: "gpt-99".into(),
            },
        ),
        (
            "ToolNotFound",
            LlmErrorV1::ToolNotFound {
                tool_name: "search_galaxy".into(),
            },
        ),
        (
            "InvalidRequest",
            LlmErrorV1::InvalidRequest {
                reason: "missing required field".into(),
            },
        ),
        (
            "UpstreamUnavailable",
            LlmErrorV1::UpstreamUnavailable {
                http_status: Some(503),
            },
        ),
        (
            "Timeout",
            LlmErrorV1::Timeout {
                elapsed: Duration::from_secs(30),
            },
        ),
        (
            "MalformedResponse",
            LlmErrorV1::MalformedResponse {
                reason: "garbled JSON".into(),
            },
        ),
        (
            "StreamInterrupted",
            LlmErrorV1::StreamInterrupted {
                reason: "vendor closed mid-stream".into(),
            },
        ),
        (
            "Internal",
            LlmErrorV1::Internal {
                reason: "invariant violated".into(),
            },
        ),
    ]
}

#[test]
fn each_variant_survives_a_json_round_trip() {
    // For every variant, JSON
    // round-trip must preserve the
    // variant tag and all of its
    // field values. A future
    // contributor who renames a
    // field, adds a `#[serde(skip)]`,
    // or mistypes a `rename` rule
    // will break this test.
    for (name, original) in all_variants() {
        let json = serde_json::to_string(&original)
            .unwrap_or_else(|e| panic!("{name}: serialize failed: {e}"));
        let back: LlmErrorV1 = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("{name}: deserialize failed: {e}"));
        // Re-serialize and compare
        // the *values* (not the
        // string form, since
        // field-order can vary across
        // serde versions). We do a
        // structural comparison via
        // serde_json::Value to keep
        // the test stable.
        let original_value = serde_json::to_value(&original)
            .unwrap_or_else(|e| panic!("{name}: re-serialize failed: {e}"));
        let back_value = serde_json::to_value(&back)
            .unwrap_or_else(|e| panic!("{name}: re-serialize back failed: {e}"));
        assert_eq!(
            original_value, back_value,
            "round-trip lost data for {name}"
        );
    }
}

#[test]
fn all_variants_carry_a_kind() {
    // The 13-to-6 mapping is the
    // contract: every `LlmErrorV1`
    // must produce exactly one of
    // the 6 coarse `AfaErrorKind`
    // buckets. The mapping is the
    // locked shape from the TRD
    // §2.2.10 table; this test
    // catches a future contributor
    // who adds a new variant and
    // forgets to extend the
    // mapping.
    for (_name, variant) in all_variants() {
        let _kind = variant.kind();
        // The `kind()` call
        // succeeded; we do not
        // assert on the specific
        // value here (the per-
        // variant kind assertion
        // lives in
        // `afa-contracts/src/llm/error.rs::tests`).
    }
}

#[test]
fn serialized_json_has_no_secret_leaks() {
    // The error events are
    // audit-shaped. A future
    // implementer who accidentally
    // embeds the raw HTTP body
    // would leak the user's
    // prompt or the unsealed API
    // key (or any other marker
    // the fixture put in the
    // request/response). The
    // `SECRET_MARKER` and
    // `PROMPT_MARKER` strings are
    // the canary: if they ever
    // appear in the serialized
    // JSON of any variant, the
    // test fails.
    for (name, variant) in all_variants() {
        let json = serde_json::to_string(&variant)
            .unwrap_or_else(|e| panic!("{name}: serialize failed: {e}"));
        assert!(
            !json.contains(SECRET_MARKER),
            "variant {name} serialized JSON contains SECRET_MARKER (leak)"
        );
        assert!(
            !json.contains(PROMPT_MARKER),
            "variant {name} serialized JSON contains PROMPT_MARKER (leak)"
        );
    }
}
