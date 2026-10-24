//! Native tick-owner redstone. Delayed components retain their queue through WAL
//! and checkpoints; block-state changes use the ordinary published block writes.
mod blocks;
pub(crate) mod containers;
pub(crate) mod custody;
#[cfg(test)]
mod dirty_signals_tests;
mod dispenser_blocks;
mod dispenser_equipment;
mod entity_query;
mod piston;
mod piston_motion;
mod planning;
mod rails;
mod signal;
#[cfg(test)]
#[path = "redstone_tests.rs"]
mod tests;

use crate::boundary::{ChunkPos, EntityId};
use crate::tick::TickState;
use blocks::offset;
use blocks::{NEIGHBOURS, facing_delta};
use mc_world::{BlockPos, BlockRegistry, BlockState, BlockStateId, ScheduledBlockTick};
use planning::BlockPlanningRead;
use std::collections::{BTreeMap, HashSet};

#[derive(Default)]
pub(crate) struct RedstoneState {
    loaded: HashSet<ChunkPos>,
    signal_residents: HashSet<ChunkPos>,
    /// World residency stamp of the last completed hydration pass.
    hydrated: Option<(u64, usize)>,
    signals: BTreeMap<(i32, i32, i32), u16>,
    wake: Vec<BlockPos>,
    dirty_signals: Vec<(i32, i32, i32)>,
    #[cfg(test)]
    signal_recomputes: usize,
    settled_wires: HashSet<BlockPos>,
    entity_query: entity_query::EntityQuery,
    event_viewers: Vec<crate::boundary::ConnectionId>,
    piston_entities: Vec<crate::entities::EntitySlot>,
}

struct WirePass {
    remaining: usize,
    settled: HashSet<BlockPos>,
}

