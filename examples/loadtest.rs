//! Load test harness — `cargo run --release --example loadtest`
//!
//! Metrics:
//! 1. Embedding raw throughput (upper bound of Cache Miss path)
//! 2. Engine ingestion throughput — cache hit path (spec goal: thousands per second)
//! 3. Concurrent load: ingest + 1s swap + search latency p50/p99
//! 4. Edge case probe: extremely long log lines
//! 5. HTTP path throughput (batch 10 POST /logs)
//! 6. Miss storm tolerance
//! 7. Multi-threaded concurrent ingestion contention (global write lock scalability)

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use turbolog::engine::{EngineConfig, TurboLogEngine};
use turbolog::http::run_server;
use turbolog::Embedder;

const TEMPLATES: usize = 10;

fn make_log(i: usize) -> String {
    match i % TEMPLATES {
        0 => format!(
            "connection accepted from 10.0.{}.{} port {}",
            i % 256,
            (i * 7) % 256,
            5000 + i % 1000
        ),
        1 => format!("user u{} login success from web console", i),
        2 => format!("disk usage at {} percent on /var", i % 100),
        3 => format!("request to /api/v1/items took {} ms", i % 900),
        4 => format!("worker {} heartbeat ok at epoch {}", i % 64, 1700000000 + i),
        5 => format!("cache evicted {} entries in shard {}", i % 5000, i % 16),
        6 => format!("gc pause of {} ms in region old-gen", i % 300),
        7 => format!("tcp retransmit count {} on eth0", i % 99),
        8 => format!("query plan hash {} executed in {} ms", i * 31, i % 50),
        _ => format!(
            "session {} renewed token expiring in {} s",
            i,
            3600 - i % 600
        ),
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
    anyhow::ensure!(
        models.join("model.onnx").exists(),
        "run ./scripts/download_model.sh first"
    );
    let data_dir = std::env::temp_dir().join(format!("turbolog_loadtest_{}", std::process::id()));
    std::fs::remove_dir_all(&data_dir).ok();

    println!("== TurboLog Load Test (release) ==\n");

    // ── 1) Embedding raw throughput (Cache Miss path upper bound) ──
    {
        let mut embedder = Embedder::new(models.join("model.onnx"), models.join("tokenizer.json"))?;
        let n = 100;
        let t = Instant::now();
        for i in 0..n {
            embedder.embed(&format!("benchmark sentence number {i} with some tokens"))?;
        }
        let el = t.elapsed();
        println!(
            "[1] Embedding (ONNX inference): {:.0} embeds/s  (avg {:.2} ms/item) — Cache Miss path upper bound",
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

    // Warm-up: template cache + calibration
    for i in 0..TEMPLATES * 3 {
        engine.ingest_log(&make_log(i))?;
    }
    engine.swap_tick()?;
    println!(
        "    Warm-up completed (detector_calibrated={})\n",
        engine.stats().detector_calibrated
    );

    // ── 2) Ingestion throughput — cache hit path ──
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
            "[2] Engine ingestion (cache hit): {:.0} logs/s  (n={n}, p50={:?}, p99={:?}, max={:?})",
            n as f64 / el.as_secs_f64(),
            percentile(&lat, 0.50),
            percentile(&lat, 0.99),
            lat.last().unwrap()
        );
        engine.swap_tick()?;
    }

    // ── 3) Concurrent load: ingest + 1s swap + search p50/p99 ──
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
                    let _ = engine
                        .search_text("disk space almost full warning", 5)
                        .unwrap();
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
            "[3] Concurrent load ingestion: {:.0} logs/s  | search (including embedding) p50={:?}, p99={:?} (n={})",
            n as f64 / el.as_secs_f64(),
            percentile(&search_lat, 0.50),
            percentile(&search_lat, 0.99),
            search_lat.len()
        );
    }

    // ── 4) Edge case probe ──
    {
        let long_line = "ERROR stack overflow ".repeat(2000); // ~42,000 chars
        match engine.ingest_log(&long_line) {
            Ok(r) => println!(
                "[4] Extremely long (42k chars) log: OK (id={}, anomaly={})",
                r.id,
                r.anomaly.is_some()
            ),
            Err(e) => println!("[4] Extremely long (42k chars) log: ERROR — {e:#}"),
        }
        match engine.ingest_log("") {
            Ok(_) => println!("    Empty string log: OK"),
            Err(e) => println!("    Empty string log: ERROR — {e:#}"),
        }
    }

    // ── 5) HTTP path (batch 10 × 4 client threads) ──
    {
        let (addr, _h) = run_server(Arc::clone(&engine), "127.0.0.1:0", 4, None)?;
        let url = format!("http://{addr}/logs");
        let reqs_per_thread = 250usize;
        let batch = 10usize;
        let t = Instant::now();
        let threads: Vec<_> = (0..4)
            .map(|w| {
                let url = url.clone();
                std::thread::spawn(move || {
                    for r in 0..reqs_per_thread {
                        let logs: Vec<String> = (0..batch)
                            .map(|j| make_log(w * 1000 + r * batch + j))
                            .collect();
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
            "[5] HTTP /logs (4 clients, batch {batch}): {:.0} logs/s ({:.0} req/s)",
            total_logs as f64 / el.as_secs_f64(),
            (4 * reqs_per_thread) as f64 / el.as_secs_f64()
        );
    }

    // ── 6) Miss storm tolerance: hit path throughput during new template burst ──
    // Before fixing, the cache lock serialized even ONNX inference, causing the hit path to collapse to ~136 logs/s.
    {
        let stop = Arc::new(AtomicBool::new(false));
        let storm = {
            let engine = Arc::clone(&engine);
            let stop = Arc::clone(&stop);
            std::thread::spawn(move || {
                let mut embedded = 0u64;
                // Unique patterns with different token counts → consecutive forced cache misses
                for i in 0..400 {
                    if stop.load(Ordering::Relaxed) {
                        break;
                    }
                    let words: Vec<String> = (0..(3 + i % 120))
                        .map(|j| format!("storm{i}tok{j}"))
                        .collect();
                    engine.ingest_log(&words.join(" ")).unwrap();
                    embedded += 1;
                }
                embedded
            })
        };

        // Measure hit path throughput during the storm (for 2 seconds)
        std::thread::sleep(Duration::from_millis(100)); // Storm warm-up
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
            "[6] Miss storm tolerance: hit path {:.0} logs/s (processed {} new templates during storm)",
            hits as f64 / el.as_secs_f64(),
            storm_count
        );
    }

    // ── 7) Multi-threaded concurrent ingestion contention: measure global write lock scalability ──
    // N threads simultaneously call engine.ingest_log to output total throughput and linear
    // scalability against the number of threads. Maintains the cache hit path to expose only WAL lock contention.
    // The lower the scale multiplier compared to 1T, the greater the room for sharding improvement.
    {
        let parallelism = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);

        println!("\n[7] Multi-threaded concurrent ingestion contention (available_parallelism={parallelism})");
        println!("    threads | logs/s       | scale vs 1T");
        println!("    --------|--------------|------------");

        // Baseline 1-thread measurement (20,000 items)
        let single_tps = {
            let n = 20_000usize;
            let t = Instant::now();
            for i in 0..n {
                engine.ingest_log(&make_log(i))?;
            }
            n as f64 / t.elapsed().as_secs_f64()
        };
        println!("    {:7} | {:12.0} | {:.2}x", 1, single_tps, 1.0f64);

        // Measure 2T, 4T, available_parallelism T
        let thread_counts: Vec<usize> = {
            let mut v = vec![2usize, 4];
            if parallelism > 4 {
                v.push(parallelism);
            }
            v.dedup();
            v
        };

        for t_count in thread_counts {
            let n_per_thread = 10_000usize;
            let barrier = Arc::new(std::sync::Barrier::new(t_count));
            let threads: Vec<_> = (0..t_count)
                .map(|w| {
                    let engine = Arc::clone(&engine);
                    let barrier = Arc::clone(&barrier);
                    std::thread::spawn(move || {
                        barrier.wait(); // All threads start simultaneously
                        let t0 = Instant::now();
                        for i in 0..n_per_thread {
                            engine.ingest_log(&make_log(w * n_per_thread + i)).unwrap();
                        }
                        t0.elapsed()
                    })
                })
                .collect();
            let durations: Vec<Duration> =
                threads.into_iter().map(|th| th.join().unwrap()).collect();
            // Total throughput = total logs ÷ wall clock of the longest-running thread
            let max_dur = durations.iter().max().unwrap();
            let total_logs = t_count * n_per_thread;
            let tps = total_logs as f64 / max_dur.as_secs_f64();
            let scale = tps / single_tps;
            println!("    {:7} | {:12.0} | {:.2}x", t_count, tps, scale);
        }
    }

    let s = engine.stats();
    println!(
        "\n== Final stats: ingested={}, cache_hit_rate={:.4}, ring_windows={}, ring_vectors={} ==",
        s.ingested_total, s.cache_hit_rate, s.ring_windows, s.ring_vectors
    );
    std::fs::remove_dir_all(&data_dir).ok();
    Ok(())
}
