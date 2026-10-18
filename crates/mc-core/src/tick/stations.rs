//! Block-facing workstation lifecycle; all input stacks remain player-owned.

use std::sync::Arc;

use super::{TickState, apply_player_effects};
use crate::boundary::{BlockChange, BlockPos, ChangeSet, ConnectionId, EntityId, PlayerChange};
use crate::inventory::MenuKind;

pub(super) fn craft_menu_for_block(name: &str) -> Option<(MenuKind, String, usize)> {
    let (kind, title) = match name {
        "minecraft:smithing_table" => (MenuKind::Smithing, "Smithing"),
        "minecraft:loom" => (MenuKind::Loom, "Loom"),
        "minecraft:stonecutter" => (MenuKind::Stonecutter, "Stonecutter"),
        _ => return None,
    };
    Some((kind, title.into(), 0))
}
pub(super) fn bookshelves_for_player(state: &TickState, entity: EntityId) -> i32 {
    if !state
        .players
        .get(&entity)
        .and_then(|player| player.open.as_ref())
        .is_some_and(|menu| menu.kind == MenuKind::EnchantingTable)
    {
        return 0;
    }
    state
        .open_containers
        .get(&entity)
        .map_or(0, |&(x, y, z)| bookshelves(state, BlockPos { x, y, z }))
}
fn bookshelves(state: &TickState, position: BlockPos) -> i32 {
    // Reuse memberships across the whole shelf/gap ring. These are borrowed
    // only for this scan, so the next scan sees public tag registry mutations.
    let providers = state
        .registry
        .tags
        .raw_ids("minecraft:block", "minecraft:enchantment_power_provider")
        .unwrap_or(&[]);
    let transmitters = state
        .registry
        .tags
        .raw_ids("minecraft:block", "minecraft:enchantment_power_transmitter")
        .unwrap_or(&[]);
    let tagged = |pos: BlockPos, raw_ids: &[i32], builtin: &str| {
        state
            .world
            .block_at(pos.x, pos.y, pos.z)
            .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
            .is_some_and(|block| {
                block.block.id.as_str() == builtin || raw_ids.contains(&block.block.raw_id)
            })
    };
    count_bookshelves(
        position,
        |pos| tagged(pos, providers, "minecraft:bookshelf"),
        |pos| tagged(pos, transmitters, "minecraft:air"),
    )
}

