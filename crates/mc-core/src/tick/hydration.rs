//! Single owner of persisted block-entity hydration for newly resident chunks.
//!
//! Policy: every fallible record of a chunk is decoded before any live store is
//! touched. One malformed record rejects the whole chunk through
//! `ChunkWorld::reject_resident` (chunk unloaded, load error recorded); live
//! stores never see a partial chunk, so there is nothing to roll back. Live
//! entries always win: hydration only fills vacant positions.

use std::sync::Arc;

use super::{BlockEntityState, TickState, furnace_slot_to_stack};
use crate::boundary::ChunkPos;

/// Owned decode of one chunk's fallible block-entity records.
struct Decoded {
    monster_spawners: crate::monster_spawner::Decoded,
    trial_spawners: Vec<((i32, i32, i32), crate::trial_spawner::Spawner)>,
    vaults: Vec<((i32, i32, i32), mc_data::trial_nbt::VaultData)>,
    crafters: crate::crafter::Decoded,
    shelves: Vec<((i32, i32, i32), Vec<mc_data::ItemStack>)>,
    shulkers: Vec<((i32, i32, i32), Vec<mc_data::ItemStack>)>,
    special_storage: Vec<((i32, i32, i32), Vec<mc_data::ItemStack>)>,
    chiseled: Vec<((i32, i32, i32), Vec<mc_data::ItemStack>)>,
    hives: Vec<((i32, i32, i32), crate::bees::Hive)>,
    jukeboxes: Vec<((i32, i32, i32), crate::jukebox::Jukebox)>,
    campfires: Vec<((i32, i32, i32), crate::campfire::CampfireState)>,
    lecterns: crate::lectern::Decoded,
    brewing: Vec<(
        (i32, i32, i32),
        crate::brewing::BrewingState,
        Vec<mc_data::ItemStack>,
    )>,
    containers: crate::redstone::containers::Decoded,
}

/// Records read lazily from chunk bytes at runtime (beacon, conduit, sculk,
/// shrieker) are validated after the typed owners, so a record's own owner
/// names the rejection; those readers may then treat decode as an invariant.
fn decode(state: &TickState, chunk: &mc_world::Chunk) -> Result<Decoded, String> {
    let registry = &state.registry;
    let decoded = Decoded {
        monster_spawners: crate::monster_spawner::decode_chunk(registry, chunk)?,
        trial_spawners: crate::trial_spawner::decode_chunk(&state.trial_spawners, registry, chunk)?,
        vaults: crate::vault::decode_chunk(&state.vaults, registry, chunk)?,
        crafters: crate::crafter::decode_chunk(&state.crafters, registry, chunk)?,
        shelves: crate::shelf::decode_chunk(&state.block_entities, registry, chunk)?,
        special_storage: crate::special_storage::decode_chunk(
            &state.block_entities,
            registry,
            chunk,
        )?,
        chiseled: crate::chiseled_shelf::decode_chunk(&state.block_entities, registry, chunk)?,
        shulkers: crate::shulker::decode_chunk(&state.block_entities, registry, chunk)
            .map_err(|e| e.to_string())?,
        hives: crate::bees::decode_chunk(&state.hives, registry, chunk)
            .map_err(|e| e.to_string())?,
        jukeboxes: crate::jukebox::decode_chunk(&state.jukeboxes, registry, chunk)
            .map_err(|e| e.to_string())?,
        campfires: crate::campfire::decode_chunk(&state.campfires, registry, chunk)?,
        lecterns: crate::lectern::decode_chunk(&state.lecterns, registry, chunk)?,
        brewing: crate::brewing::decode_chunk(&state.brewing, registry, chunk)?,
        containers: crate::redstone::containers::decode_chunk(
            &state.block_entities,
            &state.redstone_containers,
            registry,
            chunk,
            state.tick,
        )?,
    };
    crate::beacon::validate_chunk(state, chunk)?;
    crate::conduit::validate_chunk(state, chunk)?;
    crate::sculk::validate_chunk(state, chunk)?;
    crate::shrieker::validate_chunk(state, chunk)?;
    Ok(decoded)
}

