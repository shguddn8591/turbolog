//! Phase 2 integration test: PingPong Indexer (swap/concurrency/chunk backup) + K-Centroid 2-tier detection.

#![cfg(feature = "server")]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use turbolog::{AnomalyDetector, DetectionResult, PingPongIndexer};

const DIM: usize = 32;

/// Deterministic pseudorandom unit vector (LCG).
fn unit_vector(seed: u64) -> Vec<f32> {
    let mut state = seed
        .wrapping_mul(6364136223846793005)
        .wrapping_add(1442695040888963407);
    let mut v: Vec<f32> = (0..DIM)
        .map(|_| {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
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
        "Search snapshot should be empty before swap"
    );

    indexer.swap_and_flush(None).unwrap();

    let snapshot = indexer.get_search_index();
    assert_eq!(snapshot.len(), 64, "Entire sealed window is published");
    // Allow quantization loss: pass if self is in top-5
    for id in [1u64, 17, 42, 64] {
        let (_scores, ids) = snapshot.search(&unit_vector(id), 5);
        assert!(ids.contains(&id), "id {id} is not in top-5: {ids:?}");
    }

    // Next window starts with an empty write index
    indexer.ingest(100, &unit_vector(100)).unwrap();
    indexer.swap_and_flush(None).unwrap();
    assert_eq!(
        indexer.get_search_index().len(),
        1,
        "Window semantics: only the previous window"
    );
}

#[test]
fn snapshot_survives_swap() {
    let indexer = PingPongIndexer::new(DIM, 4).unwrap();
    indexer.ingest(7, &unit_vector(7)).unwrap();
    indexer.swap_and_flush(None).unwrap();

    let old_snapshot = indexer.get_search_index();
    indexer.swap_and_flush(None).unwrap(); // Swapped with an empty window

    // Even after swapping, previously acquired Arc snapshot can be safely searched (no use-after-free)
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
    assert_eq!(loaded.len(), 10, ".tvim roundtrip");
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

    // Repeat swap while write/search is running (spec: 10 second period — short in test)
    for _ in 0..10 {
        thread::sleep(std::time::Duration::from_millis(5));
        indexer.swap_and_flush(None).unwrap();
    }
    writer.join().unwrap();
    indexer.swap_and_flush(None).unwrap(); // Publish remaining
    stop.store(true, Ordering::Relaxed);
    let searches = searcher.join().unwrap();

    assert!(searches > 0, "Search thread should have operated");
    // Completed without deadlock/panic, and the last window is published
    let final_len = indexer.get_search_index().len();
    assert!(final_len <= 2000);
}

#[test]
fn detector_two_tier() {
    // Calibrate with 2 normal clusters
    let mut normal = Vec::new();
    for i in 0..20u64 {
        let mut a = unit_vector(1);
        let mut b = unit_vector(2);
        // Micro-variation within the cluster
        a[(i % DIM as u64) as usize] += 0.01;
        b[((i + 3) % DIM as u64) as usize] += 0.01;
        normal.extend_from_slice(&a);
        normal.extend_from_slice(&b);
    }
    let detector = AnomalyDetector::fit(&normal, DIM, 2, 0.5);

    let indexer = PingPongIndexer::new(DIM, 4).unwrap();
    indexer.ingest(10, &unit_vector(1)).unwrap();
    indexer.ingest(20, &unit_vector(2)).unwrap();
    indexer.ingest(777, &unit_vector(999)).unwrap(); // Similar past incident
    indexer.swap_and_flush(None).unwrap();
    let snapshot = indexer.get_search_index();

    // Tier 1: Normal vectors pass immediately
    assert!(matches!(
        detector.detect(&unit_vector(1), &snapshot),
        DetectionResult::Normal
    ));

    // Exceeds Tier 1 -> Tier 2: Anomaly vectors reported with similar incident context
    let outlier = unit_vector(999);
    match detector.detect(&outlier, &snapshot) {
        DetectionResult::Anomaly {
            score,
            nearest_incidents,
        } => {
            assert!(score > 0.5, "score={score}");
            assert!(
                nearest_incidents.first() == Some(&777),
                "The most similar past incident should be ranked first: {nearest_incidents:?}"
            );
        }
        DetectionResult::Normal => panic!("Anomaly vector classified as Normal"),
    }

    // Tier 2 allowlist: Restricted to the same server (id 10, 20)
    match detector.detect_filtered(&outlier, &snapshot, Some(&[10, 20])) {
        DetectionResult::Anomaly {
            nearest_incidents, ..
        } => {
            assert!(!nearest_incidents.contains(&777), "Exclude IDs outside allowlist");
            assert!(nearest_incidents.iter().all(|id| [10, 20].contains(id)));
        }
        DetectionResult::Normal => panic!("Anomaly vector classified as Normal"),
    }

    // If allowlist ID is not in index, return empty context instead of panic
    match detector.detect_filtered(&outlier, &snapshot, Some(&[555])) {
        DetectionResult::Anomaly {
            nearest_incidents, ..
        } => assert!(nearest_incidents.is_empty()),
        DetectionResult::Normal => panic!("Anomaly vector classified as Normal"),
    }
}
