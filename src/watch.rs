//! `turbolog watch` — real-time stdin streaming with anomaly highlighting.

use std::io::{BufRead, BufReader, IsTerminal};

use anyhow::Result;

use crate::llm::LlmClient;
use crate::pipeline::{LineResult, LocalPipeline};

const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";

pub fn run_watch(pipeline: &mut LocalPipeline, llm: Option<&LlmClient>) -> Result<()> {
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
            Ok(result) => print_result(&line, &result, use_color, llm),
            Err(e) => eprintln!("turbolog: embedding error: {e}"),
        }
    }

    Ok(())
}

fn print_result(line: &str, result: &LineResult, color: bool, llm: Option<&LlmClient>) {
    if result.is_anomaly {
        if let Some(score) = result.score {
            if color {
                println!("{RED}[ANOMALY {score:.2}]{RESET} {line}");
            } else {
                println!("[ANOMALY {score:.2}] {line}");
            }
            if let Some(client) = llm {
                match client.explain(line, score) {
                    Some(explanation) => {
                        if color {
                            println!("  {CYAN}└─ {explanation}{RESET}");
                        } else {
                            println!("  └─ {explanation}");
                        }
                    }
                    None => {
                        if color {
                            println!("  {DIM}└─ (LLM explanation unavailable){RESET}");
                        }
                    }
                }
            }
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