fn chest_viewers(state: &TickState, pos: BlockPos) -> u8 {
    if at(state, pos).is_none_or(|block| block.block.id.path() != "trapped_chest") {
        return 0;
    }
    let cells = crate::chests::storage_cells(&state.registry.blocks, pos, &|cell: BlockPos| {
        state.world.block_at(cell.x, cell.y, cell.z)
    });
    state
        .open_containers
        .values()
        .filter(|cell| cells.iter().any(|pos| **cell == (pos.x, pos.y, pos.z)))
        .count()
        .min(15) as u8
}
pub(super) fn prop<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    state
        .properties
        .iter()
        .find(|(name, _)| name == key)
        .map(|(_, value)| value.as_str())
}
pub(super) fn sibling(
    blocks: &BlockRegistry,
    state: &BlockState,
    key: &str,
    value: &str,
) -> Option<BlockStateId> {
    if prop(state, key) == Some(value) {
        return Some(state.id);
    }
    let mut props = state.properties.clone();
    let (_, entry) = props.iter_mut().find(|(name, _)| name == key)?;
    *entry = value.into();
    blocks.by_name_and_props(&state.block.id, &props)
}
pub(super) fn reactive(path: &str) -> bool {
    matches!(
        path,
        "redstone_wire"
            | "redstone_torch"
            | "redstone_wall_torch"
            | "repeater"
            | "comparator"
            | "observer"
            | "piston"
            | "sticky_piston"
            | "redstone_lamp"
            | "daylight_detector"
            | "hopper"
            | "dispenser"
            | "dropper"
            | "crafter"
            | "powered_rail"
            | "detector_rail"
            | "activator_rail"
            | "rail"
            | "note_block"
            | "jukebox"
            | "lectern"
            | "sculk_sensor"
            | "calibrated_sculk_sensor"
    ) || path.ends_with("_door")
        || path.ends_with("_trapdoor")
        || path.ends_with("_fence_gate")
        || path.ends_with("_button")
        || path.ends_with("_pressure_plate")
        || path.ends_with("lightning_rod")
        || crate::shelf::is_shelf(path)
}
fn at(state: &TickState, pos: BlockPos) -> Option<&BlockState> {
    state
        .registry
        .blocks
        .by_id(BlockStateId(state.world.block_at(pos.x, pos.y, pos.z)?))
}
pub(crate) fn conducts(block: &BlockState) -> bool {
    signal::conductor(block)
}
pub(crate) fn output(state: &TickState, source: BlockPos, target: BlockPos) -> u8 {
    signal::received(state, source, target, true)
}
fn weak_output(state: &TickState, source: BlockPos, target: BlockPos) -> u8 {
    let Some(block) = at(state, source) else {
        return 0;
    };
    let path = block.block.id.path();
    match path {
        "redstone_block" => 15,
        "lectern" => {
            if prop(block, "powered") == Some("true") {
                15
            } else {
                0
            }
        }
        "jukebox" => {
            if crate::jukebox::playing(state, source) {
                15
            } else {
                0
            }
        }
        "sculk_sensor" | "calibrated_sculk_sensor" => crate::sculk::signal(state, source, target),
        "redstone_wire" => signal::dust(block, source, target),
        "daylight_detector" => prop(block, "power")
            .and_then(|p| p.parse().ok())
            .unwrap_or(0),
        "redstone_torch" | "redstone_wall_torch" => signal::torch(block, source, target),
        "repeater" | "observer" | "comparator" => {
            // Vanilla diodes face their input; observers face their watched cell.
            let Some(d) = prop(block, "facing").and_then(facing_delta) else {
                return 0;
            };
            if offset(source, (-d.0, -d.1, -d.2)) != Some(target) {
                return 0;
            }
            if path == "comparator" {
                comparator_output(state, source)
            } else if prop(block, "powered") == Some("true") {
                15
            } else {
                0
            }
        }
        "trapped_chest" => chest_viewers(state, source),
        _ if matches!(path, "lever" | "detector_rail") || path.ends_with("lightning_rod") => {
            if prop(block, "powered") == Some("true") {
                15
            } else {
                0
            }
        }
        _ if path.ends_with("_button") || path.ends_with("_pressure_plate") => prop(block, "power")
            .and_then(|p| p.parse().ok())
            .unwrap_or_else(|| {
                if prop(block, "powered") == Some("true") {
                    15
                } else {
                    0
                }
            }),
        _ => 0,
    }
}
pub(crate) fn power_at(state: &TickState, pos: BlockPos) -> u8 {
    NEIGHBOURS
        .into_iter()
        .filter_map(|d| offset(pos, d))
        .map(|source| output(state, source, pos))
        .max()
        .unwrap_or(0)
}
/// Vanilla quasi-connectivity: a component also reacts to power at the block above it
/// (`hasNeighborSignal(pos.above())`, i.e. the above cell's neighbours except the one below),
/// but only when a neighbour update reaches the component itself.
pub(crate) fn quasi_connected(state: &TickState, pos: BlockPos) -> bool {
    let Some(above) = offset(pos, (0, 1, 0)) else {
        return false;
    };
    NEIGHBOURS
        .into_iter()
        .filter(|d| *d != (0, -1, 0))
        .filter_map(|d| offset(above, d))
        .any(|source| output(state, source, above) > 0)
}
/// Vanilla `PistonBaseBlock.getNeighborSignal`: power from every side except the piston's own
/// face, plus quasi-connectivity through the block above.
pub(crate) fn piston_powered(state: &TickState, pos: BlockPos, facing: &str) -> bool {
    let face = facing_delta(facing);
    NEIGHBOURS
        .into_iter()
        .filter(|d| Some(*d) != face)
        .filter_map(|d| offset(pos, d))
        .any(|source| output(state, source, pos) > 0)
        || quasi_connected(state, pos)
}
struct Read<'a>(&'a TickState);
impl BlockPlanningRead for Read<'_> {
    fn get_cached_block(&self, pos: BlockPos) -> Option<BlockStateId> {
        self.0.world.block_at(pos.x, pos.y, pos.z).map(BlockStateId)
    }
    fn emission(&self, source: BlockPos, target: BlockPos) -> u8 {
        signal::received(self.0, source, target, false)
    }
    fn protection_allows(&self, pos: BlockPos) -> bool {
        // Pistons have no player identity: use the same environmental zone
        // admission as fire/explosions, including public-use protected claims.
        !crate::plugin_player::protects_cell(self.0, pos)
    }
}
pub(crate) fn schedule_cell(state: &mut TickState, pos: BlockPos, delay: u64, priority: i32) {
    let Some(block) = at(state, pos) else {
        return;
    };
    if !reactive(block.block.id.path()) {
        return;
    }
    let id = block.block.id.clone();
    if state.block_ticks.contains(pos, &id, priority) {
        return;
    }
    state.block_ticks.schedule(ScheduledBlockTick::new(
        pos,
        id,
        crate::command_context::execution_tick(state).saturating_add(delay),
        priority,
    ));
}
pub(crate) fn set(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    pos: BlockPos,
    new: u32,
) {
    if state.world.block_at(pos.x, pos.y, pos.z) == Some(new) {
        return;
    }
    state.world.set_block(pos.x, pos.y, pos.z, new);
    writes.retain(|(p, _)| *p != pos);
    writes.push((pos, new));
}
fn change(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    pos: BlockPos,
    key: &str,
    value: &str,
) {
    let Some(new) =
        at(state, pos).and_then(|block| sibling(&state.registry.blocks, block, key, value))
    else {
        return;
    };
    set(state, writes, pos, new.0);
}
pub(crate) fn neighbours(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    for (pos, _) in writes {
        rails::wake_diagonals(state, *pos);
        for d in std::iter::once((0, 0, 0)).chain(NEIGHBOURS) {
            let Some(cell) = offset(*pos, d) else {
                continue;
            };
            crate::fire::changed(state, cell);
            crate::tnt::changed(state, cell);
            crate::command_blocks::changed(state, cell);
            crate::enchantment_locations::changed(state, cell);
            if at(state, cell).is_some_and(conducts) {
                for direction in NEIGHBOURS {
                    let Some(consumer) = offset(cell, direction) else {
                        continue;
                    };
                    crate::tnt::changed(state, consumer);
                    if at(state, consumer).is_some_and(|block| {
                        let path = block.block.id.path();
                        reactive(path)
                            && path != "observer"
                            && !path.ends_with("_button")
                            && !path.ends_with("lightning_rod")
                    }) {
                        schedule_cell(state, consumer, 1, 0);
                    }
                }
            }
            let Some(block) = at(state, cell) else {
                continue;
            };
            let path = block.block.id.path();
            if !reactive(path) {
                continue;
            }
            if path == "observer" {
                if prop(block, "powered") == Some("true") {
                    continue;
                }
                let watched = prop(block, "facing")
                    .and_then(facing_delta)
                    .and_then(|d| offset(cell, d));
                if watched != Some(*pos) {
                    continue;
                }
                schedule_cell(state, cell, 2, 2);
            } else if path.ends_with("_button") {
                if cell == *pos && prop(block, "powered") == Some("true") {
                    let delay = if matches!(path, "stone_button" | "polished_blackstone_button") {
                        20
                    } else {
                        30
                    };
                    schedule_cell(state, cell, delay, 3);
                }
            } else {
                schedule_cell(state, cell, 1, 0);
            }
        }
    }
}
pub(crate) fn wake(state: &mut TickState, pos: BlockPos) {
    state.redstone.wake.push(pos);
}

