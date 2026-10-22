//! Offline chunk pregeneration: fill the native store with terrain before
//! players connect, using the same chunk workers the live world uses.
//!
//! The world is opened exactly like serving (world lease, plugin-applied
//! registry, overworld terrain) and closed immediately; generation then runs
//! on the world's chunk-loader workers (`ChunkSource::load_or_generate`, the runtime's
//! generate-and-bake-light path) and each batch is written as immutable
//! native baselines through `NativeStore::fill_generated`. That write skips any
//! position the store already covers, so a rerun (or an interrupted run)
//! only generates what is still missing.
//!
//! Chunks that a join would have generated anyway are byte-for-byte the ones
//! this command stores, so a pregenerated join loads from disk instead of
//! running worldgen.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, Instant};

use mc_world::native::NativeStore;
use mc_world::{Chunk, ChunkGenerator, ChunkPos};

use crate::chunk_loader::{ChunkLoader, ChunkSource};
use crate::config::{CoreConfig, CoreError};
use crate::core::{Core, overworld_terrain};
use crate::dimensions::Dimension;

/// Chunks per durable native transaction; the runtime's generated save batch.
const SAVE_BATCH: usize = 64;

/// Fail closed above this rectangle (4096x4096 chunks), the retired v1 cap.
const MAX_CHUNKS: i64 = 4_194_304;

/// How often the progress callback fires while generating.
const PROGRESS_INTERVAL: Duration = Duration::from_secs(1);

/// What to pregenerate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregenerateRequest {
    /// Radius in blocks around `center` (both axes, inclusive).
    pub radius: u32,
    /// Block `[x, z]` center; `None` uses the world spawn.
    pub center: Option<[i32; 2]>,
}

impl PregenerateRequest {
    /// A square radius in blocks around the world spawn.
    #[must_use]
    pub fn new(radius: u32) -> Self {
        Self {
            radius,
            center: None,
        }
    }
}

/// Progress sample, reported at most once per second and once at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregenerateProgress {
    /// Chunks already covered by the store before this run started.
    pub already_stored: u64,
    /// Chunks this run generated and stored so far.
    pub generated: u64,
    /// Chunks in the requested rectangle.
    pub chunks: u64,
    /// Wall time since the command started.
    pub elapsed_ms: u128,
}

/// What [`pregenerate`] covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PregenerateReport {
    /// Block center used for the rectangle.
    pub center: [i32; 2],
    /// Requested radius in blocks.
    pub radius: u32,
    /// Inclusive chunk rectangle, `[min_x, min_z]`.
    pub chunk_min: [i32; 2],
    /// Inclusive chunk rectangle, `[max_x, max_z]`.
    pub chunk_max: [i32; 2],
    /// Chunks in the rectangle.
    pub chunks: u64,
    /// Chunks already stored before this run (skipped).
    pub already_stored: u64,
    /// Chunks generated and stored by this run.
    pub generated: u64,
    /// Wall time of the whole command, generation only after setup.
    pub elapsed_ms: u128,
}

fn storage(error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(error.to_string())
}

