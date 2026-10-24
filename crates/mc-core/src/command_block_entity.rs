//! Prepared command block-entity images, installed after old-container removal.
use crate::boundary::BlockPos;
use crate::tick::{BlockEntityState, TickState};
use mc_data::block_entity_kinds::BlockEntityKind;
use mc_nbt::Tag;
use std::sync::Arc;

#[derive(Debug, Clone)]
pub(crate) struct Prepared {
    fields: Vec<(String, Tag)>,
    storage: Option<BlockEntityState>,
}

pub(crate) fn prepare(
    state: &TickState,
    block: u32,
    bytes: Option<&[u8]>,
) -> Result<Option<Prepared>, String> {
    let Some(bytes) = bytes else {
        return Ok(None);
    };
    let Some(kind) = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(block))
        .and_then(|block| BlockEntityKind::for_block(block.block.id.path()))
    else {
        return Ok(None);
    };
    let Tag::Compound(mut fields) =
        mc_nbt::read_network(&mut &*bytes).map_err(|error| error.to_string())?
    else {
        return Err("Block entity data must be a compound".into());
    };
    fields.retain(|(name, _)| !matches!(name.as_str(), "id" | "x" | "y" | "z"));
    fields.push(("id".into(), Tag::String(kind.vanilla_id().into())));
    mc_data::storage_blocks::validate(&fields, &state.registry.items)?;
    mc_data::trial_nbt::validate(&fields, Some(&state.registry.items))?;
    if kind == BlockEntityKind::Conduit {
        mc_data::conduit_nbt::target(&fields)?;
    }
    let storage = if matches!(
        kind,
        BlockEntityKind::Furnace | BlockEntityKind::Smoker | BlockEntityKind::BlastFurnace
    ) {
        let furnace = mc_world::anvil::chunk_nbt::decode_furnace(&fields, &state.registry.items)
            .map_err(|error| error.to_string())?;
        Some(BlockEntityState {
            slots: furnace
                .slots
                .iter()
                .map(crate::tick::furnace_slot_to_stack)
                .collect(),
            furnace: Some(crate::inventory::furnace::FurnaceState {
                lit_remaining: i32::from(furnace.burn_remaining),
                lit_total: i32::from(furnace.burn_total),
                cook_progress: i32::from(furnace.cook_progress),
                cook_total: i32::from(furnace.cook_total),
                recipes_used: Arc::new(furnace.recipes_used),
            }),
        })
    } else if let Some(size) = kind.inventory_size() {
        Some(BlockEntityState {
            slots: mc_data::storage_blocks::decode_slots(&fields, &state.registry.items, size)?,
            furnace: None,
        })
    } else {
        None
    };
    // Resident hydration must never quarantine a chunk because a command supplied malformed data.
    let mut normalized = Vec::new();
    mc_nbt::write_network(&mut normalized, &Tag::Compound(fields.clone()))
        .map_err(|error| error.to_string())?;
    match kind {
        BlockEntityKind::Beehive => {
            crate::bees::Hive::decode(&normalized).map_err(|error| error.to_string())?;
        }
        BlockEntityKind::Jukebox => {
            crate::jukebox::Jukebox::decode(&normalized, &state.registry.items)
                .map_err(|error| error.to_string())?;
        }
        BlockEntityKind::MobSpawner => {
            crate::monster_spawner::Spawner::decode(&normalized, &state.registry)?;
        }
        BlockEntityKind::TrialSpawner => {
            crate::trial_spawner::Spawner::decode(&normalized, &state.registry.items)?;
        }
        _ => {}
    }
    Ok(Some(Prepared { fields, storage }))
}

pub(crate) fn install(state: &mut TickState, pos: BlockPos, mut image: Prepared) {
    let cell = (pos.x, pos.y, pos.z);
    image.fields.extend([
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(image.fields))
        .expect("validated block entity image");
    state.block_entities.remove(&cell);
    state.brewing.remove(&cell);
    state.crafters.remove(&cell);
    state.lecterns.remove(&cell);
    state.jukeboxes.remove(&cell);
    state.hives.remove(&cell);
    state.campfires.remove(&cell);
    state.trial_spawners.remove(&cell);
    state.vaults.remove(&cell);
    state.world.install_command_block_entity(pos, bytes.clone());
    state.opaque_changes.push((pos, Some(bytes)));
    if let Some(storage) = image.storage {
        state.block_entities.insert(cell, storage);
    }
    crate::monster_spawner::forget(&mut state.monster_spawners, pos);
    state.block_entities.mark_dirty(cell);
}