/// Called after all ordinary world writes, before folding the overlay.
pub(crate) fn run(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    hydrate(state);
    neighbours(state, writes);
    // Inventory and lid changes wake comparator and trapped-chest neighbours.
    let mut wake = std::mem::take(&mut state.redstone.wake);
    refresh_signals(state, writes, &mut wake);
    #[cfg(test)]
    dirty_signals_tests::assert_signal_cache_matches_full_scan(state);
    for pos in wake.drain(..) {
        neighbours(
            state,
            &[(pos, state.world.block_at(pos.x, pos.y, pos.z).unwrap_or(0))],
        );
    }
    state.redstone.wake = wake;
    let due = state
        .block_ticks
        .drain_loaded(state.tick, 1024, &state.world);
    let registry = std::sync::Arc::clone(&state.registry);
    let mut wires = WirePass {
        remaining: 1024,
        settled: std::mem::take(&mut state.redstone.settled_wires),
    };
    wires.settled.clear();
    let mut entity_query = std::mem::take(&mut state.redstone.entity_query);
    if due.iter().any(|tick| {
        at(state, tick.pos).is_some_and(|block| {
            let path = block.block.id.path();
            path.ends_with("_pressure_plate")
                || path == "detector_rail"
                || (path.ends_with("_button")
                    && !matches!(path, "stone_button" | "polished_blackstone_button"))
        })
    }) {
        entity_query.rebuild(state);
    }
    for tick in due {
        if at(state, tick.pos).is_some_and(|b| b.block.id == tick.block) {
            tick_cell(
                state,
                writes,
                spawned,
                tick,
                &registry,
                &mut wires,
                &entity_query,
            );
        }
    }
    state.redstone.settled_wires = wires.settled;
    state.redstone.entity_query = entity_query;
    neighbours(state, writes);
    containers::run(state, writes, spawned);
}
/// Detect signal changes only at custody/viewer mutations and world transitions.
/// Buffers retain capacity; the sorted positions preserve the former map order.
fn refresh_signals(state: &mut TickState, writes: &[(BlockPos, u32)], wake: &mut Vec<BlockPos>) {
    let mut dirty = std::mem::take(&mut state.redstone.dirty_signals);
    state.block_entities.drain_dirty_into(&mut dirty);
    state.open_containers.drain_dirty_into(&mut dirty);
    // Shape changes/removal can split a pair: include potential previous halves,
    // even though they no longer resolve as a pair in the post-write world.
    for &(pos, _) in writes {
        dirty.push((pos.x, pos.y, pos.z));
        for d in [(1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)] {
            if let Some(cell) = offset(pos, d) {
                dirty.push((cell.x, cell.y, cell.z));
            }
        }
    }
    // Contents or viewers of either half affect both halves' comparator/lid
    // output, but unrelated nearby single containers remain asleep.
    for index in 0..dirty.len() {
        let (x, y, z) = dirty[index];
        let pos = BlockPos { x, y, z };
        if let Some((_, partner)) =
            crate::chests::storage_partner(&state.registry.blocks, pos, &|cell| {
                state.world.block_at(cell.x, cell.y, cell.z)
            })
        {
            dirty.push((partner.x, partner.y, partner.z));
        }
    }
    dirty.sort_unstable();
    dirty.dedup();
    #[cfg(test)]
    {
        state.redstone.signal_recomputes = 0;
    }
    for &(x, y, z) in &dirty {
        let pos = BlockPos { x, y, z };
        if state.block_entities.contains_key(&(x, y, z)) {
            #[cfg(test)]
            {
                state.redstone.signal_recomputes += 1;
            }
            let level = u16::from(containers::signal(state, pos)) << 8
                | u16::from(chest_viewers(state, pos));
            if state.redstone.signals.insert((x, y, z), level) != Some(level) {
                wake.push(pos);
            }
        } else if state.redstone.signals.remove(&(x, y, z)).is_some() {
            wake.push(pos);
        }
    }
    dirty.clear();
    state.redstone.dirty_signals = dirty;
}

