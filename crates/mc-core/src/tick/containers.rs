//! Container menus, viewer refresh and furnace ticking.
//! Moved verbatim from `tick.rs`.

use super::*;

/// Container menu for a block; both halves share one double-chest menu.
pub(super) fn container_menu_for(
    state: &TickState,
    x: i32,
    y: i32,
    z: i32,
) -> Option<(crate::inventory::MenuKind, String, usize)> {
    let current = state.world.block_at(x, y, z)?;
    let menu = container_kind_for_state(state, current)?;
    if container_cells(state, (x, y, z)).len() == 2 {
        return Some((
            crate::inventory::MenuKind::Chest { rows: 6 },
            "Large Chest".to_owned(),
            54,
        ));
    }
    Some(menu)
}

/// Storage cells behind the container at `pos`, in menu order: one, or the
/// two halves of a double chest.
pub(super) fn container_cells(state: &TickState, pos: (i32, i32, i32)) -> Vec<(i32, i32, i32)> {
    let at = BlockPos {
        x: pos.0,
        y: pos.1,
        z: pos.2,
    };
    crate::chests::storage_cells(&state.registry.blocks, at, &|cell: BlockPos| {
        state.world.block_at(cell.x, cell.y, cell.z)
    })
    .into_iter()
    .map(|cell| (cell.x, cell.y, cell.z))
    .collect()
}

/// The contents of `cells` back to back (menu order).
pub(super) fn container_slots(
    state: &TickState,
    cells: &[(i32, i32, i32)],
) -> Vec<mc_data::ItemStack> {
    cells
        .iter()
        .filter_map(|cell| state.block_entities.get(cell))
        .flat_map(|record| record.slots.iter().cloned())
        .collect()
}

/// Resync the real current menu after refusing a native inventory transition.
/// A vehicle or block menu must never be imaged with placeholder empty cargo.
pub(crate) fn inventory_transition_refused(
    state: &TickState,
    entity: EntityId,
) -> crate::player::PlayerEffects {
    let Some(player) = state.players.get(&entity) else {
        return crate::player::PlayerEffects::default();
    };
    if let Some(slots) = crate::vehicles::container_slots_for_player(state, entity) {
        return player.inventory_transition_refused(slots);
    }
    if crate::ender_chest::is_open(state, entity) {
        return crate::ender_chest::contents(&player.opaque_player_fields, &state.registry.items)
            .map(|slots| player.inventory_transition_refused(&slots))
            .unwrap_or_default();
    }
    let cells = state
        .open_containers
        .get(&entity)
        .map(|open| container_cells(state, *open))
        .unwrap_or_default();
    player.inventory_transition_refused(&container_slots(state, &cells))
}

/// Publish the current authoritative menu image to every viewer after external
/// native custody mutation, with one fresh revision per affected menu.
pub(crate) fn refresh_container_viewers(state: &mut TickState, position: BlockPos) {
    let cells = container_cells(state, (position.x, position.y, position.z));
    let viewers: Vec<_> = state
        .open_containers
        .iter()
        .filter(|(_, open)| container_cells(state, **open) == cells)
        .filter_map(|(viewer, _)| {
            state
                .connections
                .entity_connection(*viewer)
                .map(|connection| (*viewer, connection))
        })
        .collect();
    if viewers.is_empty() {
        return;
    }
    let contents = container_slots(state, &cells);
    for (viewer, connection) in viewers {
        let Some(player) = state.players.get_mut(&viewer) else {
            continue;
        };
        if let Some(menu) = player.open.as_mut() {
            menu.state_id = (menu.state_id + 1) & 0x7fff;
        }
        for change in player.sync_container_view(&contents) {
            state.push_player(connection, change);
        }
    }
}

/// Write menu-order `slots` back over `cells`; a half whose contents
/// changed is marked dirty.
pub(super) fn store_container_slots(
    state: &mut TickState,
    cells: &[(i32, i32, i32)],
    slots: &[mc_data::ItemStack],
) {
    let mut rest = slots;
    for cell in cells {
        let Some(record) = state.block_entities.get_mut(cell) else {
            continue;
        };
        let (mine, tail) = rest.split_at(record.slots.len().min(rest.len()));
        rest = tail;
        if record.slots != mine {
            record.slots.clear();
            record.slots.extend_from_slice(mine);
            crate::brewing::slots_replaced(&mut state.brewing, *cell, &record.slots);
            let position = BlockPos {
                x: cell.0,
                y: cell.1,
                z: cell.2,
            };
            crate::redstone::containers::inventory_changed(
                &mut state.redstone_containers,
                position,
            );
            state.world.mark_block_entity_dirty(position);
        }
    }
}

