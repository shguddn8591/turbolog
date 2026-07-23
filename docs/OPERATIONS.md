# TurboLog Operations Guide

> Version: 0.3.0 | Last Modified: 2026-07-23  
> **Audience:** people running the **CLI** day to day, plus anyone poking at the **experimental** `serve` feature.

TurboLog’s supported product is a **local pipe CLI** (`watch` / `scan` / `history`).
It is **not** a production multi-replica observability platform. Older “1M concurrent connections”
topology docs are retired; do not use them as deployment guidance.

---

## Table of Contents

1. [CLI operations (supported)](#1-cli-operations-supported)
2. [Experimental `serve` (honest status)](#2-experimental-serve-honest-status)
3. [Environment variables](#3-environment-variables)
4. [Data & privacy](#4-data--privacy)
5. [Deploy scaffolding](#5-deploy-scaffolding)
6. [When to escalate to a real observability stack](#6-when-to-escalate-to-a-real-observability-stack)

---

## 1. CLI operations (supported)

### Typical workflows

```bash
# Live triage while developing
tail -f /var/log/app.log | turbolog watch
docker logs -f my-app 2>&1 | turbolog watch --only-anomalies

# Batch review / CI artifact
turbolog scan --format json < fail.log > anomalies.json

# Optional narration (local LLM only)
turbolog scan --explain < fail.log

# Past hits on this machine
turbolog history --since 24h --template "timeout"
```

### Calibration & scores

- `watch` calibrates on early unique templates (or a line-budget fallback on low-cardinality streams).
- `scan` can finalize calibration at EOF if at least 8 distinct templates were seen.
- **Score = novelty distance** from frozen centroids, not P(incident).
- Centroids stay frozen for the process lifetime — restart the CLI (or raise `--threshold`) if the log regime changes a lot.

### Exit codes

| Code | Meaning |
|------|---------|
| `0` | Success, no anomalies |
| `1` | Anomalies detected (`watch` / `scan`) |
| `2` | Runtime / usage error |

Useful for CI: fail the job when `scan` exits `1`, or parse `--format json` instead.

### History database

Path (Linux XDG-style): `~/.local/share/turbolog/history.db` (SQLite).

- Written when anomalies are detected in `watch` / `scan`.
- Local to the machine; not a multi-user store.
- Safe to delete if you want a clean slate (you lose recurrence context).

### Model files

Detection needs `model.onnx` + `tokenizer.json` (all-MiniLM-L6-v2).

- Default `cargo install` / embedded-model builds bake or fetch these.
- Override with `TURBOLOG_MODEL_DIR`.
- Offline builds: set `TURBOLOG_SKIP_MODEL_DOWNLOAD=1` at compile time and supply models yourself (`./scripts/download_model.sh` when online).

### Local LLM (`--explain`)

- Optional. Detection never depends on it.
- Auto-detect order: `TURBOLOG_LLM_URL` → Ollama `:11434` → LM Studio `:1234`.
- Explain requests time out (tens of seconds); on failure the anomaly line still prints without a footer.
- Treat explanations as **unverified hints**, not root-cause truth.

---

## 2. Experimental `serve` (honest status)

Build with `--features server`. Default bind is localhost-oriented (`TURBOLOG_BIND`, `TURBOLOG_PORT`).

### What works today

| Path | Method | Notes |
|------|--------|--------|
| `/logs` | POST | Batch ingest `{"logs":[...]}` (1 MiB body cap) |
| `/search` | POST | Semantic search `{"query","k"}` |
| `/stats` | GET | Engine counters |

Optional `TURBOLOG_AUTH_TOKEN` → require `Authorization: Bearer <token>` (constant-time compare).

### What does **not** work yet (do not pretend otherwise)

| Claim in older docs / manifests | Reality |
|----------------------------------|---------|
| `GET /health`, `/ready`, `/metrics` | **Not implemented** in `http.rs` |
| `TURBOLOG_MAX_INFLIGHT` backpressure | **Not wired** |
| Graceful SIGTERM → final `swap_tick` | **Not implemented** |
| Kubernetes probes in `deploy/k8s` | **Will fail** against current binary |
| Multi-replica consistent-hash mesh | **Unsupported product** |

`src/metrics.rs` exists as an in-process registry; it is **not** exposed over HTTP until someone lands that work under the experimental bar in `tasks/todo.md`.

### Intended niche (if revived)

Single-node / trusted-network daemonization of **one** log stream or host — e.g. local compose next to an app — **after** health endpoints exist. Not a cluster-wide log brain.

---

## 3. Environment variables

### CLI

| Variable | Default | Description |
|----------|---------|-------------|
| `TURBOLOG_MODEL_DIR` | `./models` | ONNX + tokenizer directory |
| `TURBOLOG_LLM_URL` | _(auto)_ | OpenAI-compatible base URL |
| `TURBOLOG_LLM_MODEL` | _(auto)_ | Model name for `/v1/chat/completions` |
| `NO_COLOR` | unset | Disable ANSI colors when set |

### `serve` (experimental)

| Variable | Default | Description |
|----------|---------|-------------|
| `TURBOLOG_PORT` | `8087` | Listen port |
| `TURBOLOG_BIND` | `127.0.0.1` | Bind address |
| `TURBOLOG_DATA_DIR` | `./data` | WAL / chunks |
| `TURBOLOG_MODEL_DIR` | `./models` | Models |
| `TURBOLOG_EMBEDDERS` | `2` | ONNX sessions (~90 MB each) |
| `TURBOLOG_AUTH_TOKEN` | unset | Optional bearer token |

---

## 4. Data & privacy

- **Local-first:** anomaly detection does not call a cloud API.
- **Model download** may hit the network at build or first run (Hugging Face / configured URL) unless you pre-provision models and skip download.
- **`--explain`** sends anomalous lines to whatever LLM URL you configured (often localhost).
- **History DB** retains anomaly text/templates on disk — treat like any other local log derivative (permissions, disk encryption, secrets in logs).

---

## 5. Deploy scaffolding

`deploy/docker-compose.yml` and `deploy/k8s/*` are **experimental scaffolding** retained for contributors exploring `serve`. They are **not** a supported production topology. Start from [`deploy/README.md`](../deploy/README.md).

Before using them even locally:

1. Build an image with `--features server`.
2. Replace placeholder registry/model URLs.
3. Leave healthchecks/probes commented out until `/health` / `/ready` exist in `http.rs` (see `deploy/README.md`).
4. Prefer binding to localhost or a private network; terminate TLS at a reverse proxy if you expose beyond the host.

There is no supported guide for HPA max=50, AZ anti-affinity, or “1M req/s” sizing. Those targets are out of product scope.

---

## 6. When to escalate to a real observability stack

Use TurboLog for **interactive triage** and light scripting. Reach for Loki/ELK/Datadog/etc. when you need:

- multi-tenant retention and search across a fleet
- SLO alerting on known failure modes
- RBAC, audit, compliance pipelines
- high-availability ingest

TurboLog’s frozen-centroid novelty screen is a poor fit for those jobs; don’t stretch it.
