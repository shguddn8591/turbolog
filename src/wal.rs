//! Write-Ahead Log — Provides fault recovery for the active write window (default: 10s) between swaps.
//!
//! Log records contain pre-embedded (id, vector) pairs, enabling instant replay upon recovery without re-triggering embedding.
//! Once a window is sealed into a `.tvim` chunk, the WAL is truncated via `rotate`.
//!
//! Durability Policy: Performs write+flush on every append (process crash-safe). To maintain high throughput,
//! per-record fsync is omitted — a few seconds of data might be lost on full OS crashes.
//!
//! Format: Header `"TLWAL1"` + dim (u32 LE), Record = id (u64 LE) + dim×f32 (LE).
//! Truncated trailing records (from partial writes during a crash) are safely ignored on replay.

use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};

const MAGIC: &[u8; 6] = b"TLWAL1";
const HEADER_LEN: u64 = 6 + 4;

pub struct Wal {
    writer: BufWriter<File>,
    path: PathBuf,
    dim: usize,
}

impl Wal {
    /// Opens the WAL file. Creates it with the header if absent, otherwise seeks to the end.
    pub fn open(path: impl AsRef<Path>, dim: usize) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("Failed to open WAL file at {}", path.display()))?;

        let len = file.metadata()?.len();
        if len == 0 {
            file.write_all(MAGIC)?;
            file.write_all(&(dim as u32).to_le_bytes())?;
            file.flush()?;
        } else {
            let existing_dim = read_header(&mut file)?;
            ensure!(
                existing_dim == dim,
                "WAL dimension mismatch: file has {existing_dim}, requested {dim}"
            );
            file.seek(SeekFrom::End(0))?;
        }
        Ok(Self {
            writer: BufWriter::new(file),
            path,
            dim,
        })
    }

    pub fn append(&mut self, id: u64, vector: &[f32]) -> Result<()> {
        ensure!(vector.len() == self.dim, "Vector dimension mismatch");
        self.writer.write_all(&id.to_le_bytes())?;
        for v in vector {
            self.writer.write_all(&v.to_le_bytes())?;
        }
        self.writer.flush()?;
        Ok(())
    }

    /// Invoked upon a successful window seal — truncates the WAL, leaving only the header.
    pub fn rotate(&mut self) -> Result<()> {
        self.writer.flush()?;
        let file = self.writer.get_mut();
        file.set_len(HEADER_LEN)?;
        file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    /// Detaches the current WAL as a sealed file (rename) and reopens a fresh active WAL.
    /// The sealed filename is derived from the active WAL's stem: `{stem}-sealed-{nanos}.bin`.
    /// For example, `wal-0.bin` becomes `wal-0-sealed-{nanos}.bin` in the same directory.
    ///
    /// Only metadata operations happen here (flush + rename + create) — safe to call under
    /// the write-path lock without stalling ingestion.
    ///
    /// The returned sealed file must be deleted by the caller only AFTER the corresponding
    /// `.tvim` segment has been durably written. If the process crashes in between, the
    /// sealed file is picked up by `sealed_leftovers` + `replay` on the next startup.
    pub fn detach_sealed(&mut self) -> Result<PathBuf> {
        self.writer.flush()?;
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        // Derive sealed name from the WAL stem so shards stay isolated.
        // wal-0.bin → wal-0-sealed-{nanos}.bin
        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("wal");
        let sealed_name = format!("{stem}-sealed-{nanos}.bin");
        let sealed_path = self.path.with_file_name(sealed_name);
        std::fs::rename(&self.path, &sealed_path)
            .with_context(|| format!("Failed to detach WAL to {}", sealed_path.display()))?;
        *self = Self::open(&self.path, self.dim)?;
        Ok(sealed_path)
    }

    /// Lists sealed leftover files in `dir` whose names start with `prefix` (crash residue),
    /// oldest first. Each shard passes its own prefix, e.g. `"wal-0-sealed-"`.
    pub fn sealed_leftovers(dir: &Path, prefix: &str) -> Result<Vec<PathBuf>> {
        let mut files: Vec<PathBuf> = match std::fs::read_dir(dir) {
            Ok(entries) => entries
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.file_name()
                        .and_then(|n| n.to_str())
                        .is_some_and(|n| n.starts_with(prefix) && n.ends_with(".bin"))
                })
                .collect(),
            Err(_) => Vec::new(),
        };
        files.sort();
        Ok(files)
    }

    /// Replay Recovery: Reads all complete records from the WAL file. Returns an empty list if the file is absent.
    /// Trailing records that are partially written are ignored as crash residue.
    pub fn replay(path: impl AsRef<Path>, dim: usize) -> Result<Vec<(u64, Vec<f32>)>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut file = File::open(path)?;
        let file_dim = read_header(&mut file)?;
        ensure!(
            file_dim == dim,
            "WAL dimension mismatch: file has {file_dim}, requested {dim}"
        );

        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        let record_len = 8 + dim * 4;
        let mut entries = Vec::with_capacity(bytes.len() / record_len);
        for record in bytes.chunks_exact(record_len) {
            let id = u64::from_le_bytes(record[..8].try_into().unwrap());
            let vector: Vec<f32> = record[8..]
                .chunks_exact(4)
                .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
                .collect();
            entries.push((id, vector));
        }
        Ok(entries)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

