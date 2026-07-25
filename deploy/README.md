# Deploy scaffolding (experimental)

TurboLog’s **supported product** is the local CLI (`watch` / `scan` / `history`).
See the root [README](../README.md) and [`tasks/todo.md`](../tasks/todo.md).

This directory holds **optional** manifests for people experimenting with
`cargo build --features server`. It is **not** a supported production topology.

| Path | Status |
|------|--------|
| `docker-compose.yml` | Profile `experimental-server`; healthcheck commented out (`/health` missing) |
| `k8s/*` | Placeholders (example.com images); probes commented out; HPA/PDB stubs only |

Before trying any of this:

1. Read [docs/OPERATIONS.md](../OPERATIONS.md) §2 and §5.
2. Build an image that includes `--features server` (see root `Dockerfile` banner).
3. Supply real `model.onnx` / `tokenizer.json`.
4. Do **not** enable k8s/compose health probes until `GET /health` exists in `http.rs`.

Former “1M concurrent connections” / multi-replica capacity guidance has been removed on purpose.
