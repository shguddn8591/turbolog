//! TurboLog Engine — Assembly of the entire data pipeline.
//!
//! Ingest → Parse (Drain) → Embed (Cache/ONNX) → WAL → Ping-Pong Indexing → Anomaly Detection
//! 10-second periodic `swap_tick`: Seal window → Backup `.tvim` segment → Publish to Ring → Rotate WAL
//!
//! ## Sharded Write Path
//! Writes are distributed across N independent shards by `id % N`. Each shard owns its own
//! WAL file (`wal-{i}.bin`), PingPongIndexer, and ring buffer, eliminating the single global
//! write lock and allowing 1 M concurrent connections without contention.
//!
//! ## Lock Order Invariants
//! Within each shard the `wal` Mutex is the single serialization point (WAL append + indexer
//! ingest / seal + WAL detach). Locks on different shards are never held simultaneously.
//! The lock only covers in-memory operations and metadata syscalls — `prepare()` and `.tvim`
//! segment writes happen OUTSIDE the lock so a large window seal never stalls ingestion.
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
    /// Number of independent write shards. Defaults to the available parallelism.
    /// Each shard owns its own WAL file, indexer, and ring buffer.
    pub shards: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        let shards = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
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
            shards,
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

/// One independent write shard: owns its own WAL, indexer, and ring buffer.
struct Shard {
    /// Write path serialization point — WAL append + indexer ingest/seal + WAL detach.
    wal: Mutex<Wal>,
    indexer: PingPongIndexer,
    /// Ring buffer of recently sealed windows for this shard (newest first).
    ring: Mutex<VecDeque<Arc<IdMapIndex>>>,
}

