//! TurboLog binary entry point — subcommands: watch, scan, history, ui, serve.
//!
//! Environment Variables (serve mode):
//!   TURBOLOG_PORT       (default: 8087)
//!   TURBOLOG_DATA_DIR   (default: ./data)
//!   TURBOLOG_MODEL_DIR  (default: ./models)
//!   TURBOLOG_EMBEDDERS  (default: 2)
//!   TURBOLOG_AUTH_TOKEN (optional)

use std::path::PathBuf;

use clap::CommandFactory;
use clap::Parser;
use turbolog::cli::{Cli, Command};

/// Exit 0 — success, no anomalies detected.
pub const EXIT_OK: i32 = 0;
/// Exit 1 — anomalies detected (watch/scan).
pub const EXIT_ANOMALIES: i32 = 1;

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key).unwrap_or_else(|_| default.to_string())
}

fn main() {
    let code = match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("turbolog: {e:#}");
            2
        }
    };
    std::process::exit(code);
}

fn run() -> anyhow::Result<i32> {
    let cli = Cli::parse();
    match cli.command {
        Command::Serve => {
            run_serve()?;
            Ok(EXIT_OK)
        }
        Command::Watch {
            threshold,
            explain,
            llm_url,
            llm_model,
            only_anomalies,
            quiet,
        } => run_watch_cmd(
            threshold,
            explain,
            llm_url.as_deref(),
            llm_model.as_deref(),
            only_anomalies,
            quiet,
        ),
        Command::Scan {
            format,
            threshold,
            explain,
            llm_url,
            llm_model,
            quiet,
        } => run_scan_cmd(
            &format,
            threshold,
            explain,
            llm_url.as_deref(),
            llm_model.as_deref(),
            quiet,
        ),
        Command::History {
            since,
            template,
            format,
            limit,
            recurring,
        } => {
            run_history_cmd(&since, template.as_deref(), &format, limit, recurring)?;
            Ok(EXIT_OK)
        }
        Command::Ui { server, standalone } => {
            run_ui_cmd(&server, standalone)?;
            Ok(EXIT_OK)
        }
        Command::Completions { shell } => {
            let shell: clap_complete::Shell = shell.into();
            clap_complete::generate(
                shell,
                &mut Cli::command(),
                "turbolog",
                &mut std::io::stdout(),
            );
            Ok(EXIT_OK)
        }
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
        .map(|_| turbolog::embedded::make_embedder(&model_dir))
        .collect::<anyhow::Result<Vec<_>>>()?;
    let engine = Arc::new(TurboLogEngine::open(cfg, embedders)?);

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
    only_anomalies: bool,
    quiet: bool,
) -> anyhow::Result<i32> {
    use turbolog::embedded::make_embedder;
    use turbolog::history::HistoryStore;
    use turbolog::pipeline::LocalPipeline;
    use turbolog::watch::WatchOptions;

    let model_dir = PathBuf::from(env_or("TURBOLOG_MODEL_DIR", "./models"));
    let embedder = make_embedder(&model_dir)?;
    let mut pipeline = LocalPipeline::new(embedder, threshold);

    let llm = if explain {
        setup_llm(llm_url, llm_model, quiet)
    } else {
        None
    };

    let history = HistoryStore::open().ok();
    if !quiet {
        eprintln!(
            "[turbolog] streaming anomaly detection active (calibrating: 64 templates or 500 lines with >=3 templates)"
        );
    }

    let stats = turbolog::watch::run_watch(
        &mut pipeline,
        llm.as_ref(),
        history.as_ref(),
        WatchOptions {
            only_anomalies,
            quiet,
        },
    )?;

    Ok(exit_for_anomalies(stats))
}

fn run_scan_cmd(
    format: &str,
    threshold: Option<f32>,
    explain: bool,
    llm_url: Option<&str>,
    llm_model: Option<&str>,
    quiet: bool,
) -> anyhow::Result<i32> {
    use turbolog::embedded::make_embedder;
    use turbolog::history::HistoryStore;
    use turbolog::pipeline::LocalPipeline;

    let model_dir = PathBuf::from(env_or("TURBOLOG_MODEL_DIR", "./models"));
    let embedder = make_embedder(&model_dir)?;
    let mut pipeline = LocalPipeline::new(embedder, threshold);

    let llm = if explain {
        setup_llm(llm_url, llm_model, quiet)
    } else {
        None
    };

    let history = HistoryStore::open().ok();
    let stats = turbolog::scan::run_scan(&mut pipeline, format, llm.as_ref(), history.as_ref())?;

    Ok(if stats.anomaly_count > 0 {
        EXIT_ANOMALIES
    } else {
        EXIT_OK
    })
}

fn run_history_cmd(
    since: &str,
    template: Option<&str>,
    format: &str,
    limit: usize,
    recurring: bool,
) -> anyhow::Result<()> {
    use turbolog::history::{HistoryQuery, HistoryStore};

    let since_secs = parse_duration(since)
        .ok_or_else(|| anyhow::anyhow!("Invalid --since value '{since}'. Use: 7d, 24h, 30m"))?;

    let store = HistoryStore::open()?;
    let query = HistoryQuery {
        since_secs: Some(since_secs),
        template: template.map(|s| s.to_string()),
        limit,
    };

    if recurring {
        let entries = store.query_recurring(&query)?;
        match format {
            "json" => {
                let json: Vec<serde_json::Value> = entries
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "template": e.template,
                            "count": e.count,
                            "last_seen": e.last_seen,
                            "max_score": e.max_score,
                            "sample_line": e.sample_line,
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&json)?);
            }
            _ => {
                if entries.is_empty() {
                    println!("No recurring anomalies found in the last {since}.");
                } else {
                    println!();
                    println!("--- TurboLog Recurring History (last {since}) ---");
                    println!(
                        "  {:<6}  {:<20}  {:<9}  template",
                        "count", "last seen", "max score"
                    );
                    println!("  {}", "-".repeat(80));
                    for e in &entries {
                        let dt = format_timestamp(e.last_seen);
                        let template = truncate_display(&e.template, 45);
                        let sample = truncate_display(&e.sample_line, 68);
                        println!(
                            "  {:<6}  {:<20}  {:<9.2}  {}",
                            e.count, dt, e.max_score, template
                        );
                        println!("    └─ sample: {sample}");
                    }
                    println!();
                    println!(
                        "Total: {} recurring template{}",
                        entries.len(),
                        if entries.len() == 1 { "" } else { "s" }
                    );
                    println!();
                }
            }
        }
        return Ok(());
    }

    let entries = store.query(&query)?;

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
                    let display = truncate_display(&e.line, 59);
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

fn setup_llm(
    llm_url: Option<&str>,
    llm_model: Option<&str>,
    quiet: bool,
) -> Option<turbolog::llm::LlmClient> {
    let client = turbolog::llm::LlmClient::detect(llm_url, llm_model);
    if !quiet {
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
    }
    client
}

fn exit_for_anomalies(stats: turbolog::watch::WatchStats) -> i32 {
    if stats.anomaly_count > 0 {
        EXIT_ANOMALIES
    } else {
        EXIT_OK
    }
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
    let secs = ts.max(0) as u64;
    let s = secs % 60;
    let m = (secs / 60) % 60;
    let h = (secs / 3600) % 24;
    let days = secs / 86_400;
    let (year, month, day) = days_to_ymd(days);
    format!("{year:04}-{month:02}-{day:02} {h:02}:{m:02}:{s:02}")
}

fn days_to_ymd(days: u64) -> (u64, u64, u64) {
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

fn truncate_display(s: &str, max: usize) -> String {
    if s.chars().nth(max).is_some() {
        format!("{}…", s.chars().take(max).collect::<String>())
    } else {
        s.to_string()
    }
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
