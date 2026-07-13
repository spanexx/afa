//! Code Map: InMemoryIndex stub
//!
//! Phase 0 placeholder for the in-memory
//! inverted index. Phase 1 will replace this
//! with the real struct that holds the
//! `topics: HashMap<String, TopicEntry>` and
//! the `tag_index: HashMap<String, HashSet<Uuid>>`.
//!
//! Story (plain English): The index is the
//! card-catalog's secret notebook. It maps
//! "this drawer label" to "this list of
//! cards in the drawer," and "this tag" to
//! "this list of cards with the tag." Phase
//! 0 only needs the type to exist so
//! `JsonKnowledgeAdapter::new` can hold an
//! `Arc<InMemoryIndex>`; Phase 1 fills in
//! the real fields.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-index-001 -> InMemoryIndex
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-index-" crates/afa-plugin-knowledge-json/src/index.rs

// CID:afa-plugin-knowledge-json-index-001 - InMemoryIndex
// Purpose: The Phase 0 stub for the in-memory
// inverted index. Phase 1 will replace this
// with a real struct that holds the topic map
// and the tag index (the data the
// `find_information` method walks to score
// candidates). The struct is a unit struct for
// now; Phase 1 adds `pub topics: HashMap<String,
// TopicEntry>` and `pub tag_index: HashMap<String,
// HashSet<Uuid>>` (the `TopicEntry` is also a
// Phase 1 type).
pub struct InMemoryIndex;

impl InMemoryIndex {
    /// Phase 0 placeholder constructor. Phase 1
    /// will build the empty maps.
    pub fn new() -> Self {
        Self
    }
}

impl Default for InMemoryIndex {
    fn default() -> Self {
        Self::new()
    }
}
