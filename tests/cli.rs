//! CLI integration tests for `turbolog watch` and `turbolog scan`.
//!
//! These tests invoke the compiled binary via `std::process::Command`.
//! They are skipped when the ONNX model is not present (same guard as other tests).

use std::io::Write;
use std::process::{Command, ExitStatus, Stdio};

fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_turbolog"))
}

fn models_available() -> bool {
    std::path::Path::new("models/model.onnx").exists()
        && std::path::Path::new("models/tokenizer.json").exists()
}

fn exit_code(status: ExitStatus) -> Option<i32> {
    status.code()
}

/// Feed lines to turbolog watch and return (exit_code, stdout, stderr).
fn pipe_to_watch(input: &str, extra_args: &[&str]) -> (Option<i32>, String, String) {
    let mut cmd = Command::new(binary())
        .arg("watch")
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("NO_COLOR", "1")
        .env("TURBOLOG_MODEL_DIR", "./models")
        .spawn()
        .expect("failed to spawn turbolog");

    cmd.stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();

    let output = cmd.wait_with_output().unwrap();
    (
        exit_code(output.status),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn pipe_to_scan(input: &str, format: &str) -> (Option<i32>, String, String) {
    pipe_to_scan_args(input, format, &[])
}

fn pipe_to_scan_args(
    input: &str,
    format: &str,
    extra_args: &[&str],
) -> (Option<i32>, String, String) {
    let mut cmd = Command::new(binary())
        .arg("scan")
        .arg("--format")
        .arg(format)
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("TURBOLOG_MODEL_DIR", "./models")
        .spawn()
        .expect("failed to spawn turbolog");

    cmd.stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();

    let output = cmd.wait_with_output().unwrap();
    (
        exit_code(output.status),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

#[test]
fn watch_exits_zero_on_empty_input() {
    if !models_available() {
        eprintln!("skipping cli::watch_exits_zero_on_empty_input — models not present");
        return;
    }
    let (code, _stdout, _stderr) = pipe_to_watch("", &[]);
    assert_eq!(code, Some(0), "turbolog watch should exit 0 on empty stdin");
}

#[test]
fn watch_outputs_lines_for_input() {
    if !models_available() {
        eprintln!("skipping cli::watch_outputs_lines_for_input — models not present");
        return;
    }
    let input = "2024-01-01 INFO server started on port 8080\n\
                 2024-01-01 INFO request received from 192.168.1.1\n\
                 2024-01-01 INFO server started on port 8080\n";
    let (code, stdout, _stderr) = pipe_to_watch(input, &[]);
    assert_eq!(
        code,
        Some(0),
        "turbolog watch should exit 0 when no anomalies"
    );
    let out_lines = stdout.lines().count();
    assert!(
        out_lines >= 3,
        "expected at least 3 output lines, got {out_lines}: {stdout}"
    );
}

#[test]
fn watch_only_anomalies_suppresses_normal_lines() {
    if !models_available() {
        eprintln!(
            "skipping cli::watch_only_anomalies_suppresses_normal_lines — models not present"
        );
        return;
    }
    let input = "user login OK\nrequest processed in 12ms\n";
    let (code, stdout, _stderr) = pipe_to_watch(input, &["--only-anomalies", "--quiet"]);
    assert_eq!(code, Some(0));
    assert!(
        !stdout.contains("user login OK"),
        "only-anomalies should suppress normal lines: {stdout}"
    );
}

#[test]
fn watch_exits_one_when_anomaly_detected() {
    if !models_available() {
        eprintln!("skipping cli::watch_exits_one_when_anomaly_detected — models not present");
        return;
    }
    // Calibrate via line budget (500 lines, 3 templates), then emit a clear anomaly.
    let seeds = [
        "user login OK",
        "request processed in 12ms",
        "cache hit while fetching key",
    ];
    let mut input = String::new();
    for i in 0..510 {
        input.push_str(seeds[i % seeds.len()]);
        input.push('\n');
    }
    input.push_str("OOM killer activated for pid 4821\n");
    let (code, stdout, _stderr) = pipe_to_watch(&input, &["--threshold", "0", "--quiet"]);
    assert_eq!(
        code,
        Some(1),
        "watch should exit 1 when anomalies are detected"
    );
    assert!(
        stdout.contains("ANOMALY") || stdout.contains("OOM"),
        "expected anomaly output: {}",
        stdout.lines().last().unwrap_or("")
    );
}

#[test]
fn scan_text_report_contains_summary() {
    if !models_available() {
        eprintln!("skipping cli::scan_text_report_contains_summary — models not present");
        return;
    }
    let input = "INFO user login success\n".repeat(5);
    let (code, stdout, _stderr) = pipe_to_scan(&input, "text");
    assert_eq!(
        code,
        Some(0),
        "turbolog scan should exit 0 when no anomalies"
    );
    assert!(
        stdout.contains("Lines processed"),
        "text report should contain 'Lines processed': {stdout}"
    );
    assert!(
        stdout.contains("Templates found"),
        "text report should contain 'Templates found': {stdout}"
    );
}

#[test]
fn scan_json_report_is_valid() {
    if !models_available() {
        eprintln!("skipping cli::scan_json_report_is_valid — models not present");
        return;
    }
    let input = "ERROR database connection failed\n".repeat(3);
    let (code, stdout, _stderr) = pipe_to_scan(&input, "json");
    assert_eq!(code, Some(0), "uniform lines should not trigger anomalies");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("scan --format json must emit valid JSON");
    assert!(parsed["lines_processed"].is_number());
    assert!(parsed["templates_found"].is_number());
    assert!(parsed["anomalies_total"].is_number());
}

#[test]
fn scan_calibrates_below_64_templates() {
    if !models_available() {
        eprintln!("skipping cli::scan_calibrates_below_64_templates — models not present");
        return;
    }
    let lines = [
        "user authentication succeeded for account",
        "disk space running low on primary partition",
        "cache miss while fetching session key",
        "outbound email delivered to recipient",
        "database migration completed without errors",
        "scheduled backup job finished cleanly",
        "payment authorized through external gateway",
        "configuration reloaded from environment",
        "websocket client subscribed to channel",
        "image thumbnail generated and stored",
        "search index rebuilt from latest snapshot",
    ];
    let input = lines.join("\n");
    let (code, stdout, _stderr) = pipe_to_scan_args(&input, "json", &["--threshold", "0"]);
    assert_eq!(
        code,
        Some(1),
        "scan with --threshold 0 should exit 1 when anomalies are scored"
    );
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("scan --format json must emit valid JSON");
    assert_eq!(parsed["calibrated"], serde_json::Value::Bool(true));
    assert!(parsed["anomalies_total"].as_u64().unwrap_or(0) > 0);
}

#[test]
fn help_flag_works() {
    let output = Command::new(binary())
        .arg("--help")
        .output()
        .expect("failed to run turbolog --help");
    assert!(output.status.success(), "turbolog --help should exit 0");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("watch") && stdout.contains("scan") && stdout.contains("diagnose"),
        "help output should mention subcommands: {stdout}"
    );
}

#[test]
fn no_subcommand_shows_help() {
    let output = Command::new(binary())
        .output()
        .expect("failed to run turbolog");
    // clap prints help and exits 2 when a required subcommand is missing.
    assert_eq!(
        exit_code(output.status),
        Some(2),
        "expected exit 2 without subcommand"
    );
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        combined.contains("Usage:"),
        "expected usage text when no subcommand: {combined}"
    );
}

#[test]
fn subcommand_help_works() {
    for sub in &[
        "serve",
        "watch",
        "scan",
        "history",
        "diagnose",
        "ui",
        "completions",
    ] {
        let output = Command::new(binary())
            .arg(sub)
            .arg("--help")
            .output()
            .expect("failed to run help");
        assert!(
            output.status.success(),
            "turbolog {sub} --help should exit 0"
        );
    }
}

#[test]
fn completions_bash_generates_script() {
    let output = Command::new(binary())
        .args(["completions", "bash"])
        .output()
        .expect("failed to run completions");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("turbolog") && stdout.contains("complete"),
        "bash completion script expected: {stdout}"
    );
}

#[test]
fn diagnose_empty_history_exits_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = Command::new(binary())
        .args(["diagnose", "--since", "1h", "--format", "json"])
        .env("XDG_DATA_HOME", dir.path())
        .output()
        .expect("failed to run diagnose");
    assert_eq!(exit_code(output.status), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: serde_json::Value = serde_json::from_str(&stdout).expect("valid json");
    assert_eq!(parsed["total_anomalies"], 0);
}

#[test]
fn history_top_json_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    let output = Command::new(binary())
        .args(["history", "--top", "--format", "json", "--since", "1h"])
        .env("XDG_DATA_HOME", dir.path())
        .output()
        .expect("failed to run history --top");
    assert!(output.status.success());
    let parsed: serde_json::Value =
        serde_json::from_str(&String::from_utf8_lossy(&output.stdout)).expect("valid json array");
    assert!(parsed.is_array());
}
