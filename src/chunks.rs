//! Historical Chunk Store — Groups sealed `.tvim` segments (produced every 10s swap) into 1-hour directories.
//!
//! System Constraint (Spec v1.0 §4.2 — Hard Physical Deletion):
//! To avoid filesystem fragmentation, expired directories are unlinked completely at the OS level
//! rather than performing individual per-vector deletions.
//!
//! Layout: `<root>/hour-<unix_hour>/seg-<unix_millis>.tvim`

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

pub struct ChunkStore {
    root: PathBuf,
}

impl ChunkStore {
    pub fn new(root: impl AsRef<Path>) -> Result<Self> {
        let root = root.as_ref().to_path_buf();
        std::fs::create_dir_all(&root)
            .with_context(|| format!("Failed to create chunk root directory at {}", root.display()))?;
        Ok(Self { root })
    }

    /// Generates the segment path for a given timestamp and ensures the parent hour directory exists.
    pub fn segment_path(&self, unix_millis: i64) -> Result<PathBuf> {
        let hour = unix_millis / 3_600_000;
        let dir = self.root.join(format!("hour-{hour}"));
        std::fs::create_dir_all(&dir)?;
        Ok(dir.join(format!("seg-{unix_millis}.tvim")))
    }

    /// Deletes entire hour directories that exceed the retention duration. Returns the count of deleted directories.
    pub fn sweep(&self, retention_hours: u64, now_millis: i64) -> Result<usize> {
        let current_hour = now_millis / 3_600_000;
        let cutoff = current_hour - retention_hours as i64;
        let mut removed = 0;
        for entry in std::fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let Some(hour) = name
                .to_str()
                .and_then(|n| n.strip_prefix("hour-"))
                .and_then(|h| h.parse::<i64>().ok())
            else {
                continue; // Do not touch entries with unmatched format layouts
            };
            if hour < cutoff {
                std::fs::remove_dir_all(entry.path())
                    .with_context(|| format!("Failed to delete chunk directory at {}", entry.path().display()))?;
                removed += 1;
            }
        }
        Ok(removed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_layout_and_sweep() {
        let root = std::env::temp_dir().join(format!("turbolog_chunks_{}", std::process::id()));
        std::fs::remove_dir_all(&root).ok();
        let store = ChunkStore::new(&root).unwrap();

        let now: i64 = 1_770_000_000_000; // arbitrary reference time (ms)
        let old = now - 10 * 3_600_000; // 10 hours ago
        let p_now = store.segment_path(now).unwrap();
        let p_old = store.segment_path(old).unwrap();
        std::fs::write(&p_now, b"x").unwrap();
        std::fs::write(&p_old, b"x").unwrap();
        assert_ne!(p_now.parent(), p_old.parent(), "Segments must reside in different hourly directories");

        // Retention set to 7 hours -> only the segment from 10 hours ago is deleted.
        let removed = store.sweep(7, now).unwrap();
        assert_eq!(removed, 1);
        assert!(p_now.exists());
        assert!(!p_old.exists());
        std::fs::remove_dir_all(&root).ok();
    }
}
