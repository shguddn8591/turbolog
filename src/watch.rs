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

pub fn run_watch(
    pipeline: &mut LocalPipeline,
    llm: Option<&LlmClient>,
    history: Option<&HistoryStore>,
) -> Result<()> {
    let use_color = std::env::var("NO_COLOR").is_err() && std::io::stderr().is_terminal();
    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            println!();
            continue;
        }

        match pipeline.process(&line) {
            Ok(result) => handle_result(&line, &result, use_color, llm, history),
            Err(e) => eprintln!("turbolog: embedding error: {e}"),
        }
    }

    Ok(())
}

fn handle_result(
    line: &str,
    result: &LineResult,
    color: bool,
    llm: Option<&LlmClient>,
    history: Option<&HistoryStore>,
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
        if color {
            println!("{DIM}[calibrating]{RESET} {line}");
        } else {
            println!("[calibrating] {line}");
        }
    } else {
        println!("{line}");
    }
}

/// Prints a one-time status line to stderr when calibration completes.
pub fn print_calibration_complete(use_color: bool) {
    if use_color {
        eprintln!("{YELLOW}[turbolog] calibration complete — anomaly detection active{RESET}");
    } else {
        eprintln!("[turbolog] calibration complete — anomaly detection active");
    }
}
