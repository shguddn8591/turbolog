<p align="center">
  <h1 align="center">⚡ TurboLog</h1>
  <p align="center">
    <strong>Pipe-friendly log triage with local anomaly highlighting</strong><br>
    No API key. No Python. No cloud dependency at runtime. Just pipe.
  </p>
  <p align="center">
    <a href="https://github.com/shguddn8591/turbolog/actions"><img src="https://img.shields.io/github/actions/workflow/status/shguddn8591/turbolog/ci.yml?branch=main&style=flat-square&logo=github&label=CI" alt="CI"></a>
    <a href="https://codecov.io/gh/shguddn8591/turbolog"><img src="https://img.shields.io/codecov/c/github/shguddn8591/turbolog?style=flat-square&logo=codecov" alt="Coverage"></a>
    <a href="https://crates.io/crates/turbolog"><img src="https://img.shields.io/crates/v/turbolog?style=flat-square&logo=rust" alt="Crates.io"></a>
    <a href="https://github.com/shguddn8591/turbolog/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
  </p>
</p>

---

TurboLog is a **local-first CLI** for solo developers and small teams.
Pipe logs in, get the unusual lines out — optionally with a one-line explanation from your local Ollama or LM Studio.

It is a **log triage assistant**, not a hosted observability platform.
Novelty (distance from calibrated “normal” templates) is highlighted; that is useful while watching a live stream or scanning a failure log. It is **not** a substitute for metrics, traces, or production alerting.

```
[ANOMALY 0.91] OOM killer activated for pid 4821
  └─ Kernel killed the process due to memory exhaustion. Check memory limits and RSS growth.

[ANOMALY 0.87] Connection refused to postgres:5432 after 3 retries
  └─ Connection pool likely exhausted or DB is down. Check pg_stat_activity and pool settings.
```

