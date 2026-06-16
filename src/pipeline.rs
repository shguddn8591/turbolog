//! Standalone single-threaded pipeline for `watch` and `scan` subcommands.
//!
//! Reuses the same Drain + VectorCache + AnomalyDetector stack as the HTTP server
//! but without WAL, indexing, or concurrency — suitable for piped stdin processing.

use anyhow::Result;

use crate::detect::AnomalyDetector;
use crate::ingest::{Embedder, VectorCache};

/// Calibration: collect this many distinct template vectors before freezing centroids.
const CALIBRATION_TEMPLATES: usize = 64;
/// K-means cluster count for calibration.
const CENTROID_K: usize = 8;
/// Minimum anomaly threshold floor.
const THRESHOLD_FLOOR: f32 = 0.10;
/// Embedding dimension for all-MiniLM-L6-v2.
const DIM: usize = 384;

pub struct LineResult {
    pub template: String,
    pub score: Option<f32>,
    pub is_anomaly: bool,
}

pub struct LocalPipeline {
    cache: VectorCache,
    detector: Option<AnomalyDetector>,
    /// Flat buffer of calibration vectors (n × DIM).
    calibration_buf: Vec<f32>,
    /// Number of distinct templates seen so far (caps at CALIBRATION_TEMPLATES).
    calibration_count: usize,
    /// User-supplied threshold override from `--threshold`; `None` uses auto-calibrated value.
    threshold_override: Option<f32>,
    seen_templates: std::collections::HashSet<String>,
}

impl LocalPipeline {
    pub fn new(embedder: Embedder, threshold_override: Option<f32>) -> Self {
        Self {
            cache: VectorCache::new(embedder),
            detector: None,
            calibration_buf: Vec::new(),
            calibration_count: 0,
            threshold_override,
            seen_templates: std::collections::HashSet::new(),
        }
    }

    /// Processes a single log line. Returns a `LineResult` describing whether it is anomalous.
    /// Before calibration completes (first 64 novel templates), `score` is `None`.
    pub fn process(&mut self, line: &str) -> Result<LineResult> {
        let (parsed, vector) = self.cache.get_or_embed(line)?;

        // Calibration phase: accumulate novel template vectors.
        if self.detector.is_none() {
            if self.seen_templates.insert(parsed.template.clone()) {
                if self.calibration_count < CALIBRATION_TEMPLATES {
                    self.calibration_buf.extend_from_slice(&vector);
                    self.calibration_count += 1;
                }
            }
            if self.calibration_count >= CALIBRATION_TEMPLATES {
                let k = CENTROID_K.min(self.calibration_count);
                self.detector = Some(AnomalyDetector::fit_auto(
                    &self.calibration_buf,
                    DIM,
                    k,
                    THRESHOLD_FLOOR,
                ));
            }
            return Ok(LineResult {
                template: parsed.template,
                score: None,
                is_anomaly: false,
            });
        }

        let detector = self.detector.as_ref().unwrap();
        let score = detector.min_distance(&vector);
        let threshold = self
            .threshold_override
            .unwrap_or_else(|| detector.threshold());
        let is_anomaly = score > threshold;

        Ok(LineResult {
            template: parsed.template,
            score: Some(score),
            is_anomaly,
        })
    }

    pub fn calibrated(&self) -> bool {
        self.detector.is_some()
    }

    pub fn calibration_progress(&self) -> usize {
        self.calibration_count
    }
}
