//! Off-tick chunk load/generate workers (ADR 0013: chunk generation/loading
//! runs on pure workers, owned input to owned output, joined at the tick's
//! input step).
//!
//! Workers share the native store's read-only access and immutable inputs;
//! each worker owns its job channel. Results return over the tick-owned
//! channel. Every request carries a ticket (revision); the tick accepts it
//! only while it is current for that position, so superseded or unwanted loads
//! are simply discarded on arrival instead of being cancelled in place.

use std::panic::{AssertUnwindSafe, catch_unwind, resume_unwind};
use std::sync::Arc;
use std::sync::mpsc;
use std::thread::JoinHandle;

use mc_world::native::{NativeError, NativeStore};
use mc_world::{BlockRegistry, Chunk, ChunkGenerator, ChunkPos};

/// A stored chunk that cannot be loaded. It must never be replaced by worldgen.
#[derive(Debug, Clone)]
pub struct ChunkLoadError {
    pub dimension: u32,
    pub pos: ChunkPos,
    pub reason: String,
}

impl std::fmt::Display for ChunkLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "native dimension {} chunk ({}, {}): {}",
            self.dimension, self.pos.x, self.pos.z, self.reason,
        )
    }
}

impl std::error::Error for ChunkLoadError {}

/// Immutable inputs a worker needs: native coverage wins over worldgen.
pub(crate) struct ChunkSource {
    pub(crate) native: Option<Arc<NativeStore>>,
    pub(crate) dimension: u32,
    pub(crate) generator: Arc<dyn ChunkGenerator>,
    pub(crate) registry: Arc<BlockRegistry>,
    pub(crate) items: Arc<mc_data::items::ItemRegistry>,
    pub(crate) chest_loot: Arc<mc_data::loot::chest_26_1_2::ChestLootCatalog>,
    pub(crate) light: Arc<mc_data::block_light::BlockLightTable>,
    pub(crate) geometry: mc_world::ChunkGeometry,
}

impl ChunkSource {
    /// Generate only when the native store reports no coverage. Failed reads
    /// remain failed, including imported all-air and malformed stored chunks.
    pub(crate) fn load_or_generate(&self, pos: ChunkPos) -> Result<Produced, ChunkLoadError> {
        let (mut chunk, generated) = match self.load_native(pos)? {
            Some(chunk) => (chunk, false),
            None => (self.generator.generate(pos), true),
        };
        if chunk.pos != pos || chunk.geometry() != self.geometry {
            return Err(self.load_error(
                pos,
                NativeError::Invalid("loaded/generated chunk position or geometry mismatch"),
            ));
        }
        crate::light::bake_missing(&mut chunk, &self.light);
        Ok((chunk, generated))
    }

    pub(crate) fn load_native(&self, pos: ChunkPos) -> Result<Option<Chunk>, ChunkLoadError> {
        let Some(store) = &self.native else {
            return Ok(None);
        };
        store
            .read_chunk(
                self.dimension,
                pos,
                &self.registry,
                &self.items,
                &self.chest_loot,
                self.generator.as_ref(),
            )
            .map_err(|error| self.load_error(pos, error))
    }

    /// Resolve the immutable baseline, never the composed gameplay image.
    /// Checkpoints persist new bases, so later captures do not regenerate them.
    pub(crate) fn resolve_base(&self, pos: ChunkPos) -> Result<Chunk, ChunkLoadError> {
        let Some(store) = &self.native else {
            return Ok(self.generator.generate(pos));
        };
        store
            .resolve_base(
                self.dimension,
                pos,
                &self.registry,
                &self.items,
                &self.chest_loot,
                || Ok(Some(self.generator.generate(pos))),
            )
            .and_then(|base| base.ok_or(NativeError::Invalid("missing checkpoint baseline")))
            .map_err(|error| self.load_error(pos, error))
    }

    fn load_error(&self, pos: ChunkPos, error: NativeError) -> ChunkLoadError {
        ChunkLoadError {
            dimension: self.dimension,
            pos,
            reason: error.to_string(),
        }
    }
}

/// Requests queued per worker beyond the one it is producing.
const JOBS_PER_WORKER: usize = 1;

