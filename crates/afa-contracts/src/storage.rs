//! Code Map: Storage DTOs
//! - `Migration`: The "one versioned SQL block" DTO the
//!   `afa-storage` crate consumes from its callers. A
//!   `Migration` is a literal-type struct (no derives —
//!   the `sql` field is `&'static str` and the version
//!   is the integer the storage engine uses to decide
//!   which migrations to apply). Callers build a
//!   `&[Migration]` list at boot time and hand it to
//!   `Storage::migrate`; the storage engine sorts by
//!   `version` and applies the ones it has not seen yet.
//!
//! Story (plain English): Imagine a filing cabinet that
//! only accepts new drawers in numbered order. When the
//! clerk wants to add a new kind of folder, they write
//! the SQL for "how to build drawer 7" on a card, label
//! the card "7", and hand it to the cabinet. The cabinet
//! checks: have I already filed drawer 7? If yes, skip.
//! If no, run the SQL, stamp the card "applied at
//! 2026-07-15 14:23", and move on. The card is a
//! `Migration` — a `(version, sql)` pair, nothing more.
//! The clerk is every engine that needs its own tables
//! (`afa-security`'s `sealed_secrets` table, the
//! `afa-observability` engine's `spans` table, future
//! engines' tables); the cabinet is the `Storage::migrate`
//! function. The list of cards is the `&[Migration]` the
//! engine hands to the cabinet at boot.
//!
//! CID Index:
//! CID:storage-001 -> Migration
//!
//! Quick lookup: rg -n "CID:storage-" crates/afa-contracts/src/storage.rs

// CID:storage-001 - Migration
// Purpose: The "one versioned SQL block" DTO. No derives
// on purpose: this is a literal-type struct, not a
// serialisable DTO. The `sql` field is a `&'static str`
// (the SQL is always baked into the binary at compile
// time, never loaded from a file or a network source);
// the `version` is the integer the storage engine uses
// to decide which migrations to apply. The
// `afa-storage::migrate` function sorts a `&[Migration]`
// by `version` and applies the ones with a `version`
// greater than the current `_afa_migrations.version` max.
// Uses: nothing — it is just a `(u32, &'static str)` pair.
// Used by: every engine that ships a SQLite schema
// (`afa-security` for the `sealed_secrets` table,
// `afa-observability` for the `spans` table, future
// engines' tables). The list is built at boot and handed
// to `Storage::migrate`.
pub struct Migration {
    /// The integer version of this migration. Must be
    /// unique within a `&[Migration]` list (the storage
    /// engine does NOT dedupe — a duplicate `version`
    /// is a programmer error and the engine panics on
    /// it). Monotonically increasing across releases of
    /// the same engine.
    pub version: u32,
    /// The SQL to run. Always a `&'static str` (baked
    /// into the binary) — never loaded from a file or a
    /// network source. May contain multiple statements
    /// (the storage engine runs them inside a single
    /// transaction).
    pub sql: &'static str,
}
