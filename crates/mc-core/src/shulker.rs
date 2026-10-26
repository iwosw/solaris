//! Native 27-slot shulker custody; accepted item components restore after removal.
use crate::boundary::{BlockPos, ChunkPos};
use crate::tick::{BlockEntityState, TickState};
use mc_data::block_entity_kinds::BlockEntityKind;
use mc_data::item_nbt::{field, stack_from_fields, stack_to_fields};
use mc_data::{ItemStack, items::ItemRegistry};
use mc_nbt::Tag;

fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}
pub(crate) fn is_block(path: &str) -> bool {
    BlockEntityKind::for_block(path) == Some(BlockEntityKind::ShulkerBox)
}
pub(crate) fn is_item(items: &ItemRegistry, stack: &ItemStack) -> bool {
    items
        .name_of(stack.item_id)
        .is_some_and(|id| is_block(id.path()))
}
fn at(state: &TickState, pos: BlockPos) -> bool {
    state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| is_block(block.block.id.path()))
}
fn failure(message: impl std::fmt::Display) -> crate::CoreError {
    crate::CoreError::Storage(format!("shulker box: {message}"))
}
fn decode(bytes: &[u8], items: &ItemRegistry) -> Result<Vec<ItemStack>, crate::CoreError> {
    let mut input = bytes;
    let Tag::Compound(fields) = mc_nbt::read_network(&mut input).map_err(failure)? else {
        return Err(failure("expected compound NBT"));
    };
    if !input.is_empty() {
        return Err(failure("trailing NBT"));
    }
    if field(&fields, "id")
        != Some(&Tag::String(
            BlockEntityKind::ShulkerBox.vanilla_id().into(),
        ))
    {
        return Err(failure("id: expected minecraft:shulker_box"));
    }
    mc_data::storage_blocks::decode_slots(&fields, items, 27).map_err(failure)
}
pub(crate) fn decode_chunk(
    inventories: &crate::redstone::custody::BlockEntities,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) -> Result<Vec<((i32, i32, i32), Vec<ItemStack>)>, crate::CoreError> {
    let mut decoded = Vec::new();
    for (pos, bytes) in &chunk.block_entities {
        if inventories.contains_key(&key(*pos)) {
            continue;
        }
        if chunk
            .get_block(
                pos.x.rem_euclid(16) as u8,
                pos.y,
                pos.z.rem_euclid(16) as u8,
            )
            .and_then(|id| registry.blocks.by_id(id))
            .is_none_or(|block| !is_block(block.block.id.path()))
        {
            continue;
        }
        decoded.push((key(*pos), decode(bytes, &registry.items)?));
    }
    Ok(decoded)
}

pub(crate) struct Placement {
    pos: BlockPos,
    slots: Vec<ItemStack>,
    name: Option<Tag>,
}
fn component<'a>(
    fields: &'a [(String, Tag)],
    name: &str,
) -> Result<Option<&'a Tag>, crate::CoreError> {
    let mut found = fields.iter().filter(|(key, _)| key == name);
    let value = found.next().map(|(_, value)| value);
    if found.next().is_some() {
        return Err(failure(format!("{name}: duplicate component")));
    }
    Ok(value)
}
pub(crate) fn place_from_item(
    state: &mut TickState,
    intent: &crate::intent::Intent,
) -> Result<(), crate::CoreError> {
    let crate::intent::IntentKind::PlaceBlock { pos, state: id } = intent.kind else {
        return Ok(());
    };
    if state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .is_none_or(|block| !is_block(block.block.id.path()))
    {
        return Ok(());
    }
    let claim = intent
        .footprint
        .inv
        .iter()
        .find(|claim| claim.owner == intent.actor);
    let stack = claim
        .map(|claim| {
            state
                .players
                .get(&claim.owner)
                .and_then(|player| player.inventory.slots.get(usize::from(claim.slot)))
                .ok_or_else(|| failure("missing accepted placement stack"))
        })
        .transpose()?;
    let placement = placement_from_stack(state, pos, stack)?;
    state.merge_shulker_placements.push(placement);
    Ok(())
}