/// Container menu for a block state id.
pub(super) fn container_kind_for_state(
    state: &TickState,
    state_id: u32,
) -> Option<(crate::inventory::MenuKind, String, usize)> {
    let block_state = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(state_id))?;
    let id = block_state.block.id.as_str();
    if let Some(menu) = stations::craft_menu_for_block(id) {
        return Some(menu);
    }
    if crate::copper::chest(block_state.block.id.path()) {
        return Some((
            crate::inventory::MenuKind::Chest { rows: 3 },
            "Copper Chest".into(),
            27,
        ));
    }
    if crate::shulker::is_block(block_state.block.id.path()) {
        return Some((
            crate::inventory::MenuKind::ShulkerBox,
            "Shulker Box".into(),
            27,
        ));
    }
    match id {
        "minecraft:chest" | "minecraft:trapped_chest" => Some((
            crate::inventory::MenuKind::Chest { rows: 3 },
            "Chest".to_owned(),
            27,
        )),
        "minecraft:hopper" => Some((crate::inventory::MenuKind::Hopper, "Hopper".into(), 5)),
        "minecraft:crafter" => Some((crate::inventory::MenuKind::Crafter, "Crafter".into(), 9)),
        "minecraft:brewing_stand" => Some((
            crate::inventory::MenuKind::BrewingStand,
            "Brewing Stand".into(),
            5,
        )),
        "minecraft:dispenser" | "minecraft:dropper" => Some((
            crate::inventory::MenuKind::Dispenser,
            if id == "minecraft:dropper" {
                "Dropper"
            } else {
                "Dispenser"
            }
            .into(),
            9,
        )),
        "minecraft:barrel" => Some((
            crate::inventory::MenuKind::Chest { rows: 3 },
            "Barrel".to_owned(),
            27,
        )),
        "minecraft:crafting_table" => Some((
            crate::inventory::MenuKind::CraftingTable,
            "Crafting Table".to_owned(),
            // Grid/result custody belongs to this player's Menu, not the block.
            0,
        )),
        "minecraft:beacon" => Some((crate::inventory::MenuKind::Beacon, "Beacon".into(), 0)),
        "minecraft:lectern" => Some((crate::inventory::MenuKind::Lectern, "Lectern".into(), 1)),
        "minecraft:enchanting_table" => Some((
            crate::inventory::MenuKind::EnchantingTable,
            "Enchant".into(),
            0,
        )),
        "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil" => {
            Some((crate::inventory::MenuKind::Anvil, "Repair & Name".into(), 0))
        }
        "minecraft:grindstone" => Some((
            crate::inventory::MenuKind::Grindstone,
            "Repair & Disenchant".into(),
            0,
        )),
        "minecraft:furnace" => Some((
            crate::inventory::MenuKind::Furnace(mc_data::recipes::CookingKind::Furnace),
            "Furnace".to_owned(),
            3,
        )),
        "minecraft:smoker" => Some((
            crate::inventory::MenuKind::Furnace(mc_data::recipes::CookingKind::Smoker),
            "Smoker".to_owned(),
            3,
        )),
        "minecraft:blast_furnace" => Some((
            crate::inventory::MenuKind::Furnace(mc_data::recipes::CookingKind::BlastFurnace),
            "Blast Furnace".to_owned(),
            3,
        )),
        _ => None,
    }
}

