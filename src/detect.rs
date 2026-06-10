//! Anomaly Detection Layer — K-Centroid 기반 2-tier 고속 스크리닝 필터.
//!
//! - **Tier 1**: 고정 centroid들과의 유클리드 거리 — O(k·dim), 전수 조사 회피.
//!   정상 범주면 즉시 통과.
//! - **Tier 2**: 임계치 초과 벡터만 turbovec `IdMapIndex` 심층 검색으로 넘겨,
//!   유사 사건 ID(allowlist로 동일 서버 등 부분집합 제한 가능)를 확보한다.
//!
//! 시스템 제약 (스펙 v1.0 §4.1 — No Dynamic Re-training):
//! centroid는 시동 시 1회 캘리브레이션(`fit`) 후 전체 수명 주기 동안 고정된다.
//! 들어오는 데이터에 맞춰 실시간으로 재학습하는 로직은 절대 구현하지 않는다.

use turbovec::IdMapIndex;

/// Tier 2 심층 검색에서 확보할 유사 사건 수.
pub const TIER2_K: usize = 5;

pub enum DetectionResult {
    Normal,
    Anomaly {
        /// 가장 가까운 정상 centroid까지의 유클리드 거리.
        score: f32,
        /// 최근 윈도우에서 검색된 유사 사건의 외부 ID들.
        nearest_incidents: Vec<u64>,
    },
}

pub struct AnomalyDetector {
    /// 오염을 막기 위해 런타임에 변경되지 않는 고정 중심점들.
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
        assert!(!frozen_centroids.is_empty(), "centroid는 최소 1개 필요");
        Self {
            frozen_centroids,
            anomaly_threshold,
        }
    }

    /// 시동 시 1회 K-means(Lloyd, 16회 반복, 결정적 초기화) 캘리브레이션으로
    /// 정상 군집 중심점을 학습한 뒤 동결한다. `normal_vectors`는 n×dim 평탄 배열.
    pub fn fit(normal_vectors: &[f32], dim: usize, k: usize, anomaly_threshold: f32) -> Self {
        assert!(dim > 0 && !normal_vectors.is_empty());
        assert!(normal_vectors.len().is_multiple_of(dim), "n×dim 평탄 배열이어야 함");
        let n = normal_vectors.len() / dim;
        let k = k.clamp(1, n);
        let row = |i: usize| &normal_vectors[i * dim..(i + 1) * dim];

        // 결정적 초기화: 균등 간격 샘플
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
                // 빈 클러스터는 기존 centroid 유지
            }
        }
        Self::new(centroids, anomaly_threshold)
    }

    /// Tier 1 원시 연산: 가장 가까운 고정 centroid까지의 유클리드 거리. O(k·dim).
    /// 임계치 캘리브레이션(예: 정상 샘플 거리의 p99)에도 사용한다.
    pub fn min_distance(&self, vector: &[f32]) -> f32 {
        self.frozen_centroids
            .iter()
            .map(|c| euclidean(c, vector))
            .fold(f32::INFINITY, f32::min)
    }

    /// O(k) 복잡도로 정상 판별 후, 임계치 초과 시 search_index를 뒤진다.
    pub fn detect(&self, vector: &[f32], search_index: &IdMapIndex) -> DetectionResult {
        self.detect_filtered(vector, search_index, None)
    }

    /// Tier 2 검색을 allowlist(외부 u64 ID 집합 — 예: 동일 서버의 로그)로 제한한다.
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

/// Tier 2: 최근 윈도우 인덱스에서 유사 사건 ID 확보.
fn tier2_context(vector: &[f32], index: &IdMapIndex, allowlist: Option<&[u64]>) -> Vec<u64> {
    if index.is_empty() || index.dim() != vector.len() {
        return Vec::new();
    }
    match allowlist {
        Some(ids) => {
            // search_with_allowlist는 빈 목록·미존재 ID에 panic — 사전 필터링 필수
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
        // e1 주변과 e2 주변 두 군집 (dim=4)
        let mut data = Vec::new();
        for j in 0..10 {
            let eps = j as f32 * 0.01;
            data.extend_from_slice(&[1.0, eps, 0.0, 0.0]);
            data.extend_from_slice(&[0.0, 0.0, 1.0, eps]);
        }
        let det = AnomalyDetector::fit(&data, 4, 2, 0.5);
        assert!(
            det.min_distance(&[1.0, 0.0, 0.0, 0.0]) < 0.2,
            "군집 내부는 가까움"
        );
        assert!(
            det.min_distance(&[-1.0, 0.0, 0.0, 0.0]) > 1.0,
            "반대 방향은 멀어짐"
        );
    }
}
