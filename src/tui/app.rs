use std::collections::VecDeque;
use std::time::Instant;

pub const MAX_LOG_LINES: usize = 20;
pub const SPARKLINE_LEN: usize = 60;

#[derive(Debug, Clone)]
pub struct LogEntry {
    pub text: String,
    pub is_anomaly: bool,
    pub score: Option<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DashMode {
    HttpClient,
    Standalone,
}

pub struct AppState {
    pub mode: DashMode,
    pub server_url: String,
    pub recent_logs: VecDeque<LogEntry>,
    /// Anomaly rate × 10000 for ratatui Sparkline (u64 input).
    pub anomaly_sparkline: VecDeque<u64>,
    pub ingested_total: u64,
    pub ingested_per_sec: f64,
    pub anomaly_rate: f64,
    pub cache_hit_rate: f64,
    pub detector_calibrated: bool,
    pub start_time: Instant,
    pub last_ingested: u64,
    pub last_tick: Instant,
}

impl AppState {
    pub fn new(mode: DashMode, server_url: String) -> Self {
        Self {
            mode,
            server_url,
            recent_logs: VecDeque::with_capacity(MAX_LOG_LINES + 1),
            anomaly_sparkline: VecDeque::from(vec![0u64; SPARKLINE_LEN]),
            ingested_total: 0,
            ingested_per_sec: 0.0,
            anomaly_rate: 0.0,
            cache_hit_rate: 0.0,
            detector_calibrated: false,
            start_time: Instant::now(),
            last_ingested: 0,
            last_tick: Instant::now(),
        }
    }

    pub fn push_log(&mut self, entry: LogEntry) {
        if self.recent_logs.len() >= MAX_LOG_LINES {
            self.recent_logs.pop_front();
        }
        self.recent_logs.push_back(entry);
    }

    pub fn push_sparkline(&mut self, anomaly_rate: f64) {
        if self.anomaly_sparkline.len() >= SPARKLINE_LEN {
            self.anomaly_sparkline.pop_front();
        }
        self.anomaly_sparkline
            .push_back((anomaly_rate * 10_000.0) as u64);
    }

    pub fn uptime_secs(&self) -> u64 {
        self.start_time.elapsed().as_secs()
    }
}

/// Events sent from the data thread to the TUI render loop.
pub enum DashEvent {
    /// Stats update from HTTP server or local pipeline.
    StatsUpdate {
        ingested_total: u64,
        cache_hit_rate: f64,
        anomaly_rate: f64,
        detector_calibrated: bool,
    },
    /// A new log line was processed (standalone mode).
    LogLine(LogEntry),
    /// Connection error (HTTP mode).
    ConnError(String),
}
