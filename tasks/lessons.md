# Lessons

## 2026-07-23 Product focus (CLI-first)

- **Two products in one repo was the real bug:** pipe CLI for developers vs “1M concurrent”
  observability engine. Docs (SLO, OPERATIONS, k8s) sold the latter while code and adoption
  only supported the former. **Narrow the product; demote the rest to experimental.**
- **Lab logs/s ≠ product.** Sharding and µs hit-path benches are interesting engineering; they
  do not justify multi-replica marketing when `/health` is missing and calibration is frozen.
- **Novelty ≠ incident.** Keep README/ops language from promising root-cause or calibrated
  outage probability. `--explain` is optional narration.
- **Merged stacked PRs that never land on `main` destroy trust.** Track roadmap items as done
  only when they exist on the default branch.
- **When rewriting docs, update every surface** (README, todo, CONTRIBUTING, OPERATIONS, SLO,
  deploy comments, issue templates) or the old 1M narrative will keep leaking.

## 2026-06-10 Phase 2
- **Ping-pong window semantics finalized**: The specification does not define the handling of existing data in the write index after a swap.
  Adopted a design of replacing with an empty index after sealing — search snapshot = previously sealed window (10 seconds).
  Historical search is handled by Phase 3 time chunk (.tvim) files. (Alternatives like clone/double-write/rebuild are impossible due to cost or
  `turbovec` Clone not being implemented)
- **`turbovec` `search_with_allowlist` panics on empty list/non-existent IDs** —
  Pre-filtering with `contains()` in Tier 2 is mandatory (detect.rs `tier2_context`).
- **Call `prepare()` before publishing**: Made the swap thread pay the initialization cost of rotation matrix/SIMD layout
  to prevent latency spikes for the first searcher.
- The specification's `AtomicPtr<*mut>` ping-pong causes use-after-free on read references during swap → Corrected to `ArcSwap` snapshot +
  write-only Mutex (untouched by the search path). Approved during the planning phase.

## 2026-06-10 Phase 3
- **Write path serialization point = WAL Mutex**: WAL append + indexer ingest, and swap (seal+rotate)
  must be grouped under the same lock, otherwise a loss race occurs where "rotate immediately after seal deletes WAL records of the new window".
  Documented as an invariant in engine.rs module doc.
- **turbovec score = dot product similarity (larger is closer)**: Not specified in documentation, confirmed empirically
  (identical≈1.0, orthogonal≈0, opposite≈-1.0). Ring merge search is sorted in descending order.
- **Skip swapping empty windows**: Swapping when idle replaces the search snapshot with an empty index and spams segment
  files — no-op if pending==0.
- **Search depth = merging ring (recent N sealed windows)**: `turbovec` lacks merge, so search per window then
  merge scores. Historical data beyond the ring range requires loading disk segments (future work).

## 2026-06-10 Performance Fix ([Mid 1]·[Mid 2])
- **Reason for switching WAL truncate(rotate) → rename(detach_sealed)**: To write segments outside the lock,
  we must avoid the loss race of "new appends coming in after seal but before rotate". rename is a metadata operation so
  it is safe inside the lock, and the sealed file is deleted only after the segment is settled → maintains crash durability.
  Restart recovery merges the remaining sealed + active WAL into a single WAL after id deduplication (atomic rename).
- **The key to embedder pool separation is non-blocking, not parallelization**: Even with 1 pool, the essence is that the hit path
  is not blocked by inference (hit path 136 → 59k logs/s during a miss storm).
  Simultaneous misses on the same template may cause duplicate embeddings — results are identical, so intentionally allowed.
- The max ~86ms spike in load test [2] is unrelated to swapping (no swap in that interval) —
  Presumed to be an OS write stall during WAL file growth, 1 in 50k · p99 26µs so acceptable.

## 2026-06-15 Phase 4 (historical) — production hardening / 1M narrative

> **Status:** Engineering lessons below remain valid for the experimental engine.
> The **product goal of “1M concurrent connections” is retired** (2026-07-23). Do not cite
> this section as current roadmap.

- **A single global write lock is the real throughput bottleneck**: Quantitatively proven with loadtest [7] that multi-thread ingestion
  *anti-scales* (lock contention) from 10 threads to 0.86x, justifying sharding. Do not assume it is "fast"
  without measurement. → Mitigated by independent WAL+index+ring per shard, routing with `id % N`.
- **Setting shard boundaries**: Only WAL/index/ring are sharded; template cache, embedder pool, detector,
  and calibration are kept global. Sharding state-sharing parts (cache hit rate, frozen centroid)
  only increases consistency and memory costs without benefits.
- **Freeze shared contracts first → parallel implementation**: Fix the metrics module signature first, then
  separate workstreams into non-overlapping file ownerships.
- **Stacked PRs**: Useful for parallel agents — but verify the final merge actually sits on `main`.
- **1M math was always a stretch**: stateful index + frozen calibration + incomplete HTTP ops ≠ platform.
  Prefer single-node experimental daemonization only if CLI demand appears.
- **TLS at ingress** remains fine advice *if* `serve` is ever exposed beyond localhost; keep the app plain HTTP on a trusted network.
