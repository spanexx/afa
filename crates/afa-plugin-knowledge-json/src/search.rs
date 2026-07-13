//! Code Map: search module
//! - `tokenize`: The pure function that
//!   turns a chunk of text into the
//!   set of tokens used for free-text
//!   scoring. The rules: lowercase,
//!   split on whitespace and ASCII
//!   punctuation, drop tokens shorter
//!   than 2 characters, drop pure-digit
//!   tokens (the engine does not score
//!   on numbers in v1).
//! - `score_candidate`: The pure
//!   function that scores one record
//!   against one request. Returns a
//!   `f32` in `[0.0, 1.0]`. The
//!   formula is the locked shape from
//!   the TRD §3.4:
//!   `0.4 * tag_overlap
//!   + 0.4 * free_text_token_overlap
//!   + 0.2 * topic_match`.
//! - `filter_and_score`: The
//!   orchestrator. The Phase 2 entry
//!   point. Takes the index, the
//!   request, and a content-loader
//!   closure (the adapter provides
//!   the closure that reads the
//!   on-disk file and caches the
//!   tokenized content in the
//!   index). Returns the
//!   `Vec<(RecordId, f32)>` the
//!   adapter turns into a
//!   `FindInformationResponse`.
//!
//! Story (plain English): The search
//! module is the part of the adapter
//! that figures out which records match
//! the patron's question. The patron
//! asks "cancellation policy" with the
//! "billing" tag, in topic "FAQ"; the
//! search module tokenizes the patron's
//! words, looks at the index, and
//! returns the cards that best match.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-search-001 -> tokenize
//! CID:afa-plugin-knowledge-json-search-002 -> score_candidate
//! CID:afa-plugin-knowledge-json-search-003 -> filter_and_score
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-search-" crates/afa-plugin-knowledge-json/src/search.rs

use std::collections::BTreeSet;

use afa_contracts::{FindInformationRequest, RecordId};

use crate::index::RecordMeta;

// CID:afa-plugin-knowledge-json-search-001 - tokenize
// Purpose: The pure function that turns
// a chunk of text into the set of
// tokens used for free-text scoring.
// Rules (per the TRD §3.4):
// 1. Lowercase the entire string.
// 2. Split on whitespace + ASCII
//    punctuation (the
//    `is_ascii_punctuation` +
//    `is_whitespace` check).
// 3. Drop tokens shorter than 2
//    characters (the engine does not
//    score on single letters; "a",
//    "I", etc. add noise).
// 4. Drop pure-digit tokens (the
//    engine does not score on numbers
//    in v1; the design says "tokens
//    are words, not numbers").
//
// The function is pure: no I/O, no
// allocation beyond the returned
// `BTreeSet`, no side effects. The
// same input always produces the same
// output, which is essential for the
// "score is deterministic" property
// the audit trail relies on.
//
// **Call pattern**:
// `tokenize("Hello, World!")` returns
// `{hello, world}`. `tokenize("I have
// $100")` returns `{have}` (the
// "I" is dropped for length; the
// "100" is dropped for being all
// digits; the "$" is dropped as
// punctuation).
pub fn tokenize(text: &str) -> BTreeSet<String> {
    let lower = text.to_ascii_lowercase();
    let mut tokens: BTreeSet<String> = BTreeSet::new();
    let mut current = String::new();
    for c in lower.chars() {
        if c.is_whitespace() || c.is_ascii_punctuation() {
            if !current.is_empty() {
                push_token(&mut tokens, std::mem::take(&mut current));
            }
        } else {
            current.push(c);
        }
    }
    if !current.is_empty() {
        push_token(&mut tokens, current);
    }
    tokens
}

/// Helper: apply the length + digit
/// filters and push a token onto the
/// set. The filter is:
/// - `len() >= 2` (drop short tokens).
/// - Not all ASCII digits (drop pure-
///   digit tokens; "abc123" passes,
///   "123" does not).
fn push_token(set: &mut BTreeSet<String>, token: String) {
    if token.len() < 2 {
        return;
    }
    if token.chars().all(|c| c.is_ascii_digit()) {
        return;
    }
    set.insert(token);
}