fn hydrate(state: &mut TickState) {
    // `active_revision` advances on every resident insert/remove, so an
    // unchanged stamp means every resident chunk is already in
    // `signal_residents` and `loaded`. Chunk content (block entities, scheduled
    // ticks) can still change in place, so those hydrations run every pass.
    let stamp = (state.world.active_revision(), state.world.resident_count());
    let settled = state.redstone.hydrated == Some(stamp);
    if !settled {
        state.redstone.signal_residents.retain(|cp| {
            if state.world.has_chunk(cp.x, cp.z) {
                true
            } else {
                state.block_entities.mark_chunk_dirty(*cp);
                false
            }
        });
    }
    let mut resident = state.world.resident_positions();
    resident.sort_unstable_by_key(|pos| (pos.x, pos.z));
    for cp in resident {
        let Some(chunk) = state.world.chunk(cp) else {
            continue;
        };
        state.block_ticks.hydrate_chunk(chunk, state.tick);
        containers::hydrate_chunk(
            &mut state.block_entities,
            &mut state.redstone_containers,
            &state.registry,
            chunk,
            state.tick,
        );
        if settled {
            continue;
        }
        if state.redstone.signal_residents.insert(cp) {
            state.block_entities.mark_chunk_dirty(cp);
        }
        if !state.redstone.loaded.insert(cp) {
            continue;
        }
        // One initial scan at residency acquisition; steady-state is event driven.
        let mut cells = Vec::new();
        for (index, section) in chunk.sections.iter().enumerate() {
            if section.is_empty() {
                continue;
            }
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        let id = section.get(x, y, z);
                        if state.registry.blocks.by_id(id).is_some_and(|b| {
                            reactive(b.block.id.path())
                                || b.block.id.path() == "tnt"
                                || crate::command_blocks::is_block(b.block.id.path())
                        }) {
                            cells.push(BlockPos {
                                x: cp.x * 16 + i32::from(x),
                                y: chunk.geometry().min_y() + index as i32 * 16 + i32::from(y),
                                z: cp.z * 16 + i32::from(z),
                            });
                        }
                    }
                }
            }
        }
        for cell in cells {
            crate::tnt::changed(state, cell);
            crate::command_blocks::changed(state, cell);
            containers::register(state, cell);
            schedule_cell(state, cell, 1, 0);
        }
    }
    state.redstone.hydrated = Some(stamp);
}
fn tick_cell(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
    tick: ScheduledBlockTick,
    registry: &crate::registry::CoreRegistry,
    wires: &mut WirePass,
    entity_query: &entity_query::EntityQuery,
) {
    let pos = tick.pos;
    rails::reshape(state, writes, pos);
    let Some(id) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return;
    };
    let Some(block) = registry.blocks.by_id(BlockStateId(id)) else {
        return;
    };
    let path = block.block.id.path();
    match path {
        _ if crate::falling::gravity(&registry.blocks, id) => {
            crate::falling::scheduled(state, pos, writes, spawned);
        }
        _ if path.ends_with("lightning_rod") => {
            if tick.priority == 4 {
                change(state, writes, pos, "powered", "false");
            }
        }
        "command_block" | "repeating_command_block" => state.command_blocks_due.push(pos),
        "redstone_wire" => {
            if wires.settled.contains(&pos) {
                return;
            }
            if wires.remaining == 0 {
                schedule_cell(state, pos, 1, 0);
                return;
            }
            let read = Read(state);
            let mut plan = planning::Settle::new(&state.registry.blocks, &read);
            let (cells, frontier) = plan.dust_component(pos, wires.remaining, &wires.settled);
            wires.remaining -= cells.len();
            wires.settled.extend(cells);
            let edits = plan.into_edits();
            let changed = !edits.is_empty();
            for edit in edits {
                set(state, writes, edit.pos, edit.new_state.0);
            }
            if changed {
                for cell in frontier {
                    schedule_cell(state, cell, 1, 0);
                }
            }
        }
        "repeater" => {
            let Some(d) = prop(block, "facing").and_then(facing_delta) else {
                return;
            };
            let sides = if d.0 == 0 {
                [(-1, 0, 0), (1, 0, 0)]
            } else {
                [(0, 0, -1), (0, 0, 1)]
            };
            let locked = sides
                .into_iter()
                .filter_map(|d| offset(pos, d))
                .any(|side| {
                    at(state, side)
                        .is_some_and(|b| matches!(b.block.id.path(), "repeater" | "comparator"))
                        && output(state, side, pos) > 0
                });
            let powered = prop(block, "powered") == Some("true");
            let input = offset(pos, d).map_or(0, |back| signal::diode_input(state, back, pos)) > 0;
            let delay = prop(block, "delay")
                .and_then(|p| p.parse::<u64>().ok())
                .unwrap_or(1)
                * 2;
            change(
                state,
                writes,
                pos,
                "locked",
                if locked { "true" } else { "false" },
            );
            if !locked {
                if tick.priority == 1 && (!powered || !input) {
                    change(
                        state,
                        writes,
                        pos,
                        "powered",
                        if powered { "false" } else { "true" },
                    );
                    if !powered && !input {
                        schedule_cell(state, pos, delay, 1);
                    }
                } else if tick.priority != 1 && input != powered {
                    schedule_cell(state, pos, delay, 1);
                }
            }
        }
        "redstone_torch" | "redstone_wall_torch" => {
            let support = if let Some(d) = prop(block, "facing").and_then(facing_delta) {
                offset(pos, (-d.0, -d.1, -d.2))
            } else {
                offset(pos, (0, -1, 0))
            };
            let lit = support.is_none_or(|p| {
                NEIGHBOURS
                    .into_iter()
                    .filter_map(|d| offset(p, d))
                    .filter(|p| *p != pos)
                    .all(|source| output(state, source, p) == 0)
            });
            if (prop(block, "lit") == Some("true")) != lit {
                if tick.priority == 1 {
                    change(
                        state,
                        writes,
                        pos,
                        "lit",
                        if lit { "true" } else { "false" },
                    );
                } else {
                    schedule_cell(state, pos, 2, 1);
                }
            }
        }
        "observer" => {
            if tick.priority == 2 {
                change(state, writes, pos, "powered", "true");
                schedule_cell(state, pos, 2, 4);
            } else if tick.priority == 4 {
                change(state, writes, pos, "powered", "false");
            }
        }
        "lectern" => {
            if tick.priority == 4 {
                change(state, writes, pos, "powered", "false");
            }
        }
        _ if path.ends_with("lightning_rod") => {
            if tick.priority == 4 {
                change(state, writes, pos, "powered", "false");
            }
        }
        "comparator" => comparator_tick(state, writes, pos, tick.priority),
        "daylight_detector" => {
            let level = signal::daylight(state, pos, prop(block, "inverted") == Some("true"));
            change(state, writes, pos, "power", &level.to_string());
            schedule_cell(state, pos, 20, 0);
        }
        "composter" => {
            crate::composter::mature(state, pos, writes);
        }
        "fire" | "soul_fire" => {
            crate::fire::scheduled_tick(state, pos, writes, spawned);
        }
        "frosted_ice" => {
            crate::enchantment_locations::scheduled(state, pos, writes);
        }
        _ if path.ends_with("_leaves") => {
            crate::leaves::scheduled(state, pos, writes);
        }
        "piston" | "sticky_piston" => {
            // The planner validates the entire move before publishing any cell.
            let powered = piston_powered(state, pos, prop(block, "facing").unwrap_or("north"));
            let read = Read(state);
            let mut plan = planning::Settle::new(&state.registry.blocks, &read);
            match piston::plan_move(&mut plan, pos, block, path == "sticky_piston", powered) {
                Ok(motion) => {
                    piston::apply_edits(state, writes, spawned, plan.into_edits(), motion);
                }
                Err(piston::Refusal::Unloaded(_)) => schedule_cell(state, pos, 1, 0),
                Err(_) => {}
            }
        }
        "crafter" => crate::crafter::scheduled(state, pos, tick.priority, writes, spawned),
        _ if crate::shelf::is_shelf(path) => crate::shelf::scheduled(state, pos, writes),
        "dispenser" | "dropper" => {
            let powered = power_at(state, pos) > 0 || quasi_connected(state, pos);
            let triggered = prop(block, "triggered") == Some("true");
            containers::register(state, pos);
            if tick.priority == 5 {
                containers::trigger(state, pos, writes, spawned);
            } else if powered && !triggered {
                change(state, writes, pos, "triggered", "true");
                schedule_cell(state, pos, 4, 5);
            } else if !powered {
                change(state, writes, pos, "triggered", "false");
            }
        }
        "hopper" => {
            containers::register(state, pos);
            let enabled = power_at(state, pos) == 0;
            change(
                state,
                writes,
                pos,
                "enabled",
                if enabled { "true" } else { "false" },
            );
        }
        "redstone_lamp" => {
            let powered = power_at(state, pos) > 0;
            if powered {
                change(state, writes, pos, "lit", "true");
            } else if tick.priority == 6 {
                change(state, writes, pos, "lit", "false");
            } else if prop(block, "lit") == Some("true") {
                schedule_cell(state, pos, 4, 6);
            }
        }
        _ if path.ends_with("_button") => {
            let wooden = !matches!(path, "stone_button" | "polished_blackstone_button");
            let arrow = wooden
                && entity_query.count(
                    [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)],
                    [
                        f64::from(pos.x) + 1.0,
                        f64::from(pos.y) + 1.0,
                        f64::from(pos.z) + 1.0,
                    ],
                    1,
                    |slot| {
                        let crate::entities::EntityKind::Mob { type_id } =
                            state.entities.body(slot).kind
                        else {
                            return false;
                        };
                        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                            .is_some_and(|entity| {
                                matches!(
                                    entity.name,
                                    "minecraft:arrow" | "minecraft:spectral_arrow"
                                )
                            })
                    },
                ) > 0;
            if arrow {
                change(state, writes, pos, "powered", "true");
                schedule_cell(state, pos, 30, 3);
            } else if tick.priority == 3 {
                change(state, writes, pos, "powered", "false");
            }
            if wooden && !arrow && (prop(block, "powered") != Some("true") || tick.priority == 3) {
                schedule_cell(state, pos, 10, 0);
            }
        }
        _ if path.ends_with("_pressure_plate") || path == "detector_rail" => {
            // An idle poll left over from an earlier wake must not shorten
            // the 20-tick pressed tail.
            if tick.priority == 7
                && (prop(block, "powered") == Some("true")
                    || prop(block, "power").is_some_and(|power| power != "0"))
            {
                return;
            }
            let limit = match path {
                "heavy_weighted_pressure_plate" => 150,
                "light_weighted_pressure_plate" => 15,
                _ => 1,
            };
            let count = entity_query.count(
                [
                    f64::from(pos.x) + 0.125,
                    f64::from(pos.y),
                    f64::from(pos.z) + 0.125,
                ],
                [
                    f64::from(pos.x) + 0.875,
                    f64::from(pos.y) + if path == "detector_rail" { 0.8 } else { 0.25 },
                    f64::from(pos.z) + 0.875,
                ],
                limit,
                |slot| {
                    let body = state.entities.body(slot);
                    if path == "detector_rail" {
                        let crate::entities::EntityKind::Mob { type_id } = body.kind else {
                            return false;
                        };
                        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                            .is_some_and(|entity| entity.name.ends_with("minecart"))
                    } else {
                        !matches!(
                            path,
                            "stone_pressure_plate" | "polished_blackstone_pressure_plate"
                        ) || matches!(
                            body.kind,
                            crate::entities::EntityKind::Player { .. }
                                | crate::entities::EntityKind::Mob { .. }
                        )
                    }
                },
            );
            let level = if path == "heavy_weighted_pressure_plate" {
                count.div_ceil(10).min(15)
            } else if path == "light_weighted_pressure_plate" {
                count.min(15)
            } else {
                usize::from(count > 0) * 15
            };
            if prop(block, "power").is_some() {
                change(state, writes, pos, "power", &level.to_string());
            } else {
                change(
                    state,
                    writes,
                    pos,
                    "powered",
                    if level > 0 { "true" } else { "false" },
                );
            }
            schedule_cell(
                state,
                pos,
                if level > 0 { 20 } else { 10 },
                if level > 0 { 0 } else { 7 },
            );
        }
        "powered_rail" | "activator_rail" => {
            let powered = rails::powered(state, pos);
            change(
                state,
                writes,
                pos,
                "powered",
                if powered { "true" } else { "false" },
            );
        }
        _ if path.ends_with("_door")
            || path.ends_with("_trapdoor")
            || path.ends_with("_fence_gate") =>
        {
            let mut powered = power_at(state, pos) > 0;
            let pair = if path.ends_with("_door") {
                prop(block, "half")
                    .and_then(|half| {
                        offset(
                            pos,
                            if half == "lower" {
                                (0, 1, 0)
                            } else {
                                (0, -1, 0)
                            },
                        )
                    })
                    .filter(|other| at(state, *other).is_some_and(|b| b.block.id.path() == path))
            } else {
                None
            };
            if let Some(other) = pair {
                powered |= power_at(state, other) > 0;
            }
            if prop(block, "powered") == Some(if powered { "false" } else { "true" }) {
                for cell in std::iter::once(pos).chain(pair) {
                    change(
                        state,
                        writes,
                        cell,
                        "powered",
                        if powered { "true" } else { "false" },
                    );
                    change(
                        state,
                        writes,
                        cell,
                        "open",
                        if powered { "true" } else { "false" },
                    );
                }
            }
        }
        "note_block" => {
            let powered = power_at(state, pos) > 0;
            let rising = powered && prop(block, "powered") != Some("true");
            let above_instrument = offset(pos, (0, 1, 0))
                .and_then(|cell| at(state, cell))
                .map(|block| mc_data::block_facts::note_instrument(&block.block.id));
            let instrument = above_instrument
                .filter(|instrument| works_above(instrument))
                .unwrap_or_else(|| {
                    let below = offset(pos, (0, -1, 0))
                        .and_then(|cell| at(state, cell))
                        .map_or("harp", |block| {
                            mc_data::block_facts::note_instrument(&block.block.id)
                        });
                    if works_above(below) { "harp" } else { below }
                });
            change(state, writes, pos, "instrument", instrument);
            change(
                state,
                writes,
                pos,
                "powered",
                if powered { "true" } else { "false" },
            );
            if rising {
                play_note(state, pos);
            }
        }
        _ => {}
    }
}

