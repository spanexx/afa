//! Code Map: HealthStatus reason cap regression-proof
//! - 4 tests covering the `Display` impl's
//!   `REASON_DISPLAY_CAP` (200 char) truncation:
//!   - short reason: not truncated
//!   - exactly 200 chars: not truncated
//!   - 201 chars: truncated to 200 + "..."
//!   - 1000 chars: truncated to 200 + "..."
//! - 2 tests covering the `serde_json` round-trip of
//!   `HealthStatus` so a future change to the serde
//!   attributes is caught (the dashboard relies on
//!   `{"status": "degraded", "reason": "..."}` form).
//! - 1 test covering the `HealthCheck` trait object
//!   dyn-compatibility (the kernel's aggregator holds
//!   `Vec<Arc<dyn HealthCheck>>`).
//!
//! Story (plain English): A hospital's ward-board is only
//! useful if every nurse can read it from across the room.
//! If one engine's "why is this patient sick?" note is 5
//! kilobytes long, the board overflows and no one can read
//! any of the other notes. The 200-char cap is the
//! dashboard's "stay readable" rule — a runaway plugin
//! cannot blow out the board with a megabyte of log text.
//! These tests are the regression-proof for the cap.

use afa_contracts::{HealthCheck, HealthStatus};
use std::sync::Arc;

// CID:health-check-shape-001 - reason cap on Display
#[test]
fn display_short_reason_not_truncated() {
    let s = HealthStatus::Degraded {
        reason: "3 drops in last hour".into(),
    };
    let out = format!("{s}");
    assert_eq!(out, "degraded:3 drops in last hour");
}

#[test]
fn display_exactly_200_char_reason_not_truncated() {
    // Exactly 200 chars is the boundary case — not
    // truncated (no ellipsis).
    let exact = "y".repeat(200);
    let s = HealthStatus::Unhealthy {
        reason: exact.clone(),
    };
    let out = format!("{s}");
    // prefix "unhealthy:" is 10 chars + 200 'y' = 210.
    assert_eq!(out.len(), 10 + 200, "got {out:?}");
    assert!(!out.ends_with("..."), "must not be truncated at boundary");
    assert!(out.ends_with(&exact), "full reason must be present");
}

#[test]
fn display_201_char_reason_truncated_to_200_with_ellipsis() {
    // 201 chars is one past the cap — must be truncated
    // and suffixed with "...".
    let long = "z".repeat(201);
    let s = HealthStatus::Degraded { reason: long };
    let out = format!("{s}");
    // prefix "degraded:" is 9 chars + 200 'z' + 3 dots = 212.
    assert_eq!(out.len(), 9 + 200 + 3, "got {out:?}");
    assert!(out.starts_with("degraded:"));
    assert!(out.ends_with("..."));
}

#[test]
fn display_1000_char_reason_truncated_to_200_with_ellipsis() {
    // 1000 chars is well past the cap — must be
    // truncated to exactly 200 + ellipsis. This is the
    // "runaway plugin" scenario the cap is designed for.
    let long = "x".repeat(1000);
    let s = HealthStatus::Unhealthy { reason: long };
    let out = format!("{s}");
    assert_eq!(out.len(), 10 + 200 + 3, "got {out:?}");
    assert!(out.starts_with("unhealthy:"));
    assert!(out.ends_with("..."));
    // The truncation must not silently drop the prefix —
    // a reader who sees "..." in the output needs to be
    // able to scroll back to find "unhealthy:" or
    // "degraded:".
    assert!(out.contains("unhealthy:"));
}

