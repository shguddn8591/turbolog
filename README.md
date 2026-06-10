<p align="center">
  <h1 align="center">⚡ TurboLog</h1>
  <p align="center">
    <strong>Ultralight time-series log vector engine</strong><br>
    Real-time indexing &amp; anomaly detection — no GPU, no heavyweight vector DB.
  </p>
  <p align="center">
    <a href="https://github.com/shguddn8591/turbolog/actions"><img src="https://img.shields.io/github/actions/workflow/status/shguddn8591/turbolog/ci.yml?branch=main&style=flat-square&logo=github&label=CI" alt="CI"></a>
    <a href="https://codecov.io/gh/shguddn8591/turbolog"><img src="https://img.shields.io/codecov/c/github/shguddn8591/turbolog?style=flat-square&logo=codecov" alt="Coverage"></a>
    <a href="https://crates.io/crates/turbolog"><img src="https://img.shields.io/crates/v/turbolog?style=flat-square&logo=rust" alt="Crates.io"></a>
    <a href="https://github.com/shguddn8591/turbolog/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
    <a href="https://github.com/shguddn8591/turbolog/stargazers"><img src="https://img.shields.io/github/stars/shguddn8591/turbolog?style=flat-square&logo=github" alt="Stars"></a>
  </p>
</p>

---

TurboLog ingests **thousands of log lines per second** on a single CPU core, automatically clusters them into templates, embeds them into vectors, and detects anomalies — all without touching a GPU or deploying Elasticsearch / Milvus / Pinecone.

