# =============================================================================
# TurboLog — Multi-stage Dockerfile
# =============================================================================
# Structure:
#   Stage 1 (builder) : cargo build --release from rust:1-bookworm
#   Stage 2 (runtime) : debian:bookworm-slim, non-root user uid=10001
#
# ONNX Runtime notes:
#   ort 2.0 crate automatically downloads onnxruntime during build by default
#   (Static link when ORT_STRATEGY=download and cargo-features = "load-dynamic" is not used).
#   For dynamic link builds (ORT_DYLIB_PATH or --features load-dynamic),
#   you must copy libonnxruntime*.so generated in the builder stage to the runtime stage.
#   It is unnecessary for default (static) builds. Assuming default static link based on the current Cargo.toml.
#
# Model file injection:
#   model.onnx / tokenizer.json (~90 MB) are not baked into the image.
#   Specify the volume path via the TURBOLOG_MODEL_DIR environment variable at runtime.
#   In Kubernetes, inject via initContainer or a pre-baked model-init image.
#   (See deploy/k8s/deployment.yaml)
#
# Environment variables:
#   TURBOLOG_PORT         HTTP listen port              (Default: 8087)
#   TURBOLOG_DATA_DIR     WAL/Index storage path        (Default: /data)
#   TURBOLOG_MODEL_DIR    Model file path               (Default: /models)
#   TURBOLOG_EMBEDDERS    Number of embedder threads    (Default: 2)
#   TURBOLOG_AUTH_TOKEN   Bearer token (No auth if empty)
#   TURBOLOG_MAX_INFLIGHT Max concurrent processing     (Backpressure)
# =============================================================================

# ──────────────────────────────────────────────────────────────────────────────
# Stage 1: builder
# ──────────────────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

WORKDIR /build

# Dependency cache layer: Copy only Cargo.toml / Cargo.lock first
# to reuse the dependency download even when the source changes.
COPY Cargo.toml Cargo.lock ./

# Pre-compile only dependencies using a dummy main.rs.
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs && \
    cargo build --release 2>&1 | tail -5 || true && \
    # Remove dummy artifacts (inducing a rebuild with the actual source)
    rm -rf src target/release/turbolog target/release/deps/turbolog*

# Copy the actual source and build the release
COPY src ./src

# ORT_STRATEGY=download : The ort crate automatically downloads the onnxruntime binary.
# If dynamic linking is required:
#   ENV ORT_LIB_LOCATION=/build/target/release/build/.../onnxruntime-...
#   After COPY, copy to /usr/local/lib/ in the runtime stage + run ldconfig
ENV ORT_STRATEGY=download

RUN cargo build --release && \
    strip target/release/turbolog

# ──────────────────────────────────────────────────────────────────────────────
# Stage 2: runtime
# ──────────────────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# Runtime dependencies: CA certificates, curl (for healthcheck)
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
    && rm -rf /var/lib/apt/lists/*

# Create a non-root user (uid=10001, gid=10001)
RUN groupadd -g 10001 turbolog && \
    useradd -u 10001 -g 10001 -s /sbin/nologin -M turbolog

# Copy the binary
COPY --from=builder /build/target/release/turbolog /usr/local/bin/turbolog

# [Uncomment if using dynamic linking]
# COPY --from=builder /build/target/release/build/ort-*/out/lib/libonnxruntime*.so* /usr/local/lib/
# RUN ldconfig

# Create data/model directories (mount points for volumes)
RUN mkdir -p /data /models && chown -R 10001:10001 /data /models

# Default environment variables
ENV TURBOLOG_PORT=8087 \
    TURBOLOG_DATA_DIR=/data \
    TURBOLOG_MODEL_DIR=/models \
    TURBOLOG_EMBEDDERS=2

# Model files are injected via volume/initContainer — not included in the image.
# VOLUME ["/data", "/models"]

EXPOSE 8087

# Healthcheck via /health endpoint
# Note: If /health does not exist in http.rs, replace it with /stats or add /health to http.rs.
# /health + /ready + /metrics are planned to be added in WS3 (http.rs hardening).
HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=3 \
    CMD curl -f http://localhost:${TURBOLOG_PORT}/health || exit 1

USER 10001

ENTRYPOINT ["/usr/local/bin/turbolog"]
