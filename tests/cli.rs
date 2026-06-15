//! CLI integration tests for `turbolog watch` and `turbolog scan`.
//!
//! These tests invoke the compiled binary via `std::process::Command`.
//! They are skipped when the ONNX model is not present (same guard as other tests).

use std::io::Write;
use std::process::{Command, Stdio};

fn binary() -> std::path::PathBuf {
    std::path::PathBuf::from(env!("CARGO_BIN_EXE_turbolog"))
}

fn models_available() -> bool {
    std::path::Path::new("models/model.onnx").exists()
        && std::path::Path::new("models/tokenizer.json").exists()
}

/// Feed lines to turbolog watch and return (exit_code, stdout, stderr).
fn pipe_to_watch(input: &str, extra_args: &[&str]) -> (bool, String, String) {
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
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).into_owned(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

fn pipe_to_scan(input: &str, format: &str) -> (bool, String, String) {
    let mut cmd = Command::new(binary())
        .arg("scan")
        .arg("--format")
        .arg(format)
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
        output.status.success(),
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
    let (ok, _stdout, _stderr) = pipe_to_watch("", &[]);
    assert!(ok, "turbolog watch should exit 0 on empty stdin");
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
    let (ok, stdout, _stderr) = pipe_to_watch(input, &[]);
    assert!(ok, "turbolog watch should exit 0");
    // Should output at least as many lines as input (may include [calibrating] prefix).
    let out_lines = stdout.lines().count();
    assert!(
        out_lines >= 3,
        "expected at least 3 output lines, got {out_lines}: {stdout}"
    );
}

#[test]
fn scan_text_report_contains_summary() {
    if !models_available() {
        eprintln!("skipping cli::scan_text_report_contains_summary — models not present");
        return;
    }
    let input = "INFO user login success\n".repeat(5);
    let (ok, stdout, _stderr) = pipe_to_scan(&input, "text");
    assert!(ok, "turbolog scan should exit 0");
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
    let (ok, stdout, _stderr) = pipe_to_scan(&input, "json");
    assert!(ok, "turbolog scan --format json should exit 0");
    let parsed: serde_json::Value =
        serde_json::from_str(&stdout).expect("scan --format json must emit valid JSON");
    assert!(
        parsed["lines_processed"].is_number(),
        "JSON report must contain 'lines_processed'"
    );
    assert!(
        parsed["templates_found"].is_number(),
        "JSON report must contain 'templates_found'"
    );
    assert!(
        parsed["anomalies_total"].is_number(),
        "JSON report must contain 'anomalies_total'"
    );
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
        stdout.contains("watch") && stdout.contains("scan"),
        "help output should mention subcommands: {stdout}"
    );
}

#[test]
fn subcommand_help_works() {
    for sub in &["serve", "watch", "scan", "ui"] {
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
