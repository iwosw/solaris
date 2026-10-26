//! Decorated pots: native single-slot custody, no menu.
//! Chiseled-bookcase custody lives in VanillaB-owned code; this module
//! must not claim it (Main ruling 2026-10-26).
use crate::boundary::{BlockPos, EntityId, Hand};
use crate::tick::{BlockEntityState, TickState};
use mc_data::ItemStack;
use mc_data::item_nbt::{field, stack_from_fields, stack_to_fields};
use mc_nbt::Tag;

pub(crate) fn owns(path: &str) -> bool {
    path == "decorated_pot"
}
fn fields(state: &TickState, pos: BlockPos) -> Vec<(String, Tag)> {
    state
        .world
        .chunk(crate::boundary::ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|c| c.block_entities.get(&pos))
        .and_then(|bytes| mc_nbt::read_network(&mut bytes.as_slice()).ok())
        .and_then(|tag| {
            if let Tag::Compound(fields) = tag {
                Some(fields)
            } else {
                None
            }
        })
        .unwrap_or_default()
}
fn decode(
    fields: &[(String, Tag)],
    items: &mc_data::items::ItemRegistry,
) -> Result<Vec<ItemStack>, String> {
    let stack = match field(fields, "item") {
        None => ItemStack::EMPTY,
        Some(Tag::Compound(item)) => stack_from_fields(items, item)
            .map_err(|e| format!("item: {e}"))?
            .ok_or("item: missing item id")?,
        Some(_) => return Err("item: expected item compound".into()),
    };
    Ok(vec![stack])
}
pub(crate) fn decode_chunk(
    inventories: &crate::redstone::custody::BlockEntities,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) -> Result<Vec<((i32, i32, i32), Vec<ItemStack>)>, String> {
    let mut decoded = Vec::new();
    for (pos, bytes) in &chunk.block_entities {
        let Some(block) = chunk
            .get_block(
                pos.x.rem_euclid(16) as u8,
                pos.y,
                pos.z.rem_euclid(16) as u8,
            )
            .and_then(|id| registry.blocks.by_id(id))
        else {
            continue;
        };
        let key = (pos.x, pos.y, pos.z);
        if !owns(block.block.id.path()) || inventories.contains_key(&key) {
            continue;
        }
        let fields = match mc_nbt::read_network(&mut bytes.as_slice()) {
            Ok(Tag::Compound(fields)) => fields,
            Ok(_) => return Err(format!("{} {pos:?}: expected compound", block.block.id)),
            Err(error) => return Err(format!("{} {pos:?}: {error}", block.block.id)),
        };
        let slots = decode(&fields, &registry.items)
            .map_err(|e| format!("{} {pos:?}: {e}", block.block.id))?;
        decoded.push((key, slots));
    }
    Ok(decoded)
}
pub(crate) fn comparator(state: &TickState, pos: BlockPos) -> Option<u8> {
    let block = state.registry.blocks.by_id(mc_world::BlockStateId(
        state.world.block_at(pos.x, pos.y, pos.z)?,
    ))?;
    if block.block.id.path() != "decorated_pot" {
        return None;
    }
    let slots = &state.block_entities.get(&(pos.x, pos.y, pos.z))?.slots;
    let item = &slots[0];
    Some(if item.is_empty() {
        0
    } else {
        (1 + 14 * item.count / state.registry.item_rules().max_stack(item).max(1)).min(15) as u8
    })
}
fn persist(state: &mut TickState, pos: BlockPos) {
    let mut fields = fields(state, pos);
    for (name, value) in [
        ("id", Tag::String("minecraft:decorated_pot".into())),
        ("x", Tag::Int(pos.x)),
        ("y", Tag::Int(pos.y)),
        ("z", Tag::Int(pos.z)),
    ] {
        fields.retain(|(key, _)| key != name);
        fields.push((name.into(), value));
    }
    let slots = &state.block_entities[&(pos.x, pos.y, pos.z)].slots;
    fields.retain(|(key, _)| key != "item");
    if !slots[0].is_empty() {
        fields.push((
            "item".into(),
            Tag::Compound(
                stack_to_fields(&state.registry.items, &slots[0]).expect("stored item NBT"),
            ),
        ));
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("block storage NBT");
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
    state.world.mark_block_entity_dirty(pos);
    crate::redstone::wake(state, pos);
}
pub(crate) fn use_on(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    pos: BlockPos,
    _face: u8,
    _cursor: [f32; 3],
) -> bool {
    let Some(block) = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
    else {
        return false;
    };
    if !owns(block.block.id.path()) || crate::tick::secondary_use(state, actor) {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    if player.spectator() || player.vitals.is_dead() {
        return true;
    }
    let held_index = crate::tick::held_slot_index(player, hand);
    let held = player.inventory.slots[held_index].clone();
    let key = (pos.x, pos.y, pos.z);
    // Persisted pots enter custody through chunk hydration; a missing entry
    // here is a freshly placed pot.
    if !state.block_entities.contains_key(&key) {
        let slots = vec![ItemStack::EMPTY];
        state.block_entities.insert(
            key,
            BlockEntityState {
                slots,
                furnace: None,
            },
        );
    }
    let stored = &state.block_entities[&key].slots[0];
    if held.is_empty() {
        return true;
    }
    if !stored.is_empty()
        && (!mc_data::inventory_semantics_26_1_2::can_stack(stored, &held)
            || stored.count >= state.registry.item_rules().max_stack(stored))
    {
        return true;
    }
    if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
        return true;
    }
    let stored = &mut state.block_entities.get_mut(&key).expect("storage").slots[0];
    if stored.is_empty() {
        *stored = held;
        stored.count = 1
    } else {
        stored.count += 1
    }
    let player = state.players.get_mut(&actor).expect("player");
    player.consume_held(hand, 1);
    let player = state.players.get_mut(&actor).expect("player");
    let connection = player.connection;
    let contents = player.inventory_contents();
    state.push_player(connection, contents);
    persist(state, pos);
    true
}
