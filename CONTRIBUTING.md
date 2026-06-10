# Contributing to TurboLog

Thank you for your interest in contributing to TurboLog! As an ultralight time-series log vector engine, we welcome community contributions to make this project faster, safer, and more robust.

Please take a moment to review this document before submitting your pull requests.

## 🤝 Code of Conduct

By participating in this project, you agree to abide by our Code of Conduct (be respectful, collaborative, and constructive).

## 🚀 Setting Up the Development Environment

1. **Clone the Repository:**
   ```bash
   git clone https://github.com/shguddn8591/turbolog.git
   cd turbolog
   ```

2. **Download Required Models:**
   TurboLog uses an ONNX model for text log embedding. Execute the following script to download it:
   ```bash
   ./scripts/download_model.sh
   ```

3. **Install Rust:**
   Make sure you have Rust 1.70+ installed.
   ```bash
   rustup update stable
   ```

4. **Verify Build & Run Tests:**
   ```bash
   cargo build
   cargo test
   ```

## 🛠️ Code Style & Standards

We enforce strict formatting and linting rules to keep the codebase clean.

- **Formatting:** Always format your code before committing.
  ```bash
  cargo fmt --all -- --check
  ```
- **Lints:** Ensure there are no warnings or clippy violations.
  ```bash
  cargo clippy --all-targets --all-features -- -D warnings
  ```
- **No TODO Markers:** Avoid leaving unresolved `TODO`, `FIXME`, or `HACK` comments in release paths.
- **MSRV:** We maintain compatibility with Rust `1.70.0`. Do not introduce language features that break older versions unless there's a strong consensus to bump the MSRV.

## 🔬 Core Architecture Invariants

Any contribution touching the core engine must respect these design constraints (Spec v1.0 §4):

1. **No Dynamic Re-training:** The K-means centroids and rotation matrices are frozen after the initial calibration phase. Incremental online re-learning is prohibited to prevent vector space drift.
2. **Hard Physical Deletion:** Retention policies are enforced by unlinking hourly segment directory chunks at the OS level. Avoid executing per-vector removal loops to prevent fragmentation.
3. **Stateless Embedder:** The embedder instance must not keep state between requests, allowing it to scale horizontally on separate thread pools.

## 📥 Pull Request Guidelines

1. **Create a Feature Branch:** Always work on a new branch instead of committing directly to `main`.
   ```bash
   git checkout -b feature/your-awesome-feature
   ```
2. **Commit Messages:** Follow the [Conventional Commits](https://www.conventionalcommits.org/) specification:
   - `feat: add gRPC ingestion server`
   - `fix: prevent race condition during segment swap`
   - `docs: improve API documentation examples`
3. **Add Tests:** Every bug fix or new feature must be accompanied by relevant unit or integration tests.
4. **Update Documentation:** If you modify engine options or add API endpoints, update the README and code comments.

If you have any questions, feel free to open a Discussion or join our Discord community!
