//! TurboLog Engine — Assembly of the entire data pipeline.
//!
//! Ingest → Parse (Drain) → Embed (Cache/ONNX) → WAL → Ping-Pong Indexing → Anomaly Detection
//! 10-second periodic `swap_tick`: Seal window → Backup `.tvim` segment → Publish to Ring → Rotate WAL
//!
//! ## Lock Order Invariants
//! The `wal` Mutex is the single serialization point for the write path (WAL append + indexer
//! ingest / seal + WAL detach). Violating this introduces a race condition where data exists in
//! the WAL of the wrong window. Crucially, the lock only ever covers in-memory operations and
//! metadata syscalls (append/flush, rename) — `prepare()` and `.tvim` segment writes happen
//! OUTSIDE the lock so a large window seal never stalls ingestion.
//!
//! ## Embedder Pool
//! The template cache (parse + LRU lookup, microseconds) and the embedders (ONNX inference,
//! milliseconds) are guarded separately. A cache-miss embeds outside the cache lock on a
//! round-robin embedder slot, so a novel-template storm degrades only the miss path — the
//! cache-hit ingest path keeps running at full speed. Two threads missing the same template
//! concurrently may both embed it (identical result, last insert wins) — harmless by design.
//!
//! ## Calibration (Spec §4.1 — No Dynamic Re-training)
//! Upon startup, novel template vectors are buffered. Once `calibration_templates` are collected,
//! K-means is executed exactly once to freeze the centroids. No dynamic re-training is performed
//! thereafter.

use std::collections::HashSet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{ensure, Context, Result};
use serde::Serialize;
use turbovec::IdMapIndex;

use crate::chunks::ChunkStore;
use crate::detect::{AnomalyDetector, DetectionResult};
use crate::index::PingPongIndexer;
use crate::ingest::{Embedder, TemplateCache};
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
    /// FLOOR (minimum) anomaly threshold. The effective threshold is derived at
    /// calibration time as max(floor, p99 of calibration distances × 1.25) — see
    /// `AnomalyDetector::fit_auto`. The floor dominates for tightly clustered templates.
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
    /// Effective anomaly threshold once calibrated (max(floor, p99 × 1.25)).
    pub detector_threshold: Option<f32>,
    pub ingested_total: u64,
}

pub struct TurboLogEngine {
    cfg: EngineConfig,
    /// Cheap path: Drain parse + LRU lookup. Never held during ONNX inference.
    templates: Mutex<TemplateCache>,
    /// Expensive path: ONNX inference slots, picked round-robin.
    embedders: Vec<Mutex<Embedder>>,
    embed_rr: AtomicUsize,
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
    /// Opens the engine with a pool of embedders (>= 1). Pool size bounds how many cache-miss
    /// embeddings can run in parallel; each embedder holds its own ONNX session (~90 MB).
    ///
    /// Crash recovery: replays leftover `wal-sealed-*.bin` files (windows sealed but whose
    /// segment flush never completed) plus the active WAL, deduplicated by id, then
    /// consolidates everything back into a single fresh active WAL.
    pub fn open(cfg: EngineConfig, embedders: Vec<Embedder>) -> Result<Self> {
        ensure!(!embedders.is_empty(), "At least one embedder is required");
        std::fs::create_dir_all(&cfg.data_dir)?;
        let wal_path = cfg.data_dir.join("wal.bin");
        let indexer = PingPongIndexer::new(cfg.dim, cfg.bit_width)?;

        // Gather entries: sealed leftovers (oldest first) + active WAL. A crash between the
        // WAL consolidation rename and leftover deletion can leave duplicates — dedupe by id.
        let leftovers = Wal::sealed_leftovers(&cfg.data_dir)?;
        let mut entries: Vec<(u64, Vec<f32>)> = Vec::new();
        for file in &leftovers {
            entries.extend(Wal::replay(file, cfg.dim)?);
        }
        entries.extend(Wal::replay(&wal_path, cfg.dim)?);
        let mut seen = HashSet::with_capacity(entries.len());
        entries.retain(|(id, _)| seen.insert(*id));

        let mut max_replayed_id = 0u64;
        for (id, vector) in &entries {
            indexer.ingest(*id, vector)?;
            max_replayed_id = max_replayed_id.max(*id);
        }

        // Consolidate: rewrite all recovered entries into one fresh active WAL (atomic
        // replace), then drop the leftover files. Only needed when leftovers exist.
        if !leftovers.is_empty() {
            let tmp_path = cfg.data_dir.join("wal.bin.tmp");
            std::fs::remove_file(&tmp_path).ok();
            {
                let mut tmp = Wal::open(&tmp_path, cfg.dim)?;
                for (id, vector) in &entries {
                    tmp.append(*id, vector)?;
                }
            }
            std::fs::rename(&tmp_path, &wal_path).context("WAL consolidation failed")?;
            for file in &leftovers {
                std::fs::remove_file(file).ok();
            }
        }

        // Time-based starting ID — monotonically increases to prevent collisions with historical segment IDs upon restart.
        let next_id = ((now_millis() as u64) << 20).max(max_replayed_id + 1);

        Ok(Self {
            chunks: ChunkStore::new(cfg.data_dir.join("chunks"))?,
            wal: Mutex::new(Wal::open(&wal_path, cfg.dim)?),
            indexer,
            templates: Mutex::new(TemplateCache::new()),
            embedders: embedders.into_iter().map(Mutex::new).collect(),
            embed_rr: AtomicUsize::new(0),
            ring: Mutex::new(VecDeque::new()),
            detector: RwLock::new(None),
            calibration: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(next_id),
            ingested_total: AtomicU64::new(entries.len() as u64),
            cfg,
        })
    }

