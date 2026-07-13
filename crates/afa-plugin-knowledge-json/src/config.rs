//! Code Map: JsonKnowledgeConfig stub
//!
//! Phase 0 placeholder for the adapter
//! configuration. Phase 1 will replace this
//! with a real struct that holds the
//! `storage_root: PathBuf` and the
//! `capabilities: KnowledgeCapabilities`.
//!
//! Story (plain English): The config is the
//! small settings card the adapter is built
//! with. Phase 0 only needs the type to
//! exist so `JsonKnowledgeAdapter::new` can
//! take it; Phase 1 fills in the real fields.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-config-001 -> JsonKnowledgeConfig
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-config-" crates/afa-plugin-knowledge-json/src/config.rs

// CID:afa-plugin-knowledge-json-config-001 - JsonKnowledgeConfig
// Purpose: The Phase 0 stub for the adapter
// config. Phase 0 only needs the type to
// exist so `JsonKnowledgeAdapter::new` can
// take a `JsonKnowledgeConfig` and the
// `cargo build --workspace` stays green. The
// struct is a unit struct (no fields) for
// now; Phase 1 will replace it with
// `pub struct JsonKnowledgeConfig { pub
// storage_root: PathBuf, pub capabilities:
// KnowledgeCapabilities, }`. Any code that
// names `JsonKnowledgeConfig` today will
// need to be updated in Phase 1, so the
// blast radius is contained to this crate
// (no other crate touches the config).
pub struct JsonKnowledgeConfig;

impl JsonKnowledgeConfig {
    /// Phase 0 placeholder constructor. Phase 1
    /// will: (a) take the `storage_root: PathBuf`
    /// and the `capabilities: KnowledgeCapabilities`
    /// as parameters, (b) verify the storage root
    /// exists, (c) return `Ok(Self)` /
    /// `Err(KnowledgeErrorV1::StorageUnavailable)`.
    pub fn new() -> Self {
        Self
    }
}

impl Default for JsonKnowledgeConfig {
    fn default() -> Self {
        Self::new()
    }
}
