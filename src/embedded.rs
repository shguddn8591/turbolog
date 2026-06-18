use std::path::{Path, PathBuf};

use anyhow::{anyhow, Result};

use crate::ingest::Embedder;

#[cfg(feature = "embedded-model")]
pub static MODEL_BYTES: &[u8] = include_bytes!("../models/model.onnx");

#[cfg(feature = "embedded-model")]
pub static TOKENIZER_BYTES: &[u8] = include_bytes!("../models/tokenizer.json");

/// Creates an Embedder from embedded bytes (feature = "embedded-model") or from disk.
/// When not embedded, looks for model files in (priority order):
///   1. `model_dir` (explicit --model-dir or TURBOLOG_MODEL_DIR)
///   2. $XDG_DATA_HOME/turbolog/models  (~/.local/share/turbolog/models)
/// If neither exists, downloads from Hugging Face automatically.
pub fn make_embedder(model_dir: &Path) -> Result<Embedder> {
    #[cfg(feature = "embedded-model")]
    {
        Embedder::from_bytes(MODEL_BYTES, TOKENIZER_BYTES)
    }
    #[cfg(not(feature = "embedded-model"))]
    {
        let dir = resolve_model_dir(model_dir);
        ensure_models(&dir)?;
        Embedder::new(dir.join("model.onnx"), dir.join("tokenizer.json"))
    }
}

#[cfg(not(feature = "embedded-model"))]
fn resolve_model_dir(explicit: &Path) -> PathBuf {
    if explicit.join("model.onnx").exists() {
        return explicit.to_path_buf();
    }
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
                .join(".local/share")
        });
    base.join("turbolog/models")
}

#[cfg(not(feature = "embedded-model"))]
fn ensure_models(dir: &Path) -> Result<()> {
    let model = dir.join("model.onnx");
    let tokenizer = dir.join("tokenizer.json");
    if model.exists() && tokenizer.exists() {
        return Ok(());
    }
    std::fs::create_dir_all(dir)?;
    const BASE: &str =
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main";
    if !model.exists() {
        eprintln!("[turbolog] First run: downloading model (~86 MB) to {} ...", dir.display());
        download(&format!("{BASE}/onnx/model.onnx"), &model)?;
    }
    if !tokenizer.exists() {
        download(&format!("{BASE}/tokenizer.json"), &tokenizer)?;
    }
    eprintln!("[turbolog] Model ready.");
    Ok(())
}

#[cfg(not(feature = "embedded-model"))]
fn download(url: &str, dest: &Path) -> Result<()> {
    let resp = ureq::get(url)
        .call()
        .map_err(|e| anyhow!("Failed to download {url}: {e}"))?;
    let mut file = std::fs::File::create(dest)?;
    std::io::copy(&mut resp.into_reader(), &mut file)?;
    Ok(())
}
