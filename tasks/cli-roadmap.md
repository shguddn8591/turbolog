# TurboLog — CLI Roadmap (Terminal-First)

> Target: terminal-centric, local-first developers.

## Layer 0 — Pipe Core ✅

- [x] `watch` / `scan` stdin piping
- [x] Embedded MiniLM ONNX model
- [x] `--explain` (Ollama / LM Studio)
- [x] `history` SQLite store

## Layer 1 — Shell Ergonomics ✅

- [x] `--only-anomalies` / `--quiet`
- [x] Exit codes (`0` ok, `1` anomalies, `2` error)
- [x] `turbolog completions` (bash, zsh, fish)
- [x] `arg_required_else_help` — no implicit `serve`

## Layer 2 — Local Diagnosis ✅

- [x] History-aware LLM context (`context_for`)
- [x] `turbolog diagnose`
- [x] `history --top`
- [x] Recurring pattern badge in `watch`

## Layer 3 — Terminal Integration

- [x] `turbolog ui --standalone`
- [ ] TUI history panel (read `history.db` in sidebar)
- [ ] Neovim integration guide / snippet pack
- [ ] fzf recipe collection in docs

## v1.0 CLI Definition of Done

- [x] 60-second onboarding (`install.sh` + demo pipe)
- [x] `--only-anomalies` as recommended daily workflow
- [x] Exit codes + completions
- [x] `diagnose --since 24h` from local history
- [x] No server required for core workflow
