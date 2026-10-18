//! Brewing stands: shared block-owned slots, loaded-tick batches and vanilla NBT.

use std::collections::BTreeMap;
use std::sync::Arc;

use mc_data::inventory_semantics_26_1_2::decrement_stack;
use mc_data::item_nbt::{field, stack_from_fields, stack_to_fields};
use mc_data::{Identifier, ItemStack, items::ItemRegistry};
use mc_nbt::{ListTag, Tag};

use crate::boundary::{BlockPos, ConnectionId, EntityId, PlayerChange};
use crate::registry::CoreRegistry;
use crate::tick::{BlockEntityState, TickState, spawn_item_entity};

pub(crate) type BrewingStands = BTreeMap<(i32, i32, i32), BrewingState>;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct BrewingState {
    pub(crate) brew_time: i16,
    pub(crate) fuel: i16,
    ingredient: Option<u32>,
    /// Previous visible timer when a menu edit cancels within this input batch.
    cancelled_time: Option<i16>,
}

impl BrewingState {
    fn properties(&self) -> [i16; 2] {
        [self.brew_time, self.fuel]
    }

    /// The ingredient fence runs before the final decrement. Cancellation spends
    /// the already-debited batch fuel but never consumes ingredient or bottles.
    fn tick(&mut self, slots: &mut [ItemStack], items: &ItemRegistry) -> Option<ItemStack> {
        if self.fuel <= 0 && is_named(items, &slots[4], "blaze_powder") {
            decrement_stack(&mut slots[4]);
            self.fuel = 20;
        }
        let brewable = slots[..3]
            .iter()
            .any(|bottle| mc_data::stations::brewing_result(bottle, &slots[3], items).is_some());
        if self.brew_time > 0 {
            if !brewable || self.ingredient != Some(slots[3].item_id) {
                self.brew_time = 0;
                self.ingredient = None;
            } else {
                self.brew_time -= 1;
                if self.brew_time == 0 {
                    for i in 0..3 {
                        if let Some(result) =
                            mc_data::stations::brewing_result(&slots[i], &slots[3], items)
                        {
                            slots[i] = result;
                        }
                    }
                    let dragon_breath = is_named(items, &slots[3], "dragon_breath");
                    decrement_stack(&mut slots[3]);
                    self.ingredient = None;
                    if dragon_breath {
                        let bottle = items
                            .id_of(
                                &Identifier::parse("minecraft:glass_bottle").expect("vanilla id"),
                            )
                            .map(|id| ItemStack::new(id, 1));
                        if let Some(bottle) = bottle {
                            if slots[3].is_empty() {
                                slots[3] = bottle;
                            } else {
                                return Some(bottle);
                            }
                        }
                    }
                }
            }
        } else if brewable && self.fuel > 0 {
            self.fuel -= 1;
            self.brew_time = 400;
            self.ingredient = Some(slots[3].item_id);
        }
        None
    }

