//! Personal chest custody lives in the player's retained EnderItems image,
//! not in the placed block. Decode only for admission/menu events, never ticks.
use crate::boundary::{BlockPos, ChangeSet, ClickMode, ConnectionId, EntityId};
use crate::config::CoreError;
use crate::tick::TickState;
use mc_data::{ItemStack, item_nbt};
use mc_nbt::{ListTag, Tag, tag_type};
use std::sync::Arc;

fn invalid(message: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("EnderItems: {message}"))
}
fn root(bytes: &[u8]) -> Result<Vec<(String, Tag)>, CoreError> {
    if bytes.is_empty() {
        return Ok(Vec::new());
    }
    let mut cursor = std::io::Cursor::new(bytes);
    let (_, tag) = mc_nbt::read_named(&mut cursor).map_err(invalid)?;
    if cursor.position() != bytes.len() as u64 {
        return Err(invalid("trailing NBT"));
    }
    let Tag::Compound(fields) = tag else {
        return Err(invalid("player fields must be compound"));
    };
    Ok(fields)
}
pub(crate) fn contents(
    bytes: &[u8],
    items: &mc_data::items::ItemRegistry,
) -> Result<Vec<ItemStack>, CoreError> {
    let fields = root(bytes)?;
    let mut found = fields.iter().filter(|(key, _)| key == "EnderItems");
    let mut slots = vec![ItemStack::EMPTY; 27];
    let Some((_, value)) = found.next() else {
        return Ok(slots);
    };
    if found.next().is_some() {
        return Err(invalid("duplicate field"));
    }
    let Tag::List(list) = value else {
        return Err(invalid("expected list"));
    };
    if list.elements.len() > 27 {
        return Err(invalid("too many slots"));
    }
    let mut occupied = 0_u32;
    for entry in &list.elements {
        let Tag::Compound(fields) = entry else {
            return Err(invalid("expected item compound"));
        };
        let Some(Tag::Byte(slot)) = item_nbt::field(fields, "Slot") else {
            return Err(invalid("missing byte Slot"));
        };
        if !(0..27).contains(slot) || occupied & (1 << *slot) != 0 {
            return Err(invalid("invalid or duplicate Slot"));
        }
        occupied |= 1 << *slot;
        slots[*slot as usize] = item_nbt::stack_from_fields(items, fields)
            .map_err(invalid)?
            .ok_or_else(|| invalid("empty item"))?;
    }
    Ok(slots)
}
fn store(
    bytes: &[u8],
    slots: &[ItemStack],
    items: &mc_data::items::ItemRegistry,
) -> Result<Vec<u8>, CoreError> {
    let mut fields = root(bytes)?;
    let mut entries = Vec::new();
    for (slot, stack) in slots
        .iter()
        .enumerate()
        .filter(|(_, stack)| !stack.is_empty())
    {
        let mut fields = item_nbt::stack_to_fields(items, stack).map_err(invalid)?;
        fields.push(("Slot".into(), Tag::Byte(slot as i8)));
        entries.push(Tag::Compound(fields));
    }
    fields.retain(|(name, _)| name != "EnderItems");
    fields.push((
        "EnderItems".into(),
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: entries,
        }),
    ));
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", &Tag::Compound(fields)).map_err(invalid)?;
    Ok(bytes)
}
fn is_chest(state: &TickState, position: BlockPos) -> bool {
    state
        .world
        .block_at(position.x, position.y, position.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| block.block.id.as_str() == "minecraft:ender_chest")
}
pub(crate) fn is_open(state: &TickState, actor: EntityId) -> bool {
    state
        .open_containers
        .get(&actor)
        .is_some_and(|&(x, y, z)| is_chest(state, BlockPos { x, y, z }))
}
pub(crate) fn open(
    state: &mut TickState,
    actor: EntityId,
    connection: ConnectionId,
    position: BlockPos,
    changes: &mut ChangeSet,
) -> bool {
    if !is_chest(state, position) {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    if player.vitals.is_dead() {
        return true;
    }
    // A full cube directly above blocks the lid, like an ordinary chest.
    if state
        .world
        .block_at(position.x, position.y + 1, position.z)
        .and_then(|id| {
            state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(id))
                .map(|block| (id, block))
        })
        .is_none_or(|(id, block)| {
            mc_data::block_facts::has_full_sturdy_face(
                id,
                &block.block.id,
                &block.properties,
                mc_data::block_facts::SturdyFace::Down,
            )
        })
    {
        return true;
    }
    let Ok(slots) = contents(&player.opaque_player_fields, &state.registry.items) else {
        return true;
    };
    let registry = Arc::clone(&state.registry);
    let effects = state
        .players
        .get_mut(&actor)
        .expect("chest owner")
        .open_menu(
            &registry.item_rules(),
            crate::inventory::MenuKind::Chest { rows: 3 },
            "Ender Chest".into(),
            &slots,
        );
    crate::tick::apply_player_effects(state, connection, actor, effects, changes);
    state
        .open_containers
        .insert(actor, (position.x, position.y, position.z));
    crate::statistics::opened(state, actor, position);
    true
}
#[allow(clippy::too_many_arguments)]
pub(crate) fn click(
    state: &mut TickState,
    actor: EntityId,
    connection: ConnectionId,
    window: u8,
    state_id: i32,
    slot: i16,
    button: i8,
    mode: ClickMode,
    changes: &mut ChangeSet,
) {
    let Some(player) = state.players.get(&actor) else {
        return;
    };
    let registry = Arc::clone(&state.registry);
    let Ok(mut slots) = contents(&player.opaque_player_fields, &registry.items) else {
        return;
    };
    let native = |player: &mut crate::player::PlayerState, slots: &mut [ItemStack]| {
        player.click(
            &registry.item_rules(),
            window,
            state_id,
            slot,
            button,
            mode,
            slots,
        )
    };
    // Plan first: the personal postimage and player debit commit together, and
    // unsupported serialization cannot consume a stack or mutate cursor custody.
    let plan = player.plan_inventory_transition(&slots, native);
    if crate::plugin_inventories::below_player_floors(state, actor, plan.player_slots()) {
        let effects = player.inventory_transition_refused(&slots);
        crate::tick::apply_player_effects(state, connection, actor, effects, changes);
        return;
    }
    let Ok(bytes) = store(
        &player.opaque_player_fields,
        plan.container_slots(),
        &registry.items,
    ) else {
        return;
    };
    let player = state.players.get_mut(&actor).expect("chest owner");
    let effects = player.apply_inventory_transition(plan, &mut slots);
    if effects.container_changed {
        player.opaque_player_fields = bytes;
    }
    crate::tick::apply_player_effects(state, connection, actor, effects, changes);
}

#[cfg(test)]
#[path = "ender_chest_tests.rs"]
mod tests;
