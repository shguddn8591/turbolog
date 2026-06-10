//! 통합 테스트: raw 로그 → 템플릿 파싱 → 캐시/임베딩 파이프라인.
//! `scripts/download_model.sh` 실행 후 models/ 파일이 있어야 동작 (없으면 skip).

use std::path::PathBuf;

use turbolog::{Embedder, VectorCache};

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
