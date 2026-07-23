use clap::{Parser, Subcommand, ValueEnum};

#[derive(Parser)]
#[command(
    name = "turbolog",
    about = "Ultralight log anomaly detection — no API key, no Python, no bullshit",
    arg_required_else_help = true
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Run the HTTP server daemon
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
        /// Print only anomalous lines (suppress normal and calibrating lines)
        #[arg(long)]
        only_anomalies: bool,
        /// Suppress status messages on stderr (errors are still shown)
        #[arg(short, long)]
        quiet: bool,
    },
    /// Read stdin to EOF and print a summary report
    Scan {
        /// Output format: "text" (default) or "json"
        #[arg(long, default_value = "text")]
        format: String,
        /// Anomaly score threshold override (default: auto-calibrated from data).
        /// Use this to tune sensitivity on small or low-cardinality inputs.
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
        /// Suppress status messages on stderr (errors are still shown)
        #[arg(short, long)]
        quiet: bool,
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
        /// Group entries by recurring template
        #[arg(long)]
        recurring: bool,
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
    /// Generate shell completion scripts (bash, zsh, fish, …)
    Completions {
        /// Target shell
        #[arg(value_enum)]
        shell: ShellName,
    },
}

#[derive(Clone, Copy, ValueEnum)]
pub enum ShellName {
    Bash,
    Elvish,
    Fish,
    PowerShell,
    Zsh,
}

impl From<ShellName> for clap_complete::Shell {
    fn from(s: ShellName) -> Self {
        match s {
            ShellName::Bash => Self::Bash,
            ShellName::Elvish => Self::Elvish,
            ShellName::Fish => Self::Fish,
            ShellName::PowerShell => Self::PowerShell,
            ShellName::Zsh => Self::Zsh,
        }
    }
}
