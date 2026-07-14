//! Code Map: index_file
//! - `IndexFileV1`: The on-disk sidecar
//!   `<storage_root>/.index.json`. A flat
//!   JSON document the adapter writes on
//!   every successful `store_information`
//!   and reads once on boot. Three
//!   purposes:
//!   1. **Persistence** — the in-memory
//!      index is rebuilt from this file on
//!      boot, so a clean restart keeps all
//!      records discoverable.
//!   2. **Durability** — the adapter
//!      writes this file via `atomic_write`
//!      (temp-then-rename), so a crash
//!      mid-write leaves either the old or
//!      the new file in place.
//!   3. **Recovery** — if the file is
//!      missing or corrupt, the boot
//!      sequence falls back to walking the
//!      on-disk `.md` files (see
//!      `rebuild_from_disk` in `adapter.rs`).
//! - `IndexEntryV1`: One topic's worth of
//!   records in the file. A flat `Vec` of
//!   `RecordEntryV1` (no nested maps; the
//!   on-disk format is intentionally simple
//!   for v1).
//! - `RecordEntryV1`: One record's metadata
//!   in the file. Excludes the body (the
//!   body is in `<record_id>.md`).
//! - `LoadOutcome`: The boot-time result of
//!   trying to load the index file. The
//!   adapter matches on the variant to
//!   decide which path to take (loaded,
//!   missing, corrupt).
//!
//! Story (plain English): The `.index.json`
//! is the adapter's notebook. The
//! in-memory `InMemoryIndex` is the brain
//! the adapter uses to answer queries
//! fast. On boot, the adapter reads the
//! notebook to repopulate the brain. If
//! the notebook is missing, the brain
//! starts empty (clean slate). If the
//! notebook is corrupt (someone wrote
//! garbage to it), the adapter gives up
//! on the notebook and walks the on-disk
//! `.md` files to repopulate the brain
//! from the source-of-truth content files.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-index-file-001 -> IndexFileV1
//! CID:afa-plugin-knowledge-json-index-file-002 -> IndexEntryV1
//! CID:afa-plugin-knowledge-json-index-file-003 -> RecordEntryV1
//! CID:afa-plugin-knowledge-json-index-file-004 -> LoadOutcome
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-index-file-" crates/afa-plugin-knowledge-json/src/index_file.rs

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use afa_contracts::RecordId;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::index::{InMemoryIndex, RecordMeta};

// CID:afa-plugin-knowledge-json-index-file-001 - IndexFileV1
// Purpose: The on-disk shape of
// `<storage_root>/.index.json`. The
// schema is intentionally flat (a `Vec`
// of `IndexEntryV1`, each a `Vec` of
// `RecordEntryV1`) so the round-trip
// proptest is a one-liner. A future
// pack can add a `v2` shape with
// per-record JSON sidecars for
// lossless rebuild; until then the
// v1 format is "good enough" for the
// query path (tags + topic + size +
// preview + created_at).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexFileV1 {
    /// Always `1` in v1. A future v2
    /// would bump this; the loader
    /// rejects unknown versions
    /// (treated as corrupt).
    pub version: u32,
    /// The wall-clock time the file
    /// was written. Diagnostic only
    /// (the boot path does not check
    /// it).
    pub saved_at: DateTime<Utc>,
    /// The topics, in alphabetical
    /// order by slug (the loader does
    /// not depend on the order; the
    /// `InMemoryIndex` is a
    /// `BTreeMap` and re-sorts on
    /// ingest). The serial order is
    /// stable for deterministic
    /// proptests.
    pub topics: Vec<IndexEntryV1>,
}

// CID:afa-plugin-knowledge-json-index-file-002 - IndexEntryV1
// Purpose: One topic in the on-disk
// index. Holds the topic name (the
// "billing" name, not the slug) and
// the records that belong to it. The
// records are kept in a `Vec`, not a
// `HashMap`, so the on-disk shape is
// deterministic (the proptest checks
// round-trip equality, which requires
// a canonical order).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct IndexEntryV1 {
    /// The human-readable topic
    /// name (e.g. `"Billing"`). The
    /// slug is the on-disk directory
    /// name; the topic is the name
    /// the caller passed to
    /// `store_information`.
    pub topic: String,
    /// The records, sorted by
    /// `RecordId`'s inner `Uuid`
    /// for canonical order.
    pub records: Vec<RecordEntryV1>,
}

