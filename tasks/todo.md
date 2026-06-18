# TurboLog — Phase 4: Production Hardening (1M Concurrent Connections)

> Goal: Production-level readiness for active use in a 1M concurrent connection service.
> Strategy: Freeze shared contracts (metrics API) → Parallelize 4 workstreams with non-overlapping file ownership (sonnet) → Integration and validation.

## Diagnostics (Bottleneck Priority)
- **P0 Throughput**: `wal: Mutex<Wal>` single global write lock serializes all ingestion → Sharding.
- **P0 Observability**: Missing Prometheus/health/ready → Cannot operate.
- **P1 Resilience**: Missing backpressure/timeout/graceful shutdown.
- **P1 Deployment**: Missing Docker/k8s/HPA (1M = N replicas horizontal scaling).
- **P2 Validation**: Missing criterion bench and SLO documentation.

## Contract Freeze (Prerequisite, I will do this)
- [ ] `src/metrics.rs` — Process-global Prometheus text exposition (0 dependencies). Called by all agents.
- [ ] `Cargo.toml` — signal-hook, criterion(dev)+`[[bench]]`, `[profile.release]` lto.
- [ ] `src/lib.rs` — `pub mod metrics;` declaration.

## WS1 — Sharded Ingestion Engine (engine.rs, index.rs, wal.rs)
- [ ] N Shards: Per-shard `Wal` + `PingPongIndexer` (Remove global lock)
- [ ] Per-shard swap_tick / Multi-shard crash recovery / Search N-shards x ring merge
- [ ] Public API (open/ingest_log/search_text/swap_tick/sweep_chunks/stats) remains unchanged
- [ ] Metrics instrumentation: ingest count/latency, anomaly
- [ ] Keep all existing 19 tests green

## WS3 — HTTP Edge Resilience (http.rs)
- [ ] `/health`(liveness), `/ready`(readiness), `/metrics`(Prometheus)
- [ ] Inflight backpressure (503 when exceeded) + Request body read timeout
- [ ] Introduce `ServerConfig` (addr/workers/auth/max_inflight/shutdown)
- [ ] Request metrics instrumentation (2xx/4xx/5xx/rejected/inflight)

## WS4 — Deployment & Operations (New files only)
- [x] Multi-stage `Dockerfile` (non-root, distroless/slim) + `.dockerignore` (Translated Korean comments to English)
- [ ] `deploy/k8s/`: Deployment/Service/HPA/PDB/ConfigMap (probe→/ready,/health)
- [ ] `deploy/docker-compose.yml` + `docs/OPERATIONS.md` (1M horizontal scaling topology, TLS@ingress)

## WS5 — Benchmark & SLO (Mainly new files)
- [ ] `benches/throughput.rs` criterion (Model-independent: parse/cache/detect/fnv)
- [ ] `examples/loadtest.rs` Add multi-thread contention ingestion measurement
- [ ] `docs/SLO.md`: Latency/throughput goals + Measurement results

## Integration & Validation (Depends on prerequisites, I will do this)
- [ ] Align run_server call sites (main.rs/loadtest) + SIGTERM graceful shutdown
- [ ] `cargo build/test/clippy` green + loadtest demonstration
- [ ] Update tasks/lessons.md

---

# TurboLog — Phase 1 Checklist

## Scaffold
- [x] Directory structure + git init
- [x] Cargo.toml dependencies (turbovec, drain-rs, lru, ort, tokenizers, arc-swap, anyhow)
- [x] .gitignore (/target, /models)
- [x] scripts/download_model.sh (all-MiniLM-L6-v2 ONNX + tokenizer.json)

## Phase 1: Core Bindings & Cache (src/ingest.rs)
- [x] ParsedLog struct
- [x] TemplateParser (drain-rs wrapper, template_id = FNV-1a template hash)
- [x] Embedder (ort Session + tokenizers, mean pooling + L2 norm)
- [x] VectorCache (LruCache<u64, Arc<[f32]>>, capacity 10,000, hit/miss counters)

## Skeleton (No implementation, types only)
- [x] detect.rs — DetectionResult, AnomalyDetector (Calibrated with IdMapIndex)
- [x] index.rs — PingPongIndexer (Calibrated with arc-swap)
- [x] lib.rs module connections

## Validation
- [x] Unit tests: Template ID stability, Cache hit/miss — 3 passed
- [x] Integration tests: 100 synthetic logs → 384-dimensional L2≈1.0, hit rate 95.0% (hits=95, misses=5)
- [x] cargo build no warnings + cargo test all passed (5/5)
- [x] README.md + initial commit
- [x] Establish GitHub Actions advanced CI pipeline (Lint, Matrix OS tests, Security Audit, Code Coverage)

## Phase 2: Ping-Pong & Centroid
- [x] Implement PingPongIndexer (Write Mutex + ArcSwap snapshot read, swap_and_flush)
- [x] Backup sealed window .tvim chunks (flush_path) + load round-trip validation
- [x] AnomalyDetector Tier 1 (Fixed centroid Euclidean distance, fit = frozen after 1 K-means)
- [x] Tier 2 IdMapIndex deep search + allowlist filter (includes panic guard)
- [x] Concurrency testing (ingest/search/swap 3-threads) + E2E log anomaly detection test
- [x] 12/12 tests passed

## Phase 3: Persistence & API
- [x] WAL disaster recovery (wal.rs — append/rotate/replay, ignore incomplete tails, crash recovery test)
- [x] Time chunk management (chunks.rs — hour-N directory, OS unlink sweep upon expiration)
- [x] Engine assembly (engine.rs — WAL→Indexing serialization, ring merge search, freeze after auto-calibration)
- [x] HTTP API (http.rs — POST /logs, POST /search, GET /stats, tiny_http worker pool)
- [x] Server daemon (main.rs — 10-second swap tick + 1-hour sweep, env config)
- [x] 19/19 tests passed + release binary smoke run (swap daemon, search, chunks, WAL rotate demonstration)

## Future (Improvement candidates outside of spec)
- [ ] gRPC interface (Spec parallel item — added on top of the same engine if needed)
- [ ] History search targeting disk segments (Timeframes exceeding ring scope)
- [x] Separate embedder pool (§4.3 Phase 1 — In-process pool, TURBOLOG_EMBEDDERS)
- [ ] Stateless Embedder horizontal scaling (§4.3 Final Form — Worker process separate deployment)
