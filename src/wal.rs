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
}