Built on [turbovec](https://github.com/RyanCodrai/turbovec) (TurboQuant quantized vector index) for blazing-fast approximate nearest-neighbor search.

## ✨ Key Features

| Feature | Description |
|---|---|
| 🧠 **Drain-based Template Parsing** | Extracts static templates from dynamic log lines using the Drain algorithm — identical patterns share one embedding |
| ⚡ **LRU Vector Cache** | Known templates return cached vectors instantly (zero compute). Only novel patterns trigger ONNX inference |
| 🔍 **2-Tier Anomaly Detection** | Tier 1: O(k) centroid distance screening. Tier 2: quantized ANN deep search on anomalous vectors only |
| 🏓 **Ping-Pong Indexing** | `arc-swap`-based read/write index isolation — zero read-latency spikes during window swaps |
| 💾 **WAL Crash Recovery** | Write-ahead log survives process crashes. Truncated tails are safely ignored on replay |
| 📦 **Hourly Chunk Compaction** | Sealed windows are flushed to `.tvim` segment files; expired chunks are deleted at the OS level (no fragmentation) |
| 🌐 **HTTP API** | Simple JSON endpoints for log ingestion, semantic search, and engine stats |

## 🏗️ Architecture

```
                        ┌─────────────────────────────────────────────┐
                        │              TurboLog Engine                │
                        │                                             │
  Log line ──▶ ┌────────┴──────────┐    ┌──────────────┐              │
               │  Drain Parser     │───▶│  LRU Cache   │              │
               │  (template ID)    │    │  (10K slots)  │              │
               └───────────────────┘    └───┬──────────┘              │
                                            │                         │
                                  hit? ─────┤                         │
                                  │ yes     │ no                      │
                                  │         ▼                         │
                                  │  ┌──────────────┐                 │
                                  │  │ ONNX Embedder│                 │
                                  │  │ MiniLM-L6-v2 │                 │
                                  │  │ (384-dim CPU) │                 │
                                  │  └──────┬───────┘                 │
                                  │         │                         │
                                  ▼         ▼                         │
                              ┌──────────────────┐                    │
                              │    WAL Append     │                   │
                              │   (crash-safe)    │                   │
                              └────────┬─────────┘                    │
                                       │                              │
                              ┌────────▼─────────┐                    │
                              │  Write Index      │                   │
                              │  (Mutex-guarded)  │                   │
                              └────────┬─────────┘                    │
                                       │ swap (every 10s)             │
                              ┌────────▼─────────┐  ┌──────────────┐  │
                              │  Read Snapshot    │──│  Ring Buffer  │ │
                              │  (ArcSwap, lock-  │  │  (30 windows)│ │
                              │   free reads)     │  └──────────────┘ │
                              └────────┬─────────┘                    │
                                       │                              │
                              ┌────────▼─────────┐                    │
                              │ Anomaly Detector  │                   │
                              │ Tier 1: Centroid  │                   │
                              │ Tier 2: ANN Search│                   │
                              └──────────────────┘                    │
                        └─────────────────────────────────────────────┘
```

## 📊 Data Flow

```
Ingest → Parse(Drain) → Embed(LRU cache / CPU ONNX) → Tier 1/2 Detection → Ping-Pong Indexing → Flush
```

- **Cache Hit**: Known template → vector returned from memory instantly (zero compute cost)
- **Cache Miss**: Novel template → vectorized by all-MiniLM-L6-v2 (ONNX, 384-dim)

## 🚀 Quick Start

### Prerequisites

- Rust 1.70+ (edition 2021)
- ~90 MB disk space for the ONNX model

### Build & Run

```bash
# Clone the repository
git clone https://github.com/shguddn8591/turbolog.git
cd turbolog

# Download the ONNX model + tokenizer (~86 MB)
./scripts/download_model.sh

# Build and run
cargo build --release
./target/release/turbolog
# => TurboLog listening on http://0.0.0.0:8087
```

### Configuration

| Environment Variable | Default | Description |
|---|---|---|
| `TURBOLOG_PORT` | `8087` | HTTP server port |
| `TURBOLOG_DATA_DIR` | `./data` | Directory for WAL and chunk segments |
| `TURBOLOG_MODEL_DIR` | `./models` | Directory containing ONNX model and tokenizer |

## 📡 API Reference

### Ingest Logs — `POST /logs`

```bash
curl -X POST http://localhost:8087/logs \
  -H "Content-Type: application/json" \
  -d '{"logs": [
    "Node 42 is online",
    "connection accepted from 10.0.0.5 port 5432",
    "disk usage at 95 percent on /var"
  ]}'
```

**Response:**
```json
{
  "results": [
    { "id": 123456, "template_id": 8817264, "timestamp": 1718000000, "anomaly": null },
    { "id": 123457, "template_id": 5523891, "timestamp": 1718000000, "anomaly": null },
    { "id": 123458, "template_id": 7701234, "timestamp": 1718000000,
      "anomaly": { "score": 0.73, "nearest_incidents": [123401, 123389] } }
  ]
}
```

### Semantic Search — `POST /search`

```bash
curl -X POST http://localhost:8087/search \
  -H "Content-Type: application/json" \
  -d '{"query": "disk full error", "k": 5}'
```

### Engine Stats — `GET /stats`

```bash
curl http://localhost:8087/stats
```

**Response:**
```json
{
  "cache_hits": 9523,
  "cache_misses": 477,
  "cache_hit_rate": 0.952,
  "pending_window_len": 84,
  "ring_windows": 12,
  "ring_vectors": 1024,
  "detector_calibrated": true,
  "ingested_total": 10000
}
```

## 🧪 Testing

```bash
# Download ONNX model (required for integration tests)
./scripts/download_model.sh

# Run all tests
cargo test

# Unit tests only (no model required)
cargo test --lib

# With debug logging
RUST_LOG=turbolog=debug cargo test --tests
```

## 🔬 Design Constraints

These invariants are enforced across the codebase:

| Constraint | Rationale |
|---|---|
| **No Dynamic Re-training** | TQ+ calibration/rotation matrices are frozen after initial K-means. No runtime re-learning. |
| **Hard Physical Deletion** | On retention expiry, hourly chunk directories are `unlink`ed at the OS level — no per-vector `remove()` fragmentation. |
| **Stateless Embedder** | The embedding worker holds no request-to-request state, enabling horizontal scaling on a separate thread pool. |

## 📋 Roadmap

- [x] **Phase 1 — Core Bindings & Cache**: Drain parser (`drain-rs`), LRU cache, CPU (ONNX) embedding pipeline — integration tests: 100 synthetic logs, 95% cache hit rate, 384-dim L2 normalization verified
- [x] **Phase 2 — Ping-Pong & Centroid**: `arc-swap` read/write index isolation, `.tvim` chunk backup, K-Centroid Tier 1 + turbovec `IdMapIndex` allowlist Tier 2 — concurrency (ingest/search/swap 3-thread) and E2E anomaly detection tests passed
- [ ] **Phase 3 — Persistence & API** *(in progress)*: WAL crash recovery, HTTP/gRPC interface, historical chunk search

## 🛠️ Implementation Notes

- `TurboQuantIndex` → **`IdMapIndex`**: External u64 ID semantics (`ingest(id, …)`, allowlist search) are provided by `IdMapIndex`. `TurboQuantIndex` is position-based and slots shift on `swap_remove`.
- `AtomicPtr<*mut>` → **`arc-swap`**: Eliminates use-after-free during swaps where readers hold stale references.
- `template_id` = **FNV-1a 64-bit hash** of the template string (drain-rs lacks stable cluster IDs).

## 🤝 Contributing

Contributions are welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## 📄 License

[MIT](LICENSE) © 2025

---

<p align="center">
  <sub>If TurboLog saved you from deploying yet another Elasticsearch cluster, consider giving it a ⭐</sub>
</p>