fn works_above(instrument: &str) -> bool {
    matches!(
        instrument,
        "zombie" | "skeleton" | "creeper" | "dragon" | "wither_skeleton" | "piglin" | "custom_head"
    )
}

/// Vanilla queues action/parameter 0/0. NoteBlock.triggerEvent reads the note
/// and instrument from the block state and performs both sound and particles.
fn play_note(state: &mut TickState, pos: BlockPos) {
    let Some(block) = at(state, pos) else {
        return;
    };
    let instrument = prop(block, "instrument").unwrap_or("harp");
    let above = offset(pos, (0, 1, 0)).and_then(|above| at(state, above));
    if !works_above(instrument)
        && above
            .is_none_or(|above| !matches!(above.block.id.path(), "air" | "cave_air" | "void_air"))
    {
        return;
    }
    let Ok(block_id) = u32::try_from(block.block.raw_id) else {
        return;
    };
    let block_state = block.id.0;
    let mut viewers = std::mem::take(&mut state.redstone.event_viewers);
    viewers.clear();
    viewers.extend(state.players.iter().filter_map(|(id, player)| {
        let slot = state.entities.slot_of(*id)?;
        let p = state.entities.pos(slot);
        let distance = (p[0] - f64::from(pos.x)).powi(2)
            + (p[1] - f64::from(pos.y)).powi(2)
            + (p[2] - f64::from(pos.z)).powi(2);
        (distance < 64.0 * 64.0).then_some(player.connection)
    }));
    for connection in viewers.drain(..) {
        state.push_player(
            connection,
            crate::boundary::PlayerChange::BlockEvent {
                position: pos,
                block: block_id,
                state: block_state,
                action: 0,
                parameter: 0,
            },
        );
    }
    state.redstone.event_viewers = viewers;
    crate::allay::note_played(state, pos);
    crate::vibration::emit(
        state,
        crate::vibration::centre(pos),
        crate::vibration::GameEvent::NoteBlockPlay,
        None,
    );
}

