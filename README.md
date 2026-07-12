<p align="center">
  <h1 align="center">⚡ TurboLog</h1>
  <p align="center">
    <strong>Pipe-friendly log anomaly detection with local LLM explanation</strong><br>
    No API key. No Python. No cloud. Just pipe.
  </p>
  <p align="center">
    <a href="https://github.com/shguddn8591/turbolog/actions"><img src="https://img.shields.io/github/actions/workflow/status/shguddn8591/turbolog/ci.yml?branch=main&style=flat-square&logo=github&label=CI" alt="CI"></a>
    <a href="https://codecov.io/gh/shguddn8591/turbolog"><img src="https://img.shields.io/codecov/c/github/shguddn8591/turbolog?style=flat-square&logo=codecov" alt="Coverage"></a>
    <a href="https://crates.io/crates/turbolog"><img src="https://img.shields.io/crates/v/turbolog?style=flat-square&logo=rust" alt="Crates.io"></a>
    <a href="https://github.com/shguddn8591/turbolog/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
  </p>
</p>

---

TurboLog is a **local-first log anomaly detector** for **terminal-centric developers**.
Pipe your logs in, get anomalies out — with optional one-line AI explanations from your local Ollama or LM Studio.

TurboLog learns your normal traffic, then flags the outliers:

```
$ tail -f app.log | turbolog watch --only-anomalies
[ANOMALY 1.03] ERROR OOM killer activated for pid 4821 memory exhausted
[ANOMALY 1.07] ERROR connection refused to postgres 5432 after 3 retries
  ↻ recurring pattern
[ANOMALY 1.08] FATAL segfault in worker thread null pointer dereference
```

> **Anomaly ≠ error.** TurboLog flags lines that are *statistically unusual* relative
> to the baseline it just learned — novel, rare, or off-distribution messages. It does
> **not** parse severity or understand meaning. Two consequences worth knowing:
> - A frequent, expected `ERROR` (e.g. a health check that always 500s) becomes part of
>   the baseline and **won't** be flagged.
> - An unusual but harmless line (a new deploy banner, a first-time INFO message) **can**
>   be flagged until it's seen enough to look normal.
>
> Think of it as "this doesn't look like your usual traffic," not "this is broken."
> Use `--explain` when you want a local LLM to judge whether a flagged line actually matters.

