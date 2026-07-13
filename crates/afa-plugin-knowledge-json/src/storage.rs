//! Code Map: Phase 0 storage stub.
//!
//! Phase 0 placeholder for the storage-layer
//! helpers (read record, write record, list
//! topic dir, atomic write). Phase 1 populates.
//!
//! Story (plain English): The storage layer is
//! the part of the adapter that talks to the
//! disk. Phase 0 only needs the module to exist
//! so the `pub mod storage;` declaration in
//! `lib.rs` resolves; Phase 1 fills in the
//! real read / write / list functions.

#[cfg(test)]
mod tests {
    #[test]
    fn storage_module_compiles() {
        // Phase 0 placeholder test: the module
        // exists and compiles. Phase 1 will
        // replace this with the per-method
        // storage tests.
    }
}