    fn encode(&self, pos: BlockPos, slots: &[ItemStack], items: &ItemRegistry) -> Vec<u8> {
        let elements = slots
            .iter()
            .enumerate()
            .filter(|(_, stack)| !stack.is_empty())
            .map(|(slot, stack)| {
                let mut fields = stack_to_fields(items, stack).expect("registered brewing item");
                fields.push(("Slot".into(), Tag::Byte(slot as i8)));
                Tag::Compound(fields)
            })
            .collect();
        let mut fields = vec![
            ("id".into(), Tag::String("minecraft:brewing_stand".into())),
            ("x".into(), Tag::Int(pos.x)),
            ("y".into(), Tag::Int(pos.y)),
            ("z".into(), Tag::Int(pos.z)),
            ("BrewTime".into(), Tag::Short(self.brew_time)),
            ("Fuel".into(), Tag::Byte(self.fuel as i8)),
            (
                "Items".into(),
                Tag::List(ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements,
                }),
            ),
        ];
        // Vanilla reloads its ingredient from slot 3. The WAL also fences its
        // identity explicitly, so a post-image cannot restart a changed batch.
        if let Some(name) = self.ingredient.and_then(|id| items.name_of(id)) {
            fields.push((
                "solaris:brewing_ingredient".into(),
                Tag::String(name.as_str().into()),
            ));
        }
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("brewing NBT encodes");
        bytes
    }

    fn decode(mut bytes: &[u8], items: &ItemRegistry) -> Option<(Self, Vec<ItemStack>)> {
        let Tag::Compound(fields) = mc_nbt::read_network(&mut bytes).ok()? else {
            return None;
        };
        let mut slots = vec![ItemStack::EMPTY; 5];
        if let Some(Tag::List(list)) = field(&fields, "Items") {
            for entry in &list.elements {
                let Tag::Compound(fields) = entry else {
                    continue;
                };
                let slot = match field(fields, "Slot") {
                    Some(Tag::Byte(slot)) => i32::from(*slot),
                    Some(Tag::Int(slot)) => *slot,
                    _ => continue,
                };
                if (0..5).contains(&slot) {
                    slots[slot as usize] = stack_from_fields(items, fields).ok()??;
                }
            }
        }
        let brew_time = match field(&fields, "BrewTime") {
            Some(Tag::Short(value)) => (*value).clamp(0, 400),
            _ => 0,
        };
        let fuel = match field(&fields, "Fuel") {
            Some(Tag::Byte(value)) => i16::from(*value).clamp(0, 20),
            _ => 0,
        };
        let ingredient = if brew_time == 0 {
            None
        } else {
            match field(&fields, "solaris:brewing_ingredient") {
                Some(Tag::String(name)) => Identifier::parse(name)
                    .ok()
                    .and_then(|name| items.id_of(&name)),
                _ => (!slots[3].is_empty()).then_some(slots[3].item_id),
            }
        };
        Some((
            Self {
                brew_time,
                fuel,
                ingredient,
                cancelled_time: None,
            },
            slots,
        ))
    }
}

fn is_named(items: &ItemRegistry, stack: &ItemStack, name: &str) -> bool {
    !stack.is_empty()
        && items
            .name_of(stack.item_id)
            .is_some_and(|id| id.path() == name)
}

fn is_stand(registry: &CoreRegistry, id: u32) -> bool {
    registry
        .blocks
        .by_id(mc_world::BlockStateId(id))
        .is_some_and(|block| block.block.id.as_str() == "minecraft:brewing_stand")
}

/// Timers and slots hydrate independently: player/automation WAL slots are
/// authoritative even if the native Anvil timer was not yet loaded.
pub(crate) fn hydrate_chunk(
    stands: &mut BrewingStands,
    inventories: &mut crate::redstone::custody::BlockEntities,
    registry: &CoreRegistry,
    chunk: &mc_world::Chunk,
) {
    for (pos, bytes) in &chunk.block_entities {
        let key = (pos.x, pos.y, pos.z);
        if stands.contains_key(&key) {
            continue;
        }
        if !chunk
            .get_block(
                pos.x.rem_euclid(16) as u8,
                pos.y,
                pos.z.rem_euclid(16) as u8,
            )
            .is_some_and(|id| is_stand(registry, id.0))
        {
            continue;
        }
        if let Some((stand, slots)) = BrewingState::decode(bytes, &registry.items) {
            stands.insert(key, stand);
            inventories.hydrate(key, || BlockEntityState {
                slots,
                furnace: None,
            });
        }
    }
    for (pos, stand) in &chunk.brewing_stands {
        let key = (pos.x, pos.y, pos.z);
        if !chunk
            .get_block(
                pos.x.rem_euclid(16) as u8,
                pos.y,
                pos.z.rem_euclid(16) as u8,
            )
            .is_some_and(|id| is_stand(registry, id.0))
        {
            continue;
        }
        stands.entry(key).or_insert_with(|| BrewingState {
            brew_time: stand.brew_time,
            fuel: stand.fuel,
            ingredient: stand.ingredient,
            cancelled_time: None,
        });
        inventories.hydrate(key, || BlockEntityState {
            slots: stand
                .slots
                .iter()
                .map(|slot| ItemStack {
                    count: slot.count,
                    item_id: slot.item_id,
                    damage: slot.damage,
                    enchantments: slot.enchantments.clone(),
                    custom_name: slot.custom_name.as_deref().map(Into::into),
                    item_model: slot.item_model.clone(),
                    stew_effects: slot.stew_effects.clone(),
                    components: slot.components.clone(),
                })
                .collect(),
            furnace: None,
        });
    }
}

