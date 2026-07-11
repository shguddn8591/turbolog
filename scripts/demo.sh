#!/usr/bin/env bash
# TurboLog demo — generates a realistic log stream and pipes it through TurboLog.
#
#   ./scripts/demo.sh            # live streaming highlight (default, best for asciinema)
#   ./scripts/demo.sh explain    # live streaming + local LLM explanations (needs Ollama/LM Studio)
#
# TurboLog learns normal traffic first, then flags outliers — so this demo streams
# a realistic baseline (suppressed by --only-anomalies) before the anomalies arrive.
#
# Works whether TurboLog is installed (in PATH) or built locally in this repo.
set -euo pipefail

# ── Locate the turbolog binary ───────────────────────────────────────────────
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
if command -v turbolog >/dev/null 2>&1; then
    BIN="turbolog"
elif [ -x "$ROOT/target/release/turbolog" ]; then
    BIN="$ROOT/target/release/turbolog"
elif [ -x "$ROOT/target/debug/turbolog" ]; then
    BIN="$ROOT/target/debug/turbolog"
else
    echo "turbolog not found. Install it (cargo install turbolog) or build it (cargo build)." >&2
    exit 1
fi

# Prefer repo-local models when running from a source checkout.
if [ -f "$ROOT/models/model.onnx" ] && [ -z "${TURBOLOG_MODEL_DIR:-}" ]; then
    export TURBOLOG_MODEL_DIR="$ROOT/models"
fi

# ── Synthetic log generator ──────────────────────────────────────────────────
# Emits a stream of normal service logs with realistic template diversity — the
# baseline the detector calibrates on. Real applications emit dozens of distinct
# log templates; that spread is what lets the robust threshold separate genuine
# outliers from ordinary noise. Anomalies are injected separately below so the
# demo mirrors real usage: learn normal first, then flag the outliers.
NORMAL=(
    "INFO  nginx GET /api/users 200 served in %dms"
    "INFO  nginx POST /api/orders 201 served in %dms"
    "INFO  nginx GET /healthz 200 served in %dms"
    "INFO  auth session renewed for account %d"
    "INFO  auth login success for user id %d"
    "INFO  auth token refreshed for client %d"
    "INFO  cache hit for session token in shard %d"
    "INFO  cache set key user profile ttl %d seconds"
    "INFO  cache evicted %d cold entries from ring"
    "INFO  worker heartbeat ok queue depth %d"
    "INFO  worker picked up job from queue id %d"
    "INFO  worker finished job in %d milliseconds"
    "INFO  db query SELECT orders executed in %dms"
    "INFO  db query UPDATE inventory affected %d rows"
    "INFO  db connection pool acquired handle %d"
    "INFO  db transaction committed with %d statements"
    "INFO  scheduler cleanup job completed run %d"
    "INFO  scheduler enqueued nightly report task %d"
    "INFO  email delivered to recipient batch %d"
    "INFO  email queued newsletter to segment %d"
    "INFO  payment charge authorized via gateway id %d"
    "INFO  payment refund processed for order %d"
    "INFO  cdn asset served from edge node %d"
    "INFO  cdn cache purged for path revision %d"
    "INFO  websocket client subscribed to channel %d"
    "INFO  websocket ping pong latency %d ms"
    "INFO  metrics flushed %d datapoints to sink"
    "INFO  config reloaded revision %d applied"
    "INFO  featureflag evaluated rollout bucket %d"
    "INFO  ratelimiter allowed request within budget %d"
    "INFO  storage uploaded object part number %d"
    "INFO  search indexed %d documents into segment"
    "INFO  session garbage collected %d expired keys"
    "INFO  webhook dispatched event to subscriber %d"
    "INFO  audit recorded action by principal %d"
    "INFO  dns resolved upstream host in %d ms"
)
ANOMALIES=(
    "ERROR OOM killer activated for pid 4821 memory exhausted"
    "ERROR connection refused to postgres 5432 after 3 retries"
    "FATAL segfault in worker thread null pointer dereference"
    "PANIC kernel watchdog detected soft lockup on cpu core"
)

emit_normal() {
    local count="$1" i tmpl
    for ((i = 0; i < count; i++)); do
        tmpl="${NORMAL[$((RANDOM % ${#NORMAL[@]}))]}"
        # shellcheck disable=SC2059
        printf "$tmpl\n" "$((RANDOM % 1000))"
    done
}

MODE="${1:-watch}"

case "$MODE" in
watch | explain)
    extra=()
    [ "$MODE" = "explain" ] && extra=(--explain)
    echo "# Live log stream → TurboLog flags anomalies in real time"
    echo "\$ tail -f app.log | turbolog watch --only-anomalies ${extra[*]}"
    echo "# (learning normal traffic, then streaming — only anomalies are shown)"
    echo
    {
        # Warm-up baseline so the detector calibrates (suppressed by --only-anomalies).
        emit_normal 520
        # Now the interesting part: anomalies buried in ongoing normal traffic.
        for a in "${ANOMALIES[@]}"; do
            emit_normal 4
            sleep 0.5
            printf '%s\n' "$a"
        done
        emit_normal 4
    } | "$BIN" watch --only-anomalies "${extra[@]}"
    ;;
*)
    echo "Usage: $0 [watch|explain]" >&2
    exit 2
    ;;
esac
