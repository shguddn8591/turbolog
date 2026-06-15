# TurboLog 운영 가이드

> 버전: 0.1.0 | 최종 수정: 2026-06-15

---

## 목차

1. [100만 동시접속 토폴로지](#1-100만-동시접속-토폴로지)
2. [환경변수 표](#2-환경변수-표)
3. [엔드포인트 표](#3-엔드포인트-표)
4. [TLS 및 네트워크 보안](#4-tls-및-네트워크-보안)
5. [스케일링 및 롤아웃](#5-스케일링-및-롤아웃)
6. [관측성 (Observability)](#6-관측성-observability)
7. [그레이스풀 셧다운](#7-그레이스풀-셧다운)
8. [백업 및 복구](#8-백업-및-복구)

---

## 1. 100만 동시접속 토폴로지

### 핵심 아키텍처 원칙

TurboLog 노드는 **인메모리 인덱스(arc-swap)**를 보유한 상태풀(stateful-lite) 서비스다.
무작위 LB(라운드로빈)로는 동일 로그 스트림이 여러 노드에 분산되어 인덱스 일관성이 깨진다.
따라서 **수집 에이전트/클라이언트는 키(테넌트 ID 또는 스트림 ID) 기준 일관 해시(consistent hash)로 특정 레플리카에 고정**된다.

### 수평 확장 계산

```
1,000,000 동시 요청/s
  ÷ 레플리카당 처리량 (예: 20,000 req/s, TURBOLOG_EMBEDDERS=4 기준)
  = 50 레플리카 (HPA max)
```

각 레플리카는 내부적으로 `TURBOLOG_EMBEDDERS` 수만큼 임베딩 워커를 병렬 운용한다.
즉, **총 처리량 = N 레플리카 × 레플리카당 임베더 수 × 임베더 처리속도**의 곱으로 확장된다.

### ASCII 아키텍처 다이어그램

```
클라이언트/수집 에이전트 (로그 소스)
        │
        ▼
┌─────────────────────────────────────────────┐
│         외부 로드밸런서 / Ingress              │
│   일관 해시 라우팅 (hash_key = tenant_id)     │
│   TLS 종단 ← 여기서만 처리, 앱 내부는 평문    │
└────────────┬─────────────┬──────────────────┘
             │             │          ...
             ▼             ▼
     ┌──────────┐   ┌──────────┐   ┌──────────┐
     │ TurboLog │   │ TurboLog │   │ TurboLog │   (레플리카 6~50개)
     │ Pod #0   │   │ Pod #1   │   │ Pod #N   │
     │          │   │          │   │          │
     │ shard A  │   │ shard B  │   │ shard C  │   ← 테넌트/스트림 샤드
     │          │   │          │   │          │
     │ [embed0] │   │ [embed0] │   │ [embed0] │
     │ [embed1] │   │ [embed1] │   │ [embed1] │   ← 노드 내 임베더 병렬화
     │ [embed2] │   │ [embed2] │   │ [embed2] │
     │          │   │          │   │          │
     │ /data    │   │ /data    │   │ /data    │   ← WAL + .tvim (노드 로컬)
     └──────────┘   └──────────┘   └──────────┘
             │             │          ...
             ▼             ▼
     ┌──────────────────────────────────┐
     │       Prometheus / Grafana       │
     │  (스크레이프: /metrics 각 파드)   │
     └──────────────────────────────────┘
```

### 일관 해시 LB 설정 예시 (Nginx Ingress)

```nginx
# nginx.conf 또는 Ingress annotation
upstream turbolog {
    hash $http_x_tenant_id consistent;
    server turbolog-0.turbolog:8087;
    server turbolog-1.turbolog:8087;
    # ...
}
```

Envoy 사용 시: `hash_policy` → `header: x-tenant-id`.

---

## 2. 환경변수 표

| 변수명 | 기본값 | 필수 | 설명 |
|--------|--------|------|------|
| `TURBOLOG_PORT` | `8087` | 아니오 | HTTP 리슨 포트 |
| `TURBOLOG_DATA_DIR` | `./data` | 아니오 | WAL·인덱스 스냅샷 경로. PVC 마운트 권장 |
| `TURBOLOG_MODEL_DIR` | `./models` | 예* | `model.onnx`, `tokenizer.json` 위치. initContainer로 주입 |
| `TURBOLOG_EMBEDDERS` | `2` | 아니오 | ONNX 임베딩 워커 수. 늘릴수록 메모리·CPU 증가 (세션당 ~90 MB) |
| `TURBOLOG_AUTH_TOKEN` | _(없음)_ | 아니오 | Bearer 토큰. 설정 시 모든 요청에 `Authorization: Bearer <token>` 필요 |
| `TURBOLOG_MAX_INFLIGHT` | _(무제한)_ | 아니오 | 동시 처리 요청 수 상한. 초과 시 503 반환 (백프레셔) |

> *`TURBOLOG_MODEL_DIR` 경로에 파일이 없으면 임베딩이 실패한다. Kubernetes에서는 initContainer가 파일을 채워 넣은 뒤 메인 컨테이너가 기동된다.

---

## 3. 엔드포인트 표

| 경로 | 메서드 | 인증 | 설명 | 응답 예시 |
|------|--------|------|------|-----------|
| `/logs` | POST | 필요* | 로그 라인 배치 인입. 본문: `{"logs": ["line1", ...]}`. 최대 1 MiB | `{"results": [...]}` |
| `/search` | POST | 필요* | 벡터 유사도 검색. 본문: `{"query": "...", "k": 5}` | `{"results": [...]}` |
| `/stats` | GET | 필요* | 엔진 통계 (인입 수, 인덱스 크기 등) | `{...stats...}` |
| `/health` | GET | 아니오 | 라이브니스 프로브. 앱 프로세스 정상 시 200 | `{"status":"ok"}` |
| `/ready` | GET | 아니오 | 레디니스 프로브. 모델 로드 완료 시 200, 준비 중 503 | `{"status":"ready"}` |
| `/metrics` | GET | 아니오 | Prometheus 텍스트 형식 메트릭 | `# HELP ...` |

> *`TURBOLOG_AUTH_TOKEN` 설정 시 인증 필요. 미설정 시 모든 경로 무인증 접근 가능.
>
> **주의**: `/health`, `/ready`, `/metrics`는 WS3(http.rs 하드닝) 작업에서 추가해야 한다.
> 현재 `http.rs`에는 `/logs`, `/search`, `/stats`만 구현되어 있다.

---

## 4. TLS 및 네트워크 보안

**TLS 종단은 인그레스 또는 외부 로드밸런서에서 처리한다.**
TurboLog 앱은 평문 HTTP(포트 8087)로만 통신하며, 클러스터 내부 신뢰 네트워크를 전제로 한다.

- **인그레스 → 파드** 구간: 클러스터 내부 평문 (신뢰 네트워크)
- **외부 → 인그레스** 구간: TLS 1.2+ (인그레스 컨트롤러가 인증서 관리)
- mTLS가 필요한 경우 Istio/Linkerd 사이드카 패턴으로 구성한다.

```
클라이언트 ─── TLS ─── [Ingress / ELB] ─── HTTP ─── TurboLog Pod
              (443)                         (8087)
```

---

## 5. 스케일링 및 롤아웃

### 롤링 업데이트

```bash
# 이미지 업데이트
kubectl -n turbolog set image deployment/turbolog turbolog=registry.example.com/turbolog:0.2.0

# 롤아웃 상태 확인
kubectl -n turbolog rollout status deployment/turbolog

# 문제 발생 시 롤백
kubectl -n turbolog rollout undo deployment/turbolog
```

롤링 업데이트 중 PDB(`minAvailable: 4`)가 최소 4개 레플리카를 보장하므로
6개 레플리카 기준 최대 2개씩 순차 교체된다.

### 수동 스케일

```bash
# 임시 스케일아웃 (트래픽 급증 대비)
kubectl -n turbolog scale deployment/turbolog --replicas=20

# HPA 오버라이드 해제 후 자동 복귀
kubectl -n turbolog patch hpa turbolog -p '{"spec":{"minReplicas":6}}'
```

### HPA 동작 확인

```bash
kubectl -n turbolog get hpa turbolog -w
```

---

## 6. 관측성 (Observability)

### Prometheus 스크레이프 설정

각 파드는 `prometheus.io/scrape: "true"` 어노테이션을 보유한다.
`/metrics` 엔드포인트에서 Prometheus 텍스트 형식으로 지표를 노출한다.

```yaml
# prometheus.yml (scrape_config 예시, 어노테이션 기반)
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

### 핵심 지표

| 지표명 | 타입 | 설명 | 알림 권장 임계치 |
|--------|------|------|-----------------|
| `turbolog_ingested_total` | Counter | 전체 인입 로그 수 | — |
| `turbolog_inflight_requests` | Gauge | 현재 처리 중인 요청 수 | > `TURBOLOG_MAX_INFLIGHT × 0.8` |
| `turbolog_ingest_latency_seconds` | Histogram | 인입 처리 지연. p99 기준 | p99 > 200ms |
| `turbolog_http_5xx_total` | Counter | HTTP 500번대 응답 수 | 분당 > 10 |
| `turbolog_cache_hit_rate` | Gauge | 임베딩 캐시 적중률 (0~1) | < 0.5 (캐시 효율 저하) |
| `turbolog_anomaly_detections_total` | Counter | 이상 탐지 발생 횟수 | — |

### 권장 Grafana 대시보드 패널

1. **인입 처리량** (ingested_total rate 1m)
2. **인-플라이트 요청 수** (inflight gauge)
3. **지연 분포** (ingest_latency p50/p95/p99)
4. **에러율** (5xx rate / 전체 요청 rate)
5. **캐시 적중률**
6. **레플리카 수** (kube_deployment_status_replicas)

---

## 7. 그레이스풀 셧다운

TurboLog는 SIGTERM 수신 시 다음 순서로 종료한다:

```
SIGTERM 수신
    │
    ├─ 1. 신규 요청 수락 중단 (tiny_http 서버 클로즈)
    │
    ├─ 2. 진행 중인 요청 완료 대기 (최대 terminationGracePeriodSeconds=30s)
    │
    ├─ 3. 마지막 swap_tick 실행 (인덱스 스냅샷 + WAL 플러시)
    │
    └─ 4. 프로세스 종료
```

Kubernetes는 파드를 Service 엔드포인트에서 제거한 뒤 SIGTERM을 전송하므로,
`preStop: sleep 5`로 엔드포인트 전파 완료를 기다린다.

**강제 종료 방지**: `terminationGracePeriodSeconds: 30` 이내에 종료가 완료되지 않으면
SIGKILL이 전송된다. 처리량이 높은 환경에서는 이 값을 60초로 늘리는 것을 고려한다.

---

## 8. 백업 및 복구

### 저장 파일 구조

```
/data/
  ├── wal-<shard_id>.wal      # Write-Ahead Log (바이너리)
  └── index-<shard_id>.tvim   # 인덱스 스냅샷 (직렬화된 벡터 인덱스)
```

### 백업 절차

```bash
# 1. 대상 파드 선택
POD=$(kubectl -n turbolog get pods -l app.kubernetes.io/name=turbolog -o name | head -1)

# 2. WAL + 인덱스 스냅샷 압축 복사
kubectl -n turbolog exec "$POD" -- tar czf - /data | \
  gzip > "turbolog-backup-$(date +%Y%m%d-%H%M%S).tar.gz"

# 3. 원격 스토리지 업로드 (예: S3)
aws s3 cp turbolog-backup-*.tar.gz s3://my-bucket/turbolog-backups/
```

PVC 사용 시 스냅샷(VolumeSnapshot) API 또는 클라우드 스토리지 스냅샷을 권장한다.

### 복구 절차

```bash
# 1. 파드 중지 (replicas=0)
kubectl -n turbolog scale deployment/turbolog --replicas=0

# 2. 백업 압축 해제 후 PVC 또는 emptyDir에 복사
# (별도 복구 파드 또는 kubectl cp 활용)

# 3. 파드 재기동
kubectl -n turbolog scale deployment/turbolog --replicas=6

# 4. 정상 기동 확인
kubectl -n turbolog rollout status deployment/turbolog
kubectl -n turbolog logs -l app.kubernetes.io/name=turbolog --tail=50
```

### 복구 소요 시간 목표 (RTO)

| 시나리오 | 예상 RTO |
|----------|----------|
| 파드 재시작 (emptyDir, 인덱스 재구성) | WAL 크기에 비례, 보통 30~120초 |
| PVC 스냅샷 복구 (동일 AZ) | 5~15분 |
| 전체 클러스터 재구성 | 30분~1시간 (모델 다운로드 포함) |

> **참고**: TurboLog의 인메모리 인덱스는 WAL을 재생(replay)하여 재구성할 수 있다.
> WAL 보존 기간은 디스크 용량과 RTO 목표에 맞게 조정한다.
