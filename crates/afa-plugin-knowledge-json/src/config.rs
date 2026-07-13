//! Code Map: JsonKnowledgeConfig
//! - `JsonKnowledgeConfig`: The settings card the
//!   `JsonKnowledgeAdapter` is built with. Holds
//!   the on-disk `storage_root` (the directory
//!   under which the adapter writes one
//!   `<topic_slug>/<record_id>.md` per record +
//!   one `.index.json` per storage root) and the
//!   `KnowledgeCapabilities` (the locked shape
//!   the adapter reports via
//!   `describe_capabilities`).
//!
//! Story (plain English): The config is the small
//! settings card the adapter is built with. It
//! answers two questions: "where on disk do I
//! file things?" (`storage_root`) and "what
//! should I tell callers I can do?"
//! (`capabilities`). Both fields are decided at
//! adapter construction and never change for the
//! process lifetime.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-config-001 -> JsonKnowledgeConfig
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-config-" crates/afa-plugin-knowledge-json/src/config.rs

use std::path::PathBuf;

use afa_contracts::KnowledgeCapabilities;

// CID:afa-plugin-knowledge-json-config-001 - JsonKnowledgeConfig
// Purpose: The Phase 1 concrete config for the JSON
// adapter. Two fields: `storage_root: PathBuf` (the
// directory the adapter writes one
// `<topic_slug>/<record_id>.md` per record + one
// `.index.json` per storage root under) and
// `capabilities: KnowledgeCapabilities` (the
// locked shape the adapter reports via
// `describe_capabilities`). The config is the
// "settings card" the operator hands to the
// adapter at boot time; the adapter never
// modifies the config after construction.
// Uses: afa_contracts::KnowledgeCapabilities.
// Used by: `JsonKnowledgeAdapter::new` (the
// canonical place the adapter is built from
// a `JsonKnowledgeConfig`); the kernel's
// bootstrap path that constructs the adapter
// from the Configuration Layer.
pub struct JsonKnowledgeConfig {
    /// The on-disk root directory. The adapter
    /// creates this directory via
    /// `tokio::fs::create_dir_all` if it does
    /// not exist (the "first boot" case). All
    /// per-topic subdirectories and the
    /// `.index.json` file live under this
    /// root.
    pub storage_root: PathBuf,
    /// The static capabilities the adapter
    /// reports via `describe_capabilities`. The
    /// JSON v1 adapter sets the three fields
    /// per the PRD: 1 MB max record size, no
    /// semantic search, no hierarchical topics.
    /// The constructor does NOT validate the
    /// values; the caller is responsible for
    /// providing a sensible
    /// `KnowledgeCapabilities` (a future
    /// tightening will validate
    /// `max_record_size_bytes > 0`, but the
    /// current shape is permissive to keep the
    /// boot path simple).
    pub capabilities: KnowledgeCapabilities,
}

impl JsonKnowledgeConfig {
    /// Build a new `JsonKnowledgeConfig` from
    /// the two required fields. The
    /// constructor is a plain initializer —
    /// no I/O, no validation. The boot
    /// sequence (`JsonKnowledgeAdapter::new`)
    /// is the place that verifies
    /// `storage_root` exists and is writable.
    /// **Call pattern**:
    /// `JsonKnowledgeConfig::new(
    ///   storage_root,
    ///   KnowledgeCapabilities { ... }
    /// )`.
    pub fn new(storage_root: PathBuf, capabilities: KnowledgeCapabilities) -> Self {
        Self {
            storage_root,
            capabilities,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn config_carries_storage_root_and_capabilities() {
        // The two fields are the locked shape
        // from the TRD. A future contributor
        // who drops one would be forced to
        // update this test.
        let cfg = JsonKnowledgeConfig::new(
            PathBuf::from("/var/lib/afa/knowledge"),
            KnowledgeCapabilities {
                max_record_size_bytes: 1_048_576,
                supports_semantic_search: false,
                supports_hierarchical_topics: false,
            },
        );
        assert_eq!(cfg.storage_root, PathBuf::from("/var/lib/afa/knowledge"));
        assert_eq!(cfg.capabilities.max_record_size_bytes, 1_048_576);
        assert!(!cfg.capabilities.supports_semantic_search);
        assert!(!cfg.capabilities.supports_hierarchical_topics);
    }
}
