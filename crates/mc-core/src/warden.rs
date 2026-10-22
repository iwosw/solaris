//! Warden anger, animation phases and attacks. Vanilla save keys and spawn data
//! are measured from the 26.1.2 Warden/AngerManagement classes.
use crate::boundary::{EntityDataValue, EntityId};
use crate::effects::{ActiveEffect, EffectId};
use crate::entities::{ColdRow, EntitySlot};
use crate::intent::Intent;
use crate::special_mobs::{Attack, Retained, distance_sq, effect, event, metadata};
use crate::tick::TickState;
use mc_nbt::{ListTag, Tag};
use mc_physics::mth;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[cfg(test)]
thread_local! {
    static VIBRATION_WORK: std::cell::Cell<(usize,usize,usize)> = const { std::cell::Cell::new((0,0,0)) };
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Warden {
    #[serde(deserialize_with = "read_anger_map")]
    pub anger: BTreeMap<uuid::Uuid, u32>,
    pub pose: u8,
    pub phase_until: u64,
    pub calm_until: u64,
    pub vibration_until: u64,
    pub sniff_at: u64,
    pub decay_at: u64,
    pub roaring_target: Option<uuid::Uuid>,
}
impl Default for Warden {
    fn default() -> Self {
        Self {
            anger: BTreeMap::new(),
            pose: 0,
            phase_until: 0,
            calm_until: 1200,
            vibration_until: 0,
            sniff_at: 100,
            decay_at: 20,
            roaring_target: None,
        }
    }
}
pub(crate) fn validate(value: &Warden) -> Result<(), &'static str> {
    if !matches!(value.pose, 0 | 11..=14) {
        return Err("Warden.pose: expected standing/roaring/sniffing/emerging/digging");
    }
    if value.anger.values().any(|value| *value > 150) {
        return Err("Warden.anger: expected 0..=150");
    }
    if [
        value.phase_until,
        value.calm_until,
        value.vibration_until,
        value.sniff_at,
        value.decay_at,
    ]
    .iter()
    .any(|tick| *tick > i64::MAX as u64)
    {
        return Err("Warden.deadline: expected nonnegative signed long");
    }
    Ok(())
}
fn read_anger_map<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<BTreeMap<uuid::Uuid, u32>, D::Error> {
    struct Suspects;
    impl<'de> serde::de::Visitor<'de> for Suspects {
        type Value = BTreeMap<uuid::Uuid, u32>;
        fn expecting(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            out.write_str("unique UUID anger map")
        }
        fn visit_map<A: serde::de::MapAccess<'de>>(
            self,
            mut input: A,
        ) -> Result<Self::Value, A::Error> {
            let mut values = BTreeMap::new();
            while let Some((uuid, anger)) = input.next_entry::<uuid::Uuid, u32>()? {
                if anger > 150 {
                    return Err(serde::de::Error::custom("Warden.anger: expected 0..=150"));
                }
                if values.insert(uuid, anger).is_some() {
                    return Err(serde::de::Error::custom("Warden.anger: duplicate UUID"));
                }
            }
            Ok(values)
        }
    }
    decoder.deserialize_map(Suspects)
}
pub(crate) fn suppress_ai(row: &ColdRow) -> bool {
    matches!(row.special_mob.as_deref(), Some(Retained::Warden(value)) if value.pose != 0 || value.anger.values().all(|anger| *anger < 80))
}
pub(crate) fn emerging(state: &mut TickState, id: EntityId) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    if let Some(Retained::Warden(value)) = state.entities.row_mut(slot).special_mob.as_deref_mut() {
        value.pose = 13;
        value.phase_until = state.tick + 134;
    }
    restore(state, slot);
}
pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    let Some(Retained::Warden(value)) = state.entities.row(slot).special_mob.as_deref() else {
        return;
    };
    let pose = value.pose;
    let anger = value.anger.values().copied().max().unwrap_or(0);
    let (body, row) = state.entities.parts_mut(slot);
    let spawn_data = i32::from(pose == 13);
    if row.appearance.pose != pose || row.appearance.spawn_data != spawn_data {
        let appearance = std::sync::Arc::make_mut(&mut row.appearance);
        appearance.pose = pose;
        appearance.spawn_data = spawn_data;
    }
    body.height = if matches!(pose, 13 | 14) { 1.0 } else { 2.9 };
    metadata(state, slot, 16, EntityDataValue::VarInt(anger as i32));
}
pub(crate) fn darkness(state: &mut TickState, pos: [f64; 3], radius: f64) {
    let targets: Vec<_> = state
        .players
        .iter()
        .filter(|(_, p)| !p.spectator() && !state.session_locked(p.connection))
        .filter_map(|(id, _)| {
            state
                .entities
                .slot_of(*id)
                .filter(|slot| distance_sq(pos, state.entities.pos(*slot)) <= radius * radius)
                .map(|_| *id)
        })
        .collect();
    for target in targets {
        let mut darkness = ActiveEffect::new(EffectId::Darkness, 260, 0);
        darkness.ambient = false;
        darkness.show_particles = false;
        darkness.show_icon = false;
        effect(state, target, darkness);
    }
}
/// Called by the vibration bus only for admitted, non-sneaking game events.
pub(crate) fn vibration(state: &mut TickState, pos: [f64; 3], source: Option<EntityId>) {
    #[cfg(test)]
    VIBRATION_WORK.with(|work| {
        let (calls, rows, rays) = work.get();
        work.set((calls + 1, rows, rays));
    });
    if state.entities.warden_ids().is_empty() {
        return;
    }
    let Some(source) = source else {
        return;
    };
    let Some(source_slot) = state.entities.slot_of(source) else {
        return;
    };
    if state
        .players
        .get(&source)
        .is_some_and(|p| p.creative() || p.spectator() || state.session_locked(p.connection))
    {
        return;
    }
    let uuid = state.entities.row(source_slot).uuid;
    for index in 0..state.entities.warden_ids().len() {
        let id = state.entities.warden_ids()[index];
        let Some(slot) = state.entities.slot_of(id) else {
            continue;
        };
        let from = state.entities.pos(slot);
        if distance_sq(from, pos) > 256.0 {
            continue;
        }
        #[cfg(test)]
        VIBRATION_WORK.with(|work| {
            let (calls, rows, rays) = work.get();
            work.set((calls, rows + 1, rays));
        });
        let eligible = matches!(state.entities.row(slot).special_mob.as_deref(), Some(Retained::Warden(value)) if !matches!(value.pose, 13 | 14) && state.tick >= value.vibration_until);
        if !eligible {
            continue;
        }
        #[cfg(test)]
        VIBRATION_WORK.with(|work| {
            let (calls, rows, rays) = work.get();
            work.set((calls, rows, rays + 1));
        });
        if crate::vibration::occluded(state, from, pos) {
            continue;
        }
        if let Some(Retained::Warden(value)) =
            state.entities.row_mut(slot).special_mob.as_deref_mut()
        {
            let anger = value.anger.entry(uuid).or_default();
            *anger = (*anger + 35).min(150);
            value.calm_until = state.tick + 1200;
            value.vibration_until = state.tick + 40;
        }
        let id = state.entities.body(slot).id;
        event(state, id, 61);
        restore(state, slot);
    }
}
pub(crate) fn run(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    slot: EntitySlot,
    targets: &[(EntityId, [f64; 3])],
    attack: &mut Attack,
) {
    let wpos = state.entities.pos(slot);
    if !state.world.is_active(
        (wpos[0].floor() as i32).div_euclid(16),
        (wpos[2].floor() as i32).div_euclid(16),
    ) {
        return;
    }
    let Some(mut boxed) = state.entities.row_mut(slot).special_mob.take() else {
        return;
    };
    let Retained::Warden(value) = &mut *boxed else {
        return;
    };
    let pos = state.entities.pos(slot);
    if state.tick >= value.decay_at {
        for anger in value.anger.values_mut() {
            *anger = anger.saturating_sub(1);
        }
        value.anger.retain(|_, anger| *anger > 0);
        value.decay_at = state.tick + 20;
    }
    if (state.tick + id.0.unsigned_abs() as u64).is_multiple_of(120) {
        darkness(state, pos, 20.0);
    }
    if value.pose != 0 && state.tick >= value.phase_until {
        if value.pose == 14 {
            state.remove_entity(slot);
            state.hostile.removed.push(id);
            return;
        }
        value.pose = 0;
    }
    let target = targets
        .iter()
        .filter_map(|(id, at)| {
            let slot = state.entities.slot_of(*id)?;
            let uuid = state.entities.row(slot).uuid;
            let anger = value.anger.get(&uuid).copied().unwrap_or(0);
            (anger >= 80).then_some((*id, *at, uuid, anger))
        })
        .max_by_key(|(id, _, _, anger)| (*anger, std::cmp::Reverse(id.0)));
    if let Some((target, at, uuid, _)) = target {
        value.calm_until = state.tick + 1200;
        if value.pose == 0 && value.roaring_target != Some(uuid) {
            value.pose = 11;
            value.phase_until = state.tick + 84;
            value.roaring_target = Some(uuid);
            darkness(state, pos, 20.0);
        } else if value.pose == 0 {
            if distance_sq(pos, at) <= 9.0 && crate::special_mobs::visible(state, slot, at) {
                if state.entities.body(slot).attack_cooldown == 0 {
                    crate::special_mobs::damage(
                        state,
                        intents,
                        id,
                        target,
                        30.0,
                        crate::survival::DamageKind::Melee,
                    );
                    state.entities.body_mut(slot).attack_cooldown = 18;
                    event(state, id, 4);
                }
            } else {
                crate::special_mobs::beam(
                    state,
                    intents,
                    id,
                    slot,
                    crate::blueprint::GoalKind::SonicBoom,
                    &[(target, at)],
                    attack,
                );
            }
        }
    } else if value.pose == 0 {
        value.roaring_target = None;
        if state.tick >= value.calm_until && !state.entities.row(slot).persistent {
            value.pose = 14;
            value.phase_until = state.tick + 100;
        } else if state.tick >= value.sniff_at {
            value.pose = 12;
            value.phase_until = state.tick + 84;
            value.sniff_at = state.tick + 200;
            if let Some((target, _)) = targets
                .iter()
                .filter(|(_, at)| {
                    mth::length2(pos[0] - at[0], pos[2] - at[2]) <= 6.0
                        && (pos[1] - at[1]).abs() <= 20.0
                })
                .min_by(|a, b| distance_sq(pos, a.1).total_cmp(&distance_sq(pos, b.1)))
                && let Some(target_slot) = state.entities.slot_of(*target)
            {
                let anger = value
                    .anger
                    .entry(state.entities.row(target_slot).uuid)
                    .or_default();
                *anger = (*anger + 35).min(150);
                value.calm_until = state.tick + 1200;
            }
        }
    }
    state.entities.row_mut(slot).special_mob = Some(boxed);
    restore(state, slot);
}

