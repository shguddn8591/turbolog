# TurboLog SLO / performance notes

> Last updated: 2026-07-23  
> **Scope:** what we actually care about for the **CLI-first** product, plus archived
> single-node engine microbench numbers for the experimental `serve` path.
>
> This document is **not** a promise of multi-replica “1M concurrent connections”
> capacity. That goal is retired from the product roadmap (`tasks/todo.md`).

---

## 1. Product SLOs (CLI — the bar that matters)

These are qualitative targets for the supported surface (`watch` / `scan` / `history`).
They are not yet continuously enforced in CI as numeric gates; treat them as engineering intent.

| Item | Target | Notes |
|------|--------|--------|
| Time-to-first-useful-output on a small `scan` | Seconds, not minutes | After model is on disk |
| Cold start with missing model | Clear progress / error; no silent hang | Download or point `TURBOLOG_MODEL_DIR` |
| `watch` calibration visibility | User can tell calibrating vs detecting | Status lines / completion signal |
| Cache-hit line handling | Feels instant on a laptop | Dominant path after templates repeat |
| Cache-miss embedding | Acceptable interactive latency on CPU | MiniLM ONNX; expect ms–tens of ms |
| `--explain` failure mode | Anomaly still printed; explain omitted | LLM never required for detection |
| Exit codes | Stable `0` / `1` / `2` contract | Safe for scripts and CI |
| History durability | Survives process exit on local disk | SQLite under XDG data dir |

**Non-goals for CLI SLOs:** fleet-wide availability %, multi-tenant p99 ingest, HPA behavior.

---

## 2. Algorithm expectations (honest limits)

| Behavior | Expectation |
|----------|-------------|
| What we flag | Novelty vs early calibrated templates |
| What we do **not** claim | Calibrated P(outage), root-cause certainty |
| Drift | Frozen centroids → long sessions with regime change may need restart / `--threshold` |
| `--explain` text | Best-effort local LLM prose; may be wrong |

If these limits are unacceptable for your use case, TurboLog is the wrong tool — use metrics/traces and a real log platform.

---

## 3. Archived: single-node engine microbenches (`--features server`)

Measured previously on a high-end Apple Silicon laptop with a release(LTO) build.
Useful as a **rough ceiling** for the experimental HTTP engine’s *cache-hit* path — **not**
a capacity plan for production clusters.

| Item | Historical measurement | Remarks |
|------|------------------------|---------|
| Ingestion throughput (cache hit) | ~50k+ logs/s/node | Template cache hit path |
| Ingestion latency p50 / p99 | tens of µs | Hit path |
| Embedding throughput (miss) | ~100+ embeds/s/embedder | CPU MiniLM; miss storms hurt |
| HTTP `/logs` batch throughput | tens of k logs/s | Lab clients; not an SLO commitment |

Reproduce locally (requires models + `server` feature):

```bash
./scripts/download_model.sh
cargo run --release --example loadtest --features server
cargo bench --features server
```

`cargo bench` groups in `benches/throughput.rs`: template parse, cache lookup,
anomaly distance, index ingest/search.

### Scalability footnote (why we do not sell “1M”)

Historical load tests showed a global WAL lock **anti-scaling** under many threads;
sharding mitigated that *inside one process*. That still does not make TurboLog a
horizontally scaled observability product: the index is stateful, calibration is frozen,
and HTTP ops endpoints required for k8s are incomplete. Do not convert lab logs/s into
replica math for marketing.

---

## 4. How we will evolve this doc

- Prefer tightening **§1 CLI SLOs** with real measurements from `watch`/`scan` on commodity laptops.
- Keep §3 as archaeology / experimental engine notes.
- Reject PRs that reintroduce “1M concurrent connections” targets without an explicit product decision to change `tasks/todo.md`.
