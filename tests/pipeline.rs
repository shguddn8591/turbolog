//! 통합 테스트: raw 로그 → 템플릿 파싱 → 캐시/임베딩 파이프라인.
//! `scripts/download_model.sh` 실행 후 models/ 파일이 있어야 동작 (없으면 skip).

use std::path::PathBuf;

use turbolog::{AnomalyDetector, DetectionResult, Embedder, PingPongIndexer, VectorCache};

fn models_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
    (dir.join("model.onnx").exists() && dir.join("tokenizer.json").exists()).then_some(dir)
}

/// 템플릿 5종 × 20건 = 100건의 합성 로그.
fn synthetic_logs() -> Vec<String> {
    let mut logs = Vec::new();
    for i in 0..20 {
        logs.push(format!("connection accepted from 10.0.0.{i} port {}", 5000 + i));
        logs.push(format!("user u{i} login success from web console"));
        logs.push(format!("disk usage at {} percent on /var", 50 + i));
        logs.push(format!("request to /api/v1/items took {} ms", 10 * i + 3));
        logs.push(format!("worker {i} heartbeat ok at epoch {}", 1700000000 + i));
    }
    logs
}

#[test]
fn pipeline_cache_hit_rate_and_vector_shape() {
    let Some(dir) = models_dir() else {
        eprintln!("skip: models/ 없음 — ./scripts/download_model.sh 먼저 실행");
        return;
    };
    let embedder = Embedder::new(dir.join("model.onnx"), dir.join("tokenizer.json")).unwrap();
    let mut cache = VectorCache::new(embedder);

    let mut first_vector: Option<std::sync::Arc<[f32]>> = None;
    for log in synthetic_logs() {
        let (parsed, vector) = cache.get_or_embed(&log).unwrap();
        assert_eq!(vector.len(), 384, "all-MiniLM-L6-v2는 384차원");
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-3, "L2 정규화 norm={norm}");
        assert!(!parsed.template.is_empty());
        first_vector.get_or_insert(vector);
    }

    let total = cache.hits() + cache.misses();
    assert_eq!(total, 100);
    // 템플릿 5종이므로 miss는 소수 — Drain 템플릿이 초기 몇 건에서 진화하며
    // 템플릿당 1~2회 miss가 날 수 있어 히트율 하한은 90%로 잡는다.
    assert!(
        cache.hit_rate() >= 0.90,
        "히트율 {:.1}% (hits={}, misses={})",
        cache.hit_rate() * 100.0,
        cache.hits(),
        cache.misses()
    );
    println!(
        "캐시 히트율: {:.1}% (hits={}, misses={})",
        cache.hit_rate() * 100.0,
        cache.hits(),
        cache.misses()
    );
}

#[test]
fn cache_hit_skips_embedding() {
    let Some(dir) = models_dir() else {
        eprintln!("skip: models/ 없음 — ./scripts/download_model.sh 먼저 실행");
        return;
    };
    let embedder = Embedder::new(dir.join("model.onnx"), dir.join("tokenizer.json")).unwrap();
    let mut cache = VectorCache::new(embedder);

    let (a, va) = cache.get_or_embed("Node 2 is online").unwrap();
    assert_eq!(cache.misses(), 1);
    let (b, vb) = cache.get_or_embed("Node 7 is online").unwrap();
    assert_eq!(a.template_id, b.template_id, "변수만 다른 로그는 같은 템플릿");
    assert_eq!(cache.hits(), 1, "두 번째 인입은 캐시 히트");
    assert_eq!(cache.misses(), 1, "임베딩 추가 호출 없음");
    assert!(std::sync::Arc::ptr_eq(&va, &vb), "동일 캐시 엔트리 공유");
}

/// E2E: 로그 텍스트 → 임베딩 → 인덱싱 → 2-tier 이상 탐지.
#[test]
fn end_to_end_anomaly_detection() {
    let Some(dir) = models_dir() else {
        eprintln!("skip: models/ 없음 — ./scripts/download_model.sh 먼저 실행");
        return;
    };
    let embedder = Embedder::new(dir.join("model.onnx"), dir.join("tokenizer.json")).unwrap();
    let mut cache = VectorCache::new(embedder);

    // 1) 정상 로그 인입 → 벡터 수집 + 핑퐁 인덱싱
    let indexer = PingPongIndexer::new(384, 4).unwrap();
    let mut normal_flat: Vec<f32> = Vec::new();
    let mut next_id = 0u64;
    for log in synthetic_logs() {
        let (parsed, vector) = cache.get_or_embed(&log).unwrap();
        next_id += 1;
        indexer.ingest(next_id, &vector).unwrap();
        // 템플릿당 1회만 캘리브레이션 셋에 추가 (miss 시점 = 새 템플릿)
        let _ = parsed;
        if cache.misses() as usize * 384 > normal_flat.len() {
            normal_flat.extend_from_slice(&vector);
        }
    }
    indexer.swap_and_flush(None).unwrap();
    let snapshot = indexer.get_search_index();

    // 2) 정상 템플릿 5종으로 centroid 동결 (k=5)
    let detector = AnomalyDetector::fit(&normal_flat, 384, 5, 0.5);

    // 3) 정상 로그 재인입 → Tier 1 즉시 통과
    let (_p, v) = cache.get_or_embed("disk usage at 77 percent on /var").unwrap();
    assert!(
        matches!(detector.detect(&v, &snapshot), DetectionResult::Normal),
        "기존 템플릿 로그는 Normal"
    );

    // 4) 처음 보는 치명적 로그 → Anomaly + 유사 사건 컨텍스트
    let (_p, v) = cache
        .get_or_embed("FATAL kernel panic at address 0xdeadbeef, halting node")
        .unwrap();
    match detector.detect(&v, &snapshot) {
        DetectionResult::Anomaly {
            score,
            nearest_incidents,
        } => {
            assert!(score > 0.5, "score={score}");
            assert!(!nearest_incidents.is_empty(), "최근 윈도우 유사 맥락 확보");
            println!("이상 탐지: score={score:.3}, 유사 사건={nearest_incidents:?}");
        }
        DetectionResult::Normal => panic!("치명적 로그가 Normal로 분류됨"),
    }
}
