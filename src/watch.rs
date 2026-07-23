//! `turbolog watch` — real-time stdin streaming with anomaly highlighting.

use std::io::{BufRead, BufReader, IsTerminal};

use anyhow::Result;

use crate::history::HistoryStore;
use crate::llm::LlmClient;
use crate::pipeline::{LineResult, LocalPipeline};

const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

pub struct WatchOptions {
    pub only_anomalies: bool,
    pub quiet: bool,
}

pub struct WatchStats {
    pub anomaly_count: u64,
}

struct CalibrationDisplay {
    progress: usize,
    target: usize,
}

pub fn run_watch(
    pipeline: &mut LocalPipeline,
    llm: Option<&LlmClient>,
    history: Option<&HistoryStore>,
    opts: WatchOptions,
) -> Result<WatchStats> {
    let use_color =
        std::env::var("NO_COLOR").is_err() && std::io::stderr().is_terminal() && !opts.quiet;
    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    let mut stats = WatchStats { anomaly_count: 0 };

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            if !opts.only_anomalies {
                println!();
            }
            continue;
        }

        let was_calibrated = pipeline.calibrated();
        match pipeline.process(&line) {
            Ok(result) => {
                if result.is_anomaly {
                    stats.anomaly_count += 1;
                }
                handle_result(
                    &line,
                    &result,
                    CalibrationDisplay {
                        progress: pipeline.calibration_progress(),
                        target: pipeline.calibration_target(),
                    },
                    use_color,
                    llm,
                    history,
                    &opts,
                );
            }
            Err(e) => eprintln!("turbolog: embedding error: {e}"),
        }
        if !opts.quiet && !was_calibrated && pipeline.calibrated() {
            print_calibration_complete(use_color, pipeline.effective_threshold());
        }
    }

    Ok(stats)
}

fn handle_result(
    line: &str,
    result: &LineResult,
    calibration: CalibrationDisplay,
    color: bool,
    llm: Option<&LlmClient>,
    history: Option<&HistoryStore>,
    opts: &WatchOptions,
) {
    if result.is_anomaly {
        let score = result.score.unwrap_or(0.0);
        if color {
            println!("{RED}[ANOMALY {score:.2}]{RESET} {line}");
        } else {
            println!("[ANOMALY {score:.2}] {line}");
        }

        if let Some(client) = llm {
            let ctx = history.and_then(|h| h.context_for(&result.template));
            match client.explain(line, score, ctx.as_deref()) {
                Some(explanation) => {
                    if color {
                        println!("  {CYAN}└─ {explanation}{RESET}");
                    } else {
                        println!("  └─ {explanation}");
                    }
                    if let Some(h) = history {
                        let _ = h.insert(&result.template, line, score, Some(&explanation));
                    }
                }
                None => {
                    if color {
                        println!("  {DIM}└─ (LLM explanation unavailable){RESET}");
                    } else {
                        println!("  └─ (LLM explanation unavailable)");
                    }
                    if let Some(h) = history {
                        let _ = h.insert(&result.template, line, score, None);
                    }
                }
            }
        } else if let Some(h) = history {
            let _ = h.insert(&result.template, line, score, None);
        }
    } else if result.score.is_none() {
        if opts.only_anomalies {
            return;
        }
        if opts.quiet {
            println!("{line}");
            return;
        }
        if color {
            println!(
                "{DIM}[calibrating {}/{}]{RESET} {line}",
                calibration.progress, calibration.target
            );
        } else {
            println!(
                "[calibrating {}/{}] {line}",
                calibration.progress, calibration.target
            );
        }
    } else if !opts.only_anomalies {
        println!("{line}");
    }
}

/// Prints a one-time status line to stderr when calibration completes.
pub fn print_calibration_complete(use_color: bool, threshold: Option<f32>) {
    let suffix = threshold
        .map(|value| format!(" (threshold={value:.3})"))
        .unwrap_or_default();
    if use_color {
        eprintln!(
            "{YELLOW}[turbolog] calibration complete — anomaly detection active{suffix}{RESET}"
        );
    } else {
        eprintln!("[turbolog] calibration complete — anomaly detection active{suffix}");
    }
}
