//! `turbolog diagnose` — time-window anomaly pattern summary from local history.

use anyhow::Result;
use serde::Serialize;

use crate::history::{HistoryStore, TemplateSummary};
use crate::llm::LlmClient;

#[derive(Serialize)]
struct JsonDiagnose<'a> {
    since: &'a str,
    total_anomalies: u64,
    patterns: Vec<JsonPattern>,
    #[serde(skip_serializing_if = "Option::is_none")]
    summary: Option<String>,
}

#[derive(Serialize)]
struct JsonPattern {
    count: u64,
    avg_score: f32,
    template: String,
    last_seen: i64,
    sample_line: String,
}

/// Returns `true` when at least one anomaly pattern was found in the window.
pub fn run_diagnose(
    store: &HistoryStore,
    since_label: &str,
    since_secs: i64,
    format: &str,
    limit: usize,
    llm: Option<&LlmClient>,
    quiet: bool,
) -> Result<bool> {
    let patterns = store.top_templates(since_secs, limit)?;
    let total: u64 = patterns.iter().map(|p| p.count).sum();

    let summary = if let Some(client) = llm {
        if patterns.is_empty() {
            None
        } else {
            let prompt = build_diagnose_prompt(since_label, &patterns);
            client.summarize(&prompt)
        }
    } else {
        None
    };

    if !quiet && llm.is_some() && summary.is_none() && !patterns.is_empty() {
        eprintln!("[turbolog] diagnose --explain: LLM summary unavailable");
    }

    match format {
        "json" => {
            let report = JsonDiagnose {
                since: since_label,
                total_anomalies: total,
                patterns: patterns.iter().map(pattern_to_json).collect(),
                summary,
            };
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
        _ => print_text_report(since_label, total, &patterns, summary.as_deref()),
    }

    Ok(total > 0)
}

fn pattern_to_json(p: &TemplateSummary) -> JsonPattern {
    JsonPattern {
        count: p.count,
        avg_score: p.avg_score,
        template: p.template.clone(),
        last_seen: p.last_seen,
        sample_line: p.sample_line.clone(),
    }
}

fn print_text_report(
    since: &str,
    total: u64,
    patterns: &[TemplateSummary],
    summary: Option<&str>,
) {
    println!();
    println!("--- TurboLog Diagnose (last {since}) ---");
    if patterns.is_empty() {
        println!("No anomalies recorded in this window.");
        println!();
        return;
    }

    println!("Total anomalies : {total}");
    println!();
    println!("Patterns (by frequency):");
    for p in patterns {
        let sample = truncate_line(&p.sample_line, 80);
        println!(
            "  {}×  [avg {:.2}]  {}",
            p.count, p.avg_score, p.template
        );
        println!("       e.g. {sample}");
    }

    if let Some(top) = patterns.first() {
        println!();
        println!("Likely focus:");
        println!(
            "  → \"{}\" ({} occurrence{})",
            top.template,
            top.count,
            if top.count == 1 { "" } else { "s" }
        );
    }

    if let Some(s) = summary {
        println!();
        println!("Summary:");
        println!("  {s}");
    }
    println!();
}

fn build_diagnose_prompt(since: &str, patterns: &[TemplateSummary]) -> String {
    let mut lines = vec![format!(
        "Anomaly patterns from the last {since} (grouped by log template):"
    )];
    for (i, p) in patterns.iter().take(5).enumerate() {
        lines.push(format!(
            "{}. {}× avg_score={:.2} template=\"{}\" example=\"{}\"",
            i + 1,
            p.count,
            p.avg_score,
            p.template,
            truncate_line(&p.sample_line, 120)
        ));
    }
    lines.push(
        "In 2-3 sentences: what is the most likely root cause and what should the developer check first? \
         No markdown, no preamble."
            .to_string(),
    );
    lines.join("\n")
}

fn truncate_line(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max_chars.saturating_sub(1)).collect::<String>())
    }
}
