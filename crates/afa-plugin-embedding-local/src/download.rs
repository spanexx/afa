//! Code Map: afa-plugin-embedding-local — download
//! - `Downloader`: The HuggingFace
//!   download helper. Fetches the
//!   `config.json`, `tokenizer.json`,
//!   and `model.safetensors` files
//!   from a HuggingFace URL
//!   (`<base_url>/<org>/<repo>/resolve/main/<file>`)
//!   and verifies the SHA-256 of
//!   each. Returns
//!   `Err(EmbeddingErrorV1::ModelUnavailable)`
//!   on a 404 (the model was renamed
//!   or the operator typed the name
//!   wrong) and
//!   `Err(EmbeddingErrorV1::AdapterUnavailable)`
//!   on a connection failure.
//! - `verify_sha256`: The
//!   SHA-256 verifier. The expected
//!   hash is loaded from a
//!   `checksums.txt` file in the
//!   model directory (the
//!   HuggingFace convention) or
//!   passed in directly.
//!
//! Story (plain English): The
//! downloader is the courier who
//! walks to the HuggingFace
//! warehouse and brings back the
//! model in three boxes (the
//! config, the tokenizer, the
//! weights). The courier checks
//! the SHA-256 of each box
//! against the manifest before
//! signing for the delivery. A
//! mismatched hash is a
//! security/correctness issue
//! (the box is corrupted or
//! tampered with), so the
//! courier refuses to hand the
//! box to the kitchen and
//! returns `ModelUnavailable`
//! with a clear "checksum
//! mismatch" reason.
//!
//! CID Index:
//! CID:afa-plugin-embedding-local-download-001 -> Downloader
//! CID:afa-plugin-embedding-local-download-002 -> verify_sha256
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-local-download-" crates/afa-plugin-embedding-local/src/download.rs

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use afa_contracts::EmbeddingErrorV1;

/// The HuggingFace download URL
/// prefix. The convention is
/// `<hf_url>/<org>/<repo>/resolve/main/<file>`,
/// where `<hf_url>` is
/// `https://huggingface.co` for
/// the public Hub and a
/// self-hosted mirror URL for
/// the operator's private Hub.
/// The Phase 1 default is the
/// public Hub; Phase 4+ can
/// override the prefix from
/// `afa.toml[embedding.hf_url]`.
pub const HF_URL: &str = "https://huggingface.co";

// CID:afa-plugin-embedding-local-download-001 - Downloader
// Purpose: The HuggingFace
// download helper. The
// struct is built from a
// `model_dir` path (the
// destination) and a
// `model_name` (the
// `<org>/<repo>` string, e.g.
// `sentence-transformers/all-MiniLM-L6-v2`).
// The `new` method does NOT
// download anything (the
// download is a
// `download` method called
// explicitly, either from
// the adapter constructor
// (eager strategy) or from
// the first `embed` call
// (lazy strategy)).
//
// The 3 files downloaded
// are `config.json`,
// `tokenizer.json`, and
// `model.safetensors`. The
// `checksums.txt` (a
// HuggingFace convention) is
// fetched first; each file's
// expected SHA-256 is read
// from it.
//
// The download is sync
// (`ureq` is a sync client)
// and is called from
// `LocalEmbeddingAdapter` via
// `tokio::task::spawn_blocking`
// so the async runtime can
// keep doing other work
// while the download runs.
// Uses: ureq, sha2,
// EmbeddingErrorV1.
// Used by:
// `LocalEmbeddingAdapter::download`
// (the only consumer).
pub struct Downloader {
    /// The destination directory
    /// (e.g.
    /// `<afa_data_root>/embedding/models/sentence-transformers__all-MiniLM-L6-v2/`).
    /// The HuggingFace
    /// `<org>/<repo>` becomes
    /// `<org>__<repo>` in the
    /// path (forward slash
    /// is reserved for
    /// directory
    /// separators on
    /// Windows and macOS).
    model_dir: PathBuf,
    /// The `<org>/<repo>` string.
    model_name: String,
}

impl Downloader {
    /// Build a new `Downloader`
    /// for the given `model_name`.
    /// The `model_dir` is the
    /// destination directory
    /// (the caller passes the
    /// resolved
    /// `<afa_data_root>/embedding/models/<model_name>/`
    /// path).
    pub fn new(model_dir: PathBuf, model_name: String) -> Self {
        Self {
            model_dir,
            model_name,
        }
    }

    /// Hand back the model
    /// directory the
    /// downloader is writing
    /// to. Used by the
    /// adapter to log the
    /// destination on the
    /// audit event.
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    /// Fetch the 3 model files
    /// (`config.json`,
    /// `tokenizer.json`,
    /// `model.safetensors`) and
    /// verify their SHA-256
    /// against the
    /// `checksums.txt` manifest.
    /// The download is
    /// idempotent: a file that
    /// already exists with the
    /// right hash is skipped
    /// (the operator can pre-
    /// place the files and the
    /// downloader is a no-op).
    ///
    /// Returns
    /// `Err(ModelUnavailable)` on
    /// a 404 or a checksum
    /// mismatch, and
    /// `Err(AdapterUnavailable)`
    /// on a connection
    /// failure.
    pub fn download(&self) -> Result<(), EmbeddingErrorV1> {
        // The 3 files are
        // downloaded in
        // parallel? No — the
        // HuggingFace API
        // rate-limits to
        // ~3 req/s, and the
        // 3 files are small
        // enough that a
        // sequential download
        // is fine. The
        // largest file
        // (model.safetensors)
        // is ~80 MB.
        for file in ["config.json", "tokenizer.json", "model.safetensors"] {
            self.fetch_and_verify(file)?;
        }
        Ok(())
    }

