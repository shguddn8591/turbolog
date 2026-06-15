//! Phase 3 통합 테스트: WAL 크래시 복구, 링 병합 검색, 자동 캘리브레이션, HTTP API.
//! 모델 파일 필요 (`./scripts/download_model.sh`) — 없으면 skip.

use std::path::PathBuf;
use std::sync::Arc;

use turbolog::engine::{EngineConfig, TurboLogEngine};
use turbolog::http::run_server;
use turbolog::Embedder;

fn models_dir() -> Option<PathBuf> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
    (dir.join("model.onnx").exists() && dir.join("tokenizer.json").exists()).then_some(dir)
}

fn make_embedder(dir: &std::path::Path) -> Embedder {
    Embedder::new(dir.join("model.onnx"), dir.join("tokenizer.json")).unwrap()
}

fn temp_data_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("turbolog_p3_{name}_{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    dir
}

fn test_config(data_dir: PathBuf) -> EngineConfig {
    EngineConfig {
        data_dir,
        calibration_templates: 5,
        centroids: 5,
        ring_windows: 4,
        shards: 1,
        ..EngineConfig::default()
    }
}

#[test]
fn wal_crash_recovery() {
    let Some(models) = models_dir() else {
        eprintln!("skip: models/ 없음");
        return;
    };
    let data_dir = temp_data_dir("recovery");

    // 1) 인입 후 스왑 없이 종료 (크래시 시뮬레이션 — WAL에만 존재)
    {
        let engine =
            TurboLogEngine::open(test_config(data_dir.clone()), vec![make_embedder(&models)]).unwrap();
        for i in 0..7 {
            engine
                .ingest_log(&format!("payment failed for order {i} with code 502"))
                .unwrap();
        }
        assert_eq!(engine.stats().pending_window_len, 7);
        // drop — swap_tick 호출 안 함
    }

    // 2) 재기동 → WAL 재생으로 쓰기 윈도우 복원
    let engine =
        TurboLogEngine::open(test_config(data_dir.clone()), vec![make_embedder(&models)]).unwrap();
    assert_eq!(engine.stats().pending_window_len, 7, "WAL 재생 복구");

    // 3) 봉인 후 검색 가능 + WAL은 로테이트되어 비어 있음
    assert!(engine.swap_tick().unwrap());
    let hits = engine
        .search_text("payment failed with error code", 3)
        .unwrap();
    assert!(!hits.is_empty(), "복구된 벡터가 검색됨");
    assert_eq!(
        turbolog::Wal::replay(data_dir.join("wal-0.bin"), 384)
            .unwrap()
            .len(),
        0
    );

    // 4) 세그먼트 청크 파일이 시간 디렉터리에 생성됨
    let chunk_root = data_dir.join("chunks");
    let hour_dirs: Vec<_> = std::fs::read_dir(&chunk_root).unwrap().collect();
    assert_eq!(hour_dirs.len(), 1);
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn ring_merges_multiple_windows() {
    let Some(models) = models_dir() else {
        eprintln!("skip: models/ 없음");
        return;
    };
    let data_dir = temp_data_dir("ring");
    let engine =
        TurboLogEngine::open(test_config(data_dir.clone()), vec![make_embedder(&models)]).unwrap();

    // 윈도우 1: 디스크 경고 / 윈도우 2: 네트워크 타임아웃
    let disk = engine
        .ingest_log("disk usage at 95 percent on /var")
        .unwrap();
    assert!(engine.swap_tick().unwrap());
    let net = engine
        .ingest_log("network timeout connecting to upstream 10.0.0.9")
        .unwrap();
    assert!(engine.swap_tick().unwrap());
    assert!(!engine.swap_tick().unwrap(), "빈 윈도우는 스왑 스킵");

    let stats = engine.stats();
    assert_eq!(stats.ring_windows, 2);
    assert_eq!(stats.ring_vectors, 2);

    // 두 윈도우의 내용이 모두 한 번의 검색으로 나옴
    let hits = engine
        .search_text("disk space almost full warning", 2)
        .unwrap();
    assert_eq!(hits.first().map(|h| h.id), Some(disk.id), "윈도우 1 최상위");
    let hits = engine
        .search_text("connection timeout to remote host", 2)
        .unwrap();
    assert_eq!(hits.first().map(|h| h.id), Some(net.id), "윈도우 2 최상위");
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn auto_calibration_then_detection() {
    let Some(models) = models_dir() else {
        eprintln!("skip: models/ 없음");
        return;
    };
    let data_dir = temp_data_dir("calib");
    let engine =
        TurboLogEngine::open(test_config(data_dir.clone()), vec![make_embedder(&models)]).unwrap();

    assert!(!engine.stats().detector_calibrated);
    // 정상 템플릿 5종 → calibration_templates=5 도달 시 자동 동결
    for i in 0..20 {
        engine
            .ingest_log(&format!("connection accepted from 10.0.0.{i} port 5432"))
            .unwrap();
        engine
            .ingest_log(&format!("user u{i} login success from web console"))
            .unwrap();
        engine
            .ingest_log(&format!("disk usage at {i} percent on /var"))
            .unwrap();
        engine
            .ingest_log(&format!("request to /api/v1/items took {i} ms"))
            .unwrap();
        engine
            .ingest_log(&format!("worker {i} heartbeat ok at epoch {i}"))
            .unwrap();
    }
    assert!(engine.stats().detector_calibrated, "자동 캘리브레이션 완료");
    engine.swap_tick().unwrap();

    // 정상 로그 → anomaly 없음
    let normal = engine
        .ingest_log("disk usage at 42 percent on /var")
        .unwrap();
    assert!(normal.anomaly.is_none());

    // 치명적 신규 패턴 → anomaly + 최근 윈도우 유사 맥락
    let fatal = engine
        .ingest_log("FATAL kernel panic at address 0xdeadbeef, halting node")
        .unwrap();
    let report = fatal.anomaly.expect("이상 탐지되어야 함");
    assert!(report.score > 0.5);
    assert!(!report.nearest_incidents.is_empty());
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn http_api_end_to_end() {
    let Some(models) = models_dir() else {
        eprintln!("skip: models/ 없음");
        return;
    };
    let data_dir = temp_data_dir("http");
    let engine = Arc::new(
        TurboLogEngine::open(test_config(data_dir.clone()), vec![make_embedder(&models)]).unwrap(),
    );
    let (addr, _handles) = run_server(Arc::clone(&engine), "127.0.0.1:0", 2, None).unwrap();
    let base = format!("http://{addr}");

    // POST /logs
    let resp: serde_json::Value = ureq::post(&format!("{base}/logs"))
        .send_json(serde_json::json!({
            "logs": ["disk usage at 91 percent on /var", "disk usage at 12 percent on /var"]
        }))
        .unwrap()
        .into_json()
        .unwrap();
    let results = resp["results"].as_array().unwrap();
    assert_eq!(results.len(), 2);
    assert_eq!(
        results[0]["template_id"], results[1]["template_id"],
        "같은 템플릿"
    );

    // 봉인 후 POST /search
    engine.swap_tick().unwrap();
    let resp: serde_json::Value = ureq::post(&format!("{base}/search"))
        .send_json(serde_json::json!({"query": "disk space warning", "k": 2}))
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(resp["results"].as_array().unwrap().len(), 2);

    // GET /stats
    let resp: serde_json::Value = ureq::get(&format!("{base}/stats"))
        .call()
        .unwrap()
        .into_json()
        .unwrap();
    assert_eq!(resp["ingested_total"], 2);
    assert_eq!(resp["ring_windows"], 1);

    // 잘못된 요청 → 400
    let err = ureq::post(&format!("{base}/search"))
        .send_json(serde_json::json!({"k": 2}))
        .unwrap_err();
    match err {
        ureq::Error::Status(code, _) => assert_eq!(code, 400),
        other => panic!("400 응답이어야 함: {other}"),
    }
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn sealed_wal_leftover_recovery() {
    // 크래시 시나리오: 봉인(WAL detach) 직후, 세그먼트 백업 완료 전에 프로세스 사망.
    // 잔여 wal-sealed-*.bin이 재기동 시 재생·통합되어야 한다.
    let Some(models) = models_dir() else {
        eprintln!("skip: models/ 없음");
        return;
    };
    let data_dir = temp_data_dir("sealed_leftover");
    {
        let engine =
            TurboLogEngine::open(test_config(data_dir.clone()), vec![make_embedder(&models)])
                .unwrap();
        for i in 0..4 {
            engine
                .ingest_log(&format!("replica lag {i} ms on shard primary"))
                .unwrap();
        }
        // drop — 스왑 없이 종료
    }
    // 봉인 직후 크래시 흉내: 활성 WAL을 sealed 파일로 rename만 해 둔다
    std::fs::rename(
        data_dir.join("wal-0.bin"),
        data_dir.join("wal-0-sealed-99.bin"),
    )
    .unwrap();

    let engine =
        TurboLogEngine::open(test_config(data_dir.clone()), vec![make_embedder(&models)]).unwrap();
    assert_eq!(engine.stats().pending_window_len, 4, "sealed 잔여물 복구");
    // 통합 후: 잔여 파일은 사라지고 활성 WAL에 4건이 재영속화됨
    assert!(!data_dir.join("wal-0-sealed-99.bin").exists());
    assert_eq!(
        turbolog::Wal::replay(data_dir.join("wal-0.bin"), 384)
            .unwrap()
            .len(),
        4
    );
    std::fs::remove_dir_all(&data_dir).ok();
}

#[test]
fn http_auth_and_body_limit() {
    let Some(models) = models_dir() else {
        eprintln!("skip: models/ 없음");
        return;
    };
    let data_dir = temp_data_dir("http_auth");
    let engine = Arc::new(
        TurboLogEngine::open(test_config(data_dir.clone()), vec![make_embedder(&models)]).unwrap(),
    );
    let (addr, _h) = run_server(
        Arc::clone(&engine),
        "127.0.0.1:0",
        2,
        Some("secret-token".into()),
    )
    .unwrap();
    let base = format!("http://{addr}");

    // 토큰 없음 → 401
    let err = ureq::get(&format!("{base}/stats")).call().unwrap_err();
    match err {
        ureq::Error::Status(code, _) => assert_eq!(code, 401),
        other => panic!("401이어야 함: {other}"),
    }
    // 잘못된 토큰 → 401
    let err = ureq::get(&format!("{base}/stats"))
        .set("Authorization", "Bearer wrong")
        .call()
        .unwrap_err();
    match err {
        ureq::Error::Status(code, _) => assert_eq!(code, 401),
        other => panic!("401이어야 함: {other}"),
    }
    // 올바른 토큰 → 200
    let resp = ureq::get(&format!("{base}/stats"))
        .set("Authorization", "Bearer secret-token")
        .call()
        .unwrap();
    assert_eq!(resp.status(), 200);

    // 본문 한도 초과(1MiB+) → 413
    let huge = "x".repeat(1024 * 1024 + 100);
    let err = ureq::post(&format!("{base}/logs"))
        .set("Authorization", "Bearer secret-token")
        .send_json(serde_json::json!({ "logs": [huge] }))
        .unwrap_err();
    match err {
        ureq::Error::Status(code, _) => assert_eq!(code, 413),
        other => panic!("413이어야 함: {other}"),
    }
    std::fs::remove_dir_all(&data_dir).ok();
}