#[test]
fn display_truncates_on_char_boundary_not_byte_boundary() {
    // The 200-char cap is on chars, not bytes — a
    // multi-byte character that crosses the 200-byte
    // boundary must NOT be split. A naive byte-based
    // truncation would produce invalid UTF-8 (a panic
    // at the next `format!` call). The `truncate_for_display`
    // helper walks the string by chars to keep the
    // output valid UTF-8.
    //
    // Build a 250-char string of 4-byte emoji; the
    // 200th char is mid-emoji in bytes, but the
    // truncation must take exactly 200 chars (not 200
    // bytes).
    let emoji = "\u{1F600}"; // 4 bytes in UTF-8
    let long = emoji.repeat(250);
    let s = HealthStatus::Degraded { reason: long };
    let out = format!("{s}");
    // The output must be valid UTF-8 (it is — the
    // helper uses `chars().take(cap)`), and the
    // truncated portion must be exactly 200 emoji
    // characters, not 50 (which a 200-byte cap would
    // produce).
    let suffix = &out["degraded:".len()..];
    assert!(suffix.ends_with("..."), "must end with ellipsis: {out:?}");
    let truncated = &suffix[..suffix.len() - 3];
    let char_count = truncated.chars().count();
    assert_eq!(
        char_count, 200,
        "truncation must be 200 chars, got {char_count}"
    );
}

// CID:health-check-shape-002 - serde round-trip
#[test]
fn health_status_serde_round_trip() {
    // The wire form is `{"status": "healthy"}`,
    // `{"status": "degraded", "reason": "..."}`, and
    // `{"status": "unhealthy", "reason": "..."}` (the
    // `#[serde(tag = "status")]` form). This is the
    // regression-proof that the dashboard's JSON
    // expectations are met.
    for s in [
        HealthStatus::Healthy,
        HealthStatus::Degraded {
            reason: "3 drops".into(),
        },
        HealthStatus::Unhealthy {
            reason: "down".into(),
        },
    ] {
        let json = serde_json::to_string(&s).unwrap();
        let back: HealthStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(s, back);
    }
}

#[test]
fn health_status_uses_tagged_serde_representation() {
    // The dashboard parses the JSON to extract
    // `result["status"]` and (for non-Healthy)
    // `result["reason"]`. This test pins the wire form
    // — a future change to the serde attributes that
    // changes the field names would break the dashboard.
    let healthy = serde_json::to_value(HealthStatus::Healthy).unwrap();
    assert_eq!(healthy, serde_json::json!({"status": "healthy"}));

    let degraded = serde_json::to_value(HealthStatus::Degraded { reason: "x".into() }).unwrap();
    assert_eq!(
        degraded,
        serde_json::json!({"status": "degraded", "reason": "x"})
    );

    let unhealthy = serde_json::to_value(HealthStatus::Unhealthy { reason: "y".into() }).unwrap();
    assert_eq!(
        unhealthy,
        serde_json::json!({"status": "unhealthy", "reason": "y"})
    );
}

// CID:health-check-shape-003 - HealthCheck trait object
struct FakeEngine {
    status: HealthStatus,
}

impl HealthCheck for FakeEngine {
    fn health_check(&self) -> HealthStatus {
        // The trait requires a cheap, no-I/O, no-lock
        // return. This impl just hands back the cached
        // value the constructor set.
        self.status.clone()
    }
}

#[test]
fn health_check_trait_object_is_dyn_compatible() {
    // The kernel's aggregator holds
    // `Vec<Arc<dyn HealthCheck>>` and calls
    // `engine.health_check()` on each one. The
    // dyn-compatibility requires the trait to have no
    // generic methods, no `Self` in argument or return
    // position, and no async methods. This test
    // compiles only if the trait is dyn-compatible.
    let engines: Vec<Arc<dyn HealthCheck>> = vec![
        Arc::new(FakeEngine {
            status: HealthStatus::Healthy,
        }),
        Arc::new(FakeEngine {
            status: HealthStatus::Degraded {
                reason: "index load slow".into(),
            },
        }),
    ];
    let results: Vec<HealthStatus> = engines.iter().map(|e| e.health_check()).collect();
    assert_eq!(results[0], HealthStatus::Healthy);
    assert_eq!(
        results[1],
        HealthStatus::Degraded {
            reason: "index load slow".into()
        }
    );
}