/// Fence every individual shared-menu write, not merely its end-of-tick image:
/// removing and replacing an ingredient in one input batch starts a new brew.
pub(crate) fn slots_replaced(
    stands: &mut BrewingStands,
    key: (i32, i32, i32),
    slots: &[ItemStack],
) {
    let Some(stand) = stands.get_mut(&key) else {
        return;
    };
    if stand.ingredient.is_some_and(|id| {
        slots
            .get(3)
            .is_none_or(|ingredient| ingredient.is_empty() || ingredient.item_id != id)
    }) {
        stand.cancelled_time.get_or_insert(stand.brew_time);
        stand.brew_time = 0;
        stand.ingredient = None;
    }
}

pub(crate) fn opened(
    state: &mut TickState,
    entity: EntityId,
    connection: ConnectionId,
    pos: BlockPos,
) {
    let stand = state.brewing.entry((pos.x, pos.y, pos.z)).or_default();
    let properties = stand.properties();
    let Some(window) = state
        .players
        .get(&entity)
        .and_then(|p| p.open.as_ref())
        .map(|m| m.window)
    else {
        return;
    };
    for (property, value) in properties.into_iter().enumerate() {
        state.push_player(
            connection,
            PlayerChange::MenuProperty {
                window,
                property: property as i16,
                value,
            },
        );
    }
}

