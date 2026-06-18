# TurboLog Operations Guide

> Version: 0.2.0 | Last Modified: 2026-06-18

---

## Table of Contents

1. [1 Million Concurrent Connections Topology](#1-1-million-concurrent-connections-topology)
2. [Environment Variables Table](#2-environment-variables-table)
3. [Endpoints Table](#3-endpoints-table)
4. [TLS and Network Security](#4-tls-and-network-security)
5. [Scaling and Rollout](#5-scaling-and-rollout)
6. [Observability](#6-observability)
7. [Graceful Shutdown](#7-graceful-shutdown)
8. [Backup and Recovery](#8-backup-and-recovery)

---

## 1. 1 Million Concurrent Connections Topology

### Core Architecture Principles

TurboLog node is a **stateful-lite** service that holds an **in-memory index (arc-swap)**.
Random LB (Round Robin) breaks index consistency as the same log stream is distributed across multiple nodes.
Therefore, **collection agents/clients are pinned to a specific replica with a consistent hash based on a key (tenant ID or stream ID)**.

### Horizontal Scaling Calculation

```text
1,000,000 concurrent requests/s
  ÷ Throughput per replica (e.g., 20,000 req/s, based on TURBOLOG_EMBEDDERS=4)
  = 50 replicas (HPA max)
```

Each replica internally operates `TURBOLOG_EMBEDDERS` number of embedding workers in parallel.
That is, **Total Throughput = N replicas × Embedding workers per replica × Embedder processing speed**.

### ASCII Architecture Diagram

```text
Client/Collection Agent (Log Source)
        │
        ▼
┌─────────────────────────────────────────────┐
│         External Load Balancer / Ingress    │
│   Consistent hash routing (hash_key = tenant_id) │
│   TLS Termination ← Handled here only, plain text inside app │
└────────────┬─────────────┬──────────────────┘
             │             │          ...
             ▼             ▼
     ┌──────────┐   ┌──────────┐   ┌──────────┐
     │ TurboLog │   │ TurboLog │   │ TurboLog │   (6~50 replicas)
     │ Pod #0   │   │ Pod #1   │   │ Pod #N   │
     │          │   │          │   │          │
     │ shard A  │   │ shard B  │   │ shard C  │   ← Tenant/Stream Shard
     │          │   │          │   │          │
     │ [embed0] │   │ [embed0] │   │ [embed0] │
     │ [embed1] │   │ [embed1] │   │ [embed1] │   ← In-node embedder parallelization
     │ [embed2] │   │ [embed2] │   │ [embed2] │
     │          │   │          │   │          │
     │ /data    │   │ /data    │   │ /data    │   ← WAL + .tvim (Node local)
     └──────────┘   └──────────┘   └──────────┘
             │             │          ...
             ▼             ▼
     ┌──────────────────────────────────┐
     │       Prometheus / Grafana       │
     │  (Scrape: /metrics each pod)     │
     └──────────────────────────────────┘
```

### Consistent Hash LB Setup Example (Nginx Ingress)

```nginx
# nginx.conf or Ingress annotation
upstream turbolog {
    hash $http_x_tenant_id consistent;
    server turbolog-0.turbolog:8087;
    server turbolog-1.turbolog:8087;
    # ...
}
```

When using Envoy: `hash_policy` → `header: x-tenant-id`.

---

## 2. Environment Variables Table

| Variable Name | Default Value | Required | Description |
|--------|--------|------|------|
| `TURBOLOG_PORT` | `8087` | No | HTTP listen port |
| `TURBOLOG_DATA_DIR` | `./data` | No | WAL/Index snapshot path. PVC mount recommended |
| `TURBOLOG_MODEL_DIR` | `./models` | Yes* | `model.onnx`, `tokenizer.json` location. Injected via initContainer |
| `TURBOLOG_EMBEDDERS` | `2` | No | ONNX embedding worker count. Increasing it uses more Memory/CPU (~90 MB per session) |
| `TURBOLOG_AUTH_TOKEN` | _(None)_ | No | Bearer token. If set, all requests require `Authorization: Bearer <token>` |
| `TURBOLOG_MAX_INFLIGHT` | _(Unlimited)_ | No | Max concurrent processing requests. Returns 503 if exceeded (Backpressure) |

> *If files are missing in the `TURBOLOG_MODEL_DIR` path, embedding will fail. In Kubernetes, the initContainer fills the files before the main container starts.

---

## 3. Endpoints Table

| Path | Method | Auth | Description | Response Example |
|------|--------|------|------|-----------|
| `/logs` | POST | Required* | Ingests a batch of log lines. Body: `{"logs": ["line1", ...]}`. Max 1 MiB | `{"results": [...]}` |
| `/search` | POST | Required* | Vector similarity search. Body: `{"query": "...", "k": 5}` | `{"results": [...]}` |
| `/stats` | GET | Required* | Engine stats (Ingest count, index size, etc.) | `{...stats...}` |
| `/health` | GET | No | Liveness probe. 200 if app process is normal | `{"status":"ok"}` |
| `/ready` | GET | No | Readiness probe. 200 if model load is complete, 503 if preparing | `{"status":"ready"}` |
| `/metrics` | GET | No | Prometheus text format metrics | `# HELP ...` |

> *Auth is required if `TURBOLOG_AUTH_TOKEN` is set. If not set, all paths can be accessed without auth.
>
> **Caution**: `/health`, `/ready`, `/metrics` need to be added in the WS3 (http.rs hardening) task.
> Currently, `http.rs` only has `/logs`, `/search`, and `/stats` implemented.

---

## 4. TLS and Network Security

**TLS termination is handled at the Ingress or external Load Balancer.**
The TurboLog app communicates only in plain text HTTP (port 8087) and assumes a trusted network inside the cluster.

- **Ingress → Pod** segment: Cluster internal plain text (Trusted network)
- **External → Ingress** segment: TLS 1.2+ (Ingress controller manages certificates)
- If mTLS is required, configure it with the Istio/Linkerd sidecar pattern.

```text
Client ─── TLS ─── [Ingress / ELB] ─── HTTP ─── TurboLog Pod
              (443)                         (8087)
```

---

## 5. Scaling and Rollout

### Rolling Update

```bash
# Update image
kubectl -n turbolog set image deployment/turbolog turbolog=registry.example.com/turbolog:0.2.0

# Check rollout status
kubectl -n turbolog rollout status deployment/turbolog

# Rollback on issue
kubectl -n turbolog rollout undo deployment/turbolog
```

During a rolling update, PDB (`minAvailable: 4`) guarantees at least 4 replicas, so
with 6 replicas, a maximum of 2 are sequentially replaced at a time.

### Manual Scale

```bash
# Temporary scale out (Preparing for traffic surge)
kubectl -n turbolog scale deployment/turbolog --replicas=20

# Auto-return after releasing HPA override
kubectl -n turbolog patch hpa turbolog -p '{"spec":{"minReplicas":6}}'
```

### Check HPA Status

```bash
kubectl -n turbolog get hpa turbolog -w
```

---

## 6. Observability

### Prometheus Scrape Configuration

Each pod has the `prometheus.io/scrape: "true"` annotation.
It exposes metrics in Prometheus text format at the `/metrics` endpoint.

```yaml
# prometheus.yml (scrape_config example, based on annotation)
scrape_configs:
  - job_name: turbolog
    kubernetes_sd_configs:
      - role: pod
    relabel_configs:
      - source_labels: [__meta_kubernetes_pod_annotation_prometheus_io_scrape]
        action: keep
        regex: "true"
      - source_labels: [__meta_kubernetes_pod_annotation_prometheus_io_path]
        target_label: __metrics_path__
      - source_labels: [__meta_kubernetes_pod_annotation_prometheus_io_port]
        target_label: __address__
        regex: (.+)
        replacement: ${1}:8087
```

### Core Metrics

| Metric Name | Type | Description | Alert Recommended Threshold |
|--------|------|------|-----------------|
| `turbolog_ingested_total` | Counter | Total ingested logs | — |
| `turbolog_inflight_requests` | Gauge | Currently processing requests | > `TURBOLOG_MAX_INFLIGHT × 0.8` |
| `turbolog_ingest_latency_seconds` | Histogram | Ingest processing latency. p99 based | p99 > 200ms |
| `turbolog_http_5xx_total` | Counter | HTTP 5xx response count | > 10 per min |
| `turbolog_cache_hit_rate` | Gauge | Embedding cache hit rate (0~1) | < 0.5 (Cache efficiency drop) |
| `turbolog_anomaly_detections_total` | Counter | Anomaly detection occurrences | — |

### Recommended Grafana Dashboard Panels

1. **Ingest Throughput** (ingested_total rate 1m)
2. **In-Flight Requests** (inflight gauge)
3. **Latency Distribution** (ingest_latency p50/p95/p99)
4. **Error Rate** (5xx rate / total request rate)
5. **Cache Hit Rate**
6. **Replica Count** (kube_deployment_status_replicas)

---

## 7. Graceful Shutdown

TurboLog gracefully shuts down in the following order upon receiving a SIGTERM:

```text
SIGTERM Received
    │
    ├─ 1. Stop accepting new requests (Close tiny_http server)
    │
    ├─ 2. Wait for in-progress requests to complete (Max terminationGracePeriodSeconds=30s)
    │
    ├─ 3. Execute last swap_tick (Index snapshot + WAL flush)
    │
    └─ 4. Process termination
```

Kubernetes removes the pod from the Service endpoints before sending SIGTERM, so
wait for the endpoint propagation to complete with `preStop: sleep 5`.

**Force Kill Prevention**: If shutdown is not completed within `terminationGracePeriodSeconds: 30`,
SIGKILL is sent. In high throughput environments, consider increasing this value to 60 seconds.

---

## 8. Backup and Recovery

### Storage File Structure

```text
/data/
  ├── wal-<shard_id>.wal      # Write-Ahead Log (Binary)
  └── index-<shard_id>.tvim   # Index snapshot (Serialized vector index)
```

### Backup Procedure

```bash
# 1. Select target pod
POD=$(kubectl -n turbolog get pods -l app.kubernetes.io/name=turbolog -o name | head -1)

# 2. Compress and copy WAL + Index snapshot
kubectl -n turbolog exec "$POD" -- tar czf - /data | \
  gzip > "turbolog-backup-$(date +%Y%m%d-%H%M%S).tar.gz"

# 3. Upload to remote storage (e.g., S3)
aws s3 cp turbolog-backup-*.tar.gz s3://my-bucket/turbolog-backups/
```

When using PVC, Snapshot (VolumeSnapshot) API or Cloud Storage snapshot is recommended.

### Recovery Procedure

```bash
# 1. Stop pods (replicas=0)
kubectl -n turbolog scale deployment/turbolog --replicas=0

# 2. Extract backup and copy to PVC or emptyDir
# (Use separate recovery pod or kubectl cp)

# 3. Restart pods
kubectl -n turbolog scale deployment/turbolog --replicas=6

# 4. Check normal startup
kubectl -n turbolog rollout status deployment/turbolog
kubectl -n turbolog logs -l app.kubernetes.io/name=turbolog --tail=50
```

### Recovery Time Objective (RTO)

| Scenario | Estimated RTO |
|----------|----------|
| Pod restart (emptyDir, index rebuild) | Proportional to WAL size, typically 30~120s |
| PVC snapshot recovery (Same AZ) | 5~15 mins |
| Full cluster rebuild | 30 mins~1 hour (Includes model download) |

> **Note**: TurboLog's in-memory index can be rebuilt by replaying the WAL.
> Adjust the WAL retention period according to disk capacity and RTO goals.
