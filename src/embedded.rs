use std::path::Path;

use anyhow::Result;

use crate::ingest::Embedder;

#[cfg(feature = "embedded-model")]
pub static MODEL_BYTES: &[u8] = include_bytes!("../models/model.onnx");

#[cfg(feature = "embedded-model")]
pub static TOKENIZER_BYTES: &[u8] = include_bytes!("../models/tokenizer.json");

/// Creates an Embedder from embedded bytes (feature = "embedded-model") or from disk.
/// The embedded path requires no model files at runtime — the bytes are baked into the binary.
pub fn make_embedder(_model_dir: &Path) -> Result<Embedder> {
    #[cfg(feature = "embedded-model")]
    {
        Embedder::from_bytes(MODEL_BYTES, TOKENIZER_BYTES)
    }
    #[cfg(not(feature = "embedded-model"))]
    {
        Embedder::new(
            _model_dir.join("model.onnx"),
            _model_dir.join("tokenizer.json"),
        )
    }
}
