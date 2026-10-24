//! Water-cauldron washing: dyed armor/shulker color and banner patterns.
//!
//! Vanilla lowers the water by one level per washed item and washes exactly
//! one item (or one banner layer) per use, preserving unrelated components.
use mc_data::{Identifier, ItemStack};

use crate::boundary::{BlockPos, ChangeSet, EntityId, Hand};
use crate::tick::{TickState, spawn_item_entity};

/// Dyed leather goods vanilla cauldrons can wash.
fn dyeable(path: &str) -> bool {
    matches!(
        path,
        "leather_helmet"
            | "leather_chestplate"
            | "leather_leggings"
            | "leather_boots"
            | "leather_horse_armor"
            | "wolf_armor"
    )
}

fn has_dyed_color(stack: &ItemStack) -> bool {
    stack
        .extended()
        .preserved_components
        .iter()
        .any(|component| component.name == "minecraft:dyed_color")
}

/// The washed single item for `held`, or `None` when it is not washable.
fn washed(state: &TickState, held: &ItemStack) -> Option<ItemStack> {
    let name = state.registry.items.name_of(held.item_id)?;
    let path = name.path().to_owned();
    if has_dyed_color(held) && dyeable(&path) {
        let mut out = held.clone();
        out.count = 1;
        out.extended_mut()
            .preserved_components
            .retain(|component| component.name != "minecraft:dyed_color");
        return Some(out);
    }
    if !held.extended().banner_patterns.is_empty() {
        let mut out = held.clone();
        out.count = 1;
        out.extended_mut().banner_patterns.pop();
        return Some(out);
    }
    if path != "shulker_box" && path.ends_with("_shulker_box") {
        let plain = state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:shulker_box").ok()?)?;
        let mut out = held.clone();
        out.count = 1;
        out.item_id = plain;
        return Some(out);
    }
    None
}

fn water_level(state: &TickState, pos: BlockPos) -> Option<u8> {
    let block = state.registry.blocks.by_id(mc_world::BlockStateId(
        state.world.block_at(pos.x, pos.y, pos.z)?,
    ))?;
    if block.block.id.as_str() != "minecraft:water_cauldron" {
        return None;
    }
    block
        .properties
        .iter()
        .find(|(key, _)| key == "level")?
        .1
        .parse()
        .ok()
}

/// Wash one held item in a water cauldron, debiting one water level.
/// Returns true when the click was consumed (even without mutation).
pub(crate) fn use_on(
    state: &mut TickState,
    entity: EntityId,
    hand: Hand,
    pos: BlockPos,
    changes: &mut ChangeSet,
) -> bool {
    let Some(level) = water_level(state, pos) else {
        return false;
    };
    if level == 0 {
        return false;
    }
    let Some(player) = state.players.get(&entity) else {
        return false;
    };
    if player.spectator() {
        return false;
    }
    let slot = crate::tick::held_slot_index(player, hand);
    let held = player
        .inventory
        .slots
        .get(slot)
        .cloned()
        .unwrap_or(ItemStack::EMPTY);
    if held.is_empty() {
        return false;
    }
    let Some(clean) = washed(state, &held) else {
        return false;
    };
    let connection = state.players[&entity].connection;
    if crate::tick::held_debit_below_floor(state, entity, hand, 1) {
        return true;
    }
    // Lower one water level (1 becomes an empty cauldron), visible to merge.
    let next = if level <= 1 {
        state
            .registry
            .blocks
            .block(&Identifier::parse("minecraft:cauldron").expect("vanilla block"))
            .expect("cauldron")
            .default
            .0
    } else {
        state
            .registry
            .blocks
            .by_name_and_props(
                &Identifier::parse("minecraft:water_cauldron").expect("vanilla block"),
                &[("level".into(), (level - 1).to_string())],
            )
            .expect("water cauldron level")
            .0
    };
    state.world.set_block(pos.x, pos.y, pos.z, next);
    if let Some((_, value)) = state
        .merge_block_writes
        .iter_mut()
        .find(|(cell, _)| *cell == pos)
    {
        *value = next;
    } else {
        state.merge_block_writes.push((pos, next));
    }
    // Take the dirty item, return its washed single; overflow drops.
    let effects = state
        .players
        .get_mut(&entity)
        .expect("player exists")
        .consume_held(hand, 1);
    for change in effects.changes {
        state.push_player(connection, change);
    }
    let mut rest = clean;
    let rules = state.registry.item_rules();
    state
        .players
        .get_mut(&entity)
        .expect("player exists")
        .inventory
        .add(&rules, &mut rest);
    let contents = state.players[&entity].inventory_contents();
    state.push_player(connection, contents);
    if !rest.is_empty()
        && let Some(slot) = state.entities.slot_of(entity)
    {
        let at = state.entities.pos(slot);
        let drop = spawn_item_entity(state, at, rest);
        changes.entities_spawned.push(drop);
    }
    true
}
