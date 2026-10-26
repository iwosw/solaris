//! Vanilla egg scheduled clocks: turtle crack/hatch staging (night + sand),
//! sniffer thirds (moss boost), frogspawn hatch delay. Pinned 26.1.2
//! TurtleEggBlock/SnifferEggBlock/FrogspawnBlock javap in
//! .analysis/codex-logs/vanilla-a-gaps. Shared scheduler validates the
//! queued identifier before entering this hook; stale ticks cannot
//! resurrect a broken/replaced egg.
use mc_data::Identifier;
use mc_world::{BlockStateId, ScheduledBlockTick};

use crate::boundary::{BlockPos, EntityId};
use crate::tick::TickState;

/// Sniffer thirds of the unboosted 24000-tick clock (moss below halves it).
const SNIFFER_FULL: u64 = 24_000;
/// Frogspawn hatch window from bytecode constants (3600..12000).
const FROG_MIN: u64 = 3_600;
const FROG_SPAN: u64 = 8_400;

fn at(state: &TickState, pos: BlockPos) -> Option<u32> {
    state.world.block_at(pos.x, pos.y, pos.z)
}

fn prop(state: &TickState, id: u32, name: &str) -> Option<String> {
    state
        .registry
        .blocks
        .by_id(BlockStateId(id))?
        .properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

fn with_prop(state: &TickState, id: u32, name: &str, value: &str) -> Option<u32> {
    let current = state.registry.blocks.by_id(BlockStateId(id))?;
    let mut props = current.properties.clone();
    props.iter_mut().find(|(key, _)| key == name)?.1 = value.to_owned();
    state
        .registry
        .blocks
        .by_name_and_props(&current.block.id, &props)
        .map(|id| id.0)
}

fn schedule(state: &mut TickState, pos: BlockPos, id: &Identifier, delay: u64) {
    if state
        .block_ticks
        .contains(pos, id, 0)
    {
        return;
    }
    state.block_ticks.schedule(ScheduledBlockTick::new(
        pos,
        id.clone(),
        state.tick.saturating_add(delay),
        0,
    ));
}

/// Queue the egg's next clock edge after any placement-like write.
/// Turtle eggs ride the night random tick (no scheduled clock); sniffer
/// eggs and frogspawn schedule their hatch thirds/delay on placement.
pub(crate) fn changed(state: &mut TickState, pos: BlockPos) {
    let Some(id) = at(state, pos) else {
        return;
    };
    let Some(block) = state.registry.blocks.by_id(BlockStateId(id)) else {
        return;
    };
    let name = block.block.id.clone();
    match name.path() {
        "sniffer_egg" => {
            let boost = at(state, BlockPos { y: pos.y - 1, ..pos })
                .and_then(|id| state.registry.blocks.by_id(BlockStateId(id)))
                .is_some_and(|below| below.block.id.path() == "moss_block");
            let full = if boost { SNIFFER_FULL / 2 } else { SNIFFER_FULL };
            schedule(state, pos, &name, full / 3);
        }
        "frogspawn" => {
            let delay = FROG_MIN + state.tick % (FROG_SPAN + 1);
            schedule(state, pos, &name, delay);
        }
        _ => {}
    }
}

/// Scheduled clock edge for an egg block. Crack staging advances hatch
/// thirds; the final edge breaks the block and births babies/tadpoles.
/// Turtle eggs keep their night random-tick clock (no scheduled edge).
pub(crate) fn scheduled_tick(
    state: &mut TickState,
    pos: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let Some(id) = at(state, pos) else {
        return;
    };
    let Some(block) = state.registry.blocks.by_id(BlockStateId(id)) else {
        return;
    };
    let path = block.block.id.path().to_owned();
    let name = block.block.id.clone();
    match path.as_str() {
        "sniffer_egg" => {
            let hatch = prop(state, id, "hatch")
                .and_then(|v| v.parse::<u8>().ok())
                .unwrap_or(0);
            if hatch >= 2 {
                let air = state.registry.states.air;
                state.world.set_block(pos.x, pos.y, pos.z, air);
                writes.retain(|(written, _)| *written != pos);
                writes.push((pos, air));
                birth(state, "sniffer", pos, spawned);
                return;
            }
            if let Some(next) = with_prop(state, id, "hatch", &(hatch + 1).to_string()) {
                state.world.set_block(pos.x, pos.y, pos.z, next);
                writes.retain(|(written, _)| *written != pos);
                writes.push((pos, next));
            }
            let boost = at(state, BlockPos { y: pos.y - 1, ..pos })
                .and_then(|bid| state.registry.blocks.by_id(BlockStateId(bid)))
                .is_some_and(|below| below.block.id.path() == "moss_block");
            let full = if boost { SNIFFER_FULL / 2 } else { SNIFFER_FULL };
            schedule(state, pos, &name, full / 3);
        }
        "frogspawn" => {
            let below = BlockPos { y: pos.y - 1, ..pos };
            let water = at(state, below)
                .and_then(|bid| state.registry.blocks.by_id(BlockStateId(bid)))
                .is_some_and(|b| b.block.id.path() == "water");
            if !water {
                return;
            }
            let air = state.registry.states.air;
            state.world.set_block(pos.x, pos.y, pos.z, air);
            writes.retain(|(written, _)| *written != pos);
            writes.push((pos, air));
            let count = 2 + (state.tick % 4) as u8;
            for index in 0..count {
                birth_at(
                    state,
                    "tadpole",
                    [
                        f64::from(pos.x) + 0.3 + f64::from(index % 3) * 0.2,
                        f64::from(pos.y),
                        f64::from(pos.z) + 0.3 + f64::from(index / 3) * 0.2,
                    ],
                    spawned,
                );
            }
        }
        _ => {}
    }
}

fn birth(state: &mut TickState, name: &str, pos: BlockPos, spawned: &mut Vec<EntityId>) {
    birth_at(
        state,
        name,
        [f64::from(pos.x) + 0.5, f64::from(pos.y) + 1.0, f64::from(pos.z) + 0.5],
        spawned,
    );
}

fn birth_at(state: &mut TickState, name: &str, at: [f64; 3], spawned: &mut Vec<EntityId>) {
    let id = mc_data::Identifier::parse(format!("minecraft:{name}")).expect("egg birth id");
    let Some(type_id) = state.registry.entity_types.id_of(&id) else {
        return;
    };
    let health = state
        .registry
        .blueprint(type_id)
        .map_or(10.0, |blueprint| blueprint.params.max_health);
    let born = state.spawn_mob(type_id, at, health);
    crate::entity_life::make_child(state, born);
    spawned.push(born);
}
