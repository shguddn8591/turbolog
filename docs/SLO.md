# TurboLog SLO (Service Level Objectives)

> Performance/availability targets and measured basis for operating a service with 1M concurrent connections.
> Measurement environment: **Apple M5 (10-core) / 32GB RAM / rustc 1.95.0 / release(LTO)**, single node.

## 1. Objectives (SLO)

| Item | Target | Measured (Single Node) | Remarks |
|---|---|---|---|
| Ingestion throughput (Cache hit) | ≥ 30,000 logs/s/node | **52,800 logs/s** | Template cache hit path |
| Ingestion latency p50 | ≤ 100µs | **15.5µs** | |
| Ingestion latency p99 | ≤ 1ms | **29.6µs** | |
| Ingestion latency max | ≤ 100ms | 87.8ms | OS write stall during WAL growth (1 in 50k) |
| Search latency p50 (including embedding) | ≤ 15ms | **8.9ms** | Embedding (ONNX) is dominant |
| Search latency p99 (including embedding) | ≤ 20ms | **11.0ms** | |
| Embedding throughput (Cache miss cap) | ≥ 100 embeds/s/embedder | **136 embeds/s** | ONNX MiniLM-L6-v2 CPU |
| HTTP /logs throughput (Batch 10) | ≥ 30,000 logs/s/node | **42,200 logs/s** | 4 clients |
| Miss storm resilience (Hit path) | ≥ 30,000 logs/s | **57,900 logs/s** | Hit path is non-blocking even during new template storms |
| Cache hit rate (Normal traffic) | ≥ 0.99 | **0.9994** | |
| Availability | ≥ 99.9% | — | Achieved via multi-replica + PDB (production) |

## 2. Multi-thread Scalability (Core)

Scalability when N threads call `ingest_log` simultaneously on the same node:

| threads | logs/s | scale vs 1T |
|---|---|---|
| 1 | 64,550 | 1.00x |
| 2 | 57,697 | 0.89x |
| 4 | 56,465 | 0.87x |
| 10 | 55,549 | 0.86x |

**Interpretation**: In the `main` baseline (unsharded) code, a single global write lock (`wal: Mutex<Wal>`) serializes all ingestions, so throughput does not increase with more threads (it slightly drops due to lock contention). This bottleneck is removed in the **WS1 Sharded Ingestion Engine** (`feat/sharded-engine`, independent WAL/index per shard, `id % N` routing). After sharding, proportional scaling to the number of cores is expected.

## 3. Measurement Method (Reproduction)

```bash
# Model preparation (First time only)
./scripts/download_model.sh

# Load test (Outputs items [1]~[7] from the table above)
cargo run --release --example loadtest

# Micro benchmark (Model-independent hot paths: parse/cache/detect/index)
cargo bench
```

`cargo bench` measures 4 groups in `benches/throughput.rs`:
- `template_parse` — Drain parsing throughput
- `cache_lookup` — Template cache hit lookup
- `anomaly_detect` — K-centroid Tier 1 distance/decision
- `index_ingest_search` — turbovec index ingest/search

## 4. 1M Concurrent Connections Capacity Estimation

If we **conservatively set the single node ingestion throughput to 30,000 logs/s/node**:

```
Required replicas = Target throughput / Throughput per node
```

| Target Ingestion Throughput | Required Replicas (30k/node) | Recommended (30% Margin) |
|---|---|---|
| 300,000 logs/s | 10 | 13 |
| 1,000,000 logs/s | 34 | 44 |
| 3,000,000 logs/s | 100 | 130 |

- **Horizontal scaling premise**: Clients/collection agents are pinned to a specific replica via consistent hashing LB based on tenant/stream keys (since the per-node in-memory index is stateful). The interior of the node is sharded again by the number of cores (WS1).
- HPA (`deploy/k8s/hpa.yaml`) automatically scales from min 6 / max 50 based on 70% CPU, and can be scaled with the `turbolog_inflight_requests` custom metric.
- Refer to `docs/OPERATIONS.md` for detailed topology and operational procedures.

## 5. Limitations and Assumptions

- Embeddings (cache misses) have an upper limit of ~136 embeds/s per node due to CPU ONNX inference. In normal operation, misses are rare with a cache hit rate ≥0.99, but during a storm of new templates, the miss path can become a bottleneck (hit path remains non-blocking). If more miss throughput is needed, increase `TURBOLOG_EMBEDDERS` (memory ~90MB/embedder) or horizontally scale the embedder as a separate worker.
- Search latency is dominated by embedding time. Further reduction is possible with query embedding caching/pre-calculation.
- Max ingestion latency spikes (~87ms, 1 in 50k) are assumed to be OS write stalls during WAL file growth and do not impact p99.
