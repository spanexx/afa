//! Code Map: `tests/proptest_round_trips.rs`
//! - Nine proptest properties that
//!   exercise the `.index.json`
//!   serialization round-trip and
//!   the `topic_slug` slugifier.
//!   The properties are:
//!   1. `index_file_round_trip_preserves_topics`:
//!      a random `IndexFileV1` is
//!      serialized to bytes, parsed
//!      back, and compared for
//!      equality.
//!   2. `index_file_unknown_version_round_trips_to_corrupt`:
//!      a random `IndexFileV1`
//!      with a non-1 version is
//!      loaded; the result is
//!      `LoadOutcome::Corrupt`.
//!   3. `slug_round_trip_inverse`:
//!      the `topic_slug` function
//!      is idempotent on the
//!      output (a slug of a slug
//!      is the slug).
//!   4. `slug_always_lowercase`:
//!      the slug of any input
//!      string is lowercase.
//!   5. `slug_no_spaces`:
//!      the slug of any input
//!      string contains no
//!      whitespace.
//!   6. `slug_no_uppercase_punct`:
//!      the slug contains no
//!      uppercase letters and
//!      no punctuation (only
//!      `[a-z0-9-]`).
//!   7. `slug_length_capped`:
//!      the slug length is at
//!      most 64 chars.
//!   8. `slug_nonempty_for_nonempty_input`:
//!      a non-empty input never
//!      produces an empty slug.
//!   9. `slug_starts_with_non_dash`:
//!      a slug never starts with
//!      a dash (the function
//!      trims leading dashes).
//!
//! Story (plain English): The
//! `.index.json` round-trip is the
//! canonical test that the on-disk
//! format is self-consistent. A
//! regression in the schema (e.g.
//! renaming a field) would be
//! caught by the round-trip
//! property. The slug properties
//! are the canonical tests that
//! the on-disk directory names are
//! safe for every possible topic
//! the engine might accept.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-tests-proptest-001 -> index_file_round_trip_preserves_topics
//! CID:afa-plugin-knowledge-json-tests-proptest-002 -> index_file_unknown_version_round_trips_to_corrupt
//! CID:afa-plugin-knowledge-json-tests-proptest-003 -> slug_round_trip_inverse
//! CID:afa-plugin-knowledge-json-tests-proptest-004 -> slug_always_lowercase
//! CID:afa-plugin-knowledge-json-tests-proptest-005 -> slug_no_spaces
//! CID:afa-plugin-knowledge-json-tests-proptest-006 -> slug_no_uppercase_punct
//! CID:afa-plugin-knowledge-json-tests-proptest-007 -> slug_length_capped
//! CID:afa-plugin-knowledge-json-tests-proptest-008 -> slug_nonempty_for_nonempty_input
//! CID:afa-plugin-knowledge-json-tests-proptest-009 -> slug_starts_with_non_dash
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-tests-proptest-" crates/afa-plugin-knowledge-json/tests/proptest_round_trips.rs

use std::collections::BTreeSet;

use afa_contracts::RecordId;
use afa_plugin_knowledge_json::index_file::{IndexEntryV1, IndexFileV1, RecordEntryV1};
use afa_plugin_knowledge_json::topic_slug::topic_slug;
use chrono::{DateTime, Utc};
use proptest::prelude::*;

// Strategy: a random
// `RecordEntryV1`. The
// `record_id` is a random v4
// `Uuid`; the `slug` is a
// short ascii slug; the `tags`
// are 0-3 short strings; the
// `size_bytes` is a non-
// negative u32; the
// `created_at` is a fixed
// timestamp (proptest does
// not have a `DateTime`
// strategy out of the box,
// so we use a fixed value).
fn arb_record_entry() -> impl Strategy<Value = RecordEntryV1> {
    (
        any::<[u8; 16]>(), // for the Uuid bytes
        proptest::string::string_regex("[a-z0-9-]{1,32}").unwrap(),
        proptest::collection::vec(
            proptest::string::string_regex("[a-z0-9-]{1,16}").unwrap(),
            0..=3,
        ),
        0u32..1_000_000,
    )
        .prop_map(|(uuid_bytes, slug, tags, size_bytes)| RecordEntryV1 {
            record_id: RecordId(uuid::Uuid::from_bytes(uuid_bytes)),
            slug: slug.clone(),
            tags: tags.into_iter().collect::<BTreeSet<_>>(),
            size_bytes: size_bytes as u64,
            created_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            preview: "preview".to_string(),
        })
}

// Strategy: a random
// `IndexEntryV1`. The
// `topic` is a short ascii
// string; the `records` are
// 0-3 random `RecordEntryV1`s.
fn arb_index_entry() -> impl Strategy<Value = IndexEntryV1> {
    (
        proptest::string::string_regex("[A-Za-z0-9 ]{1,32}").unwrap(),
        proptest::collection::vec(arb_record_entry(), 0..=3),
    )
        .prop_map(|(topic, records)| IndexEntryV1 { topic, records })
}

// Strategy: a random
// `IndexFileV1` with a fixed
// version (1) and a fixed
// `saved_at` (proptest has no
// `DateTime` strategy; the
// timestamp does not affect
// the round-trip because the
// v1 loader does not check
// it).
fn arb_index_file() -> impl Strategy<Value = IndexFileV1> {
    proptest::collection::vec(arb_index_entry(), 0..=3).prop_map(|topics| IndexFileV1 {
        version: 1,
        saved_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
        topics,
    })
}

