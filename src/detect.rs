//! Anomaly Detection Layer — K-Centroid based 2-tier fast screening filter.
//!
//! - **Tier 1**: Euclidean distance to frozen centroids — O(k·dim), bypassing exhaustive searches.
//!   Normal patterns return immediately.
//! - **Tier 2**: Vectors exceeding the anomaly threshold are forwarded to the turbovec `IdMapIndex` deep search
//!   to retrieve similar incident IDs (allowing subset limiting via an allowlist, e.g., restrict to the same server).
//!
//! System Constraint (Spec v1.0 §4.1 — No Dynamic Re-training):
//! Centroids are calibrated once at startup (`fit`) and frozen for the engine's lifetime.
//! Real-time incremental re-training based on incoming streams is strictly prohibited.

use turbovec::IdMapIndex;

/// Number of similar incident context IDs to retrieve in Tier 2 deep searches.
pub const TIER2_K: usize = 5;

pub enum DetectionResult {
    Normal,
    Anomaly {
        /// Euclidean distance to the nearest normal centroid.
        score: f32,
        /// External IDs of similar incidents retrieved from recent windows.
        nearest_incidents: Vec<u64>,
    },
}

pub struct AnomalyDetector {
    /// Frozen centroids representing normal log distribution, protecting against drift.
    /// Stored as a flat `k × dim` buffer (contiguous in memory) so the per-line
    /// nearest-centroid scan streams linearly instead of chasing `k` heap pointers.
    centroids: Vec<f32>,
    dim: usize,
    anomaly_threshold: f32,
}

/// Number of robust standard deviations (MAD-scaled) above the median a sample must sit
/// to be flagged anomalous in [`AnomalyDetector::fit_auto`].
const AUTO_THRESHOLD_Z: f32 = 3.0;

/// `1 / Phi^-1(0.75)` — scales MAD to be a consistent estimator of the standard deviation
/// for normally-distributed data.
const MAD_TO_SIGMA: f32 = 1.4826;

/// Squared Euclidean distance. The hot path only needs ordering (argmin) and a single
/// threshold compare, both of which are monotonic in the square — so we defer the one
/// `sqrt` to the caller instead of paying it per centroid.
fn euclidean_sq(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>()
}

/// Sorts `values` in place and returns the median (average of the two middle elements
/// for even-length input). Used for the robust threshold in [`AnomalyDetector::fit_auto`].
fn median_of(values: &mut [f32]) -> f32 {
    values.sort_by(f32::total_cmp);
    let n = values.len();
    if n.is_multiple_of(2) {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    } else {
        values[n / 2]
    }
}

impl AnomalyDetector {
    pub fn new(frozen_centroids: Vec<Vec<f32>>, anomaly_threshold: f32) -> Self {
        assert!(
            !frozen_centroids.is_empty(),
            "At least one centroid is required"
        );
        let dim = frozen_centroids[0].len();
        assert!(dim > 0, "Centroids must be non-empty");
        assert!(
            frozen_centroids.iter().all(|c| c.len() == dim),
            "All centroids must share the same dimension"
        );
        let centroids = frozen_centroids.concat();
        Self {
            centroids,
            dim,
            anomaly_threshold,
        }
    }

