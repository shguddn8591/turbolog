//! Anomaly Detection Layer — K-Centroid 기반 고속 스크리닝 필터. (Phase 2 구현 예정)
//!
//! Tier 1: 고정 centroid 32개와의 유클리드 거리로 O(32) 정상 판별.
//! Tier 2: 임계치 초과 벡터만 turbovec 심층 검색 (allowlist로 동일 서버/최근 시간대 필터링).
//!
//! 시스템 제약 (스펙 v1.0 §4.1 — No Dynamic Re-training):
//! centroid와 turbovec의 TQ+ 캘리브레이션/회전 행렬은 초기 결정 후 전체 수명 주기 동안
//! 고정된다. 실시간 재학습 로직은 절대 구현하지 않는다.

pub enum DetectionResult {
    Normal,
    Anomaly {
        score: f32,
        nearest_incidents: Vec<u64>,
    },
}

pub struct AnomalyDetector {
    /// 오염을 막기 위해 런타임에 변경되지 않는 고정 중심점들.
    frozen_centroids: Vec<Vec<f32>>,
    anomaly_threshold: f32,
}

impl AnomalyDetector {
    pub fn new(frozen_centroids: Vec<Vec<f32>>, anomaly_threshold: f32) -> Self {
        Self {
            frozen_centroids,
            anomaly_threshold,
        }
    }

    /// O(32) 복잡도로 정상 판별 후, 임계치 초과 시 search_index를 뒤진다.
    pub fn detect(
        &self,
        vector: &[f32],
        search_index: &turbovec::IdMapIndex,
    ) -> DetectionResult {
        let _ = (
            vector,
            search_index,
            &self.frozen_centroids,
            self.anomaly_threshold,
        );
        todo!("Phase 2: Tier 1 centroid 거리 연산 + Tier 2 turbovec 심층 검색")
    }
}
