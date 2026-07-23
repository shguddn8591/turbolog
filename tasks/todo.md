# TurboLog — Roadmap (CLI-first)

> **Product north star:** a local terminal log triage tool.
> Pipe logs in → highlight novel / anomalous lines → optionally explain with a local LLM.
>
> **Not the north star:** a horizontally scaled observability platform, Datadog substitute,
> or “1M concurrent connections” service. Those paths fight the architecture (frozen
> calibration, stateful in-memory index) and the maintainer budget.

## Positioning rules (do not violate without an explicit decision)

1. **Primary surface** = `watch` / `scan` / `history` (+ optional `--explain`).
2. **`serve` / `ui` / k8s** = experimental power-user features, behind Cargo features / docs
   labeled *experimental*. Never marketed as production-ready until probes, backpressure,
   and graceful shutdown actually exist in code.
3. Prefer vertical niches that fit the pipe UX over generic “AI log platform” claims:
   - **Now:** live `tail -f` / `docker logs` triage while developing a service.
   - **Next:** CI/CD failure-log scan + summary.
   - **Later (only if CLI is used):** single-host sidecar — not a cluster control plane.

---

## Now — Ship a trustworthy CLI (P0)

### First-run & install UX
- [ ] Document that default `cargo install turbolog` is CLI-only (`embedded-model`);
      `server` and `tui` need `--features`
- [ ] Improve first-run model story (binary size / download progress / offline flag) so
      “just pipe” is actually true on a clean machine
- [ ] Keep release binaries in GitHub Releases as the no-Rust path; verify README links

### Detection quality for triage (not “AI ops”)
- [ ] Surface calibration state clearly (`watch`: calibrating → ready; `scan`: small-input rules)
- [ ] Make `--threshold` and auto threshold understandable in help text / docs
      (score = novelty distance, not probability)
- [ ] Reduce false novelty on known-benign bursts where cheap heuristics help
      (e.g. repeated identical templates after calibration)
- [ ] Persist / show recurring patterns via `history` so repeat anomalies are actionable

### Explain as optional garnish
- [ ] Keep `--explain` a no-op when no LLM is found (already true) — never imply it is required
- [ ] Improve history context line for recurring templates (roadmap item below)
- [ ] Do not block the pipe path on slow LLM calls without a clear timeout story
      (document 30s timeout; consider async/non-blocking explain later)

### Docs hygiene (this pass)
- [x] Rewrite `tasks/todo.md` around CLI-first direction
- [x] Align README, CONTRIBUTING, OPERATIONS, SLO with the same north star
- [x] Mark `deploy/` as experimental; stop claiming 1M / production k8s readiness
- [x] Add `deploy/README.md`; disable k8s/compose probes that target missing routes

---

## Next — Vertical wedge (P1)

### Developer live triage
- [ ] Short “recipes” in README: docker logs, journalctl, kubectl logs (single pod),
      app `tee` pipelines
- [ ] Exit codes + `--only-anomalies` / `--quiet` remain script-friendly (CI hooks)

### CI failure-log assistant
- [ ] `turbolog scan --format json` examples for GitHub Actions / other CI
- [ ] Optional `turbolog diagnose` (time-window / top anomalies / recurring badge) —
      land on `main` only when the feature actually merges and stays
- [ ] History-aware explanation context (recurring pattern detection)

### Shell & editor adjacency (only after the above sticks)
- [ ] Completions already shipped — keep them green in CI
- [ ] VS Code / Neovim extension — **deferred** until CLI has real external users

---

## Later / experimental — Server path (P2, explicit demotion)

> Build only when CLI usage creates a concrete need to daemonize *one host or one stream*.
> Do **not** resume “1M concurrent” planning as a goal.

### Honest status of current server stack
- [x] Engine sharding, WAL, embedder pool, metrics *module* exist in-tree
- [ ] `/health`, `/ready`, `/metrics` HTTP routes — **not implemented** (k8s probes will fail)
- [ ] Inflight backpressure / `TURBOLOG_MAX_INFLIGHT` — **documented but not wired in `http.rs`**
- [ ] Graceful SIGTERM shutdown — **not implemented**
- [ ] `deploy/k8s` — placeholders (example.com images, stub model-init)

### If/when server is revived (minimum bar before removing *experimental*)
- [ ] Implement `/health`, `/ready`, `/metrics` and wire `metrics.rs`
- [ ] Backpressure + body read timeout + `ServerConfig`
- [ ] Graceful shutdown + swap_tick flush
- [ ] Single-node docker-compose path that actually healthchecks
- [ ] Docs that say “single-node / trusted network”, never “1M connections”

### Explicitly out of scope (until revisited)
- [ ] gRPC interface
- [ ] Stateless embedder worker fleet
- [ ] Multi-replica consistent-hash topology as a supported product
- [ ] Competing with hosted observability platforms

---

## Done — Historical phases (reference only)

Kept for archaeology. Do not treat unchecked Phase-4 “1M” items as active work.

### Scaffold / Phase 1–3 (complete)
- [x] Core Drain + ONNX MiniLM + LRU cache
- [x] K-means Tier-1 detector + Tier-2 turbovec path (server)
- [x] WAL / chunks / engine / HTTP skeleton
- [x] Pipe CLI: `watch`, `scan`, `--explain`, `history`
- [x] Embedded model + crates.io publish path
- [x] TUI (`--features tui`), server feature gate (`--features server`)
- [x] CI: fmt, clippy, cargo-deny, nextest, coverage, release workflow

### Former “Phase 4: 1M Concurrent Connections” — superseded
Sharding, metrics registry, docker/k8s *skeletons*, benches, and SLO *numbers* landed in
various forms, but the **product goal is retired**. Remaining HTTP/ops gaps belong under
“Later / experimental” above, not under a 1M scale narrative.
