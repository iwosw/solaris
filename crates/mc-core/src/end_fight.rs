//! EnderDragonFight authority. Fight transitions and their entities/blocks share
//! the dimension's atomic WAL batch; native saved data is a checkpoint domain.
//! Wire phase ids and saved-data keys follow the local 26.1.2 server bytecode.
mod flight;
mod persistence;
mod pillars;
#[cfg(test)]
mod tests;

use crate::boundary::{ChangeSet, EntityDataValue, EntityId};
use crate::entities::EntitySlot;
use crate::intent::{Footprint, Intent, IntentKind};
use crate::tick::TickState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub(crate) use persistence::{
    entity_fields, from_nbt, hydrate_native_entity, native_appearance, to_nbt,
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[repr(i32)]
pub enum Phase {
    #[default]
    Circling = 0,
    Strafing = 1,
    LandingApproach = 2,
    Landing = 3,
    Takeoff = 4,
    PerchingFlaming = 5,
    PerchingScanning = 6,
    PerchingAttacking = 7,
    Charging = 8,
    Dying = 9,
    Hover = 10,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Crystal {
    pub uuid: Uuid,
    pub position: [f64; 3],
    pub pillar: Option<u8>,
    pub beam: Option<[i32; 3]>,
    pub invulnerable: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Respawn {
    pub crystals: [Uuid; 4],
    /// 0=start, 1=preparing, 2=pillars, 3=dragon (vanilla stage order).
    pub stage: u8,
    pub time: u16,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "Image")]
pub struct Fight {
    pub initialized: bool,
    pub dragon_uuid: Option<Uuid>,
    pub dragon_killed: bool,
    pub previously_killed: bool,
    pub exit_portal: [i32; 3],
    pub crystals: Vec<Crystal>,
    /// Remaining gateway indices, popped on a completed death.
    pub gateways: Vec<u8>,
    pub phase: Phase,
    pub phase_ticks: u16,
    pub cycle: u32,
    pub target: Option<Uuid>,
    pub fly_target: [f64; 3],
    pub yaw_accel: f32,
    /// Newest-first vanilla multipart flight samples (y, yaw).
    pub flight_history: [[f64; 2]; 17],
    pub healing_crystal: Option<Uuid>,
    pub perched_damage: f32,
    pub flame_count: u8,
    pub fireball_charge: u8,
    pub death_ticks: u16,
    pub respawn: Option<Respawn>,
    pub flame_cloud: Option<Uuid>,
    /// Startup-only marker: hydrate vanilla phase metadata before publication.
    #[serde(skip)]
    native_import: bool,
}

// Keep the validated serde boundary on every nested WAL/manifest image.
#[derive(Deserialize)]
struct Image {
    initialized: bool,
    dragon_uuid: Option<Uuid>,
    dragon_killed: bool,
    previously_killed: bool,
    exit_portal: [i32; 3],
    crystals: Vec<Crystal>,
    gateways: Vec<u8>,
    phase: Phase,
    phase_ticks: u16,
    cycle: u32,
    target: Option<Uuid>,
    fly_target: [f64; 3],
    yaw_accel: f32,
    flight_history: [[f64; 2]; 17],
    healing_crystal: Option<Uuid>,
    perched_damage: f32,
    flame_count: u8,
    fireball_charge: u8,
    death_ticks: u16,
    respawn: Option<Respawn>,
    flame_cloud: Option<Uuid>,
}
impl TryFrom<Image> for Fight {
    type Error = String;
    fn try_from(v: Image) -> Result<Self, String> {
        let value = Self {
            initialized: v.initialized,
            dragon_uuid: v.dragon_uuid,
            dragon_killed: v.dragon_killed,
            previously_killed: v.previously_killed,
            exit_portal: v.exit_portal,
            crystals: v.crystals,
            gateways: v.gateways,
            phase: v.phase,
            phase_ticks: v.phase_ticks,
            cycle: v.cycle,
            target: v.target,
            fly_target: v.fly_target,
            yaw_accel: v.yaw_accel,
            flight_history: v.flight_history,
            healing_crystal: v.healing_crystal,
            perched_damage: v.perched_damage,
            flame_count: v.flame_count,
            fireball_charge: v.fireball_charge,
            death_ticks: v.death_ticks,
            respawn: v.respawn,
            flame_cloud: v.flame_cloud,
            native_import: false,
        };
        value.validate()?;
        Ok(value)
    }
}
impl Default for Fight {
    fn default() -> Self {
        Self {
            initialized: false,
            dragon_uuid: None,
            dragon_killed: false,
            previously_killed: false,
            exit_portal: [0, 64, 0],
            crystals: Vec::new(),
            gateways: (0..20).collect(),
            phase: Phase::Circling,
            phase_ticks: 0,
            cycle: 0,
            target: None,
            fly_target: [0.0, 128.0, 0.0],
            yaw_accel: 0.0,
            flight_history: [[128.0, 0.0]; 17],
            healing_crystal: None,
            perched_damage: 0.0,
            flame_count: 0,
            fireball_charge: 0,
            death_ticks: 0,
            respawn: None,
            flame_cloud: None,
            native_import: false,
        }
    }
}
impl Fight {
    pub fn validate(&self) -> Result<(), String> {
        let error = |text: &str| format!("DragonFight: {text}");
        if self.fly_target.iter().any(|v| !v.is_finite())
            || !self.perched_damage.is_finite()
            || self.perched_damage < 0.0
            || !self.yaw_accel.is_finite()
            || self.flight_history.iter().flatten().any(|v| !v.is_finite())
        {
            return Err(error("non-finite flight target or invalid perched damage"));
        }
        if self.death_ticks > 200 || self.flame_count > 4 || self.fireball_charge > 5 {
            return Err(error(
                "death_ticks exceeds 200, flame_count exceeds 4 or fireball_charge exceeds 5",
            ));
        }
        if self.initialized && !self.dragon_killed && self.dragon_uuid.is_none() {
            return Err(error("initialized live fight is missing dragon_uuid"));
        }
        if self.flame_cloud.is_some() && self.phase != Phase::PerchingFlaming {
            return Err(error("flame_cloud retained outside perching flaming phase"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for crystal in &self.crystals {
            if !seen.insert(crystal.uuid) {
                return Err(error("duplicate crystal UUID"));
            }
            if crystal.position.iter().any(|v| !v.is_finite()) {
                return Err(error("non-finite crystal position"));
            }
            if crystal.pillar.is_some_and(|index| index >= 10) {
                return Err(error("crystal pillar index outside 0..10"));
            }
        }
        let mut indices = std::collections::BTreeSet::new();
        if self
            .gateways
            .iter()
            .any(|i| *i >= 20 || !indices.insert(*i))
        {
            return Err(error("gateway index outside 0..20 or duplicate index"));
        }
        if let Some(respawn) = &self.respawn {
            if !self.dragon_killed
                || respawn.stage > 3
                || respawn.time > [0, 100, 400, 100][usize::from(respawn.stage.min(3))]
            {
                return Err(error("invalid respawn stage/time or dragon not killed"));
            }
            let ids: std::collections::BTreeSet<_> = respawn.crystals.into_iter().collect();
            if ids.len() != 4 || !ids.iter().all(|id| seen.contains(id)) {
                return Err(error(
                    "respawn requires four distinct retained crystal UUIDs",
                ));
            }
        }
        Ok(())
    }
    fn phase(&mut self, phase: Phase) {
        if self.phase != phase {
            self.phase = phase;
            self.phase_ticks = 0;
            self.fireball_charge = 0;
        }
    }
}

pub(crate) fn type_id(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).expect("canonical entity"))
        .expect("vanilla dragon registry")
}
pub(crate) fn is_dragon(state: &TickState, slot: EntitySlot) -> bool {
    matches!(state.entities.body(slot).kind, crate::entities::EntityKind::Mob {type_id: id}
        if id == type_id(state,"minecraft:ender_dragon"))
}
pub(crate) fn is_crystal(state: &TickState, slot: EntitySlot) -> bool {
    matches!(state.entities.body(slot).kind, crate::entities::EntityKind::Mob {type_id: id}
        if id == type_id(state,"minecraft:end_crystal"))
}
fn dragon_slot(state: &TickState) -> Option<EntitySlot> {
    state
        .end_fight
        .dragon_uuid
        .and_then(|uuid| state.entities.slot_of_uuid(uuid))
}
pub(crate) fn metadata(state: &mut TickState, slot: EntitySlot, index: u8, value: EntityDataValue) {
    let (body, row) = state.entities.parts_mut(slot);
    let appearance = std::sync::Arc::make_mut(&mut row.appearance);
    if appearance
        .extra
        .iter()
        .any(|(i, v)| *i == index && *v == value)
    {
        return;
    }
    appearance.extra.retain(|(i, _)| *i != index);
    appearance.extra.push((index, value));
    state.appearance_changed.push(body.id);
}
fn spawn_dragon(state: &mut TickState, changes: &mut ChangeSet) {
    let id = state.spawn_mob(
        type_id(state, "minecraft:ender_dragon"),
        [0.0, 128.0, 0.0],
        200.0,
    );
    let slot = state.entities.slot_of(id).expect("dragon spawned");
    state.entities.row_mut(slot).persistent = true;
    state.end_fight.dragon_uuid = Some(state.entities.row(slot).uuid);
    state.end_fight.dragon_killed = false;
    state.end_fight.phase = Phase::Circling;
    state.end_fight.phase_ticks = 0;
    state.end_fight.death_ticks = 0;
    state.end_fight.target = None;
    state.end_fight.flame_count = 0;
    state.end_fight.fireball_charge = 0;
    state.end_fight.yaw_accel = 0.0;
    state.end_fight.flight_history = [[128.0, 0.0]; 17];
    metadata(state, slot, 16, EntityDataValue::VarInt(0));
    changes.entities_spawned.push(id);
}

/// Retain the dragon's live row during the native 200-tick death animation;
/// generic mob loot/removal must not race the fight's XP/portal transition.
pub(crate) fn begin_death(state: &mut TickState, slot: EntitySlot) -> bool {
    if !is_dragon(state, slot) {
        return false;
    }
    let row = state.entities.row_mut(slot);
    row.health = 0.0;
    std::sync::Arc::make_mut(&mut row.appearance).health = 0.0;
    state.end_fight.phase(Phase::Dying);
    true
}
pub(crate) fn initialize_entity(state: &mut TickState, slot: EntitySlot) {
    if is_dragon(state, slot) {
        // Client EnderDragon creates eight children at parent wire-id + 1..8.
        for _ in 0..8 {
            state.entities.alloc_id();
        }
        state.entities.row_mut(slot).persistent = true;
        metadata(state, slot, 16, EntityDataValue::VarInt(0));
    } else if is_crystal(state, slot) {
        state.entities.row_mut(slot).persistent = true;
        metadata(state, slot, 8, EntityDataValue::OptionalBlockPos(None));
        metadata(state, slot, 9, EntityDataValue::Boolean(true));
    }
}

pub(crate) fn run(state: &mut TickState, intents: &mut Vec<Intent>, changes: &mut ChangeSet) {
    if state.dimension != crate::dimensions::Dimension::End || state.players.is_empty() {
        return;
    }
    if !state.end_fight.initialized {
        if !pillars::ready(state) {
            return;
        }
        state.end_fight.initialized = true;
        state.end_fight.gateways = pillars::shuffle::<20>(state.world.seed()).to_vec();
        crate::end_portals::set_exit_portal(state, changes, false);
        state.end_fight.exit_portal = crate::end_portals::exit_portal_position(state);
        for index in 0..10 {
            pillars::build(state, changes, index);
        }
        spawn_dragon(state, changes);
    }
    respawn_effects(state, changes);
    if state.end_fight.respawn.is_some() || state.end_fight.dragon_killed {
        return;
    }
    if let Some(slot) = dragon_slot(state)
        && state.end_fight.phase != Phase::Dying
    {
        let dpos = state.entities.pos(slot);
        if state.world.is_active(
            (dpos[0].floor() as i32).div_euclid(16),
            (dpos[2].floor() as i32).div_euclid(16),
        ) {
            flight::run(state, intents, slot);
        }
    }
}

pub(crate) fn commit(state: &mut TickState, changes: &mut ChangeSet) {
    if state.dimension != crate::dimensions::Dimension::End {
        return;
    }
    if let Some(uuid) = state.end_fight.flame_cloud {
        if let Some(slot) = state.entities.slot_of_uuid(uuid) {
            if state.end_fight.phase != Phase::PerchingFlaming {
                let id = state.entities.body(slot).id;
                state.remove_entity(slot);
                changes.entities_removed.push(id);
                state.end_fight.flame_cloud = None;
            }
        } else {
            state.end_fight.flame_cloud = None;
        }
    }
    if !state.end_fight.initialized || state.players.is_empty() {
        return;
    }
    let was_respawning = state.end_fight.respawn.is_some();
    sync_crystals(state);
    if was_respawning && state.end_fight.respawn.is_none() {
        crate::end_portals::set_exit_portal(state, changes, true);
    }
    if state.end_fight.respawn.is_some() {
        respawn_tick(state, changes);
    } else if state.end_fight.dragon_killed {
        try_respawn(state, changes);
    } else if let Some(slot) = dragon_slot(state)
        && state.end_fight.phase == Phase::Dying
    {
        death_tick(state, changes, slot);
    }
    sync_crystals(state);
}

fn sync_crystals(state: &mut TickState) {
    let healing_lost = state
        .end_fight
        .healing_crystal
        .is_some_and(|uuid| state.entities.slot_of_uuid(uuid).is_none());
    if healing_lost {
        state.end_fight.healing_crystal = None;
        if let Some(slot) = dragon_slot(state) {
            let (body, row) = state.entities.parts_mut(slot);
            row.health = (row.health - 10.0).max(0.0);
            std::sync::Arc::make_mut(&mut row.appearance).health = row.health;
            state.appearance_changed.push(body.id);
            if row.health == 0.0 {
                begin_death(state, slot);
            }
            damage_committed(state, slot, 10.0);
        }
    }
    if state.end_fight.respawn.as_ref().is_some_and(|r| {
        r.crystals
            .iter()
            .any(|id| state.entities.slot_of_uuid(*id).is_none())
    }) {
        state.end_fight.respawn = None;
        for c in &mut state.end_fight.crystals {
            c.invulnerable = false;
            c.beam = None;
        }
    }
    state
        .end_fight
        .crystals
        .retain(|c| state.entities.slot_of_uuid(c.uuid).is_some());
    for index in 0..state.entities.len() {
        let slot = EntitySlot(index as u32);
        if !is_crystal(state, slot) {
            continue;
        }
        let row = state.entities.row(slot);
        if state.end_fight.crystals.iter().any(|c| c.uuid == row.uuid) {
            continue;
        }
        state.end_fight.crystals.push(Crystal {
            uuid: row.uuid,
            position: state.entities.pos(slot),
            pillar: None,
            beam: row.appearance.extra.iter().find_map(|(i, v)| match (i, v) {
                (8, EntityDataValue::OptionalBlockPos(v)) => *v,
                _ => None,
            }),
            invulnerable: row.controls.invulnerable,
        });
    }
    for index in 0..state.end_fight.crystals.len() {
        let crystal = &state.end_fight.crystals[index];
        let (uuid, beam, invulnerable) = (crystal.uuid, crystal.beam, crystal.invulnerable);
        if let Some(slot) = state.entities.slot_of_uuid(uuid) {
            state.end_fight.crystals[index].position = state.entities.pos(slot);
            metadata(state, slot, 8, EntityDataValue::OptionalBlockPos(beam));
            state.entities.row_mut(slot).controls.invulnerable = invulnerable;
        }
    }
}

fn death_tick(state: &mut TickState, changes: &mut ChangeSet, slot: EntitySlot) {
    state.end_fight.death_ticks += 1;
    let time = state.end_fight.death_ticks;
    let mut at = state.entities.pos(slot);
    at[1] += 0.1;
    state.entities.set_pos(slot, at);
    metadata(state, slot, 16, EntityDataValue::VarInt(9));
    let xp = if state.end_fight.previously_killed {
        500
    } else {
        12000
    };
    if crate::command_rules::boolean(state, "mob_drops") {
        if time > 150 && time.is_multiple_of(5) {
            crate::experience_orbs::award(state, at, xp * 8 / 100, &mut changes.entities_spawned);
        }
        if time == 200 {
            crate::experience_orbs::award(state, at, xp * 20 / 100, &mut changes.entities_spawned);
        }
    }
    if time != 200 {
        return;
    }
    let id = state.entities.body(slot).id;
    state.remove_entity(slot);
    changes.entities_removed.push(id);
    crate::end_portals::set_exit_portal(state, changes, true);
    if !state.end_fight.previously_killed {
        let [x, y, z] = state.end_fight.exit_portal;
        pillars::write_block(state, changes, [x, y + 4, z], "minecraft:dragon_egg");
    }
    if let Some(index) = state.end_fight.gateways.pop() {
        crate::end_portals::create_gateway(state, changes, index);
    }
    state.end_fight.dragon_killed = true;
    state.end_fight.previously_killed = true;
    state.end_fight.healing_crystal = None;
}

fn try_respawn(state: &mut TickState, changes: &mut ChangeSet) {
    let [x, y, z] = state.end_fight.exit_portal;
    let mut selected = [Uuid::nil(); 4];
    for (i, [dx, dz]) in [[0, -3], [3, 0], [0, 3], [-3, 0]].into_iter().enumerate() {
        let Some(crystal) = state.end_fight.crystals.iter().find(|c| {
            c.position[0] + 1.0 > f64::from(x + dx)
                && c.position[0] - 1.0 < f64::from(x + dx + 1)
                && c.position[2] + 1.0 > f64::from(z + dz)
                && c.position[2] - 1.0 < f64::from(z + dz + 1)
                && c.position[1] + 2.0 > f64::from(y + 1)
                && c.position[1] < f64::from(y + 2)
        }) else {
            return;
        };
        selected[i] = crystal.uuid;
    }
    state.end_fight.respawn = Some(Respawn {
        crystals: selected,
        stage: 0,
        time: 0,
    });
    crate::end_portals::set_exit_portal(state, changes, false);
}
fn respawn_effects(state: &mut TickState, changes: &mut ChangeSet) {
    let Some(respawn) = state.end_fight.respawn.clone() else {
        return;
    };
    if respawn.stage == 2 && respawn.time % 40 == 39 {
        pillars::rebuild(state, changes, (respawn.time / 40) as u8);
    }
    if respawn.stage == 3 && respawn.time == 100 {
        let excluded = respawn.crystals.map(|uuid| {
            state
                .entities
                .slot_of_uuid(uuid)
                .map_or(EntityId(0), |slot| state.entities.body(slot).id)
        });
        for uuid in respawn.crystals {
            if let Some(slot) = state.entities.slot_of_uuid(uuid) {
                crate::explosion::apply_no_blocks(state, state.entities.pos(slot), 6.0, &excluded);
            }
        }
    }
}
fn respawn_tick(state: &mut TickState, changes: &mut ChangeSet) {
    let Some(mut respawn) = state.end_fight.respawn.take() else {
        return;
    };
    let target = if respawn.stage == 2 {
        pillars::position(state, (respawn.time / 40).min(9) as u8).map(|v| v.floor() as i32)
    } else {
        [0, 128, 0]
    };
    for crystal in &mut state.end_fight.crystals {
        if respawn.crystals.contains(&crystal.uuid) {
            crystal.beam = Some(target);
        }
    }
    let duration = [1, 101, 401, 101][usize::from(respawn.stage)];
    respawn.time += 1;
    if respawn.time == duration {
        respawn.time = 0;
        respawn.stage += 1;
        if respawn.stage == 4 {
            for uuid in respawn.crystals {
                if let Some(slot) = state.entities.slot_of_uuid(uuid) {
                    let id = state.entities.body(slot).id;
                    state.remove_entity(slot);
                    changes.entities_removed.push(id);
                }
            }
            state
                .end_fight
                .crystals
                .retain(|c| !respawn.crystals.contains(&c.uuid));
            for crystal in &mut state.end_fight.crystals {
                crystal.beam = None;
                crystal.invulnerable = false;
            }
            spawn_dragon(state, changes);
            return;
        }
    }
    state.end_fight.respawn = Some(respawn);
}

pub(crate) fn journal(state: &mut TickState, entries: &mut Vec<crate::journal::JournalEntry>) {
    if state.dimension == crate::dimensions::Dimension::End
        && state.journaled_end_fight.as_ref() != Some(&state.end_fight)
    {
        entries.push(crate::journal::JournalEntry::EndFight {
            value: state.end_fight.clone(),
        });
        state.journaled_end_fight = Some(state.end_fight.clone());
    }
}

pub(crate) use flight::{attack_target, part_bounds};

pub(crate) fn dragon_type(id: u32) -> bool {
    mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(id)
        .is_some_and(|c| c.name == "minecraft:ender_dragon")
}

pub(crate) fn prepare(state: &mut TickState) {
    if state.dimension == crate::dimensions::Dimension::End && !state.players.is_empty() {
        state.tickets.insert(
            "solaris:end_fight".into(),
            (crate::boundary::ChunkPos { x: 0, z: 0 }, 8),
        );
    } else {
        state.tickets.remove("solaris:end_fight");
    }
}

/// Resolve crystal hits before block commit so explosion block/loot post-images
/// cannot leak into the next tick. Explosion-caused crystal deaths do not emit
/// an additional explosion (EndCrystal.hurtServer).
pub(crate) fn crystal_hits(state: &mut TickState) {
    let mut seen = std::collections::BTreeSet::new();
    loop {
        let hit = state.merge_damage.iter().find_map(|(slot, piece)| {
            let body = state.entities.body(*slot);
            let row = state.entities.row(*slot);
            (is_crystal(state, *slot)
                && !row.controls.invulnerable
                && piece.amount > 0.0
                && !seen.contains(&body.id))
            .then_some((*slot, piece.kind))
        });
        let Some((slot, kind)) = hit else {
            break;
        };
        let id = state.entities.body(slot).id;
        seen.insert(id);
        for (target, piece) in &mut state.merge_damage {
            if *target == slot {
                piece.amount = piece.amount.max(1000.0);
            }
        }
        if kind != crate::survival::DamageKind::Explosion {
            crate::explosion::apply(state, Some(id), state.entities.pos(slot), 6.0, None, None);
        }
    }
}

pub(crate) fn damage_allowed(
    state: &mut TickState,
    slot: EntitySlot,
    piece: &mut crate::tick::DamagePiece,
) -> bool {
    if !is_dragon(state, slot) {
        return true;
    }
    if state.end_fight.phase == Phase::Dying {
        return false;
    }
    if matches!(
        piece.kind,
        crate::survival::DamageKind::InFire
            | crate::survival::DamageKind::OnFire
            | crate::survival::DamageKind::Lava
            | crate::survival::DamageKind::Fireball
            | crate::survival::DamageKind::Fall
    ) {
        return false;
    }
    if matches!(
        state.end_fight.phase,
        Phase::PerchingScanning | Phase::PerchingAttacking | Phase::PerchingFlaming
    ) && piece.kind == crate::survival::DamageKind::Projectile
    {
        return false;
    }
    true
}

pub(crate) fn damage_committed(state: &mut TickState, slot: EntitySlot, amount: f32) {
    if is_dragon(state, slot)
        && matches!(
            state.end_fight.phase,
            Phase::PerchingScanning | Phase::PerchingAttacking | Phase::PerchingFlaming
        )
    {
        state.end_fight.perched_damage += amount;
        if state.end_fight.perched_damage > 50.0 {
            state.end_fight.phase(Phase::Takeoff);
            state.end_fight.fly_target = [40.0, 100.0, 0.0];
        }
    }
}

pub(crate) fn extend_blueprints(
    types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<(u32, crate::blueprint::MobBlueprint)>,
) {
    use crate::blueprint::*;
    for (name, health, width, height) in [
        ("minecraft:ender_dragon", 200.0, 8.0, 8.0),
        ("minecraft:end_crystal", 1.0, 1.0, 2.0),
        ("minecraft:experience_orb", 5.0, 0.25, 0.5),
    ] {
        let id = types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .expect("vanilla fight type");
        out.retain(|(seen, _)| *seen != id);
        out.push((
            id,
            MobBlueprint {
                name: name.to_owned(),
                type_id: id,
                movement: if name == "minecraft:experience_orb" {
                    MovementClass::Item
                } else {
                    MovementClass::Stationary
                },
                goals: Vec::new(),
                sensors: Sensors {
                    sight_range: 150.0,
                    attack_reach: 0.0,
                },
                params: MobParams {
                    max_health: health,
                    speed: 0.0,
                    attack_damage: 0.0,
                    pickup_delay: 0,
                    breed_cooldown: 0,
                    armor: 0.0,
                    knockback_resistance: 0.0,
                },
                half_width: width,
                height,
                offspring: None,
                breed_food: Vec::new(),
                loot_table: None,
                category: mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(id)
                    .unwrap()
                    .mob_category(),
                wool: false,
                spawn_weight: 0,
                capabilities: Capabilities::default(),
            },
        ));
    }
}

pub(crate) fn restore(state: &mut TickState) -> Result<(), crate::config::CoreError> {
    if state.dimension != crate::dimensions::Dimension::End {
        return Ok(());
    }
    state
        .end_fight
        .validate()
        .map_err(crate::config::CoreError::Storage)?;
    let dragons: Vec<_> = state
        .entities
        .slots()
        .filter(|s| is_dragon(state, *s))
        .collect();
    if !state.end_fight.initialized && !dragons.is_empty() {
        if dragons.len() != 1 {
            return Err(crate::config::CoreError::Storage(
                "DragonFight: native arena has multiple dragons".into(),
            ));
        }
        state.end_fight.dragon_uuid = Some(state.entities.row(dragons[0]).uuid);
        state.end_fight.initialized = true;
        state.end_fight.dragon_killed = false;
    }
    if state.end_fight.dragon_killed {
        state.end_fight.initialized = true;
    }
    if state.end_fight.dragon_killed
        && let Some(uuid) = state.end_fight.dragon_uuid
        && state.entities.slot_of_uuid(uuid).is_some()
    {
        return Err(crate::config::CoreError::Storage(format!(
            "DragonFight.dragon_uuid {uuid}: killed fight still retains the dragon entity"
        )));
    }
    if let Some(uuid) = state
        .end_fight
        .dragon_uuid
        .filter(|_| !state.end_fight.dragon_killed)
    {
        let Some(slot) = state.entities.slot_of_uuid(uuid) else {
            return Err(crate::config::CoreError::Storage(format!(
                "DragonFight.dragon_uuid {uuid}: missing retained entity"
            )));
        };
        if !is_dragon(state, slot) {
            return Err(crate::config::CoreError::Storage(format!(
                "DragonFight.dragon_uuid {uuid}: entity is not ender_dragon"
            )));
        }
        if state.end_fight.native_import {
            state.end_fight.flight_history = [[
                state.entities.pos(slot)[1],
                f64::from(state.entities.rot(slot)[0]),
            ]; 17];
            if let Some((_, EntityDataValue::VarInt(phase))) = state
                .entities
                .row(slot)
                .appearance
                .extra
                .iter()
                .find(|(index, _)| *index == 16)
            {
                state.end_fight.phase = match phase {
                    0 => Phase::Circling,
                    1 => Phase::Strafing,
                    2 => Phase::LandingApproach,
                    3 => Phase::Landing,
                    4 => Phase::Takeoff,
                    5 => Phase::PerchingFlaming,
                    6 => Phase::PerchingScanning,
                    7 => Phase::PerchingAttacking,
                    8 => Phase::Charging,
                    9 => Phase::Dying,
                    10 => Phase::Hover,
                    _ => {
                        return Err(crate::config::CoreError::Storage(format!(
                            "DragonFight.dragon_uuid {uuid}: invalid DragonPhase {phase}"
                        )));
                    }
                };
            }
        }
        if state.entities.row(slot).health <= 0.0 {
            state.end_fight.phase = Phase::Dying;
        }
        metadata(
            state,
            slot,
            16,
            EntityDataValue::VarInt(state.end_fight.phase as i32),
        );
    }
    for crystal in &state.end_fight.crystals {
        let Some(slot) = state.entities.slot_of_uuid(crystal.uuid) else {
            return Err(crate::config::CoreError::Storage(format!(
                "DragonFight crystal UUID {}: missing retained entity",
                crystal.uuid
            )));
        };
        if !is_crystal(state, slot) {
            return Err(crate::config::CoreError::Storage(format!(
                "DragonFight crystal UUID {}: entity is not end_crystal",
                crystal.uuid
            )));
        }
    }
    sync_crystals(state);
    // Identify native spike crystals without a Solaris membership extension.
    for index in 0..10 {
        let expected = pillars::position(state, index);
        for crystal in &mut state.end_fight.crystals {
            if crystal.position == expected {
                crystal.pillar = Some(index);
            }
        }
    }
    if let Some(uuid) = state.end_fight.flame_cloud {
        let Some(slot) = state.entities.slot_of_uuid(uuid) else {
            return Err(crate::config::CoreError::Storage(format!(
                "DragonFight.flame_cloud {uuid}: missing retained entity"
            )));
        };
        if state
            .entities
            .row(slot)
            .projectile
            .as_ref()
            .is_none_or(|p| {
                p.kind != crate::projectiles::ProjectileKind::AreaEffectCloud
                    || p.owner != state.end_fight.dragon_uuid
            })
        {
            return Err(crate::config::CoreError::Storage(format!(
                "DragonFight.flame_cloud {uuid}: wrong cloud kind or dragon owner"
            )));
        }
    }
    state.end_fight.native_import = false;
    Ok(())
}

pub(crate) fn reserved_high_water(state: &TickState) -> Result<i32, crate::config::CoreError> {
    let worlds = || std::iter::once(state).chain(state.other_dimensions.values().map(|w| &**w));
    let mut ids = std::collections::BTreeMap::new();
    for world in worlds() {
        for slot in world.entities.slots() {
            let body = world.entities.body(slot);
            let row = world.entities.row(slot);
            if body.id.0 > 0 {
                ids.insert(body.id.0, (world.dimension, row.uuid));
            }
        }
    }
    let mut high = ids.keys().next_back().copied().unwrap_or(0);
    let mut reserved = std::collections::BTreeSet::new();
    for world in worlds() {
        for slot in world.entities.slots().filter(|s| is_dragon(world, *s)) {
            let body = world.entities.body(slot);
            if body.id.0 <= 0 {
                continue;
            }
            let row = world.entities.row(slot);
            let end = body.id.0.checked_add(8).ok_or_else(|| {
                crate::config::CoreError::Storage(format!(
                    "{}: dragon UUID {} wire-id {}: multipart ID range overflows",
                    world.dimension.name(),
                    row.uuid,
                    body.id.0
                ))
            })?;
            for id in body.id.0 + 1..=end {
                if let Some((dimension, uuid)) = ids.get(&id) {
                    return Err(crate::config::CoreError::Storage(format!(
                        "DragonFight: {} dragon UUID {} child wire-id {id} overlaps {} entity UUID {uuid}",
                        world.dimension.name(),
                        row.uuid,
                        dimension.name()
                    )));
                }
                if !reserved.insert(id) {
                    return Err(crate::config::CoreError::Storage(format!(
                        "DragonFight: overlapping multipart wire-id {id}"
                    )));
                }
            }
            high = high.max(end);
        }
    }
    Ok(high)
}

pub(crate) fn use_crystal(
    state: &mut TickState,
    entity: EntityId,
    pos: crate::boundary::BlockPos,
    hand: crate::boundary::Hand,
    changes: &mut ChangeSet,
) -> bool {
    let Some(player) = state.players.get(&entity) else {
        return false;
    };
    let index = crate::tick::held_slot_index(player, hand);
    let held = &player.inventory.slots[index];
    if held.is_empty()
        || state
            .registry
            .items
            .name_of(held.item_id)
            .is_none_or(|id| id.as_str() != "minecraft:end_crystal")
    {
        return false;
    }
    let Some(block) = state
        .world
        .block_at(pos.x, pos.y, pos.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
    else {
        return true;
    };
    if !matches!(block.block.id.path(), "obsidian" | "bedrock") {
        return true;
    }
    if state.world.block_at(pos.x, pos.y + 1, pos.z) != Some(state.registry.states.air) {
        return true;
    }
    if state.entities.slots().any(|slot| {
        let at = state.entities.pos(slot);
        let body = state.entities.body(slot);
        at[0] + body.half_width > f64::from(pos.x)
            && at[0] - body.half_width < f64::from(pos.x + 1)
            && at[2] + body.half_width > f64::from(pos.z)
            && at[2] - body.half_width < f64::from(pos.z + 1)
            && at[1] + body.height > f64::from(pos.y + 1)
            && at[1] < f64::from(pos.y + 3)
    }) {
        return true;
    }
    let id = state.spawn_mob(
        type_id(state, "minecraft:end_crystal"),
        [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y + 1),
            f64::from(pos.z) + 0.5,
        ],
        1.0,
    );
    let slot = state.entities.slot_of(id).expect("placed crystal");
    metadata(state, slot, 9, EntityDataValue::Boolean(false));
    changes.entities_spawned.push(id);
    let player = state.players.get_mut(&entity).expect("crystal player");
    if !player.creative() {
        mc_data::inventory_semantics_26_1_2::decrement_stack(&mut player.inventory.slots[index]);
    }
    let connection = player.connection;
    let contents = player.inventory_contents();
    state.push_player(connection, contents);
    true
}
