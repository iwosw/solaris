//! Decorated pots and chiseled bookshelves: native inventory custody, no menu.
use crate::boundary::{BlockPos, EntityId, Hand};
use crate::tick::{BlockEntityState, TickState};
use mc_data::item_nbt::{field, stack_from_fields, stack_to_fields};
use mc_data::ItemStack;
use mc_nbt::Tag;

pub(crate) fn owns(path: &str) -> bool { matches!(path, "decorated_pot" | "chiseled_bookshelf") }
fn fields(state: &TickState, pos: BlockPos) -> Vec<(String, Tag)> {
    state.world.chunk(crate::boundary::ChunkPos { x: pos.x.div_euclid(16), z: pos.z.div_euclid(16) })
        .and_then(|c| c.block_entities.get(&pos))
        .and_then(|bytes| mc_nbt::read_network(&mut bytes.as_slice()).ok())
        .and_then(|tag| if let Tag::Compound(fields) = tag { Some(fields) } else { None }).unwrap_or_default()
}
fn decode(fields: &[(String, Tag)], items: &mc_data::items::ItemRegistry, pot: bool) -> Vec<ItemStack> {
    let mut slots = vec![ItemStack::EMPTY; if pot { 1 } else { 6 }];
    if pot {
        if let Some(Tag::Compound(item)) = field(fields, "item")
            && let Ok(Some(stack)) = stack_from_fields(items, item)
        {
            slots[0] = stack;
        }
    } else if let Some(Tag::List(list)) = field(fields, "Items") {
        for entry in &list.elements {
            if let Tag::Compound(item) = entry
                && let Some(Tag::Byte(index)) = field(item, "Slot")
                && let Some(slot) = slots.get_mut(*index as usize)
                && let Ok(Some(stack)) = stack_from_fields(items, item) {
                *slot = stack;
            }
        }
    }
    slots
}
pub(crate) fn hydrate_chunk(inventories: &mut crate::redstone::custody::BlockEntities, registry: &crate::registry::CoreRegistry, chunk: &mc_world::Chunk) {
    for (pos, bytes) in &chunk.block_entities {
        let Some(block) = chunk.get_block(pos.x.rem_euclid(16) as u8, pos.y, pos.z.rem_euclid(16) as u8).and_then(|id| registry.blocks.by_id(id)) else { continue };
        if !owns(block.block.id.path()) { continue }
        if let Ok(Tag::Compound(fields)) = mc_nbt::read_network(&mut bytes.as_slice()) {
            inventories.hydrate((pos.x,pos.y,pos.z), || BlockEntityState { slots: decode(&fields, &registry.items, block.block.id.path() == "decorated_pot"), furnace: None });
        }
    }
}
pub(crate) fn comparator(state: &TickState, pos: BlockPos) -> Option<u8> {
    let block = state.registry.blocks.by_id(mc_world::BlockStateId(state.world.block_at(pos.x,pos.y,pos.z)?))?;
    match block.block.id.path() {
        "chiseled_bookshelf" => Some(match field(&fields(state, pos), "last_interacted_slot") { Some(Tag::Int(slot)) => (slot + 1).clamp(0,6) as u8, _ => 0 }),
        "decorated_pot" => {
            let slots = &state.block_entities.get(&(pos.x,pos.y,pos.z))?.slots;
            let item = &slots[0];
            Some(if item.is_empty() { 0 } else { (1 + 14 * item.count / state.registry.item_rules().max_stack(item).max(1)).min(15) as u8 })
        }
        _ => None,
    }
}
fn persist(state: &mut TickState, pos: BlockPos, pot: bool, last: usize) {
    let mut fields = fields(state, pos);
    for (name, value) in [("id", Tag::String(format!("minecraft:{}", if pot { "decorated_pot" } else { "chiseled_bookshelf" }))), ("x", Tag::Int(pos.x)), ("y", Tag::Int(pos.y)), ("z", Tag::Int(pos.z))] {
        fields.retain(|(key,_)| key != name);
        fields.push((name.into(), value));
    }
    let slots = &state.block_entities[&(pos.x,pos.y,pos.z)].slots;
    if pot {
        fields.retain(|(key,_)| key != "item");
        if !slots[0].is_empty() { fields.push(("item".into(), Tag::Compound(stack_to_fields(&state.registry.items, &slots[0]).expect("stored item NBT")))); }
    } else {
        mc_data::storage_blocks::encode_items(&mut fields, slots, &state.registry.items);
        fields.retain(|(key,_)| key != "last_interacted_slot");
        fields.push(("last_interacted_slot".into(), Tag::Int(last as i32)));
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("block storage NBT");
    state.world.set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
    state.world.mark_block_entity_dirty(pos);
    crate::redstone::wake(state, pos);
}
pub(crate) fn use_on(state: &mut TickState, actor: EntityId, hand: Hand, pos: BlockPos, face: u8, cursor: [f32;3]) -> bool {
    let Some(block) = state.world.block_at(pos.x,pos.y,pos.z).and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id))) else { return false };
    let path = block.block.id.path();
    if !owns(path) || crate::tick::secondary_use(state, actor) { return false }
    let pot = path == "decorated_pot";
    let mut next_props = block.properties.clone();
    let block_name = block.block.id.clone();
    let index = if pot { 0 } else {
        let facing = block.properties.iter().find(|(key,_)| key == "facing").map(|(_,v)| v.as_str()).unwrap_or("north");
        let (wanted, u) = match facing { "north" => (2, 1.0-cursor[0]), "south" => (3,cursor[0]), "west" => (4,cursor[2]), _ => (5,1.0-cursor[2]) };
        if face != wanted { return false }
        let column = if u < 0.375 { 0 } else if u < 0.6875 { 1 } else { 2 };
        column + if cursor[1] >= 0.5 { 0 } else { 3 }
    };
    let Some(player) = state.players.get(&actor) else { return true };
    if player.spectator() || player.vitals.is_dead() { return true }
    let held_index = crate::tick::held_slot_index(player, hand);
    let held = player.inventory.slots[held_index].clone();
    let key = (pos.x,pos.y,pos.z);
    if !state.block_entities.contains_key(&key) {
        let slots = decode(&fields(state,pos), &state.registry.items, pot);
        state.block_entities.insert(key, BlockEntityState { slots, furnace: None });
    }
    let stored = &state.block_entities[&key].slots[index];
    let take = !pot && !stored.is_empty();
    if take {
        if crate::plugin_inventories::has_reservations_for_block(state, pos) { return true }
        let mut stack = stored.clone();
        state.block_entities.get_mut(&key).expect("storage").slots[index] = ItemStack::EMPTY;
        let rules = state.registry.item_rules();
        state.players.get_mut(&actor).expect("player").inventory.add(&rules, &mut stack);
        if !stack.is_empty() {
            let id = crate::tick::spawn_item_entity(state, [f64::from(pos.x)+0.5,f64::from(pos.y)+0.5,f64::from(pos.z)+0.5], stack);
            state.hostile.spawned.push(id);
        }
    } else {
        if held.is_empty() || (!pot && !state.registry.tags.contains_raw_id("minecraft:item", "minecraft:bookshelf_books", held.item_id as i32)) { return true }
        if !stored.is_empty() && (!mc_data::inventory_semantics_26_1_2::can_stack(stored, &held) || stored.count >= state.registry.item_rules().max_stack(stored)) { return true }
        if crate::tick::held_debit_below_floor(state, actor, hand, 1) { return true }
        let stored = &mut state.block_entities.get_mut(&key).expect("storage").slots[index];
        if stored.is_empty() { *stored = held; stored.count = 1 } else { stored.count += 1 }
        let player = state.players.get_mut(&actor).expect("player");
        player.consume_held(hand, 1);
    }
    if !pot {
        let property = format!("slot_{index}_occupied");
        if let Some((_,value)) = next_props.iter_mut().find(|(key,_)| key == &property) { *value = (!take).to_string(); }
        if let Some(next) = state.registry.blocks.by_name_and_props(&block_name, &next_props) {
            state.world.set_block(pos.x,pos.y,pos.z,next.0);
            state.merge_block_writes.push((pos,next.0));
        }
    }
    let player = state.players.get_mut(&actor).expect("player");
    let connection = player.connection;
    let contents = player.inventory_contents();
    state.push_player(connection, contents);
    persist(state,pos,pot,index);
    true
}
