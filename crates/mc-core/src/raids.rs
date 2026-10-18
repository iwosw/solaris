//! Raid owner: omen custody, wave membership and native saved-data post-images.
mod persistence;
#[cfg(test)]
#[path = "raids_tests.rs"]
pub(crate) mod tests;

use crate::boundary::ChangeSet;
use crate::effects::{ActiveEffect, EffectId};
use crate::tick::TickState;
pub(crate) use persistence::{from_nbt, to_nbt};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Raid {
    pub center: [i32; 3],
    pub started: bool,
    pub active: bool,
    pub ticks_active: u64,
    pub raid_omen_level: i32,
    pub groups_spawned: i32,
    pub cooldown_ticks: i32,
    pub post_raid_ticks: i32,
    pub total_health: f32,
    pub group_count: i32,
    pub status: String,
    pub heroes: BTreeSet<Uuid>,
    pub members: BTreeSet<Uuid>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PendingOmen {
    pub center: [i32; 3],
    pub deadline: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Raids {
    pub next_id: i32,
    pub tick: i32,
    /// World tick through which the retained native counters have advanced.
    /// Missing on a vanilla import, whose counters start at the import boundary.
    #[serde(default)]
    pub clock_tick: Option<u64>,
    pub raids: BTreeMap<i32, Raid>,
    pub pending: BTreeMap<Uuid, PendingOmen>,
}
impl Default for Raids {
    fn default() -> Self {
        Self {
            next_id: 1,
            tick: 0,
            clock_tick: None,
            raids: BTreeMap::new(),
            pending: BTreeMap::new(),
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub struct RaidBar {
    pub uuid: Uuid,
    pub center: [i32; 3],
    pub progress: f32,
    pub name: &'static str,
}

fn distance(pos: [f64; 3], center: [i32; 3]) -> f64 {
    pos.into_iter()
        .zip(center)
        .map(|(a, b)| (a - f64::from(b)).powi(2))
        .sum()
}
fn village_center(state: &TickState, pos: [f64; 3]) -> Option<[i32; 3]> {
    // Claimed home/meeting memories are the existing occupied village POIs.
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let row = state.entities.row(slot);
            let villager = row.villager.as_ref()?;
            if !state.entities.body(slot).alive
                || row.health <= 0.0
                || villager.conversion_time.is_some()
            {
                return None;
            }
            let home = villager.home?;
            if villager.home_dimension != state.dimension.name() {
                return None;
            }
            let block = state.world.block_at(home.x, home.y, home.z)?;
            if !state
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(block))?
                .block
                .id
                .path()
                .ends_with("_bed")
            {
                return None;
            }
            let center = villager.meeting_point.unwrap_or(home);
            let center = [center.x, center.y, center.z];
            (distance(pos, center) < 4096.0).then_some(center)
        })
        .min_by(|a, b| distance(pos, *a).total_cmp(&distance(pos, *b)))
}
fn residents(state: &TickState, center: [i32; 3]) -> bool {
    state.entities.slots().any(|slot| {
        let row = state.entities.row(slot);
        state.entities.body(slot).alive
            && row.health > 0.0
            && row
                .villager
                .as_ref()
                .is_some_and(|v| v.conversion_time.is_none())
            && distance(state.entities.pos(slot), center) < 9216.0
    })
}

pub(crate) fn raid_omens(state: &mut TickState) {
    if state.dimension != crate::dimensions::Dimension::Overworld {
        return;
    }
    let ids: Vec<_> = state.players.keys().copied().collect();
    for id in ids {
        let Some(slot) = state.entities.slot_of(id) else {
            continue;
        };
        let uuid = state.entities.row(slot).uuid;
        let pos = state.entities.pos(slot);
        let allowed = crate::command_rules::boolean(state, "raids")
            && state.difficulty != crate::survival::Difficulty::Peaceful
            && !state.players[&id].spectator();
        let bad = state.players[&id]
            .active_effects
            .get(EffectId::BadOmen)
            .cloned();
        if allowed
            && let Some(bad) = bad
            && let Some(center) = village_center(state, pos)
        {
            let player = state.players.get_mut(&id).unwrap();
            let mut changes = player.clear_effects(Some(EffectId::BadOmen));
            changes.extend(player.give_effect(ActiveEffect::new(
                EffectId::RaidOmen,
                600,
                bad.amplifier,
            )));
            let connection = player.connection;
            state.raids.pending.insert(
                uuid,
                PendingOmen {
                    center,
                    deadline: state.tick + 600,
                },
            );
            for change in changes {
                state.push_player(connection, change);
            }
            crate::effects::sync_appearance(state, id);
        }
        let omen = state.players[&id]
            .active_effects
            .get(EffectId::RaidOmen)
            .cloned();
        if let Some(pending) = state.raids.pending.get(&uuid).cloned() {
            if omen.is_none() || !allowed {
                state.raids.pending.remove(&uuid);
            } else if let Some(omen) = omen
                && (state.tick >= pending.deadline || omen.duration == 1)
            {
                let level = i32::from(omen.amplifier) + 1;
                state.raids.pending.remove(&uuid);
                let player = state.players.get_mut(&id).unwrap();
                let changes = player.clear_effects(Some(EffectId::RaidOmen));
                let connection = player.connection;
                for change in changes {
                    state.push_player(connection, change);
                }
                crate::effects::sync_appearance(state, id);
                start(state, pending.center, uuid, level);
            }
        }
    }
}
fn start(state: &mut TickState, center: [i32; 3], hero: Uuid, level: i32) -> bool {
    if !residents(state, center) {
        return false;
    }
    if let Some(raid) = state
        .raids
        .raids
        .values_mut()
        .find(|r| r.status == "ongoing" && distance(center.map(f64::from), r.center) < 9216.0)
    {
        raid.raid_omen_level = (raid.raid_omen_level + level).min(5);
        raid.heroes.insert(hero);
        if raid.groups_spawned == 0
            && let Some(slot) = state.entities.slot_of_uuid(hero)
            && let Some(player) = state.players.get_mut(&state.entities.body(slot).id)
        {
            player
                .statistics
                .increment_custom(mc_data::statistics::CustomStat::RaidTrigger, 1);
        }
        return true;
    }
    let Some(id) = state.raids.next_id.checked_add(1) else {
        return false;
    };
    state.raids.next_id = id;
    let group_count = match state.difficulty {
        crate::survival::Difficulty::Easy => 3,
        crate::survival::Difficulty::Normal => 5,
        crate::survival::Difficulty::Hard => 7,
        _ => 0,
    };
    state.raids.raids.insert(
        id,
        Raid {
            center,
            started: false,
            active: true,
            ticks_active: 0,
            raid_omen_level: level.clamp(1, 5),
            groups_spawned: 0,
            cooldown_ticks: 300,
            post_raid_ticks: 0,
            total_health: 0.0,
            group_count,
            status: "ongoing".into(),
            heroes: [hero].into_iter().collect(),
            members: BTreeSet::new(),
        },
    );
    if let Some(slot) = state.entities.slot_of_uuid(hero)
        && let Some(player) = state.players.get_mut(&state.entities.body(slot).id)
    {
        player
            .statistics
            .increment_custom(mc_data::statistics::CustomStat::RaidTrigger, 1);
    }
    true
}
const WAVES: &[(&str, [usize; 8])] = &[
    ("vindicator", [0, 0, 2, 0, 1, 4, 2, 5]),
    ("evoker", [0, 0, 0, 0, 0, 1, 1, 2]),
    ("pillager", [0, 4, 3, 3, 4, 4, 4, 2]),
    ("witch", [0, 0, 0, 0, 3, 0, 0, 1]),
    ("ravager", [0, 0, 0, 1, 0, 1, 0, 2]),
];
fn spawn_wave(state: &mut TickState, raid: &mut Raid, changes: &mut ChangeSet) -> bool {
    let wave = (raid.groups_spawned + 1).min(raid.group_count) as usize;
    let center = raid.center;
    let at = [
        f64::from(center[0]) + 16.5,
        f64::from(center[1]),
        f64::from(center[2]) + 0.5,
    ];
    if state
        .world
        .block_at(at[0].floor() as i32, center[1], center[2])
        .is_none()
    {
        return false;
    }
    raid.total_health = 0.0;
    let mut index = 0;
    for &(name, counts) in WAVES {
        let ident = mc_data::Identifier::parse(format!("minecraft:{name}")).expect("raider id");
        let type_id = state
            .registry
            .entity_types
            .id_of(&ident)
            .expect("vanilla raider");
        let health = state
            .registry
            .blueprint(type_id)
            .expect("raider blueprint")
            .params
            .max_health;
        for _ in 0..counts[wave] {
            let id = state.spawn_mob(
                type_id,
                [
                    at[0] + f64::from(index % 4),
                    at[1],
                    at[2] + f64::from(index / 4),
                ],
                health,
            );
            let slot = state.entities.slot_of(id).unwrap();
            state.entities.row_mut(slot).persistent = true;
            raid.members.insert(state.entities.row(slot).uuid);
            raid.total_health += health;
            changes.entities_spawned.push(id);
            index += 1;
        }
    }
    raid.groups_spawned += 1;
    raid.started = true;
    true
}
pub(crate) fn run(state: &mut TickState, changes: &mut ChangeSet) {
    if state.dimension != crate::dimensions::Dimension::Overworld {
        return;
    }
    gifts(state, changes);
    if state.raids.raids.is_empty() && state.raids.pending.is_empty() {
        return;
    }
    state.raids.tick = state.raids.tick.wrapping_add(1);
    // run_dimension_tick publishes tick + 1 after this owner has advanced.
    state.raids.clock_tick = Some(state.tick + 1);
    let enabled = crate::command_rules::boolean(state, "raids")
        && state.difficulty != crate::survival::Difficulty::Peaceful;
    let mut raids = std::mem::take(&mut state.raids.raids);
    for raid in raids.values_mut() {
        if !enabled {
            raid.status = "stopped".into();
            raid.active = false;
            continue;
        }
        // Entity death/removal is durable even while the village chunk is absent.
        raid.members.retain(|uuid| {
            state
                .entities
                .slot_of_uuid(*uuid)
                .is_some_and(|s| state.entities.body(s).alive && state.entities.row(s).health > 0.0)
        });
        if raid.status != "ongoing" {
            raid.post_raid_ticks += 1;
            continue;
        }
        raid.active = state
            .world
            .is_active(raid.center[0].div_euclid(16), raid.center[2].div_euclid(16))
            && state
                .world
                .block_at(raid.center[0], raid.center[1], raid.center[2])
                .is_some();
        raid.ticks_active += 1;
        if !residents(state, raid.center) {
            raid.status = "loss".into();
            raid.post_raid_ticks = 0;
            continue;
        }
        if raid.ticks_active >= 48000 {
            raid.status = "stopped".into();
            raid.active = false;
            continue;
        }
        if !raid.members.is_empty() {
            continue;
        }
        let waves = raid.group_count + i32::from(raid.raid_omen_level > 1);
        if raid.groups_spawned >= waves {
            raid.post_raid_ticks += 1;
            if raid.post_raid_ticks >= 40 {
                raid.status = "victory".into();
                raid.post_raid_ticks = 0;
                for uuid in &raid.heroes {
                    if let Some(slot) = state.entities.slot_of_uuid(*uuid) {
                        let id = state.entities.body(slot).id;
                        if let Some(player) = state.players.get_mut(&id) {
                            if !player.spectator() {
                                player
                                    .statistics
                                    .increment_custom(mc_data::statistics::CustomStat::RaidWin, 1);
                            }
                            let connection = player.connection;
                            let out = player.give_effect(ActiveEffect::new(
                                EffectId::HeroOfTheVillage,
                                48000,
                                (raid.raid_omen_level - 1) as u8,
                            ));
                            let changed = !out.is_empty();
                            for change in out {
                                state.push_player(connection, change);
                            }
                            crate::effects::sync_appearance(state, id);
                            if changed {
                                crate::advancements::trigger(
                                    state,
                                    id,
                                    "minecraft:hero_of_the_village",
                                    serde_json::json!({ "raid_omen_level": raid.raid_omen_level }),
                                );
                            }
                        }
                    }
                }
            }
        } else {
            if raid.cooldown_ticks > 0 {
                raid.cooldown_ticks -= 1;
            }
            if raid.cooldown_ticks == 0 && spawn_wave(state, raid, changes) {
                raid.cooldown_ticks = 300;
            }
        }
    }
    raids
        .retain(|_, r| r.status != "stopped" && (r.status == "ongoing" || r.post_raid_ticks < 600));
    state.raids.raids = raids;
}
pub(crate) fn bars(state: &TickState) -> Vec<RaidBar> {
    state
        .raids
        .raids
        .iter()
        .filter(|(_, r)| r.active)
        .map(|(&id, r)| {
            let health: f32 = r
                .members
                .iter()
                .filter_map(|u| state.entities.slot_of_uuid(*u))
                .map(|s| state.entities.row(s).health.max(0.0))
                .sum();
            RaidBar {
                uuid: Uuid::from_u128(0x72616964000000000000000000000000 | id as u128),
                center: r.center,
                progress: if r.members.is_empty() && r.status == "ongoing" {
                    1.0 - r.cooldown_ticks as f32 / 300.0
                } else {
                    health / r.total_health.max(1.0)
                },
                name: match r.status.as_str() {
                    "victory" => "event.minecraft.raid.victory.full",
                    "loss" => "event.minecraft.raid.defeat.full",
                    _ => "event.minecraft.raid",
                },
            }
        })
        .collect()
}
/// Timers advance deterministically between durable phase changes. Comparing
/// only phase/custody avoids a full raid post-image for each countdown tick.
fn same_phase(left: &Raids, right: &Raids) -> bool {
    left.next_id == right.next_id
        && left.pending == right.pending
        && left.raids.len() == right.raids.len()
        && left
            .raids
            .iter()
            .zip(&right.raids)
            .all(|((&id, a), (&other, b))| {
                id == other
                    && a.center == b.center
                    && a.started == b.started
                    && a.active == b.active
                    && a.raid_omen_level == b.raid_omen_level
                    && a.groups_spawned == b.groups_spawned
                    && a.total_health == b.total_health
                    && a.group_count == b.group_count
                    && a.status == b.status
                    && a.heroes == b.heroes
                    && a.members == b.members
            })
}
pub(crate) fn journal(state: &mut TickState, entries: &mut Vec<crate::journal::JournalEntry>) {
    if state.dimension == crate::dimensions::Dimension::Overworld
        && !state
            .journaled_raids
            .as_ref()
            .is_some_and(|image| same_phase(image, &state.raids))
    {
        entries.push(crate::journal::JournalEntry::Raids {
            value: state.raids.clone(),
        });
        state.journaled_raids = Some(state.raids.clone());
    }
}
fn gifts(state: &mut TickState, changes: &mut ChangeSet) {
    // Only this cadence can produce gifts; the other 5,999 ticks do no census.
    if !state.tick.is_multiple_of(6000) {
        return;
    }
    let mut gifts = BTreeMap::new();
    for (&hero, player) in &state.players {
        if !player.active_effects.has(EffectId::HeroOfTheVillage) {
            continue;
        }
        let Some(hero_slot) = state.entities.slot_of(hero) else {
            continue;
        };
        let at = state.entities.pos(hero_slot);
        for slot in state
            .entities
            .slots()
            .filter(|&slot| super::special_mobs::distance_sq(state.entities.pos(slot), at) < 25.0)
        {
            let row = state.entities.row(slot);
            let Some(villager) = row.villager.as_ref() else {
                continue;
            };
            if state.entities.body(slot).alive
                && villager.age >= 0
                && villager.conversion_time.is_none()
            {
                // A villager near two heroes still has custody of one gift.
                gifts
                    .entry(state.entities.body(slot).id)
                    .or_insert((state.entities.pos(slot), villager.data.profession.name()));
            }
        }
    }
    for (_, (pos, profession)) in gifts {
        let item = match profession {
            "armorer" => "iron_helmet",
            "butcher" => "cooked_porkchop",
            "cartographer" => "paper",
            "cleric" => "redstone",
            "farmer" => "bread",
            "fisherman" => "cod",
            "fletcher" => "arrow",
            "leatherworker" => "leather",
            "librarian" => "book",
            "mason" => "clay_ball",
            "shepherd" => "white_wool",
            "toolsmith" => "stone_pickaxe",
            "weaponsmith" => "stone_axe",
            _ => "wheat_seeds",
        };
        if let Some(item) = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{item}")).unwrap())
        {
            let id = crate::tick::spawn_item_entity(state, pos, mc_data::ItemStack::new(item, 1));
            changes.entities_spawned.push(id);
        }
    }
}

pub(crate) fn deserialize<'de, D: serde::Deserializer<'de>>(decoder: D) -> Result<Raids, D::Error> {
    let image = Raids::deserialize(decoder)?;
    image.validate().map_err(serde::de::Error::custom)?;
    Ok(image)
}

pub(crate) fn hydrate_native_entity(
    raids: &mut Raids,
    entity: &mc_data::entity_nbt::VanillaEntity,
) -> Result<(), String> {
    let field = |key: &str| -> Result<Option<&mc_nbt::Tag>, String> {
        let mut fields = entity.extra_fields.iter().filter(|(name, _)| name == key);
        let value = fields.next().map(|(_, value)| value);
        if fields.next().is_some() {
            return Err(format!("duplicate raider field {key}"));
        }
        Ok(value)
    };
    let Some(tag) = field("RaidId")? else {
        return Ok(());
    };
    let mc_nbt::Tag::Int(id) = tag else {
        return Err("invalid raider RaidId".into());
    };
    let raid = raids
        .raids
        .get_mut(id)
        .ok_or_else(|| format!("raider RaidId {id} has no saved raid"))?;
    let wave = match field("Wave")? {
        Some(mc_nbt::Tag::Int(wave)) => *wave,
        _ => return Err("invalid raider Wave".into()),
    };
    if wave <= 0 || wave > raid.groups_spawned {
        return Err(format!("raider RaidId {id}: invalid Wave {wave}"));
    }
    raid.members.insert(entity.uuid);
    Ok(())
}

pub(crate) fn entity_fields(
    state: &TickState,
    slot: crate::entities::EntitySlot,
) -> Vec<(String, mc_nbt::Tag)> {
    let row = state.entities.row(slot);
    if !crate::special_mobs::raiders::owns(state.entities.body(slot).kind) {
        return Vec::new();
    }
    let member = state
        .raids
        .raids
        .iter()
        .find(|(_, raid)| raid.members.contains(&row.uuid));
    let mut fields = vec![
        (
            "Wave".into(),
            mc_nbt::Tag::Int(member.map_or(0, |(_, raid)| raid.groups_spawned)),
        ),
        ("CanJoinRaid".into(), mc_nbt::Tag::Byte(1)),
    ];
    if let Some((&id, _)) = member {
        fields.push(("RaidId".into(), mc_nbt::Tag::Int(id)));
    }
    fields
}

pub(crate) fn restore(state: &mut TickState) -> Result<(), crate::config::CoreError> {
    state.raids.validate()?;
    if let Some(captured) = state.raids.clock_tick {
        let elapsed = state.tick.checked_sub(captured).ok_or_else(|| {
            crate::config::CoreError::Storage(
                "raids: clock_tick exceeds recovered world tick".into(),
            )
        })?;
        if !state.raids.raids.is_empty() || !state.raids.pending.is_empty() {
            state.raids.tick = state.raids.tick.wrapping_add(elapsed as i32);
        }
        for raid in state.raids.raids.values_mut() {
            if raid.status != "ongoing" {
                raid.post_raid_ticks = raid
                    .post_raid_ticks
                    .saturating_add(elapsed.min(i32::MAX as u64) as i32);
            } else if raid.active {
                raid.ticks_active = raid.ticks_active.saturating_add(elapsed);
                if raid.members.is_empty() {
                    if raid.groups_spawned >= raid.group_count + i32::from(raid.raid_omen_level > 1)
                    {
                        raid.post_raid_ticks = raid
                            .post_raid_ticks
                            .saturating_add(elapsed.min(i32::MAX as u64) as i32);
                    } else {
                        raid.cooldown_ticks = raid
                            .cooldown_ticks
                            .saturating_sub(elapsed.min(i32::MAX as u64) as i32)
                            .max(0);
                    }
                }
            }
        }
        state.raids.clock_tick = Some(state.tick);
    }
    // Native raids discover loaded members lazily. Missing/dead entities at a
    // restart boundary are ordinary lifecycle state, not malformed saved data.
    for raid in state.raids.raids.values_mut() {
        raid.members.retain(|uuid| {
            state.entities.slot_of_uuid(*uuid).is_some_and(|slot| {
                let row = state.entities.row(slot);
                state.entities.body(slot).alive
                    && row.health > 0.0
                    && crate::special_mobs::raiders::owns(state.entities.body(slot).kind)
            })
        });
    }
    Ok(())
}

pub(crate) fn killed(
    state: &mut TickState,
    victim: crate::boundary::EntityId,
    killer: Option<crate::boundary::EntityId>,
) {
    let Some(killer) = killer.filter(|id| state.players.contains_key(id)) else {
        return;
    };
    let Some(victim) = state
        .entities
        .slot_of_any(victim)
        .map(|slot| state.entities.row(slot).uuid)
    else {
        return;
    };
    let Some(hero) = state
        .entities
        .slot_of(killer)
        .map(|slot| state.entities.row(slot).uuid)
    else {
        return;
    };
    if let Some(raid) = state
        .raids
        .raids
        .values_mut()
        .find(|raid| raid.members.contains(&victim))
    {
        raid.heroes.insert(hero);
    }
}
