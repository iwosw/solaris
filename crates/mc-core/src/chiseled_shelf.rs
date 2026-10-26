//! Chiseled bookshelf storage (vanilla 6-slot, books only).
use mc_data::ItemStack;
use mc_data::block_entity_kinds::BlockEntityKind;
use mc_nbt::Tag;

use crate::boundary::{BlockPos, EntityId, Hand, PlayerChange};
use crate::tick::TickState;

pub(crate) fn is_chiseled(path: &str) -> bool {
    BlockEntityKind::for_block(path) == Some(BlockEntityKind::ChiseledBookshelf)
}

fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}

fn at(state: &TickState, pos: BlockPos) -> Option<&mc_world::BlockState> {
    state.registry.blocks.by_id(mc_world::BlockStateId(
        state.world.block_at(pos.x, pos.y, pos.z)?,
    ))
}

fn prop<'a>(block: &'a mc_world::BlockState, name: &str) -> Option<&'a str> {
    block
        .properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}

/// Vanilla BOOKSHELF_BOOKS tag (bookshelf_books.json, 26.1.2 data).
fn accepts(state: &TickState, stack: &ItemStack) -> bool {
    !stack.is_empty()
        && state.registry.tags.contains_raw_id(
            "minecraft:item",
            "minecraft:bookshelf_books",
            stack.item_id as i32,
        )
}

/// Comparator output (ChiseledBookShelfBlock.getAnalogOutputSignal,
/// 26.1.2): getLastInteractedSlot() + 1 (block entity field, -1 when never
/// touched). This is the last slot the player added to or removed from —
/// not the filled count.
pub(crate) fn comparator(state: &TickState, pos: BlockPos) -> u8 {
    let chunk = state.world.chunk(crate::boundary::ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    });
    let slot = chunk
        .and_then(|chunk| chunk.block_entities.get(&pos))
        .and_then(|bytes| mc_nbt::read_network(&mut bytes.as_slice()).ok())
        .and_then(|tag| {
            if let Tag::Compound(fields) = tag {
                Some(fields)
            } else {
                None
            }
        })
        .and_then(|fields| {
            fields.iter().find_map(|(name, tag)| {
                if name == "last_interacted_slot" {
                    if let Tag::Int(v) = tag {
                        Some(*v)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
        })
        .unwrap_or(-1);
    (slot + 1).clamp(0, 6) as u8
}

fn sync_occupied(state: &mut TickState, pos: BlockPos) {
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(current)) else {
        return;
    };
    let occupied: Vec<bool> = state
        .block_entities
        .get(&key(pos))
        .map(|record| record.slots.iter().map(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    if occupied.len() < 6 {
        return;
    }
    let mut properties = block.properties.clone();
    for (i, full) in occupied.iter().enumerate() {
        let name = format!("slot_{i}_occupied");
        if let Some((_, value)) = properties.iter_mut().find(|(k, _)| *k == name) {
            *value = if *full { "true".into() } else { "false".into() };
        }
    }
    let Some(next) = state
        .registry
        .blocks
        .by_name_and_props(&block.block.id, &properties)
    else {
        return;
    };
    state.world.set_block(pos.x, pos.y, pos.z, next.0);
    state.merge_block_writes.push((pos, next.0));
}

/// Insert/swap/take a book by cursor (top row = slots 0-2, bottom = 3-5).
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
    if !is_chiseled(block.block.id.path()) || hand != Hand::Main {
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
    // Chiseled shelves keep 6 book slots (independent of container custody).
    state
        .block_entities
        .entry(key(pos))
        .or_insert_with(|| crate::tick::BlockEntityState {
            slots: vec![ItemStack::EMPTY; 6],
            furnace: None,
        });
    if state.block_entities.get(&key(pos)).map(|r| r.slots.len()) != Some(6) {
        state.block_entities.insert(
            key(pos),
            crate::tick::BlockEntityState {
                slots: vec![ItemStack::EMPTY; 6],
                furnace: None,
            },
        );
    }
    // SelectableSlotContainer default (26.1.2): section(f, n) =
    // clamp(floor(f * 16 / (16 / n)), 0, n - 1); index = col + row * 3
    // with row from (1 - y) over 2 rows, col from u over 3 columns.
    let section =
        |f: f32, n: i32| ((f * 16.0 / (16.0 / n as f32)).floor() as i32).clamp(0, n - 1) as usize;
    let col = section(u, 3);
    let row = section(1.0 - cursor[1], 2);
    let index = col + row * 3;
    let selected = state
        .players
        .get(&actor)
        .map(|p| usize::from(p.inventory.selected.min(8)))
        .unwrap_or(0);
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    let held = player.inventory.slots[selected].clone();
    if !held.is_empty() && !accepts(state, &held) {
        return false;
    }
    let Some(player) = state.players.get_mut(&actor) else {
        return true;
    };
    let shelf = &mut state
        .block_entities
        .get_mut(&key(pos))
        .expect("chiseled custody")
        .slots[index];
    std::mem::swap(shelf, &mut player.inventory.slots[selected]);
    state.world.mark_block_entity_dirty(pos);
    sync_occupied(state, pos);
    if let Some(player) = state.players.get_mut(&actor) {
        let connection = player.connection;
        let contents = player.inventory_contents();
        state.push_player(connection, contents);
    }
    for delta in [(0, 0, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)] {
        crate::redstone::schedule_cell(
            state,
            BlockPos {
                x: pos.x + delta.0,
                y: pos.y + delta.1,
                z: pos.z + delta.2,
            },
            1,
            0,
        );
    }
    persist(state, pos, index as i32);
    true
}

fn persist(state: &mut TickState, pos: BlockPos, last_slot: i32) {
    let Some(record) = state.block_entities.get(&key(pos)) else {
        return;
    };
    let mut fields = vec![
        (
            "id".into(),
            Tag::String(BlockEntityKind::ChiseledBookshelf.vanilla_id().into()),
        ),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ];
    mc_data::storage_blocks::encode_items(&mut fields, &record.slots, &state.registry.items);
    fields.push(("last_interacted_slot".into(), Tag::Int(last_slot)));
    let nbt = Tag::Compound(fields);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &nbt).expect("chiseled shelf NBT encodes");
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
                block_entity_type: BlockEntityKind::ChiseledBookshelf.protocol_id(),
                nbt: nbt.clone(),
            },
        );
    }
}