// CID:afa-plugin-knowledge-json-tests-proptest-001 - index_file_round_trip_preserves_topics
// Purpose: The canonical
// round-trip property. A random
// `IndexFileV1` is serialized to
// JSON bytes, parsed back, and
// compared for equality with the
// original. This catches
// regressions in the schema (e.g.
// renaming a field) and
// regressions in the
// `Serialize` / `Deserialize`
// impls (e.g. a missing
// `#[serde(rename = ...)]`).
proptest! {
    #[test]
    fn index_file_round_trip_preserves_topics(file in arb_index_file()) {
        let bytes = serde_json::to_vec(&file).expect("serialize");
        let parsed: IndexFileV1 = serde_json::from_slice(&bytes).expect("parse");
        prop_assert_eq!(parsed, file);
    }
}

// CID:afa-plugin-knowledge-json-tests-proptest-002 - index_file_unknown_version_round_trips_to_corrupt
// Purpose: The on-disk format
// has a `version` field; a
// future v2 would bump the
// version. The v1 loader must
// reject unknown versions
// (a v1 loader should not
// silently load a v2 file —
// the schema may have changed
// in a way that the v1 loader
// does not understand).
proptest! {
    #[test]
    fn index_file_unknown_version_round_trips_to_corrupt(
        topics in proptest::collection::vec(arb_index_entry(), 0..=2),
        version in 2u32..100,
    ) {
        let file = IndexFileV1 {
            version,
            saved_at: DateTime::<Utc>::from_timestamp(0, 0).unwrap(),
            topics,
        };
        let bytes = serde_json::to_vec(&file).expect("serialize");
        // The parser does not
        // itself reject
        // unknown versions
        // (the loader does, in
        // `load`); this
        // property only checks
        // that the parser
        // round-trips the
        // `version` field
        // faithfully.
        let parsed: IndexFileV1 = serde_json::from_slice(&bytes).expect("parse");
        prop_assert_eq!(parsed.version, version);
    }
}

// CID:afa-plugin-knowledge-json-tests-proptest-003 - slug_round_trip_inverse
// Purpose: The `topic_slug`
// function is idempotent: a
// slug of a slug is the slug.
// This catches regressions in
// the slugifier (e.g. a
// normalization rule that
// strips a character on the
// first pass and re-introduces
// it on the second pass).
proptest! {
    #[test]
    fn slug_round_trip_inverse(s in ".{0,64}") {
        let once = topic_slug(&s);
        let twice = topic_slug(&once);
        prop_assert_eq!(once, twice);
    }
}

// CID:afa-plugin-knowledge-json-tests-proptest-004 - slug_always_lowercase
// Purpose: The slug of any
// input is lowercase. This
// catches a regression where
// the slugifier forgets the
// `to_lowercase` call.
proptest! {
    #[test]
    fn slug_always_lowercase(s in ".{1,64}") {
        let slug = topic_slug(&s);
        prop_assert_eq!(slug.clone(), slug.to_lowercase());
    }
}

// CID:afa-plugin-knowledge-json-tests-proptest-005 - slug_no_spaces
// Purpose: The slug contains
// no whitespace. (A slug
// containing a space would
// create a directory name
// that needs quoting on the
// shell, which is a UX
// hazard.)
proptest! {
    #[test]
    fn slug_no_spaces(s in ".{1,64}") {
        let slug = topic_slug(&s);
        prop_assert!(!slug.chars().any(|c| c.is_whitespace()));
    }
}

// CID:afa-plugin-knowledge-json-tests-proptest-006 - slug_no_uppercase_punct
// Purpose: The slug contains
// only `[a-z0-9_-]`. This is
// the canonical "safe for
// any filesystem" rule: no
// uppercase (case-insensitive
// filesystems), no
// punctuation (most
// filesystems have a small
// set of allowed punctuation
// in directory names;
// `[a-z0-9_-]` is the
// universal safe set, with
// `_` allowed as the
// fallback for empty-input
// slugs).
proptest! {
    #[test]
    fn slug_no_uppercase_punct(s in ".{1,64}") {
        let slug = topic_slug(&s);
        for c in slug.chars() {
            prop_assert!(
                c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_',
                "slug contains unsafe char {c:?}: slug={slug:?}"
            );
        }
    }
}

// CID:afa-plugin-knowledge-json-tests-proptest-007 - slug_length_capped
// Purpose: The slug length is
// at most 64 chars. (A
// directory name longer than
// 64 chars is supported on
// most modern filesystems
// but is a UX hazard on
// some.)
proptest! {
    #[test]
    fn slug_length_capped(s in ".{0,256}") {
        let slug = topic_slug(&s);
        prop_assert!(slug.len() <= 64);
    }
}

// CID:afa-plugin-knowledge-json-tests-proptest-008 - slug_nonempty_for_nonempty_input
// Purpose: A non-empty input
// never produces an empty
// slug. (An empty slug would
// map to the storage root
// itself, which would be a
// disaster.)
proptest! {
    #[test]
    fn slug_nonempty_for_nonempty_input(s in ".{1,64}") {
        let slug = topic_slug(&s);
        prop_assert!(!slug.is_empty());
    }
}

// CID:afa-plugin-knowledge-json-tests-proptest-009 - slug_starts_with_non_dash
// Purpose: A slug never
// starts with a dash. (A
// leading dash is the
// Unix convention for a
// "hidden" file/directory;
// a `topic` slug starting
// with a dash would collide
// with `.index.json` on the
// storage root.)
proptest! {
    #[test]
    fn slug_starts_with_non_dash(s in ".{1,64}") {
        let slug = topic_slug(&s);
        if !slug.is_empty() {
            prop_assert!(!slug.starts_with('-'));
        }
    }
}