**Two layers — only one is required:**
- **MiniLM** (built-in, always on): an ~86 MB ONNX model. Powers novelty / anomaly scoring. No API key; no network needed at runtime after the model is present.
- **LLM** (optional, `--explain` only): calls a locally running [Ollama](https://ollama.ai) or [LM Studio](https://lmstudio.ai) to narrate *why* a line looks unusual. Auto-detected. Never required for detection.

**Product focus:** `watch` · `scan` · `history`.  
`serve` and `ui` exist behind Cargo features and are **experimental** — see [Experimental: serve / ui](#experimental-serve--ui).

---

## Install

**Step 1 — install Rust** (skip if you already have it):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**Step 2 — install TurboLog (CLI defaults):**

```bash
cargo install turbolog
```

> The first build downloads the embedded MiniLM model (~86 MB) and bakes it into the binary
> (or fetches it on first run if needed). After that, detection runs offline.
> Default install includes `embedded-model` only — not the HTTP server or TUI.

Alternatively, grab a prebuilt binary from [Releases](https://github.com/shguddn8591/turbolog/releases).

Optional feature builds:

```bash
cargo install turbolog --features server   # experimental HTTP daemon
cargo install turbolog --features tui      # experimental terminal UI
cargo install turbolog --features server,tui
```

---

## Quick Start

No log file? Paste this to try it immediately:

```bash
printf 'user login OK\nrequest processed in 12ms\nOOM killer activated for pid 4821\ndisk usage at 99%%\ncache miss while fetching session key\ndatabase migration completed\nbackup job finished cleanly\npayment authorized through gateway\n' \
  | turbolog scan
```

> `scan` reads to EOF and calibrates once it has at least 8 distinct templates, so it works on small batches.
> `watch` is for live streams (`tail -f`) and calibrates as templates accumulate.

With your own logs:

```bash
# Scan a file and print an anomaly report
turbolog scan < app.log

# Real-time monitoring (pipe any stream)
tail -f /var/log/app.log | turbolog watch
docker logs -f my-app 2>&1 | turbolog watch --only-anomalies

# Add plain-English explanations (requires Ollama or LM Studio locally)
tail -f /var/log/app.log | turbolog watch --explain

# Batch scan with AI analysis of top anomalies
turbolog scan --explain < app.log

# Machine-readable output (good for scripts / CI)
turbolog scan --format json < app.log

# Query stored anomaly history
turbolog history --since 24h
turbolog history --since 7d --template "connection" --format json
```

**What the score means:** higher ≈ farther from calibrated normal templates (novelty).
It is **not** a calibrated probability of “something is broken.” Tune with `--threshold` when needed.

---

## Subcommands

### `watch` — Real-time streaming

Reads stdin line-by-line and highlights anomalies as they arrive.

```bash
tail -f /var/log/app.log | turbolog watch
tail -f /var/log/app.log | turbolog watch --explain
tail -f /var/log/app.log | turbolog watch --threshold 0.8
tail -f /var/log/app.log | turbolog watch --only-anomalies
```

| Flag | Description |
|---|---|
| `--explain` | Call local LLM to explain each anomaly |
| `--threshold <f32>` | Override auto-calibrated anomaly score floor |
| `--only-anomalies` | Print anomaly lines only |
| `--quiet` | Suppress calibrating / status noise |
| `--llm-url <url>` | LLM base URL (default: auto-detect). Also: `TURBOLOG_LLM_URL` |
| `--llm-model <name>` | LLM model name (default: `llama3.2`). Also: `TURBOLOG_LLM_MODEL` |

Exit codes: `0` = no anomalies, `1` = anomalies seen, `2` = error.

Output format:

```
[calibrating 1/64]   app started on port 8080          ← calibration progress
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
| `--threshold <f32>` | Override auto threshold |
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

When `--explain` is active in `watch` or `scan`, history entries also store the LLM explanation and can reuse it as context for later occurrences of the same pattern:

```
[ANOMALY 0.87] Connection refused to postgres:5432
  └─ Context: seen 3× in the last 7 days (last seen: 2h ago)
     Connection pool likely exhausted. Check pg_stat_activity.
```

### `completions` — Shell completions

```bash
turbolog completions bash
turbolog completions zsh
turbolog completions fish
```

---

## Experimental: `serve` / `ui`

These require a feature build and are **not** the supported product path today.

```bash
# Rebuild with features
cargo install turbolog --features server,tui

# HTTP daemon (trusted network / localhost). Optional bearer auth via TURBOLOG_AUTH_TOKEN.
turbolog serve

# TUI — standalone stdin, or attach to a running serve instance
turbolog ui --standalone < app.log
turbolog ui --server http://localhost:8087
```

**Known gaps (do not deploy as production observability):**

- HTTP exposes `/logs`, `/search`, `/stats` only — **no** `/health`, `/ready`, or `/metrics` yet
- No request backpressure / graceful shutdown wired end-to-end
- `deploy/` manifests are **scaffolding** only — see [deploy/README.md](deploy/README.md)

See [docs/OPERATIONS.md](docs/OPERATIONS.md) for an honest ops note, and [tasks/todo.md](tasks/todo.md) for the CLI-first roadmap.

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

# Use a remote endpoint on your LAN
turbolog watch --explain --llm-url http://192.168.1.10:11434

# Via environment variables
TURBOLOG_LLM_URL=http://localhost:11434 TURBOLOG_LLM_MODEL=llama3.2 \
  cat app.log | turbolog watch --explain
```

If no LLM is found, `watch` and `scan` work normally — `--explain` is a no-op.

---

## Environment Variables

### CLI (watch / scan / history) — primary

| Variable | Description |
|---|---|
| `TURBOLOG_MODEL_DIR` | Directory containing `model.onnx` and `tokenizer.json` (default: `./models`) |
| `TURBOLOG_LLM_URL` | LLM base URL override |
| `TURBOLOG_LLM_MODEL` | LLM model name override |
| `NO_COLOR` | Disable ANSI colors when set |

### Server (`serve`, experimental)

| Variable | Default | Description |
|---|---|---|
| `TURBOLOG_PORT` | `8087` | HTTP listen port |
| `TURBOLOG_BIND` | `127.0.0.1` | Bind address |
| `TURBOLOG_DATA_DIR` | `./data` | WAL and chunk segments directory |
| `TURBOLOG_MODEL_DIR` | `./models` | ONNX model directory |
| `TURBOLOG_EMBEDDERS` | `2` | Embedder pool size (~90 MB each) |
| `TURBOLOG_AUTH_TOKEN` | _(unset)_ | Bearer token for endpoints |

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
│  3. K-means novelty screen              │
│     Calibration: early unique           │
│     templates → fit centroids once      │
│     Detection: distance > threshold     │
│     → flagged (novelty ≠ proven outage) │
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

Centroids are **frozen after calibration** (no online re-training). That keeps the CLI predictable for a session; long-running streams with heavy drift may need a restart or a manual `--threshold`.

---

## Building from Source

```bash
git clone https://github.com/shguddn8591/turbolog.git
cd turbolog

# Download the ONNX model (~86 MB, required for embedding)
./scripts/download_model.sh

# CLI (default)
cargo build --release

# Optional features
cargo build --release --features tui
cargo build --release --features server

# Run tests
cargo test
```

**Minimum Rust version**: 1.88 (stable)

---

## HTTP API (`serve`, experimental)

Implemented today:

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

**Not implemented yet:** `/health`, `/ready`, `/metrics`. Do not point Kubernetes probes at this binary until those exist.

---

## Roadmap

Tracked in detail in [`tasks/todo.md`](tasks/todo.md). Summary:

**Primary (CLI triage)**
- [x] Drain templates + LRU vector cache + MiniLM ONNX
- [x] `watch` / `scan` / `history` + optional `--explain`
- [x] Embedded model + `cargo install` / Releases
- [x] Shell completions + script-friendly exit codes
- [ ] Stronger first-run / model UX
- [ ] Clearer calibration + threshold UX; recurring-pattern triage
- [ ] CI failure-log recipes + optional `diagnose`
- [ ] History-aware explanation context

**Experimental (not the product bet)**
- [x] Feature-gated HTTP engine + TUI (incomplete ops surface)
- [ ] Real health/ready/metrics + single-node compose that works
- [ ] VS Code / Neovim extension — deferred until CLI has external users

**Explicitly not pursuing:** multi-replica “1M concurrent connections” observability platform.

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Please prefer CLI-path improvements over platform-scale work unless discussed first.

## License

[MIT](LICENSE) © 2025

---

<p align="center">
  <sub>If TurboLog saved you from staring at a wall of logs, consider giving it a ⭐</sub>
</p>
