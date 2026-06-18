# Lessons

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

## 2026-06-15 Phase 4 Production Hardening (1 Million Concurrent Connections)
- **A single global write lock is the real throughput bottleneck**: Quantitatively proven with loadtest [7] that multi-thread ingestion
  *anti-scales* (lock contention) from 10 threads to 0.86x, justifying sharding. Do not assume it is "fast"
  without measurement. → Eliminated by independent WAL+index+ring per shard, routing with `id % N`.
- **Setting shard boundaries**: Only WAL/index/ring are sharded; template cache, embedder pool, detector,
  and calibration are kept global. Sharding state-sharing parts (cache hit rate, frozen centroid)
  only increases consistency and memory costs without benefits.
- **Freeze shared contracts first → parallel implementation**: Fix the metrics module signature first (by myself), then
  separate the 4 workstreams into non-overlapping file ownerships for parallel execution. If cross-files (detect.rs clippy
  fixes) are identical changes, 3-way merge resolves automatically, making duplicate allowance simpler.
- **Stacked PRs**: Sharding and HTTP are stacked on top of the metrics contract (foundation), while deployment and benchmarks are independent from main.
  When foundation merges, GitHub automatically retargets the stack PR base.
- **1 Million = single-node optimization × horizontal scaling**: Nodes are stateful due to in-memory index → node pinning with
  tenant key consistent hash LB + intra-node core sharding. Based on a conservative 30k logs/s/node, 1M → ~44 replicas.
- **TLS is terminated at ingress**: The app remains plaintext (inside the trusted network). In-process TLS only increases
  operational complexity. Observability probes (/health, /ready, /metrics) are exempt from authentication.
