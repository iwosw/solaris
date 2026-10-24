//! Loaded trial spawners: UUID admission, bounded waves, ejection and absolute cooldowns.
use crate::boundary::{BlockPos, ChangeSet, EntityId, PlayerChange};
use crate::tick::{TickState, spawn_item_entity};
use mc_data::item_nbt::{field, stack_from_fields, stack_to_fields};
use mc_data::trial_nbt::{self as nbt, Fields, SpawnerData};
use mc_data::{Identifier, ItemStack};
use mc_nbt::{ListTag, Tag, tag_type};
use std::collections::BTreeMap;

pub(crate) type Spawners = BTreeMap<(i32, i32, i32), Spawner>;
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Spawner {
    pub data: SpawnerData,
    pending: Option<Pending>,
}
#[derive(Debug, Clone, PartialEq)]
struct Pending {
    uuid: uuid::Uuid,
    item: ItemStack,
    deadline: u64,
    position: [f64; 3],
}

pub(crate) fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}
pub(crate) fn position(key: (i32, i32, i32)) -> BlockPos {
    BlockPos {
        x: key.0,
        y: key.1,
        z: key.2,
    }
}
pub(crate) fn center(pos: BlockPos) -> [f64; 3] {
    [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5]
}
pub(crate) fn block(state: &TickState, pos: BlockPos, name: &str) -> Option<u32> {
    let id = state.world.block_at(pos.x, pos.y, pos.z)?;
    let block = state.registry.blocks.by_id(mc_world::BlockStateId(id))?;
    (block.block.id.path() == name).then_some(id)
}
pub(crate) fn property<'a>(state: &'a TickState, pos: BlockPos, name: &str) -> Option<&'a str> {
    let id = state.world.block_at(pos.x, pos.y, pos.z)?;
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(id))?
        .properties
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.as_str())
}
pub(crate) fn set_properties(
    state: &mut TickState,
    pos: BlockPos,
    values: &[(&str, &str)],
    writes: &mut Vec<(BlockPos, u32)>,
) {
    let Some(id) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
        return;
    };
    let mut properties = block.properties.clone();
    for (name, value) in values {
        if let Some((_, old)) = properties.iter_mut().find(|(key, _)| key == name) {
            *old = (*value).into();
        }
    }
    if let Some(next) = state
        .registry
        .blocks
        .by_name_and_props(&block.block.id, &properties)
        && next.0 != id
    {
        state.world.set_block(pos.x, pos.y, pos.z, next.0);
        writes.push((pos, next.0));
    }
}
pub(crate) fn persist(
    state: &mut TickState,
    pos: BlockPos,
    id: &str,
    fields: Fields,
    wire: Fields,
    type_id: u32,
) {
    let bytes = nbt::encode(fields, id, [pos.x, pos.y, pos.z]);
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
    state.world.mark_block_entity_visible(pos);
    let connections: Vec<_> = state
        .players
        .values()
        .map(|player| player.connection)
        .collect();
    for connection in connections {
        state.push_player(
            connection,
            PlayerChange::BlockEntityData {
                position: pos,
                block_entity_type: type_id,
                nbt: Tag::Compound(wire.clone()),
            },
        );
    }
}
impl Spawner {
    fn new() -> Self {
        let mut data = SpawnerData::default();
        nbt::set(
            &mut data.fields,
            "spawn_data",
            Tag::Compound(vec![(
                "entity".into(),
                Tag::Compound(vec![("id".into(), Tag::String("minecraft:zombie".into()))]),
            )]),
        );
        Self {
            data,
            pending: None,
        }
    }
    pub(crate) fn decode(
        bytes: &[u8],
        items: &mc_data::items::ItemRegistry,
    ) -> Result<Self, String> {
        let fields = nbt::decode(bytes)?;
        let data = SpawnerData::from_fields(fields)?;
        let pending = if let Some(fields) = nbt::compound(&data.fields, "solaris:ominous_item")? {
            let uuids = nbt::uuids(fields, "uuid")?;
            let uuid = uuids
                .into_iter()
                .next()
                .ok_or("solaris:ominous_item.uuid: missing UUID")?;
            let item = stack_from_fields(
                items,
                nbt::compound(fields, "item")?
                    .ok_or("solaris:ominous_item.item: missing Compound")?,
            )
            .map_err(|e| e.to_string())?
            .ok_or("solaris:ominous_item.item: empty")?;
            let deadline = nbt::long(fields, "eject_at", 0)?;
            let Some(Tag::List(list)) = field(fields, "position") else {
                return Err("solaris:ominous_item.position: missing Double list".into());
            };
            let [Tag::Double(x), Tag::Double(y), Tag::Double(z)] = list.elements.as_slice() else {
                return Err("solaris:ominous_item.position: expected Double[3]".into());
            };
            Some(Pending {
                uuid,
                item,
                deadline,
                position: [*x, *y, *z],
            })
        } else {
            None
        };
        Ok(Self { data, pending })
    }
    fn fields(&self, items: &mc_data::items::ItemRegistry) -> Fields {
        let mut fields = self.data.to_fields();
        fields.retain(|(name, _)| name != "solaris:ominous_item");
        if let Some(pending) = &self.pending {
            fields.push((
                "solaris:ominous_item".into(),
                Tag::Compound(vec![
                    (
                        "uuid".into(),
                        nbt::uuid_list(&[pending.uuid].into_iter().collect()),
                    ),
                    (
                        "item".into(),
                        Tag::Compound(
                            stack_to_fields(items, &pending.item).expect("registered ominous item"),
                        ),
                    ),
                    ("eject_at".into(), Tag::Long(pending.deadline as i64)),
                    (
                        "position".into(),
                        Tag::List(ListTag {
                            element_type: tag_type::DOUBLE,
                            elements: pending.position.into_iter().map(Tag::Double).collect(),
                        }),
                    ),
                ]),
            ));
        }
        fields
    }
    fn persist(&self, state: &mut TickState, pos: BlockPos) {
        let fields = self.fields(&state.registry.items);
        let wire = fields
            .iter()
            .filter(|(name, _)| matches!(name.as_str(), "spawn_data" | "next_mob_spawns_at"))
            .cloned()
            .collect();
        let kind = mc_data::block_entity_kinds::BlockEntityKind::TrialSpawner;
        persist(
            state,
            pos,
            kind.vanilla_id(),
            fields,
            wire,
            kind.protocol_id(),
        );
    }
}
pub(crate) fn hydrate_chunk(
    spawners: &mut Spawners,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) -> Result<(), String> {
    for (pos, bytes) in &chunk.block_entities {
        if spawners.contains_key(&key(*pos)) {
            continue;
        }
        if chunk
            .get_block(
                pos.x.rem_euclid(16) as u8,
                pos.y,
                pos.z.rem_euclid(16) as u8,
            )
            .and_then(|id| registry.blocks.by_id(id))
            .is_some_and(|block| block.block.id.path() == "trial_spawner")
        {
            let spawner = Spawner::decode(bytes, &registry.items).map_err(|error| {
                format!("trial_spawner at ({},{},{}): {error}", pos.x, pos.y, pos.z)
            })?;
            spawners.insert(key(*pos), spawner);
        }
    }
    Ok(())
}
pub(crate) fn after_blocks(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    for &(pos, _) in writes {
        if block(state, pos, "trial_spawner").is_some()
            && !state.trial_spawners.contains_key(&key(pos))
        {
            let spawner = Spawner::new();
            spawner.persist(state, pos);
            state.trial_spawners.insert(key(pos), spawner);
        }
    }
}
fn config(data: &SpawnerData, ominous: bool) -> Option<&Fields> {
    nbt::compound(
        &data.fields,
        if ominous {
            "ominous_config"
        } else {
            "normal_config"
        },
    )
    .ok()
    .flatten()
}
fn number(config: Option<&Fields>, name: &str, default: f32) -> f32 {
    config
        .and_then(|fields| field(fields, name))
        .and_then(|value| {
            if let Tag::Float(value) = value {
                Some(*value)
            } else {
                None
            }
        })
        .unwrap_or(default)
}
// Named presets follow TrialSpawnerConfigs' base/slow-ranged/breeze builders.
fn limits(data: &SpawnerData, ominous: bool) -> (i32, usize, i32, i32) {
    let extra = data.players.len().saturating_sub(1) as f32;
    if let Some(Tag::String(name)) = field(
        &data.fields,
        if ominous {
            "ominous_config"
        } else {
            "normal_config"
        },
    ) {
        let preset_ominous = name.ends_with("/ominous");
        let (total, simultaneous, total_added, simultaneous_added, ticks) =
            if name.contains("/breeze/") {
                (
                    if preset_ominous { 4.0 } else { 2.0 },
                    if preset_ominous { 2.0 } else { 1.0 },
                    1.0,
                    0.5,
                    20,
                )
            } else if name.contains("/slow_ranged/") {
                (6.0, 4.0, 2.0, 2.0, 160)
            } else if name.contains("/baby_zombie/") {
                (6.0, 2.0, 2.0, 0.5, 20)
            } else if preset_ominous
                && (name.contains("/spider/")
                    || name.contains("/cave_spider/")
                    || name.contains("/silverfish/")
                    || name.contains("/slime/"))
            {
                (12.0, 4.0, 2.0, 0.5, 20)
            } else {
                (6.0, 3.0, 2.0, 0.5, 20)
            };
        return (
            (total + total_added * extra).floor() as i32,
            (simultaneous + simultaneous_added * extra).floor() as usize,
            ticks,
            4,
        );
    }
    let config = config(data, ominous);
    let total = (number(config, "total_mobs", 6.0)
        + number(config, "total_mobs_added_per_player", 2.0) * extra)
        .floor() as i32;
    let cap = (number(config, "simultaneous_mobs", 2.0)
        + number(config, "simultaneous_mobs_added_per_player", 1.0) * extra)
        .floor() as usize;
    let ticks = config
        .map(|fields| nbt::int(fields, "ticks_between_spawn", 40).expect("validated ticks"))
        .unwrap_or(40);
    let range = config
        .map(|fields| nbt::int(fields, "spawn_range", 4).expect("validated range"))
        .unwrap_or(4);
    (total, cap, ticks, range)
}
fn mob_name(data: &SpawnerData, ominous: bool, seed: u64) -> Option<String> {
    if let Some(fields) = config(data, ominous)
        && let Some(Tag::List(list)) = field(fields, "spawn_potentials")
        && !list.elements.is_empty()
    {
        let total: u64 = list
            .elements
            .iter()
            .map(|entry| {
                if let Tag::Compound(fields) = entry {
                    nbt::int(fields, "weight", 1).unwrap_or(1) as u64
                } else {
                    0
                }
            })
            .sum();
        let mut pick = seed % total.max(1);
        for entry in &list.elements {
            let Tag::Compound(fields) = entry else {
                continue;
            };
            let weight = nbt::int(fields, "weight", 1).unwrap_or(1) as u64;
            if pick >= weight {
                pick -= weight;
                continue;
            }
            let data = nbt::compound(fields, "data").ok().flatten()?;
            let entity = nbt::compound(data, "entity").ok().flatten()?;
            return match field(entity, "id") {
                Some(Tag::String(name)) => Some(name.clone()),
                _ => None,
            };
        }
    }
    if let Some(Tag::String(name)) = field(
        &data.fields,
        if ominous {
            "ominous_config"
        } else {
            "normal_config"
        },
    ) {
        let stem = name
            .strip_suffix("/normal")
            .or_else(|| name.strip_suffix("/ominous"))?;
        let suffix = stem.rsplit('/').next()?;
        let mob = match suffix {
            "baby_zombie" => "zombie",
            "poison_skeleton" => "bogged",
            name => name,
        };
        return Some(format!("minecraft:{mob}"));
    }
    let entity = nbt::compound(
        nbt::compound(&data.fields, "spawn_data").ok().flatten()?,
        "entity",
    )
    .ok()
    .flatten()?;
    match field(entity, "id") {
        Some(Tag::String(name)) => Some(name.clone()),
        _ => None,
    }
}
pub(crate) fn nearby(
    state: &TickState,
    pos: BlockPos,
    range: f64,
    include_creative: bool,
) -> Vec<EntityId> {
    let center = center(pos);
    state
        .players
        .iter()
        .filter_map(|(&entity, player)| {
            if player.game_mode == crate::commands::GameMode::Spectator
                || (!include_creative && player.creative())
                || player.vitals.is_dead()
            {
                return None;
            }
            let at = state.entities.pos(state.entities.slot_of(entity)?);
            (at.into_iter()
                .zip(center)
                .map(|(a, b)| (a - b).powi(2))
                .sum::<f64>()
                <= range * range)
                .then_some(entity)
        })
        .collect()
}
fn pending_tick(state: &mut TickState, spawner: &mut Spawner, changes: &mut ChangeSet) {
    let Some(pending) = spawner.pending.as_ref() else {
        return;
    };
    if let Some(slot) = state.entities.slot_of_uuid(pending.uuid) {
        state.entities.set_pos(slot, pending.position);
        let (body, row) = state.entities.parts_mut(slot);
        body.movement = crate::blueprint::MovementClass::Stationary;
        row.controls.no_ai = true;
    }
    if state.tick < pending.deadline {
        return;
    }
    let pending = spawner.pending.take().expect("pending");
    if let Some(slot) = state.entities.slot_of_uuid(pending.uuid) {
        changes.entities_removed.push(state.entities.body(slot).id);
        state.remove_entity(slot);
    }
    let kind = state
        .registry
        .items
        .name_of(pending.item.item_id)
        .and_then(|name| match name.path() {
            "arrow" | "tipped_arrow" => Some(crate::projectiles::ProjectileKind::Arrow),
            "lingering_potion" => Some(crate::projectiles::ProjectileKind::LingeringPotion),
            "wind_charge" => Some(crate::projectiles::ProjectileKind::WindCharge),
            "fire_charge" => Some(crate::projectiles::ProjectileKind::SmallFireball),
            _ => None,
        });
    if let Some(kind) = kind {
        let record = crate::projectiles::ProjectileState::new(kind, None, pending.item);
        crate::projectiles::launch(state, record, pending.position, [0.0, -0.3, 0.0])
            .expect("registered ominous projectile");
    } else {
        changes
            .entities_spawned
            .push(spawn_item_entity(state, pending.position, pending.item));
    }
}
fn spawn_ominous_item(
    state: &mut TickState,
    spawner: &mut Spawner,
    pos: BlockPos,
    seed: u64,
    changes: &mut ChangeSet,
) {
    if spawner.pending.is_some() {
        return;
    }
    let table = config(&spawner.data, true)
        .and_then(|fields| field(fields, "items_to_drop_when_ominous"))
        .and_then(|value| {
            if let Tag::String(name) = value {
                Some(name.as_str())
            } else {
                None
            }
        })
        .unwrap_or("minecraft:spawners/trial_chamber/items_to_drop_when_ominous");
    let Some(item) = state
        .registry
        .trial_loot
        .roll(
            table,
            seed,
            &state.registry.items,
            &state.registry.item_facts,
        )
        .expect("validated ominous loot")
        .into_iter()
        .next()
    else {
        return;
    };
    let mut at = center(pos);
    if let Some(entity) = nearby(state, pos, 14.0, false).first()
        && let Some(slot) = state.entities.slot_of(*entity)
    {
        at = state.entities.pos(slot);
    }
    at[1] += 5.0;
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:ominous_item_spawner").expect("id"))
        .expect("marker type");
    let id = state.spawn_mob(type_id, at, 1.0);
    let slot = state.entities.slot_of(id).expect("spawned marker");
    let (body, row) = state.entities.parts_mut(slot);
    body.movement = crate::blueprint::MovementClass::Stationary;
    row.controls.no_ai = true;
    row.controls.invulnerable = true;
    row.persistent = true;
    let mut appearance = (*row.appearance).clone();
    appearance.extra.push((
        8,
        crate::boundary::EntityDataValue::Item(Some(item.clone())),
    ));
    row.appearance = std::sync::Arc::new(appearance);
    spawner.pending = Some(Pending {
        uuid: row.uuid,
        item,
        deadline: state.tick + 60 + seed % 61,
        position: at,
    });
    changes.entities_spawned.push(id);
}

