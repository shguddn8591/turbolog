//! Standalone single-threaded pipeline for `watch` and `scan` subcommands.
//!
//! Reuses the same Drain + VectorCache + AnomalyDetector stack as the HTTP server
//! but without WAL, indexing, or concurrency — suitable for piped stdin processing.

use anyhow::Result;

use crate::detect::AnomalyDetector;
use crate::ingest::{Embedder, VectorCache};

/// Calibration: collect this many distinct template vectors before freezing centroids.
const CALIBRATION_TEMPLATES: usize = 64;
/// Floor: the fewest distinct templates `scan`'s EOF finalize will calibrate on.
/// `tests/cli.rs::scan_calibrates_below_64_templates` and scan.rs's "not enough data"
/// message both assume this value — keep them in sync if it changes.
const MIN_CALIBRATION_TEMPLATES: usize = 8;
/// Floor for the streaming line-budget path (`watch`) — deliberately lower than
/// `MIN_CALIBRATION_TEMPLATES`. A live stream can be low-cardinality forever (e.g. 3
/// repeating templates), and 500 lines is already plenty of evidence for a handful of
/// templates; requiring 8 distinct ones meant such a stream never calibrated at all.
/// ponytail: fewer templates -> coarser centroids (k shrinks with them too, see
/// `CENTROID_K.min(calibration_count)`), but the robust median/MAD threshold (detect.rs)
/// keeps that workable instead of relying on a wide percentile margin.
const MIN_CALIBRATION_TEMPLATES_BUDGET: usize = 3;
/// After this many lines, calibrate on whatever templates we have (>= the budget floor
/// above) instead of waiting for the full 64 — so a low-cardinality live stream still
/// turns detection on.
const CALIBRATION_LINE_BUDGET: usize = 500;
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
    threshold_override: Option<f32>,
    seen_templates: std::collections::HashSet<String>,
    /// Lines processed while still uncalibrated — drives the line-budget fallback.
    lines_seen: usize,
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
            lines_seen: 0,
        }
    }

    /// Processes a single log line. Returns a `LineResult` describing whether it is anomalous.
    /// Before calibration completes (first 64 novel templates), `score` is `None`.
    pub fn process(&mut self, line: &str) -> Result<LineResult> {
        let (parsed, vector) = self.cache.get_or_embed(line)?;

        // Calibration phase: accumulate novel template vectors.
        if self.detector.is_none() {
            self.lines_seen += 1;
            if self.seen_templates.insert(parsed.template.clone())
                && self.calibration_count < CALIBRATION_TEMPLATES
            {
                self.calibration_buf.extend_from_slice(&vector);
                self.calibration_count += 1;
            }
            // Fire when we have the full sample, OR the line budget is spent and we have
            // at least the (lower) budget-path minimum — a low-cardinality stream still
            // gets a detector instead of running uncalibrated forever.
            let budget_reached = self.lines_seen >= CALIBRATION_LINE_BUDGET
                && self.calibration_count >= MIN_CALIBRATION_TEMPLATES_BUDGET;
            if self.calibration_count >= CALIBRATION_TEMPLATES || budget_reached {
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

    /// Force calibration at end-of-input (batch mode) when the streaming triggers never
    /// fired. Fits on whatever templates were collected if at least
    /// `MIN_CALIBRATION_TEMPLATES` are present. Returns whether the detector is now calibrated.
    /// After this, earlier lines (which returned `score: None`) can be re-scored by re-calling
    /// `process` — once the detector exists, `process` is a pure scorer.
    pub fn finalize(&mut self) -> bool {
        if self.detector.is_none() && self.calibration_count >= MIN_CALIBRATION_TEMPLATES {
            let k = CENTROID_K.min(self.calibration_count);
            self.detector = Some(AnomalyDetector::fit_auto(
                &self.calibration_buf,
                DIM,
                k,
                THRESHOLD_FLOOR,
            ));
        }
        self.calibrated()
    }

    /// Re-scores an already-known template (e.g. from a prior `process` call's
    /// `LineResult.template`) without re-running Drain — used by `scan`'s EOF re-score
    /// loop so lines seen before calibration aren't re-fed into the Drain tree a second
    /// time. Requires the detector to already be calibrated; returns `Ok(None)` otherwise.
    pub fn rescore(&mut self, template: &str) -> Result<Option<LineResult>> {
        let Some(detector) = self.detector.as_ref() else {
            return Ok(None);
        };
        let vector = self.cache.vector_for_template(template)?;
        let score = detector.min_distance(&vector);
        let threshold = self
            .threshold_override
            .unwrap_or_else(|| detector.threshold());
        Ok(Some(LineResult {
            template: template.to_string(),
            score: Some(score),
            is_anomaly: score > threshold,
        }))
    }

    pub fn calibrated(&self) -> bool {
        self.detector.is_some()
    }

    pub fn calibration_progress(&self) -> usize {
        self.calibration_count
    }
}