**Two AI layers — only one is required:**
- **MiniLM** (built-in, always on): a 86 MB ONNX model baked into the binary. Powers anomaly detection. No API key, no internet at runtime.
- **LLM** (optional, `--explain` only): calls your locally running [Ollama](https://ollama.ai) or [LM Studio](https://lmstudio.ai) to explain *why* a line looks anomalous. Zero config — TurboLog auto-detects them.

---

## Install

**Step 1 — install Rust** (skip if you already have it):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**Step 2 — install TurboLog:**

```bash
cargo install turbolog
```

> The first build downloads the embedded MiniLM model (~86 MB) and bakes it into the binary.
> After that, `turbolog` runs fully offline — no API key, no cloud.

Alternatively, grab a prebuilt binary (no Rust needed) from [Releases](https://github.com/shguddn8591/turbolog/releases).

---

## Quick Start

Point TurboLog at your live logs:

```bash
tail -f /var/log/app.log | turbolog watch --only-anomalies
```

> **How detection works:** TurboLog calibrates on your normal traffic first, then
> flags outliers. `watch` calibrates as a live stream accumulates templates (or after a
> 500-line warm-up). `scan` reads a batch to EOF — best for a file whose anomalies are
> *novel* relative to the bulk of the log, since it calibrates on the same batch it scores.

With your own logs:

```bash
# Scan a file and print an anomaly report
turbolog scan < app.log

# Real-time monitoring (pipe any stream)
tail -f /var/log/app.log | turbolog watch

# Add plain-English explanations (requires Ollama or LM Studio running locally)
tail -f /var/log/app.log | turbolog watch --explain

# Batch scan with AI analysis of top anomalies
turbolog scan --explain < app.log

# Machine-readable output
turbolog scan --format json < app.log

# Shell-friendly: only anomalies, quiet status noise
tail -f /var/log/app.log | turbolog watch --only-anomalies --quiet

# CI gate: exit 1 when anomalies found
turbolog scan < build.log || echo "anomalies detected"

# Query stored anomaly history
turbolog history --since 24h
turbolog history --top --since 7d --format json
turbolog history --since 7d --template "connection" --format json

# Diagnose recurring patterns from local history
turbolog diagnose --since 24h
turbolog diagnose --since 2h --explain --format json
```

---

## Subcommands

### `watch` — Real-time streaming

Reads stdin line-by-line and highlights anomalies as they arrive.

```bash
tail -f /var/log/app.log | turbolog watch
tail -f /var/log/app.log | turbolog watch --explain
tail -f /var/log/app.log | turbolog watch --threshold 0.8
```

| Flag | Description |
|---|---|
| `--explain` | Call local LLM to explain each anomaly |
| `--only-anomalies` | Print only anomalous lines (hide normal/calibrating) |
| `--quiet` / `-q` | Suppress status messages on stderr |
| `--threshold <f32>` | Override auto-calibrated anomaly score floor |
| `--llm-url <url>` | LLM base URL (default: auto-detect). Also: `TURBOLOG_LLM_URL` |
| `--llm-model <name>` | LLM model name (default: `llama3.2`). Also: `TURBOLOG_LLM_MODEL` |

Output format:

```
[calibrating]        app started on port 8080          ← first 64 unique templates
INFO                 request processed in 12ms          ← normal line (no prefix)
[ANOMALY 0.91]       OOM killer activated for pid 4821  ← anomaly (red)
  └─ Memory exhausted; check process RSS and limits.    ← LLM explanation (cyan)
```

### `scan` — Batch scan to EOF

Reads all of stdin, then prints a summary report.

```bash
turbolog scan < app.log
turbolog scan --explain < app.log
turbolog scan --format json < app.log
turbolog scan --format json --explain < app.log
```

| Flag | Description |
|---|---|
| <code>--format text&#124;json</code> | Output format (default: `text`) |
| `--explain` | Explain top 5 anomalies with local LLM |
| `--quiet` / `-q` | Suppress status messages on stderr |
| `--llm-url`, `--llm-model` | Same as `watch` |

**Exit codes:** `0` = no anomalies, `1` = anomalies detected, `2` = error.

Text report:

```
--- TurboLog Scan Report ---
Lines processed : 8432
Templates found : 47
Anomalies       : 12 (0.14%)

Top anomalies:
  [score=0.94] OOM killer activated for pid 4821
    └─ Memory pressure triggered kernel OOM killer...
  [score=0.87] Connection refused to postgres:5432
    └─ Database connection pool exhausted...
```

JSON report adds `"explanation"` field per anomaly when `--explain` is set.

### `history` — Query anomaly history

Every detected anomaly is stored in `~/.local/share/turbolog/history.db` (SQLite). Query it later:

```bash
turbolog history                              # last 7 days
turbolog history --since 1h                  # last hour
turbolog history --since 30d --limit 100     # last 30 days, up to 100 rows
turbolog history --template "connection"     # filter by template substring
turbolog history --format json               # JSON output for piping
```

| Flag | Description |
|---|---|
| `--since <DURATION>` | Look back this far: `7d`, `24h`, `1h`, `30m` (default: `7d`) |
| `--template <PATTERN>` | Filter by Drain template substring |
| `--top` | Group by template, sorted by frequency (most common first) |
| <code>--format text&#124;json</code> | Output format (default: `text`) |
| `--limit <N>` | Max rows to return (default: `50`) |

### `diagnose` — Pattern summary from history

Summarize recurring anomaly templates over a time window (reads `~/.local/share/turbolog/history.db`).

```bash
turbolog diagnose --since 24h
turbolog diagnose --since 2h --explain
turbolog diagnose --since 7d --format json --limit 5
```

| Flag | Description |
|---|---|
| `--since <DURATION>` | Look back this far (default: `24h`) |
| <code>--format text&#124;json</code> | Output format (default: `text`) |
| `--explain` | Summarize patterns with local LLM |
| `--limit <N>` | Max patterns to include (default: `10`) |
| `--quiet` / `-q` | Suppress status messages on stderr |

### `completions` — Shell tab completion

```bash
turbolog completions bash > ~/.local/share/bash-completion/completions/turbolog
turbolog completions zsh  > ~/.zfunc/_turbolog
turbolog completions fish > ~/.config/fish/completions/turbolog.fish
```

When `--explain` is active in `watch` or `scan`, history entries also store the LLM explanation and use it as context for future occurrences of the same pattern:

```
[ANOMALY 0.87] Connection refused to postgres:5432
  └─ Context: seen 3× in the last 7 days (last seen: 2h ago)
     Connection pool likely exhausted. Check pg_stat_activity.
```

### `ui` — TUI dashboard

A real-time terminal dashboard. Connects to a running `turbolog serve` server, or reads stdin locally in standalone mode.

```bash
# Standalone mode (no server needed)
turbolog ui --standalone < app.log

# Connect to a server
turbolog ui --server http://localhost:8087
```

### `serve` — HTTP server daemon

For centralized deployment. Accepts logs over HTTP, stores them, and serves search/stats endpoints.

```bash
turbolog serve
# => TurboLog listening on http://0.0.0.0:8087
```

---

## LLM Integration

TurboLog auto-detects a running local LLM on startup when `--explain` is passed:

| Priority | Server | Default port |
|---|---|---|
| 1 | `TURBOLOG_LLM_URL` env var | — |
| 2 | [Ollama](https://ollama.ai) | `:11434` |
| 3 | [LM Studio](https://lmstudio.ai) | `:1234` |

Any OpenAI-compatible `/v1/chat/completions` endpoint works.

```bash
# Use a specific model
turbolog watch --explain --llm-model mistral

# Use a remote endpoint
turbolog watch --explain --llm-url http://192.168.1.10:11434

# Via environment variables
TURBOLOG_LLM_URL=http://localhost:11434 TURBOLOG_LLM_MODEL=llama3.2 \
  cat app.log | turbolog watch --explain
```

If no LLM is found, `watch` and `scan` work normally — `--explain` is a no-op.

---

## Environment Variables

### CLI (watch / scan / history)

| Variable | Description |
|---|---|
| `TURBOLOG_MODEL_DIR` | Directory containing `model.onnx` and `tokenizer.json` (default: `./models`) |
| `TURBOLOG_LLM_URL` | LLM base URL override |
| `TURBOLOG_LLM_MODEL` | LLM model name override |

### Server (serve)

| Variable | Default | Description |
|---|---|---|
| `TURBOLOG_PORT` | `8087` | HTTP listen port |
| `TURBOLOG_DATA_DIR` | `./data` | WAL and chunk segments directory |
| `TURBOLOG_MODEL_DIR` | `./models` | ONNX model directory |
| `TURBOLOG_EMBEDDERS` | `2` | Embedder pool size (~90 MB each) |
| `TURBOLOG_AUTH_TOKEN` | _(unset)_ | Bearer token for all endpoints |

---

## How It Works

```
stdin line
    │
    ▼
┌─────────────────────────────────────────┐
│  1. Drain Parser                        │
│     "OOM killer pid 4821" →             │
│     template: "OOM killer pid <*>"      │
└───────────────┬─────────────────────────┘
                │
                ▼
┌─────────────────────────────────────────┐
│  2. LRU Vector Cache                    │
│     Known template? → cached 384-dim   │
│     vector (zero compute)               │
│     New template? → ONNX inference      │
│     (all-MiniLM-L6-v2, CPU only)        │
└───────────────┬─────────────────────────┘
                │
                ▼
┌─────────────────────────────────────────┐
│  3. K-means Anomaly Detection           │
│     Calibration: first 64 unique        │
│     templates → fit k=8 centroids       │
│     Detection: centroid distance >      │
│     threshold → anomaly                 │
└───────────────┬─────────────────────────┘
                │
          is_anomaly?
          ┌────┴─────┐
          │ yes      │ no
          ▼          ▼
   ┌─────────────┐  print line
   │ LLM explain │  as-is
   │ (optional)  │
   └──────┬──────┘
          │
          ▼
   SQLite history
   (~/.local/share/turbolog/history.db)
```

**Two AI layers:**
- **MiniLM** (always on): fast, local, no network — detects anomalies in milliseconds
- **LLM** (optional): explains anomalies in plain English — only called on anomalous lines

---

## Building from Source

```bash
git clone https://github.com/shguddn8591/turbolog.git
cd turbolog

# Download the ONNX model (~86 MB, required for embedding)
./scripts/download_model.sh

# Build
cargo build --release

# Build with TUI support
cargo build --release --features tui

# Run tests
cargo test
```

**Minimum Rust version**: 1.88 (stable)

---

## HTTP API (serve mode)

### `POST /logs` — Ingest

```bash
curl -X POST http://localhost:8087/logs \
  -H "Content-Type: application/json" \
  -d '{"logs": ["disk usage at 95%", "connection timeout"]}'
```

### `POST /search` — Semantic search

```bash
curl -X POST http://localhost:8087/search \
  -H "Content-Type: application/json" \
  -d '{"query": "disk full error", "k": 5}'
```

### `GET /stats` — Engine stats

```bash
curl http://localhost:8087/stats
```

---

## Shell Recipes

```bash
# ~/.bashrc or ~/.zshrc
alias tw='turbolog watch --only-anomalies'
alias ts='turbolog scan --format json'
alias td='turbolog diagnose --since 24h'

# Dev server logs
npm run dev 2>&1 | turbolog watch --explain

# Docker
docker logs -f myapp 2>&1 | turbolog watch --only-anomalies --quiet

# jq + fzf over history
turbolog history --top --format json | jq -r '.[] | "\(.count)x \(.template)"' | fzf

# CI anomaly gate
turbolog scan < build.log || exit 1
```

See [docs/CLI.md](docs/CLI.md) for the full terminal-first CLI reference.

A ready-to-run demo script (`scripts/demo.sh`) lands in a follow-up change.

---

## Roadmap

> For terminal-first, local-first developers. Pipe in, anomalies out.

### Layer 0 — Pipe Core ✅
- [x] `watch` / `scan` with stdin piping
- [x] Embedded MiniLM (offline anomaly detection)
- [x] `--explain` via Ollama / LM Studio
- [x] `history` — SQLite anomaly memory

### Layer 1 — Shell Ergonomics ✅
- [x] `--only-anomalies` / `--quiet` flags
- [x] Exit codes for shell scripts (`1` = anomalies found)
- [x] bash / zsh / fish completions (`turbolog completions`)
- [x] Default command shows help (not HTTP server)

### Layer 2 — Local Diagnosis ✅
- [x] History-aware LLM context (recurring patterns)
- [x] `turbolog diagnose` — time-window pattern summary
- [x] `history --top` — most frequent anomaly templates
- [x] Recurring pattern badge in `watch` output

### Layer 3 — Terminal Integration
- [x] TUI dashboard (`turbolog ui --standalone`)
- [ ] TUI history panel
- [ ] Neovim integration guide

### Advanced (self-hosters)
- [x] `serve` HTTP API (feature `server`)
- [x] Docker / k8s manifests
- [ ] `/health`, `/ready`, `/metrics` HTTP endpoints

See [tasks/cli-roadmap.md](tasks/cli-roadmap.md) and [tasks/advanced-server.md](tasks/advanced-server.md).

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE) © 2025

---

<p align="center">
  <sub>If TurboLog saved you from waking up at 3am to stare at raw logs, consider giving it a ⭐</sub>
</p>