pub(crate) fn run(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    changes: &mut ChangeSet,
) {
    let mut spawners = std::mem::take(&mut state.trial_spawners);
    spawners.retain(|key, spawner| {
        let pos = position(*key);
        if !state
            .world
            .is_block_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
        {
            return true;
        }
        if block(state, pos, "trial_spawner").is_none() {
            if let Some(pending) = spawner.pending.take()
                && let Some(slot) = state.entities.slot_of_uuid(pending.uuid)
            {
                changes.entities_removed.push(state.entities.body(slot).id);
                state.remove_entity(slot);
            }
            state.world.set_opaque_block_entity(pos, None);
            state.opaque_changes.push((pos, None));
            return false;
        }
        let previous = (
            spawner.data.players.len(),
            spawner.data.mobs.len(),
            spawner.data.cooldown,
            spawner.data.next_spawn,
            spawner.data.total,
            spawner.pending.is_some(),
        );
        pending_tick(state, spawner, changes);
        let phase = match property(state, pos, "trial_spawner_state") {
            Some("waiting_for_players") => "waiting_for_players",
            Some("active") => "active",
            Some("waiting_for_reward_ejection") => "waiting_for_reward_ejection",
            Some("ejecting_reward") => "ejecting_reward",
            Some("cooldown") => "cooldown",
            _ => "inactive",
        };
        let mut ominous = property(state, pos, "ominous") == Some("true");
        let seed = state.tick
            ^ (pos.x as u64).rotate_left(17)
            ^ (pos.z as u64).rotate_left(39)
            ^ spawner.data.total as u64;
        let mut next = phase;
        if phase == "cooldown" {
            if state.tick >= spawner.data.cooldown {
                spawner.data.players.clear();
                spawner.data.mobs.clear();
                spawner.data.total = 0;
                spawner
                    .data
                    .fields
                    .retain(|(name, _)| name != "ejecting_loot_table");
                ominous = false;
                next = "waiting_for_players";
            }
        } else if phase == "waiting_for_reward_ejection" {
            if state.tick >= spawner.data.next_spawn {
                next = "ejecting_reward";
            }
        } else if phase == "ejecting_reward" {
            if state.tick >= spawner.data.next_spawn {
                if let Some(uuid) = spawner.data.players.pop_first() {
                    let table = match field(&spawner.data.fields, "ejecting_loot_table") {
                        Some(Tag::String(table)) => table.as_str(),
                        _ if ominous => "minecraft:spawners/ominous/trial_chamber/key",
                        _ => "minecraft:spawners/trial_chamber/key",
                    };
                    for item in state
                        .registry
                        .trial_loot
                        .roll(
                            table,
                            seed,
                            &state.registry.items,
                            &state.registry.item_facts,
                        )
                        .expect("validated trial loot")
                    {
                        changes.entities_spawned.push(spawn_item_entity(
                            state,
                            [pos.x as f64 + 0.5, pos.y as f64 + 1.5, pos.z as f64 + 0.5],
                            item,
                        ));
                    }
                    if ominous && let Some(slot) = state.entities.slot_of_uuid(uuid) {
                        crate::omens::consume_trial(state, state.entities.body(slot).id);
                    }
                    spawner.data.next_spawn = state.tick + 30;
                } else {
                    next = "cooldown";
                }
            }
        } else {
            let range = nbt::int(&spawner.data.fields, "required_player_range", 14)
                .expect("validated range");
            let players = nearby(state, pos, range as f64, false);
            for entity in players {
                if !ominous && crate::omens::activate_trial(state, entity) {
                    ominous = true;
                }
                let uuid = state
                    .entities
                    .row(state.entities.slot_of(entity).expect("player"))
                    .uuid;
                if spawner.data.players.insert(uuid) {
                    spawner.data.next_spawn = spawner.data.next_spawn.max(state.tick + 40);
                }
            }
            let mob = mob_name(&spawner.data, ominous, seed);
            if mob.is_none() {
                next = "inactive";
            } else if spawner.data.players.is_empty() {
                next = "waiting_for_players";
            } else {
                next = "active";
                spawner.data.mobs.retain(|uuid| {
                    state.entities.slot_of_uuid(*uuid).is_some_and(|slot| {
                        state.entities.body(slot).alive && state.entities.row(slot).health > 0.0
                    })
                });
                let (target, cap, ticks, range) = limits(&spawner.data, ominous);
                let config = config(&spawner.data, ominous);
                if spawner.data.total >= target && spawner.data.mobs.is_empty() {
                    next = "waiting_for_reward_ejection";
                    let mut selected_loot = None;
                    if let Some(Tag::List(list)) =
                        config.and_then(|fields| field(fields, "loot_tables_to_eject"))
                    {
                        let total: u64 = list
                            .elements
                            .iter()
                            .filter_map(|entry| {
                                if let Tag::Compound(fields) = entry {
                                    Some(nbt::int(fields, "weight", 1).expect("validated weight")
                                        as u64)
                                } else {
                                    None
                                }
                            })
                            .sum();
                        if total > 0 {
                            let mut pick = seed % total;
                            for entry in &list.elements {
                                let Tag::Compound(fields) = entry else {
                                    continue;
                                };
                                let weight =
                                    nbt::int(fields, "weight", 1).expect("validated weight") as u64;
                                if pick < weight {
                                    selected_loot = Some(
                                        field(fields, "data")
                                            .expect("validated loot table")
                                            .clone(),
                                    );
                                    break;
                                }
                                pick -= weight;
                            }
                        }
                    }
                    if selected_loot.is_none()
                        && matches!(
                            field(
                                &spawner.data.fields,
                                if ominous {
                                    "ominous_config"
                                } else {
                                    "normal_config"
                                }
                            ),
                            Some(Tag::String(_))
                        )
                    {
                        let table = if ominous {
                            if seed % 10 < 3 {
                                "minecraft:spawners/ominous/trial_chamber/key"
                            } else {
                                "minecraft:spawners/ominous/trial_chamber/consumables"
                            }
                        } else if seed.is_multiple_of(2) {
                            "minecraft:spawners/trial_chamber/key"
                        } else {
                            "minecraft:spawners/trial_chamber/consumables"
                        };
                        selected_loot = Some(Tag::String(table.into()));
                    }
                    if let Some(selected) = selected_loot {
                        nbt::set(&mut spawner.data.fields, "ejecting_loot_table", selected);
                    }
                    spawner.data.next_spawn = state.tick + 40;
                    spawner.data.cooldown = state.tick
                        + nbt::int(&spawner.data.fields, "target_cooldown_length", 36_000)
                            .expect("validated cooldown") as u64;
                } else if state.tick >= spawner.data.next_spawn
                    && spawner.data.total < target
                    && spawner.data.mobs.len() < cap
                {
                    let mob =
                        Identifier::parse(mob.as_deref().expect("mob")).expect("validated mob id");
                    if let Some(type_id) = state.registry.entity_types.id_of(&mob) {
                        let offset = (spawner.data.total % (range * 2 + 1)) - range;
                        let at = [
                            pos.x as f64 + 0.5 + offset as f64,
                            pos.y as f64 + 1.0,
                            pos.z as f64 + 0.5,
                        ];
                        if state
                            .world
                            .block_at(at[0].floor() as i32, pos.y + 1, pos.z)
                            .is_some_and(|id| id == state.registry.states.air)
                            && state
                                .world
                                .block_at(at[0].floor() as i32, pos.y + 2, pos.z)
                                .is_some_and(|id| id == state.registry.states.air)
                        {
                            let health = state
                                .registry
                                .blueprint(type_id)
                                .map_or(20.0, |bp| bp.params.max_health);
                            let id = state.spawn_mob(type_id, at, health);
                            let slot = state.entities.slot_of(id).expect("spawned trial mob");
                            state.entities.row_mut(slot).persistent = true;
                            spawner.data.mobs.insert(state.entities.row(slot).uuid);
                            spawner.data.total += 1;
                            changes.entities_spawned.push(id);
                        }
                    }
                    spawner.data.next_spawn = state.tick + ticks.max(1) as u64;
                }
                if ominous && state.tick.is_multiple_of(160) {
                    spawn_ominous_item(state, spawner, pos, seed, changes);
                }
            }
        }
        set_properties(
            state,
            pos,
            &[
                ("trial_spawner_state", next),
                ("ominous", if ominous { "true" } else { "false" }),
            ],
            writes,
        );
        let current = (
            spawner.data.players.len(),
            spawner.data.mobs.len(),
            spawner.data.cooldown,
            spawner.data.next_spawn,
            spawner.data.total,
            spawner.pending.is_some(),
        );
        if current != previous || phase != next {
            spawner.persist(state, pos);
        }
        true
    });
    state.trial_spawners = spawners;
}
pub(crate) fn entity_fields(state: &TickState, uuid: uuid::Uuid) -> Fields {
    let Some(pending) = state
        .trial_spawners
        .values()
        .filter_map(|spawner| spawner.pending.as_ref())
        .find(|pending| pending.uuid == uuid)
    else {
        return Vec::new();
    };
    vec![
        (
            "item".into(),
            Tag::Compound(
                stack_to_fields(&state.registry.items, &pending.item)
                    .expect("registered ominous item"),
            ),
        ),
        (
            "spawn_item_after_ticks".into(),
            Tag::Long(pending.deadline.saturating_sub(state.tick) as i64),
        ),
    ]
}

#[cfg(test)]
#[path = "trial_spawner_tests.rs"]
mod tests;