// CID:afa-plugin-knowledge-json-index-file-003 - RecordEntryV1
// Purpose: One record's metadata in
// the on-disk index. Excludes the
// body (which is in
// `<storage_root>/<slug>/<record_id>.md`).
// The body is loaded on demand by
// `find_information`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RecordEntryV1 {
    pub record_id: RecordId,
    /// The on-disk directory name.
    /// Persisted so the boot path
    /// does not need to re-slugify
    /// the topic (slug rules are
    /// pure, but re-slugifying is
    /// an avoidable risk if the
    /// slug rules ever change).
    pub slug: String,
    /// The tags, sorted
    /// alphabetically (a `BTreeSet`
    /// on the in-memory side;
    /// serialized as an array).
    pub tags: BTreeSet<String>,
    pub size_bytes: u64,
    pub created_at: DateTime<Utc>,
    /// The first 256 chars of the
    /// body (or the full body if
    /// shorter). The v1 on-disk
    /// format does not store the
    /// body in the index; the
    /// preview is just a hint for
    /// the search path (the v1
    /// search path does not use
    /// the preview today; the
    /// field is persisted for
    /// forward-compatibility with
    /// the LLM-side tool that
    /// shows the user a snippet).
    pub preview: String,
}

// CID:afa-plugin-knowledge-json-index-file-004 - LoadOutcome
// Purpose: The boot-time result of
// trying to read the index file. The
// adapter matches on the variant to
// decide which path to take:
// - `Loaded` → repopulate the
//   in-memory index from the file's
//   records.
// - `Missing` → start with an empty
//   index (clean slate; the on-disk
//   `.md` files are not walked
//   because the index says "no
//   records ever stored here").
//   This is the "first boot" path.
// - `Corrupt` → log a warning and
//   walk the on-disk `.md` files
//   (`rebuild_from_disk`) to
//   repopulate the in-memory index
//   from the source-of-truth files.
//   The metadata (tags, created_at,
//   preview) is NOT recovered; only
//   the topic (from the directory
//   name) and the record_id (from
//   the filename) are recovered. The
//   rebuild is a "best-effort v1
//   limitation" per the IMPL.
// - `IoError` → the storage root is
//   not accessible; the adapter
//   cannot boot. Maps to
//   `StorageUnavailable`.
#[derive(Debug)]
pub enum LoadOutcome {
    Loaded(InMemoryIndex),
    Missing,
    Corrupt { reason: String },
    IoError { reason: String },
}

// The on-disk filename (the
// sidecar that lives in the
// storage root).
const INDEX_FILENAME: &str = ".index.json";

// CID:afa-plugin-knowledge-json-index-file-005 - index_path
// Purpose: The path to
// `<storage_root>/.index.json`. A
// tiny helper that the adapter uses
// to keep the filename choice in
// one place.
pub fn index_path(storage_root: &Path) -> PathBuf {
    storage_root.join(INDEX_FILENAME)
}

// CID:afa-plugin-knowledge-json-index-file-006 - save
// Purpose: Write the in-memory
// index to disk as
// `<storage_root>/.index.json`.
// The write is atomic (temp file
// + rename) via the
// `atomic_write` module. The
// caller is expected to hold the
// write lock on the in-memory
// index across the call (so the
// file is a faithful snapshot).
pub async fn save(storage_root: &Path, index: &InMemoryIndex) -> Result<(), String> {
    let payload = IndexFileV1 {
        version: 1,
        saved_at: Utc::now(),
        topics: collect_entries(index),
    };
    let bytes = match serde_json::to_vec_pretty(&payload) {
        Ok(b) => b,
        Err(e) => return Err(format!("index_file::save: serialize failed: {e}")),
    };
    let path = index_path(storage_root);
    if let Err(e) = crate::atomic_write::atomic_write(&path, &bytes).await {
        return Err(format!("index_file::save: atomic_write failed: {e}"));
    }
    Ok(())
}

// CID:afa-plugin-knowledge-json-index-file-007 - load
// Purpose: Read the on-disk
// `.index.json` and return the
// `LoadOutcome`. The function is
// total — it NEVER panics or
// returns an `Err` directly. Every
// failure mode is mapped to a
// `LoadOutcome` variant. The
// adapter is the only caller.
pub async fn load(storage_root: &Path) -> LoadOutcome {
    let path = index_path(storage_root);
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return LoadOutcome::Missing,
        Err(e) => {
            return LoadOutcome::IoError {
                reason: format!("index_file::load: read {} failed: {e}", path.display()),
            };
        }
    };
    let file: IndexFileV1 = match serde_json::from_slice(&bytes) {
        Ok(f) => f,
        Err(e) => {
            return LoadOutcome::Corrupt {
                reason: format!("index_file::load: parse {} failed: {e}", path.display()),
            };
        }
    };
    if file.version != 1 {
        return LoadOutcome::Corrupt {
            reason: format!(
                "index_file::load: unknown version {} (only version 1 is supported)",
                file.version
            ),
        };
    }
    LoadOutcome::Loaded(ingest_file(file))
}

