//! CLI integration tests for `turbolog watch` and `turbolog scan`.
//!
//! These tests invoke the compiled binary via `std::process::Command`.
//! They are skipped when the ONNX model is not present (same guard as other tests).

use std::io::Write;
use std::path::Path;
use std::process::{Command, ExitStatus, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

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

fn unique_temp_dir(name: &str) -> std::path::PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("turbolog-{name}-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn seed_history_db(xdg_data_home: &Path) {
    let db_dir = xdg_data_home.join("turbolog");
    std::fs::create_dir_all(&db_dir).unwrap();
    let conn = rusqlite::Connection::open(db_dir.join("history.db")).unwrap();
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS anomalies (
            id          INTEGER PRIMARY KEY AUTOINCREMENT,
            timestamp   INTEGER NOT NULL,
            template    TEXT    NOT NULL,
            line        TEXT    NOT NULL,
            score       REAL    NOT NULL,
            explanation TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_template  ON anomalies(template);
        CREATE INDEX IF NOT EXISTS idx_timestamp ON anomalies(timestamp);",
    )
    .unwrap();

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let rows = [
        (
            now - 300,
            "db connection failed",
            "db connection failed host=primary",
            0.72,
        ),
        (
            now - 120,
            "db connection failed",
            "db connection failed host=replica",
            0.91,
        ),
        (
            now - 60,
            "db connection failed",
            "db connection failed host=primary",
            0.81,
        ),
        (now - 30, "cache miss", "cache miss key=session", 0.55),
    ];
    for (timestamp, template, line, score) in rows {
        conn.execute(
            "INSERT INTO anomalies (timestamp, template, line, score, explanation)
             VALUES (?1, ?2, ?3, ?4, NULL)",
            rusqlite::params![timestamp, template, line, score],
        )
        .unwrap();
    }
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
fn watch_shows_calibration_progress_and_threshold() {
    if !models_available() {
        eprintln!(
            "skipping cli::watch_shows_calibration_progress_and_threshold — models not present"
        );
        return;
    }
    let seeds = [
        "user login OK",
        "request processed in 12ms",
        "cache hit while fetching key",
    ];
    let mut input = String::new();
    for i in 0..503 {
        input.push_str(seeds[i % seeds.len()]);
        input.push('\n');
    }
    let (code, stdout, stderr) = pipe_to_watch(&input, &["--threshold", "10"]);
    assert_eq!(code, Some(0));
    assert!(
        stdout.contains("[calibrating ") && stdout.contains("/64]"),
        "watch should show calibration progress in line prefixes: {stdout}"
    );
    assert!(
        stderr.contains("calibration complete") && stderr.contains("threshold=10.000"),
        "watch should report effective threshold on calibration completion: {stderr}"
    );
}

#[test]
fn watch_quiet_suppresses_calibration_prefix() {
    if !models_available() {
        eprintln!("skipping cli::watch_quiet_suppresses_calibration_prefix — models not present");
        return;
    }
    let input = "user login OK\nrequest processed in 12ms\n";
    let (code, stdout, stderr) = pipe_to_watch(input, &["--quiet"]);
    assert_eq!(code, Some(0));
    assert!(
        !stdout.contains("[calibrating"),
        "quiet should suppress calibration prefixes: {stdout}"
    );
    assert!(
        !stderr.contains("calibration"),
        "quiet should suppress calibration status: {stderr}"
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
    assert!(
        stdout.contains("Calibration     : incomplete"),
        "small uniform input should explain that calibration is incomplete: {stdout}"
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
fn scan_text_report_explains_eof_finalize_threshold() {
    if !models_available() {
        eprintln!(
            "skipping cli::scan_text_report_explains_eof_finalize_threshold — models not present"
        );
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
    let (code, stdout, _stderr) = pipe_to_scan_args(&input, "text", &["--threshold", "10"]);
    assert_eq!(code, Some(0));
    assert!(
        stdout.contains("Calibration     : complete (EOF finalize, threshold=10.000)"),
        "scan text should explain EOF calibration and threshold: {stdout}"
    );
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
    assert_eq!(parsed["calibration_status"], "eof_finalized");
    assert!(parsed["effective_threshold"].is_number());
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
        stdout.contains("watch") && stdout.contains("scan") && stdout.contains("completions"),
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
    for sub in &["serve", "watch", "scan", "history", "ui", "completions"] {
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
fn history_recurring_outputs_text_and_json() {
    let xdg = unique_temp_dir("history-recurring");
    seed_history_db(&xdg);

    let text = Command::new(binary())
        .args(["history", "--recurring", "--since", "7d"])
        .env("XDG_DATA_HOME", &xdg)
        .output()
        .expect("failed to run recurring history text");
    assert!(text.status.success(), "history --recurring failed");
    let stdout = String::from_utf8_lossy(&text.stdout);
    assert!(stdout.contains("TurboLog Recurring History"), "{stdout}");
    assert!(stdout.contains("db connection failed"), "{stdout}");
    assert!(stdout.contains("sample: db connection failed"), "{stdout}");

    let json = Command::new(binary())
        .args([
            "history",
            "--recurring",
            "--since",
            "7d",
            "--format",
            "json",
        ])
        .env("XDG_DATA_HOME", &xdg)
        .output()
        .expect("failed to run recurring history json");
    assert!(
        json.status.success(),
        "history --recurring --format json failed"
    );
    let parsed: serde_json::Value =
        serde_json::from_slice(&json.stdout).expect("recurring history JSON must parse");
    let entries = parsed
        .as_array()
        .expect("recurring history JSON is an array");
    let db = entries
        .iter()
        .find(|entry| entry["template"] == "db connection failed")
        .expect("db template should be grouped");
    assert_eq!(db["count"], serde_json::json!(3));
    let max_score = db["max_score"]
        .as_f64()
        .expect("max_score should be numeric");
    assert!((max_score - 0.91).abs() < 0.000_001, "{max_score}");

    let _ = std::fs::remove_dir_all(xdg);
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
