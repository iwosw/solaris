//! Native vanilla block uses which do not open a screen.
use crate::boundary::{BlockPos, EntityId, Hand};
use crate::tick::TickState;
use mc_data::Identifier;
use mc_world::BlockStateId;

pub(crate) fn axe(state: &mut TickState, actor: EntityId, hand: Hand, pos: BlockPos) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let held = &player.inventory.slots[crate::tick::held_slot_index(player, hand)];
    if held.is_empty()
        || player.spectator()
        || !state
            .registry
            .items
            .name_of(held.item_id)
            .is_some_and(|id| id.namespace() == "minecraft" && id.path().ends_with("_axe"))
    {
        return false;
    }
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(BlockStateId(current)) else {
        return false;
    };
    let path = block.block.id.path();
    let target = if let Some(unwaxed) = path.strip_prefix("waxed_").filter(|p| p.contains("copper"))
    {
        format!("minecraft:{unwaxed}")
    } else if path.contains("copper") {
        if let Some(rest) = path.strip_prefix("oxidized_") {
            format!("minecraft:weathered_{rest}")
        } else if let Some(rest) = path.strip_prefix("weathered_") {
            format!("minecraft:exposed_{rest}")
        } else if let Some(rest) = path.strip_prefix("exposed_") {
            format!(
                "minecraft:{}",
                if rest == "copper" {
                    "copper_block"
                } else {
                    rest
                }
            )
        } else {
            return false;
        }
    } else if !path.starts_with("stripped_")
        && (path.ends_with("_log")
            || path.ends_with("_wood")
            || path.ends_with("_stem")
            || path.ends_with("_hyphae")
            || path == "bamboo_block")
    {
        format!("minecraft:stripped_{path}")
    } else {
        return false;
    };
    let Ok(target) = Identifier::parse(target) else {
        return false;
    };
    let Some(next) = state
        .registry
        .blocks
        .by_name_and_props(&target, &block.properties)
    else {
        return false;
    };
    let before = held.clone();
    let Ok(wear) = crate::copper::prepare_tool_wear(state, actor, hand) else {
        return true;
    };
    state.write_block(pos, next.0);
    crate::copper::apply_tool_wear(state, actor, wear);
    crate::advancements::accepted_block(
        state,
        actor,
        "minecraft:item_used_on_block",
        pos,
        Some(current),
        Some(before),
    );
    true
}

pub(crate) fn dragon_egg(state: &mut TickState, actor: EntityId, pos: BlockPos) -> bool {
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    if !state
        .registry
        .blocks
        .by_id(BlockStateId(current))
        .is_some_and(|b| b.block.id.path() == "dragon_egg")
    {
        return false;
    }
    if state.players.get(&actor).is_none_or(|p| p.spectator()) {
        return true;
    }
    // DragonEggBlock.teleport: 1000 attempts, differences of uniform [0,16)
    // horizontal and [0,8) vertical samples. Only resident, in-border air wins.
    let mut random = mc_data::loot::chest_26_1_2::ChestRng::new(
        state.tick
            ^ (actor.0 as u64)
            ^ (pos.x as u64).rotate_left(17)
            ^ (pos.z as u64).rotate_left(31),
    );
    for _ in 0..1000 {
        let mut sample = |bound| random.next_bounded(bound) as i32;
        let target = BlockPos {
            x: pos.x + sample(16) - sample(16),
            y: pos.y + sample(8) - sample(8),
            z: pos.z + sample(16) - sample(16),
        };
        let border = &state.border;
        if f64::from(target.x + 1) <= border.center_x - border.size / 2.0
            || f64::from(target.x) >= border.center_x + border.size / 2.0
            || f64::from(target.z + 1) <= border.center_z - border.size / 2.0
            || f64::from(target.z) >= border.center_z + border.size / 2.0
            || crate::spawn_protection::protected(state, actor, target)
        {
            continue;
        }
        if !state
            .world
            .block_at(target.x, target.y, target.z)
            .is_some_and(|id| id == state.registry.states.air)
            || !crate::plugin_player::allows_block(state, actor, target)
        {
            continue;
        }
        state.world.set_block(target.x, target.y, target.z, current);
        state
            .world
            .set_block(pos.x, pos.y, pos.z, state.registry.states.air);
        state
            .merge_block_writes
            .extend([(pos, state.registry.states.air), (target, current)]);
        return true;
    }
    true
}
