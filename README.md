# TurboLog

고비용 GPU와 무거운 벡터 DB 없이, 초당 수천 건의 로그 스트림을 실시간으로 인덱싱하고
이상 징후를 탐지하는 초경량 시계열 벡터 엔진.

[turbovec](https://github.com/RyanCodrai/turbovec) (TurboQuant 양자화 인덱스) 기반.

## 데이터 흐름

```
Ingest → Parse(Drain) → Embed(LRU 캐시 / CPU ONNX) → Tier 1/2 탐지 → Ping-Pong 인덱싱 → Flush
```

- **Cache Hit**: 아는 템플릿이면 메모리에서 벡터 즉시 반환 (연산 비용 0)
- **Cache Miss**: 처음 보는 템플릿만 all-MiniLM-L6-v2 (ONNX, 384차원)로 벡터화

## 빌드 & 테스트

```bash
./scripts/download_model.sh   # ONNX 모델(86MB) + 토크나이저 다운로드
cargo build
cargo test                    # 모델 없으면 통합 테스트는 skip
```

## 구현 현황

- [x] **Phase 1 — Core Bindings & Cache**: Drain 파서(`drain-rs`), LRU 캐시,
      CPU(ONNX) 임베딩 파이프라인 (`src/ingest.rs`)
      — 통합 테스트: 합성 로그 100건, 캐시 히트율 95%, 384차원 L2 정규화 검증
- [ ] **Phase 2 — Ping-Pong & Centroid**: `arc-swap` 기반 Read/Write 인덱스 격리,
      K-Centroid Tier 1 + turbovec `IdMapIndex` allowlist Tier 2 (`src/index.rs`, `src/detect.rs` 골격)
- [ ] **Phase 3 — Persistence & API**: WAL 장애 복구, HTTP/gRPC 인터페이스

## 시스템 제약 (스펙 v1.0 §4)

1. **No Dynamic Re-training** — TQ+ 캘리브레이션/회전 행렬은 초기 결정 후 고정. 실시간 재학습 금지.
2. **Hard Physical Deletion** — 보존 기한 만료 시 `remove()` 반복 대신 1시간 단위 청크 파일을 OS 레벨 삭제.
3. **Stateless Embedder** — 임베딩 워커는 무상태 유지, 코어 엔진과 분리된 스레드 풀에서 횡적 확장.

## 스펙 대비 교정 사항

- `TurboQuantIndex` → **`IdMapIndex`**: 외부 u64 ID 의미론(`ingest(id, …)`, allowlist 검색)은
  IdMapIndex가 제공. TurboQuantIndex는 위치 기반이라 `swap_remove` 시 슬롯이 뒤바뀜.
- `AtomicPtr<*mut>` → **`arc-swap`**: 스왑 중 읽기 참조 생존 문제(use-after-free) 제거.
- `template_id` = 템플릿 문자열의 **FNV-1a 64bit 해시** (drain-rs에 안정적 클러스터 ID가 없음).