/// Generate every overworld chunk of the requested rectangle that the native
/// store does not cover yet, and persist them as generated baselines.
///
/// Requires a stopped world: it takes the world lease for the whole run, so a
/// running server is refused instead of racing it. A missing world directory is
/// created through the normal open path (fresh world, plugin startup included).
pub fn pregenerate(
    config: &CoreConfig,
    request: &PregenerateRequest,
    progress: &mut dyn FnMut(&PregenerateProgress),
) -> Result<PregenerateReport, CoreError> {
    let started = Instant::now();
    if request.radius == 0 {
        return Err(storage("pregenerate radius must be at least one block"));
    }
    // Open and close the world exactly like serving: this creates a fresh
    // world, validates an existing one, and yields the plugin-applied registry
    // the server generates terrain with. The lease is reacquired below, after
    // the open released it, and held for the whole generation.
    let core = Core::open(config.clone())?;
    let spawn = core.world_spawn();
    let registry = Arc::clone(core.registry());
    core.shutdown();
    let lease = std::fs::File::open(&config.world_dir).map_err(storage)?;
    lease.try_lock().map_err(|error| {
        storage(format!(
            "world {} is already leased (is the server running?): {error}",
            config.world_dir.display()
        ))
    })?;

    let center = request.center.unwrap_or([spawn[0], spawn[2]]);
    let (chunk_min, chunk_max, chunks) = rectangle(center, request.radius)?;
    let store = Arc::new(
        NativeStore::open(
            &config.world_dir.join("native/state.redb"),
            crate::native_checkpoint::native_identity(config),
        )
        .map_err(|error| {
            crate::native_checkpoint::open_refusal(config, &config.world_dir, error)
        })?,
    );

    // Coverage first: a rerun (or a resumed run) regenerates nothing.
    let dimension = crate::native_checkpoint::dimension_id(Dimension::Overworld);
    let mut covered = HashSet::new();
    store
        .visit_chunk_positions(|found_dimension, pos| {
            if found_dimension == dimension
                && (chunk_min[0]..=chunk_max[0]).contains(&pos.x)
                && (chunk_min[1]..=chunk_max[1]).contains(&pos.z)
            {
                covered.insert(pos);
            }
            Ok(())
        })
        .map_err(|error| storage(error.to_string()))?;
    let already_stored = covered.len() as u64;
    let mut missing: Vec<ChunkPos> = Vec::new();
    for z in chunk_min[1]..=chunk_max[1] {
        for x in chunk_min[0]..=chunk_max[0] {
            let pos = ChunkPos { x, z };
            if !covered.contains(&pos) {
                missing.push(pos);
            }
        }
    }
    covered.clear();
    // Nearest to the center first: an interrupted run leaves a usable core.
    let center_chunk = ChunkPos {
        x: center[0].div_euclid(16),
        z: center[1].div_euclid(16),
    };
    missing.sort_unstable_by_key(|pos| {
        let dx = i64::from(pos.x) - i64::from(center_chunk.x);
        let dz = i64::from(pos.z) - i64::from(center_chunk.z);
        (dx * dx + dz * dz, pos.x, pos.z)
    });

    let mut report = PregenerateReport {
        center,
        radius: request.radius,
        chunk_min,
        chunk_max,
        chunks: chunks as u64,
        already_stored,
        generated: 0,
        elapsed_ms: 0,
    };
    progress(&PregenerateProgress {
        already_stored,
        generated: 0,
        chunks: chunks as u64,
        elapsed_ms: started.elapsed().as_millis(),
    });
    if missing.is_empty() {
        report.elapsed_ms = started.elapsed().as_millis();
        return Ok(report);
    }

    let generator: Arc<dyn ChunkGenerator> = Arc::new(overworld_terrain(config, &registry)?);
    let source = Arc::new(ChunkSource {
        native: Some(Arc::clone(&store)),
        dimension,
        generator,
        registry: Arc::clone(&registry.blocks),
        items: Arc::clone(&registry.items),
        chest_loot: Arc::clone(&registry.chest_loot),
        light: Arc::clone(&registry.light),
        geometry: Dimension::Overworld.geometry(),
    });
    // Same pool sizing as the live world: half the cores, at least one.
    let workers = (crate::tick::worker_threads() / 2).max(1);
    let mut loader = ChunkLoader::spawn(source, workers);
    let mut next = 0_usize;
    let mut next_ticket = 0_u64;
    let mut batch: Vec<Chunk> = Vec::with_capacity(SAVE_BATCH);
    let mut last_report = Instant::now();
    loop {
        while next < missing.len() {
            let ticket = next_ticket + 1;
            if !loader.try_submit(missing[next], ticket, 0) {
                break;
            }
            next_ticket = ticket;
            next += 1;
        }
        if next == missing.len() && loader.pending() == 0 {
            break;
        }
        let (_, _, result) = loader.recv();
        match result {
            Ok((chunk, generated)) => {
                if generated {
                    batch.push(chunk);
                    report.generated += 1;
                } else {
                    // Covered between the scan and generation (never on a leased
                    // world, but keep the counts honest).
                    report.already_stored += 1;
                }
            }
            Err(error) => return Err(storage(error)),
        }
        if batch.len() >= SAVE_BATCH {
            store
                .fill_generated(dimension, &batch, &registry.blocks, &registry.items)
                .map_err(|error| storage(error.to_string()))?;
            batch.clear();
        }
        if last_report.elapsed() >= PROGRESS_INTERVAL {
            last_report = Instant::now();
            progress(&PregenerateProgress {
                already_stored,
                generated: report.generated,
                chunks: chunks as u64,
                elapsed_ms: started.elapsed().as_millis(),
            });
        }
    }
    if !batch.is_empty() {
        store
            .fill_generated(dimension, &batch, &registry.blocks, &registry.items)
            .map_err(|error| storage(error.to_string()))?;
    }
    report.elapsed_ms = started.elapsed().as_millis();
    progress(&PregenerateProgress {
        already_stored,
        generated: report.generated,
        chunks: chunks as u64,
        elapsed_ms: report.elapsed_ms,
    });
    Ok(report)
}

/// Inclusive chunk rectangle covering `center ± radius` blocks on both axes.
fn rectangle(center: [i32; 2], radius: u32) -> Result<([i32; 2], [i32; 2], i64), CoreError> {
    if radius == 0 {
        return Err(storage("pregenerate radius must be at least one block"));
    }
    let radius = i64::from(radius);
    let mut rect = [0_i64; 4];
    for (index, (coordinate, negative)) in [
        (center[0], true),
        (center[0], false),
        (center[1], true),
        (center[1], false),
    ]
    .into_iter()
    .enumerate()
    {
        let block = i64::from(coordinate) + if negative { -radius } else { radius };
        rect[index] = block.div_euclid(16);
    }
    let (min_x, max_x, min_z, max_z) = (rect[0], rect[1], rect[2], rect[3]);
    let (width, depth) = (max_x - min_x + 1, max_z - min_z + 1);
    let chunks = width.checked_mul(depth).unwrap_or(i64::MAX);
    if chunks > MAX_CHUNKS {
        return Err(storage(format!(
            "pregenerate rectangle of {chunks} chunks ({width}x{depth} chunks) exceeds the \
             {MAX_CHUNKS} chunk cap; use a smaller --radius or --center"
        )));
    }
    let axis = |value: i64| i32::try_from(value).map_err(storage);
    Ok((
        [axis(min_x)?, axis(min_z)?],
        [axis(max_x)?, axis(max_z)?],
        chunks,
    ))
}

#[cfg(test)]
#[path = "pregenerate_tests.rs"]
mod tests;
