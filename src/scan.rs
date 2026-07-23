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

struct TextReport<'a> {
    total: usize,
    templates: usize,
    anomalies: usize,
    rate: f64,
    top: &'a [&'a ScanEntry],
    explanations: &'a [Option<String>],
    calibration_status: CalibrationStatus,
    effective_threshold: Option<f32>,
}

#[derive(Clone, Copy)]
enum CalibrationStatus {
    StreamingTrigger,
    EofFinalized,
    NotEnoughData,
}

impl CalibrationStatus {
    fn as_json(self) -> &'static str {
        match self {
            Self::StreamingTrigger => "streaming_trigger",
            Self::EofFinalized => "eof_finalized",
            Self::NotEnoughData => "not_enough_data",
        }
    }
}

pub struct ScanStats {
    pub anomaly_count: usize,
}

#[derive(Serialize)]
struct JsonReport<'a> {
    lines_processed: usize,
    templates_found: usize,
    anomalies_total: usize,
    anomaly_rate_pct: f64,
    calibrated: bool,
    calibration_status: &'static str,
    effective_threshold: Option<f32>,
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
) -> Result<ScanStats> {
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

    // End of input: if the streaming triggers never fired (small batch), force-calibrate
    // on the collected templates, then re-score the lines seen before the detector existed.
    // Uses `rescore` (template -> cached vector) instead of `process` so this doesn't
    // re-feed every line into Drain's stateful tree a second time.
    let was_calibrated = pipeline.calibrated();
    let calibrated = pipeline.finalize();
    if calibrated {
        for entry in &mut entries {
            if entry.result.score.is_none() {
                match pipeline.rescore(&entry.result.template) {
                    Ok(Some(result)) => entry.result = result,
                    Ok(None) => {}
                    Err(e) => eprintln!("turbolog: embedding error: {e}"),
                }
            }
        }
    }
    let calibration_status = if was_calibrated {
        CalibrationStatus::StreamingTrigger
    } else if calibrated {
        CalibrationStatus::EofFinalized
    } else {
        CalibrationStatus::NotEnoughData
    };
    let effective_threshold = pipeline.effective_threshold();

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
                calibrated,
                calibration_status: calibration_status.as_json(),
                effective_threshold,
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
        _ => print_text_report(TextReport {
            total,
            templates: templates.len(),
            anomalies: anomaly_count,
            rate,
            top: &top,
            explanations: &explanations,
            calibration_status,
            effective_threshold,
        }),
    }

    Ok(ScanStats { anomaly_count })
}

fn print_text_report(report: TextReport<'_>) {
    println!();
    println!("--- TurboLog Scan Report ---");
    println!("Lines processed : {}", report.total);
    println!("Templates found : {}", report.templates);
    println!(
        "Anomalies       : {} ({:.2}%)",
        report.anomalies, report.rate
    );
    match report.calibration_status {
        CalibrationStatus::StreamingTrigger => {
            if let Some(threshold) = report.effective_threshold {
                println!(
                    "Calibration     : complete (streaming trigger, threshold={threshold:.3})"
                );
            } else {
                println!("Calibration     : complete (streaming trigger)");
            }
        }
        CalibrationStatus::EofFinalized => {
            if let Some(threshold) = report.effective_threshold {
                println!("Calibration     : complete (EOF finalize, threshold={threshold:.3})");
            } else {
                println!("Calibration     : complete (EOF finalize)");
            }
        }
        CalibrationStatus::NotEnoughData => {
            println!(
                "Calibration     : incomplete (need at least 8 unique log templates at EOF; scores unavailable)"
            );
        }
    }
    if report.top.is_empty() {
        println!();
        println!("No anomalies detected.");
    } else {
        println!();
        println!("Top anomalies:");
        for (entry, explanation) in report.top.iter().zip(report.explanations.iter()) {
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
