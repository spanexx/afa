//! Code Map: atomic_write helper
//! - `atomic_write`: The temp-then-rename
//!   helper that guarantees the on-disk
//!   `target` file is never observed
//!   half-written. The sequence is: (1)
//!   write `data` to a temp file in the
//!   same directory as `target`, (2)
//!   `fsync` the temp file so the bytes
//!   are durable, (3) `rename` the temp
//!   file over `target` (atomic on POSIX
//!   file systems), (4) `fsync` the
//!   parent directory so the rename
//!   itself is durable across a crash.
//!
//! Story (plain English): The atomic-write
//! helper is the part of the adapter that
//! guarantees the on-disk file is never
//! half-written. Imagine you are filing a
//! new card in the catalog; if the power
//! fails halfway through, the next reader
//! should see either the old card or the
//! new card — never a torn, half-written
//! card with one line of new text and one
//! line of old. The temp-then-rename
//! pattern is how the catalog guarantees
//! that.
//!
//! CID Index:
//! CID:afa-plugin-knowledge-json-atomic-write-001 -> atomic_write
//!
//! Quick lookup: rg -n "CID:afa-plugin-knowledge-json-atomic-write-" crates/afa-plugin-knowledge-json/src/atomic_write.rs

use std::path::Path;

use afa_contracts::KnowledgeErrorV1;
use tokio::fs;
use uuid::Uuid;

