//! TurboLog binary entry point — subcommands: serve (default), watch, scan, ui.
//!
//! Environment Variables (serve mode):
//!   TURBOLOG_PORT       (default: 8087)
//!   TURBOLOG_DATA_DIR   (default: ./data)
//!   TURBOLOG_MODEL_DIR  (default: ./models)
//!   TURBOLOG_EMBEDDERS  (default: 2)
//!   TURBOLOG_AUTH_TOKEN (optional)

use std::path::PathBuf;

use clap::Parser;

use turbolog::cli::{Cli, Command};
use turbolog::embedded::make_embedder;
use turbolog::history::HistoryStore;
use turbolog::pipeline::LocalPipeline;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Serve) {
        Command::Serve => run_serve(),
        Command::Watch {
            threshold,
            explain,
            llm_url,
            llm_model,
        } => run_watch_cmd(threshold, explain, llm_url.as_deref(), llm_model.as_deref()),
        Command::Scan {
            format,
            explain,
            llm_url,
            llm_model,
        } => run_scan_cmd(&format, explain, llm_url.as_deref(), llm_model.as_deref()),
        Command::History {
            since,
            template,
            format,
            limit,
        } => run_history_cmd(&since, template.as_deref(), &format, limit),
        Command::Ui { server, standalone } => run_ui_cmd(&server, standalone),
    }
}

#[cfg(feature = "server")]
fn run_serve() -> anyhow::Result<()> {
    use std::sync::Arc;
    use std::time::Duration;

    use turbolog::engine::{EngineConfig, TurboLogEngine};
    use turbolog::http::run_server;

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
    let bind = env_or("TURBOLOG_BIND", "127.0.0.1");
    let (addr, handles) = run_server(engine, &format!("{bind}:{port}"), 4, auth_token)?;
    println!("TurboLog listening on http://{addr}");
    for handle in handles {
        let _ = handle.join();
    }
    Ok(())
}

#[cfg(not(feature = "server"))]
fn run_serve() -> anyhow::Result<()> {
    anyhow::bail!("Server mode is not compiled in. Rebuild with:\n  cargo build --features server")
}

fn run_watch_cmd(
    threshold: Option<f32>,
    explain: bool,
    llm_url: Option<&str>,
    llm_model: Option<&str>,
) -> anyhow::Result<()> {
    let model_dir = PathBuf::from(env_or("TURBOLOG_MODEL_DIR", "./models"));
    let embedder = make_embedder(&model_dir)?;
    let mut pipeline = LocalPipeline::new(embedder, threshold);

    let llm = if explain {
        let client = turbolog::llm::LlmClient::detect(llm_url, llm_model);
        match &client {
            Some(c) => eprintln!(
                "[turbolog] LLM connected: {} (model: {})",
                c.base_url(),
                c.model()
            ),
            None => {
                eprintln!("[turbolog] --explain: no local LLM found");
                eprintln!("  Ollama  → https://ollama.ai  (runs on :11434)");
                eprintln!("  LM Studio → https://lmstudio.ai  (runs on :1234)");
            }
        }
        client
    } else {
        None
    };

    let history = HistoryStore::open().ok();
    eprintln!("[turbolog] streaming anomaly detection active (calibrating on first 64 templates)");
    turbolog::watch::run_watch(&mut pipeline, llm.as_ref(), history.as_ref())
}

fn run_scan_cmd(
    format: &str,
    explain: bool,
    llm_url: Option<&str>,
    llm_model: Option<&str>,
) -> anyhow::Result<()> {
    let model_dir = PathBuf::from(env_or("TURBOLOG_MODEL_DIR", "./models"));
    let embedder = make_embedder(&model_dir)?;
    let mut pipeline = LocalPipeline::new(embedder, None);

    let llm = if explain {
        let client = turbolog::llm::LlmClient::detect(llm_url, llm_model);
        match &client {
            Some(c) => eprintln!(
                "[turbolog] LLM connected: {} (model: {})",
                c.base_url(),
                c.model()
            ),
            None => {
                eprintln!("[turbolog] --explain: no local LLM found");
                eprintln!("  Ollama    → https://ollama.ai  (runs on :11434)");
                eprintln!("  LM Studio → https://lmstudio.ai  (runs on :1234)");
            }
        }
        client
    } else {
        None
    };

    let history = HistoryStore::open().ok();
    turbolog::scan::run_scan(&mut pipeline, format, llm.as_ref(), history.as_ref())
}

fn run_history_cmd(
    since: &str,
    template: Option<&str>,
    format: &str,
    limit: usize,
) -> anyhow::Result<()> {
    let since_secs = parse_duration(since)
        .ok_or_else(|| anyhow::anyhow!("Invalid --since value '{since}'. Use: 7d, 24h, 30m"))?;

    let store = HistoryStore::open()?;
    let entries = store.query(&turbolog::history::HistoryQuery {
        since_secs: Some(since_secs),
        template: template.map(|s| s.to_string()),
        limit,
    })?;

    match format {
        "json" => {
            let json: Vec<serde_json::Value> = entries
                .iter()
                .map(|e| {
                    serde_json::json!({
                        "timestamp": e.timestamp,
                        "template": e.template,
                        "line": e.line,
                        "score": e.score,
                        "explanation": e.explanation,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        _ => {
            if entries.is_empty() {
                println!("No anomalies found in the last {since}.");
            } else {
                println!();
                println!("--- TurboLog History (last {since}) ---");
                println!("  {:<20}  {:<6}  line", "time", "score");
                println!("  {}", "-".repeat(72));
                for e in &entries {
                    let dt = format_timestamp(e.timestamp);
                    let display = if e.line.chars().nth(60).is_some() {
                        format!("{}…", e.line.chars().take(59).collect::<String>())
                    } else {
                        e.line.clone()
                    };
                    println!("  {:<20}  {:<6.2}  {}", dt, e.score, display);
                    if let Some(ref exp) = e.explanation {
                        println!("    └─ {exp}");
                    }
                }
                println!();
                println!(
                    "Total: {} anomal{}",
                    entries.len(),
                    if entries.len() == 1 { "y" } else { "ies" }
                );
                println!();
            }
        }
    }

    Ok(())
}

fn parse_duration(s: &str) -> Option<i64> {
    let s = s.trim();
    if let Some(n) = s.strip_suffix('d') {
        n.parse::<i64>().ok().and_then(|v| v.checked_mul(86_400))
    } else if let Some(n) = s.strip_suffix('h') {
        n.parse::<i64>().ok().and_then(|v| v.checked_mul(3_600))
    } else if let Some(n) = s.strip_suffix('m') {
        n.parse::<i64>().ok().and_then(|v| v.checked_mul(60))
    } else {
        s.parse::<i64>().ok()
    }
}

fn format_timestamp(ts: i64) -> String {
    // Show as "YYYY-MM-DD HH:MM:SS" UTC using only std (no chrono).
    let secs = ts.max(0) as u64;
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86_400;
    // Days since Unix epoch → Gregorian. Tomohiko Sakamoto's algorithm.
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02}")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Civil calendar from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if mo <= 2 { y + 1 } else { y };
    (y, mo, d)
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
