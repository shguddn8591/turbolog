//! 부하 테스트 하니스 — `cargo run --release --example loadtest`
//!
//! 측정 항목:
//! 1. 임베딩 원시 처리량 (Cache Miss 경로의 상한)
//! 2. 엔진 인입 처리량 — 캐시 적중 경로 (스펙 목표: 초당 수천 건)
//! 3. 동시 부하: ingest + 1초 주기 스왑 + 검색 레이턴시 p50/p99
//! 4. 엣지케이스 프로브: 초장문 로그 라인
//! 5. HTTP 경로 처리량 (배치 10건 POST /logs)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use turbolog::engine::{EngineConfig, TurboLogEngine};
use turbolog::http::run_server;
use turbolog::Embedder;

const TEMPLATES: usize = 10;

fn make_log(i: usize) -> String {
    match i % TEMPLATES {
        0 => format!("connection accepted from 10.0.{}.{} port {}", i % 256, (i * 7) % 256, 5000 + i % 1000),
        1 => format!("user u{} login success from web console", i),
        2 => format!("disk usage at {} percent on /var", i % 100),
        3 => format!("request to /api/v1/items took {} ms", i % 900),
        4 => format!("worker {} heartbeat ok at epoch {}", i % 64, 1700000000 + i),
        5 => format!("cache evicted {} entries in shard {}", i % 5000, i % 16),
        6 => format!("gc pause of {} ms in region old-gen", i % 300),
        7 => format!("tcp retransmit count {} on eth0", i % 99),
        8 => format!("query plan hash {} executed in {} ms", i * 31, i % 50),
        _ => format!("session {} renewed token expiring in {} s", i, 3600 - i % 600),
    }
}

fn percentile(sorted: &[Duration], p: f64) -> Duration {
    if sorted.is_empty() {
        return Duration::ZERO;
    }
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx]
}

