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
    frozen_centroids: Vec<Vec<f32>>,
    anomaly_threshold: f32,
}

fn euclidean(a: &[f32], b: &[f32]) -> f32 {
    a.iter()
        .zip(b)
        .map(|(x, y)| (x - y) * (x - y))
        .sum::<f32>()
        .sqrt()
}

impl AnomalyDetector {
    pub fn new(frozen_centroids: Vec<Vec<f32>>, anomaly_threshold: f32) -> Self {
        assert!(!frozen_centroids.is_empty(), "At least one centroid is required");
        Self {
            frozen_centroids,
            anomaly_threshold,
        }
    }

    /// Fits normal clustering centroids via K-means (Lloyd, 16 iterations, deterministic initialization) at startup and freezes them.
    /// `normal_vectors` is a flat array of size n×dim.
    pub fn fit(normal_vectors: &[f32], dim: usize, k: usize, anomaly_threshold: f32) -> Self {
        assert!(dim > 0 && !normal_vectors.is_empty());
        assert!(normal_vectors.len() % dim == 0, "Input vectors must form a flat n×dim array");
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
                    let d = euclidean(row(i), centroid);
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

    /// Tier 1 operation: Euclidean distance to the nearest frozen centroid. O(k·dim).
    /// Used both for filtering and threshold calibration (e.g. p99 of normal distance).
    pub fn min_distance(&self, vector: &[f32]) -> f32 {
        self.frozen_centroids
            .iter()
            .map(|c| euclidean(c, vector))
            .fold(f32::INFINITY, f32::min)
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
}
