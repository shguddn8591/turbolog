//! `turbolog watch` — real-time stdin streaming with anomaly highlighting.

use std::io::{BufRead, BufReader, IsTerminal};

use anyhow::Result;

use crate::pipeline::{LineResult, LocalPipeline};

const RED: &str = "\x1b[31m";
const DIM: &str = "\x1b[2m";
const RESET: &str = "\x1b[0m";
const YELLOW: &str = "\x1b[33m";

pub fn run_watch(pipeline: &mut LocalPipeline) -> Result<()> {
    let use_color = std::env::var() && std::io::stderr().is_terminal();
    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            println!();
            continue;
        }

        match pipeline.process(&line) {
            Ok(result) => print_result(&line, &result, use_color),
            Err(e) => eprintln!("turbolog: embedding error: {e}"),
        }
    }

    Ok(())
}

fn print_result(line: &str, result: &LineResult, color: bool) {
    if result.is_anomaly {
        if let Some(score) = result.score {
            if color {
                println!("{RED}[ANOMALY {score:.2}]{RESET} {line}");
            } else {
                println!("[ANOMALY {score:.2}] {line}");
            }
        }
    } else if result.score.is_none() {
        // Still calibrating — print with dim marker so users know we're warming up.
        if color {
            println!("{DIM}[calibrating]{RESET} {line}");
        } else {
            println!("[calibrating] {line}");
        }
    } else {
        // Normal — print line as-is; anomalies stand out by contrast.
        if color && result.score.map(|s| s > 0.0).unwrap_or(false) {
            // Slightly dim normal lines only when a score exists, so anomalies pop.
            println!("{DIM}{line}{RESET}");
        } else {
            println!("{line}");
        }
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
