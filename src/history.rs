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

pub struct HistoryQuery {
    /// Look back this many seconds (None = all time).
    pub since_secs: Option<i64>,
    /// Substring filter on the template column.
    pub template: Option<String>,
    /// Maximum rows to return.
    pub limit: usize,
}

pub struct HistoryEntry {
    pub timestamp: i64,
    pub template: String,
    pub line: String,
    pub score: f32,
    pub explanation: Option<String>,
}

pub struct RecurringHistoryEntry {
    pub template: String,
    pub count: i64,
    pub last_seen: i64,
    pub max_score: f32,
    pub sample_line: String,
}

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
        init_schema(&conn)?;
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

    /// Query anomaly history with optional filters.
    pub fn query(&self, q: &HistoryQuery) -> Result<Vec<HistoryEntry>> {
        let cutoff = q.since_secs.map(|s| now_secs() - s).unwrap_or(0);
        let limit = q.limit as i64;

        let mut entries: Vec<HistoryEntry> = Vec::new();

        if let Some(ref tmpl) = q.template {
            let pattern = format!("%{tmpl}%");
            let mut stmt = self.conn.prepare(
                "SELECT timestamp, template, line, score, explanation
                 FROM anomalies WHERE timestamp >= ?1 AND template LIKE ?2
                 ORDER BY timestamp DESC LIMIT ?3",
            )?;
            let mut rows = stmt.query(params![cutoff, pattern, limit])?;
            while let Some(row) = rows.next()? {
                entries.push(map_history_row(row)?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT timestamp, template, line, score, explanation
                 FROM anomalies WHERE timestamp >= ?1
                 ORDER BY timestamp DESC LIMIT ?2",
            )?;
            let mut rows = stmt.query(params![cutoff, limit])?;
            while let Some(row) = rows.next()? {
                entries.push(map_history_row(row)?);
            }
        }

        Ok(entries)
    }

    /// Query recurring anomaly templates with optional filters.
    pub fn query_recurring(&self, q: &HistoryQuery) -> Result<Vec<RecurringHistoryEntry>> {
        let cutoff = q.since_secs.map(|s| now_secs() - s).unwrap_or(0);
        let limit = q.limit as i64;

        let mut entries: Vec<RecurringHistoryEntry> = Vec::new();

        if let Some(ref tmpl) = q.template {
            let pattern = format!("%{tmpl}%");
            let mut stmt = self.conn.prepare(
                "SELECT
                    a.template,
                    COUNT(*) AS count,
                    MAX(a.timestamp) AS last_seen,
                    MAX(a.score) AS max_score,
                    (
                        SELECT b.line
                        FROM anomalies b
                        WHERE b.template = a.template AND b.timestamp >= ?1
                        ORDER BY b.timestamp DESC, b.id DESC
                        LIMIT 1
                    ) AS sample_line
                 FROM anomalies a
                 WHERE a.timestamp >= ?1 AND a.template LIKE ?2
                 GROUP BY a.template
                 ORDER BY count DESC, last_seen DESC
                 LIMIT ?3",
            )?;
            let mut rows = stmt.query(params![cutoff, pattern, limit])?;
            while let Some(row) = rows.next()? {
                entries.push(map_recurring_row(row)?);
            }
        } else {
            let mut stmt = self.conn.prepare(
                "SELECT
                    a.template,
                    COUNT(*) AS count,
                    MAX(a.timestamp) AS last_seen,
                    MAX(a.score) AS max_score,
                    (
                        SELECT b.line
                        FROM anomalies b
                        WHERE b.template = a.template AND b.timestamp >= ?1
                        ORDER BY b.timestamp DESC, b.id DESC
                        LIMIT 1
                    ) AS sample_line
                 FROM anomalies a
                 WHERE a.timestamp >= ?1
                 GROUP BY a.template
                 ORDER BY count DESC, last_seen DESC
                 LIMIT ?2",
            )?;
            let mut rows = stmt.query(params![cutoff, limit])?;
            while let Some(row) = rows.next()? {
                entries.push(map_recurring_row(row)?);
            }
        }

        Ok(entries)
    }

    /// Returns a one-line context string for the given template, e.g.
    /// "seen 3× in the last 7 days (last seen: 2h ago)" — or None if no prior history.
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
                "SELECT MAX(timestamp) FROM anomalies WHERE template = ?1 AND timestamp >= ?2",
                params![template, cutoff],
                |row| row.get(0),
            )
            .unwrap_or(0);

        let age = format_age(now_secs() - last_ts);
        Some(format!(
            "seen {count}× in the last 7 days (last seen: {age})"
        ))
    }
}

