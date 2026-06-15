# =============================================================================
# TurboLog — 멀티스테이지 Dockerfile
# =============================================================================
# 구조:
#   Stage 1 (builder) : rust:1-bookworm에서 cargo build --release
#   Stage 2 (runtime) : debian:bookworm-slim, non-root 유저 uid=10001
#
# ONNX 런타임 참고:
#   ort 2.0 크레이트는 기본적으로 빌드 시 onnxruntime을 자동 다운로드한다
#   (ORT_STRATEGY=download, cargo-features = "load-dynamic" 미사용 시 정적 링크).
#   동적 링크 빌드( ORT_DYLIB_PATH 또는 --features load-dynamic ) 시에는
#   builder 스테이지에서 생성된 libonnxruntime*.so 를 runtime으로 복사해야 한다.
#   기본(정적) 빌드라면 복사가 불필요하다.  현 Cargo.toml 기준 기본 정적 링크로 가정.
#
# 모델 파일 주입:
#   model.onnx / tokenizer.json (~90 MB) 은 이미지에 굽지 않는다.
#   런타임 시 TURBOLOG_MODEL_DIR 환경변수로 볼륨 경로를 지정한다.
#   Kubernetes에서는 initContainer 또는 사전 구워진 모델-init 이미지로 주입한다.
#   (deploy/k8s/deployment.yaml 참고)
#
# 환경변수:
#   TURBOLOG_PORT         HTTP 리슨 포트         (기본: 8087)
#   TURBOLOG_DATA_DIR     WAL/인덱스 저장 경로    (기본: /data)
#   TURBOLOG_MODEL_DIR    모델 파일 경로          (기본: /models)
#   TURBOLOG_EMBEDDERS    임베더 스레드 수        (기본: 2)
#   TURBOLOG_AUTH_TOKEN   Bearer 토큰 (없으면 무인증)
#   TURBOLOG_MAX_INFLIGHT 최대 동시 처리 수 (백프레셔)
# =============================================================================

# ──────────────────────────────────────────────────────────────────────────────
# Stage 1: builder
# ──────────────────────────────────────────────────────────────────────────────
FROM rust:1-bookworm AS builder

WORKDIR /build

# 의존성 캐시 레이어: Cargo.toml / Cargo.lock 만 먼저 복사해서
# 소스 변경 시에도 의존성 다운로드를 재사용한다.
COPY Cargo.toml Cargo.lock ./

# 더미 main.rs로 의존성만 미리 컴파일한다.
RUN mkdir -p src && echo 'fn main(){}' > src/main.rs && \
    cargo build --release 2>&1 | tail -5 || true && \
    # 더미 아티팩트 제거 (실제 소스로 재빌드 유도)
    rm -rf src target/release/turbolog target/release/deps/turbolog*

# 실제 소스 복사 후 릴리스 빌드
COPY src ./src

# ORT_STRATEGY=download : ort 크레이트가 onnxruntime 바이너리를 자동 다운로드.
# 동적 링크가 필요한 경우:
#   ENV ORT_LIB_LOCATION=/build/target/release/build/.../onnxruntime-...
#   COPY 후 runtime 스테이지에서 /usr/local/lib/ 로 복사 + ldconfig
ENV ORT_STRATEGY=download

RUN cargo build --release && \
    strip target/release/turbolog

# ──────────────────────────────────────────────────────────────────────────────
# Stage 2: runtime
# ──────────────────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# 런타임 의존성: CA 인증서, curl(헬스체크용)
RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        curl \
    && rm -rf /var/lib/apt/lists/*

# non-root 유저 생성 (uid=10001, gid=10001)
RUN groupadd -g 10001 turbolog && \
    useradd -u 10001 -g 10001 -s /sbin/nologin -M turbolog

# 바이너리 복사
COPY --from=builder /build/target/release/turbolog /usr/local/bin/turbolog

# [동적 링크 사용 시 주석 해제]
# COPY --from=builder /build/target/release/build/ort-*/out/lib/libonnxruntime*.so* /usr/local/lib/
# RUN ldconfig

# 데이터·모델 디렉터리 생성 (볼륨 마운트 마운트포인트)
RUN mkdir -p /data /models && chown -R 10001:10001 /data /models

# 환경변수 기본값
ENV TURBOLOG_PORT=8087 \
    TURBOLOG_DATA_DIR=/data \
    TURBOLOG_MODEL_DIR=/models \
    TURBOLOG_EMBEDDERS=2

# 모델 파일은 볼륨/initContainer로 주입 — 이미지에 포함하지 않는다.
# VOLUME ["/data", "/models"]

EXPOSE 8087

# /health 엔드포인트로 헬스체크
# 참고: 현재 http.rs에 /health가 없다면 /stats로 대체하거나 http.rs에 /health를 추가해야 함.
# WS3(http.rs 하드닝)에서 /health + /ready + /metrics 추가 예정.
HEALTHCHECK --interval=15s --timeout=5s --start-period=30s --retries=3 \
    CMD curl -f http://localhost:${TURBOLOG_PORT}/health || exit 1

USER 10001

ENTRYPOINT ["/usr/local/bin/turbolog"]