pub(crate) fn anger_tag(value: &Warden) -> Tag {
    Tag::Compound(vec![(
        "suspects".into(),
        Tag::List(ListTag {
            element_type: 10,
            elements: value
                .anger
                .iter()
                .map(|(uuid, anger)| {
                    Tag::Compound(vec![
                        (
                            "uuid".into(),
                            crate::special_mobs::persistence::uuid_tag(*uuid),
                        ),
                        ("anger".into(), Tag::Int(*anger as i32)),
                    ])
                })
                .collect(),
        }),
    )])
}
pub(crate) fn read_anger(tag: Option<&Tag>) -> Result<Warden, crate::config::CoreError> {
    let mut value = Warden::default();
    let Some(tag) = tag else {
        return Ok(value);
    };
    let fail = |msg: &str| crate::config::CoreError::Storage(format!("Warden.anger: {msg}"));
    let Tag::Compound(fields) = tag else {
        return Err(fail("expected compound"));
    };
    let suspects = unique(fields, "suspects").map_err(|err| fail(&err))?;
    let Some(suspects) = suspects else {
        return Ok(value);
    };
    let Tag::List(list) = suspects else {
        return Err(fail("suspects: expected list"));
    };
    if list.element_type != 10 && !(list.element_type == 0 && list.elements.is_empty()) {
        return Err(fail("suspects: expected compound list"));
    }
    for entry in &list.elements {
        let Tag::Compound(fields) = entry else {
            return Err(fail("suspects: expected compound"));
        };
        let Some(Tag::IntArray(words)) = unique(fields, "uuid").map_err(|err| fail(&err))? else {
            return Err(fail("suspects.uuid: expected UUID int array"));
        };
        if words.len() != 4 {
            return Err(fail("suspects.uuid: expected four UUID words"));
        }
        let mut bytes = [0u8; 16];
        for (out, word) in bytes.as_chunks_mut::<4>().0.iter_mut().zip(words) {
            out.copy_from_slice(&word.to_be_bytes());
        }
        let Some(Tag::Int(anger)) = unique(fields, "anger").map_err(|err| fail(&err))? else {
            return Err(fail("suspects.anger: expected int"));
        };
        if !(0..=150).contains(anger) {
            return Err(fail("suspects.anger: expected 0..=150"));
        }
        if value
            .anger
            .insert(uuid::Uuid::from_bytes(bytes), *anger as u32)
            .is_some()
        {
            return Err(fail("suspects.uuid: duplicate UUID"));
        }
    }
    Ok(value)
}
pub(crate) fn unique<'a>(
    fields: &'a [(String, Tag)],
    name: &str,
) -> Result<Option<&'a Tag>, String> {
    let mut values = fields.iter().filter(|(key, _)| key == name);
    let first = values.next().map(|(_, tag)| tag);
    if values.next().is_some() {
        return Err(format!("{name}: duplicate field"));
    }
    Ok(first)
}
#[cfg(test)]
#[path = "warden_tests.rs"]
mod tests;
