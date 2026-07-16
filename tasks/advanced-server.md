# TurboLog — Advanced Server Roadmap

> For self-hosters and centralized log ingestion. **Not** the default terminal-first path.

Moved from the former Phase 4 (1M concurrent connections) checklist.

## Observability (HTTP)

- [x] `src/metrics.rs` — Prometheus text exposition (dependency-free)
- [ ] Wire `/health`, `/ready`, `/metrics` in `http.rs`
- [ ] Backpressure (503 when inflight exceeded)
- [ ] Request body read timeout
- [ ] `ServerConfig` (addr, workers, auth, max_inflight, shutdown)
- [ ] SIGTERM graceful shutdown

## Deployment

- [x] Multi-stage `Dockerfile` + `.dockerignore`
- [x] `deploy/k8s/` manifests (Deployment, Service, HPA, PDB, ConfigMap)
- [x] `deploy/docker-compose.yml`
- [x] `docs/OPERATIONS.md`
- [ ] Replace K8s placeholder image URLs with real registry paths
- [ ] Init container model download URL

## Performance

- [x] Sharded ingestion engine (`engine.rs`)
- [x] `benches/throughput.rs` criterion benchmarks
- [x] `examples/loadtest.rs`
- [x] `docs/SLO.md`

## Future

- [ ] gRPC interface
- [ ] History search across disk segments (beyond ring window)
- [ ] Stateless embedder horizontal scaling
