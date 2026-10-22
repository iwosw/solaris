//! Offline storage maintenance of a stopped native world: `world prune` forgets
//! overworld chunks nobody changed or lived in, `world compact` repacks rows
//! written before compression and returns free pages to the file system.
//!
//! Both take the world lease for the whole run, so a running server is refused
//! instead of raced, and both validate the world identity like a normal open.

use std::path::PathBuf;

use mc_world::native::{NativePruneReport, NativePruneRequest, NativeStore};

use crate::config::{CoreConfig, CoreError};
use crate::core::Core;
use crate::dimensions::Dimension;

fn storage(error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(error.to_string())
}

/// What to forget. Chunks within `keep_radius` blocks of `center` are never pruned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PruneRequest {
    pub keep_radius: u32,
    /// Block center `[x, z]`; the world spawn when absent.
    pub center: Option<[i32; 2]>,
    /// Only chunks whose inhabited time is at most this many ticks are pruned.
    pub max_inhabited_ticks: i64,
    /// Report what would be pruned and write nothing.
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PruneReport {
    pub center: [i32; 2],
    pub keep_chunk_min: [i32; 2],
    pub keep_chunk_max: [i32; 2],
    pub chunks: NativePruneReport,
    pub bytes_before: u64,
    /// Equal to `bytes_before` for a dry run.
    pub bytes_after: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CompactReport {
    pub rewritten_rows: u64,
    pub bytes_before: u64,
    pub bytes_after: u64,
}

struct StoppedWorld {
    path: PathBuf,
    _lease: std::fs::File,
}

fn stopped_world(config: &CoreConfig) -> Result<StoppedWorld, CoreError> {
    let lease = std::fs::File::open(&config.world_dir)
        .map_err(|e| storage(format!("world {}: {e}", config.world_dir.display())))?;
    lease.try_lock().map_err(|e| {
        storage(format!(
            "world {} is already leased (is the server running?): {e}",
            config.world_dir.display()
        ))
    })?;
    let path = config.world_dir.join("native/state.redb");
    if !path.is_file() {
        return Err(crate::native_checkpoint::import_required(config));
    }
    Ok(StoppedWorld {
        path,
        _lease: lease,
    })
}

fn open_store(config: &CoreConfig, world: &StoppedWorld) -> Result<NativeStore, CoreError> {
    NativeStore::open(
        &world.path,
        crate::native_checkpoint::native_identity(config),
    )
    .map_err(|error| crate::native_checkpoint::open_refusal(config, &config.world_dir, error))
}

fn file_bytes(path: &std::path::Path) -> Result<u64, CoreError> {
    std::fs::metadata(path)
        .map(|meta| meta.len())
        .map_err(storage)
}

/// Inclusive chunk rectangle covering `center ± radius` blocks on both axes.
fn rectangle(center: [i32; 2], radius: u32) -> ([i32; 2], [i32; 2]) {
    let chunk = |block: i64| {
        i32::try_from(block.div_euclid(16)).unwrap_or(if block < 0 { i32::MIN } else { i32::MAX })
    };
    let radius = i64::from(radius);
    let bound = |coordinate: i32, sign: i64| chunk(i64::from(coordinate) + sign * radius);
    (
        [bound(center[0], -1), bound(center[1], -1)],
        [bound(center[0], 1), bound(center[1], 1)],
    )
}

/// Forget untouched, uninhabited overworld chunks outside the keep radius, then
/// compact the file. The world is opened and closed once first, exactly like
/// serving, so a journal tail from a crash is folded into the store before any
/// chunk is judged. Nether and End chunks are never pruned.
pub fn prune_world(config: &CoreConfig, request: &PruneRequest) -> Result<PruneReport, CoreError> {
    if request.max_inhabited_ticks < 0 {
        return Err(storage("max inhabited ticks must not be negative"));
    }
    let probe = config.world_dir.join("native/state.redb");
    if !probe.is_file() {
        return Err(crate::native_checkpoint::import_required(config));
    }
    let core = Core::open(config.clone())?;
    let spawn = core.world_spawn();
    core.shutdown();
    let world = stopped_world(config)?;
    let center = request.center.unwrap_or([spawn[0], spawn[2]]);
    let (keep_min, keep_max) = rectangle(center, request.keep_radius);
    let bytes_before = file_bytes(&world.path)?;
    let store = open_store(config, &world)?;
    let chunks = store
        .prune_pristine(&NativePruneRequest {
            dimension: crate::native_checkpoint::dimension_id(Dimension::Overworld),
            keep_min,
            keep_max,
            max_inhabited_ticks: request.max_inhabited_ticks,
            dry_run: request.dry_run,
        })
        .map_err(storage)?;
    if !request.dry_run {
        store.compact().map_err(storage)?;
    }
    Ok(PruneReport {
        center,
        keep_chunk_min: keep_min,
        keep_chunk_max: keep_max,
        chunks,
        bytes_before,
        bytes_after: file_bytes(&world.path)?,
    })
}

/// Rewrite plain-NBT chunk rows as compressed rows and return free pages to
/// the file system. A corrupt row aborts the run with the rows before it kept.
pub fn compact_world(config: &CoreConfig) -> Result<CompactReport, CoreError> {
    let world = stopped_world(config)?;
    let bytes_before = file_bytes(&world.path)?;
    let store = open_store(config, &world)?;
    let rewritten_rows = store.repack_rows().map_err(storage)?;
    store.compact().map_err(storage)?;
    Ok(CompactReport {
        rewritten_rows,
        bytes_before,
        bytes_after: file_bytes(&world.path)?,
    })
}

#[cfg(test)]
#[path = "world_maintenance_tests.rs"]
mod tests;