// CID:afa-plugin-knowledge-json-atomic-write-001 - atomic_write
// Purpose: The temp-then-rename helper. The
// four-step sequence (write temp + fsync
// temp + rename + fsync parent) is the
// canonical "crash-safe file write" pattern
// for POSIX file systems. On any error, the
// temp file may be left on disk; the boot
// sequence (Phase 3) cleans it up.
//
// **Call pattern (per IMPL Phase 1)**:
// 1. `tokio::fs::write(&temp_path, data).await?;`
// 2. `temp_file.sync_all().await?;`
// 3. `tokio::fs::rename(&temp_path, target).await?;`
// 4. `tokio::fs::File::open(&parent_dir).await?.sync_all().await?;`
//
// On any `io::Error`, the error is
// converted to
// `KnowledgeErrorV1::StorageUnavailable`
// (the dependency is "down" from the
// caller's perspective; the caller may
// retry with backoff).
pub async fn atomic_write(target: &Path, data: &[u8]) -> Result<(), KnowledgeErrorV1> {
    // The parent directory of `target`
    // is where the temp file lives
    // (same-directory rename is atomic
    // on POSIX; cross-device rename is
    // not, but the adapter always
    // writes the temp file in the same
    // directory as `target`, so this is
    // a no-device-change path).
    let parent = target.parent().ok_or_else(|| {
        // No parent: the caller passed a
        // bare filename. The adapter
        // never does this in practice,
        // but the error path is here
        // for safety.
        KnowledgeErrorV1::StorageUnavailable {
            topic: None,
            record_id: None,
            reason: format!(
                "atomic_write: target has no parent directory: {}",
                target.display()
            ),
        }
    })?;

    // Build the temp path. The
    // `<target>.tmp.<uuid>` shape
    // guarantees uniqueness even under
    // concurrent writes to the same
    // target (each call gets its own
    // `uuid::new_v4()` suffix; the
    // rename is the atomic step that
    // wins the race).
    let tmp_name = format!(
        "{}.tmp.{}",
        target
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("file"),
        Uuid::new_v4()
    );
    let tmp_path = parent.join(tmp_name);

    // Step 1: write the bytes to the
    // temp file.
    fs::write(&tmp_path, data)
        .await
        .map_err(|e| KnowledgeErrorV1::StorageUnavailable {
            topic: None,
            record_id: None,
            reason: format!("atomic_write: write temp failed: {e}"),
        })?;

    // Step 2: fsync the temp file so
    // the bytes are durable. On
    // `io::Error`, best-effort remove
    // the temp file (the temp file
    // may otherwise leak; the boot
    // sequence cleans it up but a
    // prompt cleanup is friendlier).
    if let Err(e) = fs::File::open(&tmp_path).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(KnowledgeErrorV1::StorageUnavailable {
            topic: None,
            record_id: None,
            reason: format!("atomic_write: open temp for fsync failed: {e}"),
        });
    }
    // We have to re-open because
    // `fs::write` consumed the path.
    // The temp file definitely exists
    // (we just wrote it); a second
    // open-then-fsync is fine.
    match fs::File::open(&tmp_path).await {
        Ok(file) => {
            if let Err(e) = file.sync_all().await {
                let _ = fs::remove_file(&tmp_path).await;
                return Err(KnowledgeErrorV1::StorageUnavailable {
                    topic: None,
                    record_id: None,
                    reason: format!("atomic_write: fsync temp failed: {e}"),
                });
            }
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp_path).await;
            return Err(KnowledgeErrorV1::StorageUnavailable {
                topic: None,
                record_id: None,
                reason: format!("atomic_write: re-open temp for fsync failed: {e}"),
            });
        }
    }

    // Step 3: rename the temp file
    // over the target. On POSIX this
    // is atomic (the target either
    // points to the old bytes or the
    // new bytes, never a mix).
    if let Err(e) = fs::rename(&tmp_path, target).await {
        let _ = fs::remove_file(&tmp_path).await;
        return Err(KnowledgeErrorV1::StorageUnavailable {
            topic: None,
            record_id: None,
            reason: format!("atomic_write: rename failed: {e}"),
        });
    }

    // Step 4: fsync the parent
    // directory so the rename itself
    // is durable across a crash.
    // Without this step, a power
    // failure between the rename and
    // the parent fsync could leave
    // the directory entry pointing to
    // a now-orphaned inode. The IMPL
    // Phase 1 calls this out as the
    // difference between "the file
    // looks right after a clean
    // restart" and "the file looks
    // right after a crash."
    match fs::File::open(parent).await {
        Ok(dir_file) => {
            if let Err(e) = dir_file.sync_all().await {
                return Err(KnowledgeErrorV1::StorageUnavailable {
                    topic: None,
                    record_id: None,
                    reason: format!("atomic_write: parent fsync failed: {e}"),
                });
            }
        }
        Err(e) => {
            return Err(KnowledgeErrorV1::StorageUnavailable {
                topic: None,
                record_id: None,
                reason: format!("atomic_write: open parent for fsync failed: {e}"),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn atomic_write_writes_target_with_full_contents() {
        // Happy path: the target file
        // contains the exact bytes
        // written.
        let dir = TempDir::new().expect("tempdir");
        let target = dir.path().join("hello.md");
        atomic_write(&target, b"hello world")
            .await
            .expect("atomic_write");
        let read_back = std::fs::read(&target).expect("read back");
        assert_eq!(read_back, b"hello world");
    }

    #[tokio::test]
    async fn atomic_write_overwrites_existing_file() {
        // A second atomic_write to
        // the same target replaces
        // the old bytes; no
        // half-written state is
        // observable.
        let dir = TempDir::new().expect("tempdir");
        let target = dir.path().join("over.md");
        atomic_write(&target, b"first").await.expect("first");
        atomic_write(&target, b"second").await.expect("second");
        let read_back = std::fs::read(&target).expect("read back");
        assert_eq!(read_back, b"second");
    }

    #[tokio::test]
    async fn atomic_write_does_not_leave_temp_file_on_success() {
        // After a successful
        // atomic_write, no
        // `<target>.tmp.*` file is
        // left on disk (the rename
        // consumed it).
        let dir = TempDir::new().expect("tempdir");
        let target = dir.path().join("clean.md");
        atomic_write(&target, b"x").await.expect("atomic_write");
        let entries: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read_dir")
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().contains(".tmp."))
            .collect();
        assert!(entries.is_empty(), "no temp file should remain");
    }

    #[test]
    fn atomic_write_module_compiles() {
        // Phase 0 placeholder kept as a
        // regression guard: the module
        // exists and the public surface
        // compiles. Phase 1 added the
        // per-method tests above.
    }
}