// CID:afa-plugin-knowledge-json-search-002 - score_candidate
// Purpose: Score one record against one
// request. Returns a `f32` in `[0.0,
// 1.0]`. The formula is the locked
// shape from the TRD §3.4:
// `0.4 * tag_overlap
//  + 0.4 * free_text_token_overlap
//  + 0.2 * topic_match`.
//
// Component meanings:
// - `tag_overlap`:
//   `|record.tags ∩ request.tags| / |request.tags|`
//   when `request.tags` is non-empty;
//   `1.0` when `request.tags` is empty
//   (no filter → no penalty).
// - `free_text_token_overlap`:
//   `|record_tokens ∩ query_tokens| / |query_tokens|`
//   when the query has tokens;
//   `1.0` when the query is empty
//   (no filter → no penalty).
// - `topic_match`:
//   `1.0` if the request's `topic`
//   matches the record's `topic`;
//   `1.0` if the request's `topic` is
//   `None` (no filter); `0.0` if the
//   request specifies a topic and the
//   record is in a different one.
//   (Note: the topic filter is also
//   applied upstream as a hard filter
//   by `filter_and_score`; this
//   component is a "bonus" for
//   matching the topic name verbatim
//   on a partial filter — e.g., the
//   no-topic-filter case can still
//   have its topic component set.)
//
// The function is pure: no I/O, no
// side effects. The caller passes the
// record's `content_tokens` (a
// pre-tokenized set); the function
// does not re-tokenize.
pub fn score_candidate(
    request: &FindInformationRequest,
    record: &RecordMeta,
    content_tokens: &BTreeSet<String>,
) -> f32 {
    let tag_overlap = if request.tags.is_empty() {
        1.0
    } else {
        let request_tags: BTreeSet<String> = request.tags.iter().cloned().collect();
        let intersection: BTreeSet<&String> = record.tags.intersection(&request_tags).collect();
        intersection.len() as f32 / request.tags.len() as f32
    };

    let free_text_token_overlap = if let Some(query) = &request.free_text {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            // The query was non-empty
            // but tokenized to
            // nothing (e.g. "I ?" or
            // a punctuation-only
            // string). Treat as "no
            // free-text filter" rather
            // than a 0.0 score.
            1.0
        } else {
            let intersection: BTreeSet<&String> =
                content_tokens.intersection(&query_tokens).collect();
            intersection.len() as f32 / query_tokens.len() as f32
        }
    } else {
        1.0
    };

    let topic_match = match &request.topic {
        None => 1.0,
        Some(req_topic) if req_topic == &record.topic => 1.0,
        Some(_) => 0.0,
    };

    0.4 * tag_overlap + 0.4 * free_text_token_overlap + 0.2 * topic_match
}