fn count_bookshelves(
    position: BlockPos,
    provider: impl Fn(BlockPos) -> bool,
    transmitter: impl Fn(BlockPos) -> bool,
) -> i32 {
    let mut count = 0;
    // EnchantingTableBlock.BOOKSHELF_OFFSETS: outer ring at y=0 and y=1.
    for y in 0..=1 {
        for z in -2_i32..=2 {
            for x in -2_i32..=2 {
                if x.abs() != 2 && z.abs() != 2 {
                    continue;
                }
                let shelf = BlockPos {
                    x: position.x + x,
                    y: position.y + y,
                    z: position.z + z,
                };
                let gap = BlockPos {
                    x: position.x + x / 2,
                    y: position.y + y,
                    z: position.z + z / 2,
                };
                if provider(shelf) && transmitter(gap) {
                    count += 1;
                }
            }
        }
    }
    count.min(15)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn open(
    state: &mut TickState,
    entity: EntityId,
    connection: ConnectionId,
    position: BlockPos,
    kind: MenuKind,
    title: String,
    changes: &mut ChangeSet,
) {
    let shelves = if kind == MenuKind::EnchantingTable {
        bookshelves(state, position)
    } else {
        0
    };
    let registry = Arc::clone(&state.registry);
    let rules = registry.item_rules();
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    let mut effects = player.open_menu(&rules, kind, title, &[]);
    let initialized = player.initialize_station(&registry.enchantments, &rules, shelves);
    effects.changes.extend(initialized.changes);
    let stat = match kind {
        MenuKind::Anvil => Some(mc_data::statistics::CustomStat::InteractWithAnvil),
        MenuKind::Grindstone => Some(mc_data::statistics::CustomStat::InteractWithGrindstone),
        MenuKind::Stonecutter => Some(mc_data::statistics::CustomStat::InteractWithStonecutter),
        MenuKind::Loom => Some(mc_data::statistics::CustomStat::InteractWithLoom),
        MenuKind::Smithing => Some(mc_data::statistics::CustomStat::InteractWithSmithingTable),
        _ => None,
    };
    if let Some(stat) = stat {
        player.statistics.increment_custom(stat, 1);
    }
    // Apply closure effects before replacing the old block association.
    apply_player_effects(state, connection, entity, effects, changes);
    state
        .open_containers
        .insert(entity, (position.x, position.y, position.z));
}

pub(super) fn damage_anvil(state: &mut TickState, entity: EntityId, changes: &mut ChangeSet) {
    let Some(&(x, y, z)) = state.open_containers.get(&entity) else {
        return;
    };
    let Some(current) = state.world.block_at(x, y, z) else {
        return;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(current)) else {
        return;
    };
    let next_name = match block.block.id.as_str() {
        "minecraft:anvil" => Some("minecraft:chipped_anvil"),
        "minecraft:chipped_anvil" => Some("minecraft:damaged_anvil"),
        "minecraft:damaged_anvil" => None,
        _ => return,
    };
    let revision = state
        .players
        .get(&entity)
        .and_then(|player| player.open.as_ref())
        .map_or(0, |menu| menu.state_id);
    let salt =
        state.tick ^ u64::from(entity.0 as u32).rotate_left(17) ^ (revision as u64).rotate_left(39);
    // Decorrelate neighboring ticks before the vanilla floating-point draw.
    let mut salt = salt.wrapping_add(0x9e3779b97f4a7c15);
    salt = (salt ^ (salt >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    salt = (salt ^ (salt >> 27)).wrapping_mul(0x94d049bb133111eb);
    let salt = salt ^ (salt >> 31);
    if crate::inventory::enchanting::LegacyRandom::new(salt as i64).next_float() >= 0.12 {
        return;
    }
    let next = match next_name {
        Some(name) => mc_data::Identifier::parse(name)
            .ok()
            .and_then(|name| {
                state
                    .registry
                    .blocks
                    .by_name_and_props(&name, &block.properties)
            })
            .map(|id| id.0),
        None => Some(state.registry.states.air),
    };
    let Some(next) = next else {
        return;
    };
    state.world.set_block(x, y, z, next);
    changes.blocks.push(BlockChange {
        position: BlockPos { x, y, z },
        state: next,
    });
    if next_name.is_none() {
        let registry = Arc::clone(&state.registry);
        if let Some(player) = state.players.get_mut(&entity) {
            let window = player.open.as_ref().map(|menu| menu.window);
            let mut effects = player.close(&registry.item_rules());
            if let Some(window) = window {
                effects
                    .changes
                    .insert(0, PlayerChange::MenuClosed { window });
            }
            let connection = player.connection;
            state.open_containers.remove(&entity);
            apply_player_effects(state, connection, entity, effects, changes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bookshelf_ring_obeys_gap_height_diagonals_and_cap() {
        let at = BlockPos { x: 0, y: 65, z: 0 };
        let shelf = BlockPos { x: 2, y: 66, z: 2 };
        let gap = BlockPos { x: 1, y: 66, z: 1 };
        assert_eq!(count_bookshelves(at, |pos| pos == shelf, |_| true), 1);
        assert_eq!(
            count_bookshelves(at, |pos| pos == shelf, |pos| pos != gap),
            0
        );
        assert_eq!(
            count_bookshelves(at, |pos| pos.x.abs() < 2 && pos.z.abs() < 2, |_| true),
            0
        );
        assert_eq!(count_bookshelves(at, |_| true, |_| true), 15);
    }
}
