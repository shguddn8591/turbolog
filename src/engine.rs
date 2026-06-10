//! TurboLog 엔진 — 전체 데이터 흐름의 조립.
//!
//! Ingest → Parse(Drain) → Embed(캐시/ONNX) → WAL → Ping-Pong 인덱싱 → 탐지
//! 10초 주기 `swap_tick`: 윈도우 봉인 → .tvim 세그먼트 백업 → 링 발행 → WAL 로테이트
//!
//! ## 락 순서 불변식
//! `wal` Mutex가 쓰기 경로(WAL append + indexer ingest / 스왑 + rotate)의 단일 직렬화
//! 지점이다. 이를 어기면 "WAL에는 있는데 봉인 직후 rotate로 지워지는" 유실 레이스가 생긴다.
//! 검색 경로는 어떤 쓰기 락도 건드리지 않는다 (ArcSwap 스냅샷 + ring Mutex 순간 점유).
//!
//! ## 캘리브레이션 (스펙 §4.1 — No Dynamic Re-training)
//! 시동 후 처음 보는 템플릿 벡터를 모아 `calibration_templates`개가 차면 K-means를 1회
//! 실행해 centroid를 동결한다. 이후 재학습은 없다.

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Result};
use serde::Serialize;
use turbovec::IdMapIndex;

use crate::chunks::ChunkStore;
use crate::detect::{AnomalyDetector, DetectionResult};
use crate::index::PingPongIndexer;
use crate::ingest::{Embedder, VectorCache};
use crate::wal::Wal;

pub struct EngineConfig {
    pub dim: usize,
    pub bit_width: usize,
    /// WAL 파일과 청크 디렉터리가 놓이는 곳.
    pub data_dir: PathBuf,
    pub swap_interval_secs: u64,
    /// 시간 청크 보존 기한 (기본 7일).
    pub retention_hours: u64,
    /// 인메모리에 유지하는 최근 봉인 윈도우 수 (검색 깊이 = ring × swap_interval).
    pub ring_windows: usize,
    /// K-Centroid 개수 (Tier 1).
    pub centroids: usize,
    pub anomaly_threshold: f32,
    /// 이만큼의 고유 템플릿이 모이면 centroid를 1회 학습 후 동결.
    pub calibration_templates: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            dim: 384,
            bit_width: 4,
            data_dir: PathBuf::from("./data"),
            swap_interval_secs: 10,
            retention_hours: 7 * 24,
            ring_windows: 30,
            centroids: 32,
            anomaly_threshold: 0.5,
            calibration_templates: 64,
        }
    }
}

#[derive(Serialize)]
pub struct AnomalyReport {
    pub score: f32,
    pub nearest_incidents: Vec<u64>,
}

#[derive(Serialize)]
pub struct LogReport {
    pub id: u64,
    pub template_id: u64,
    pub timestamp: i64,
    /// None = 정상 또는 캘리브레이션 전.
    pub anomaly: Option<AnomalyReport>,
}

#[derive(Serialize)]
pub struct SearchHit {
    pub id: u64,
    pub score: f32,
}

#[derive(Serialize)]
pub struct Stats {
    pub cache_hits: u64,
    pub cache_misses: u64,
    pub cache_hit_rate: f64,
    pub pending_window_len: usize,
    pub ring_windows: usize,
    pub ring_vectors: usize,
    pub detector_calibrated: bool,
    pub ingested_total: u64,
}