/// Tick every furnace block entity: cooking advances, slots mutate in
/// place. `lit` block property follows the burn state when resolvable.
pub(crate) fn tick_furnaces(
    state: &mut TickState,
    block_writes: &mut Vec<(BlockPos, u32)>,
    catch_up: Option<(ChunkPos, u64)>,
) {
    // Independent borrow of the registry (Arc): the storage pass below needs
    // `&mut state`.
    let registry = std::sync::Arc::clone(&state.registry);
    let rules = registry.item_rules();
    // Collect positions first: the tick mutates storage per entity.
    let positions: Vec<(i32, i32, i32)> = state.block_entities.keys().copied().collect();
    for pos in positions {
        let chunk = ChunkPos {
            x: pos.0.div_euclid(16),
            z: pos.2.div_euclid(16),
        };
        if catch_up.is_some_and(|(target, _)| target != chunk)
            || (catch_up.is_none() && state.world.simulation_debt(chunk) > 0)
        {
            continue;
        }
        if !state
            .world
            .is_block_active(pos.0.div_euclid(16), pos.2.div_euclid(16))
        {
            continue;
        }
        let Some(current) = state.world.block_at(pos.0, pos.1, pos.2) else {
            continue;
        };
        let kind = match furnace_kind_for_state(&registry, current) {
            Some(kind) => kind,
            None => continue,
        };
        if catch_up.is_some() {
            let expect = Footprint {
                block_expect: vec![(
                    BlockPos {
                        x: pos.0,
                        y: pos.1,
                        z: pos.2,
                    },
                    current,
                )],
                ..Default::default()
            };
            if !state.footprint_valid(&expect) {
                continue;
            }
        }
        let position = BlockPos {
            x: pos.0,
            y: pos.1,
            z: pos.2,
        };
        let candidate = if crate::plugin_inventories::has_reservations_for_block(state, position) {
            let Some(record) = state.block_entities.get(&pos) else {
                continue;
            };
            let mut slots = record.slots.clone();
            if slots.len() < 3 {
                slots.resize(3, mc_data::ItemStack::EMPTY);
            }
            let mut progress = record.furnace.clone().unwrap_or_default();
            let before = progress.properties();
            let outcome = match catch_up {
                Some((_, elapsed)) => progress.catch_up(&rules, kind, &mut slots, elapsed),
                None => progress.tick(&rules, kind, &mut slots),
            };
            let after = progress.properties();
            if crate::plugin_inventories::below_block_floors(state, position, &slots) {
                continue;
            }
            Some((slots, progress, before, outcome, after))
        } else {
            None
        };
        let record = if catch_up.is_some() {
            state.block_entities.get_mut_catch_up(&pos)
        } else {
            state.block_entities.get_mut(&pos)
        };
        let Some(record) = record else {
            continue;
        };
        let (before, outcome, after) = if let Some((slots, progress, before, outcome, after)) =
            candidate
        {
            record.slots = slots;
            record.furnace = Some(progress);
            (before, outcome, after)
        } else {
            if record.slots.len() < 3 {
                record.slots.resize(3, mc_data::ItemStack::EMPTY);
            }
            let progress = record.furnace.get_or_insert_with(Default::default);
            let before = progress.properties();
            let outcome = match catch_up {
                Some((_, elapsed)) => progress.catch_up(&rules, kind, &mut record.slots, elapsed),
                None => progress.tick(&rules, kind, &mut record.slots),
            };
            (before, outcome, progress.properties())
        };
        if !outcome.changed && before == after {
            continue;
        }
        for (viewer, open) in &state.open_containers {
            if *open != pos {
                continue;
            }
            let Some(player) = state.players.get_mut(viewer) else {
                continue;
            };
            let Some(menu) = player.open.as_mut() else {
                continue;
            };
            let connection = player.connection;
            let window = menu.window;
            let queue = state.outbox.entry(connection).or_default();
            if outcome.changed {
                menu.state_id = (menu.state_id + 1) & 0x7FFF;
                queue.extend(player.sync_container_view(&record.slots));
            }
            for (property, (old, value)) in before.into_iter().zip(after).enumerate() {
                if old != value {
                    queue.push(PlayerChange::MenuProperty {
                        window,
                        property: property as i16,
                        value: value as i16,
                    });
                }
            }
        }
        if outcome.changed {
            crate::redstone::containers::inventory_changed(
                &mut state.redstone_containers,
                position,
            );
        }
        if let Some(lit) = outcome.lit {
            set_lit_prop(state, pos, lit, block_writes, catch_up.is_some());
        }
    }
}

/// Cooking kind for a furnace-family block state.
pub(super) fn furnace_kind_for_state(
    registry: &CoreRegistry,
    state_id: u32,
) -> Option<mc_data::recipes::CookingKind> {
    let block_state = registry.blocks.by_id(mc_world::BlockStateId(state_id))?;
    match block_state.block.id.as_str() {
        "minecraft:furnace" => Some(mc_data::recipes::CookingKind::Furnace),
        "minecraft:smoker" => Some(mc_data::recipes::CookingKind::Smoker),
        "minecraft:blast_furnace" => Some(mc_data::recipes::CookingKind::BlastFurnace),
        _ => None,
    }
}

/// Set the `lit` property on the block at `pos` (no-op when unresolvable).
pub(super) fn set_lit_prop(
    state: &mut TickState,
    pos: (i32, i32, i32),
    lit: bool,
    block_writes: &mut Vec<(BlockPos, u32)>,
    catch_up: bool,
) {
    let Some(current) = state.world.block_at(pos.0, pos.1, pos.2) else {
        return;
    };
    let Some(block_state) = state.registry.blocks.by_id(mc_world::BlockStateId(current)) else {
        return;
    };
    let mut properties = block_state.properties.clone();
    let Some((_, value)) = properties.iter_mut().find(|(key, _)| key == "lit") else {
        return;
    };
    *value = if lit { "true" } else { "false" }.to_owned();
    if let Some(next) = state
        .registry
        .blocks
        .by_name_and_props(&block_state.block.id, &properties)
    {
        if catch_up {
            state.world.set_block_catch_up(pos.0, pos.1, pos.2, next.0);
        } else {
            state.world.set_block(pos.0, pos.1, pos.2, next.0);
        }
        block_writes.push((
            BlockPos {
                x: pos.0,
                y: pos.1,
                z: pos.2,
            },
            next.0,
        ));
    }
}
