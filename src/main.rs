//! TurboLog server daemon.
//!
//! TurboLog Server Daemon.
//!
//! Environment Variables: TURBOLOG_PORT (default: 8087), TURBOLOG_DATA_DIR (default: ./data),
//! TURBOLOG_MODEL_DIR (default: ./models), TURBOLOG_EMBEDDERS (default: 2),
//! TURBOLOG_MAX_INFLIGHT (default: 0 = unlimited), TURBOLOG_AUTH_TOKEN (optional).

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use turbolog::engine::{EngineConfig, TurboLogEngine};
use turbolog::http::{run_server_with, ServerConfig};
use turbolog::Embedder;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() -> anyhow::Result<()> {
    let port = env_or("TURBOLOG_PORT", "8087");
    let model_dir = PathBuf::from(env_or("TURBOLOG_MODEL_DIR", "./models"));
    let cfg = EngineConfig {
        data_dir: PathBuf::from(env_or("TURBOLOG_DATA_DIR", "./data")),
        ..EngineConfig::default()
    };

    // Embedder pool: bounds parallel cache-miss inference (each slot ≈ 90 MB ONNX session).
    let pool_size: usize = env_or("TURBOLOG_EMBEDDERS", "2").parse().unwrap_or(2).max(1);
    let embedders = (0..pool_size)
        .map(|_| {
            Embedder::new(
                model_dir.join("model.onnx"),
                model_dir.join("tokenizer.json"),
            )
        })
        .collect::<anyhow::Result<Vec<_>>>()?;
    let engine = Arc::new(TurboLogEngine::open(cfg, embedders)?);

    // Graceful shutdown flag — set by SIGTERM/SIGINT handlers below.
    let shutdown = Arc::new(AtomicBool::new(false));
    signal_hook::flag::register(signal_hook::consts::SIGTERM, Arc::clone(&shutdown))?;
    signal_hook::flag::register(signal_hook::consts::SIGINT, Arc::clone(&shutdown))?;

    // Swap Daemon: Seals window every 10s + unlinks expired retention chunks every hour.
    // Polls the shutdown flag so it exits cleanly and seals the current window on exit.
    {
        let engine = Arc::clone(&engine);
        let shutdown = Arc::clone(&shutdown);
        std::thread::spawn(move || {
            let interval = Duration::from_secs(engine.config().swap_interval_secs);
            let poll = Duration::from_millis(200);
            let mut last_sweep_hour = 0i64;
            loop {
                // Sleep in small increments so we react to shutdown quickly.
                let mut elapsed = Duration::ZERO;
                while elapsed < interval {
                    if shutdown.load(Ordering::Relaxed) {
                        // Seal the in-memory window before exiting so no data is lost.
                        if let Err(e) = engine.swap_tick() {
                            eprintln!("swap_tick (final) failed: {e:#}");
                        }
                        return;
                    }
                    std::thread::sleep(poll);
                    elapsed += poll;
                }

                if let Err(e) = engine.swap_tick() {
                    eprintln!("swap_tick failed: {e:#}");
                }
                let hour = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64 / 3600)
                    .unwrap_or(0);
                if hour != last_sweep_hour {
                    last_sweep_hour = hour;
                    match engine.sweep_chunks() {
                        Ok(0) => {}
                        Ok(n) => println!("Deleted {n} expired retention chunks"),
                        Err(e) => eprintln!("sweep failed: {e:#}"),
                    }
                }
            }
        });
    }

    // Optional bearer-token auth — leave unset only on trusted networks.
    let auth_token = std::env::var("TURBOLOG_AUTH_TOKEN").ok().filter(|t| !t.is_empty());
    let max_inflight: usize = env_or("TURBOLOG_MAX_INFLIGHT", "0").parse().unwrap_or(0);

    let (addr, handles) = run_server_with(
        Arc::clone(&engine),
        ServerConfig {
            addr: format!("0.0.0.0:{port}"),
            workers: 4,
            auth_token,
            max_inflight,
            shutdown: Arc::clone(&shutdown),
        },
    )?;
    println!("TurboLog listening on http://{addr}");

    for handle in handles {
        let _ = handle.join();
    }
    println!("TurboLog shut down cleanly.");
    Ok(())
}
