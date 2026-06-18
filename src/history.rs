//! Persistent anomaly history backed by SQLite.
//!
//! DB location (in priority order):
//!   $XDG_DATA_HOME/turbolog/history.db
//!   ~/.local/share/turbolog/history.db
//!
//! If the DB cannot be opened (read-only FS, permission error, etc.)
//! callers receive `None` — history is fully optional.

use std::path::PathBuf;

use anyhow::Result;
use rusqlite::{params, Connection};

pub struct HistoryStore {
    conn: Connection,
}

impl HistoryStore {
    pub fn open() -> Result<Self> {
        let path = db_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
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
        )?;
        Ok(Self { conn })
    }

    /// Persist a detected anomaly. `explanation` may be None if --explain is off.
    pub fn insert(
        &self,
        template: &str,
        line: &str,
        score: f32,
        explanation: Option<&str>,
    ) -> Result<()> {
        let ts = now_secs();
        self.conn.execute(
            "INSERT INTO anomalies (timestamp, template, line, score, explanation)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![ts, template, line, score as f64, explanation],
        )?;
        Ok(())
    }

    /// Returns a one-line context string for the given template, e.g.
    /// "seen 3× in the last 7 days (last: 2h ago)" — or None if no prior history.
    pub fn context_for(&self, template: &str) -> Option<String> {
        let cutoff = now_secs() - 7 * 86_400;
        let count: i64 = self
            .conn
            .query_row(
                "SELECT COUNT(*) FROM anomalies WHERE template = ?1 AND timestamp >= ?2",
                params![template, cutoff],
                |row| row.get(0),
            )
            .unwrap_or(0);

        if count == 0 {
            return None;
        }

        let last_ts: i64 = self
            .conn
            .query_row(
                "SELECT MAX(timestamp) FROM anomalies WHERE template = ?1",
                params![template],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let age = format_age(now_secs() - last_ts);
        Some(format!(
            "This log pattern has occurred {count}× in the last 7 days (last seen: {age})"
        ))
    }
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn format_age(secs: i64) -> String {
    let secs = secs.max(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3_600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3_600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

fn db_path() -> PathBuf {
    let base = std::env::var("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".to_string()))
                .join(".local/share")
        });
    base.join("turbolog/history.db")
}
