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
    },
    /// Read stdin to EOF and print a summary report
    Scan {
        /// Output format: "text" (default) or "json"
        #[arg(long, default_value = "text")]
        format: String,
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
