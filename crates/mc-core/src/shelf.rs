//! Wooden shelves: three displayed stacks and powered, bounded hotbar chains.
use crate::boundary::{BlockPos, EntityId, Hand, PlayerChange};
use crate::tick::{BlockEntityState, TickState};
use mc_data::ItemStack;
use mc_data::block_entity_kinds::BlockEntityKind;
use mc_nbt::Tag;
fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}
fn at(state: &TickState, pos: BlockPos) -> Option<&mc_world::BlockState> {
    state.registry.blocks.by_id(mc_world::BlockStateId(
        state.world.block_at(pos.x, pos.y, pos.z)?,
    ))
}
pub(crate) fn is_shelf(name: &str) -> bool {
    BlockEntityKind::for_block(name) == Some(BlockEntityKind::Shelf)
}
fn prop<'a>(block: &'a mc_world::BlockState, name: &str) -> Option<&'a str> {
    block
        .properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}
fn offset(pos: BlockPos, delta: (i32, i32, i32), n: i32) -> BlockPos {
    BlockPos {
        x: pos.x + delta.0 * n,
        y: pos.y + delta.1 * n,
        z: pos.z + delta.2 * n,
    }
}
fn left(facing: &str) -> (i32, i32, i32) {
    match facing {
        "north" => (1, 0, 0),
        "south" => (-1, 0, 0),
        "west" => (0, 0, -1),
        _ => (0, 0, 1),
    }
}
fn connected(part: &str, side: &str) -> bool {
    part == "center" || part == side
}
fn compatible(state: &TickState, pos: BlockPos, facing: &str) -> bool {
    at(state, pos).is_some_and(|b| {
        is_shelf(b.block.id.path())
            && prop(b, "powered") == Some("true")
            && prop(b, "facing") == Some(facing)
    })
}
fn chain(state: &TickState, pos: BlockPos) -> Vec<BlockPos> {
    let Some(block) = at(state, pos) else {
        return Vec::new();
    };
    let facing = prop(block, "facing").unwrap_or("north");
    if !compatible(state, pos, facing) {
        return Vec::new();
    }
    let delta = left(facing);
    let mut cells = vec![pos];
    for (sign, side) in [(1, "left"), (-1, "right")] {
        for n in 1..3 {
            let cell = offset(pos, delta, n * sign);
            if !compatible(state, cell, facing) {
                break;
            }
            let part = at(state, cell)
                .and_then(|b| prop(b, "side_chain"))
                .unwrap_or("unconnected");
            if connected(part, side) {
                if sign == 1 {
                    cells.insert(0, cell)
                } else {
                    cells.push(cell)
                }
            }
            if part != "center" {
                break;
            }
        }
    }
    // Vanilla skips the leading cells whose computed hotbar indexes are
    // negative when command-forced chain parts describe more than three.
    let excess = cells.len().saturating_sub(3);
    cells.drain(..excess);
    cells
}
fn set_property(
    state: &mut TickState,
    pos: BlockPos,
    name: &str,
    value: &str,
    writes: &mut Vec<(BlockPos, u32)>,
) {
    let Some(block) = at(state, pos) else { return };
    if prop(block, name) == Some(value) {
        return;
    }
    let mut properties = block.properties.clone();
    let Some((_, old)) = properties.iter_mut().find(|(key, _)| key == name) else {
        return;
    };
    *old = value.into();
    if let Some(next) = state
        .registry
        .blocks
        .by_name_and_props(&block.block.id, &properties)
    {
        crate::redstone::set(state, writes, pos, next.0);
    }
}
pub(crate) fn scheduled(state: &mut TickState, pos: BlockPos, writes: &mut Vec<(BlockPos, u32)>) {
    let Some(block) = at(state, pos) else { return };
    let old_powered = prop(block, "powered") == Some("true");
    let facing = prop(block, "facing").unwrap_or("north").to_owned();
    let part = prop(block, "side_chain")
        .unwrap_or("unconnected")
        .to_owned();
    let powered = crate::redstone::power_at(state, pos) > 0;
    set_property(
        state,
        pos,
        "powered",
        if powered { "true" } else { "false" },
        writes,
    );
    let delta = left(&facing);
    if !powered {
        set_property(state, pos, "side_chain", "unconnected", writes);
        for (sign, disconnect) in [(1, "right"), (-1, "left")] {
            let cell = offset(pos, delta, sign);
            if !compatible(state, cell, &facing) {
                continue;
            }
            let part = at(state, cell)
                .and_then(|b| prop(b, "side_chain"))
                .unwrap_or("unconnected");
            let next = match (disconnect, part) {
                ("right", "center") => "right",
                ("left", "center") => "left",
                ("right", "left") | ("left", "right") => "unconnected",
                _ => part,
            };
            let next = next.to_owned();
            set_property(state, cell, "side_chain", &next, writes);
        }
        return;
    }
    if old_powered && part != "unconnected" {
        return;
    }
    let mut group = vec![pos];
    // Vanilla connects to the left first, and never joins chains above three.
    for sign in [1, -1] {
        let neighbor = offset(pos, delta, sign);
        if !compatible(state, neighbor, &facing) {
            continue;
        }
        let neighbor_group = chain(state, neighbor);
        if neighbor_group.iter().any(|cell| group.contains(cell))
            || group.len() + neighbor_group.len() > 3
        {
            continue;
        }
        if sign == 1 {
            let mut joined = neighbor_group;
            joined.extend(group);
            group = joined;
        } else {
            group.extend(neighbor_group);
        }
    }
    let length = group.len();
    for (index, cell) in group.into_iter().enumerate() {
        let part = if length == 1 {
            "unconnected"
        } else if index == 0 {
            "left"
        } else if index == length - 1 {
            "right"
        } else {
            "center"
        };
        set_property(state, cell, "side_chain", part, writes);
    }
}
pub(crate) fn decode_chunk(
    inventories: &crate::redstone::custody::BlockEntities,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) -> Result<Vec<((i32, i32, i32), Vec<ItemStack>)>, String> {
    let mut decoded = Vec::new();
    for (pos, bytes) in &chunk.block_entities {
        if inventories.contains_key(&key(*pos))
            || chunk
                .get_block(
                    pos.x.rem_euclid(16) as u8,
                    pos.y,
                    pos.z.rem_euclid(16) as u8,
                )
                .and_then(|id| registry.blocks.by_id(id))
                .is_none_or(|b| {
                    !is_shelf(b.block.id.path())
                        && !crate::chiseled_shelf::is_chiseled(b.block.id.path())
                })
        {
            continue;
        }
        let fail = |error: &dyn std::fmt::Display| format!("shelf {pos:?}: {error}");
        let Tag::Compound(fields) =
            mc_nbt::read_network(&mut bytes.as_slice()).map_err(|e| fail(&e))?
        else {
            return Err(fail(&"expected compound"));
        };
        let (_, slots) = mc_data::storage_blocks::decode_shelf(&fields, &registry.items)
            .map_err(|e| fail(&e))?;
        decoded.push((key(*pos), slots));
    }
    Ok(decoded)
}
fn register(state: &mut TickState, pos: BlockPos) {
    state
        .block_entities
        .entry(key(pos))
        .or_insert_with(|| BlockEntityState {
            slots: vec![ItemStack::EMPTY; 3],
            furnace: None,
        });
}
pub(crate) fn intercepts_placement(state: &TickState, hand: Hand, pos: BlockPos, face: u8) -> bool {
    let Some(block) = at(state, pos) else {
        return false;
    };
    if !is_shelf(block.block.id.path()) || hand != Hand::Main {
        return false;
    }
    let wanted = match prop(block, "facing").unwrap_or("north") {
        "north" => 2,
        "south" => 3,
        "west" => 4,
        _ => 5,
    };
    face == wanted
        && (prop(block, "powered") != Some("true") || (1..=3).contains(&chain(state, pos).len()))
}