fn init_schema(conn: &Connection) -> Result<()> {
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
    Ok(())
}

fn map_history_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HistoryEntry> {
    Ok(HistoryEntry {
        timestamp: row.get(0)?,
        template: row.get(1)?,
        line: row.get(2)?,
        score: row.get::<_, f64>(3)? as f32,
        explanation: row.get(4)?,
    })
}

fn map_recurring_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<RecurringHistoryEntry> {
    Ok(RecurringHistoryEntry {
        template: row.get(0)?,
        count: row.get(1)?,
        last_seen: row.get(2)?,
        max_score: row.get::<_, f64>(3)? as f32,
        sample_line: row.get(4)?,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn memory_store() -> HistoryStore {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        HistoryStore { conn }
    }

    fn insert_at(store: &HistoryStore, timestamp: i64, template: &str, line: &str, score: f32) {
        store
            .conn
            .execute(
                "INSERT INTO anomalies (timestamp, template, line, score, explanation)
                 VALUES (?1, ?2, ?3, ?4, NULL)",
                params![timestamp, template, line, score as f64],
            )
            .unwrap();
    }

    #[test]
    fn recurring_history_aggregates_by_template() {
        let store = memory_store();
        let now = now_secs();
        insert_at(
            &store,
            now - 3_600,
            "db connection failed",
            "db connection failed",
            0.72,
        );
        insert_at(
            &store,
            now - 120,
            "db connection failed",
            "db connection failed retry",
            0.91,
        );
        insert_at(
            &store,
            now - 60,
            "db connection failed",
            "db connection failed final",
            0.81,
        );
        insert_at(&store, now - 30, "cache miss", "cache miss key=abc", 0.55);
        insert_at(
            &store,
            now - 8 * 86_400,
            "db connection failed",
            "old db connection failed",
            0.99,
        );

        let entries = store
            .query_recurring(&HistoryQuery {
                since_secs: Some(7 * 86_400),
                template: None,
                limit: 10,
            })
            .unwrap();

        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].template, "db connection failed");
        assert_eq!(entries[0].count, 3);
        assert_eq!(entries[0].last_seen, now - 60);
        assert!((entries[0].max_score - 0.91).abs() < f32::EPSILON);
        assert_eq!(entries[0].sample_line, "db connection failed final");
    }

    #[test]
    fn recurring_history_filters_by_template_substring() {
        let store = memory_store();
        let now = now_secs();
        insert_at(
            &store,
            now - 60,
            "db connection failed",
            "db connection failed",
            0.72,
        );
        insert_at(&store, now - 30, "cache miss", "cache miss key=abc", 0.55);

        let entries = store
            .query_recurring(&HistoryQuery {
                since_secs: Some(7 * 86_400),
                template: Some("connection".to_string()),
                limit: 10,
            })
            .unwrap();

        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].template, "db connection failed");
    }

    #[test]
    fn context_for_matches_user_visible_copy() {
        let store = memory_store();
        let now = now_secs();
        insert_at(
            &store,
            now - 3_600,
            "db connection failed",
            "db connection failed",
            0.72,
        );
        insert_at(
            &store,
            now - 60,
            "db connection failed",
            "db connection failed retry",
            0.91,
        );

        let context = store.context_for("db connection failed").unwrap();

        assert!(context.starts_with("seen 2× in the last 7 days (last seen: "));
        assert!(context.ends_with(" ago)"));
    }
}