/// Pull resident chunks' typed block-entity maps into live storage.
/// Journal post-images overwrite per position; unloaded chunks' records stay
/// until their chunk reloads (eviction does not delete storage).
pub(crate) fn sync_chunk_block_entities(state: &mut TickState) {
    for chunk_pos in state.world.take_hydration_chunks() {
        let Some(chunk) = state.world.chunk(chunk_pos) else {
            continue;
        };
        match decode(state, chunk) {
            Ok(decoded) => apply(state, chunk_pos, decoded),
            Err(error) => state.world.reject_resident(chunk_pos, error),
        }
    }
}

fn apply(state: &mut TickState, chunk_pos: ChunkPos, decoded: Decoded) {
    crate::monster_spawner::apply_chunk(
        &mut state.monster_spawners,
        chunk_pos,
        decoded.monster_spawners,
    );
    state.trial_spawners.extend(decoded.trial_spawners);
    state.vaults.extend(decoded.vaults);
    crate::crafter::apply_chunk(
        &mut state.crafters,
        &mut state.block_entities,
        decoded.crafters,
    );
    for (key, slots) in decoded
        .shelves
        .into_iter()
        .chain(decoded.special_storage)
        .chain(decoded.chiseled)
        .chain(decoded.shulkers)
    {
        state.block_entities.hydrate(key, || BlockEntityState {
            slots,
            furnace: None,
        });
    }
    state.hives.extend(decoded.hives);
    crate::jukebox::apply_chunk(
        &mut state.jukeboxes,
        &mut state.block_entities,
        decoded.jukeboxes,
    );
    let chunk = state
        .world
        .chunk(chunk_pos)
        .expect("decoded chunk stays resident until apply");
    state.campfires.extend(decoded.campfires);
    crate::lectern::apply_chunk(
        &mut state.lecterns,
        &mut state.block_entities,
        decoded.lecterns,
    );
    crate::brewing::hydrate_chunk(
        &mut state.brewing,
        &mut state.block_entities,
        &state.registry,
        chunk,
        decoded.brewing,
    );
    state.block_ticks.hydrate_chunk(chunk, state.tick);
    crate::redstone::containers::hydrate_chunk(
        &mut state.block_entities,
        &mut state.redstone_containers,
        &state.registry,
        chunk,
        state.tick,
        decoded.containers,
    );
    for (block, chest) in &chunk.chests {
        if chest.loot_table.is_some() {
            continue;
        }
        state
            .block_entities
            .hydrate((block.x, block.y, block.z), || BlockEntityState {
                slots: chest.slots.iter().map(furnace_slot_to_stack).collect(),
                furnace: None,
            });
    }
    for (block, furnace) in &chunk.furnaces {
        // Hydrate ONLY on a vacant live entry: the live record is
        // authoritative (player mutations, replay images). Overwriting
        // it from the chunk every tick restores extracted items and
        // loses inserted ones.
        if state
            .block_entities
            .contains_key(&(block.x, block.y, block.z))
        {
            continue;
        }
        let mut slots = vec![mc_data::ItemStack::EMPTY; 3];
        for (index, slot) in furnace.slots.iter().enumerate() {
            slots[index] = furnace_slot_to_stack(slot);
        }
        state
            .block_entities
            .hydrate((block.x, block.y, block.z), || BlockEntityState {
                slots,
                furnace: Some(crate::inventory::furnace::FurnaceState {
                    lit_remaining: i32::from(furnace.burn_remaining),
                    lit_total: i32::from(furnace.burn_total),
                    cook_progress: i32::from(furnace.cook_progress),
                    cook_total: i32::from(furnace.cook_total),
                    recipes_used: Arc::new(furnace.recipes_used.clone()),
                }),
            });
    }
}

#[cfg(test)]
#[path = "hydration_tests.rs"]
mod tests;
