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

## Phase 2 예정 (다음 세션)
- [ ] PingPongIndexer 구현 (단일 쓰기 스레드 + ArcSwap 스냅샷 읽기, 10초 스왑)
- [ ] AnomalyDetector Tier 1 (고정 centroid 32개 유클리드 거리)
- [ ] Tier 2 turbovec IdMapIndex allowlist 검색 연결
