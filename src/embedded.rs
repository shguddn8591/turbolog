use std::path::Path;

use anyhow::Result;

use crate::ingest::Embedder;

#[cfg(not(feature = "embedded-model"))]
use std::path::PathBuf;

#[cfg(not(feature = "embedded-model"))]
use anyhow::anyhow;

#[cfg(not(feature = "embedded-model"))]
const PROGRESS_STEP_BYTES: u64 = 5 * 1024 * 1024;

#[cfg(feature = "embedded-model")]
pub static MODEL_BYTES: &[u8] = include_bytes!("../models/model.onnx");

#[cfg(feature = "embedded-model")]
pub static TOKENIZER_BYTES: &[u8] = include_bytes!("../models/tokenizer.json");

/// Creates an Embedder from embedded bytes (feature = "embedded-model") or from disk.
/// When not embedded, looks for model files in (priority order):
///   1. `model_dir` (explicit --model-dir or TURBOLOG_MODEL_DIR)
///   2. $XDG_DATA_HOME/turbolog/models  (~/.local/share/turbolog/models)
///
/// If neither exists, downloads from Hugging Face automatically.
pub fn make_embedder(_model_dir: &Path) -> Result<Embedder> {
    #[cfg(feature = "embedded-model")]
    {
        Embedder::from_bytes(MODEL_BYTES, TOKENIZER_BYTES)
    }
    #[cfg(not(feature = "embedded-model"))]
    {
        let dir = resolve_model_dir(_model_dir);
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
    if turbolog_offline() {
        return Err(anyhow!(offline_model_message(dir)));
    }
    std::fs::create_dir_all(dir)?;
    const BASE: &str = "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main";
    if !model.exists() {
        eprintln!(
            "[turbolog] First run: downloading model (~86 MB) to {} ...",
            dir.display()
        );
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
    let total_bytes = resp
        .header("Content-Length")
        .and_then(|value| value.parse::<u64>().ok());
    let file_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("model file");
    let tmp = dest.with_extension("part");

    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(&tmp)?;
    let mut buf = [0_u8; 64 * 1024];
    let mut downloaded = 0_u64;
    let mut next_report = PROGRESS_STEP_BYTES;

    report_download_progress(file_name, downloaded, total_bytes);
    loop {
        let read = std::io::Read::read(&mut reader, &mut buf)?;
        if read == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..read])?;
        downloaded += read as u64;
        if downloaded >= next_report {
            report_download_progress(file_name, downloaded, total_bytes);
            while downloaded >= next_report {
                next_report += PROGRESS_STEP_BYTES;
            }
        }
    }
    std::io::Write::flush(&mut file)?;
    std::fs::rename(tmp, dest)?;
    report_download_complete(file_name, downloaded, total_bytes);
    Ok(())
}

#[cfg(not(feature = "embedded-model"))]
fn turbolog_offline() -> bool {
    std::env::var("TURBOLOG_OFFLINE")
        .map(|value| env_flag_enabled(&value))
        .unwrap_or(false)
}

#[cfg(not(feature = "embedded-model"))]
fn env_flag_enabled(value: &str) -> bool {
    let normalized = value.trim().to_ascii_lowercase();
    !matches!(normalized.as_str(), "" | "0" | "false" | "no" | "off")
}

#[cfg(not(feature = "embedded-model"))]
fn offline_model_message(dir: &Path) -> String {
    format!(
        "Model files are missing and TURBOLOG_OFFLINE=1 is set. Supply model.onnx \
         and tokenizer.json in {} or set TURBOLOG_MODEL_DIR to a directory that \
         contains them. To populate ./models while online, run ./scripts/download_model.sh.",
        dir.display()
    )
}

#[cfg(not(feature = "embedded-model"))]
fn report_download_progress(file_name: &str, downloaded: u64, total_bytes: Option<u64>) {
    match total_bytes {
        Some(total) if total > 0 => {
            let pct = (downloaded as f64 / total as f64) * 100.0;
            eprintln!("[turbolog] downloading {file_name}: {downloaded}/{total} bytes ({pct:.1}%)");
        }
        _ => eprintln!("[turbolog] downloading {file_name}: {downloaded} bytes"),
    }
}

#[cfg(not(feature = "embedded-model"))]
fn report_download_complete(file_name: &str, downloaded: u64, total_bytes: Option<u64>) {
    match total_bytes {
        Some(total) if total > 0 => {
            eprintln!("[turbolog] downloaded {file_name}: {downloaded}/{total} bytes");
        }
        _ => eprintln!("[turbolog] downloaded {file_name}: {downloaded} bytes"),
    }
}

#[cfg(all(test, not(feature = "embedded-model")))]
mod tests {
    use super::*;

    #[test]
    fn env_flag_enabled_accepts_common_truthy_values() {
        for value in ["1", "true", "yes", "on", "anything"] {
            assert!(env_flag_enabled(value));
        }
    }

    #[test]
    fn env_flag_enabled_rejects_common_falsey_values() {
        for value in ["", "0", "false", "no", "off", " OFF "] {
            assert!(!env_flag_enabled(value));
        }
    }

    #[test]
    fn offline_model_message_points_to_model_sources() {
        let message = offline_model_message(Path::new("/tmp/turbolog-models"));
        assert!(message.contains("TURBOLOG_OFFLINE=1"));
        assert!(message.contains("TURBOLOG_MODEL_DIR"));
        assert!(message.contains("model.onnx"));
        assert!(message.contains("tokenizer.json"));
        assert!(message.contains("/tmp/turbolog-models"));
    }
}
