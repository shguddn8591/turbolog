//! Ping-Pong Indexer — Read/Write 인덱스를 물리적으로 격리해 레이턴시 스파이크 제거.
//!
//! 스펙의 `AtomicPtr<*mut>` 설계는 스왑 중 읽기 참조 생존 문제(use-after-free)가 있어
//! `arc-swap`으로 교정: 검색자는 `ArcSwap::load_full`로 스냅샷 Arc를 얻고, 스왑 후에도
//! 기존 Arc가 살아있는 동안 안전하게 읽는다. 쓰기는 ingest 스레드 전용 Mutex를 거치지만
//! 검색 경로는 이 락을 절대 건드리지 않으므로 읽기 레이턴시 스파이크가 없다.
//!
//! ## 윈도우 의미론
//! `swap_and_flush` 호출 시 현재 쓰기 인덱스가 봉인(seal)되어 검색 스냅샷으로 발행되고,
//! 쓰기는 빈 인덱스에서 다시 시작한다. 즉 검색 스냅샷은 **직전 봉인 윈도우(기본 10초)**의
//! 데이터를 담는다. 윈도우를 넘는 이력 검색은 Phase 3의 시간 청크(.tvim) 파일이 담당한다.
//!
//! 시스템 제약 (스펙 v1.0 §4.2 — Hard Physical Deletion):
//! 슬라이딩 윈도우 만료 시 `remove()` 반복 호출로 파편화를 유발하지 않는다.
//! 1시간 단위 청크 파일(.tvim) 자체를 OS 레벨에서 삭제(unlink)한다.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{ensure, Context, Result};
use arc_swap::ArcSwap;
use turbovec::IdMapIndex;

pub struct PingPongIndexer {
    /// Active Write 인덱스 — ingest 경로 전용. 검색 경로는 이 락을 건드리지 않는다.
    write: Mutex<IdMapIndex>,
    /// Active Read 스냅샷 — 검색 스레드들이 락 없이 로드.
    search: ArcSwap<IdMapIndex>,
    dim: usize,
    bit_width: usize,
}

impl PingPongIndexer {
    pub fn new(dim: usize, bit_width: usize) -> Result<Self> {
        Ok(Self {
            write: Mutex::new(IdMapIndex::new(dim, bit_width)?),
            search: ArcSwap::from_pointee(IdMapIndex::new(dim, bit_width)?),
            dim,
            bit_width,
        })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Active Write 인덱스에 벡터 삽입. 검색 경로와 락을 공유하지 않는다.
    pub fn ingest(&self, id: u64, vector: &[f32]) -> Result<()> {
        ensure!(
            vector.len() == self.dim,
            "벡터 차원 불일치: {} != {}",
            vector.len(),
            self.dim
        );
        self.write
            .lock()
            .unwrap()
            .add_with_ids(vector, &[id])
            .with_context(|| format!("id {id} 삽입 실패"))?;
        Ok(())
    }

    /// Active Read 스냅샷 반환 — 락 프리. 스왑이 일어나도 반환된 Arc는 유효하다.
    pub fn get_search_index(&self) -> Arc<IdMapIndex> {
        self.search.load_full()
    }

    /// 10초 주기로 백그라운드 스레드에서 호출.
    /// 쓰기 인덱스를 봉인해 검색 스냅샷으로 원자적 발행하고, `flush_path`가 주어지면
    /// 봉인된 윈도우를 .tvim 청크로 디스크에 백업한다. (WAL 복구 로직은 Phase 3)
    pub fn swap_and_flush(&self, flush_path: Option<&Path>) -> Result<()> {
        let fresh = IdMapIndex::new(self.dim, self.bit_width)?;
        let sealed = {
            let mut guard = self.write.lock().unwrap();
            std::mem::replace(&mut *guard, fresh)
        };
        // 검색 캐시(회전 행렬, SIMD 레이아웃) 일회성 비용을 발행 전에 지불 —
        // 첫 검색자가 초기화 비용을 떠안는 스파이크 방지.
        sealed.prepare();
        if let Some(path) = flush_path {
            sealed
                .write(path)
                .with_context(|| format!("청크 백업 실패: {}", path.display()))?;
        }
        self.search.store(Arc::new(sealed));
        Ok(())
    }
}
