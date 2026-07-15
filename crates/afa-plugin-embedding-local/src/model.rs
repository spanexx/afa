#![allow(clippy::doc_lazy_continuation)]
//! Code Map: afa-plugin-embedding-local — model
//! - `BertEmbedder`: The wrapped
//!   candle `BertModel` +
//!   `Tokenizer`. Phase 1.5: the
//!   real BERT forward pass is
//!   wired (the Phase 1 SHA-256
//!   stub is removed). The
//!   `embed_batch` method runs
//!   `BertModel::forward`,
//!   mean-pools over the
//!   sequence axis weighted by
//!   the attention mask, and
//!   L2-normalizes. The output
//!   is a semantically meaningful
//!   vector: cosine similarity of
//!   two similar texts is close
//!   to 1.0; two unrelated texts
//!   is close to 0.0.
//! - `BertError`: The 4 typed errors
//!   the `BertEmbedder` can return,
//!   with a `From` impl to
//!   `EmbeddingErrorV1`.
//!
//! Story (plain English): The model
//! module is the kitchen's recipe
//! book. The `BertEmbedder` is the
//! chef who can turn a text into a
//! vector. In Phase 1.5 the chef
//! uses a real BERT encoder (the
//! `all-MiniLM-L6-v2` model from
//! HuggingFace) loaded via the pure
//! Rust `candle` stack (no native
//! `libtorch` dependency, per
//! ADR-029).
//!
//! CID Index:
//! CID:afa-plugin-embedding-local-model-001 -> BertEmbedder
//! CID:afa-plugin-embedding-local-model-002 -> BertError
//! CID:afa-plugin-embedding-local-model-003 -> embed_batch
//!
//! Quick lookup: rg -n "CID:afa-plugin-embedding-local-model-" crates/afa-plugin-embedding-local/src/model.rs

use std::path::{Path, PathBuf};

use afa_contracts::EmbeddingErrorV1;
use candle_core::{DType, Device, Tensor};
use candle_nn::VarBuilder;
use candle_transformers::models::bert::{BertModel, Config};
use tokenizers::utils::padding::{PaddingDirection, PaddingParams, PaddingStrategy};
use tokenizers::Tokenizer;

// CID:afa-plugin-embedding-local-model-002 - BertError
// Purpose: The 4 typed errors the
// `BertEmbedder` can return, with a
// `From` impl to `EmbeddingErrorV1`.
// The 4 errors map onto the 4
// `EmbeddingErrorV1` variants.
// Uses: `From<...> for EmbeddingErrorV1`.
// Used by: `BertEmbedder::load` and
// `BertEmbedder::embed_batch`.
#[derive(Debug, thiserror::Error)]
pub enum BertError {
    /// The model directory is
    /// missing a required file
    /// (`config.json`,
    /// `tokenizer.json`,
    /// `model.safetensors`). Maps
    /// to
    /// `EmbeddingErrorV1::ModelUnavailable`.
    #[error("model file missing: {path}")]
    ModelFileMissing { path: PathBuf },
    /// A model file is present
    /// but malformed (corrupt
    /// safetensors header,
    /// invalid config.json,
    /// etc.). Maps to
    /// `EmbeddingErrorV1::ModelUnavailable`.
    #[error("model file malformed: {path}: {reason}")]
    ModelFileMalformed { path: PathBuf, reason: String },
    /// The tokenization failed
    /// (token id out of vocab,
    /// etc.). Maps to
    /// `EmbeddingErrorV1::InvalidInput`.
    #[error("tokenization failed: {reason}")]
    TokenizationFailed { reason: String },
    /// The candle forward pass
    /// failed (shape mismatch,
    /// OOM, etc.). Maps to
    /// `EmbeddingErrorV1::Internal`.
    #[error("candle forward pass failed: {reason}")]
    ForwardPassFailed { reason: String },
}

impl From<BertError> for EmbeddingErrorV1 {
    fn from(e: BertError) -> Self {
        match e {
            BertError::ModelFileMissing { path } => EmbeddingErrorV1::ModelUnavailable {
                model_name: "all-MiniLM-L6-v2".to_string(),
                reason: format!("model file missing: {}", path.display()),
            },
            BertError::ModelFileMalformed { path, reason } => EmbeddingErrorV1::ModelUnavailable {
                model_name: "all-MiniLM-L6-v2".to_string(),
                reason: format!("model file malformed: {}: {reason}", path.display()),
            },
            BertError::TokenizationFailed { reason } => EmbeddingErrorV1::InvalidInput { reason },
            BertError::ForwardPassFailed { reason } => EmbeddingErrorV1::Internal { reason },
        }
    }
}

