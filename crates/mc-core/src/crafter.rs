//! Merge-owned nine-slot crafter; native NBT post-images and scheduled rising edges.
use crate::boundary::{BlockPos, EntityId, PlayerChange};
use crate::inventory::MenuKind;
use crate::tick::{BlockEntityState, TickState, spawn_item_entity};
use mc_data::{ItemStack, storage_blocks::CrafterSettings};
use mc_nbt::Tag;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub(crate) struct Crafter {
    pub settings: CrafterSettings,
    dirty: bool,
}
pub(crate) type Crafters = BTreeMap<(i32, i32, i32), Crafter>;
fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}
fn position(key: (i32, i32, i32)) -> BlockPos {
    BlockPos {
        x: key.0,
        y: key.1,
        z: key.2,
    }
}
fn is_crafter(registry: &crate::registry::CoreRegistry, id: u32) -> bool {
    registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .is_some_and(|b| b.block.id.path() == "crafter")
}
pub(crate) fn hydrate_chunk(
    crafters: &mut Crafters,
    inventories: &mut crate::redstone::custody::BlockEntities,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) {
    for (pos, bytes) in &chunk.block_entities {
        let key = key(*pos);
        if crafters.contains_key(&key)
            || chunk
                .get_block(
                    pos.x.rem_euclid(16) as u8,
                    pos.y,
                    pos.z.rem_euclid(16) as u8,
                )
                .is_none_or(|id| !is_crafter(registry, id.0))
        {
            continue;
        }
        let Tag::Compound(fields) =
            mc_nbt::read_network(&mut bytes.as_slice()).expect("validated crafter NBT")
        else {
            continue;
        };
        let (settings, slots) = mc_data::storage_blocks::decode_crafter(&fields, &registry.items)
            .expect("validated crafter fields");
        crafters.insert(
            key,
            Crafter {
                settings,
                dirty: false,
            },
        );
        inventories.hydrate(key, || BlockEntityState {
            slots,
            furnace: None,
        });
    }
}
pub(crate) fn register(state: &mut TickState, pos: BlockPos) {
    let key = key(pos);
    state.crafters.entry(key).or_default();
    state
        .block_entities
        .entry(key)
        .or_insert_with(|| BlockEntityState {
            slots: vec![ItemStack::EMPTY; 9],
            furnace: None,
        });
}
pub(crate) fn allowed(
    settings: CrafterSettings,
    slots: &[ItemStack],
    index: usize,
    max: i32,
) -> bool {
    if settings.disabled_slots & (1 << index) != 0 {
        return false;
    }
    let slot = &slots[index];
    if slot.count >= max {
        return false;
    }
    slot.is_empty()
        || !slots.iter().enumerate().skip(index + 1).any(|(i, other)| {
            settings.disabled_slots & (1 << i) == 0
                && (other.is_empty()
                    || (other.count < slot.count
                        && mc_data::inventory_semantics_26_1_2::can_stack(other, slot)))
        })
}
pub(crate) fn signal(state: &TickState, pos: BlockPos) -> u8 {
    let settings = state
        .crafters
        .get(&key(pos))
        .map(|c| c.settings)
        .unwrap_or_default();
    (0..9)
        .filter(|i| {
            settings.disabled_slots & (1 << i) != 0
                || state
                    .block_entities
                    .get(&key(pos))
                    .is_some_and(|r| !r.slots[*i].is_empty())
        })
        .count() as u8
}
fn fields(state: &TickState, pos: BlockPos) -> Vec<(String, Tag)> {
    state
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
                    Tag::String(
                        mc_data::block_entity_kinds::BlockEntityKind::Crafter
                            .vanilla_id()
                            .into(),
                    ),
                ),
                ("x".into(), Tag::Int(pos.x)),
                ("y".into(), Tag::Int(pos.y)),
                ("z".into(), Tag::Int(pos.z)),
            ]
        })
}
fn persist(state: &mut TickState, pos: BlockPos) {
    let Some(crafter) = state.crafters.get(&key(pos)) else {
        return;
    };
    let Some(record) = state.block_entities.get(&key(pos)) else {
        return;
    };
    let mut fields = fields(state, pos);
    mc_data::storage_blocks::encode_items(&mut fields, &record.slots, &state.registry.items);
    mc_data::storage_blocks::encode_crafter_fields(&mut fields, crafter.settings);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("crafter NBT encodes");
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
}
fn property(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    pos: BlockPos,
    name: &str,
    value: &str,
) {
    let next = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .and_then(|block| {
            let mut properties = block.properties.clone();
            let (_, old) = properties.iter_mut().find(|(key, _)| key == name)?;
            *old = value.into();
            state
                .registry
                .blocks
                .by_name_and_props(&block.block.id, &properties)
        });
    if let Some(next) = next {
        crate::redstone::set(state, writes, pos, next.0);
    }
}
pub(crate) fn scheduled(
    state: &mut TickState,
    pos: BlockPos,
    priority: i32,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    register(state, pos);
    if priority == 8 {
        craft(state, pos, writes, spawned);
        return;
    }
    let powered = crate::redstone::power_at(state, pos) > 0;
    let crafter = state
        .crafters
        .get_mut(&key(pos))
        .expect("crafter registered");
    let rising = powered && !crafter.settings.triggered;
    if crafter.settings.triggered != powered {
        crafter.settings.triggered = powered;
        crafter.dirty = true;
        property(
            state,
            writes,
            pos,
            "triggered",
            if powered { "true" } else { "false" },
        );
    }
    if rising {
        crate::redstone::schedule_cell(state, pos, 4, 8);
    }
}
fn input(state: &TickState, pos: BlockPos) -> [ItemStack; 9] {
    let settings = state.crafters[&key(pos)].settings;
    std::array::from_fn(|i| {
        if settings.disabled_slots & (1 << i) != 0 {
            ItemStack::EMPTY
        } else {
            state.block_entities[&key(pos)].slots[i].clone()
        }
    })
}
fn result(state: &TickState, pos: BlockPos) -> ItemStack {
    mc_data::recipes::crafting_result_from_input(
        &state.registry.items,
        &state.registry.item_facts,
        &state.registry.tags,
        &state.registry.recipes,
        &input(state, pos),
    )
}
pub(crate) fn direction(orientation: &str) -> (i32, i32, i32) {
    match orientation.split('_').next().unwrap_or("north") {
        "down" => (0, -1, 0),
        "up" => (0, 1, 0),
        "south" => (0, 0, 1),
        "west" => (-1, 0, 0),
        "east" => (1, 0, 0),
        _ => (0, 0, -1),
    }
}
fn eject(
    state: &mut TickState,
    pos: BlockPos,
    travel: (i32, i32, i32),
    mut stack: ItemStack,
    spawned: &mut Vec<EntityId>,
) -> bool {
    let target = BlockPos {
        x: pos.x + travel.0,
        y: pos.y + travel.1,
        z: pos.z + travel.2,
    };
    crate::redstone::containers::insert_stack(state, target, travel, &mut stack);
    if !stack.is_empty() {
        let id = spawn_item_entity(
            state,
            [
                f64::from(pos.x) + 0.5 + f64::from(travel.0) * 0.7,
                f64::from(pos.y) + 0.5 + f64::from(travel.1) * 0.7,
                f64::from(pos.z) + 0.5 + f64::from(travel.2) * 0.7,
            ],
            stack,
        );
        if let Some(slot) = state.entities.slot_of(id) {
            state.entities.set_vel(
                slot,
                [
                    f64::from(travel.0) * 0.2,
                    0.2 + f64::from(travel.1) * 0.2,
                    f64::from(travel.2) * 0.2,
                ],
            );
        }
        spawned.push(id);
        return true;
    }
    false
}
fn event(state: &mut TickState, pos: BlockPos, event: i32, data: i32) {
    let viewers: Vec<_> = state
        .players
        .values()
        .map(|player| player.connection)
        .collect();
    for connection in viewers {
        state.push_player(
            connection,
            PlayerChange::ProjectileLevelEvent {
                position: pos,
                event,
                data,
            },
        );
    }
}
fn craft(
    state: &mut TickState,
    pos: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let ingredients = input(state, pos);
    let repair = mc_data::recipes::repair_item_crafting_result(
        &state.registry.items,
        &state.registry.item_facts,
        &ingredients,
    )
    .or_else(|| mc_data::recipes::armor_dye_crafting_result(&state.registry.items, &ingredients));
    let recipe = repair
        .is_none()
        .then(|| {
            state.registry.recipes.iter().find(|recipe| {
                mc_data::recipes::crafting_recipe_matches(
                    &state.registry.items,
                    &state.registry.item_facts,
                    &state.registry.tags,
                    &ingredients,
                    recipe,
                )
            })
        })
        .flatten();
    let output = repair.unwrap_or_else(|| {
        recipe
            .and_then(|recipe| {
                recipe
                    .result
                    .to_stack(&state.registry.items, &state.registry.item_facts)
            })
            .unwrap_or(ItemStack::EMPTY)
    });
    let evidence = recipe
        .filter(|_| !state.registry.advancements.definitions.is_empty())
        .map(|recipe| serde_json::json!({
            "recipe_id": recipe.id.as_str(),
            "ingredients": ingredients.iter().filter(|stack| !stack.is_empty())
                .map(|stack| crate::advancements::item_snapshot(state, stack)).collect::<Vec<_>>(),
        }));
    if output.is_empty() {
        event(state, pos, 1050, 0);
        return;
    }
    let settings = state.crafters[&key(pos)].settings;
    if crate::plugin_inventories::has_reservations_for_block(state, pos) {
        let mut candidate = state.block_entities[&key(pos)].slots.clone();
        for (i, stack) in candidate.iter_mut().enumerate() {
            if settings.disabled_slots & (1 << i) == 0 && !stack.is_empty() {
                mc_data::inventory_semantics_26_1_2::decrement_stack(stack);
            }
        }
        if crate::plugin_inventories::below_block_floors(state, pos, &candidate) {
            event(state, pos, 1050, 0);
            return;
        }
    }
    let orientation = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .and_then(|b| b.properties.iter().find(|(k, _)| k == "orientation"))
        .map(|(_, value)| value.as_str())
        .unwrap_or("north_up");
    let travel = direction(orientation);
    let mut remainders = Vec::new();
    for (i, stack) in state
        .block_entities
        .get_mut(&key(pos))
        .expect("crafter storage")
        .slots
        .iter_mut()
        .enumerate()
    {
        if settings.disabled_slots & (1 << i) != 0 || stack.is_empty() {
            continue;
        }
        let remainder = state
            .registry
            .items
            .name_of(stack.item_id)
            .and_then(|id| match id.path() {
                "water_bucket" | "lava_bucket" | "milk_bucket" => Some("minecraft:bucket"),
                "dragon_breath" | "honey_bottle" => Some("minecraft:glass_bottle"),
                _ => None,
            })
            .and_then(|id| {
                state
                    .registry
                    .items
                    .id_of(&mc_data::Identifier::parse(id).expect("vanilla remainder"))
            });
        if let Some(id) = remainder {
            remainders.push(ItemStack::new(id, 1));
        }
        mc_data::inventory_semantics_26_1_2::decrement_stack(stack);
    }
    let crafter = state.crafters.get_mut(&key(pos)).expect("crafter");
    crafter.settings.crafting_ticks_remaining = 6;
    crafter.dirty = true;
    property(state, writes, pos, "crafting", "true");
    let mut ejected = eject(state, pos, travel, output, spawned);
    for stack in remainders {
        ejected |= eject(state, pos, travel, stack, spawned);
    }
    if ejected && let Some(evidence) = evidence {
        // 26.1.2 CrafterBlock.dispenseItem uses a 17-wide AABB, not a sphere.
        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        let players: Vec<_> = state
            .players
            .keys()
            .copied()
            .filter(|entity| {
                let Some(slot) = state.entities.slot_of(*entity) else {
                    return false;
                };
                let at = state.entities.pos(slot);
                let body = state.entities.body(slot);
                at[0] + body.half_width > centre[0] - 8.5
                    && at[0] - body.half_width < centre[0] + 8.5
                    && at[1] + body.height > centre[1] - 8.5
                    && at[1] < centre[1] + 8.5
                    && at[2] + body.half_width > centre[2] - 8.5
                    && at[2] - body.half_width < centre[2] + 8.5
            })
            .collect();
        for player in players {
            crate::advancements::trigger(
                state,
                player,
                "minecraft:crafter_recipe_crafted",
                evidence.clone(),
            );
        }
    }
    state.world.mark_block_entity_dirty(pos);
    event(state, pos, 1049, 0);
    event(
        state,
        pos,
        2010,
        match travel {
            (0, -1, 0) => 0,
            (0, 1, 0) => 1,
            (0, 0, -1) => 2,
            (0, 0, 1) => 3,
            (-1, 0, 0) => 4,
            _ => 5,
        },
    );
}
pub(crate) fn opened(state: &mut TickState, entity: EntityId, pos: BlockPos) {
    register(state, pos);
    sync(state, entity, pos);
}
fn sync(state: &mut TickState, entity: EntityId, pos: BlockPos) {
    let settings = state.crafters[&key(pos)].settings;
    let output = result(state, pos);
    let slots = &state.block_entities[&key(pos)].slots;
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    let Some(menu) = player.open.as_mut() else {
        return;
    };
    if menu.kind != MenuKind::Crafter {
        return;
    }
    if menu.result != output || menu.crafter_disabled != settings.disabled_slots {
        menu.state_id = menu.state_id.wrapping_add(1);
    }
    menu.result = output;
    menu.crafter_disabled = settings.disabled_slots;
    let window = menu.window;
    let connection = player.connection;
    let changes = player.sync_container_view(slots);
    state.outbox.entry(connection).or_default().extend(changes);
    for property in 0..10 {
        let value = if property == 9 {
            i16::from(settings.triggered)
        } else {
            i16::from(settings.disabled_slots & (1 << property) != 0)
        };
        state.push_player(
            connection,
            PlayerChange::MenuProperty {
                window,
                property,
                value,
            },
        );
    }
}
pub(crate) fn slot_state(
    state: &mut TickState,
    entity: EntityId,
    window: u8,
    index: i32,
    enabled: bool,
) {
    let Some(player) = state.players.get(&entity) else {
        return;
    };
    if player.spectator() {
        return;
    }
    if player
        .open
        .as_ref()
        .is_none_or(|menu| menu.window != window || menu.kind != MenuKind::Crafter)
    {
        return;
    }
    let Some(&key) = state.open_containers.get(&entity) else {
        return;
    };
    let Ok(index) = usize::try_from(index) else {
        return;
    };
    if index >= 9 {
        return;
    }
    let pos = position(key);
    if state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .is_none_or(|id| !is_crafter(&state.registry, id))
    {
        return;
    }
    let Some(slot) = state
        .block_entities
        .get(&key)
        .and_then(|record| record.slots.get(index))
    else {
        return;
    };
    if !slot.is_empty() {
        return;
    }
    let Some(crafter) = state.crafters.get_mut(&key) else {
        return;
    };
    if enabled {
        crafter.settings.disabled_slots &= !(1 << index);
    } else {
        crafter.settings.disabled_slots |= 1 << index;
    }
    crafter.dirty = true;
    state.world.mark_block_entity_dirty(pos);
    state.block_entities.mark_dirty(key);
    for step in [(0, 0, 0), (1, 0, 0), (-1, 0, 0), (0, 0, 1), (0, 0, -1)] {
        crate::redstone::schedule_cell(
            state,
            BlockPos {
                x: pos.x + step.0,
                y: pos.y + step.1,
                z: pos.z + step.2,
            },
            1,
            0,
        );
    }
}
pub(crate) fn advance(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>) {
    let active: Vec<_> = state
        .crafters
        .iter()
        .filter_map(|(key, crafter)| {
            (crafter.settings.crafting_ticks_remaining > 0
                && state
                    .world
                    .is_active(key.0.div_euclid(16), key.2.div_euclid(16)))
            .then_some(*key)
        })
        .collect();
    for key in active {
        let crafter = state.crafters.get_mut(&key).expect("active crafter");
        crafter.settings.crafting_ticks_remaining -= 1;
        crafter.dirty = true;
        if crafter.settings.crafting_ticks_remaining == 0 {
            property(state, writes, position(key), "crafting", "false");
        }
    }
}
pub(crate) fn run(state: &mut TickState) {
    let keys: Vec<_> = state.crafters.keys().copied().collect();
    for key in keys {
        let pos = position(key);
        if !state
            .world
            .is_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
        {
            continue;
        }
        if state
            .world
            .block_at(pos.x, pos.y, pos.z)
            .is_none_or(|id| !is_crafter(&state.registry, id))
        {
            state.crafters.remove(&key);
            state.world.set_opaque_block_entity(pos, None);
            state.opaque_changes.push((pos, None));
            continue;
        }
        let crafter = state.crafters.get_mut(&key).expect("crafter");
        let mut changed = crafter.dirty;
        crafter.dirty = false;
        changed |= state.block_entities.get(&key).is_some_and(|record| {
            state
                .journaled_block_entities
                .get(&key)
                .is_none_or(|image| image.0 != record.slots)
        });
        if changed {
            persist(state, pos);
            let viewers: Vec<_> = state
                .open_containers
                .iter()
                .filter_map(|(id, open)| (*open == key).then_some(*id))
                .collect();
            for id in viewers {
                sync(state, id, pos);
            }
        }
    }
}
pub(crate) fn checkpoint(state: &mut TickState) {
    let keys: Vec<_> = state.crafters.keys().copied().collect();
    for key in keys {
        persist(state, position(key));
    }
}

#[cfg(test)]
#[path = "crafter_tests.rs"]
pub(crate) mod tests;
