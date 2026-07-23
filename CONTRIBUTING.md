# Contributing to TurboLog

Thanks for considering a contribution. TurboLog is a **local-first CLI** for log triage
(`watch` / `scan` / `history`), with an optional local LLM explain path.

It is **not** aiming to become a horizontally scaled observability platform.
Please read [`tasks/todo.md`](tasks/todo.md) before proposing large server/k8s work.

## Code of Conduct

Be respectful, collaborative, and constructive.

## Product direction (please follow)

1. **Prefer CLI improvements** that help someone pipe logs today (UX, detection triage quality, history, docs, install).
2. **`serve` / `ui` / `deploy/`** are experimental. Changes there should be labeled as such and must not claim production readiness while `/health` and friends are missing.
3. **Do not revive “1M concurrent connections”** as a goal in docs or issues unless maintainers explicitly reopen that product bet.
4. Novelty scoring ≠ proven incident — keep user-facing language honest.

## Setting up the development environment

1. Clone and enter the repo:
   ```bash
   git clone https://github.com/shguddn8591/turbolog.git
   cd turbolog
   ```

2. Download the ONNX model (required for embedding tests):
   ```bash
   ./scripts/download_model.sh
   ```

3. Rust **1.88+**:
   ```bash
   rustup update stable
   ```

4. Build & test (CLI defaults):
   ```bash
   cargo build
   cargo test
   ```

5. Optional feature builds:
   ```bash
   cargo build --features server
   cargo build --features tui
   cargo test --all-features
   ```

## Code style & standards

- Format: `cargo fmt --all -- --check`
- Lint: `cargo clippy --all-targets --all-features -- -D warnings`
- Avoid leaving unresolved `TODO` / `FIXME` / `HACK` in release paths
- MSRV: Rust `1.88.0` — do not bump casually

## Architecture invariants

These still apply to detection and the experimental engine:

1. **No dynamic re-training by default:** K-means centroids are frozen after calibration for a process lifetime. Online re-learning is a deliberate product change (and fights the current CLI session model) — discuss before implementing.
2. **Hard physical deletion (server chunks):** Retention unlinks hourly segment directories; avoid per-vector delete loops.
3. **Stateless Embedder:** No cross-request mutable state inside an embedder instance so pooling stays safe.

CLI path note: `LocalPipeline` is the primary runtime for `watch`/`scan`. Keep it correct even when the HTTP engine diverges.

## Pull request guidelines

1. Branch off `main` (or the agreed base); do not commit directly to `main`.
2. Prefer [Conventional Commits](https://www.conventionalcommits.org/):
   - `feat(cli): …`
   - `fix(detect): …`
   - `docs: align OPERATIONS with CLI-first scope`
3. Include tests for behavior changes.
4. Update README / `docs/` / `tasks/todo.md` when you change user-facing behavior or roadmap scope.
5. For experimental server work: state limitations in the PR body (missing probes, etc.).

Questions? Open a Discussion or an issue with the `enhancement` / `bug` templates.
