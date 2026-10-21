//! Offline operator path that keeps a native world across a `WORLDGEN_REVISION` bump.
//!
//! The default open refuses any world whose header identity or generation/dimension
//! records name another revision. This command rewrites exactly those three values, and
//! only for a stopped world whose identity differs from the configured one in nothing but
//! the (older) revision. Stored chunks, entities and gameplay state are not touched: only
//! chunks generated after the upgrade use the new generator.

use crate::native_checkpoint::{
    generation_records_at, native_identity_at, revision_only_difference,
};
use crate::{CoreConfig, CoreError};
use mc_world::native::{NativeGenerationContract, NativeStore};

/// What [`upgrade_worldgen_revision`] found and did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldgenUpgrade {
    /// Revision recorded in the world before the command.
    pub from_revision: u32,
    /// Revision recorded afterwards (the running build's).
    pub to_revision: u32,
    /// False when the world already recorded `to_revision` and nothing was written.
    pub rewritten: bool,
}

fn contract(config: &CoreConfig, revision: u32) -> Result<NativeGenerationContract, CoreError> {
    let mut records = generation_records_at(config, revision)?;
    Ok(NativeGenerationContract {
        identity: native_identity_at(config, revision),
        generation: records.remove("generation").expect("generated").payload,
        dimensions: records.remove("dimensions").expect("generated").payload,
    })
}

/// Rewrite the header identity and generation/dimension records of the stopped world at
/// `config.world_dir` to the current `WORLDGEN_REVISION`, in one durable transaction.
///
/// Refuses (changing nothing) when the world is leased by a running server, has no native
/// store, records a different seed/format/geometry, or records a newer revision.
/// Running it again on an upgraded world verifies the contract and writes nothing.
pub fn upgrade_worldgen_revision(config: &CoreConfig) -> Result<WorldgenUpgrade, CoreError> {
    let storage = |error: mc_world::native::NativeError| CoreError::Storage(error.to_string());
    let lease = std::fs::File::open(&config.world_dir)
        .map_err(|e| CoreError::Storage(format!("world {}: {e}", config.world_dir.display())))?;
    lease.try_lock().map_err(|e| {
        CoreError::Storage(format!(
            "world {} is already leased (is the server running?): {e}",
            config.world_dir.display()
        ))
    })?;
    let path = config.world_dir.join("native/state.redb");
    if !path.is_file() {
        return Err(crate::native_checkpoint::import_required(config));
    }
    let current = mc_worldgen::WORLDGEN_REVISION;
    let stored_identity = NativeStore::read_identity(&path).map_err(storage)?;
    let Some(stored) = revision_only_difference(config, stored_identity) else {
        return Err(CoreError::Storage(
            "world identity differs from the configured seed or format, not only in the worldgen \
             revision; refusing to upgrade"
                .into(),
        ));
    };
    if stored > current {
        return Err(CoreError::Storage(format!(
            "world records worldgen revision {stored}, newer than this build's {current}; \
             refusing to downgrade"
        )));
    }
    NativeStore::replace_generation_contract(
        &path,
        &contract(config, stored)?,
        &contract(config, current)?,
    )
    .map_err(storage)?;
    Ok(WorldgenUpgrade {
        from_revision: stored,
        to_revision: current,
        rewritten: stored != current,
    })
}

#[cfg(test)]
#[path = "worldgen_upgrade_tests.rs"]
mod tests;
