//! Retained allay collection, liked note blocks, jukebox dance and duplication.
//! 26.1.2 uses Inventory, equipment.mainhand, Brain memories and DuplicationCooldown.
use crate::bees::{block, centre, distance, fail, field, fly, pos_tag, position, uuid};
use crate::boundary::{BlockPos, ChangeSet, EntityDataValue, EntityId, Hand};
use crate::config::CoreError;
use crate::entities::EntitySlot;
use crate::tick::TickState;
use mc_data::item_nbt::{stack_from_fields, stack_to_fields};
use mc_data::{Identifier, ItemStack, items::ItemRegistry};
use mc_nbt::{ListTag, Tag};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

static ALLAY_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| crate::piglin::type_id_of("minecraft:allay"));

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllayState {
    pub held: ItemStack,
    pub inventory: ItemStack,
    pub liked_player: Option<uuid::Uuid>,
    #[serde(with = "crate::bees::position_serde")]
    pub note: Option<BlockPos>,
    pub note_dimension: String,
    pub note_ticks: u32,
    pub duplication_cooldown: u32,
    pub dancing: bool,
    pub pickup_cooldown: u32,
}
impl AllayState {
    fn validate(&self) -> Result<(), &'static str> {
        if self.duplication_cooldown > 6000 {
            return Err("Allay.DuplicationCooldown: expected 0..=6000");
        }
        if self.note_ticks > 600 {
            return Err("Allay.liked_noteblock_cooldown_ticks: expected 0..=600");
        }
        if self.pickup_cooldown > 60 {
            return Err("Allay.item_pickup_cooldown_ticks: expected 0..=60");
        }
        if (!self.held.is_empty() && self.held.count != 1)
            || self.held.count < 0
            || self.inventory.count < 0
            || self.inventory.count > 64
        {
            return Err("Allay.Inventory: invalid stack count");
        }
        Ok(())
    }
}
pub(crate) fn deserialize<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<AllayState>, D::Error> {
    let value = Option::<AllayState>::deserialize(d)?;
    if let Some(v) = &value {
        v.validate().map_err(serde::de::Error::custom)?;
    }
    Ok(value)
}
fn stack(items: &ItemRegistry, tag: &Tag, key: &str) -> Result<ItemStack, CoreError> {
    let Tag::Compound(fields) = tag else {
        return Err(fail(key, "expected item compound"));
    };
    stack_from_fields(items, fields)
        .map_err(|e| fail(key, &e.to_string()))?
        .ok_or_else(|| fail(key, "missing item id"))
}
fn memory<'a>(memories: &'a [(String, Tag)], key: &str) -> Result<Option<&'a Tag>, CoreError> {
    let Some(tag) = field(memories, key)? else {
        return Ok(None);
    };
    let Tag::Compound(fields) = tag else {
        return Err(fail(key, "expected memory compound"));
    };
    field(fields, "value")?
        .map(Some)
        .ok_or_else(|| fail(key, "missing value"))
}
pub(crate) fn from_record(
    items: &ItemRegistry,
    record: &mc_data::entity_nbt::VanillaEntity,
) -> Result<Option<AllayState>, CoreError> {
    if record.id != "minecraft:allay" {
        return Ok(None);
    }
    let fields = &record.extra_fields;
    let mut value = AllayState {
        held: crate::entity_loot::equipment_from_fields(items, fields)?[0]
            .clone()
            .unwrap_or_default(),
        ..Default::default()
    };
    if let Some(tag) = field(fields, "Inventory")? {
        let Tag::List(list) = tag else {
            return Err(fail("Allay.Inventory", "expected item list"));
        };
        if list.elements.len() > 1 {
            return Err(fail("Allay.Inventory", "expected at most one stack"));
        }
        if let Some(tag) = list.elements.first() {
            value.inventory = stack(items, tag, "Allay.Inventory")?;
        }
    }
    if let Some(tag) = field(fields, "DuplicationCooldown")? {
        let Tag::Long(v) = tag else {
            return Err(fail("Allay.DuplicationCooldown", "expected long"));
        };
        value.duplication_cooldown = u32::try_from(*v)
            .map_err(|_| fail("Allay.DuplicationCooldown", "expected 0..=6000"))?;
    }
    if let Some(tag) = field(fields, "Brain")? {
        let Tag::Compound(brain) = tag else {
            return Err(fail("Allay.Brain", "expected compound"));
        };
        if let Some(tag) = field(brain, "memories")? {
            let Tag::Compound(memories) = tag else {
                return Err(fail("Allay.Brain.memories", "expected compound"));
            };
            value.liked_player = memory(memories, "minecraft:liked_player")?
                .map(|tag| uuid(tag, "Allay.liked_player"))
                .transpose()?;
            if let Some(tag) = memory(memories, "minecraft:liked_noteblock")? {
                let Tag::Compound(global) = tag else {
                    return Err(fail("Allay.liked_noteblock", "expected global position"));
                };
                let Some(Tag::String(dimension)) = field(global, "dimension")? else {
                    return Err(fail("Allay.liked_noteblock.dimension", "expected string"));
                };
                let Some(pos) = field(global, "pos")? else {
                    return Err(fail("Allay.liked_noteblock.pos", "missing position"));
                };
                value.note = Some(position(pos, "Allay.liked_noteblock.pos")?);
                value.note_dimension = dimension.clone();
            }
            for (key, timer) in [
                (
                    "minecraft:liked_noteblock_cooldown_ticks",
                    &mut value.note_ticks,
                ),
                (
                    "minecraft:item_pickup_cooldown_ticks",
                    &mut value.pickup_cooldown,
                ),
            ] {
                if let Some(tag) = memory(memories, key)? {
                    let Tag::Int(v) = tag else {
                        return Err(fail(key, "expected int"));
                    };
                    *timer =
                        u32::try_from(*v).map_err(|_| fail(key, "expected nonnegative int"))?;
                }
            }
        }
    }
    if let Some(tag) = field(fields, "solaris:AllayDancing")? {
        let Tag::Byte(v @ (0 | 1)) = tag else {
            return Err(fail("Allay.dancing", "expected boolean byte"));
        };
        value.dancing = *v != 0;
    }
    value.validate().map_err(|e| fail("Allay", e))?;
    Ok(Some(value))
}
fn saved_memory(value: Tag) -> Tag {
    Tag::Compound(vec![("value".into(), value)])
}
pub(crate) fn to_fields(value: &AllayState, items: &ItemRegistry) -> Vec<(String, Tag)> {
    let inventory = if value.inventory.is_empty() {
        Vec::new()
    } else {
        vec![Tag::Compound(
            stack_to_fields(items, &value.inventory).expect("allay stack"),
        )]
    };
    let mut memories = Vec::new();
    if let Some(uuid) = value.liked_player {
        memories.push((
            "minecraft:liked_player".into(),
            saved_memory(crate::special_mobs::persistence::uuid_tag(uuid)),
        ));
    }
    if let Some(pos) = value.note {
        memories.push((
            "minecraft:liked_noteblock".into(),
            saved_memory(Tag::Compound(vec![
                (
                    "dimension".into(),
                    Tag::String(value.note_dimension.clone()),
                ),
                ("pos".into(), pos_tag(pos)),
            ])),
        ));
    }
    if value.note_ticks > 0 {
        memories.push((
            "minecraft:liked_noteblock_cooldown_ticks".into(),
            saved_memory(Tag::Int(value.note_ticks as i32)),
        ));
    }
    if value.pickup_cooldown > 0 {
        memories.push((
            "minecraft:item_pickup_cooldown_ticks".into(),
            saved_memory(Tag::Int(value.pickup_cooldown as i32)),
        ));
    }
    vec![
        (
            "Inventory".into(),
            Tag::List(ListTag {
                element_type: 10,
                elements: inventory,
            }),
        ),
        (
            "DuplicationCooldown".into(),
            Tag::Long(i64::from(value.duplication_cooldown)),
        ),
        (
            "Brain".into(),
            Tag::Compound(vec![("memories".into(), Tag::Compound(memories))]),
        ),
        (
            "listener".into(),
            Tag::Compound(vec![
                (
                    "selector".into(),
                    Tag::Compound(vec![("tick".into(), Tag::Long(-1))]),
                ),
                ("event_delay".into(), Tag::Int(0)),
            ]),
        ),
        (
            "solaris:AllayDancing".into(),
            Tag::Byte(i8::from(value.dancing)),
        ),
    ]
}
pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot, type_id: u32) {
    if state
        .registry
        .blueprint(type_id)
        .is_some_and(|b| b.name == "minecraft:allay")
    {
        debug_assert!(
            matches!(
                state.entities.body(slot).kind,
                crate::entities::EntityKind::Mob { type_id: kind }
                if kind == type_id
            ),
            "allay component implies the allay mob lane"
        );
        state
            .entities
            .row_mut(slot)
            .allay
            .get_or_insert_with(Box::default);
        appearance(state, slot);
    }
}
pub(crate) fn appearance(state: &mut TickState, slot: EntitySlot) {
    if let Some(value) = state.entities.row(slot).allay.as_ref() {
        let dancing = value.dancing;
        let duplicate = value.duplication_cooldown == 0;
        let expected = (!value.held.is_empty()).then_some(&value.held);
        let changed = state.entities.row(slot).appearance.equipment[0].as_ref() != expected;
        let held = changed.then(|| expected.cloned());
        let row = state.entities.row_mut(slot);
        row.loot.drop_chances[0] = 0.0;
        if let Some(held) = held {
            Arc::make_mut(&mut row.appearance).equipment[0] = held;
        }
        crate::special_mobs::metadata(state, slot, 16, EntityDataValue::Boolean(dancing));
        crate::special_mobs::metadata(state, slot, 17, EntityDataValue::Boolean(duplicate));
    }
}
pub(crate) fn note_played(state: &mut TickState, pos: BlockPos) {
    for index in 0..state.entities.len() {
        let slot = EntitySlot(index as u32);
        if distance(state.entities.pos(slot), centre(pos)) > 16.0 * 16.0 {
            continue;
        }
        if let Some(value) = state.entities.row_mut(slot).allay.as_deref_mut()
            && (value.note.is_none() || value.note == Some(pos))
        {
            value.note = Some(pos);
            value.note_dimension = state.dimension.name().to_owned();
            value.note_ticks = 600;
        }
    }
}
pub(crate) fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
    changes: &mut ChangeSet,
) -> bool {
    let Some(slot) = state.entities.slot_of(target) else {
        return false;
    };
    if state.entities.row(slot).allay.is_none() {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    if player.spectator() {
        return true;
    }
    let connection = player.connection;
    let held = player.inventory.slots[crate::tick::held_slot_index(player, hand)].clone();
    let actor_uuid = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    let shard = !held.is_empty()
        && state
            .registry
            .items
            .name_of(held.item_id)
            .is_some_and(|id| id.path() == "amethyst_shard");
    let value = state
        .entities
        .row(slot)
        .allay
        .as_ref()
        .expect("native allay");
    if ((shard && value.dancing && value.duplication_cooldown == 0)
        || (value.held.is_empty() && !held.is_empty()))
        && crate::tick::held_debit_below_floor(state, actor, hand, 1)
    {
        return false;
    }
    let mut boxed = state.entities.row_mut(slot).allay.take().unwrap();
    let value = &mut *boxed;
    let interaction_item = crate::advancements::item_snapshot(state, &held);
    let mut accepted = true;
    if shard && value.dancing && value.duplication_cooldown == 0 {
        value.duplication_cooldown = 6000;
        let type_id = state
            .registry
            .entity_types
            .id_of(&Identifier::parse("minecraft:allay").unwrap())
            .unwrap();
        let pos = state.entities.pos(slot);
        let child = state.spawn_mob(type_id, pos, 20.0);
        let child_slot = state.entities.slot_of(child).unwrap();
        state
            .entities
            .row_mut(child_slot)
            .allay
            .as_mut()
            .unwrap()
            .duplication_cooldown = 6000;
        state.entities.row_mut(child_slot).persistent = true;
        appearance(state, child_slot);
        changes.entities_spawned.push(child);
        let effects = state.players.get_mut(&actor).unwrap().consume_held(hand, 1);
        for change in effects.changes {
            state.push_player(connection, change);
        }
    } else if value.held.is_empty() && !held.is_empty() {
        value.held = held;
        value.held.count = 1;
        value.liked_player = Some(actor_uuid);
        let effects = state.players.get_mut(&actor).unwrap().consume_held(hand, 1);
        for change in effects.changes {
            state.push_player(connection, change);
        }
    } else if held.is_empty() && !value.held.is_empty() {
        let registry = Arc::clone(&state.registry);
        let rules = registry.item_rules();
        let mut returns = vec![std::mem::take(&mut value.held)];
        if !value.inventory.is_empty() {
            returns.push(std::mem::take(&mut value.inventory));
        }
        for stack in returns {
            let effects = state
                .players
                .get_mut(&actor)
                .unwrap()
                .receive_stack(&rules, stack);
            for change in effects.changes {
                state.push_player(connection, change);
            }
            for stack in effects.dropped {
                changes
                    .entities_spawned
                    .push(crate::tick::spawn_item_entity(
                        state,
                        state.entities.pos(slot),
                        stack,
                    ));
            }
        }
        value.liked_player = None;
        value.note = None;
        value.note_ticks = 0;
    } else {
        accepted = false;
    }
    state.entities.row_mut(slot).allay = Some(boxed);
    state.entities.row_mut(slot).persistent = true;
    appearance(state, slot);
    if accepted {
        let entity = crate::advancements::entity_snapshot(state, target);
        crate::advancements::trigger(
            state,
            actor,
            "minecraft:player_interacted_with_entity",
            serde_json::json!({"item": interaction_item, "entity": entity}),
        );
    }
    true
}
pub(crate) fn run(state: &mut TickState, changes: &mut ChangeSet) {
    let ids: Vec<_> = ALLAY_TYPE
        .iter()
        .flat_map(|&type_id| state.entities.mob_slots(type_id))
        .filter(|&slot| state.entities.row(slot).allay.is_some())
        .map(|slot| state.entities.body(slot).id)
        .collect();
    let search_tick = state.tick.is_multiple_of(20);
    let mut census_built = false;
    for id in ids {
        let Some(slot) = state.entities.slot_of_any(id) else {
            continue;
        };
        let p = state.entities.pos(slot);
        if !state.world.is_active(
            (p[0].floor() as i32).div_euclid(16),
            (p[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        if !state.entities.body(slot).alive {
            continue;
        }
        let mut boxed = state.entities.row_mut(slot).allay.take().unwrap();
        let value = &mut *boxed;
        value.duplication_cooldown = value.duplication_cooldown.saturating_sub(1);
        value.pickup_cooldown = value.pickup_cooldown.saturating_sub(1);
        value.note_ticks = value.note_ticks.saturating_sub(1);
        if value.note_ticks == 0 {
            value.note = None;
        }
        if state.tick.is_multiple_of(20) {
            value.dancing = crate::jukebox::nearby_playing(state, p);
        }
        if !value.dancing && !value.held.is_empty() {
            let note_destination = value
                .note
                .filter(|_| value.note_dimension == state.dimension.name())
                .filter(|pos| {
                    block(state, *pos).is_some_and(|b| b.block.id.path() == "note_block")
                });
            let destination = note_destination.map(centre).or_else(|| {
                value
                    .liked_player
                    .and_then(|uuid| state.entities.slot_of_uuid(uuid))
                    .map(|slot| state.entities.pos(slot))
            });
            if !value.inventory.is_empty() {
                if let Some(target) = destination {
                    fly(state, slot, [target[0], target[1] + 1.0, target[2]]);
                    if distance(p, target) < 9.0 {
                        // AllayAi.getLikedPlayer requires survival/creative and
                        // strictly less than 64 blocks from the delivering allay.
                        let recipient = note_destination.and_then(|_| {
                            let player_slot = state.entities.slot_of_uuid(value.liked_player?)?;
                            let actor = state.entities.body(player_slot).id;
                            let player = state.players.get(&actor)?;
                            (matches!(
                                player.game_mode,
                                crate::commands::GameMode::Survival
                                    | crate::commands::GameMode::Creative
                            ) && distance(
                                state.entities.pos(player_slot),
                                state.entities.pos(slot),
                            ) < 64.0 * 64.0)
                                .then_some(actor)
                        });
                        let evidence = recipient.map(|actor| {
                            (actor, serde_json::json!({
                                "item": crate::advancements::item_snapshot(state, &value.inventory),
                                "location": crate::advancements::block_location(state, note_destination.unwrap(), None),
                            }))
                        });
                        let item = crate::tick::spawn_item_entity(
                            state,
                            [target[0], target[1] + 0.6, target[2]],
                            std::mem::take(&mut value.inventory),
                        );
                        changes.entities_spawned.push(item);
                        value.pickup_cooldown = 60;
                        if let Some((actor, evidence)) = evidence {
                            crate::advancements::trigger(
                                state,
                                actor,
                                "minecraft:allay_drop_item_on_block",
                                evidence,
                            );
                        }
                    }
                }
            } else if value.pickup_cooldown == 0
                && search_tick
                && crate::command_rules::boolean(state, "mob_griefing")
            {
                if !census_built {
                    // One current-tick rebuild for collecting allays; reuse the
                    // chunk buckets and revalidate every ID against live rows.
                    crate::natural_spawn::census(state, &[]);
                    census_built = true;
                }
                let item = state
                    .spawn_census
                    .nearby_entities(p, 32.0)
                    .filter_map(|id| {
                        let s = state.entities.slot_of(id)?;
                        let body = state.entities.body(s);
                        let row = state.entities.row(s);
                        let carried = row.item.as_ref()?;
                        let plain = ItemStack::new(carried.item_id, carried.count);
                        let stack = carried.stack.as_ref().unwrap_or(&plain);
                        let held_potion = value.held.extended();
                        let potion = stack.extended();
                        let d = distance(p, state.entities.pos(s));
                        (body.alive
                            && body.pickup_delay == 0
                            && carried.item_id == value.held.item_id
                            && potion.potion == held_potion.potion
                            && potion.potion_custom_color == held_potion.potion_custom_color
                            && potion.potion_custom_effects == held_potion.potion_custom_effects
                            && potion.potion_custom_name == held_potion.potion_custom_name
                            && d <= 32.0 * 32.0)
                            .then_some((body.id, d))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1));
                if let Some((item, d)) = item {
                    let item_slot = state.entities.slot_of(item).unwrap();
                    let target = state.entities.pos(item_slot);
                    fly(state, slot, target);
                    if d < 2.0 {
                        let stack = state
                            .entities
                            .row(item_slot)
                            .item
                            .as_ref()
                            .unwrap()
                            .full_stack();
                        let count = stack
                            .count
                            .min(state.registry.item_rules().max_stack(&stack))
                            .min(64);
                        value.inventory = stack;
                        value.inventory.count = count;
                        let row = state.entities.row_mut(item_slot);
                        let carried = row.item.as_mut().unwrap();
                        carried.count -= count;
                        if let Some(stack) = &mut carried.stack {
                            stack.count = carried.count;
                        }
                        if carried.count == 0 {
                            state.entities.remove(item_slot);
                            changes.entities_removed.push(item);
                        } else {
                            Arc::make_mut(&mut row.appearance).item = Some(carried.full_stack());
                            state.appearance_changed.push(item);
                        }
                    }
                } else if let Some(target) = destination
                    && distance(p, target) > 16.0
                {
                    fly(state, slot, [target[0], target[1] + 1.0, target[2]]);
                }
            }
        }
        let Some(slot) = state.entities.slot_of(id) else {
            continue;
        };
        state.entities.row_mut(slot).allay = Some(boxed);
        appearance(state, slot);
    }
}
#[cfg(test)]
#[path = "allay_tests.rs"]
mod tests;
