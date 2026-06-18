//! `turbolog scan` — read stdin to EOF and print a summary report.

use std::io::{BufRead, BufReader};

use anyhow::Result;
use serde::Serialize;

use crate::history::HistoryStore;
use crate::llm::LlmClient;
use crate::pipeline::{LineResult, LocalPipeline};

struct ScanEntry {
    line: String,
    result: LineResult,
}

#[derive(Serialize)]
struct JsonReport<'a> {
    lines_processed: usize,
    templates_found: usize,
    anomalies_total: usize,
    anomaly_rate_pct: f64,
    calibrated: bool,
    top_anomalies: Vec<JsonAnomaly<'a>>,
}

#[derive(Serialize)]
struct JsonAnomaly<'a> {
    score: f32,
    line: &'a str,
    template: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    explanation: Option<String>,
}

pub fn run_scan(
    pipeline: &mut LocalPipeline,
    format: &str,
    llm: Option<&LlmClient>,
    history: Option<&HistoryStore>,
) -> Result<()> {
    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    let mut entries: Vec<ScanEntry> = Vec::new();

    for line in reader.lines() {
        let line = line?;
        if line.is_empty() {
            continue;
        }
        match pipeline.process(&line) {
            Ok(result) => entries.push(ScanEntry { line, result }),
            Err(e) => eprintln!("turbolog: embedding error: {e}"),
        }
    }

    let total = entries.len();
    let anomalies: Vec<&ScanEntry> = entries.iter().filter(|e| e.result.is_anomaly).collect();
    let anomaly_count = anomalies.len();
    let rate = if total > 0 {
        anomaly_count as f64 / total as f64 * 100.0
    } else {
        0.0
    };

    let mut templates: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for e in &entries {
        templates.insert(&e.result.template);
    }

    // Top anomalies sorted by score descending (up to 10).
    let mut top: Vec<&ScanEntry> = anomalies;
    top.sort_by(|a, b| {
        let sa = a.result.score.unwrap_or(0.0);
        let sb = b.result.score.unwrap_or(0.0);
        sb.partial_cmp(&sa).unwrap_or(std::cmp::Ordering::Equal)
    });
    let top: Vec<&ScanEntry> = top.into_iter().take(10).collect();

    // Explain top 5, save all top 10 to history.
    let explanations: Vec<Option<String>> = top
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let score = e.result.score.unwrap_or(0.0);
            let explanation = if i < 5 {
                llm.and_then(|c| {
                    let ctx = history.and_then(|h| h.context_for(&e.result.template));
                    c.explain(&e.line, score, ctx.as_deref())
                })
            } else {
                None
            };
            if let Some(h) = history {
                let _ = h.insert(&e.result.template, &e.line, score, explanation.as_deref());
            }
            explanation
        })
        .collect();

    match format {
        "json" => {
            let report = JsonReport {
                lines_processed: total,
                templates_found: templates.len(),
                anomalies_total: anomaly_count,
                anomaly_rate_pct: rate,
                calibrated: pipeline.calibrated(),
                top_anomalies: top
                    .iter()
                    .zip(explanations.iter())
                    .map(|(e, explanation)| JsonAnomaly {
                        score: e.result.score.unwrap_or(0.0),
                        line: &e.line,
                        template: &e.result.template,
                        explanation: explanation.clone(),
                    })
                    .collect(),
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        _ => print_text_report(
            total,
            templates.len(),
            anomaly_count,
            rate,
            &top,
            &explanations,
            pipeline.calibrated(),
        ),
    }

    Ok(())
}

fn print_text_report(
    total: usize,
    templates: usize,
    anomalies: usize,
    rate: f64,
    top: &[&ScanEntry],
    explanations: &[Option<String>],
    calibrated: bool,
) {
    println!();
    println!("--- TurboLog Scan Report ---");
    println!("Lines processed : {total}");
    println!("Templates found : {templates}");
    println!("Anomalies       : {anomalies} ({rate:.2}%)");
    if !calibrated {
        println!(
            "Note            : Calibration incomplete (need 64 unique templates) — scores may be unreliable"
        );
    }
    if top.is_empty() {
        println!();
        println!("No anomalies detected.");
    } else {
        println!();
        println!("Top anomalies:");
        for (entry, explanation) in top.iter().zip(explanations.iter()) {
            let score = entry.result.score.unwrap_or(0.0);
            let display = if entry.line.chars().count() > 120 {
                format!("{}…", entry.line.chars().take(119).collect::<String>())
            } else {
                entry.line.clone()
            };
            println!("  [score={score:.2}] {display}");
            if let Some(exp) = explanation {
                println!("    └─ {exp}");
            }
        }
    }
    println!();
}
