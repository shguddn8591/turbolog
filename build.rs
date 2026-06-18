// Download all-MiniLM-L6-v2 ONNX model into models/ when building with
// the embedded-model feature. Skipped if files already exist (cached).
// Set TURBOLOG_SKIP_MODEL_DOWNLOAD=1 to disable (e.g. offline builds).

fn main() {
    if std::env::var("CARGO_FEATURE_EMBEDDED_MODEL").is_err() {
        return;
    }
    if std::env::var("TURBOLOG_SKIP_MODEL_DOWNLOAD").is_ok() {
        return;
    }

    let root = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let dir = root.join("models");
    let model = dir.join("model.onnx");
    let tokenizer = dir.join("tokenizer.json");

    if model.exists() && tokenizer.exists() {
        println!("cargo:rerun-if-changed=models/model.onnx");
        println!("cargo:rerun-if-changed=models/tokenizer.json");
        return;
    }

    std::fs::create_dir_all(&dir).expect("Failed to create models/");

    const BASE: &str =
        "https://huggingface.co/sentence-transformers/all-MiniLM-L6-v2/resolve/main";

    if !model.exists() {
        println!("cargo:warning=Downloading all-MiniLM-L6-v2 model (~86 MB)...");
        fetch(&format!("{BASE}/onnx/model.onnx"), &model);
    }
    if !tokenizer.exists() {
        fetch(&format!("{BASE}/tokenizer.json"), &tokenizer);
    }
    println!("cargo:warning=Model ready.");

    println!("cargo:rerun-if-changed=models/model.onnx");
    println!("cargo:rerun-if-changed=models/tokenizer.json");
}

fn fetch(url: &str, dest: &std::path::Path) {
    let st = std::process::Command::new("curl")
        .args(["--fail", "--location", "--silent", "--show-error", "-o"])
        .arg(dest)
        .arg(url)
        .status()
        .unwrap_or_else(|e| panic!("curl not found: {e}"));
    if !st.success() {
        panic!("Failed to download {url} (curl exit {:?})", st.code());
    }
}
