//! TurboLog Engine — Assembly of the entire data pipeline.
//!
//! Ingest → Parse (Drain) → Embed (Cache/ONNX) → WAL → Ping-Pong Indexing → Anomaly Detection
//! 10-second periodic `swap_tick`: Seal window → Backup `.tvim` segment → Publish to Ring → Rotate WAL
//!
//! ## Lock Order Invariants
//! The `wal` Mutex is the single serialization point for the write path (WAL append + indexer ingest / swap + rotate).
//! Violating this introduces a race condition where data exists in the WAL but gets deleted by a rotation immediately after sealing.
//! The search path never acquires any write locks (leverages ArcSwap snapshots + short-lived ring Mutex acquisitions).
//!
//! ## Calibration (Spec §4.1 — No Dynamic Re-training)
//! Upon startup, novel template vectors are buffered. Once `calibration_templates` are collected,
//! K-means is executed exactly once to freeze the centroids. No dynamic re-training is performed thereafter.

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
    /// Directory where WAL files and chunk directories are stored.
    pub data_dir: PathBuf,
    pub swap_interval_secs: u64,
    /// Hour-based chunk retention limit (default: 7 days).
    pub retention_hours: u64,
    /// Number of recently sealed windows retained in-memory (Search Depth = ring_windows × swap_interval).
    pub ring_windows: usize,
    /// Number of K-Centroids (Tier 1).
    pub centroids: usize,
    pub anomaly_threshold: f32,
    /// Number of unique templates required to fit and freeze centroids.
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
    /// None = normal or pre-calibration.
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
    /// Write path serialization point — see Lock Order Invariants in module docs.
    wal: Mutex<Wal>,
    chunks: ChunkStore,
    /// Ring buffer of recently sealed windows (newest first). Search queries merge this ring.
    ring: Mutex<VecDeque<Arc<IdMapIndex>>>,
    detector: RwLock<Option<AnomalyDetector>>,
    /// Vector buffer of novel templates for calibration (cleared after freezing).
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
    /// Opens the engine. Replays any existing records in the WAL (crash recovery) into the write window.
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

        // Time-based starting ID — monotonically increases to prevent collisions with historical segment IDs upon restart.
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

    /// Ingests a single log line: Parse/Embed → WAL → Index → Anomaly Detection.
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

    /// Searches the ring buffer of recently sealed windows using a semantic text query.
    /// The score in turbovec represents inner-product similarity (higher is closer) — merged in descending order.
    pub fn search_text(&self, query: &str, k: usize) -> Result<Vec<SearchHit>> {
        ensure!(k > 0, "k must be 1 or greater");
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

    /// Invoked periodically by the swap daemon: Seals the active write index → flushes the `.tvim` segment → publishes the new search snapshot → rotates the WAL.
    /// If the active index is empty, it returns early to prevent creating empty segment files during idle periods.
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

    /// Deletes hourly chunk directories that exceed the retention window at the OS level.
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

    /// Buffers novel template vectors and triggers calibration (K-means fitting) once the target limit is reached.
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