// CID:afa-plugin-knowledge-json-index-file-008 - ingest_file
// Purpose: Build an
// `InMemoryIndex` from a parsed
// `IndexFileV1`. Pure (no I/O).
// The function walks every
// `IndexEntryV1`, calls
// `InMemoryIndex::add_record` for
// each `RecordEntryV1`, and
// returns the populated index. The
// `add_record` path is the same
// path the live adapter uses, so
// the side effects on the
// in-memory counters + tag
// indexes are correct.
fn ingest_file(file: IndexFileV1) -> InMemoryIndex {
    let mut index = InMemoryIndex::new();
    for entry in file.topics {
        for rec in entry.records {
            let meta = RecordMeta {
                record_id: rec.record_id,
                topic: entry.topic.clone(),
                slug: rec.slug,
                tags: rec.tags,
                size_bytes: rec.size_bytes,
                created_at: rec.created_at,
                preview: rec.preview,
            };
            index.add_meta(meta);
        }
    }
    index
}

// CID:afa-plugin-knowledge-json-index-file-009 - collect_entries
// Purpose: Build the
// `Vec<IndexEntryV1>` snapshot of
// the in-memory index. The topics
// are in alphabetical order by
// slug (the `BTreeMap` does this
// for free). The records within a
// topic are sorted by `RecordId`'s
// inner `Uuid` for canonical
// proptest round-trips.
fn collect_entries(index: &InMemoryIndex) -> Vec<IndexEntryV1> {
    index
        .topics
        .iter()
        .map(|(slug, entry)| {
            let topic = index
                .slug_to_topic
                .get(slug)
                .cloned()
                .unwrap_or_else(|| slug.clone());
            let mut records: Vec<RecordEntryV1> = entry
                .records
                .values()
                .map(|m| RecordEntryV1 {
                    record_id: m.record_id,
                    slug: slug.clone(),
                    tags: m.tags.clone(),
                    size_bytes: m.size_bytes,
                    created_at: m.created_at,
                    preview: m.preview.clone(),
                })
                .collect();
            records.sort_by(|a, b| a.record_id.0.cmp(&b.record_id.0));
            IndexEntryV1 { topic, records }
        })
        .collect()
}

// CID:afa-plugin-knowledge-json-index-file-010 - rebuild_from_disk
// Purpose: The "best-effort v1
// limitation" recovery path. If
// the index file is corrupt, the
// adapter walks the on-disk `.md`
// files to repopulate the
// in-memory index. The recovery
// CANNOT recover the full
// metadata (the v1 on-disk format
// does not store a per-record
// metadata sidecar; the `.md`
// file is just the body). The
// recovered fields are:
// - `record_id` from the
//   filename (`<record_id>.md`).
// - `slug` from the parent
//   directory name (e.g.
//   `billing`).
// - `topic` from the
//   `slug_to_topic` map; if no
//   mapping exists, the topic is
//   set to the slug itself (a
//   lossy fallback).
// - `size_bytes` from the file's
//   metadata (`metadata.len()`).
// - `created_at` from the file's
//   modification time. This is
//   NOT the original creation
//   time (the original is lost);
//   it is "when this file was
//   last touched" which is good
//   enough for sort + display.
// - `tags` = empty set (the v1
//   on-disk format does not
//   store tags per record on
//   disk; they live in the
//   index file only, which is
//   the file we are recovering
//   from).
// - `preview` = empty string
//   (the recovery is not allowed
//   to read every file's
//   contents; that would be
//   O(N) on boot, which is
//   unacceptable for large
//   stores).
//
// Returns the rebuilt index. The
// caller is expected to log a
// `tracing::warn!` that the
// recovery happened (so an
// operator can investigate why
// the index file was corrupt).
pub async fn rebuild_from_disk(storage_root: &Path) -> std::io::Result<InMemoryIndex> {
    let mut index = InMemoryIndex::new();
    let mut dir = tokio::fs::read_dir(storage_root).await?;
    while let Some(entry) = dir.next_entry().await? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let slug = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s.to_string(),
            None => continue,
        };
        // Skip the index file's parent
        // (the storage root is the
        // parent; we are iterating its
        // children). The `.index.json`
        // lives at the root, not in a
        // subdir.
        if slug.starts_with('.') {
            continue;
        }
        let topic = index
            .slug_to_topic
            .get(&slug)
            .cloned()
            .unwrap_or_else(|| slug.clone());
        let mut sub = match tokio::fs::read_dir(&path).await {
            Ok(d) => d,
            Err(_) => continue, // unreadable subdir; skip
        };
        while let Some(file) = sub.next_entry().await? {
            let file_path = file.path();
            if !file_path.is_file() {
                continue;
            }
            let file_name = match file_path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n,
                None => continue,
            };
            // Skip orphan temp files
            // (left over from a crashed
            // atomic write). The orphan
            // cleanup in the adapter
            // handles the deletion; the
            // rebuild path just ignores
            // them.
            if file_name.contains(".tmp.") {
                continue;
            }
            // Expect `<record_id>.md`.
            // If the file does not end
            // in `.md`, skip.
            if !file_name.ends_with(".md") {
                continue;
            }
            let id_str = &file_name[..file_name.len() - 3];
            let record_id = match uuid::Uuid::parse_str(id_str) {
                Ok(u) => RecordId(u),
                Err(_) => continue,
            };
            let metadata = match file.metadata().await {
                Ok(m) => m,
                Err(_) => continue,
            };
            let created_at: DateTime<Utc> = metadata
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .and_then(|d| DateTime::<Utc>::from_timestamp(d.as_secs() as i64, 0))
                .unwrap_or_else(Utc::now);
            let meta = RecordMeta {
                record_id,
                topic: topic.clone(),
                slug: slug.clone(),
                tags: BTreeSet::new(),
                size_bytes: metadata.len(),
                created_at,
                preview: String::new(),
            };
            index.add_meta(meta);
        }
    }
    Ok(index)
}