// CID:afa-plugin-embedding-local-model-001 - BertEmbedder
// Purpose: The wrapped candle
// `BertModel` + `Tokenizer`.
// Phase 1.5: holds the real
// `candle_transformers::models::bert::BertModel`
// + a `tokenizers::Tokenizer` +
// the `Device` the forward pass
// runs on + the parsed `Config`
// (so `dimension()` can return
// `config.hidden_size` without
// re-reading the file). The
// struct is `Send + Sync` so it
// can be held behind
// `Arc<dyn EmbeddingV1>` in
// the CapabilityRegistry.
//
// Phase 1.5 NOTE: the struct
// fields changed from
// `{model_dir, dimension}`
// (the Phase 1 stub) to
// `{model_dir, config, model,
// tokenizer, device}`
// (the real candle forward
// pass).
pub struct BertEmbedder {
    model_dir: PathBuf,
    config: Config,
    model: BertModel,
    tokenizer: Tokenizer,
    device: Device,
}

impl BertEmbedder {
    /// Load a `BertEmbedder`
    /// from a `model_dir`. The
    /// directory must contain
    /// `config.json`,
    /// `tokenizer.json`, and
    /// `model.safetensors`. The
    /// function:
    /// 1. Parses `config.json`
    ///    as a
    ///    `candle_transformers::models::bert::Config`
    /// 2. Loads `tokenizer.json`
    ///    with
    ///    `Tokenizer::from_file`
    ///    + configures
    ///    batch-longest padding
    /// 3. Loads
    ///    `model.safetensors` as
    ///    a `VarBuilder` (the
    ///    `DType` is `F32`)
    /// 4. Constructs
    ///    `BertModel::load`
    ///    on the CPU device
    ///    (`Device::Cpu`; CUDA
    ///    and Metal are
    ///    operator options
    ///    added in Phase 4 per
    ///    the `afa.toml[embedding]`
    ///    feature flag)
    pub fn load(model_dir: &Path) -> Result<Self, BertError> {
        let config_path = model_dir.join("config.json");
        let tokenizer_path = model_dir.join("tokenizer.json");
        let weights_path = model_dir.join("model.safetensors");

        for path in [&config_path, &tokenizer_path, &weights_path] {
            if !path.exists() {
                return Err(BertError::ModelFileMissing { path: path.clone() });
            }
        }

        // The `config.json`
        // parse. Phase 1.5
        // validates the full
        // HuggingFace
        // `bert_config` schema.
        let config: Config =
            serde_json::from_str(&std::fs::read_to_string(&config_path).map_err(|e| {
                BertError::ModelFileMalformed {
                    path: config_path.clone(),
                    reason: e.to_string(),
                }
            })?)
            .map_err(|e| BertError::ModelFileMalformed {
                path: config_path.clone(),
                reason: e.to_string(),
            })?;

        // The tokenizer load.
        // The default
        // tokenizer from
        // HuggingFace does
        // NOT have padding
        // configured. We set
        // it here so
        // `encode_batch`
        // pads to the
        // longest in the
        // batch. The
        // `pad_id` is 0
        // (the BERT
        // `[PAD]` token
        // id); the
        // `pad_token` is
        // `[PAD]`.
        let mut tokenizer =
            Tokenizer::from_file(&tokenizer_path).map_err(|e| BertError::ModelFileMalformed {
                path: tokenizer_path.clone(),
                reason: e.to_string(),
            })?;
        tokenizer.with_padding(Some(PaddingParams {
            strategy: PaddingStrategy::BatchLongest,
            direction: PaddingDirection::Right,
            pad_id: 0,
            pad_token: "[PAD]".to_string(),
            pad_to_multiple_of: None,
            pad_type_id: 0,
        }));

        // The safetensors
        // load.
        let device = Device::Cpu;
        let weights = std::fs::read(&weights_path).map_err(|e| BertError::ModelFileMalformed {
            path: weights_path.clone(),
            reason: e.to_string(),
        })?;
        let vb =
            VarBuilder::from_buffered_safetensors(weights, DType::F32, &device).map_err(|e| {
                BertError::ModelFileMalformed {
                    path: weights_path.clone(),
                    reason: e.to_string(),
                }
            })?;

        // The
        // `BertModel::load`.
        let model = BertModel::load(vb, &config).map_err(|e| BertError::ModelFileMalformed {
            path: weights_path.clone(),
            reason: e.to_string(),
        })?;

        Ok(Self {
            model_dir: model_dir.to_path_buf(),
            config,
            model,
            tokenizer,
            device,
        })
    }

    /// Hand back the model
    /// directory the embedder
    /// was loaded from.
    pub fn model_dir(&self) -> &Path {
        &self.model_dir
    }

    /// Hand back the output
    /// dimension. Reads it
    /// from
    /// `Config.hidden_size`
    /// (set from
    /// `config.json` at
    /// load time).
    pub fn dimension(&self) -> usize {
        self.config.hidden_size
    }