    /// Fits normal clustering centroids via K-means (Lloyd, 16 iterations, deterministic initialization) at startup and freezes them.
    /// `normal_vectors` is a flat array of size n×dim.
    pub fn fit(normal_vectors: &[f32], dim: usize, k: usize, anomaly_threshold: f32) -> Self {
        assert!(dim > 0 && !normal_vectors.is_empty());
        assert!(
            normal_vectors.len().is_multiple_of(dim),
            "Input vectors must form a flat n×dim array"
        );
        let n = normal_vectors.len() / dim;
        let k = k.clamp(1, n);
        let row = |i: usize| &normal_vectors[i * dim..(i + 1) * dim];

        // Deterministic initialization: uniform spacing samples
        let mut centroids: Vec<Vec<f32>> = (0..k).map(|c| row(c * n / k).to_vec()).collect();
        let mut assignment = vec![0usize; n];
        for _ in 0..16 {
            for (i, assign) in assignment.iter_mut().enumerate().take(n) {
                let mut best = (f32::INFINITY, 0usize);
                for (c, centroid) in centroids.iter().enumerate() {
                    let d = euclidean_sq(row(i), centroid);
                    if d < best.0 {
                        best = (d, c);
                    }
                }
                *assign = best.1;
            }
            let mut sums = vec![vec![0f32; dim]; k];
            let mut counts = vec![0usize; k];
            for i in 0..n {
                counts[assignment[i]] += 1;
                for (s, v) in sums[assignment[i]].iter_mut().zip(row(i)) {
                    *s += v;
                }
            }
            for c in 0..k {
                if counts[c] > 0 {
                    for s in sums[c].iter_mut() {
                        *s /= counts[c] as f32;
                    }
                    centroids[c] = sums[c].clone();
                }
                // Empty clusters retain their previous centroids.
            }
        }
        Self::new(centroids, anomaly_threshold)
    }

    /// Like [`Self::fit`], but derives the anomaly threshold from the calibration data
    /// itself via a robust (median + MAD) estimator rather than a percentile of the same
    /// sample set.
    ///
    /// A naive p99-of-calibration-distances threshold is train-on-test: it sits above
    /// ~99% of the very data it was fit on by construction, so outliers already present
    /// in the calibration set (and most future ones) fall below it and never get flagged.
    /// Median and MAD are robust statistics — a handful of far-away points barely moves
    /// either — so `threshold = median + Z * robust_sigma` stays low enough that those
    /// same outliers land above it.
    ///
    /// `floor` is the minimum threshold — it dominates when the calibration samples are
    /// tightly clustered (median ≈ mad ≈ 0), preventing a degenerate zero threshold.
    pub fn fit_auto(normal_vectors: &[f32], dim: usize, k: usize, floor: f32) -> Self {
        let mut detector = Self::fit(normal_vectors, dim, k, floor);
        let n = normal_vectors.len() / dim;
        let mut distances: Vec<f32> = (0..n)
            .map(|i| detector.min_distance(&normal_vectors[i * dim..(i + 1) * dim]))
            .collect();
        let median = median_of(&mut distances);
        let mut abs_dev: Vec<f32> = distances.iter().map(|d| (d - median).abs()).collect();
        let mad = median_of(&mut abs_dev);
        let robust_sigma = MAD_TO_SIGMA * mad;
        detector.anomaly_threshold = (median + AUTO_THRESHOLD_Z * robust_sigma).max(floor);
        detector
    }

    /// The effective anomaly threshold (Euclidean distance on the unit sphere).
    pub fn threshold(&self) -> f32 {
        self.anomaly_threshold
    }

    /// Tier 1 operation: Euclidean distance to the nearest frozen centroid. O(k·dim).
    /// Used both for filtering and threshold calibration (e.g. p99 of normal distance).
    pub fn min_distance(&self, vector: &[f32]) -> f32 {
        // euclidean_sq zips, so a wrong-length vector would silently score over a prefix
        // rather than erroring. Always upheld by construction (single 384-d embedder), so
        // a debug-only check keeps the per-line hot path free in release.
        debug_assert_eq!(
            vector.len(),
            self.dim,
            "query vector dim {} != centroid dim {}",
            vector.len(),
            self.dim
        );
        self.centroids
            .chunks_exact(self.dim)
            .map(|c| euclidean_sq(c, vector))
            .fold(f32::INFINITY, f32::min)
            .sqrt()
    }

    /// Classifies an incoming vector in O(k) complexity. Falls back to deep search in search_index on threshold breach.
    pub fn detect(&self, vector: &[f32], search_index: &IdMapIndex) -> DetectionResult {
        self.detect_filtered(vector, search_index, None)
    }

    /// Constrains the Tier 2 deep search to a subset of external u64 IDs (e.g. logs from the same host).
    pub fn detect_filtered(
        &self,
        vector: &[f32],
        search_index: &IdMapIndex,
        allowlist: Option<&[u64]>,
    ) -> DetectionResult {
        let score = self.min_distance(vector);
        if score <= self.anomaly_threshold {
            return DetectionResult::Normal;
        }
        DetectionResult::Anomaly {
            score,
            nearest_incidents: tier2_context(vector, search_index, allowlist),
        }
    }
}