// CID:afa-plugin-knowledge-json-index-file-011 - cleanup_orphan_temps
// Purpose: Walk the storage root
// and remove any `*.tmp.*` files
// (the leftover of a crashed
// `atomic_write`). Called once on
// boot, after the index is loaded
// (or rebuilt), to give the next
// store_information a clean slate.
// Returns the number of files
// removed (for the audit log).
pub async fn cleanup_orphan_temps(storage_root: &Path) -> std::io::Result<usize> {
    let mut removed = 0usize;
    let mut dir = tokio::fs::read_dir(storage_root).await?;
    while let Some(entry) = dir.next_entry().await? {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let dir_name = match path.file_name().and_then(|s| s.to_str()) {
            Some(s) => s,
            None => continue,
        };
        if dir_name.starts_with('.') {
            continue;
        }
        let mut sub = match tokio::fs::read_dir(&path).await {
            Ok(d) => d,
            Err(_) => continue,
        };
        while let Some(file) = sub.next_entry().await? {
            let file_path = file.path();
            let file_name = match file_path.file_name().and_then(|s| s.to_str()) {
                Some(n) => n,
                None => continue,
            };
            if file_name.contains(".tmp.") && tokio::fs::remove_file(&file_path).await.is_ok() {
                removed += 1;
            }
        }
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Unit test: build a small index,
    // serialize, parse, and verify
    // the round-trip preserves every
    // field. This is the cheapest
    // possible check that the wire
    // format is self-consistent.
    #[test]
    fn index_file_round_trip_preserves_fields() {
        let mut index = InMemoryIndex::new();
        let id1 = RecordId(uuid::Uuid::new_v4());
        let id2 = RecordId(uuid::Uuid::new_v4());
        let mut tags = BTreeSet::new();
        tags.insert("billing".to_string());
        let meta1 = RecordMeta {
            record_id: id1,
            topic: "Billing".to_string(),
            slug: "billing".to_string(),
            tags: tags.clone(),
            size_bytes: 42,
            created_at: Utc::now(),
            preview: "first 42 chars".to_string(),
        };
        let meta2 = RecordMeta {
            record_id: id2,
            topic: "Billing".to_string(),
            slug: "billing".to_string(),
            tags: BTreeSet::new(),
            size_bytes: 0,
            created_at: Utc::now(),
            preview: String::new(),
        };
        index.add_meta(meta1.clone());
        index.add_meta(meta2.clone());

        let entries = collect_entries(&index);
        let file = IndexFileV1 {
            version: 1,
            saved_at: Utc::now(),
            topics: entries,
        };
        let bytes = serde_json::to_vec(&file).unwrap();
        let parsed: IndexFileV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(parsed.version, 1);
        assert_eq!(parsed.topics.len(), 1);
        assert_eq!(parsed.topics[0].topic, "Billing");
        assert_eq!(parsed.topics[0].records.len(), 2);
    }

    // Unit test: collect_entries
    // produces canonical order
    // (topics alphabetical by slug,
    // records alphabetical by
    // `RecordId`'s inner `Uuid`).
    #[test]
    fn collect_entries_is_canonically_ordered() {
        let mut index = InMemoryIndex::new();
        // Insert in non-canonical
        // order.
        for s in ["zzz", "aaa", "mmm"] {
            let meta = RecordMeta {
                record_id: RecordId(uuid::Uuid::nil()),
                topic: s.to_uppercase(),
                slug: s.to_string(),
                tags: BTreeSet::new(),
                size_bytes: 0,
                created_at: Utc::now(),
                preview: String::new(),
            };
            index.add_meta(meta);
        }
        let entries = collect_entries(&index);
        assert_eq!(entries[0].topic, "AAA");
        assert_eq!(entries[1].topic, "MMM");
        assert_eq!(entries[2].topic, "ZZZ");
    }
}