fn main() -> anyhow::Result<()> {
    let models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("models");
    anyhow::ensure!(models.join("model.onnx").exists(), "먼저 ./scripts/download_model.sh 실행");
    let data_dir = std::env::temp_dir().join(format!("turbolog_loadtest_{}", std::process::id()));
    std::fs::remove_dir_all(&data_dir).ok();

    println!("== TurboLog 부하 테스트 (release) ==\n");

    // ── 1) 임베딩 원시 처리량 (Cache Miss 경로 상한) ──
    {
        let mut embedder = Embedder::new(models.join("model.onnx"), models.join("tokenizer.json"))?;
        let n = 100;
        let t = Instant::now();
        for i in 0..n {
            embedder.embed(&format!("benchmark sentence number {i} with some tokens"))?;
        }
        let el = t.elapsed();
        println!(
            "[1] 임베딩(ONNX 추론): {:.0} embeds/s  (평균 {:.2} ms/건) — Cache Miss 경로 상한",
            n as f64 / el.as_secs_f64(),
            el.as_secs_f64() * 1000.0 / n as f64
        );
    }

    let cfg = EngineConfig {
        data_dir: data_dir.clone(),
        calibration_templates: TEMPLATES,
        centroids: TEMPLATES,
        ..EngineConfig::default()
    };
    let embedder = Embedder::new(models.join("model.onnx"), models.join("tokenizer.json"))?;
    let engine = Arc::new(TurboLogEngine::open(cfg, vec![embedder])?);

    // 워밍업: 템플릿 캐시 + 캘리브레이션
    for i in 0..TEMPLATES * 3 {
        engine.ingest_log(&make_log(i))?;
    }
    engine.swap_tick()?;
    println!(
        "    워밍업 완료 (detector_calibrated={})\n",
        engine.stats().detector_calibrated
    );

    // ── 2) 인입 처리량 — 캐시 적중 경로 ──
    {
        let n = 50_000;
        let mut lat = Vec::with_capacity(n);
        let t = Instant::now();
        for i in 0..n {
            let t0 = Instant::now();
            engine.ingest_log(&make_log(i))?;
            lat.push(t0.elapsed());
        }
        let el = t.elapsed();
        lat.sort();
        println!(
            "[2] 엔진 인입(캐시 적중): {:.0} logs/s  (n={n}, p50={:?}, p99={:?}, max={:?})",
            n as f64 / el.as_secs_f64(),
            percentile(&lat, 0.50),
            percentile(&lat, 0.99),
            lat.last().unwrap()
        );
        engine.swap_tick()?;
    }

    // ── 3) 동시 부하: ingest + 1초 스왑 + 검색 p50/p99 ──
    {
        let stop = Arc::new(AtomicBool::new(false));
        let ingested = Arc::new(AtomicU64::new(0));

        let swapper = {
            let engine = Arc::clone(&engine);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_secs(1));
                    engine.swap_tick().unwrap();
                }
            })
        };
        let searcher = {
            let engine = Arc::clone(&engine);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut lat = Vec::new();
                while !stop.load(Ordering::Relaxed) {
                    let t0 = Instant::now();
                    let _ = engine.search_text("disk space almost full warning", 5).unwrap();
                    lat.push(t0.elapsed());
                    std::thread::sleep(Duration::from_millis(10));
                }
                lat
            })
        };

        let n = 30_000;
        let t = Instant::now();
        for i in 0..n {
            engine.ingest_log(&make_log(i))?;
            ingested.fetch_add(1, Ordering::Relaxed);
        }
        let el = t.elapsed();
        stop.store(true, Ordering::Relaxed);
        swapper.join().unwrap();
        let mut search_lat = searcher.join().unwrap();
        search_lat.sort();
        println!(
            "[3] 동시 부하 인입: {:.0} logs/s  | 검색(임베딩 포함) p50={:?}, p99={:?} (n={})",
            n as f64 / el.as_secs_f64(),
            percentile(&search_lat, 0.50),
            percentile(&search_lat, 0.99),
            search_lat.len()
        );
    }

    // ── 4) 엣지케이스 프로브 ──
    {
        let long_line = "ERROR stack overflow ".repeat(2000); // ~42,000자
        match engine.ingest_log(&long_line) {
            Ok(r) => println!("[4] 초장문(42k자) 로그: OK (id={}, anomaly={})", r.id, r.anomaly.is_some()),
            Err(e) => println!("[4] 초장문(42k자) 로그: ERROR — {e:#}"),
        }
        match engine.ingest_log("") {
            Ok(_) => println!("    빈 문자열 로그: OK"),
            Err(e) => println!("    빈 문자열 로그: ERROR — {e:#}"),
        }
    }

    // ── 5) HTTP 경로 (배치 10건 × 4 클라이언트 스레드) ──
    {
        let (addr, _h) = run_server(Arc::clone(&engine), "127.0.0.1:0", 4)?;
        let url = format!("http://{addr}/logs");
        let reqs_per_thread = 250usize;
        let batch = 10usize;
        let t = Instant::now();
        let threads: Vec<_> = (0..4)
            .map(|w| {
                let url = url.clone();
                std::thread::spawn(move || {
                    for r in 0..reqs_per_thread {
                        let logs: Vec<String> =
                            (0..batch).map(|j| make_log(w * 1000 + r * batch + j)).collect();
                        let resp = ureq::post(&url)
                            .send_json(serde_json::json!({ "logs": logs }))
                            .unwrap();
                        assert_eq!(resp.status(), 200);
                    }
                })
            })
            .collect();
        for th in threads {
            th.join().unwrap();
        }
        let el = t.elapsed();
        let total_logs = 4 * reqs_per_thread * batch;
        println!(
            "[5] HTTP /logs (4 클라이언트, 배치 {batch}): {:.0} logs/s ({:.0} req/s)",
            total_logs as f64 / el.as_secs_f64(),
            (4 * reqs_per_thread) as f64 / el.as_secs_f64()
        );
    }

    // ── 6) 미스 폭풍 내성: 신규 템플릿 폭주 중 적중 경로 처리량 ──
    // 수정 전에는 캐시 락이 ONNX 추론까지 직렬화해 적중 경로도 ~136 logs/s로 붕괴했다.
    {
        let stop = Arc::new(AtomicBool::new(false));
        let storm = {
            let engine = Arc::clone(&engine);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut embedded = 0u64;
                // 토큰 수가 모두 다른 고유 패턴 → 강제 캐시 미스 연속
                for i in 0..400 {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let words: Vec<String> =
                        (0..(3 + i % 120)).map(|j| format!("storm{i}tok{j}")).collect();
                    engine.ingest_log(&words.join(" ")).unwrap();
                    embedded += 1;
                }
                embedded
            })
        };

        // 폭풍이 도는 동안 적중 경로 처리량 측정 (2초간)
        std::thread::sleep(Duration::from_millis(100)); // 폭풍 워밍업
        let t = Instant::now();
        let mut hits = 0u64;
        while t.elapsed() < Duration::from_secs(2) {
            engine.ingest_log(&make_log(hits as usize))?;
            hits += 1;
        }
        let el = t.elapsed();
        stop.store(true, Ordering::Relaxed);
        let storm_count = storm.join().unwrap();
        println!(
            "[6] 미스 폭풍 내성: 적중 경로 {:.0} logs/s (폭풍 중 신규 템플릿 {}건 처리)",
            hits as f64 / el.as_secs_f64(),
            storm_count
        );
    }

    let s = engine.stats();
    println!(
        "\n== 최종 stats: ingested={}, cache_hit_rate={:.4}, ring_windows={}, ring_vectors={} ==",
        s.ingested_total, s.cache_hit_rate, s.ring_windows, s.ring_vectors
    );
    std::fs::remove_dir_all(&data_dir).ok();
    Ok(())
}
