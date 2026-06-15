# TurboLog — Phase 4: 프로덕션 하드닝 (100만 동시접속)

> 목표: 100만 동시접속 서비스에서 적극 사용 가능한 프로덕션 레벨.
> 전략: 공유 계약(metrics API) 동결 → 파일 소유권 비중첩 4개 워크스트림 병렬(sonnet) → 통합 검증.

## 진단 (병목 우선순위)
- **P0 처리량**: `wal: Mutex<Wal>` 단일 글로벌 쓰기 락이 모든 인입 직렬화 → 샤딩.
- **P0 관측성**: Prometheus/health/ready 부재 → 운영 불가.
- **P1 회복성**: 백프레셔/타임아웃/그레이스풀 셧다운 부재.
- **P1 배포**: Docker/k8s/HPA 부재 (100만 = N 레플리카 수평확장).
- **P2 검증**: criterion 벤치·SLO 문서 부재.

## 계약 동결 (선행, 내가 직접)
- [ ] `src/metrics.rs` — 프로세스 전역 Prometheus 텍스트 노출(의존성 0). 모든 에이전트가 호출.
- [ ] `Cargo.toml` — signal-hook, criterion(dev)+`[[bench]]`, `[profile.release]` lto.
- [ ] `src/lib.rs` — `pub mod metrics;` 선언.

## WS1 — 샤딩 인입 엔진 (engine.rs, index.rs, wal.rs)
- [ ] N 샤드: 샤드별 `Wal` + `PingPongIndexer` (글로벌 락 제거)
- [ ] 샤드별 swap_tick / 멀티샤드 크래시 복구 / 검색 N샤드×링 병합
- [ ] 공개 API(open/ingest_log/search_text/swap_tick/sweep_chunks/stats) 불변
- [ ] 메트릭 계측: ingest count/latency, anomaly
- [ ] 기존 19 테스트 전부 녹색 유지

## WS3 — HTTP 엣지 회복성 (http.rs)
- [ ] `/health`(liveness), `/ready`(readiness), `/metrics`(Prometheus)
- [ ] 인플라이트 백프레셔(초과 시 503) + 요청 본문 read 타임아웃
- [ ] `ServerConfig` 도입(addr/workers/auth/max_inflight/shutdown)
- [ ] 요청 메트릭 계측(2xx/4xx/5xx/rejected/inflight)

## WS4 — 배포·운영 (신규 파일만)
- [ ] 멀티스테이지 `Dockerfile`(non-root, distroless/slim) + `.dockerignore`
- [ ] `deploy/k8s/`: Deployment/Service/HPA/PDB/ConfigMap (probe→/ready,/health)
- [ ] `deploy/docker-compose.yml` + `docs/OPERATIONS.md`(100만 수평확장 토폴로지·TLS@ingress)

## WS5 — 벤치마크·SLO (신규 파일 위주)
- [ ] `benches/throughput.rs` criterion (모델 비의존: parse/cache/detect/fnv)
- [ ] `examples/loadtest.rs` 멀티스레드 경합 인입 측정 추가
- [ ] `docs/SLO.md`: 지연/처리량 목표 + 측정 결과

## 통합·검증 (선행 의존, 내가 직접)
- [ ] run_server 콜사이트(main.rs/loadtest) 정합 + SIGTERM 그레이스풀 셧다운
- [ ] `cargo build/test/clippy` 녹색 + loadtest 실증
- [ ] tasks/lessons.md 갱신

---

# TurboLog — Phase 1 체크리스트

## 스캐폴드
- [x] 디렉터리 구조 + git init
- [x] Cargo.toml 의존성 (turbovec, drain-rs, lru, ort, tokenizers, arc-swap, anyhow)
- [x] .gitignore (/target, /models)
- [x] scripts/download_model.sh (all-MiniLM-L6-v2 ONNX + tokenizer.json)

## Phase 1: Core Bindings & Cache (src/ingest.rs)
- [x] ParsedLog 구조체
- [x] TemplateParser (drain-rs 래퍼, template_id = FNV-1a 템플릿 해시)
- [x] Embedder (ort Session + tokenizers, mean pooling + L2 norm)
- [x] VectorCache (LruCache<u64, Arc<[f32]>>, 용량 10,000, hit/miss 카운터)

## 골격 (구현 없음, 타입만)
- [x] detect.rs — DetectionResult, AnomalyDetector (IdMapIndex로 교정)
- [x] index.rs — PingPongIndexer (arc-swap으로 교정)
- [x] lib.rs 모듈 연결

## 검증
- [x] 단위 테스트: 템플릿 ID 안정성, 캐시 hit/miss — 3개 통과
- [x] 통합 테스트: 합성 로그 100건 → 384차원 L2≈1.0, 히트율 95.0% (hits=95, misses=5)
- [x] cargo build 경고 없음 + cargo test 전체 통과 (5/5)
- [x] README.md + 첫 커밋
- [x] GitHub Actions 고급 CI 파이프라인 구축 (Lint, 매트릭스 OS 테스트, Security Audit, Code Coverage)

## Phase 2: Ping-Pong & Centroid
- [x] PingPongIndexer 구현 (쓰기 Mutex + ArcSwap 스냅샷 읽기, swap_and_flush)
- [x] 봉인 윈도우 .tvim 청크 백업 (flush_path) + load 라운드트립 검증
- [x] AnomalyDetector Tier 1 (고정 centroid 유클리드 거리, fit = 1회 K-means 후 동결)
- [x] Tier 2 IdMapIndex 심층 검색 + allowlist 필터 (panic 가드 포함)
- [x] 동시성 테스트 (ingest/search/swap 3-스레드) + E2E 로그 이상 탐지 테스트
- [x] 테스트 12/12 통과

## Phase 3: Persistence & API
- [x] WAL 장애 복구 (wal.rs — append/rotate/replay, 불완전 꼬리 무시, 크래시 복구 테스트)
- [x] 시간 청크 관리 (chunks.rs — hour-N 디렉터리, 만료 시 OS unlink sweep)
- [x] 엔진 조립 (engine.rs — WAL→인덱싱 직렬화, 링 병합 검색, 자동 캘리브레이션 후 동결)
- [x] HTTP API (http.rs — POST /logs, POST /search, GET /stats, tiny_http 워커 풀)
- [x] 서버 데몬 (main.rs — 10초 스왑 틱 + 1시간 sweep, env 설정)
- [x] 테스트 19/19 통과 + 릴리스 바이너리 스모크 런 (스왑 데몬·검색·청크·WAL 로테이트 실증)

## 향후 (스펙 외 개선 후보)
- [ ] gRPC 인터페이스 (스펙 병기 항목 — 필요 시 동일 엔진 위에 추가)
- [ ] 디스크 세그먼트 대상 이력 검색 (링 범위 초과 시간대)
- [x] 임베더 풀 분리 (§4.3 1단계 — 프로세스 내 풀, TURBOLOG_EMBEDDERS)
- [ ] Stateless Embedder 횡적 확장 (§4.3 완전체 — 워커 프로세스 분리 배포)