/// Runs after automation, before folding blocks. Progress is independent of
/// viewers and pauses outside resident chunks. Slots have exactly one owner.
pub(crate) fn run(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    drops: &mut Vec<EntityId>,
) {
    let registry = Arc::clone(&state.registry);
    let mut stands = std::mem::take(&mut state.brewing);
    for (key, _) in &state.block_entities {
        if state
            .world
            .block_at(key.0, key.1, key.2)
            .is_some_and(|id| is_stand(&registry, id))
        {
            stands.entry(*key).or_default();
        }
    }
    stands.retain(|key, stand| {
        let pos = BlockPos {
            x: key.0,
            y: key.1,
            z: key.2,
        };
        if !state
            .world
            .is_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
        {
            return true;
        }
        if !state
            .world
            .block_at(pos.x, pos.y, pos.z)
            .is_some_and(|id| is_stand(&registry, id))
        {
            // Generic merged container removal owns drops, tombstones and closure.
            state.world.set_opaque_block_entity(pos, None);
            state.opaque_changes.push((pos, None));
            return false;
        }
        {
            let Some(record) = state.block_entities.get_mut(key) else {
                return true;
            };
            if record.slots.len() < 5 {
                record.slots.resize(5, ItemStack::EMPTY);
            }
        }
        let before = [
            stand.cancelled_time.take().unwrap_or(stand.brew_time),
            stand.fuel,
        ];
        let ingredient_before = stand.ingredient;
        let fuel_count = state.block_entities[key].slots[4].count;
        let completing = stand.brew_time == 1;
        let changes_inventory = completing
            || (stand.fuel <= 0
                && is_named(
                    &registry.items,
                    &state.block_entities[key].slots[4],
                    "blaze_powder",
                ));
        let remainder = if changes_inventory
            && crate::plugin_inventories::has_reservations_for_block(state, pos)
        {
            let mut candidate = state.block_entities[key].slots.clone();
            let mut next = stand.clone();
            let remainder = next.tick(&mut candidate, &registry.items);
            if crate::plugin_inventories::below_block_floors(state, pos, &candidate) {
                None
            } else {
                *stand = next;
                state.block_entities.get_mut(key).unwrap().slots = candidate;
                remainder
            }
        } else {
            stand.tick(
                &mut state.block_entities.get_mut(key).unwrap().slots,
                &registry.items,
            )
        };
        let record = &state.block_entities[key];
        let slots_changed = fuel_count != record.slots[4].count
            || (completing && stand.ingredient.is_none())
            || state
                .journaled_block_entities
                .get(key)
                .is_none_or(|image| image.0 != record.slots);
        let after = stand.properties();
        if slots_changed || before != after || ingredient_before != stand.ingredient {
            let bytes = stand.encode(pos, &record.slots, &registry.items);
            let has_opaque = state
                .world
                .chunk(crate::boundary::ChunkPos {
                    x: pos.x.div_euclid(16),
                    z: pos.z.div_euclid(16),
                })
                .is_some_and(|chunk| chunk.block_entities.contains_key(&pos));
            if slots_changed || !has_opaque {
                state
                    .world
                    .set_opaque_block_entity(pos, Some(bytes.clone()));
            }
            state.opaque_changes.push((pos, Some(bytes)));
            for (viewer, open) in &state.open_containers {
                if open != key {
                    continue;
                }
                let Some(player) = state.players.get_mut(viewer) else {
                    continue;
                };
                let Some(menu) = player.open.as_mut() else {
                    continue;
                };
                if menu.kind != crate::inventory::MenuKind::BrewingStand {
                    continue;
                }
                let window = menu.window;
                let connection = player.connection;
                let queue = state.outbox.entry(connection).or_default();
                if slots_changed {
                    menu.state_id = (menu.state_id + 1) & 0x7fff;
                    queue.extend(player.sync_container_view(&record.slots));
                }
                for (property, (old, value)) in before.into_iter().zip(after).enumerate() {
                    if old != value {
                        queue.push(PlayerChange::MenuProperty {
                            window,
                            property: property as i16,
                            value,
                        });
                    }
                }
            }
        }
        if let Some(remainder) = remainder {
            drops.push(spawn_item_entity(
                state,
                [
                    f64::from(pos.x) + 0.5,
                    f64::from(pos.y) + 0.5,
                    f64::from(pos.z) + 0.5,
                ],
                remainder,
            ));
        }
        let record = &state.block_entities[key];
        let current = state
            .world
            .block_at(pos.x, pos.y, pos.z)
            .expect("resident stand");
        let block = registry
            .blocks
            .by_id(mc_world::BlockStateId(current))
            .expect("registered stand");
        const BOTTLE_PROPERTIES: [&str; 3] = ["has_bottle_0", "has_bottle_1", "has_bottle_2"];
        let occupied = std::array::from_fn::<_, 3, _>(|i| {
            if record.slots[i].is_empty() {
                "false"
            } else {
                "true"
            }
        });
        if BOTTLE_PROPERTIES
            .into_iter()
            .zip(occupied)
            .any(|(name, value)| {
                block
                    .properties
                    .iter()
                    .any(|(key, old)| key == name && old != value)
            })
        {
            let mut properties = block.properties.clone();
            for (name, next) in BOTTLE_PROPERTIES.into_iter().zip(occupied) {
                if let Some((_, value)) = properties.iter_mut().find(|(key, _)| key == name) {
                    *value = next.into();
                }
            }
            if let Some(next) = registry
                .blocks
                .by_name_and_props(&block.block.id, &properties)
            {
                crate::redstone::set(state, writes, pos, next.0);
            }
        }
        true
    });
    state.brewing = stands;
}

/// Freeze latest timers into the region base before truncating their WAL.
pub(crate) fn checkpoint(state: &mut TickState) {
    for (key, stand) in &state.brewing {
        let Some(record) = state.block_entities.get(key) else {
            continue;
        };
        let pos = BlockPos {
            x: key.0,
            y: key.1,
            z: key.2,
        };
        let bytes = stand.encode(pos, &record.slots, &state.registry.items);
        state.world.set_opaque_block_entity(pos, Some(bytes));
    }
}

#[cfg(test)]
#[path = "brewing_tests.rs"]
mod tests;