pub struct TurboLogEngine {
    cfg: EngineConfig,
    cache: Mutex<VectorCache>,
    indexer: PingPongIndexer,
    /// 쓰기 경로 직렬화 지점 — 모듈 doc의 락 순서 불변식 참조.
    wal: Mutex<Wal>,
    chunks: ChunkStore,
    /// 최근 봉인 윈도우들 (최신이 앞). 검색은 이 링을 병합한다.
    ring: Mutex<VecDeque<Arc<IdMapIndex>>>,
    detector: RwLock<Option<AnomalyDetector>>,
    /// 캘리브레이션용 신규 템플릿 벡터 버퍼 (동결 후 비움).
    calibration: Mutex<Vec<f32>>,
    next_id: AtomicU64,
    ingested_total: AtomicU64,
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

impl TurboLogEngine {
    /// 엔진을 연다. WAL에 잔여 레코드가 있으면(크래시 복구) 쓰기 윈도우로 재생한다.
    pub fn open(cfg: EngineConfig, embedder: Embedder) -> Result<Self> {
        std::fs::create_dir_all(&cfg.data_dir)?;
        let wal_path = cfg.data_dir.join("wal.bin");
        let indexer = PingPongIndexer::new(cfg.dim, cfg.bit_width)?;

        let replayed = Wal::replay(&wal_path, cfg.dim)?;
        let mut max_replayed_id = 0u64;
        for (id, vector) in &replayed {
            indexer.ingest(*id, vector)?;
            max_replayed_id = max_replayed_id.max(*id);
        }

        // 시간 기반 시작점 — 재시작 후에도 과거 세그먼트의 ID와 충돌하지 않게 단조 증가.
        let next_id = ((now_millis() as u64) << 20).max(max_replayed_id + 1);

        Ok(Self {
            chunks: ChunkStore::new(cfg.data_dir.join("chunks"))?,
            wal: Mutex::new(Wal::open(&wal_path, cfg.dim)?),
            indexer,
            cache: Mutex::new(VectorCache::new(embedder)),
            ring: Mutex::new(VecDeque::new()),
            detector: RwLock::new(None),
            calibration: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(next_id),
            ingested_total: AtomicU64::new(replayed.len() as u64),
            cfg,
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }

    /// 로그 1건 인입: 파싱/임베딩 → WAL → 인덱싱 → 탐지.
    pub fn ingest_log(&self, line: &str) -> Result<LogReport> {
        let (parsed, vector, new_template) = {
            let mut cache = self.cache.lock().unwrap();
            let misses_before = cache.misses();
            let (parsed, vector) = cache.get_or_embed(line)?;
            (parsed, vector, cache.misses() > misses_before)
        };

        if new_template {
            self.maybe_calibrate(&vector);
        }

        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        {
            let mut wal = self.wal.lock().unwrap();
            wal.append(id, &vector)?;
            self.indexer.ingest(id, &vector)?;
        }
        self.ingested_total.fetch_add(1, Ordering::Relaxed);

        let anomaly = {
            let detector = self.detector.read().unwrap();
            detector.as_ref().and_then(|d| {
                match d.detect(&vector, &self.indexer.get_search_index()) {
                    DetectionResult::Normal => None,
                    DetectionResult::Anomaly {
                        score,
                        nearest_incidents,
                    } => Some(AnomalyReport {
                        score,
                        nearest_incidents,
                    }),
                }
            })
        };

        Ok(LogReport {
            id,
            template_id: parsed.template_id,
            timestamp: parsed.timestamp,
            anomaly,
        })
    }

    /// 텍스트 쿼리로 링(최근 봉인 윈도우들)을 병합 검색한다.
    /// turbovec 점수는 내적 유사도(클수록 가까움) — 내림차순 병합.
    pub fn search_text(&self, query: &str, k: usize) -> Result<Vec<SearchHit>> {
        ensure!(k > 0, "k는 1 이상");
        let vector = self.cache.lock().unwrap().embed_uncached(query)?;
        let windows: Vec<Arc<IdMapIndex>> = self.ring.lock().unwrap().iter().cloned().collect();

        let mut hits: Vec<SearchHit> = Vec::new();
        for window in windows {
            if window.is_empty() {
                continue;
            }
            let (scores, ids) = window.search(&vector, k);
            hits.extend(
                scores
                    .into_iter()
                    .zip(ids)
                    .map(|(score, id)| SearchHit { id, score }),
            );
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(k);
        Ok(hits)
    }

    /// 스왑 주기마다 호출: 윈도우 봉인 → .tvim 세그먼트 백업 → 링 발행 → WAL 로테이트.
    /// 빈 윈도우면 아무것도 하지 않는다 (유휴 시 세그먼트 파일 스팸 방지).
    pub fn swap_tick(&self) -> Result<bool> {
        let mut wal = self.wal.lock().unwrap();
        if self.indexer.pending_len() == 0 {
            return Ok(false);
        }
        let segment = self.chunks.segment_path(now_millis())?;
        self.indexer.swap_and_flush(Some(&segment))?;
        wal.rotate()?;
        drop(wal);

        let sealed = self.indexer.get_search_index();
        let mut ring = self.ring.lock().unwrap();
        ring.push_front(sealed);
        ring.truncate(self.cfg.ring_windows.max(1));
        Ok(true)
    }

    /// 보존 기한이 지난 시간 청크 디렉터리를 OS 레벨에서 삭제한다.
    pub fn sweep_chunks(&self) -> Result<usize> {
        self.chunks.sweep(self.cfg.retention_hours, now_millis())
    }

    pub fn stats(&self) -> Stats {
        let (cache_hits, cache_misses, cache_hit_rate) = {
            let cache = self.cache.lock().unwrap();
            (cache.hits(), cache.misses(), cache.hit_rate())
        };
        let ring = self.ring.lock().unwrap();
        Stats {
            cache_hits,
            cache_misses,
            cache_hit_rate,
            pending_window_len: self.indexer.pending_len(),
            ring_windows: ring.len(),
            ring_vectors: ring.iter().map(|w| w.len()).sum(),
            detector_calibrated: self.detector.read().unwrap().is_some(),
            ingested_total: self.ingested_total.load(Ordering::Relaxed),
        }
    }

    /// 신규 템플릿 벡터를 모아 목표치에 도달하면 centroid를 1회 학습 후 동결한다.
    fn maybe_calibrate(&self, vector: &[f32]) {
        if self.detector.read().unwrap().is_some() {
            return;
        }
        let mut calibration = self.calibration.lock().unwrap();
        calibration.extend_from_slice(vector);
        let templates = calibration.len() / self.cfg.dim;
        if templates < self.cfg.calibration_templates {
            return;
        }
        let detector = AnomalyDetector::fit(
            &calibration,
            self.cfg.dim,
            self.cfg.centroids,
            self.cfg.anomaly_threshold,
        );
        let mut slot = self.detector.write().unwrap();
        if slot.is_none() {
            *slot = Some(detector);
            calibration.clear();
            calibration.shrink_to_fit();
        }
    }
}
