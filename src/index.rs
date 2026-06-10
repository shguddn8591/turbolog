//! Ping-Pong Indexer — Read/Write 인덱스를 물리적으로 격리해 레이턴시 스파이크 제거.
//! (Phase 2 구현 예정)
//!
//! 스펙의 `AtomicPtr<*mut>` 설계는 스왑 중 읽기 참조 생존 문제(use-after-free)가 있어
//! `arc-swap`으로 교정: 검색자는 `ArcSwap::load`로 스냅샷 Arc를 얻고, 스왑 후에도
//! 기존 Arc가 살아있는 동안 안전하게 읽는다. 쓰기는 단일 ingest 스레드가 전담한다.
//!
//! 시스템 제약 (스펙 v1.0 §4.2 — Hard Physical Deletion):
//! 슬라이딩 윈도우 만료 시 `remove()` 반복 호출로 파편화를 유발하지 않는다.
//! 1시간 단위 청크 파일(.tvim) 자체를 OS 레벨에서 삭제(unlink)한다.

use arc_swap::ArcSwap;
use turbovec::IdMapIndex;

pub struct PingPongIndexer {
    /// Active Write 인덱스 — 단일 ingest 스레드만 접근.
    ptr_ingest: ArcSwap<IdMapIndex>,
    /// Active Read 인덱스 — 검색 스레드들이 락 없이 스냅샷 로드.
    ptr_search: ArcSwap<IdMapIndex>,
}

impl PingPongIndexer {
    /// Active Write 인덱스에 락 없이 벡터 삽입.
    pub fn ingest(&self, id: u64, vector: &[f32]) {
        let _ = (id, vector, &self.ptr_ingest);
        todo!("Phase 2: 단일 쓰기 스레드 ingest")
    }

    /// Active Read 인덱스 스냅샷 반환.
    pub fn get_search_index(&self) -> std::sync::Arc<IdMapIndex> {
        let _ = &self.ptr_search;
        todo!("Phase 2: ArcSwap::load 스냅샷")
    }

    /// 10초 주기 백그라운드 호출 — 포인터 원자적 스왑 및 디스크(WAL/Snapshot) 백업.
    pub fn swap_and_flush(&self, wal_path: &str) {
        let _ = wal_path;
        todo!("Phase 3: WAL 메커니즘")
    }
}