fn read_header(file: &mut File) -> Result<usize> {
    file.seek(SeekFrom::Start(0))?;
    let mut magic = [0u8; 6];
    file.read_exact(&mut magic)?;
    if &magic != MAGIC {
        bail!("WAL magic mismatch — file is corrupted or of incorrect type");
    }
    let mut dim_bytes = [0u8; 4];
    file.read_exact(&mut dim_bytes)?;
    Ok(u32::from_le_bytes(dim_bytes) as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("turbolog_wal_{name}_{}.bin", std::process::id()))
    }

    #[test]
    fn append_replay_rotate() {
        let path = temp_path("roundtrip");
        std::fs::remove_file(&path).ok();
        {
            let mut wal = Wal::open(&path, 4).unwrap();
            wal.append(1, &[0.1, 0.2, 0.3, 0.4]).unwrap();
            wal.append(2, &[1.0, 0.0, 0.0, 0.0]).unwrap();
        }
        let entries = Wal::replay(&path, 4).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].0, 1);
        assert_eq!(entries[1].1, vec![1.0, 0.0, 0.0, 0.0]);

        // Reopen and append further -> original records should be preserved
        {
            let mut wal = Wal::open(&path, 4).unwrap();
            wal.append(3, &[0.0; 4]).unwrap();
        }
        assert_eq!(Wal::replay(&path, 4).unwrap().len(), 3);

        // rotate -> cleared
        {
            let mut wal = Wal::open(&path, 4).unwrap();
            wal.rotate().unwrap();
            wal.append(9, &[0.5; 4]).unwrap();
        }
        let entries = Wal::replay(&path, 4).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].0, 9);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn truncated_tail_ignored() {
        let path = temp_path("truncated");
        std::fs::remove_file(&path).ok();
        {
            let mut wal = Wal::open(&path, 4).unwrap();
            wal.append(1, &[0.1; 4]).unwrap();
        }
        // Simulate a partial record: tail is only partially written
        {
            use std::io::Write;
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&7u64.to_le_bytes()).unwrap();
            f.write_all(&[0u8; 5]).unwrap();
        }
        let entries = Wal::replay(&path, 4).unwrap();
        assert_eq!(entries.len(), 1, "incomplete trailing records should be ignored");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn detach_sealed_derives_name_from_stem() {
        // WAL at turbolog_wal_detach_{pid}.bin → sealed at turbolog_wal_detach_{pid}-sealed-{nanos}.bin
        let path = temp_path("detach");
        std::fs::remove_file(&path).ok();
        {
            let mut wal = Wal::open(&path, 4).unwrap();
            wal.append(1, &[0.1; 4]).unwrap();
            let sealed = wal.detach_sealed().unwrap();
            // Sealed file exists; active WAL is fresh (0 records).
            assert!(sealed.exists(), "sealed file must exist");
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap();
            let sealed_name = sealed.file_name().and_then(|n| n.to_str()).unwrap();
            assert!(
                sealed_name.starts_with(stem),
                "sealed name should start with WAL stem"
            );
            assert!(sealed_name.contains("-sealed-"));
            assert!(!path.exists() || Wal::replay(&path, 4).unwrap().is_empty(),
                "active WAL should be fresh after detach");
            std::fs::remove_file(&sealed).ok();
        }
        std::fs::remove_file(&path).ok();
    }
}