/// Bound all produced-but-unadmitted chunks plus queued/running jobs. Admission
/// passes its ready count back to dispatch, so draining the channel cannot
/// accidentally remove backpressure.
pub(crate) const READY_LOADS_MAX: usize = 32;

/// One produced chunk and whether it was generated (not read from disk).
pub(crate) type Produced = (Chunk, bool);
pub(crate) type Ready = (ChunkPos, u64, Result<Produced, ChunkLoadError>);

/// One finished load: position, the request's ticket, and the load result
/// (or the worker's panic, resumed on the tick thread).
pub(crate) type Loaded = (
    ChunkPos,
    u64,
    std::thread::Result<Result<Produced, ChunkLoadError>>,
);

/// Fixed pool of chunk workers; the tick thread is the only producer of
/// jobs and the only consumer of results.
pub(crate) struct ChunkLoader {
    /// Per-worker bounded job queues: the bound, not bookkeeping, keeps at
    /// most [`JOBS_PER_WORKER`] requests queued at a worker.
    jobs: Vec<mpsc::SyncSender<(ChunkPos, u64)>>,
    next_worker: usize,
    done: tokio::sync::mpsc::UnboundedReceiver<Loaded>,
    pending: usize,
    workers: Vec<JoinHandle<()>>,
}

impl ChunkLoader {
    pub(crate) fn spawn(source: Arc<ChunkSource>, workers: usize) -> Self {
        let (done_tx, done) = tokio::sync::mpsc::unbounded_channel();
        let mut jobs = Vec::new();
        let mut handles = Vec::new();
        for index in 0..workers.max(1) {
            let (job_tx, job_rx) = mpsc::sync_channel::<(ChunkPos, u64)>(JOBS_PER_WORKER);
            let source = Arc::clone(&source);
            let done_tx = done_tx.clone();
            handles.push(
                std::thread::Builder::new()
                    .name(format!("chunk-load-{index}"))
                    .spawn(move || {
                        for (pos, ticket) in job_rx {
                            let chunk =
                                catch_unwind(AssertUnwindSafe(|| source.load_or_generate(pos)));
                            if done_tx.send((pos, ticket, chunk)).is_err() {
                                return;
                            }
                        }
                    })
                    .expect("spawn chunk loader thread"),
            );
            jobs.push(job_tx);
        }
        Self {
            jobs,
            next_worker: 0,
            done,
            pending: 0,
            workers: handles,
        }
    }

    /// Hand one request to the first worker with queue room, starting after
    /// the last one used. Never blocks; `false` when every queue is full.
    pub(crate) fn try_submit(&mut self, pos: ChunkPos, ticket: u64, ready: usize) -> bool {
        if self.pending + ready >= READY_LOADS_MAX {
            return false;
        }
        let workers = self.jobs.len();
        for offset in 0..workers {
            let worker = (self.next_worker + offset) % workers;
            if self.jobs[worker].try_send((pos, ticket)).is_ok() {
                self.next_worker = worker + 1;
                self.pending += 1;
                return true;
            }
        }
        false
    }

    /// Finished loads since the last call; never blocks.
    pub(crate) fn drain(&mut self) -> Vec<Ready> {
        let mut out = Vec::new();
        while let Ok(loaded) = self.done.try_recv() {
            self.pending -= 1;
            out.push(unwrap_loaded(loaded));
        }
        out
    }

    pub(crate) fn pending(&self) -> usize {
        self.pending
    }

    /// Block until the next finished load (tests; never on the tick).
    pub(crate) fn recv(&mut self) -> Ready {
        self.pending -= 1;
        unwrap_loaded(
            self.done
                .blocking_recv()
                .expect("chunk loader workers alive"),
        )
    }
}

fn unwrap_loaded(
    (pos, ticket, chunk): Loaded,
) -> (ChunkPos, u64, Result<Produced, ChunkLoadError>) {
    match chunk {
        Ok(chunk) => (pos, ticket, chunk),
        Err(panic) => resume_unwind(panic),
    }
}

impl Drop for ChunkLoader {
    fn drop(&mut self) {
        // Closing every job channel ends each worker after its current job.
        self.jobs.clear();
        self.done.close();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
#[path = "chunk_loader_tests.rs"]
mod tests;
