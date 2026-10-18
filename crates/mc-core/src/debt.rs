//! Per-chunk simulation clock and persisted catch-up debt (decision 001-f).
//!
//! `last_sim` advances only during resident active simulation. Inactive time
//! is materialized as `remaining` on activation, excluding the activation's
//! live tick. The tick merge owner spends one bounded Delta for the whole
//! chunk and journals debt minus Delta alongside its effects. No catch-up
//! path requests chunk loading. Accepted non-catch-up block/container writes
//! and item births/chunk crossings cancel old debt in that same WAL batch;
//! no target fingerprints or revisions are needed.
//!
//! Active boundary images project their live anchor to the recovered WAL
//! tick; inactive boundaries freeze. Remaining debt never projects. Every
//! authority change pins the chunk with a storage-only revision until a
//! matching checkpoint ACK. Native extras and checkpoint/WAL images carry
//! the same absolute anchors, including zero images after cancellation.

use crate::config::CoreError;
use mc_world::Chunk;

/// Root-extra key for the authoritative last-simulated tick.
pub(crate) const LAST_SIM_KEY: &str = "SolarisSimLastTick";
/// Root-extra key for outstanding pre-existing catch-up debt.
pub(crate) const REMAINING_KEY: &str = "SolarisSimDebt";

/// Per-chunk runtime debt authority. Active membership is recomputed by
/// [`crate::world::ChunkWorld::set_active_chunks`]; the WAL image separately
/// persists the open clock interval so recovery can project it.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SimDebtRecord {
    /// Live simulation anchor; outstanding abstract time is separate.
    pub last_sim: u64,
    /// Dormant catch-up time still owed to this entire chunk.
    pub remaining: u64,
    /// Newer than the last acknowledged checkpoint; pins eviction.
    pub dirty: bool,
    /// Resident+member at the last [`crate::world::ChunkWorld::set_active_chunks`].
    pub active: bool,
}

/// Native NBT longs cannot represent authority above the signed range.
/// Validate before any mutation so a failed capture cannot normalize debt.
pub(crate) fn native_values(
    pos: mc_world::ChunkPos,
    last_sim: u64,
    remaining: u64,
) -> Result<(i64, i64), CoreError> {
    let encode =
        |key, value| {
            i64::try_from(value).map_err(|_| CoreError::Storage(format!(
            "simulation debt at chunk ({}, {}): {key} value {value} exceeds native Long range",
            pos.x, pos.z,
        )))
        };
    Ok((
        encode(LAST_SIM_KEY, last_sim)?,
        encode(REMAINING_KEY, remaining)?,
    ))
}

/// Project authority values into a captured chunk's root extras (durable
/// only; never read back on the tick path, which uses the record map).
pub(crate) fn project(chunk: &mut Chunk, last_sim: u64, remaining: u64) -> Result<(), CoreError> {
    let (last_sim, remaining) = native_values(chunk.pos, last_sim, remaining)?;
    chunk
        .extras
        .retain(|(key, _)| key != LAST_SIM_KEY && key != REMAINING_KEY);
    chunk
        .extras
        .push((LAST_SIM_KEY.into(), mc_nbt::Tag::Long(last_sim)));
    chunk
        .extras
        .push((REMAINING_KEY.into(), mc_nbt::Tag::Long(remaining)));
    chunk.mark_dirty();
    Ok(())
}

/// Validate persisted authority before admission, even if newer WAL authority
/// wins. Only an entirely absent pair is a new chunk; malformed or partial
/// images refuse admission without mutating either the chunk or live record.
pub(crate) fn adopt_native_extras(
    pos: mc_world::ChunkPos,
    chunk: &std::sync::Arc<Chunk>,
    records: &mut rustc_hash::FxHashMap<mc_world::ChunkPos, SimDebtRecord>,
) -> Result<(), CoreError> {
    let read = |key: &str| -> Result<Option<u64>, CoreError> {
        let mut values = chunk.extras.iter().filter(|(name, _)| name == key);
        let Some((_, tag)) = values.next() else {
            return Ok(None);
        };
        let invalid = |reason| {
            CoreError::Storage(format!(
                "simulation debt at chunk ({}, {}): {key} {reason}",
                pos.x, pos.z,
            ))
        };
        if values.next().is_some() {
            return Err(invalid("is duplicated"));
        }
        match tag {
            mc_nbt::Tag::Long(value) => u64::try_from(*value)
                .map(Some)
                .map_err(|_| invalid("must be nonnegative")),
            _ => Err(invalid("must be a Long")),
        }
    };
    let (last_sim, remaining) = match (read(LAST_SIM_KEY)?, read(REMAINING_KEY)?) {
        (None, None) => return Ok(()),
        (Some(last_sim), Some(remaining)) => (last_sim, remaining),
        _ => {
            return Err(CoreError::Storage(format!(
                "simulation debt at chunk ({}, {}): {LAST_SIM_KEY} and {REMAINING_KEY} must both be present",
                pos.x, pos.z,
            )));
        }
    };
    records.entry(pos).or_insert(SimDebtRecord {
        last_sim,
        remaining,
        dirty: false,
        active: false,
    });
    Ok(())
}
