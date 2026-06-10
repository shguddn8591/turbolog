//! Write-Ahead Log — 스왑 사이 윈도우(기본 10초)의 장애 복구.
//!
//! 레코드는 임베딩 완료된 (id, 벡터) 쌍이라 재기동 시 재임베딩 없이 복구된다.
//! 스왑이 성공해 윈도우가 .tvim 청크로 봉인되면 `rotate`로 WAL을 비운다.
//!
//! 내구성 정책: append마다 write+flush(프로세스 크래시 안전). fsync는 레코드당
//! 호출 시 처리량을 잡아먹으므로 하지 않는다 — OS 크래시 직전 수 초는 유실 허용.
//!
//! 포맷: 헤더 `"TLWAL1"` + dim(u32 LE), 레코드 = id(u64 LE) + dim×f32(LE).
//! 꼬리의 불완전 레코드(크래시 잔여)는 replay 시 무시한다.

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
    /// WAL 파일을 연다 (없으면 헤더와 함께 생성, 있으면 append 위치로 이동).
    pub fn open(path: impl AsRef<Path>, dim: usize) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&path)
            .with_context(|| format!("WAL 열기 실패: {}", path.display()))?;

        let len = file.metadata()?.len();
        if len == 0 {
            file.write_all(MAGIC)?;
            file.write_all(&(dim as u32).to_le_bytes())?;
            file.flush()?;
        } else {
            let existing_dim = read_header(&mut file)?;
            ensure!(
                existing_dim == dim,
                "WAL dim 불일치: 파일 {existing_dim}, 요청 {dim}"
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
        ensure!(vector.len() == self.dim, "벡터 차원 불일치");
        self.writer.write_all(&id.to_le_bytes())?;
        for v in vector {
            self.writer.write_all(&v.to_le_bytes())?;
        }
        self.writer.flush()?;
        Ok(())
    }

    /// 윈도우 봉인 성공 후 호출 — WAL을 헤더만 남기고 비운다.
    pub fn rotate(&mut self) -> Result<()> {
        self.writer.flush()?;
        let file = self.writer.get_mut();
        file.set_len(HEADER_LEN)?;
        file.seek(SeekFrom::End(0))?;
        Ok(())
    }

    /// 재기동 복구: WAL의 전체 레코드를 읽는다. 파일이 없으면 빈 목록.
    /// 꼬리의 불완전 레코드는 크래시 잔여물로 보고 무시한다.
    pub fn replay(path: impl AsRef<Path>, dim: usize) -> Result<Vec<(u64, Vec<f32>)>> {
        let path = path.as_ref();
        if !path.exists() {
            return Ok(Vec::new());
        }
        let mut file = File::open(path)?;
        let file_dim = read_header(&mut file)?;
        ensure!(
            file_dim == dim,
            "WAL dim 불일치: 파일 {file_dim}, 요청 {dim}"
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
        bail!("WAL magic 불일치 — 손상되었거나 다른 파일");
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

        // 재오픈 후 추가 append → 기존 레코드 보존
        {
            let mut wal = Wal::open(&path, 4).unwrap();
            wal.append(3, &[0.0; 4]).unwrap();
        }
        assert_eq!(Wal::replay(&path, 4).unwrap().len(), 3);

        // rotate → 비워짐
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
        // 불완전 레코드 흉내: 절반만 기록된 꼬리
        {
            use std::io::Write;
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(&7u64.to_le_bytes()).unwrap();
            f.write_all(&[0u8; 5]).unwrap();
        }
        let entries = Wal::replay(&path, 4).unwrap();
        assert_eq!(entries.len(), 1, "불완전 꼬리는 무시");
        std::fs::remove_file(&path).ok();
    }
}