    pub fn config(&self) -> &EngineConfig {
        &self.cfg
    }

    /// Runs `f` on a round-robin embedder slot. Blocks only on that slot, never on the
    /// template cache — concurrent cache-hit ingests proceed unimpeded.
    fn with_embedder<T>(&self, f: impl FnOnce(&mut Embedder) -> Result<T>) -> Result<T> {
        let slot = self.embed_rr.fetch_add(1, Ordering::Relaxed) % self.embedders.len();
        let mut embedder = self.embedders[slot].lock().unwrap();
        f(&mut embedder)
    }

    /// Ingests a single log line: Parse/Embed → WAL → Index → Anomaly Detection.
    pub fn ingest_log(&self, line: &str) -> Result<LogReport> {
        // Cheap path under the cache lock: parse + lookup only.
        let (parsed, cached) = self.templates.lock().unwrap().parse_and_lookup(line);

        let (vector, new_template) = match cached {
            Some(vector) => (vector, false),
            None => {
                // Expensive path OUTSIDE the cache lock: a miss storm serializes here
                // (per pool slot) while cache hits keep flowing.
                let vector: Arc<[f32]> =
                    self.with_embedder(|e| e.embed(&parsed.template))?.into();
                self.templates
                    .lock()
                    .unwrap()
                    .insert(parsed.template_id, Arc::clone(&vector));
                (vector, true)
            }
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
        let vector = self.with_embedder(|e| e.embed(query))?;
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

    /// Invoked periodically by the swap daemon (single-threaded caller assumed).
    ///
    /// Under the write-path lock only the seal (`mem::replace`) and the WAL detach (rename)
    /// happen — microsecond-scale metadata operations. The expensive `prepare()` and `.tvim`
    /// segment flush run outside the lock, so ingestion never stalls behind disk I/O.
    /// The detached sealed WAL is deleted only after the segment is durably written; if the
    /// process crashes in between, startup recovery replays it.
    pub fn swap_tick(&self) -> Result<bool> {
        let (sealed, sealed_wal) = {
            let mut wal = self.wal.lock().unwrap();
            if self.indexer.pending_len() == 0 {
                return Ok(false);
            }
            let sealed = self.indexer.seal()?;
            let sealed_wal = wal.detach_sealed()?;
            (sealed, sealed_wal)
        };

        // Outside the lock: pre-build search caches, flush the segment, then publish.
        sealed.prepare();
        let segment = self.chunks.segment_path(now_millis())?;
        sealed
            .write(&segment)
            .with_context(|| format!("Failed to backup chunk to {}", segment.display()))?;
        std::fs::remove_file(&sealed_wal).ok();

        let sealed = Arc::new(sealed);
        self.indexer.publish(Arc::clone(&sealed));
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
            let templates = self.templates.lock().unwrap();
            (templates.hits(), templates.misses(), templates.hit_rate())
        };
        let ring = self.ring.lock().unwrap();
        let detector_threshold = self
            .detector
            .read()
            .unwrap()
            .as_ref()
            .map(|d| d.threshold());
        Stats {
            cache_hits,
            cache_misses,
            cache_hit_rate,
            pending_window_len: self.indexer.pending_len(),
            ring_windows: ring.len(),
            ring_vectors: ring.iter().map(|w| w.len()).sum(),
            detector_calibrated: detector_threshold.is_some(),
            detector_threshold,
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
        let detector = AnomalyDetector::fit_auto(
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
