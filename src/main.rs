//! TurboLog binary entry point — subcommands: serve (default), watch, scan, ui.
//!
//! Environment Variables (serve mode):
//!   TURBOLOG_PORT       (default: 8087)
//!   TURBOLOG_DATA_DIR   (default: ./data)
//!   TURBOLOG_MODEL_DIR  (default: ./models)
//!   TURBOLOG_EMBEDDERS  (default: 2)
//!   TURBOLOG_AUTH_TOKEN (optional)

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use clap::Parser;

use turbolog::cli::{Cli, Command};
use turbolog::embedded::make_embedder;
use turbolog::engine::{EngineConfig, TurboLogEngine};
use turbolog::http::run_server;
use turbolog::pipeline::LocalPipeline;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => run_serve(),
        Command::Watch { threshold } => run_watch_cmd(threshold),
        Command::Scan { format } => run_scan_cmd(&format),
        Command::Ui { server, standalone } => run_ui_cmd(&server, standalone),
    }
}

fn run_serve() -> anyhow::Result<()> {
    let port = env_or("TURBOLOG_PORT", "8087");
    let model_dir = PathBuf::from(env_or("TURBOLOG_MODEL_DIR", "./models"));
    let cfg = EngineConfig {
        data_dir: PathBuf::from(env_or("TURBOLOG_DATA_DIR", "./data")),
        ..EngineConfig::default()
    };

    let pool_size: usize = env_or("TURBOLOG_EMBEDDERS", "2")
        .parse()
        .unwrap_or(2)
        .max(1);
    let embedders = (0..pool_size)
        .map(|_| make_embedder(&model_dir))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let engine = Arc::new(TurboLogEngine::open(cfg, embedders)?);

    // Swap Daemon: seals window every 10s, sweeps expired retention chunks every hour.
    {
        let engine = Arc::clone(&engine);
        std::thread::spawn(move || {
            let interval = Duration::from_secs(engine.config().swap_interval_secs);
            let mut last_sweep_hour = 0i64;
            loop {
                std::thread::sleep(interval);
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

    let auth_token = std::env::var("TURBOLOG_AUTH_TOKEN")
        .ok()
        .filter(|t| !t.is_empty());
    let (addr, handles) = run_server(engine, &format!("0.0.0.0:{port}"), 4, auth_token)?;
    println!("TurboLog listening on http://{addr}");
    for handle in handles {
        let _ = handle.join();
    }
    Ok(())
}

fn run_watch_cmd(threshold: Option<f32>) -> anyhow::Result<()> {
    let model_dir = PathBuf::from(env_or("TURBOLOG_MODEL_DIR", "./models"));
    let embedder = make_embedder(&model_dir)?;
    let mut pipeline = LocalPipeline::new(embedder);
    eprintln!("[turbolog] streaming anomaly detection active (calibrating on first 64 templates)");
    turbolog::watch::run_watch(&mut pipeline, threshold)
}

fn run_scan_cmd(format: &str) -> anyhow::Result<()> {
    let model_dir = PathBuf::from(env_or("TURBOLOG_MODEL_DIR", "./models"));
    let embedder = make_embedder(&model_dir)?;
    let mut pipeline = LocalPipeline::new(embedder);
    turbolog::scan::run_scan(&mut pipeline, format)
}

fn run_ui_cmd(server: &str, standalone: bool) -> anyhow::Result<()> {
    #[cfg(feature = "tui")]
    {
        turbolog::tui::run_ui(server, standalone)
    }
    #[cfg(not(feature = "tui"))]
    {
        let _ = (server, standalone);
        anyhow::bail!("TUI support is not compiled in. Rebuild with:\n  cargo build --features tui")
    }
}