/// Queues the vanilla bell block action and positional sound for nearby players.
pub(crate) fn ring_bell(
    state: &mut TickState,
    position: BlockPos,
    block: u32,
    block_state: u32,
    parameter: u8,
) {
    let mut viewers = std::mem::take(&mut state.redstone.event_viewers);
    viewers.clear();
    viewers.extend(state.players.iter().filter_map(|(id, player)| {
        let slot = state.entities.slot_of(*id)?;
        let player_position = state.entities.pos(slot);
        let distance = (player_position[0] - f64::from(position.x)).powi(2)
            + (player_position[1] - f64::from(position.y)).powi(2)
            + (player_position[2] - f64::from(position.z)).powi(2);
        (distance < 64.0 * 64.0).then_some(player.connection)
    }));
    for connection in viewers.drain(..) {
        state.push_player(
            connection,
            crate::boundary::PlayerChange::BlockEvent {
                position,
                block,
                state: block_state,
                action: 1,
                parameter,
            },
        );
        state.push_player(
            connection,
            crate::boundary::PlayerChange::BellRung { position },
        );
    }
    state.redstone.event_viewers = viewers;
}
fn comparator_output(state: &TickState, pos: BlockPos) -> u8 {
    let Some(chunk) = state.world.chunk(ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    }) else {
        return 0;
    };
    let Some(bytes) = chunk.block_entities.get(&pos) else {
        return 0;
    };
    let Ok(mc_nbt::Tag::Compound(fields)) = mc_nbt::read_network(&mut bytes.as_slice()) else {
        return 0;
    };
    fields
        .iter()
        .find_map(|(name, tag)| {
            if name == "OutputSignal" {
                if let mc_nbt::Tag::Int(v) = tag {
                    Some((*v).clamp(0, 15) as u8)
                } else {
                    None
                }
            } else {
                None
            }
        })
        .unwrap_or(0)
}
fn comparator_tick(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    pos: BlockPos,
    priority: i32,
) {
    let Some(block) = at(state, pos) else {
        return;
    };
    let Some(d) = prop(block, "facing").and_then(facing_delta) else {
        return;
    };
    let mode = prop(block, "mode") == Some("subtract");
    let main = signal::comparator_input(state, pos, d);
    let sides = if d.0 == 0 {
        [(-1, 0, 0), (1, 0, 0)]
    } else {
        [(0, 0, -1), (0, 0, 1)]
    };
    let side = sides
        .into_iter()
        .filter_map(|d| offset(pos, d))
        .map(|p| signal::comparator_side(state, p, pos))
        .max()
        .unwrap_or(0);
    let level = if mode {
        main.saturating_sub(side)
    } else if main >= side {
        main
    } else {
        0
    };
    if level == comparator_output(state, pos)
        && (prop(block, "powered") == Some("true")) == (level > 0)
    {
        return;
    }
    if priority != 1 {
        schedule_cell(state, pos, 2, 1);
        return;
    }
    let fields = vec![
        (
            "id".into(),
            mc_nbt::Tag::String("minecraft:comparator".into()),
        ),
        ("x".into(), mc_nbt::Tag::Int(pos.x)),
        ("y".into(), mc_nbt::Tag::Int(pos.y)),
        ("z".into(), mc_nbt::Tag::Int(pos.z)),
        ("OutputSignal".into(), mc_nbt::Tag::Int(i32::from(level))),
    ];
    change(
        state,
        writes,
        pos,
        "powered",
        if level > 0 { "true" } else { "false" },
    );
    let mut bytes = Vec::new();
    if mc_nbt::write_network(&mut bytes, &mc_nbt::Tag::Compound(fields)).is_ok() {
        state
            .world
            .set_opaque_block_entity(pos, Some(bytes.clone()));
        state.opaque_changes.push((pos, Some(bytes)));
    }
    neighbours(
        state,
        &[(pos, state.world.block_at(pos.x, pos.y, pos.z).unwrap_or(0))],
    );
}