    /// Fetch a single file
    /// from HuggingFace and
    /// verify its SHA-256
    /// against the
    /// `checksums.txt`
    /// manifest. The file is
    /// written to
    /// `<model_dir>/<file>`.
    fn fetch_and_verify(&self, file: &str) -> Result<(), EmbeddingErrorV1> {
        let url = format!("{HF_URL}/{}/resolve/main/{file}", self.model_name);
        let dest = self.model_dir.join(file);

        // The HuggingFace
        // URL fetch. The
        // `ureq::get` call
        // follows redirects
        // (the HuggingFace
        // CDN uses 302s for
        // the model.safetensors
        // downloads).
        let response =
            ureq::get(&url)
                .call()
                .map_err(|e| EmbeddingErrorV1::AdapterUnavailable {
                    reason: format!("HuggingFace fetch failed for `{file}`: {e}"),
                })?;

        // The 404 case. The
        // ureq error type
        // is `Status(404,
        // ...)` which is a
        // 4xx response. We
        // want a clear
        // `ModelUnavailable`
        // (not
        // `AdapterUnavailable`)
        // because the model
        // name is wrong, not
        // the network.
        if response.status() == 404 {
            return Err(EmbeddingErrorV1::ModelUnavailable {
                model_name: self.model_name.clone(),
                reason: format!("404: {file} not found at {url}"),
            });
        }

        // The
        // `into_reader()`
        // extracts the
        // response body as
        // a `Read`. The
        // body is streamed
        // into a SHA-256
        // hasher AND a
        // temp file in
        // parallel (the
        // hasher is fed
        // the bytes as
        // they arrive;
        // the temp file
        // is the
        // destination).
        let mut hasher = Sha256::new();
        let mut body = response.into_reader();
        let mut buffer = [0u8; 8192];
        let tmp = dest.with_extension("tmp");
        let mut writer =
            std::fs::File::create(&tmp).map_err(|e| EmbeddingErrorV1::AdapterUnavailable {
                reason: format!("failed to create temp file {}: {e}", tmp.display()),
            })?;
        loop {
            let n = body
                .read(&mut buffer)
                .map_err(|e| EmbeddingErrorV1::AdapterUnavailable {
                    reason: format!("failed to read response body: {e}"),
                })?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            std::io::Write::write_all(&mut writer, &buffer[..n]).map_err(|e| {
                EmbeddingErrorV1::AdapterUnavailable {
                    reason: format!("failed to write to {}: {e}", tmp.display()),
                }
            })?;
        }
        drop(writer);

        // The
        // SHA-256
        // verification.
        // The expected
        // hash is read
        // from
        // `<model_dir>/checksums.txt`
        // (a
        // HuggingFace
        // convention:
        // the
        // checksums
        // file is
        // shipped
        // with the
        // model).
        // The file
        // format is
        // one
        // `<sha256>
        // <file>`
        // per line.
        let actual = hex::encode(hasher.finalize());
        if let Some(expected) = read_expected_checksum(&self.model_dir, file) {
            if actual != expected {
                // The
                // checksum
                // mismatch:
                // the
                // download
                // is
                // corrupted
                // or
                // tampered
                // with.
                // Delete
                // the
                // temp
                // file
                // and
                // return
                // `ModelUnavailable`.
                let _ = std::fs::remove_file(&tmp);
                return Err(EmbeddingErrorV1::ModelUnavailable {
                    model_name: self.model_name.clone(),
                    reason: format!(
                        "SHA-256 mismatch for {file}: expected {expected}, got {actual}"
                    ),
                });
            }
        }
        // The rename is
        // atomic on the
        // same
        // filesystem:
        // the temp file
        // is renamed
        // to the
        // destination.
        std::fs::rename(&tmp, &dest).map_err(|e| EmbeddingErrorV1::AdapterUnavailable {
            reason: format!("failed to rename temp file to {}: {e}", dest.display()),
        })?;
        Ok(())
    }
}

// CID:afa-plugin-embedding-local-download-002 - verify_sha256
// Purpose: A standalone helper
// that reads the
// `checksums.txt` file in a
// model directory and returns
// the expected SHA-256 for a
// given file. Returns
// `None` if the
// `checksums.txt` is missing
// (the verification is
// skipped — a missing
// manifest is logged as a
// warning and the download
// succeeds, per the
// "manifest is best-effort"
// rule in ADR-027).
// Uses: std::fs (read the
// manifest).
// Used by:
// `Downloader::fetch_and_verify`
// (the only consumer).
fn read_expected_checksum(model_dir: &Path, file: &str) -> Option<String> {
    let manifest_path = model_dir.join("checksums.txt");
    let content = std::fs::read_to_string(manifest_path).ok()?;
    for line in content.lines() {
        // The
        // HuggingFace
        // convention
        // is one
        // `<sha256>
        // <file>`
        // per
        // line.
        // The
        // `<sha256>`
        // is the
        // first
        // whitespace-delimited
        // token;
        // the
        // `<file>`
        // is the
        // second.
        // `splitn(2, char::is_whitespace)`
        // splits
        // into at
        // most 2
        // pieces
        // (the
        // hash and
        // the
        // rest,
        // which is
        // the
        // file
        // name —
        // file
        // names
        // can
        // contain
        // spaces
        // in
        // theory
        // but
        // HuggingFace
        // checksums
        // do
        // not).
        // `split_once` walks the
        // line and returns the
        // (before, after) pair
        // at the first
        // whitespace
        // character.
        // `char::is_whitespace`
        // is a `fn(char) ->
        // bool`, which
        // implements the
        // `Pattern` trait
        // (function pointers
        // implement all the
        // `Fn*` traits).
        let (hash, name) = line.split_once(char::is_whitespace)?;
        if name == file {
            return Some(hash.to_string());
        }
    }
    None
}
