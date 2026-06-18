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

TurboLog is a **local-first log anomaly detector** for solo developers.
Pipe your logs in, get anomalies out — with optional one-line AI explanations from your local Ollama or LM Studio.

```
[ANOMALY 0.91] OOM killer activated for pid 4821
  └─ Kernel killed the process due to memory exhaustion. Check memory limits and RSS growth.

[ANOMALY 0.87] Connection refused to postgres:5432 after 3 retries
  └─ Connection pool likely exhausted or DB is down. Check pg_stat_activity and pool settings.
```

**Under the hood**: Drain template extraction → all-MiniLM-L6-v2 ONNX embedding (CPU, no GPU) → k-means centroid anomaly detection. All in one binary, no external services.

---

## Install

```bash
cargo install turbolog
```

Or download a prebuilt binary from [Releases](https://github.com/shguddn8591/turbolog/releases).

---

## Quick Start

```bash
# Real-time anomaly detection from stdin
cat app.log | turbolog watch

# With LLM explanation (auto-detects Ollama on :11434 or LM Studio on :1234)
cat app.log | turbolog watch --explain

# Scan a file and print a report
turbolog scan < app.log

# Scan with AI analysis of top anomalies
turbolog scan --explain < app.log

# Scan and get machine-readable JSON
turbolog scan --format json < app.log

# Query stored anomaly history
turbolog history --since 24h
turbolog history --since 7d --template "connection" --format json
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
| `--llm-url`, `--llm-model` | Same as `watch` |

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
| <code>--format text&#124;json</code> | Output format (default: `text`) |
| `--limit <N>` | Max rows to return (default: `50`) |

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

## Roadmap

- [x] Drain template parsing + LRU vector cache
- [x] K-means anomaly detection (calibration → detection)
- [x] WAL crash recovery + hourly chunk compaction
- [x] HTTP server with ingest / search / stats API
- [x] `turbolog watch` — pipe CLI real-time streaming
- [x] `turbolog scan` — batch scan with JSON output
- [x] Embedded all-MiniLM-L6-v2 ONNX model (CPU, no GPU)
- [x] `--explain` flag — Ollama / LM Studio anomaly explanation
- [x] SQLite anomaly history (`~/.local/share/turbolog/history.db`)
- [x] `turbolog history` — query past anomalies
- [x] TUI dashboard (`turbolog ui`)
- [x] GitHub Release automation + `cargo install` via crates.io
- [ ] `turbolog diagnose` — root cause analysis across a time window
- [ ] History-aware explanation context (recurring pattern detection)
- [ ] VS Code / Neovim extension

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md).

## License

[MIT](LICENSE) © 2025

---

<p align="center">
  <sub>If TurboLog saved you from waking up at 3am to stare at raw logs, consider giving it a ⭐</sub>
</p>
