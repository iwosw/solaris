//! Native-owner container automation. Inventory and entity consumption commit in
//! the same tick; cooldown post-images use vanilla hopper NBT in the opaque WAL.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use mc_data::inventory_semantics_26_1_2::{can_stack, decrement_stack};
use mc_data::item_nbt::{field, stack_from_fields, stack_to_fields};
use mc_data::{Identifier, ItemStack};
use mc_nbt::{ListTag, Tag, tag_type};

use crate::boundary::{BlockPos, ChunkPos, EntityId};
use crate::entities::{EntityKind, StackRow};
use crate::registry::CoreRegistry;
use crate::tick::{BlockEntityState, TickState, spawn_item_entity};

#[derive(Debug, Default)]
pub(crate) struct ContainerState {
    /// Absolute simulation-tick deadlines, not per-tick persisted countdowns.
    cooldowns: BTreeMap<(i32, i32, i32), u64>,
    /// Exact positions of loaded hopper block entities; never scan every
    /// container to discover the hopper lane.
    hoppers: BTreeSet<(i32, i32, i32)>,
    /// Due hoppers that found neither an extractable source nor an insertable
    /// destination. A local inventory or item-motion change wakes them.
    sleeping: BTreeSet<(i32, i32, i32)>,
    dirty: BTreeSet<(i32, i32, i32)>,
    last_tick: u64,
}

impl ContainerState {
    fn remaining(&self, pos: (i32, i32, i32), tick: u64) -> i32 {
        self.cooldowns
            .get(&pos)
            .copied()
            .unwrap_or(tick)
            .saturating_sub(tick)
            .min(8) as i32
    }

    fn set_cooldown(&mut self, pos: (i32, i32, i32), ticks: u64, tick: u64) {
        self.cooldowns.insert(pos, tick.saturating_add(ticks));
        self.dirty.insert(pos);
    }
}

fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}
fn position(pos: (i32, i32, i32)) -> BlockPos {
    BlockPos {
        x: pos.0,
        y: pos.1,
        z: pos.2,
    }
}

/// Wake hoppers whose own inventory or one adjacent inventory changed.
pub(crate) fn inventory_changed(containers: &mut ContainerState, pos: BlockPos) {
    for step in [
        (0, 0, 0),
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ] {
        if let Some(cell) = super::offset(pos, step) {
            containers.sleeping.remove(&key(cell));
        }
    }
}

/// Item movement wakes only hopper bowls which the item can enter, retaining
/// vanilla suction without making unrelated sleeping hoppers poll each tick.
pub(crate) fn item_moved(containers: &mut ContainerState, pos: [f64; 3]) {
    let x = pos[0].floor() as i32;
    let y = pos[1].floor() as i32;
    let z = pos[2].floor() as i32;
    for hopper_y in [y, y - 1] {
        for hopper_z in [z - 1, z] {
            for hopper_x in [x - 1, x] {
                containers.sleeping.remove(&(hopper_x, hopper_y, hopper_z));
            }
        }
    }
}

fn kind(state: &TickState, pos: BlockPos) -> Option<&str> {
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(
            state.world.block_at(pos.x, pos.y, pos.z)?,
        ))
        .map(|block| block.block.id.path())
}

fn size(name: &str) -> Option<usize> {
    match name {
        _ if crate::copper::chest(name) => Some(27),
        "chest" | "trapped_chest" | "barrel" => Some(27),
        name if crate::shulker::is_block(name) => Some(27),
        "hopper" | "brewing_stand" => Some(5),
        "jukebox" => Some(1),
        "dispenser" | "dropper" | "crafter" => Some(9),
        "furnace" | "smoker" | "blast_furnace" => Some(3),
        name if crate::shelf::is_shelf(name) => Some(3),
        _ => None,
    }
}

pub(super) fn has_analog_output(name: &str) -> bool {
    size(name).is_some()
        || crate::copper::family(name).is_some_and(|(family, _, _)| family == "copper_golem_statue")
}

fn automated(name: &str) -> bool {
    matches!(name, "hopper" | "dropper" | "dispenser")
}

/// Called for cells discovered by the runtime/placement pass. Loaded custody
/// always wins over empty defaults, including when a hopper was never opened.
pub(crate) fn register(state: &mut TickState, pos: BlockPos) {
    let Some(name) = kind(state, pos) else { return };
    if name == "jukebox" {
        crate::jukebox::register(state, pos);
        return;
    }
    let Some(count) = size(name) else { return };
    let hopper = name == "hopper";
    if state.block_entities.contains_key(&key(pos)) {
        if hopper {
            state.redstone_containers.hoppers.insert(key(pos));
            state.redstone_containers.sleeping.remove(&key(pos));
            state
                .redstone_containers
                .cooldowns
                .entry(key(pos))
                .or_insert(state.tick);
        }
        return;
    }
    let automation = automated(name);
    let furnace = matches!(name, "furnace" | "smoker" | "blast_furnace").then(Default::default);
    let loaded = state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos))
        .and_then(|bytes| decode(bytes, &state.registry.items, count, state.tick));
    let (slots, cooldown) = loaded.unwrap_or_else(|| (vec![ItemStack::EMPTY; count], state.tick));
    state
        .block_entities
        .insert(key(pos), BlockEntityState { slots, furnace });
    if automation {
        state.redstone_containers.dirty.insert(key(pos));
    }
    if hopper {
        state.redstone_containers.hoppers.insert(key(pos));
        state.redstone_containers.sleeping.remove(&key(pos));
        state
            .redstone_containers
            .cooldowns
            .entry(key(pos))
            .or_insert(cooldown);
    }
}

