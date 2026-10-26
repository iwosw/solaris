//! Dispenser beehive/bee-nest harvesting: bottles and shears.

use crate::boundary::{BlockPos, EntityId};
use crate::tick::TickState;
use mc_data::ItemStack;

/// Harvest a full hive at `target` with the dispenser's bottle/shears.
/// Returns true when the held item was consumed by a harvest (or refused
/// because the hive is unripe); false when this behavior does not apply.
pub(super) fn harvest(
    state: &mut TickState,
    pos: BlockPos,
    target: BlockPos,
    index: usize,
    held: &ItemStack,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) -> bool {
    let Some(name) = state
        .registry
        .items
        .name_of(held.item_id)
        .map(|id| id.path())
    else {
        return false;
    };
    let bottle = name == "glass_bottle";
    let shears = name == "shears";
    if !bottle && !shears {
        return false;
    }
    // Only hives/nests admit harvesting; other targets eject the item.
    let Some(level) = hive_honey(state, target) else {
        return false;
    };
    if level < 5 {
        return true;
    }
    let product = state
        .registry
        .items
        .id_of(
            &mc_data::Identifier::parse(if bottle {
                "minecraft:honey_bottle"
            } else {
                "minecraft:honeycomb"
            })
            .unwrap(),
        )
        .unwrap();
    let product = ItemStack::new(product, if bottle { 1 } else { 3 });
    // Debit the dispenser slot: bottle consumed into honey bottle custody,
    // shears take wear and stay.
    let key = (pos.x, pos.y, pos.z);
    if bottle {
        mc_data::inventory_semantics_26_1_2::decrement_stack(
            &mut state
                .block_entities
                .get_mut(&key)
                .expect("hive-harvest custody")
                .slots[index],
        );
        let slot = &mut state
            .block_entities
            .get_mut(&key)
            .expect("hive-harvest custody")
            .slots[index];
        if slot.is_empty() {
            *slot = product;
        } else {
            spawned.push(crate::tick::spawn_item_entity(
                state,
                crate::bees::centre(target),
                product,
            ));
        }
    } else {
        let slot = &mut state
            .block_entities
            .get_mut(&key)
            .expect("hive-harvest custody")
            .slots[index];
        let damage = slot.damage.unwrap_or(0).saturating_add(1);
        let max = mc_data::item_semantics_26_1_2::max_damage_for_stack(
            &state.registry.item_facts,
            &state.registry.items,
            slot,
        );
        if max.is_some_and(|max| damage >= max) {
            *slot = ItemStack::EMPTY;
        } else {
            slot.damage = Some(damage);
        }
        for _ in 0..3 {
            spawned.push(crate::tick::spawn_item_entity(
                state,
                crate::bees::centre(target),
                ItemStack::new(product.item_id, 1),
            ));
        }
    }
    // Reset honey level; bees inside emerge angered absent campfire smoke
    if let Some(next) = crate::bees::property(state, target, "honey_level", "0") {
        state.world.set_block(target.x, target.y, target.z, next);
        writes.push((target, next));
    }
    if !crate::bees::smoke(state, target) {
        // Vanilla dispenser harvest angers nearby bees absent smoke. The
        // hive's retained occupants keep their normal exit schedule.
        crate::bees::anger_nearby(state, crate::bees::centre(target));
    }
    true
}

/// Honey level of a beehive/bee nest, or None for other blocks.
fn hive_honey(state: &TickState, target: BlockPos) -> Option<u8> {
    let current = state.world.block_at(target.x, target.y, target.z)?;
    let block = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))?;
    if !matches!(block.block.id.path(), "beehive" | "bee_nest") {
        return None;
    }
    block
        .properties
        .iter()
        .find(|(key, _)| key == "honey_level")?
        .1
        .parse()
        .ok()
}
