use std::sync::mpsc::Sender;
use std::time::Duration;

use serde::Deserialize;

use crate::pipeline::LocalPipeline;
use crate::tui::app::{DashEvent, LogEntry};

#[derive(Deserialize)]
struct StatsResponse {
    ingested_total: u64,
    cache_hit_rate: f64,
    detector_calibrated: bool,
    #[serde(default)]
    anomalies_total: u64,
}

/// Polls `GET /stats` every 500ms and sends `DashEvent::StatsUpdate` to the UI thread.
pub fn http_poll_loop(server_url: String, tx: Sender<DashEvent>) {
    let stats_url = format!("{server_url}/stats");
    let mut prev_ingested = 0u64;
    let mut prev_anomalies = 0u64;

    loop {
        let stats: Option<StatsResponse> = ureq::get(&stats_url)
            .call()
            .ok()
            .and_then(|r| r.into_json().ok());

        match stats {
            Some(stats) => {
                let delta_ingested = stats.ingested_total.saturating_sub(prev_ingested);
                let delta_anomalies = stats.anomalies_total.saturating_sub(prev_anomalies);
                let anomaly_rate = if delta_ingested > 0 {
                    delta_anomalies as f64 / delta_ingested as f64
                } else {
                    0.0
                };
                prev_ingested = stats.ingested_total;
                prev_anomalies = stats.anomalies_total;

                let _ = tx.send(DashEvent::StatsUpdate {
                    ingested_total: stats.ingested_total,
                    cache_hit_rate: stats.cache_hit_rate,
                    anomaly_rate,
                    detector_calibrated: stats.detector_calibrated,
                });
            }
            None => {
                let _ = tx.send(DashEvent::ConnError(format!(
                    "Cannot reach {server_url}/stats — is the server running?"
                )));
            }
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

/// Reads stdin with a LocalPipeline and sends log + stats events to the UI thread.
pub fn standalone_loop(mut pipeline: LocalPipeline, tx: Sender<DashEvent>) {
    use std::io::{BufRead, BufReader};

    let stdin = std::io::stdin();
    let reader = BufReader::new(stdin.lock());
    let mut total: u64 = 0;
    let mut anomalies: u64 = 0;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.is_empty() {
            continue;
        }

        if let Ok(result) = pipeline.process(&line) {
            total += 1;
            if result.is_anomaly {
                anomalies += 1;
            }
            let anomaly_rate = if total > 0 {
                anomalies as f64 / total as f64
            } else {
                0.0
            };
            let _ = tx.send(DashEvent::LogLine(LogEntry {
                text: line,
                is_anomaly: result.is_anomaly,
                score: result.score,
            }));
            let _ = tx.send(DashEvent::StatsUpdate {
                ingested_total: total,
                cache_hit_rate: 0.0, // VectorCache hit_rate not exposed via LocalPipeline
                anomaly_rate,
                detector_calibrated: pipeline.calibrated(),
            });
        }
    }
}
