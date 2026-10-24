//! Iron and snow golem construction and iron-ingot repair.
//! Placing a carved pumpkin or jack o'lantern completes the pattern; the body
//! and head are cleared with block-break particles and the golem is born in the
//! same journal tick. Iron golems are marked `PlayerCreated`.
use crate::boundary::{BlockPos, PlayerChange};
use crate::special_mobs::Retained;
use crate::tick::TickState;
use mc_data::Identifier;
use mc_world::BlockStateId;

/// `LevelEvent` for block-break particles and sound, data = block state id.
const BLOCK_BREAK_EVENT: i32 = 2001;

/// Vanilla `IronGolem.mobInteract`: heal 25, consuming only if health changes.
pub(crate) fn repair(
    state: &mut TickState,
    actor: crate::boundary::EntityId,
    target: crate::boundary::EntityId,
    hand: crate::boundary::Hand,
) -> bool {
    let Some(slot) = state.entities.slot_of(target) else {
        return false;
    };
    let body = state.entities.body(slot);
    let crate::entities::EntityKind::Mob { type_id } = body.kind else {
        return false;
    };
    if !body.alive || state.registry.entity_type_name(type_id) != Some("minecraft:iron_golem") {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let held_slot = crate::tick::held_slot_index(player, hand);
    let held = &player.inventory.slots[held_slot];
    if player.spectator()
        || held.is_empty()
        || state
            .registry
            .items
            .name_of(held.item_id)
            .is_none_or(|name| name.as_str() != "minecraft:iron_ingot")
        || crate::tick::held_debit_below_floor(state, actor, hand, 1)
    {
        return false;
    }
    let Some(blueprint) = state.registry.blueprint(type_id) else {
        return false;
    };
    let row = state.entities.row_mut(slot);
    if row.health <= 0.0 || row.health >= blueprint.params.max_health {
        return false;
    }
    row.health = (row.health + 25.0).min(blueprint.params.max_health);
    std::sync::Arc::make_mut(&mut row.appearance).health = row.health;
    state.appearance_changed.push(target);
    let player = state.players.get_mut(&actor).expect("interacting player");
    if !player.creative() {
        mc_data::inventory_semantics_26_1_2::decrement_stack(
            &mut player.inventory.slots[held_slot],
        );
    }
    let connection = player.connection;
    let contents = player.inventory_contents();
    state.push_player(connection, contents);
    true
}

struct Golem {
    entity: &'static str,
    /// Head plus body blocks, all consumed.
    cells: Vec<BlockPos>,
    /// Lowest body block: the golem stands in its cell.
    base: BlockPos,
    player_created: bool,
}

fn path(state: &TickState, pos: BlockPos) -> Option<&str> {
    let id = state.world.block_at(pos.x, pos.y, pos.z)?;
    Some(
        state
            .registry
            .blocks
            .by_id(BlockStateId(id))?
            .block
            .id
            .path(),
    )
}

fn is(state: &TickState, pos: BlockPos, expected: &str) -> bool {
    path(state, pos) == Some(expected)
}

fn is_air(state: &TickState, pos: BlockPos) -> bool {
    matches!(path(state, pos), Some("air" | "cave_air" | "void_air"))
}

fn below(pos: BlockPos, dy: i32) -> BlockPos {
    BlockPos {
        y: pos.y - dy,
        ..pos
    }
}

/// Snow: pumpkin over two snow blocks. Iron: pumpkin over a T of four iron
/// blocks (arms along X or Z) with air in the four cells around the T's
/// shoulders and stem (`"~^~", "###", "~#~"`).
fn pattern(state: &TickState, head: BlockPos) -> Option<Golem> {
    if is(state, below(head, 1), "snow_block") && is(state, below(head, 2), "snow_block") {
        return Some(Golem {
            entity: "minecraft:snow_golem",
            cells: vec![head, below(head, 1), below(head, 2)],
            base: below(head, 2),
            player_created: false,
        });
    }
    for (dx, dz) in [(1, 0), (0, 1)] {
        let side = |sign: i32, dy: i32| BlockPos {
            x: head.x + dx * sign,
            y: head.y - dy,
            z: head.z + dz * sign,
        };
        let shoulders = [side(-1, 1), side(0, 1), side(1, 1)];
        let stem = side(0, 2);
        let air = [side(-1, 0), side(1, 0), side(-1, 2), side(1, 2)];
        if shoulders
            .iter()
            .chain([&stem])
            .all(|&pos| is(state, pos, "iron_block"))
            && air.iter().all(|&pos| is_air(state, pos))
        {
            let mut cells = vec![head, stem];
            cells.extend(shoulders);
            return Some(Golem {
                entity: "minecraft:iron_golem",
                cells,
                base: stem,
                player_created: true,
            });
        }
    }
    None
}

fn break_event(state: &mut TickState, pos: BlockPos, block: u32) {
    let at = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) + 0.5,
    ];
    let viewers: Vec<_> = state
        .players
        .iter()
        .filter_map(|(id, player)| {
            let slot = state.entities.slot_of(*id)?;
            let near = state.entities.pos(slot);
            let distance_sq: f64 = (0..3).map(|i| (near[i] - at[i]).powi(2)).sum();
            (distance_sq <= 64.0 * 64.0).then_some(player.connection)
        })
        .collect();
    for connection in viewers {
        state.push_player(
            connection,
            PlayerChange::ProjectileLevelEvent {
                position: pos,
                event: BLOCK_BREAK_EVENT,
                data: block as i32,
            },
        );
    }
}

/// Pumpkin placement consumes a completed pattern and spawns its golem.
pub(crate) fn after_blocks(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>) {
    let heads: Vec<BlockPos> = writes
        .iter()
        .filter(|&&(pos, block)| {
            state.world.block_at(pos.x, pos.y, pos.z) == Some(block)
                && matches!(path(state, pos), Some("carved_pumpkin" | "jack_o_lantern"))
        })
        .map(|&(pos, _)| pos)
        .collect();
    for head in heads {
        let Some(golem) = pattern(state, head) else {
            continue;
        };
        let Some(type_id) = state
            .registry
            .entity_types
            .id_of(&Identifier::parse(golem.entity).expect("vanilla entity id"))
        else {
            continue;
        };
        let health = state
            .registry
            .blueprint(type_id)
            .map_or(20.0, |blueprint| blueprint.params.max_health);
        for &pos in &golem.cells {
            if let Some(block) = state.world.block_at(pos.x, pos.y, pos.z) {
                break_event(state, pos, block);
            }
            crate::redstone::set(state, writes, pos, state.registry.states.air);
        }
        let id = state.spawn_mob(
            type_id,
            [
                f64::from(golem.base.x) + 0.5,
                f64::from(golem.base.y) + 0.05,
                f64::from(golem.base.z) + 0.5,
            ],
            health,
        );
        if golem.player_created {
            let slot = state.entities.slot_of(id).expect("spawned golem");
            state
                .entities
                .row_mut(slot)
                .set_special_mob(Retained::PlayerIronGolem);
        }
        state.hostile.spawned.push(id);
        crate::special_mobs::summoned_nearby(state, id);
    }
}

#[cfg(test)]
#[path = "golems_tests.rs"]
mod tests;
