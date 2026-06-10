# Lessons

## 2026-06-10 Phase 2
- **핑퐁 윈도우 의미론 확정**: 스펙은 스왑 후 쓰기 인덱스의 기존 데이터 처리를 정의하지 않음.
  봉인(seal) 후 빈 인덱스로 교체하는 설계를 채택 — 검색 스냅샷 = 직전 봉인 윈도우(10초).
  이력 검색은 Phase 3 시간 청크(.tvim) 파일 담당. (대안인 클론/이중쓰기/재구축은 비용 또는
  turbovec Clone 미구현으로 불가)
- **turbovec `search_with_allowlist`는 빈 목록·미존재 ID에 panic** —
  Tier 2에서 `contains()`로 사전 필터링 필수 (detect.rs `tier2_context`).
- **`prepare()`를 발행 전에 호출**: 회전 행렬/SIMD 레이아웃 초기화 비용을 스왑 스레드가
  지불하게 해서 첫 검색자의 레이턴시 스파이크 방지.
- 스펙의 `AtomicPtr<*mut>` 핑퐁은 스왑 중 읽기 참조 use-after-free → `ArcSwap` 스냅샷 +
  쓰기 전용 Mutex(검색 경로 비접촉)로 교정. 계획 단계에서 승인됨.