/// Map-style hydration allows borrowing the resident chunk and native storage
/// independently. Replayed inventory images are never replaced by Anvil data.
pub(crate) fn hydrate_chunk(
    inventories: &mut super::custody::BlockEntities,
    containers: &mut ContainerState,
    registry: &CoreRegistry,
    chunk: &mc_world::Chunk,
    tick: u64,
) {
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
        let name = block.block.id.path();
        // Native chest/furnace hydration owns those typed inventories and
        // furnace progress; never preempt it with an opaque slots-only image.
        if !automated(name) {
            continue;
        }
        if inventories.contains_key(&key(*pos))
            && (name != "hopper" || containers.cooldowns.contains_key(&key(*pos)))
        {
            if name == "hopper" {
                containers.hoppers.insert(key(*pos));
                containers.sleeping.remove(&key(*pos));
            }
            continue;
        }
        let Some(count) = size(name) else { continue };
        let Some((slots, cooldown)) = decode(bytes, &registry.items, count, tick) else {
            continue;
        };
        inventories.hydrate(key(*pos), || BlockEntityState {
            slots,
            furnace: None,
        });
        if name == "hopper" {
            containers.hoppers.insert(key(*pos));
            containers.sleeping.remove(&key(*pos));
            containers.cooldowns.entry(key(*pos)).or_insert(cooldown);
        }
    }
    for (pos, hopper) in &chunk.hoppers {
        let pos = key(*pos);
        inventories.hydrate(pos, || BlockEntityState {
            slots: hopper.slots.iter().map(slot_stack).collect(),
            furnace: None,
        });
        containers.hoppers.insert(pos);
        containers.sleeping.remove(&pos);
        containers
            .cooldowns
            .entry(pos)
            .or_insert(tick.saturating_add(hopper.transfer_cooldown.clamp(0, 8) as u64));
    }
}

fn slot_stack(slot: &mc_world::FurnaceSlot) -> ItemStack {
    if slot.is_empty() {
        return ItemStack::EMPTY;
    }
    ItemStack {
        count: slot.count,
        item_id: slot.item_id,
        damage: slot.damage,
        enchantments: slot.enchantments.clone(),
        custom_name: slot.custom_name.as_deref().map(Into::into),
        item_model: slot.item_model.clone(),
        stew_effects: slot.stew_effects.clone(),
        components: slot.components.clone(),
    }
}

fn decode(
    mut bytes: &[u8],
    items: &mc_data::items::ItemRegistry,
    count: usize,
    tick: u64,
) -> Option<(Vec<ItemStack>, u64)> {
    let Tag::Compound(fields) = mc_nbt::read_network(&mut bytes).ok()? else {
        return None;
    };
    let mut slots = vec![ItemStack::EMPTY; count];
    if let Some(Tag::List(items_nbt)) = field(&fields, "Items") {
        for item in &items_nbt.elements {
            let Tag::Compound(fields) = item else {
                return None;
            };
            let index = match field(fields, "Slot") {
                Some(Tag::Byte(index)) => i32::from(*index),
                Some(Tag::Int(index)) => *index,
                _ => continue,
            };
            if let Ok(index) = usize::try_from(index)
                && index < count
            {
                slots[index] = stack_from_fields(items, fields).ok()??;
            }
        }
    }
    let cooldown = match field(&fields, "solaris:TransferCooldownUntil") {
        Some(Tag::Long(value)) => {
            let until = (*value).max(0) as u64;
            tick.saturating_add(until.saturating_sub(tick).min(8))
        }
        _ => match field(&fields, "TransferCooldown") {
            Some(Tag::Int(value)) => tick.saturating_add((*value).clamp(0, 8) as u64),
            _ => tick,
        },
    };
    Some((slots, cooldown))
}

