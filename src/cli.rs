use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "turbolog",
    about = "Ultralight log anomaly detection — no API key, no Python, no bullshit"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the HTTP server daemon (default when no subcommand is given)
    Serve,
    /// Read stdin line-by-line and stream anomaly results with color highlighting
    Watch {
        /// Anomaly score floor override (default: auto-calibrated from data)
        #[arg(long)]
        threshold: Option<f32>,
        /// Explain anomalies using a local LLM (auto-detects Ollama or LM Studio)
        #[arg(long)]
        explain: bool,
        /// Local LLM base URL (e.g. http://localhost:11434). Overrides auto-detect.
        /// Can also be set via TURBOLOG_LLM_URL env var.
        #[arg(long)]
        llm_url: Option<String>,
        /// LLM model name (e.g. llama3.2, mistral). Overrides auto-detect default.
        /// Can also be set via TURBOLOG_LLM_MODEL env var.
        #[arg(long)]
        llm_model: Option<String>,
    },
    /// Read stdin to EOF and print a summary report
    Scan {
        /// Output format: "text" (default) or "json"
        #[arg(long, default_value = "text")]
        format: String,
        /// Anomaly score floor override (default: auto-calibrated from data).
        /// Use this to surface anomalies in a single small file, where the
        /// auto-threshold is set above every in-sample line by design.
        #[arg(long)]
        threshold: Option<f32>,
        /// Explain top anomalies using a local LLM (auto-detects Ollama or LM Studio)
        #[arg(long)]
        explain: bool,
        /// Local LLM base URL. Overrides auto-detect. Also: TURBOLOG_LLM_URL
        #[arg(long)]
        llm_url: Option<String>,
        /// LLM model name. Overrides auto-detect default. Also: TURBOLOG_LLM_MODEL
        #[arg(long)]
        llm_model: Option<String>,
    },
    /// Query stored anomaly history
    History {
        /// Look back this far: 7d, 24h, 1h, 30m (default: 7d)
        #[arg(long, default_value = "7d")]
        since: String,
        /// Filter by template substring
        #[arg(long)]
        template: Option<String>,
        /// Output format: "text" (default) or "json"
        #[arg(long, default_value = "text")]
        format: String,
        /// Maximum number of rows to show (default: 50)
        #[arg(long, default_value = "50")]
        limit: usize,
    },
    /// Real-time TUI dashboard connecting to a running TurboLog server
    Ui {
        /// TurboLog server URL
        #[arg(long, default_value = "http://localhost:8087")]
        server: String,
        /// Standalone mode: read stdin locally instead of connecting to a server
        #[arg(long)]
        standalone: bool,
    },
}