// CID:afa-plugin-knowledge-json-search-003 - filter_and_score
// Purpose: The orchestrator. The Phase 2
// entry point. Takes the index, the
// request, and a `content_loader`
// closure (the adapter provides the
// closure that reads the on-disk file
// and caches the tokenized content in
// the index). Returns the
// `Vec<(RecordId, f32)>` the adapter
// turns into a
// `FindInformationResponse`.
//
// The orchestrator does four things:
// 1. **Filter** — apply the topic
//    filter (None = all topics) and
//    the tag-AND filter (empty = no
//    filter).
// 2. **Load content** — for each
//    candidate record that has a
//    non-empty `free_text` to score,
//    invoke the `content_loader` to
//    get the tokenized content (the
//    loader reads the file from disk
//    on cache miss and caches the
//    tokens in the index).
// 3. **Score** — call
//    `score_candidate` for each
//    candidate.
// 4. **Sort + filter** — drop the
//    zero-score records (records that
//    matched no positive signal),
//    sort by descending score, return
//    the full list (the adapter
//    applies the `limit` and assembles
//    the `KnowledgeRecord` bodies).
//
// The function is async (the
// `content_loader` is async — file
// reads are async). The signature
// takes `&InMemoryIndex` (the adapter
// already holds the read lock when
// it calls this).
pub async fn filter_and_score<F, Fut>(
    index: &crate::InMemoryIndex,
    request: &FindInformationRequest,
    content_loader: F,
) -> Vec<(RecordId, f32)>
where
    F: FnMut(RecordId) -> Fut,
    Fut: std::future::Future<Output = ()>,
{
    // Step 1: filter by topic.
    let topic_filtered: Vec<(RecordId, RecordMeta)> = match &request.topic {
        Some(req_topic) => {
            let slug = crate::topic_slug::topic_slug(req_topic);
            // If the request's topic
            // does not exist in the
            // index, the result is
            // empty (the `records_in_topic`
            // for an unknown slug is
            // empty). We do NOT fall
            // back to "all topics" —
            // the topic filter is a
            // hard filter.
            index.records_in_topic(&slug)
        }
        None => index.all_records(),
    };

    // Step 2: filter by tag AND-set.
    let tag_filtered: Vec<(RecordId, RecordMeta)> = if request.tags.is_empty() {
        topic_filtered
    } else {
        // Dedup the request tags
        // (an AND-filter of
        // ["billing", "billing"] is
        // the same as
        // ["billing"]).
        let mut dedup: BTreeSet<String> = BTreeSet::new();
        for t in &request.tags {
            dedup.insert(t.clone());
        }
        let allowed = index.records_with_all_tags(&dedup.iter().cloned().collect::<Vec<_>>());
        topic_filtered
            .into_iter()
            .filter(|(rid, _)| allowed.contains(rid))
            .collect()
    };

    // Step 3: score. We need the
    // content tokens for any record
    // where the free-text filter is
    // set. If `free_text` is None,
    // we can skip the loader
    // entirely and pass an empty
    // token set (the scoring
    // function uses the
    // `free_text_token_overlap =
    // 1.0` path on an empty query).
    let needs_content_tokens = request.free_text.is_some();
    let mut content_loader = content_loader;

    // If we need tokens, walk the
    // candidates and call the
    // loader for each one. The
    // loader is responsible for
    // caching the tokens in the
    // index; this function does
    // not touch the index's
    // `content_tokens` directly
    // (the lock is held by the
    // adapter). We collect the
    // token sets into a local
    // map so the score step can
    // look them up.
    let mut local_tokens: std::collections::HashMap<RecordId, BTreeSet<String>> =
        std::collections::HashMap::new();
    if needs_content_tokens {
        for (rid, _meta) in &tag_filtered {
            // Check the cache first.
            if let Some(cached) = index.content_tokens.get(rid) {
                local_tokens.insert(*rid, cached.clone());
            } else {
                // Cache miss: invoke
                // the loader. The
                // loader reads the
                // file from disk
                // and stores the
                // tokens in the
                // index's
                // `content_tokens`
                // (the adapter's
                // closure does
                // this). After
                // the await, the
                // cache should
                // have the entry.
                content_loader(*rid).await;
                if let Some(cached) = index.content_tokens.get(rid) {
                    local_tokens.insert(*rid, cached.clone());
                }
            }
        }
    }

    // Step 4: score + sort + drop zeros.
    let mut scored: Vec<(RecordId, f32)> = tag_filtered
        .into_iter()
        .map(|(rid, meta)| {
            let tokens = local_tokens.get(&rid).cloned().unwrap_or_default();
            (rid, score_candidate(request, &meta, &tokens))
        })
        .filter(|(_, s)| *s > 0.0)
        .collect();
    // Stable sort by descending
    // score; ties broken by
    // `created_at` descending
    // (newer first), then by
    // `RecordId`'s inner `Uuid`
    // for determinism (`RecordId`
    // does not implement `Ord`;
    // the inner `Uuid` does).
    scored.sort_by(|a, b| {
        b.1.partial_cmp(&a.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                // Lookup the
                // created_at
                // from the
                // index. We
                // do a
                // linear
                // scan over
                // the
                // topic
                // entries
                // here; for
                // v1 the
                // candidate
                // set is
                // small
                // enough
                // that this
                // is fine
                // (O(n) in
                // the
                // candidate
                // count).
                let a_meta = lookup_meta(index, &a.0);
                let b_meta = lookup_meta(index, &b.0);
                b_meta
                    .created_at
                    .cmp(&a_meta.created_at)
                    .then(a.0 .0.cmp(&b.0 .0))
            })
    });
    scored
}

