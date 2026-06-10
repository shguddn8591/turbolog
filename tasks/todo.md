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

## Phase 2: Ping-Pong & Centroid
- [x] PingPongIndexer 구현 (쓰기 Mutex + ArcSwap 스냅샷 읽기, swap_and_flush)
- [x] 봉인 윈도우 .tvim 청크 백업 (flush_path) + load 라운드트립 검증
- [x] AnomalyDetector Tier 1 (고정 centroid 유클리드 거리, fit = 1회 K-means 후 동결)
- [x] Tier 2 IdMapIndex 심층 검색 + allowlist 필터 (panic 가드 포함)
- [x] 동시성 테스트 (ingest/search/swap 3-스레드) + E2E 로그 이상 탐지 테스트
- [x] 테스트 12/12 통과

## Phase 3 예정 (다음 세션)
- [ ] WAL 장애 복구 로직 (재시작 시 청크 로드)
- [ ] 시간 청크 관리 (1시간 단위 생성, 보존 기한 만료 시 OS unlink)
- [ ] HTTP/gRPC 인터페이스 (삽입/검색)
- [ ] 10초 주기 스왑 백그라운드 스레드 (데몬 런타임)