pub struct TurboLogEngine {
    cfg: EngineConfig,
    /// Cheap path: Drain parse + LRU lookup. Never held during ONNX inference.
    templates: Mutex<TemplateCache>,
    /// Expensive path: ONNX inference slots, picked round-robin.
    embedders: Vec<Mutex<Embedder>>,
    embed_rr: AtomicUsize,
    shards: Vec<Shard>,
    chunks: ChunkStore,
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
    /// Crash recovery: for each shard replays leftover `wal-{i}-sealed-*.bin` files (windows
    /// sealed but whose segment flush never completed) plus the active WAL, deduplicated by id,
    /// then consolidates everything back into a single fresh active WAL per shard.
    pub fn open(cfg: EngineConfig, embedders: Vec<Embedder>) -> Result<Self> {
        ensure!(!embedders.is_empty(), "At least one embedder is required");
        ensure!(cfg.shards >= 1, "At least one shard is required");
        std::fs::create_dir_all(&cfg.data_dir)?;

        let mut shards = Vec::with_capacity(cfg.shards);
        let mut max_replayed_id = 0u64;
        let mut total_entries = 0usize;

        for i in 0..cfg.shards {
            let wal_path = cfg.data_dir.join(format!("wal-{i}.bin"));
            let sealed_prefix = format!("wal-{i}-sealed-");
            let indexer = PingPongIndexer::new(cfg.dim, cfg.bit_width)?;

            // Gather entries: sealed leftovers (oldest first) + active WAL, dedup by id.
            let leftovers = Wal::sealed_leftovers(&cfg.data_dir, &sealed_prefix)?;
            let mut entries: Vec<(u64, Vec<f32>)> = Vec::new();
            for file in &leftovers {
                entries.extend(Wal::replay(file, cfg.dim)?);
            }
            entries.extend(Wal::replay(&wal_path, cfg.dim)?);
            let mut seen = HashSet::with_capacity(entries.len());
            entries.retain(|(id, _)| seen.insert(*id));

            for (id, vector) in &entries {
                indexer.ingest(*id, vector)?;
                max_replayed_id = max_replayed_id.max(*id);
            }
            total_entries += entries.len();

            // Consolidate when leftovers exist: atomic tmp→rename, then drop leftover files.
            if !leftovers.is_empty() {
                let tmp_path = cfg.data_dir.join(format!("wal-{i}.bin.tmp"));
                std::fs::remove_file(&tmp_path).ok();
                {
                    let mut tmp = Wal::open(&tmp_path, cfg.dim)?;
                    for (id, vector) in &entries {
                        tmp.append(*id, vector)?;
                    }
                }
                std::fs::rename(&tmp_path, &wal_path)
                    .with_context(|| format!("WAL consolidation failed for shard {i}"))?;
                for file in &leftovers {
                    std::fs::remove_file(file).ok();
                }
            }

            let wal = Mutex::new(Wal::open(&wal_path, cfg.dim)?);
            shards.push(Shard {
                wal,
                indexer,
                ring: Mutex::new(VecDeque::new()),
            });
        }

        // Time-based starting ID — monotonically increases to prevent collisions with historical
        // segment IDs upon restart.
        let next_id = ((now_millis() as u64) << 20).max(max_replayed_id + 1);

        Ok(Self {
            chunks: ChunkStore::new(cfg.data_dir.join("chunks"))?,
            templates: Mutex::new(TemplateCache::new()),
            embedders: embedders.into_iter().map(Mutex::new).collect(),
            embed_rr: AtomicUsize::new(0),
            shards,
            detector: RwLock::new(None),
            calibration: Mutex::new(Vec::new()),
            next_id: AtomicU64::new(next_id),
            ingested_total: AtomicU64::new(total_entries as u64),
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
        let t0 = std::time::Instant::now();

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
        let shard_idx = (id as usize) % self.shards.len();
        let shard = &self.shards[shard_idx];

        {
            let mut wal = shard.wal.lock().unwrap();
            wal.append(id, &vector)?;
            shard.indexer.ingest(id, &vector)?;
        }
        self.ingested_total.fetch_add(1, Ordering::Relaxed);
        crate::metrics::inc_ingested(1);

        let anomaly = {
            let detector = self.detector.read().unwrap();
            detector.as_ref().and_then(|d| {
                match d.detect(&vector, &shard.indexer.get_search_index()) {
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

        if anomaly.is_some() {
            crate::metrics::inc_anomaly();
        }

        crate::metrics::observe_ingest_seconds(t0.elapsed().as_secs_f64());

        Ok(LogReport {
            id,
            template_id: parsed.template_id,
            timestamp: parsed.timestamp,
            anomaly,
        })
    }

    /// Searches the ring buffers of all shards using a semantic text query.
    /// Results from all shards are merged, sorted descending by score, and truncated to k.
    pub fn search_text(&self, query: &str, k: usize) -> Result<Vec<SearchHit>> {
        ensure!(k > 0, "k must be 1 or greater");
        let t0 = std::time::Instant::now();
        let vector = self.with_embedder(|e| e.embed(query))?;

        let mut hits: Vec<SearchHit> = Vec::new();
        for shard in &self.shards {
            let windows: Vec<Arc<IdMapIndex>> =
                shard.ring.lock().unwrap().iter().cloned().collect();
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
        }
        hits.sort_by(|a, b| b.score.total_cmp(&a.score));
        hits.truncate(k);

        crate::metrics::observe_search_seconds(t0.elapsed().as_secs_f64());
        Ok(hits)
    }

    /// Invoked periodically by the swap daemon (single-threaded caller assumed).
    ///
    /// Iterates all shards in order. For each non-empty shard: under the write-path lock,
    /// seals the indexer window and detaches the WAL (metadata-only, microseconds). Then
    /// outside the lock, runs `prepare()` and flushes the `.tvim` segment to disk, deletes
    /// the sealed WAL, and publishes the new snapshot to the ring buffer.
    ///
    /// Returns true if any shard performed a swap.
    pub fn swap_tick(&self) -> Result<bool> {
        let mut any_swapped = false;

        for (i, shard) in self.shards.iter().enumerate() {
            let (sealed, sealed_wal) = {
                let mut wal = shard.wal.lock().unwrap();
                if shard.indexer.pending_len() == 0 {
                    continue;
                }
                let sealed = shard.indexer.seal()?;
                let sealed_wal = wal.detach_sealed()?;
                (sealed, sealed_wal)
            };

            // Outside the lock: pre-build search caches, flush the segment, then publish.
            sealed.prepare();
            let segment = self.chunks.segment_path(now_millis() + i as i64)?;
            sealed
                .write(&segment)
                .with_context(|| format!("Failed to backup chunk to {} (shard {i})", segment.display()))?;
            std::fs::remove_file(&sealed_wal).ok();

            let sealed = Arc::new(sealed);
            shard.indexer.publish(Arc::clone(&sealed));
            let mut ring = shard.ring.lock().unwrap();
            ring.push_front(sealed);
            ring.truncate(self.cfg.ring_windows.max(1));

            any_swapped = true;
        }

        Ok(any_swapped)
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

        let mut pending_window_len = 0usize;
        let mut ring_windows = 0usize;
        let mut ring_vectors = 0usize;
        for shard in &self.shards {
            pending_window_len += shard.indexer.pending_len();
            let ring = shard.ring.lock().unwrap();
            ring_windows += ring.len();
            ring_vectors += ring.iter().map(|w| w.len()).sum::<usize>();
        }

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
            pending_window_len,
            ring_windows,
            ring_vectors,
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
