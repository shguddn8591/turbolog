pub mod app;
pub mod data;
pub mod events;
pub mod render;

use std::io;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::tui::app::{AppState, DashEvent, DashMode, LogEntry};
use crate::tui::events::KeyEvent;

struct TerminalGuard;

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(io::stdout(), LeaveAlternateScreen);
    }
}

/// Entry point for the `turbolog ui` subcommand.
pub fn run_ui(server_url: &str, standalone: bool) -> Result<()> {
    let mode = if standalone {
        DashMode::Standalone
    } else {
        DashMode::HttpClient
    };
    let mut app = AppState::new(mode, server_url.to_string());

    // Initialize pipeline before terminal setup so a model-load failure doesn't
    // leave the terminal stuck in raw mode / alternate screen.
    let standalone_pipeline = if standalone {
        let model_dir = std::path::PathBuf::from(
            std::env::var("TURBOLOG_MODEL_DIR").unwrap_or_else(|_| "./models".into()),
        );
        let embedder = crate::embedded::make_embedder(&model_dir)?;
        Some(crate::pipeline::LocalPipeline::new(embedder, None))
    } else {
        None
    };

    // Set up terminal.
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let _guard = TerminalGuard;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Data thread.
    let (data_tx, data_rx) = mpsc::channel::<DashEvent>();
    let url = server_url.to_string();
    if let Some(pipeline) = standalone_pipeline {
        std::thread::spawn(move || data::standalone_loop(pipeline, data_tx));
    } else {
        std::thread::spawn(move || data::http_poll_loop(url, data_tx));
    }

    // Keyboard thread.
    let (key_tx, key_rx) = mpsc::channel::<KeyEvent>();
    std::thread::spawn(move || events::keyboard_loop(key_tx));

    let tick = Duration::from_millis(50); // ~20fps
    let mut last_sparkline_tick = Instant::now();

    let result = run_loop(
        &mut terminal,
        &mut app,
        &data_rx,
        &key_rx,
        tick,
        &mut last_sparkline_tick,
    );

    if let Ok(_) = terminal.show_cursor() {}

    result
}

fn run_loop(
    terminal: &mut ratatui::Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
    data_rx: &mpsc::Receiver<DashEvent>,
    key_rx: &mpsc::Receiver<KeyEvent>,
    tick: Duration,
    last_sparkline_tick: &mut Instant,
) -> Result<()> {
    loop {
        // Drain data events (non-blocking).
        while let Ok(event) = data_rx.try_recv() {
            apply_event(app, event);
        }

        // Update sparkline every second.
        if last_sparkline_tick.elapsed() >= Duration::from_secs(1) {
            app.push_sparkline(app.anomaly_rate);
            *last_sparkline_tick = Instant::now();
        }

        // Compute ingested/s from last tick delta.
        let elapsed = app.last_tick.elapsed().as_secs_f64();
        if elapsed >= 0.5 {
            let delta = app.ingested_total.saturating_sub(app.last_ingested);
            app.ingested_per_sec = delta as f64 / elapsed;
            app.last_ingested = app.ingested_total;
            app.last_tick = Instant::now();
        }

        // Render.
        terminal.draw(|f| render::draw(f, app))?;

        // Check for quit.
        if key_rx.try_recv().is_ok() {
            break;
        }

        std::thread::sleep(tick);
    }
    Ok(())
}

fn apply_event(app: &mut AppState, event: DashEvent) {
    match event {
        DashEvent::StatsUpdate {
            ingested_total,
            cache_hit_rate,
            anomaly_rate,
            detector_calibrated,
        } => {
            app.ingested_total = ingested_total;
            app.cache_hit_rate = cache_hit_rate;
            app.anomaly_rate = anomaly_rate;
            app.detector_calibrated = detector_calibrated;
        }
        DashEvent::LogLine(entry) => {
            app.push_log(entry);
        }
        DashEvent::ConnError(msg) => {
            app.push_log(LogEntry {
                text: msg,
                is_anomaly: false,
                score: None,
            });
        }
    }
}
