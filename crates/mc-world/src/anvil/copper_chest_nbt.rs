//! Copper chests share vanilla ChestBlockEntity storage and retain its metadata.
use super::chunk_nbt::ChunkNbtError;
use crate::block::{BlockRegistry, BlockStateId};
use crate::chunk::ChestBlockEntity;
use mc_data::items::ItemRegistry;
use mc_data::loot::chest_26_1_2::ChestLootCatalog;

pub(super) fn is_copper(registry: &BlockRegistry, id: BlockStateId) -> bool {
    registry
        .by_id(id)
        .is_some_and(|state| state.block.id.path().ends_with("copper_chest"))
}

/// Structure-container loot becomes concrete custody once when importing the
/// native chunk. Thereafter WAL/checkpoints save Items, never another loot roll.
pub(super) fn materialize(
    chest: &mut ChestBlockEntity,
    items: &ItemRegistry,
    chest_loot: &ChestLootCatalog,
) -> Result<(), ChunkNbtError> {
    let Some(id) = &chest.loot_table else {
        return Ok(());
    };
    if chest.slots.iter().any(|slot| !slot.is_empty()) {
        return Err(ChunkNbtError::InvalidBlockEntity(
            "copper chest LootTable: unresolved loot has populated Items".into(),
        ));
    }
    let table = chest_loot.get(id).ok_or_else(|| {
        ChunkNbtError::InvalidBlockEntity(format!(
            "copper chest LootTable {id}: unsupported saved container table"
        ))
    })?;
    if table.requires_world_context() {
        return Ok(());
    }
    let mut rng = mc_data::loot::chest_26_1_2::ChestRng::new(chest.loot_seed as u64);
    let stacks = table.roll(&mut rng, items, 0.0).map_err(|error| {
        ChunkNbtError::InvalidBlockEntity(format!("copper chest LootTable {id}: {error}"))
    })?;
    let mut available = std::array::from_fn::<_, 27, _>(|index| index);
    let mut remaining = available.len();
    for stack in stacks {
        if remaining == 0 {
            break;
        }
        let choice = rng.next_bounded(remaining as u64) as usize;
        let slot = available[choice];
        remaining -= 1;
        available[choice] = available[remaining];
        chest.slots[slot] = crate::chunk::FurnaceSlot {
            count: stack.count,
            item_id: stack.item_id,
            damage: stack.damage,
            enchantments: stack.enchantments,
            custom_name: stack.custom_name.map(String::from),
            item_model: stack.item_model,
            stew_effects: stack.stew_effects,
            components: stack.components,
        };
    }
    chest.loot_table = None;
    chest.loot_seed = 0;
    Ok(())
}
