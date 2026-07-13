//! Code Map: topic_slug helper
//! - `topic_slug`: The pure function that turns a
//!   human-readable topic name (e.g. "FAQ",
//!   "Property listings") into a safe on-disk
//!   directory name (e.g. "faq",
//!   "property-listings"). Rules: lowercase;
//!   ASCII-only (non-ASCII → '-'); non-
//!   alphanumeric → '-'; collapse consecutive
//!   '-'; trim leading/trailing '-'; cap at 64
//!   chars (truncate on '-' boundary); empty
//!   result → "_".
//!
//! Story (plain English): The slug helper is the
//! part of the adapter that turns the topic the
//! writer typed ("FAQ", "Property listings") into
//! a safe directory name on disk. A topic with
//! a space in it ("Property listings") would
//! otherwise need quoting; a topic with a slash
//! ("billing/refunds") would otherwise be a
//! security hole. The slug rules are the safety
//! belt.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-topic-slug-001 -> topic_slug
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-topic-slug-" crates/afa-plugin-knowledge-json/src/topic_slug.rs

// CID:afa-plugin-knowledge-json-topic-slug-001 - topic_slug
// Purpose: The pure function that turns a
// human-readable topic name into a safe
// on-disk directory name. Rules (per the
// IMPL Phase 1 task list):
// 1. Lowercase.
// 2. ASCII-only: any non-ASCII char → '-'.
// 3. Non-alphanumeric ASCII char → '-'.
// 4. Collapse consecutive '-'.
// 5. Trim leading/trailing '-'.
// 6. Cap at 64 chars (truncate on '-' boundary
//    so the slug does not end with a dangling
//    '-').
// 7. Empty result (e.g. topic was all
//    non-alphanumeric) → "_".
//
// The function is pure: no I/O, no
// allocation beyond the returned `String`,
// no side effects. The same input always
// produces the same output, which is
// essential for the
// "topic-name slug collides with existing
// topic" check the adapter performs before
// writing a new record.
// **Call pattern (per IMPL Phase 1)**:
// `topic_slug("FAQ") == "faq"`;
// `topic_slug("Property listings") ==
//  "property-listings"`.
pub fn topic_slug(topic: &str) -> String {
    // Step 1+2+3: lowercase, ASCII-only,
    // non-alphanumeric → '-'. The
    // `to_ascii_lowercase()` is no-op for
    // ASCII chars and `?`-substituted for
    // non-ASCII; we do it first so the
    // `is_ascii_alphanumeric()` check on
    // line ~30 reads the same character
    // class as the lowercase output.
    let mut buf: String = topic
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();

    // Step 4: collapse consecutive '-'. Walk
    // the buffer and skip a '-' if the
    // previous output was already '-'.
    let mut collapsed: String = String::with_capacity(buf.len());
    let mut prev_dash = false;
    for c in buf.chars() {
        if c == '-' {
            if !prev_dash {
                collapsed.push(c);
            }
            prev_dash = true;
        } else {
            collapsed.push(c);
            prev_dash = false;
        }
    }
    buf = collapsed;

    // Step 5: trim leading/trailing '-'.
    let trimmed = buf.trim_matches('-');

    // Step 6+7: cap at 64 chars (truncate
    // on '-' boundary so the slug does not
    // end with a dangling '-'); empty
    // result → "_".
    if trimmed.is_empty() {
        return "_".to_string();
    }
    if trimmed.len() <= 64 {
        return trimmed.to_string();
    }
    // Truncate to 64 chars, then trim a
    // trailing '-' if the boundary cut in
    // the middle of a "---" run (rare but
    // possible if the topic ended in a
    // punctuation run).
    let truncated = &trimmed[..64];
    let truncated = truncated.trim_end_matches('-');
    if truncated.is_empty() {
        // Defensive: the 64-char window
        // could in theory be all dashes
        // (e.g. topic is 64
        // non-alphanumeric chars). Fall
        // back to "_" rather than the
        // empty string.
        return "_".to_string();
    }
    truncated.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_simple_lowercase() {
        // The IMPL Phase 1 call pattern:
        // `topic_slug("FAQ") == "faq"`.
        assert_eq!(topic_slug("FAQ"), "faq");
    }

    #[test]
    fn slug_replaces_spaces_with_dashes() {
        // The IMPL Phase 1 call pattern:
        // `topic_slug("Property listings")
        //  == "property-listings"`.
        assert_eq!(topic_slug("Property listings"), "property-listings");
    }

    #[test]
    fn slug_collapses_consecutive_dashes() {
        // "billing   refunds" (3 spaces)
        // → "billing-refunds" (one dash,
        // not three).
        assert_eq!(topic_slug("billing   refunds"), "billing-refunds");
        // "a!!b" → "a-b" (the two '!'
        // collapse into one dash).
        assert_eq!(topic_slug("a!!b"), "a-b");
    }

    #[test]
    fn slug_trims_leading_and_trailing_dashes() {
        // "-hello-" → "hello".
        assert_eq!(topic_slug("-hello-"), "hello");
        // "..." → "" (after
        // trim_matches) → "_" (the empty
        // fallback).
        assert_eq!(topic_slug("..."), "_");
    }

    #[test]
    fn slug_replaces_non_ascii_with_dash() {
        // "café" → "caf-" (the 'é' is
        // non-ASCII, becomes '-', then
        // trailing '-' trimmed).
        assert_eq!(topic_slug("café"), "caf");
        // "日本" → "--" → "_".
        assert_eq!(topic_slug("日本"), "_");
    }

    #[test]
    fn slug_caps_at_64_chars_on_dash_boundary() {
        // Build a 70-char topic. The slug
        // must be at most 64 chars and
        // must not end with a dangling
        // '-'.
        let topic = "a".repeat(70);
        let slug = topic_slug(&topic);
        assert!(slug.len() <= 64);
        assert!(!slug.ends_with('-'));
    }

    #[test]
    fn slug_empty_input_returns_underscore() {
        // "" → "_" (the empty fallback).
        assert_eq!(topic_slug(""), "_");
    }

    #[test]
    fn slug_handles_mixed_punctuation() {
        // A real-world topic name: "FAQ's
        // & billing?" → "faq-s-billing"
        // ('s apostrophe + space + '&' +
        // space + '?' all collapse to
        // dashes).
        assert_eq!(topic_slug("FAQ's & billing?"), "faq-s-billing");
    }

    #[test]
    fn slug_module_compiles() {
        // Phase 0 placeholder kept as a
        // regression guard: the module
        // exists and the public surface
        // compiles. Phase 1 added the
        // per-rule tests above; this
        // exists only to keep the
        // Phase-0-style "module compiles"
        // assertion in place.
    }
}