pub(crate) fn use_on(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    pos: BlockPos,
    face: u8,
    cursor: [f32; 3],
) -> bool {
    let Some(block) = at(state, pos) else {
        return false;
    };
    if crate::chiseled_shelf::is_chiseled(block.block.id.path()) {
        return crate::chiseled_shelf::use_on(state, actor, hand, pos, face, cursor);
    }
    if !is_shelf(block.block.id.path()) || hand != Hand::Main {
        return false;
    }
    let facing = prop(block, "facing").unwrap_or("north");
    let (wanted, u) = match facing {
        "north" => (2, 1.0 - cursor[0]),
        "south" => (3, cursor[0]),
        "west" => (4, cursor[2]),
        _ => (5, 1.0 - cursor[2]),
    };
    if face != wanted {
        return false;
    }
    let powered = prop(block, "powered") == Some("true");
    if powered {
        let cells = chain(state, pos);
        for cell in &cells {
            register(state, *cell);
        }
        let start = match cells.len() {
            1 => 6,
            2 => 3,
            3 => 0,
            _ => return false,
        };
        let Some(player) = state.players.get(&actor) else {
            return true;
        };
        for (i, cell) in cells.iter().enumerate() {
            let incoming = &player.inventory.slots[start + i * 3..start + (i + 1) * 3];
            if crate::plugin_inventories::has_reservations_for_block(state, *cell)
                && crate::plugin_inventories::below_block_floors(state, *cell, incoming)
            {
                return true;
            }
        }
        if crate::plugin_inventories::has_reservations_for_player(state, actor) {
            let mut candidate = player.inventory.slots.clone();
            for (i, cell) in cells.iter().enumerate() {
                candidate[start + i * 3..start + (i + 1) * 3]
                    .clone_from_slice(&state.block_entities[&key(*cell)].slots);
            }
            if crate::plugin_inventories::below_player_floors(state, actor, &candidate) {
                return true;
            }
        }
        let Some(player) = state.players.get_mut(&actor) else {
            return true;
        };
        for (cell, hotbar) in cells.into_iter().zip(
            player.inventory.slots[start..9]
                .as_chunks_mut::<3>()
                .0
                .iter_mut(),
        ) {
            let shelf = &mut state
                .block_entities
                .get_mut(&key(cell))
                .expect("shelf custody")
                .slots;
            for (stored, held) in shelf.iter_mut().zip(hotbar) {
                std::mem::swap(stored, held);
            }
            state.world.mark_block_entity_dirty(cell);
        }
    } else {
        register(state, pos);
        let selected = state
            .players
            .get(&actor)
            .map(|p| usize::from(p.inventory.selected.min(8)))
            .unwrap_or(0);
        let index = (u * 3.0).floor().clamp(0.0, 2.0) as usize;
        let Some(player) = state.players.get(&actor) else {
            return true;
        };
        if crate::plugin_inventories::has_reservations_for_block(state, pos) {
            let mut candidate = state.block_entities[&key(pos)].slots.clone();
            candidate[index] = player.inventory.slots[selected].clone();
            if crate::plugin_inventories::below_block_floors(state, pos, &candidate) {
                return true;
            }
        }
        if crate::plugin_inventories::has_reservations_for_player(state, actor) {
            let mut candidate = player.inventory.slots.clone();
            let stored = &state.block_entities[&key(pos)].slots[index];
            if !player.creative() || !stored.is_empty() {
                candidate[selected] = stored.clone();
            }
            if crate::plugin_inventories::below_player_floors(state, actor, &candidate) {
                return true;
            }
        }
        let Some(player) = state.players.get_mut(&actor) else {
            return true;
        };
        let creative = player.creative();
        let old = player.inventory.slots[selected].clone();
        let shelf = &mut state
            .block_entities
            .get_mut(&key(pos))
            .expect("shelf custody")
            .slots[index];
        std::mem::swap(shelf, &mut player.inventory.slots[selected]);
        if creative && player.inventory.slots[selected].is_empty() {
            player.inventory.slots[selected] = old;
        }
        state.world.mark_block_entity_dirty(pos);
    }
    if let Some(player) = state.players.get_mut(&actor) {
        let connection = player.connection;
        let contents = player.inventory_contents();
        state.push_player(connection, contents);
    }
    for delta in [(0, 0, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)] {
        crate::redstone::schedule_cell(state, offset(pos, delta, 1), 1, 0);
    }
    true
}
fn persist(state: &mut TickState, pos: BlockPos) {
    let Some(record) = state.block_entities.get(&key(pos)) else {
        return;
    };
    let mut fields = state
        .world
        .chunk(crate::boundary::ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos))
        .and_then(|bytes| mc_nbt::read_network(&mut bytes.as_slice()).ok())
        .and_then(|tag| {
            if let Tag::Compound(fields) = tag {
                Some(fields)
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            vec![
                (
                    "id".into(),
                    Tag::String(BlockEntityKind::Shelf.vanilla_id().into()),
                ),
                ("x".into(), Tag::Int(pos.x)),
                ("y".into(), Tag::Int(pos.y)),
                ("z".into(), Tag::Int(pos.z)),
                ("align_items_to_bottom".into(), Tag::Byte(0)),
            ]
        });
    mc_data::storage_blocks::encode_items(&mut fields, &record.slots, &state.registry.items);
    let nbt = Tag::Compound(fields);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &nbt).expect("shelf NBT encodes");
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
    state.world.mark_block_entity_visible(pos);
    let viewers: Vec<_> = state.players.values().map(|p| p.connection).collect();
    for connection in viewers {
        state.push_player(
            connection,
            PlayerChange::BlockEntityData {
                position: pos,
                block_entity_type: BlockEntityKind::Shelf.protocol_id(),
                nbt: nbt.clone(),
            },
        );
    }
}
pub(crate) fn run(state: &mut TickState) {
    let positions: Vec<_> = state
        .block_entities
        .iter()
        .filter_map(|(key, record)| {
            let pos = BlockPos {
                x: key.0,
                y: key.1,
                z: key.2,
            };
            (at(state, pos).is_some_and(|b| is_shelf(b.block.id.path()))
                && state
                    .journaled_block_entities
                    .get(key)
                    .is_none_or(|image| image.0 != record.slots))
            .then_some(pos)
        })
        .collect();
    for pos in positions {
        persist(state, pos);
    }
}
pub(crate) fn checkpoint(state: &mut TickState) {
    let positions: Vec<_> = state
        .block_entities
        .keys()
        .filter_map(|key| {
            let pos = BlockPos {
                x: key.0,
                y: key.1,
                z: key.2,
            };
            at(state, pos)
                .is_some_and(|b| is_shelf(b.block.id.path()))
                .then_some(pos)
        })
        .collect();
    for pos in positions {
        persist(state, pos);
    }
}
#[cfg(test)]
#[path = "shelf_tests.rs"]
mod tests;
