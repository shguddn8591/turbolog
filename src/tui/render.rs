use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    symbols,
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph, Sparkline},
    Frame,
};

use crate::tui::app::{AppState, DashMode};

pub fn draw(f: &mut Frame, app: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage(40),
            Constraint::Percentage(35),
            Constraint::Percentage(25),
        ])
        .split(f.area());

    draw_log_stream(f, app, chunks[0]);
    draw_sparkline(f, app, chunks[1]);
    draw_stats_bar(f, app, chunks[2]);
}

fn draw_log_stream(f: &mut Frame, app: &AppState, area: ratatui::layout::Rect) {
    let items: Vec<ListItem> = app
        .recent_logs
        .iter()
        .map(|entry| {
            let style = if entry.is_anomaly {
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            let prefix = if let Some(score) = entry.score {
                if entry.is_anomaly {
                    format!("[ANOMALY {score:.2}] ")
                } else {
                    String::new()
                }
            } else {
                "[calibrating] ".to_string()
            };
            let text = format!("{}{}", prefix, entry.text);
            // Truncate to terminal width to avoid wrapping.
            let display = if text.len() > area.width as usize {
                format!("{}…", &text[..area.width.saturating_sub(1) as usize])
            } else {
                text
            };
            ListItem::new(Line::from(Span::styled(display, style)))
        })
        .collect();

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Log Stream (last 20) ")
        .title_alignment(Alignment::Left);
    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

fn draw_sparkline(f: &mut Frame, app: &AppState, area: ratatui::layout::Rect) {
    let data: Vec<u64> = app.anomaly_sparkline.iter().copied().collect();
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Anomaly Rate — last 60 ticks ")
        .title_alignment(Alignment::Left);
    let spark = Sparkline::default()
        .block(block)
        .data(&data)
        .style(Style::default().fg(Color::Yellow))
        .bar_set(symbols::bar::NINE_LEVELS);
    f.render_widget(spark, area);
}

fn draw_stats_bar(f: &mut Frame, app: &AppState, area: ratatui::layout::Rect) {
    let uptime = app.uptime_secs();
    let h = uptime / 3600;
    let m = (uptime % 3600) / 60;
    let s = uptime % 60;
    let uptime_str = format!("{h}h{m:02}m{s:02}s");

    let mode_str = match app.mode {
        DashMode::HttpClient => format!("HTTP ({})", app.server_url),
        DashMode::Standalone => "Standalone".to_string(),
    };

    let cal_str = if app.detector_calibrated {
        "YES"
    } else {
        "calibrating…"
    };
    let cal_color = if app.detector_calibrated {
        Color::Green
    } else {
        Color::Yellow
    };

    let text = vec![
        Line::from(vec![
            Span::raw(format!(" Ingested/s: {:.0}  │  ", app.ingested_per_sec)),
            Span::raw(format!("Anomaly: {:.2}%  │  ", app.anomaly_rate * 100.0)),
            Span::raw(format!("Cache: {:.1}%  │  ", app.cache_hit_rate * 100.0)),
            Span::raw(format!("Uptime: {uptime_str}")),
        ]),
        Line::from(vec![
            Span::raw(" Calibrated: "),
            Span::styled(cal_str, Style::default().fg(cal_color)),
            Span::raw(format!("  │  Mode: {mode_str}  │  [q] quit")),
        ]),
    ];

    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Stats ")
        .title_alignment(Alignment::Left);
    let para = Paragraph::new(text).block(block);
    f.render_widget(para, area);
}
