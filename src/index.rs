//! Ping-Pong Indexer — Physically isolates Read/Write indices to eliminate latency spikes.
//!
//! The spec's `AtomicPtr<*mut>` design suffers from a use-after-free issue during swapping where reader references live on.
//! This is corrected using `arc-swap`: Readers fetch the snapshot Arc via `ArcSwap::load_full`, ensuring safe reads even after swaps.
//! While writes pass through an ingest thread-exclusive Mutex, the search path never accesses this lock, avoiding latency spikes.
//!
//! ## Window Semantics
//! Invoking `swap_and_flush` seals the active write index, publishes it as the search snapshot, and restarts writes on a fresh index.
//! Thus, the search snapshot contains vectors from the **immediately prior sealed window (default: 10s)**.
//! Historical searches extending beyond this window are handled by Phase 3 historical chunks (`.tvim` files).
//!
//! System Constraint (Spec v1.0 §4.2 — Hard Physical Deletion):
//! On sliding window expiry, the engine avoids per-vector `remove()` calls to prevent fragmentation.
//! The 1-hour chunk files (`.tvim`) themselves are deleted (unlinked) at the OS level.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{ensure, Context, Result};
use arc_swap::ArcSwap;
use turbovec::IdMapIndex;

pub struct PingPongIndexer {
    /// Active Write index — exclusive to the ingest path. The search path never touches this lock.
    write: Mutex<IdMapIndex>,
    /// Active Read snapshot — loaded by search threads without locks.
    search: ArcSwap<IdMapIndex>,
    dim: usize,
    bit_width: usize,
}

impl PingPongIndexer {
    pub fn new(dim: usize, bit_width: usize) -> Result<Self> {
        Ok(Self {
            write: Mutex::new(IdMapIndex::new(dim, bit_width)?),
            search: ArcSwap::from_pointee(IdMapIndex::new(dim, bit_width)?),
            dim,
            bit_width,
        })
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    /// Ingests a vector into the active write index. Does not share locks with the search path.
    pub fn ingest(&self, id: u64, vector: &[f32]) -> Result<()> {
        ensure!(
            vector.len() == self.dim,
            "Vector dimension mismatch: {} != {}",
            vector.len(),
            self.dim
        );
        self.write
            .lock()
            .unwrap()
            .add_with_ids(vector, &[id])
            .with_context(|| format!("Failed to ingest vector with ID {id}"))?;
        Ok(())
    }

    /// Returns the active read snapshot — lock-free. The returned Arc remains valid even after swaps.
    pub fn get_search_index(&self) -> Arc<IdMapIndex> {
        self.search.load_full()
    }

    /// Returns the number of vectors accumulated in the current write window (used for skip-swap checks).
    pub fn pending_len(&self) -> usize {
        self.write.lock().unwrap().len()
    }

    /// Seals the active write window and hands it to the caller. The write lock is held
    /// only for a `mem::replace` (microseconds) — `prepare()` and disk flushes must happen
    /// outside, then the sealed index is made searchable via [`Self::publish`].
    pub fn seal(&self) -> Result<IdMapIndex> {
        let fresh = IdMapIndex::new(self.dim, self.bit_width)?;
        let mut guard = self.write.lock().unwrap();
        Ok(std::mem::replace(&mut *guard, fresh))
    }

    /// Atomically publishes a sealed index as the active search snapshot.
    pub fn publish(&self, sealed: Arc<IdMapIndex>) {
        self.search.store(sealed);
    }

    /// Invoked periodically by the background thread.
    /// Seals the active write index, atomically publishes it as the search snapshot,
    /// and flushes the sealed window to disk as a `.tvim` chunk if `flush_path` is provided.
    ///
    /// Convenience wrapper over [`Self::seal`] + [`Self::publish`]. Note that the disk
    /// flush happens outside the write lock, but callers needing WAL-coordinated sealing
    /// (see `engine::swap_tick`) should use `seal`/`publish` directly.
    pub fn swap_and_flush(&self, flush_path: Option<&Path>) -> Result<()> {
        let sealed = self.seal()?;
        // Pre-compute the search cache (rotation matrix, SIMD layout) before publishing to the snapshot
        // to prevent latency spikes for the first search query.
        sealed.prepare();
        if let Some(path) = flush_path {
            sealed
                .write(path)
                .with_context(|| format!("Failed to backup chunk to {}", path.display()))?;
        }
        self.search.store(Arc::new(sealed));
        Ok(())
    }
}