fn encode(state: &TickState, pos: BlockPos, name: &str, slots: &[ItemStack]) -> Vec<u8> {
    // Keep other vanilla fields (CustomName, Lock, etc.) intact.
    let mut fields = state
        .world
        .chunk(ChunkPos {
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
        .unwrap_or_default();
    fields.retain(|(name, _)| {
        !matches!(
            name.as_str(),
            "id" | "x" | "y" | "z" | "Items" | "TransferCooldown" | "solaris:TransferCooldownUntil"
        )
    });
    fields.extend([
        ("id".into(), Tag::String(format!("minecraft:{name}"))),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ]);
    let elements: Vec<_> = slots
        .iter()
        .enumerate()
        .filter(|(_, stack)| !stack.is_empty())
        .map(|(index, stack)| {
            let mut fields = stack_to_fields(&state.registry.items, stack)
                .expect("registered native container item");
            fields.push(("Slot".into(), Tag::Byte(index as i8)));
            Tag::Compound(fields)
        })
        .collect();
    fields.push((
        "Items".into(),
        Tag::List(ListTag {
            element_type: if elements.is_empty() {
                tag_type::END
            } else {
                tag_type::COMPOUND
            },
            elements,
        }),
    ));
    if name == "hopper" {
        // The deadline is stable during cooling, so elapsed time needs no WAL
        // image. Checkpoints also refresh the vanilla relative export field.
        fields.push((
            "TransferCooldown".into(),
            Tag::Int(state.redstone_containers.remaining(key(pos), state.tick)),
        ));
        fields.push((
            "solaris:TransferCooldownUntil".into(),
            Tag::Long(
                state
                    .redstone_containers
                    .cooldowns
                    .get(&key(pos))
                    .copied()
                    .unwrap_or(state.tick)
                    .min(i64::MAX as u64) as i64,
            ),
        ));
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("container NBT encodes");
    bytes
}

fn persist(state: &mut TickState, pos: BlockPos) {
    state.redstone_containers.dirty.remove(&key(pos));
    let Some(name) = kind(state, pos) else { return };
    if !automated(name) {
        return;
    }
    let Some(record) = state.block_entities.get(&key(pos)) else {
        return;
    };
    let bytes = encode(state, pos, name, &record.slots);
    let previous = state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos));
    if previous == Some(&bytes) {
        return;
    }
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
}

/// Freeze automation custody and remaining cooldown into the region base.
/// `run` also persists each tick before WAL post-images are collected.
pub(crate) fn checkpoint(state: &mut TickState) {
    let positions: Vec<_> = state.block_entities.keys().copied().collect();
    for pos in positions {
        persist(state, position(pos));
    }
}

fn cells(state: &TickState, pos: BlockPos) -> Vec<BlockPos> {
    crate::chests::storage_cells(&state.registry.blocks, pos, &|cell| {
        state.world.block_at(cell.x, cell.y, cell.z)
    })
}

/// Container comparator strength uses the actual item stack limit, not 64.
pub(super) fn signal(state: &TickState, pos: BlockPos) -> u8 {
    if let Some(signal) = crate::jukebox::comparator(state, pos) {
        return signal;
    }
    if kind(state, pos) == Some("crafter") {
        return crate::crafter::signal(state, pos);
    }
    if let Some(block) = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        && crate::copper::family(block.block.id.path())
            .is_some_and(|(family, _, _)| family == "copper_golem_statue")
    {
        return block
            .properties
            .iter()
            .find(|(key, _)| key == "copper_golem_pose")
            .and_then(|(_, value)| crate::copper::POSES.iter().position(|pose| pose == value))
            .map_or(1, |pose| pose as u8 + 1);
    }
    if kind(state, pos).and_then(size).is_none() {
        return 0;
    }
    let mut fullness = 0.0;
    let mut count = 0;
    let mut occupied = false;
    for cell in cells(state, pos) {
        count += kind(state, cell).and_then(size).unwrap_or(0) as u32;
        let Some(record) = state.block_entities.get(&key(cell)) else {
            continue;
        };
        for stack in &record.slots {
            if !stack.is_empty() {
                occupied = true;
                fullness += f64::from(stack.count)
                    / f64::from(state.registry.item_rules().max_stack(stack).clamp(1, 99));
            }
        }
    }
    if count == 0 {
        0
    } else {
        ((fullness / f64::from(count) * 14.0).floor() as u8 + u8::from(occupied)).min(15)
    }
}

fn direction(facing: &str) -> Option<(i32, i32, i32)> {
    match facing {
        "down" => Some((0, -1, 0)),
        "up" => Some((0, 1, 0)),
        "north" => Some((0, 0, -1)),
        "south" => Some((0, 0, 1)),
        "west" => Some((-1, 0, 0)),
        "east" => Some((1, 0, 0)),
        _ => None,
    }
}

fn insertion_slot(
    state: &TickState,
    pos: BlockPos,
    incoming: &ItemStack,
    travel: (i32, i32, i32),
) -> Option<(BlockPos, usize)> {
    let name = kind(state, pos)?;
    size(name)?;
    if crate::shulker::is_block(name) && crate::shulker::is_item(&state.registry.items, incoming) {
        return None;
    }
    if name == "jukebox" && !crate::jukebox::accepts(state, incoming) {
        return None;
    }
    let furnace = match name {
        "furnace" => Some(mc_data::recipes::CookingKind::Furnace),
        "smoker" => Some(mc_data::recipes::CookingKind::Smoker),
        "blast_furnace" => Some(mc_data::recipes::CookingKind::BlastFurnace),
        _ => None,
    };
    let permitted = if let Some(furnace) = furnace {
        if travel == (0, -1, 0) {
            Some(0)
        } else if travel == (0, 1, 0) {
            return None;
        } else if mc_data::recipes::furnace_fuel_ticks(
            &state.registry.tags,
            furnace,
            incoming.item_id,
        )
        .is_some()
            || state
                .registry
                .items
                .name_of(incoming.item_id)
                .is_some_and(|id| id.path() == "bucket")
        {
            Some(1)
        } else {
            return None;
        }
    } else {
        None
    };
    let max = state.registry.item_rules().max_stack(incoming).min(99);
    for cell in cells(state, pos) {
        let Some(record) = state.block_entities.get(&key(cell)) else {
            continue;
        };
        for (index, slot) in record.slots.iter().enumerate() {
            if permitted.is_some_and(|allowed| index != allowed) {
                continue;
            }
            if name == "jukebox" && !slot.is_empty() {
                continue;
            }
            if name == "crafter" {
                let settings = state
                    .crafters
                    .get(&key(cell))
                    .map(|crafter| crafter.settings)
                    .unwrap_or_default();
                if !crate::crafter::allowed(settings, &record.slots, index, max) {
                    continue;
                }
            }
            if name == "brewing_stand" {
                let item = state
                    .registry
                    .items
                    .name_of(incoming.item_id)
                    .map(|id| id.path());
                let bottle = matches!(
                    item,
                    Some("potion" | "splash_potion" | "lingering_potion" | "glass_bottle")
                );
                let allowed = match travel {
                    (0, -1, 0) => {
                        index == 3
                            && mc_data::stations::brewing_ingredient(
                                incoming,
                                &state.registry.items,
                            )
                    }
                    (0, 1, 0) => {
                        (index < 3 && bottle)
                            || (index == 3
                                && mc_data::stations::brewing_ingredient(
                                    incoming,
                                    &state.registry.items,
                                ))
                    }
                    _ => (index < 3 && bottle) || (index == 4 && item == Some("blaze_powder")),
                };
                if !allowed || (index < 3 && !slot.is_empty()) {
                    continue;
                }
            }
            if slot.is_empty() || (can_stack(slot, incoming) && slot.count < max) {
                return Some((cell, index));
            }
        }
    }
    None
}

fn changed(state: &mut TickState, pos: BlockPos) {
    inventory_changed(&mut state.redstone_containers, pos);
    state.world.mark_block_entity_dirty(pos);
    if kind(state, pos).is_some_and(automated) {
        state.redstone_containers.dirty.insert(key(pos));
    }
    for step in [
        (0, 0, 0),
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ] {
        if let Some(cell) = super::offset(pos, step) {
            super::schedule_cell(state, cell, 1, 0);
        }
    }
    // Synchronize every viewer, including viewers whose menu covers both halves.
    let viewers: Vec<_> = state
        .open_containers
        .iter()
        .filter_map(|(id, open)| {
            let open = position(*open);
            cells(state, open).contains(&pos).then_some((*id, open))
        })
        .collect();
    for (id, open) in viewers {
        let slots: Vec<_> = cells(state, open)
            .iter()
            .filter_map(|cell| state.block_entities.get(&key(*cell)))
            .flat_map(|record| record.slots.iter().cloned())
            .collect();
        if let Some(player) = state.players.get_mut(&id) {
            if let Some(menu) = player.open.as_mut() {
                menu.state_id = (menu.state_id + 1) & 0x7fff;
            }
            let connection = player.connection;
            let changes = player.sync_container_view(&slots);
            state.outbox.entry(connection).or_default().extend(changes);
        }
    }
}

fn insert(state: &mut TickState, target: BlockPos, index: usize, moving: &ItemStack, count: i32) {
    let record = state
        .block_entities
        .get_mut(&key(target))
        .expect("planned target custody");
    let slot = &mut record.slots[index];
    if slot.is_empty() {
        *slot = moving.clone();
        slot.count = count;
    } else {
        slot.count += count;
    }
}
/// Crafter output tries native hopper-style insertion before ejecting leftovers.
pub(crate) fn insert_stack(
    state: &mut TickState,
    target: BlockPos,
    travel: (i32, i32, i32),
    moving: &mut ItemStack,
) {
    for cell in cells(state, target) {
        register(state, cell);
    }
    while !moving.is_empty() {
        let Some((destination, slot)) = insertion_slot(state, target, moving, travel) else {
            break;
        };
        insert(state, destination, slot, moving, 1);
        mc_data::inventory_semantics_26_1_2::decrement_stack(moving);
        changed(state, destination);
        persist(state, destination);
    }
}

/// Plan the debit before publishing either side of an automatic transfer.
fn can_debit(state: &TickState, source: BlockPos, index: usize) -> bool {
    if !crate::plugin_inventories::has_reservations_for_block(state, source) {
        return true;
    }
    let mut candidate = state.block_entities[&key(source)].slots.clone();
    decrement_stack(&mut candidate[index]);
    !crate::plugin_inventories::below_block_floors(state, source, &candidate)
}

fn can_trigger(state: &TickState, pos: BlockPos, index: usize, target: BlockPos) -> bool {
    if !crate::plugin_inventories::has_reservations_for_block(state, pos) {
        return true;
    }
    let slots = &state.block_entities[&key(pos)].slots;
    let stack = &slots[index];
    if kind(state, pos) == Some("dispenser")
        && Some(stack.item_id) == item_id(state, "flint_and_steel")
        && state.world.block_at(target.x, target.y, target.z).is_some()
    {
        let damage = stack.damage.unwrap_or(0).saturating_add(1);
        let max = mc_data::item_semantics_26_1_2::max_damage_for_stack(
            &state.registry.item_facts,
            &state.registry.items,
            stack,
        );
        // Reserve v1 holds resource quantities, not the tool's damage value.
        if max.is_none_or(|max| damage < max) {
            return true;
        }
        let mut candidate = slots.clone();
        candidate[index] = ItemStack::EMPTY;
        return !crate::plugin_inventories::below_block_floors(state, pos, &candidate);
    }
    let mut candidate = slots.clone();
    decrement_stack(&mut candidate[index]);
    !crate::plugin_inventories::below_block_floors(state, pos, &candidate)
}

fn transfer_one(
    state: &mut TickState,
    source: BlockPos,
    target: BlockPos,
    travel: (i32, i32, i32),
    pull: bool,
) -> bool {
    for cell in cells(state, source) {
        register(state, cell);
    }
    for cell in cells(state, target) {
        register(state, cell);
    }
    let source_furnace = matches!(
        kind(state, source),
        Some("furnace" | "smoker" | "blast_furnace")
    );
    for cell in cells(state, source) {
        let length = state
            .block_entities
            .get(&key(cell))
            .map_or(0, |record| record.slots.len());
        for index in 0..length {
            if pull && source_furnace && index != 2 && index != 1 {
                continue;
            }
            let moving = state.block_entities[&key(cell)].slots[index].clone();
            if moving.is_empty() {
                continue;
            }
            if pull && kind(state, source) == Some("brewing_stand") {
                let empty_bottle = state
                    .registry
                    .items
                    .name_of(moving.item_id)
                    .is_some_and(|id| id.path() == "glass_bottle");
                if index >= 3 && !(index == 3 && empty_bottle) {
                    continue;
                }
            }
            if pull
                && source_furnace
                && index == 1
                && !state
                    .registry
                    .items
                    .name_of(moving.item_id)
                    .is_some_and(|id| matches!(id.path(), "bucket" | "water_bucket"))
            {
                continue;
            }
            if !can_debit(state, cell, index) {
                continue;
            }
            let Some((destination, slot)) = insertion_slot(state, target, &moving, travel) else {
                continue;
            };
            if destination == cell {
                continue;
            }
            let was_empty = state.block_entities[&key(destination)]
                .slots
                .iter()
                .all(ItemStack::is_empty);
            insert(state, destination, slot, &moving, 1);
            decrement_stack(
                &mut state
                    .block_entities
                    .get_mut(&key(cell))
                    .expect("source custody")
                    .slots[index],
            );
            if was_empty && kind(state, destination) == Some("hopper") {
                state
                    .redstone_containers
                    .set_cooldown(key(destination), 8, state.tick);
            }
            changed(state, cell);
            changed(state, destination);
            return true;
        }
    }
    false
}

/// Vehicle extraction shares bottom-sided furnace and brewing custody rules.
pub(crate) fn take_for_vehicle(
    state: &mut TickState,
    source: BlockPos,
    slots: &mut [ItemStack],
) -> bool {
    let registry = Arc::clone(&state.registry);
    let source_furnace = matches!(
        kind(state, source),
        Some("furnace" | "smoker" | "blast_furnace")
    );
    let brewing = kind(state, source) == Some("brewing_stand");
    for cell in cells(state, source) {
        register(state, cell);
        let length = state
            .block_entities
            .get(&key(cell))
            .map_or(0, |r| r.slots.len());
        for index in 0..length {
            if source_furnace && index != 2 && index != 1 {
                continue;
            }
            let moving = state.block_entities[&key(cell)].slots[index].clone();
            if moving.is_empty() {
                continue;
            }
            let name = registry.items.name_of(moving.item_id).map(|id| id.path());
            if source_furnace && index == 1 && !matches!(name, Some("bucket" | "water_bucket")) {
                continue;
            }
            if brewing && index >= 3 && !(index == 3 && name == Some("glass_bottle")) {
                continue;
            }
            if !can_debit(state, cell, index) {
                continue;
            }
            if crate::vehicles::insert_one(&registry.item_rules(), slots, &moving) {
                decrement_stack(
                    &mut state.block_entities.get_mut(&key(cell)).unwrap().slots[index],
                );
                changed(state, cell);
                return true;
            }
        }
    }
    false
}

fn suction(state: &mut TickState, pos: BlockPos, above: BlockPos) -> bool {
    let Some(above_id) = state.world.block_at(above.x, above.y, above.z) else {
        return false;
    };
    if state
        .registry
        .collision_boxes
        .get(above_id as usize)
        .is_some_and(|boxes| boxes.contains(&mc_physics::BlockCollisionBox::FULL_BLOCK))
    {
        return false;
    }
    let candidates: Vec<_> = state
        .entities
        .slots()
        .filter(|slot| {
            let body = state.entities.body(*slot);
            if !body.alive
                || !matches!(body.kind, EntityKind::Item { .. })
                || state.merge_consumed_items.contains(slot)
            {
                return false;
            }
            let at = state.entities.pos(*slot);
            let x = at[0] - f64::from(pos.x);
            let y = at[1] - f64::from(pos.y);
            let z = at[2] - f64::from(pos.z);
            // Item positions are feet, not centers. Include the bowl floor
            // (11/16) as well as the block above: a cooling hopper must still
            // collect an item after it has fallen into its hollow interior.
            let overlaps = |min: [f64; 3], max: [f64; 3]| {
                x + body.half_width > min[0]
                    && x - body.half_width < max[0]
                    && y + body.height > min[1]
                    && y < max[1]
                    && z + body.half_width > min[2]
                    && z - body.half_width < max[2]
            };
            overlaps([0.0, 1.0, 0.0], [1.0, 2.0, 1.0])
                || overlaps([0.125, 0.6875, 0.125], [0.875, 1.0, 0.875])
        })
        .collect();
    for entity in candidates {
        let Some(item) = state.entities.row(entity).item.as_ref() else {
            continue;
        };
        let mut moving = item.full_stack();
        moving.count = item.count;
        let mut rest = moving.count;
        let max = state.registry.item_rules().max_stack(&moving).min(99);
        let record = state
            .block_entities
            .get_mut(&key(pos))
            .expect("hopper custody");
        for slot in &mut record.slots {
            if rest <= 0 {
                break;
            }
            let room = if slot.is_empty() {
                max
            } else if can_stack(slot, &moving) {
                (max - slot.count).max(0)
            } else {
                0
            };
            let take = room.min(rest);
            if take <= 0 {
                continue;
            }
            if slot.is_empty() {
                *slot = moving.clone();
                slot.count = take;
            } else {
                slot.count += take;
            }
            rest -= take;
        }
        if rest == moving.count {
            continue;
        }
        if rest == 0 {
            // Dense slots must not be removed until commit drains all consumers.
            state.merge_consumed_items.insert(entity);
            let row = state.entities.row_mut(entity);
            crate::entities::set_boxed(
                &mut row.item,
                StackRow {
                    item_id: moving.item_id,
                    count: 0,
                    stack: Some(ItemStack::EMPTY),
                },
            );
        } else {
            moving.count = rest;
            let row = state.entities.row_mut(entity);
            crate::entities::set_boxed(
                &mut row.item,
                StackRow {
                    item_id: moving.item_id,
                    count: rest,
                    stack: Some(moving.clone()),
                },
            );
            Arc::make_mut(&mut row.appearance).item = Some(moving);
            state
                .appearance_changed
                .push(state.entities.body(entity).id);
        }
        changed(state, pos);
        return true;
    }
    false
}

pub(super) fn run(
    state: &mut TickState,
    _writes: &mut Vec<(BlockPos, u32)>,
    _spawned: &mut Vec<EntityId>,
) {
    let mut hoppers = std::mem::take(&mut state.redstone_containers.hoppers);
    let world = &state.world;
    let blocks = &state.registry.blocks;
    let elapsed = state
        .tick
        .saturating_sub(state.redstone_containers.last_tick);
    state.redstone_containers.last_tick = state.tick;
    hoppers.retain(|pos| {
        let block = world
            .block_at(pos.0, pos.1, pos.2)
            .and_then(|id| blocks.by_id(mc_world::BlockStateId(id)));
        block.is_none_or(|block| block.block.id.path() == "hopper")
    });
    state.redstone_containers.cooldowns.retain(|pos, until| {
        if !hoppers.contains(pos) {
            return false;
        }
        let block = world
            .block_at(pos.0, pos.1, pos.2)
            .and_then(|id| blocks.by_id(mc_world::BlockStateId(id)));
        if block.is_none() {
            // Unavailable columns pause automation, including its remaining
            // delay. The next event/checkpoint freezes the shifted deadline.
            *until = until.saturating_add(elapsed);
        }
        true
    });
    for hopper in &hoppers {
        if state.redstone_containers.sleeping.contains(hopper)
            || state.redstone_containers.remaining(*hopper, state.tick) > 0
        {
            continue;
        }
        let pos = position(*hopper);
        if !state
            .world
            .is_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
        {
            continue;
        }
        let Some(block) = state
            .world
            .block_at(pos.x, pos.y, pos.z)
            .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        else {
            continue;
        };
        if super::prop(block, "enabled") == Some("false") || super::power_at(state, pos) > 0 {
            continue;
        }
        let travel = direction(super::prop(block, "facing").unwrap_or("down"));
        let mut moved = false;
        if let Some(travel) = travel
            && let Some(target) = super::offset(pos, travel)
        {
            moved |= transfer_one(state, pos, target, travel, false);
        }
        if let Some(above) = super::offset(pos, (0, 1, 0)) {
            moved |= if kind(state, above).and_then(size).is_some() {
                transfer_one(state, above, pos, (0, -1, 0), true)
            } else {
                crate::vehicles::pull_into_hopper(state, above, pos) || suction(state, pos, above)
            };
        }
        if moved {
            state
                .redstone_containers
                .set_cooldown(key(pos), 8, state.tick);
        } else {
            state.redstone_containers.sleeping.insert(*hopper);
        }
    }
    hoppers.extend(std::mem::take(&mut state.redstone_containers.hoppers));
    state
        .redstone_containers
        .sleeping
        .retain(|pos| hoppers.contains(pos));
    state.redstone_containers.hoppers = hoppers;
    // Menu edits use native inventory post-images; detect their changed slots
    // without serializing unchanged dispenser inventories every tick.
    for (pos, record) in &state.block_entities {
        if kind(state, position(*pos)).is_some_and(automated)
            && state
                .journaled_block_entities
                .get(pos)
                .is_none_or(|image| image.0 != record.slots)
        {
            state.redstone_containers.dirty.insert(*pos);
        }
    }
    let dirty = std::mem::take(&mut state.redstone_containers.dirty);
    for pos in dirty {
        persist(state, position(pos));
    }
}

fn item_id(state: &TickState, name: &str) -> Option<u32> {
    state
        .registry
        .items
        .id_of(&Identifier::parse(format!("minecraft:{name}")).ok()?)
}

/// Scheduled dispenser/dropper edge. A blocked container insertion leaves the
/// source untouched; it must not fall back to ejecting that same item.
pub(super) fn trigger(
    state: &mut TickState,
    pos: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let Some(block) = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
    else {
        return;
    };
    let dropper = block.block.id.path() == "dropper";
    if !dropper && block.block.id.path() != "dispenser" {
        return;
    }
    let Some(travel) = direction(super::prop(block, "facing").unwrap_or("north")) else {
        return;
    };
    let Some(target) = super::offset(pos, travel) else {
        return;
    };
    register(state, pos);
    let Some(record) = state.block_entities.get(&key(pos)) else {
        return;
    };
    let occupied = record
        .slots
        .iter()
        .filter(|stack| !stack.is_empty())
        .count();
    if occupied == 0 {
        return;
    }
    let seed = selection_seed(state.tick, pos);
    let index = record
        .slots
        .iter()
        .enumerate()
        .filter(|(_, stack)| !stack.is_empty())
        .nth((seed % occupied as u64) as usize)
        .map(|(index, _)| index)
        .expect("occupied slot");
    if !can_trigger(state, pos, index, target) {
        return;
    }
    if dropper && kind(state, target).and_then(size).is_some() {
        for cell in cells(state, target) {
            register(state, cell);
        }
        let moving = state.block_entities[&key(pos)].slots[index].clone();
        let Some((destination, slot)) = insertion_slot(state, target, &moving, travel) else {
            return;
        };
        let empty = state.block_entities[&key(destination)]
            .slots
            .iter()
            .all(ItemStack::is_empty);
        insert(state, destination, slot, &moving, 1);
        decrement_stack(
            &mut state
                .block_entities
                .get_mut(&key(pos))
                .expect("dropper custody")
                .slots[index],
        );
        if empty && kind(state, destination) == Some("hopper") {
            state
                .redstone_containers
                .set_cooldown(key(destination), 8, state.tick);
        }
        changed(state, pos);
        changed(state, destination);
        persist(state, pos);
        persist(state, destination);
        return;
    }
    let mut moving = state.block_entities[&key(pos)].slots[index].clone();
    moving.count = 1;
    let direction = [
        f64::from(travel.0),
        f64::from(travel.1),
        f64::from(travel.2),
    ];
    let origin = [
        f64::from(pos.x) + 0.5 + direction[0] * 0.7,
        f64::from(pos.y) + 0.5 + direction[1] * 0.7,
        f64::from(pos.z) + 0.5 + direction[2] * 0.7,
    ];
    if !dropper && crate::projectiles::dispense(state, &moving, origin, direction) {
        decrement_stack(
            &mut state
                .block_entities
                .get_mut(&key(pos))
                .expect("dispenser custody")
                .slots[index],
        );
    } else if !dropper && nonprojectile(state, pos, index, target, writes, spawned) {
        // Action commits inventory itself (buckets/tool wear retain custody).
    } else {
        let id = spawn_item_entity(state, origin, moving);
        if let Some(slot) = state.entities.slot_of(id) {
            state.entities.set_vel(
                slot,
                [
                    direction[0] * 4.0,
                    4.0 + direction[1] * 4.0,
                    direction[2] * 4.0,
                ],
            );
        }
        spawned.push(id);
        decrement_stack(
            &mut state
                .block_entities
                .get_mut(&key(pos))
                .expect("ejector custody")
                .slots[index],
        );
    }
    changed(state, pos);
    persist(state, pos);
}

fn nonprojectile(
    state: &mut TickState,
    pos: BlockPos,
    index: usize,
    target: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) -> bool {
    let held = state.block_entities[&key(pos)].slots[index].clone();
    let registry = Arc::clone(&state.registry);
    let Some(name) = registry.items.name_of(held.item_id).map(|id| id.path()) else {
        return false;
    };
    let Some(current) = state.world.block_at(target.x, target.y, target.z) else {
        return false;
    };
    if name == "tnt" {
        let center = [
            f64::from(target.x) + 0.5,
            f64::from(target.y),
            f64::from(target.z) + 0.5,
        ];
        if crate::tnt::prime_at(state, center, 80).is_some() {
            decrement_stack(
                &mut state
                    .block_entities
                    .get_mut(&key(pos))
                    .expect("TNT custody")
                    .slots[index],
            );
        }
        // Priming owns spawn publication and journaling; a failed prime keeps
        // the dispenser's item rather than turning it into a fake success.
        return true;
    }
    if name == "flint_and_steel" {
        let success = if kind(state, target) == Some("tnt") {
            crate::tnt::prime_block(state, target, 80).is_some()
        } else {
            let ignited = crate::fire::ignite_block(state, target);
            if ignited && let Some(new) = state.world.block_at(target.x, target.y, target.z) {
                writes.retain(|(written, _)| *written != target);
                writes.push((target, new));
            }
            ignited
        };
        if success {
            let max = mc_data::item_semantics_26_1_2::max_damage_for_stack(
                &state.registry.item_facts,
                &state.registry.items,
                &held,
            );
            let slot = &mut state
                .block_entities
                .get_mut(&key(pos))
                .expect("ignition custody")
                .slots[index];
            let damage = slot.damage.unwrap_or(0).saturating_add(1);
            if max.is_some_and(|max| damage >= max) {
                *slot = ItemStack::EMPTY;
            } else {
                slot.damage = Some(damage);
            }
        }
        return true;
    }
    if name == "bone_meal" {
        let seed = selection_seed(state.tick, target);
        let success = if let Some(grown) = crate::farming::bone_meal_state(state, target, seed) {
            super::set(state, writes, target, grown);
            true
        } else if let Some(plan) = crate::plants::bonemeal(state, target, seed)
            && plan
                .expects
                .iter()
                .all(|(pos, expected)| state.world.block_at(pos.x, pos.y, pos.z) == Some(*expected))
        {
            // Tree/grass/stem growth is one native-owner transaction: validate
            // the complete read fence before applying any of its output cells.
            for (pos, new) in plan.writes {
                super::set(state, writes, pos, new);
            }
            true
        } else {
            false
        };
        if success {
            decrement_stack(
                &mut state
                    .block_entities
                    .get_mut(&key(pos))
                    .expect("bone-meal custody")
                    .slots[index],
            );
        }
        // Optional dispense behavior retains bone meal when it cannot grow.
        return true;
    }
    let bucket = match name {
        "water_bucket" | "lava_bucket" => {
            if current != state.registry.states.air
                && crate::fluids::fluid_at(&state.registry.states, current).is_none()
            {
                return false;
            }
            let new = if name == "water_bucket" {
                state.registry.states.water_levels[0]
            } else {
                state.registry.states.lava_levels[0]
            };
            item_id(state, "bucket").map(|remainder| (new, remainder))
        }
        "bucket" => match crate::fluids::fluid_at(&state.registry.states, current) {
            Some((fluid, 0)) => item_id(
                state,
                if fluid == crate::fluids::Fluid::Water {
                    "water_bucket"
                } else {
                    "lava_bucket"
                },
            )
            .map(|remainder| (state.registry.states.air, remainder)),
            _ => None,
        },
        _ => None,
    };
    if let Some((new, remainder)) = bucket {
        super::set(state, writes, target, new);
        crate::fluids::schedule(state, &[(target, new)]);
        let slot = &mut state
            .block_entities
            .get_mut(&key(pos))
            .expect("bucket custody")
            .slots[index];
        decrement_stack(slot);
        let replacement = ItemStack::new(remainder, 1);
        if slot.is_empty() {
            *slot = replacement;
        } else if let Some((destination, slot)) =
            insertion_slot(state, pos, &replacement, (0, 0, 0))
        {
            insert(state, destination, slot, &replacement, 1);
        } else {
            spawned.push(spawn_item_entity(
                state,
                [
                    f64::from(target.x) + 0.5,
                    f64::from(target.y) + 0.5,
                    f64::from(target.z) + 0.5,
                ],
                replacement,
            ));
        }
        return true;
    }
    false
}

fn selection_seed(tick: u64, pos: BlockPos) -> u64 {
    let mut seed = tick
        ^ (pos.x as u64).rotate_left(17)
        ^ (pos.y as u64).rotate_left(31)
        ^ (pos.z as u64).rotate_left(47);
    seed = (seed ^ (seed >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    seed = (seed ^ (seed >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    seed ^ (seed >> 31)
}

#[cfg(test)]
#[path = "containers_tests.rs"]
mod tests;