pub(crate) fn placement_from_stack(
    state: &TickState,
    pos: BlockPos,
    stack: Option<&ItemStack>,
) -> Result<Placement, crate::CoreError> {
    let mut slots = vec![ItemStack::EMPTY; 27];
    let mut name = None;
    if let Some(stack) = stack {
        let fields = stack_to_fields(&state.registry.items, stack).map_err(failure)?;
        if let Some(Tag::Compound(components)) = field(&fields, "components") {
            name = component(components, "minecraft:custom_name")?.cloned();
            if let Some(container) = component(components, "minecraft:container")? {
                let Tag::List(list) = container else {
                    return Err(failure("minecraft:container: expected compound list"));
                };
                let mut seen = 0u32;
                for entry in &list.elements {
                    let Tag::Compound(entry) = entry else {
                        return Err(failure("minecraft:container[]: expected compound"));
                    };
                    let Some(Tag::Int(slot)) = component(entry, "slot")? else {
                        return Err(failure("minecraft:container[].slot: expected int"));
                    };
                    if !(0..27).contains(slot) {
                        return Err(failure("minecraft:container[].slot: expected 0..=26"));
                    }
                    let slot = *slot as usize;
                    if seen & (1 << slot) != 0 {
                        return Err(failure(format!(
                            "minecraft:container[].slot: duplicate slot {slot}"
                        )));
                    }
                    seen |= 1 << slot;
                    let Some(Tag::Compound(item)) = component(entry, "item")? else {
                        return Err(failure("minecraft:container[].item: expected compound"));
                    };
                    slots[slot] = stack_from_fields(&state.registry.items, item)
                        .map_err(failure)?
                        .ok_or_else(|| failure("minecraft:container[].item: missing item id"))?;
                }
            }
        }
    }
    Ok(Placement { pos, slots, name })
}
pub(crate) fn commit_placements(state: &mut TickState) {
    for placement in std::mem::take(&mut state.merge_shulker_placements) {
        commit_placement(state, placement);
    }
}

pub(crate) fn commit_placement(state: &mut TickState, placement: Placement) {
    if !at(state, placement.pos) {
        return;
    }
    state.block_entities.insert(
        key(placement.pos),
        BlockEntityState {
            slots: placement.slots,
            furnace: None,
        },
    );
    let mut fields = identity(placement.pos);
    if let Some(name) = placement.name {
        fields.push(("CustomName".into(), name));
    }
    persist_fields(state, placement.pos, fields);
}
fn identity(pos: BlockPos) -> Vec<(String, Tag)> {
    vec![
        (
            "id".into(),
            Tag::String(BlockEntityKind::ShulkerBox.vanilla_id().into()),
        ),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ]
}
fn persist_fields(state: &mut TickState, pos: BlockPos, mut fields: Vec<(String, Tag)>) {
    let Some(record) = state.block_entities.get(&key(pos)) else {
        return;
    };
    mc_data::storage_blocks::encode_items(&mut fields, &record.slots, &state.registry.items);
    // ShulkerBoxBlockEntity uses ContainerHelper.saveAllItems(..., false).
    if record.slots.iter().all(ItemStack::is_empty) {
        fields.retain(|(key, _)| key != "Items");
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("shulker NBT encodes");
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
}
fn persist(state: &mut TickState, pos: BlockPos) {
    let fields = state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos))
        .and_then(|bytes| mc_nbt::read_network(&mut bytes.as_slice()).ok())
        .and_then(|tag| match tag {
            Tag::Compound(fields) => Some(fields),
            _ => None,
        })
        .unwrap_or_else(|| identity(pos));
    persist_fields(state, pos, fields);
}
fn flush(state: &mut TickState, checkpoint: bool) {
    let positions: Vec<_> = state
        .block_entities
        .iter()
        .filter_map(|(key, record)| {
            let pos = BlockPos {
                x: key.0,
                y: key.1,
                z: key.2,
            };
            (at(state, pos)
                && (checkpoint
                    || state
                        .journaled_block_entities
                        .get(key)
                        .is_none_or(|image| image.0 != record.slots)))
            .then_some(pos)
        })
        .collect();
    for pos in positions {
        persist(state, pos);
    }
}
pub(crate) fn run(state: &mut TickState) {
    flush(state, false);
}
pub(crate) fn checkpoint(state: &mut TickState) {
    flush(state, true);
}

#[cfg(test)]
#[path = "shulker_tests.rs"]
mod tests;
