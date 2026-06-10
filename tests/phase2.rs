//! Phase 2 통합 테스트: 핑퐁 인덱서 (스왑/동시성/청크 백업) + K-Centroid 2-tier 탐지.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use turbolog::{AnomalyDetector, DetectionResult, PingPongIndexer};

const DIM: usize = 32;

/// 결정적 의사난수 단위 벡터 (LCG).
fn unit_vector(seed: u64) -> Vec<f32> {
    let mut state = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    let mut v: Vec<f32> = (0..DIM)
        .map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((state >> 33) as f32 / (1u64 << 31) as f32) - 1.0
        })
        .collect();
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    for x in v.iter_mut() {
        *x /= norm;
    }
    v
}

#[test]
fn swap_publishes_sealed_window() {
    let indexer = PingPongIndexer::new(DIM, 4).unwrap();
    for id in 1..=64u64 {
        indexer.ingest(id, &unit_vector(id)).unwrap();
    }
    assert!(
        indexer.get_search_index().is_empty(),
        "스왑 전에는 검색 스냅샷이 비어 있어야 함"
    );

    indexer.swap_and_flush(None).unwrap();

    let snapshot = indexer.get_search_index();
    assert_eq!(snapshot.len(), 64, "봉인된 윈도우 전체가 발행됨");
    // 양자화 손실 허용: 자기 자신이 top-5 안에 있으면 통과
    for id in [1u64, 17, 42, 64] {
        let (_scores, ids) = snapshot.search(&unit_vector(id), 5);
        assert!(ids.contains(&id), "id {id}가 top-5에 없음: {ids:?}");
    }

    // 다음 윈도우는 빈 쓰기 인덱스에서 시작
    indexer.ingest(100, &unit_vector(100)).unwrap();
    indexer.swap_and_flush(None).unwrap();
    assert_eq!(indexer.get_search_index().len(), 1, "윈도우 의미론: 직전 윈도우만");
}

#[test]
fn snapshot_survives_swap() {
    let indexer = PingPongIndexer::new(DIM, 4).unwrap();
    indexer.ingest(7, &unit_vector(7)).unwrap();
    indexer.swap_and_flush(None).unwrap();

    let old_snapshot = indexer.get_search_index();
    indexer.swap_and_flush(None).unwrap(); // 빈 윈도우로 교체됨

    // 스왑 후에도 이전에 얻은 Arc 스냅샷은 안전하게 검색 가능 (use-after-free 없음)
    let (_s, ids) = old_snapshot.search(&unit_vector(7), 1);
    assert_eq!(ids, vec![7]);
    assert!(indexer.get_search_index().is_empty());
}

#[test]
fn flush_writes_loadable_tvim_chunk() {
    let path = std::env::temp_dir().join("turbolog_test_chunk.tvim");
    let indexer = PingPongIndexer::new(DIM, 4).unwrap();
    for id in 1..=10u64 {
        indexer.ingest(id, &unit_vector(id)).unwrap();
    }
    indexer.swap_and_flush(Some(&path)).unwrap();

    let loaded = turbovec::IdMapIndex::load(&path).unwrap();
    assert_eq!(loaded.len(), 10, ".tvim 라운드트립");
    assert!(loaded.contains(5));
    std::fs::remove_file(&path).ok();
}

#[test]
fn concurrent_ingest_search_swap() {
    let indexer = Arc::new(PingPongIndexer::new(DIM, 4).unwrap());
    let stop = Arc::new(AtomicBool::new(false));

    let writer = {
        let indexer = Arc::clone(&indexer);
        thread::spawn(move || {
            for id in 1..=2000u64 {
                indexer.ingest(id, &unit_vector(id)).unwrap();
            }
        })
    };
    let searcher = {
        let indexer = Arc::clone(&indexer);
        let stop = Arc::clone(&stop);
        thread::spawn(move || {
            let query = unit_vector(1);
            let mut searches = 0u64;
            while !stop.load(Ordering::Relaxed) {
                let snapshot = indexer.get_search_index();
                if !snapshot.is_empty() {
                    let _ = snapshot.search(&query, 3);
                }
                searches += 1;
            }
            searches
        })
    };

    // 쓰기/검색이 도는 동안 스왑 반복 (스펙: 10초 주기 — 테스트에서는 짧게)
    for _ in 0..10 {
        thread::sleep(std::time::Duration::from_millis(5));
        indexer.swap_and_flush(None).unwrap();
    }
    writer.join().unwrap();
    indexer.swap_and_flush(None).unwrap(); // 잔여분 발행
    stop.store(true, Ordering::Relaxed);
    let searches = searcher.join().unwrap();

    assert!(searches > 0, "검색 스레드가 동작했어야 함");
    // 데드락·패닉 없이 완료되었고, 마지막 윈도우가 발행됨
    let final_len = indexer.get_search_index().len();
    assert!(final_len <= 2000);
}

#[test]
fn detector_two_tier() {
    // 정상 군집 2개로 캘리브레이션
    let mut normal = Vec::new();
    for i in 0..20u64 {
        let mut a = unit_vector(1);
        let mut b = unit_vector(2);
        // 군집 내 미세 변형
        a[(i % DIM as u64) as usize] += 0.01;
        b[((i + 3) % DIM as u64) as usize] += 0.01;
        normal.extend_from_slice(&a);
        normal.extend_from_slice(&b);
    }
    let detector = AnomalyDetector::fit(&normal, DIM, 2, 0.5);

    let indexer = PingPongIndexer::new(DIM, 4).unwrap();
    indexer.ingest(10, &unit_vector(1)).unwrap();
    indexer.ingest(20, &unit_vector(2)).unwrap();
    indexer.ingest(777, &unit_vector(999)).unwrap(); // 과거 유사 사건
    indexer.swap_and_flush(None).unwrap();
    let snapshot = indexer.get_search_index();

    // Tier 1: 정상 벡터는 즉시 통과
    assert!(matches!(
        detector.detect(&unit_vector(1), &snapshot),
        DetectionResult::Normal
    ));

    // Tier 1 초과 → Tier 2: 이상 벡터는 유사 사건 컨텍스트와 함께 보고
    let outlier = unit_vector(999);
    match detector.detect(&outlier, &snapshot) {
        DetectionResult::Anomaly {
            score,
            nearest_incidents,
        } => {
            assert!(score > 0.5, "score={score}");
            assert!(
                nearest_incidents.first() == Some(&777),
                "가장 유사한 과거 사건이 1순위여야 함: {nearest_incidents:?}"
            );
        }
        DetectionResult::Normal => panic!("이상 벡터가 Normal로 분류됨"),
    }

    // Tier 2 allowlist: 동일 서버(id 10, 20)로 제한
    match detector.detect_filtered(&outlier, &snapshot, Some(&[10, 20])) {
        DetectionResult::Anomaly {
            nearest_incidents, ..
        } => {
            assert!(!nearest_incidents.contains(&777), "allowlist 밖 ID 제외");
            assert!(nearest_incidents.iter().all(|id| [10, 20].contains(id)));
        }
        DetectionResult::Normal => panic!("이상 벡터가 Normal로 분류됨"),
    }

    // allowlist의 ID가 인덱스에 없으면 panic 대신 빈 컨텍스트
    match detector.detect_filtered(&outlier, &snapshot, Some(&[555])) {
        DetectionResult::Anomaly {
            nearest_incidents, ..
        } => assert!(nearest_incidents.is_empty()),
        DetectionResult::Normal => panic!("이상 벡터가 Normal로 분류됨"),
    }
}