/// Tier 2: Retrieves similar incident context IDs from the recently sealed index window.
fn tier2_context(vector: &[f32], index: &IdMapIndex, allowlist: Option<&[u64]>) -> Vec<u64> {
    if index.is_empty() || index.dim() != vector.len() {
        return Vec::new();
    }
    match allowlist {
        Some(ids) => {
            // search_with_allowlist panics on empty or non-existent IDs — pre-filtering is mandatory.
            let present: Vec<u64> = ids
                .iter()
                .copied()
                .filter(|&id| index.contains(id))
                .collect();
            if present.is_empty() {
                return Vec::new();
            }
            index
                .search_with_allowlist(vector, TIER2_K, Some(&present))
                .1
        }
        None => index.search(vector, TIER2_K).1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_two_clusters_and_min_distance() {
        // Two clusters centered around unit vectors e1 and e2 (dim=4)
        let mut data = Vec::new();
        for j in 0..10 {
            let eps = j as f32 * 0.01;
            data.extend_from_slice(&[1.0, eps, 0.0, 0.0]);
            data.extend_from_slice(&[0.0, 0.0, 1.0, eps]);
        }
        let det = AnomalyDetector::fit(&data, 4, 2, 0.5);
        assert!(
            det.min_distance(&[1.0, 0.0, 0.0, 0.0]) < 0.2,
            "Cluster centers should be close to their respective members"
        );
        assert!(
            det.min_distance(&[-1.0, 0.0, 0.0, 0.0]) > 1.0,
            "Opposite directions should yield large distances"
        );
    }

    #[test]
    fn fit_auto_derives_threshold_from_spread() {
        // Cluster with intra-cluster spread: median + Z*robust_sigma should exceed the floor.
        let mut data = Vec::new();
        for j in 0..50 {
            let eps = j as f32 * 0.01; // up to 0.49 away from e1 in one coordinate
            data.extend_from_slice(&[1.0, eps, 0.0, 0.0]);
        }
        let det = AnomalyDetector::fit_auto(&data, 4, 1, 0.05);
        assert!(
            det.threshold() > 0.05,
            "robust threshold should exceed the floor given real spread: {}",
            det.threshold()
        );
        // A new sample within the observed spread stays normal under min_distance.
        assert!(det.min_distance(&[1.0, 0.2, 0.0, 0.0]) <= det.threshold());

        // Degenerate case: identical samples → median = mad = 0 → floor dominates.
        let tight: Vec<f32> = [1.0f32, 0.0, 0.0, 0.0].repeat(20);
        let det = AnomalyDetector::fit_auto(&tight, 4, 1, 0.5);
        assert_eq!(
            det.threshold(),
            0.5,
            "floor must dominate when median/mad ≈ 0"
        );
    }

    #[test]
    fn fit_auto_flags_in_sample_outliers() {
        // Regression for the train-on-test bug: a tight cluster plus a couple of
        // far-away points, calibrated together. A naive p99*1.25 threshold would sit
        // above the outliers themselves (since they're part of the same sample), so
        // they'd never be flagged. The robust median/MAD threshold should not be
        // dragged up by them, leaving the outliers above `threshold()`.
        let mut data = Vec::new();
        for j in 0..30 {
            let eps = j as f32 * 0.001; // tight cluster around e1
            data.extend_from_slice(&[1.0, eps, 0.0, 0.0]);
        }
        let outliers: [[f32; 4]; 2] = [[1.0, 5.0, 0.0, 0.0], [1.0, -5.0, 0.0, 0.0]];
        for o in &outliers {
            data.extend_from_slice(o);
        }
        let det = AnomalyDetector::fit_auto(&data, 4, 1, 0.05);
        for o in &outliers {
            let score = det.min_distance(o);
            assert!(
                score > det.threshold(),
                "injected outlier should score above threshold ({} <= {})",
                score,
                det.threshold()
            );
        }
    }
}