    // CID:afa-plugin-embedding-local-model-003 - embed_batch
    // Purpose: The batched forward
    // pass. Phase 1.5: the real
    // candle BERT forward pass
    // + mean-pooling over
    // `seq_len` weighted by the
    // attention mask +
    // L2-normalization. The
    // algorithm is:
    // 1. Encode the batch
    //    with
    //    `tokenizer.encode_batch(texts, true)`
    // 2. Stack the
    //    `get_ids()` and
    //    `get_attention_mask()`
    //    slices into tensors
    //    of shape
    //    `[N, seq_len]`
    // 3. Build a
    //    `token_type_ids`
    //    tensor of shape
    //    `[N, seq_len]`
    //    (all zeros — the
    //    AFA Embedding
    //    engine does not
    //    use sentence
    //    pairs)
    // 4. Call
    //    `BertModel::forward`
    //    (returns
    //    `last_hidden_state`
    //    of shape
    //    `[N, seq_len, hidden_size]`)
    // 5. Mean-pool: multiply
    //    by the attention
    //    mask (zero out
    //    padding), sum
    //    over `seq_len`,
    //    divide by the sum
    //    of the attention
    //    mask
    // 6. L2-normalize: divide
    //    each row by its
    //    L2 norm
    // 7. Return the
    //    `Vec<Vec<f32>>`
    pub fn embed_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, BertError> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        // (1) Encode the batch.
        let inputs: Vec<&str> = texts.iter().map(String::as_str).collect();
        let encodings = self.tokenizer.encode_batch(inputs, true).map_err(|e| {
            BertError::TokenizationFailed {
                reason: e.to_string(),
            }
        })?;

        // The batch dim + the
        // padded seq length.
        let n = encodings.len();
        let seq_len = encodings[0].get_ids().len();

        // (2) Build the
        // token-id tensor
        // `[N, seq_len]`.
        let mut ids: Vec<u32> = Vec::with_capacity(n * seq_len);
        for enc in &encodings {
            ids.extend(enc.get_ids().iter().copied());
        }
        let input_ids = Tensor::from_vec(ids, (n, seq_len), &self.device).map_err(|e| {
            BertError::ForwardPassFailed {
                reason: e.to_string(),
            }
        })?;

        // Build the
        // attention-mask
        // tensor
        // `[N, seq_len]`
        // (1 for real
        // tokens, 0 for
        // padding).
        let mut mask: Vec<u32> = Vec::with_capacity(n * seq_len);
        for enc in &encodings {
            mask.extend(enc.get_attention_mask().iter().copied());
        }
        let attention_mask = Tensor::from_vec(mask, (n, seq_len), &self.device).map_err(|e| {
            BertError::ForwardPassFailed {
                reason: e.to_string(),
            }
        })?;

        // (3) Build the
        // token-type-id
        // tensor (all
        // zeros).
        let token_type_ids = input_ids
            .zeros_like()
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?;

        // (4) The BERT
        // forward pass.
        let hidden = self
            .model
            .forward(&input_ids, &token_type_ids, Some(&attention_mask))
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?;

        // (5) Mean-pool
        // weighted by
        // the attention
        // mask.
        let mask_f32 =
            attention_mask
                .to_dtype(DType::F32)
                .map_err(|e| BertError::ForwardPassFailed {
                    reason: e.to_string(),
                })?;
        let mask_broadcast = mask_f32
            .unsqueeze(2)
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?;
        let masked =
            hidden
                .broadcast_mul(&mask_broadcast)
                .map_err(|e| BertError::ForwardPassFailed {
                    reason: e.to_string(),
                })?;
        let summed = masked.sum(1).map_err(|e| BertError::ForwardPassFailed {
            reason: e.to_string(),
        })?;
        let counts = mask_f32
            .sum(1)
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?
            .unsqueeze(1)
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?;
        // The `clamp`
        // avoids a
        // divide-by-zero
        // if the mask
        // is all-zero.
        let counts_clamped =
            counts
                .clamp(1e-9, f64::INFINITY)
                .map_err(|e| BertError::ForwardPassFailed {
                    reason: e.to_string(),
                })?;
        let pooled =
            summed
                .broadcast_div(&counts_clamped)
                .map_err(|e| BertError::ForwardPassFailed {
                    reason: e.to_string(),
                })?;

        // (6) L2-normalize.
        let norm = pooled
            .sqr()
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?
            .sum(1)
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?
            .sqrt()
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?
            .unsqueeze(1)
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?;
        let norm_clamped =
            norm.clamp(1e-9, f64::INFINITY)
                .map_err(|e| BertError::ForwardPassFailed {
                    reason: e.to_string(),
                })?;
        let normalized =
            pooled
                .broadcast_div(&norm_clamped)
                .map_err(|e| BertError::ForwardPassFailed {
                    reason: e.to_string(),
                })?;

        // (7) Flatten the
        // `[N, hidden_size]`
        // tensor to
        // `Vec<Vec<f32>>`.
        let flat: Vec<f32> = normalized
            .flatten_all()
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?
            .to_vec1()
            .map_err(|e| BertError::ForwardPassFailed {
                reason: e.to_string(),
            })?;
        let h_size = self.config.hidden_size;
        Ok(flat.chunks(h_size).map(|chunk| chunk.to_vec()).collect())
    }
}

// `Send + Sync` is required
// because the
// `LocalEmbeddingAdapter`
// holds the `BertEmbedder`
// behind `Arc<dyn EmbeddingV1>`
// and the `embed` /
// `embed_batch` futures run
// on the tokio runtime
// (which may move the future
// between threads).
unsafe impl Send for BertEmbedder {}
unsafe impl Sync for BertEmbedder {}