/// Helper: linear scan to find the
/// `RecordMeta` for `rid`. Used only
/// in the sort step (the candidate
/// set is small; O(n) is fine).
fn lookup_meta(index: &crate::InMemoryIndex, rid: &RecordId) -> RecordMeta {
    for entry in index.topics.values() {
        if let Some(m) = entry.records.get(rid) {
            return m.clone();
        }
    }
    // The `score_candidate` path
    // already filtered to known
    // records; reaching here
    // indicates an invariant
    // violation. Return a
    // placeholder (created_at =
    // epoch, everything else
    // empty) so the sort
    // step does not panic. The
    // adapter's `find_information`
    // maps an invariant violation
    // to `KnowledgeErrorV1::Internal`.
    RecordMeta {
        record_id: *rid,
        topic: String::new(),
        tags: BTreeSet::new(),
        size_bytes: 0,
        created_at: chrono::DateTime::from_timestamp(0, 0).unwrap(),
        preview: String::new(),
        slug: String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::index::RecordMeta;
    use chrono::{TimeZone, Utc};

    fn meta_with_tags_and_content(
        topic: &str,
        tags: &[&str],
        content_tokens: &[&str],
    ) -> RecordMeta {
        let mut set: BTreeSet<String> = BTreeSet::new();
        for t in tags {
            set.insert((*t).to_string());
        }
        let mut content_set: BTreeSet<String> = BTreeSet::new();
        for t in content_tokens {
            content_set.insert((*t).to_string());
        }
        RecordMeta {
            record_id: RecordId::new(),
            topic: topic.to_string(),
            tags: set,
            size_bytes: 0,
            created_at: Utc.with_ymd_and_hms(2026, 7, 13, 0, 0, 0).unwrap(),
            preview: String::new(),
            slug: String::new(),
            // Note: `RecordMeta` does
            // not own the content
            // tokens; the test
            // passes them in
            // separately.
        }
    }

    #[test]
    fn tokenize_lowercases_and_splits_on_punctuation() {
        let t = tokenize("Hello, World!");
        assert!(t.contains("hello"));
        assert!(t.contains("world"));
        assert_eq!(t.len(), 2);
    }

    #[test]
    fn tokenize_drops_short_tokens() {
        let t = tokenize("I have a cat");
        // "I" and "a" are dropped
        // for being < 2 chars.
        assert!(t.contains("have"));
        assert!(t.contains("cat"));
        assert!(!t.contains("i"));
        assert!(!t.contains("a"));
    }

    #[test]
    fn tokenize_drops_pure_digit_tokens() {
        let t = tokenize("I have $100 in account 12345");
        // "100" and "12345" are
        // dropped for being all
        // digits; "$" is dropped
        // as punctuation.
        assert!(t.contains("have"));
        assert!(t.contains("account"));
        assert!(!t.contains("100"));
        assert!(!t.contains("12345"));
    }

    #[test]
    fn tokenize_handles_mixed_punctuation() {
        let t = tokenize("Cancellation? Refunds, yes!");
        assert!(t.contains("cancellation"));
        assert!(t.contains("refunds"));
        assert!(t.contains("yes"));
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn tokenize_empty_string_returns_empty() {
        let t = tokenize("");
        assert!(t.is_empty());
    }

    #[test]
    fn score_candidate_no_filters_returns_one() {
        // A request with no
        // filters and an empty
        // content-token set
        // (because the score
        // function does not
        // consult content when
        // `free_text` is None)
        // scores 1.0 on every
        // record.
        let m = meta_with_tags_and_content("FAQ", &[], &[]);
        let req = FindInformationRequest::default();
        let s = score_candidate(&req, &m, &BTreeSet::new());
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn score_candidate_topic_match_weight() {
        // A record that matches
        // the requested topic
        // gets the full 0.2
        // topic weight; a record
        // that does not gets 0
        // for the topic
        // component.
        let m = meta_with_tags_and_content("FAQ", &[], &[]);
        let req_match = FindInformationRequest {
            topic: Some("FAQ".to_string()),
            ..Default::default()
        };
        let req_miss = FindInformationRequest {
            topic: Some("Other".to_string()),
            ..Default::default()
        };
        let s_match = score_candidate(&req_match, &m, &BTreeSet::new());
        let s_miss = score_candidate(&req_miss, &m, &BTreeSet::new());
        assert!((s_match - 1.0).abs() < 1e-6);
        assert!((s_miss - 0.8).abs() < 1e-6);
    }

    #[test]
    fn score_candidate_tag_overlap_weight() {
        // A request with one tag
        // and a record with the
        // same tag gets 0.4
        // from the tag
        // component.
        let m = meta_with_tags_and_content("FAQ", &["billing"], &[]);
        let req = FindInformationRequest {
            tags: vec!["billing".to_string()],
            ..Default::default()
        };
        let s = score_candidate(&req, &m, &BTreeSet::new());
        assert!((s - 1.0).abs() < 1e-6);
    }

    #[test]
    fn score_candidate_tag_partial_overlap() {
        // A request with two
        // tags, a record with
        // one of them: 0.5
        // overlap → 0.4 * 0.5
        // = 0.2 from the tag
        // component.
        let m = meta_with_tags_and_content("FAQ", &["billing"], &[]);
        let req = FindInformationRequest {
            tags: vec!["billing".to_string(), "refund".to_string()],
            ..Default::default()
        };
        let s = score_candidate(&req, &m, &BTreeSet::new());
        // 0.2 (tag overlap 1/2)
        // + 0.4 (no free-text
        // filter → 1.0)
        // + 0.2 (no topic
        // filter → 1.0)
        // = 0.8
        assert!((s - 0.8).abs() < 1e-6);
    }

    #[test]
    fn score_candidate_free_text_overlap_weight() {
        // A request with a
        // free-text query that
        // matches half the
        // tokens: 0.5
        // overlap → 0.4 * 0.5
        // = 0.2 from the
        // free-text component.
        let m = meta_with_tags_and_content("FAQ", &[], &["refund", "policy"]);
        let content_tokens: BTreeSet<String> =
            ["refund", "policy"].iter().map(|s| s.to_string()).collect();
        let req = FindInformationRequest {
            free_text: Some("refund policy something".to_string()),
            ..Default::default()
        };
        let s = score_candidate(&req, &m, &content_tokens);
        // 0.4 (2/2 overlap) +
        // 0.2 (topic match) = 0.6
        // Wait, "refund policy
        // something" tokenizes
        // to {refund, policy,
        // something} and the
        // record has
        // {refund, policy}.
        // That's 2/3 overlap.
        // 0.4 * 2/3 = 0.2666
        // 0.4 + 0.2666 + 0.2 =
        // 0.8666
        assert!((s - 0.8666).abs() < 0.01);
    }

    #[test]
    fn score_candidate_free_text_no_overlap_is_zero() {
        // A record with no
        // matching tokens
        // scores 0 on the
        // free-text
        // component; the
        // total is 0.6
        // (tags empty →
        // 1.0; topic match
        // → 0.2; free text
        // → 0).
        let m = meta_with_tags_and_content("FAQ", &[], &["shipping"]);
        let content_tokens: BTreeSet<String> = ["shipping"].iter().map(|s| s.to_string()).collect();
        let req = FindInformationRequest {
            free_text: Some("refund".to_string()),
            ..Default::default()
        };
        let s = score_candidate(&req, &m, &content_tokens);
        // 0.4 (tag empty → 1.0)
        // + 0.0 (free-text no
        // overlap) + 0.2
        // (topic match) = 0.6
        assert!((s - 0.6).abs() < 1e-6);
    }
}
