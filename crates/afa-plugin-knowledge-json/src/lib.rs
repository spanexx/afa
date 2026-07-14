//! Code Map: afa-plugin-knowledge-json
//! - `JsonKnowledgeAdapter`: The concrete
//!   adapter the kernel registers via
//!   `CapabilityRegistry::register_knowledge`.
//! - `JsonKnowledgeConfig`: The settings card
//!   the adapter is built with.
//! - `InMemoryIndex`: The in-RAM index the
//!   adapter holds (re-exported for the
//!   conformance suite and the kernel
//!   tests).
//!
//! Story (plain English): This is the entry
//! point of the `afa-plugin-knowledge-json`
//! crate. It re-exports the three public
//! types the kernel and the conformance suite
//! touch: the adapter (what the kernel hands
//! out), the config (the settings card), and
//! the in-memory index (the search index the
//! adapter holds). Everything else is module-
//! private.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-lib-001 -> module re-exports
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-lib-" crates/afa-plugin-knowledge-json/src/lib.rs

pub mod adapter;
pub mod atomic_write;
pub mod config;
pub mod index;
pub mod index_file;
pub mod search;
pub mod storage;
pub mod topic_slug;

pub use adapter::JsonKnowledgeAdapter;
pub use config::JsonKnowledgeConfig;
pub use index::InMemoryIndex;

// Re-export the `Topic` type from the
// contracts crate so the `list_topics`
// path returns the contract type
// without the adapter having to
// qualify the path in every call
// site.
pub use afa_contracts::Topic;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn re_exports_point_to_concrete_types() {
        // A future contributor who breaks
        // a re-export (renames the type
        // without updating the public
        // surface) would be forced to
        // update this test. The test
        // does NOT construct an adapter
        // or a config (those require
        // runtime state); it only
        // confirms the type aliases
        // resolve at compile time.
        fn _accepts_adapter(_: JsonKnowledgeAdapter) {}
        fn _accepts_config(_: JsonKnowledgeConfig) {}
        fn _accepts_index(_: InMemoryIndex) {}
    }
}
