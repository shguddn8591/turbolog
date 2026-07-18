# AGENTS.md

## Cursor Cloud specific instructions

TurboLog is a **single Rust crate** (`turbolog`) that builds one binary with several
subcommands: `scan` / `watch` / `history` (pipe-friendly CLI, always available),
plus optional `serve` (HTTP daemon, `server` feature) and `ui` (TUI, `tui` feature).
There is no separate frontend/backend — everything is this one Cargo project.

Standard build/lint/test/run commands are already documented in `README.md`
("Building from Source", "HTTP API") and `CONTRIBUTING.md`. Use those. The notes
below only cover non-obvious, environment-specific gotchas.

### Toolchain
- Uses Rust **stable** (currently 1.97). MSRV is **1.88** (`Cargo.toml` `rust-version`),
  so the default `nvm`/preinstalled `rustc 1.83` is too old — `rustup default stable`
  is already applied in the environment snapshot.

### System dependencies (already provided by the snapshot)
- `libopenblas-dev` (BLAS backend for the vector engine) and `libssl-dev` + `pkg-config`
  (`openssl-sys`, pulled in only via `--all-features`).
- **clang gotcha (important):** the base image's default `cc`/`c++` alternatives point at
  `clang`, which cannot find libstdc++ headers/libs. This breaks the C++ dep `esaxx-rs`
  (`cstdint file not found`) and final linking (`unable to find library -lstdc++`).
  The snapshot fixes this by pointing the alternatives at gcc/g++:
  `sudo update-alternatives --set cc /usr/bin/gcc && sudo update-alternatives --set c++ /usr/bin/g++`.
  If a fresh VM ever regresses to clang, re-run those two commands (or build with
  `CC=gcc CXX=g++` and `RUSTFLAGS="-C linker=gcc"`).

### ONNX model (required for embeddings / anomaly detection)
- The `all-MiniLM-L6-v2` model (`models/model.onnx` + `models/tokenizer.json`, ~87 MB)
  is **gitignored** and downloaded from HuggingFace. `build.rs` auto-downloads it on the
  first build with the default `embedded-model` feature; `scripts/download_model.sh` does
  the same manually. It is retained in the snapshot.
- Set `TURBOLOG_SKIP_MODEL_DOWNLOAD=1` to skip the `build.rs` network fetch when the
  model files already exist (useful for offline/restricted-egress builds).

### Running / testing gotchas
- Server search is **eventually consistent**: logs `POST`ed to `/logs` do not appear in
  `/search` until a background swap seals the pending window into the in-memory ring
  (default `swap_interval_secs = 10`). Wait ~10s (or watch `ring_vectors` in `/stats`)
  before expecting search hits.
- `serve` startup loads `TURBOLOG_EMBEDDERS` (default 2) copies of the ONNX model
  (~90 MB each), so it takes several seconds before it prints `TurboLog listening on ...`.
- CLI `history` persists to `~/.local/share/turbolog/history.db` (SQLite); `watch`/`scan`
  write every detected anomaly there automatically.
- `serve`/`ui` need their features enabled: `cargo build --features server` /
  `--features tui`, or `cargo build --all-features` for everything.
