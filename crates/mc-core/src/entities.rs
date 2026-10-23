//! Columnar (SoA) entity store: cache-aligned hot columns on the tick
//! thread.
//!
//! The authoritative store lives on the tick thread. Each committed tick
//! publishes its wire projection ([`EntityStore::views`]) inside the
//! immutable `WorldSnapshot`; readers hold that `Arc` as long as they like
//! and the core never waits for them.

use std::sync::Arc;

use uuid::Uuid;

use crate::blueprint::MovementClass;
use crate::boundary::{ChunkPos, EntityAppearance, EntityId, EntityView};

/// Dense entity index: position in every column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct EntitySlot(pub u32);

/// Entity kind as data; never branched on by type name in the tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityKind {
    Player {
        connection: crate::boundary::ConnectionId,
    },
    Mob {
        /// Protocol entity type id; selects the blueprint.
        type_id: u32,
    },
    Item {
        /// Protocol entity type id of `minecraft:item`.
        type_id: u32,
    },
    ExperienceOrb {
        type_id: u32,
    },
}

/// Hot single-owner contact column: identity, kind, liveness, dimensions and
/// the timers the contact/motion hot path reads every tick. `#[repr(C)]` keeps
/// the contact set (`id`, `kind`, `alive`, `half_width`, `height`,
/// `burn_ticks`) inside the first 64 bytes.
/// Full [`EntityKind`] is retained here as the sole kind/type authority.
#[derive(Debug, Clone, Copy, PartialEq)]
#[repr(C)]
pub struct Body {
    /// Change through `EntityStore::set_kind` to retain tick memberships.
    pub kind: EntityKind,
    // Write through EntityStore::set_half_width to retain the spatial query bound.
    pub half_width: f64,
    pub height: f64,
    pub id: EntityId,
    pub burn_ticks: u32,
    pub alive: bool,
    pub movement: MovementClass,
    pub air_supply: i32,
    pub pickup_delay: u32,
    /// Ticks until this mob may melee again (vanilla `MeleeAttackGoal`).
    pub attack_cooldown: u32,
    /// Tick of the last hit that started hurt invulnerability.
    pub hurt_at: Option<u64>,
}

/// Cold per-entity state kept out of the hot path.
#[derive(Debug, Clone, PartialEq)]
pub struct ColdRow {
    pub uuid: Uuid,
    pub health: f32,
    pub active_effects: crate::effects::ActiveEffects,
    pub villager: Option<Box<crate::villager::VillagerState>>,
    pub projectile: Option<Box<crate::projectiles::ProjectileState>>,
    pub special_mob: Option<Box<crate::special_mobs::Retained>>,
    pub bee: Option<Box<crate::bees::BeeState>>,
    pub allay: Option<Box<crate::allay::AllayState>>,
    pub falling: Option<crate::falling::FallingState>,
    pub tnt: Option<crate::tnt::PrimedTntState>,
    /// Replace through `EntityStore::set_entity_life`; rider/leash have setters.
    pub entity_life: crate::entity_life::EntityLifeState,
    pub loot: crate::entity_loot::EntityLootState,
    pub controls: crate::entity_controls::Controls,
    pub persistent: bool,
    pub no_action_time: u32,
    pub breed_cooldown: u32,
    /// Ticks left in love (fed breeding food), retained across restart.
    pub love: u32,
    pub age: u32,
    pub item: Option<Box<StackRow>>,
    pub appearance: Arc<EntityAppearance>,
    pub(crate) embedded_projectile_timers: [i32; 2],
    pub speed: f32,
    pub attack_damage: f32,
    pub sight_range: f32,
    pub attack_reach: f32,
    /// Pre-armor amount of the strongest hit inside that window.
    pub last_hurt: f32,
    /// Vanilla player-kill credit remains for 100 ticks after a player hit.
    pub(crate) last_player_hit: Option<(EntityId, u64)>,
    /// Ticks before this one ignore damage (vanilla player
    /// `spawnInvulnerableTime` after join/respawn).
    pub protected_until: u64,
    /// Named NBT compound of retained entity fields; typed checkpoint fields win.
    pub opaque_entity_fields: Vec<u8>,
}

/// Store `value` in a boxed rare component, overwriting an existing box
/// instead of allocating a new one.
pub(crate) fn set_boxed<T>(slot: &mut Option<Box<T>>, value: T) {
    match slot.as_deref_mut() {
        Some(current) => *current = value,
        None => *slot = Some(Box::new(value)),
    }
}

/// Vanilla `ServerPlayer.spawnInvulnerableTime` after join and respawn.
pub const SPAWN_INVULNERABLE_TICKS: u64 = 60;

/// Ticks a hit protects against weaker hits (vanilla `invulnerableTime`
/// above 10 of its 20).
pub const HURT_INVULNERABLE_TICKS: u64 = 10;

impl ColdRow {
    /// Replace the retained special-mob state, reusing its allocation.
    pub fn set_special_mob(&mut self, value: crate::special_mobs::Retained) {
        set_boxed(&mut self.special_mob, value);
    }
}

/// Item stack carried by an item entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackRow {
    pub item_id: u32,
    pub count: i32,
    /// Full components (enchantments, name, damage...); identity-critical.
    pub stack: Option<mc_data::ItemStack>,
}

impl StackRow {
    /// Full stack for journaling: components if present, else plain id+count.
    #[must_use]
    pub fn full_stack(&self) -> mc_data::ItemStack {
        if let Some(stack) = &self.stack {
            stack.clone()
        } else {
            mc_data::ItemStack::new(self.item_id, self.count)
        }
    }
}

/// Authoritative columnar store on the tick thread.
#[derive(Debug, Default)]
pub struct EntityStore {
    pos: Vec<[f64; 3]>,
    vel: Vec<[f64; 3]>,
    /// Combat momentum separate from the AI's standing horizontal decision,
    /// in blocks/second; transient, decayed by walker physics.
    impulse: Vec<[f64; 2]>,
    rot: Vec<[f32; 3]>,
    ground: Vec<bool>,
    bodies: Vec<Body>,
    cold: Vec<ColdRow>,
    next_wire_id: i32,
    /// Live id -> slot. Maintained on spawn/remove; swap-remove fixup included.
    id_index: rustc_hash::FxHashMap<crate::boundary::EntityId, EntitySlot>,
    /// Durable UUID -> current dense slot; projectile owners survive wire-id
    /// reassignment on restart without scanning the whole entity population.
    uuid_index: std::collections::HashMap<Uuid, EntitySlot>,
    /// Feature scans traverse type members in the original dense-slot order.
    mob_members: rustc_hash::FxHashMap<u32, std::collections::BTreeSet<EntitySlot>>,
    /// Exact kind lanes, including dead rows; independent of lazy motion flags.
    player_members: std::collections::BTreeSet<EntitySlot>,
    item_members: std::collections::BTreeSet<EntitySlot>,
    rider_members: std::collections::BTreeSet<EntitySlot>,
    leash_members: std::collections::BTreeSet<EntitySlot>,
    /// Sparse orb index: worlds without XP orbs do no additional entity scan.
    orbs: std::collections::BTreeSet<EntityId>,
    /// Module-owned primed TNT; empty worlds skip population scans.
    tnt: std::collections::BTreeSet<EntityId>,
    /// Sparse, sorted native warden IDs; vibration delivery never scans other bodies.
    wardens: Vec<EntityId>,
    /// Journal custody (see [`Self::row_mut`]): per-slot flag deduplicating the
    /// ids of rows whose owner took `row_mut` (or that entered the store).
    touched_flag: Vec<bool>,
    touched: Vec<EntityId>,
    removed: Vec<EntityId>,
    /// Non-authoritative per-slot motion flags for the entity phase.
    /// Refreshed sparsely from the dirty queue in `prepare_motion_hints`;
    /// every column compacts with the dense slot on remove/transfer.
    flags: Vec<u16>,
    hints: Vec<MotionHint>,
    dirty_flag: Vec<bool>,
    dirty_ids: Vec<EntityId>,
    /// Resident brain bodies, indexed by chunk and stable id, independent of
    /// dense-slot compaction. Other entities supply chunk-local brain sensors.
    villager_chunks: rustc_hash::FxHashMap<ChunkPos, std::collections::BTreeSet<EntityId>>,
    sensor_chunks: rustc_hash::FxHashMap<ChunkPos, std::collections::BTreeSet<EntityId>>,
    /// Conservative horizontal query extent. Shrinking/removal never reduces it.
    max_half_width: f64,
    brain_revision: u64,
    brain_changes: rustc_hash::FxHashSet<EntityId>,
    /// Wire candidate deduplication is dense for live rows; only identities
    /// removed before publication need sparse custody across a possible rebirth.
    wire_marked: Vec<bool>,
    wire_removed: rustc_hash::FxHashSet<EntityId>,
    /// Dense, mark-time-deduplicated queue with validated source-slot hints.
    wire_pending: Vec<(EntityId, EntitySlot)>,
    wire_views: crate::boundary::EntityViews,
    /// Outgoing-only labels; never stored in native rows, WAL, or checkpoints.
    nameplates: rustc_hash::FxHashMap<EntityId, crate::villager::nameplates::Nameplate>,
    #[cfg(test)]
    touch_sites: std::collections::BTreeMap<(&'static str, u32), [usize; 3]>,
}

/// Non-authoritative per-slot motion gate inputs, refreshed sparsely in
/// `prepare_motion_hints`. Parent checks hot dynamic inputs (ground,
/// velocity, current tick, hurt expiry, footprint, fuse/bow/target state).
pub(crate) const MOTION_OWNED: u16 = 1 << 0;
pub(crate) const MOTION_PLAYER: u16 = 1 << 1;
pub(crate) const MOTION_REST: u16 = 1 << 2;
pub(crate) const MOTION_ALIVE: u16 = 1 << 3;
pub(crate) const MOTION_SPECIAL: u16 = 1 << 4;
pub(crate) const MOTION_EFFECTS: u16 = 1 << 5;
pub(crate) const MOTION_LIFE: u16 = 1 << 6;
pub(crate) const MOTION_PHASE_SHIFT: u32 = 7;
pub(crate) const MOTION_PHASE_MASK: u16 = 3 << 7;
pub(crate) const MOTION_WANDER_REST: u16 = 1 << 9;
pub(crate) const MOTION_TARGETLESS_WANDER_REST: u16 = 1 << 10;
pub(crate) const MOTION_ARMORED: u16 = 1 << 11;
pub(crate) const MOTION_QUIET_TIMERS: u16 = 1 << 12;

/// Derived motion inputs, refreshed only for dirty entities. Authoritative
/// identity, dimensions and hurt time live exclusively in `Body`.
#[derive(Debug)]
pub(crate) struct MotionHint {
    pub(crate) special: Option<crate::hostile::Special>,
    pub(crate) swell_metadata_neutral: bool,
}

/// One entity-step result for [`EntityStore::apply_motion_result`]. Rotation is
/// yaw/pitch only (the stored third component mirrors yaw). `attack_cooldown`
/// is `None` for the ordinary one-tick decrement.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MotionResult {
    pub(crate) pos: [f64; 3],
    pub(crate) vel: [f64; 3],
    pub(crate) yaw: f32,
    pub(crate) pitch: f32,
    pub(crate) ground: bool,
    pub(crate) horizontal_impulse: [f64; 2],
    pub(crate) air_supply: i32,
    pub(crate) attack_cooldown: Option<u32>,
}

/// Motion-owned scalar writeback cannot read retained cold state. The caller
/// applies custody marks: air is durable, the other clocks are transient.
fn apply_motion_state(
    bodies: &mut [Body],
    impulses: &mut [[f64; 2]],
    index: usize,
    result: &MotionResult,
) -> (bool, bool) {
    let body = &mut bodies[index];
    let pickup_delay = body.pickup_delay.saturating_sub(1);
    let attack_cooldown = result
        .attack_cooldown
        .unwrap_or_else(|| body.attack_cooldown.saturating_sub(1));
    let air_changed = body.air_supply != result.air_supply;
    let changed = air_changed
        || impulses[index] != result.horizontal_impulse
        || body.pickup_delay != pickup_delay
        || body.attack_cooldown != attack_cooldown;
    if changed {
        body.air_supply = result.air_supply;
        body.pickup_delay = pickup_delay;
        body.attack_cooldown = attack_cooldown;
        impulses[index] = result.horizontal_impulse;
    }
    (air_changed, changed)
}

impl EntityStore {
    fn brain_chunk(pos: [f64; 3]) -> ChunkPos {
        ChunkPos {
            x: mc_physics::mth::floor_i32(pos[0]).div_euclid(16),
            z: mc_physics::mth::floor_i32(pos[2]).div_euclid(16),
        }
    }

    fn resident_kind(kind: EntityKind) -> bool {
        let EntityKind::Mob { type_id } = kind else {
            return false;
        };
        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id).is_some_and(
            |contract| {
                matches!(
                    contract
                        .name
                        .strip_prefix("minecraft:")
                        .unwrap_or(contract.name),
                    "villager" | "zombie_villager" | "wandering_trader"
                )
            },
        )
    }

    /// One notification point for entity mutations. Additional consumers (for
    /// example the journal) belong here; clock-only projection bypasses it.
    fn entity_changed(&mut self, id: EntityId, resident: bool, slot: EntitySlot) {
        if !std::mem::replace(&mut self.wire_marked[slot.0 as usize], true) {
            self.wire_pending.push((id, slot));
        }
        if resident {
            self.brain_changes.insert(id);
        }
    }

    fn index_brain_body(
        &mut self,
        id: EntityId,
        kind: EntityKind,
        pos: [f64; 3],
        insert: bool,
        slot: EntitySlot,
    ) {
        let resident = Self::resident_kind(kind);
        self.entity_changed(id, resident, slot);
        let index = if resident {
            &mut self.villager_chunks
        } else {
            &mut self.sensor_chunks
        };
        let chunk = Self::brain_chunk(pos);
        if insert {
            index.entry(chunk).or_default().insert(id);
        } else if let Some(ids) = index.get_mut(&chunk) {
            ids.remove(&id);
            if ids.is_empty() {
                index.remove(&chunk);
            }
        }
    }

    pub(crate) fn villager_chunks(
        &self,
    ) -> &rustc_hash::FxHashMap<ChunkPos, std::collections::BTreeSet<EntityId>> {
        &self.villager_chunks
    }

    /// Slots of every id indexed as a resident brain body, ascending and unique.
    /// Stale old-kind memberships can list an id twice; ids that left the store
    /// are skipped. Callers filter on live row state.
    pub(crate) fn resident_slots_sorted(&self, out: &mut Vec<EntitySlot>) {
        out.clear();
        for ids in self.villager_chunks.values() {
            out.extend(ids.iter().filter_map(|id| self.id_index.get(id).copied()));
        }
        out.sort_unstable_by_key(|slot| slot.0);
        out.dedup();
    }

    pub(crate) fn brain_sensor_chunks(
        &self,
    ) -> &rustc_hash::FxHashMap<ChunkPos, std::collections::BTreeSet<EntityId>> {
        &self.sensor_chunks
    }

    /// Each current row has a membership at its current position. Kind changes
    /// can leave extra old memberships; an exact count proves there are none,
    /// so dense position scans are equivalent to visiting the chunk indexes.
    pub(crate) fn has_exact_chunk_membership(&self) -> bool {
        self.villager_chunks
            .values()
            .map(|ids| ids.len())
            .sum::<usize>()
            + self
                .sensor_chunks
                .values()
                .map(|ids| ids.len())
                .sum::<usize>()
            == self.len()
    }

    #[inline]
    pub(crate) fn chunk_of(&self, slot: EntitySlot) -> ChunkPos {
        Self::brain_chunk(self.pos[slot.0 as usize])
    }

    /// Visit current dense slots without a nested iterator chain. Stable IDs
    /// remain authoritative; callers deduplicate old type-transition entries.
    pub(crate) fn for_each_slot_in_chunk(&self, pos: ChunkPos, mut visit: impl FnMut(EntitySlot)) {
        if let Some(ids) = self.villager_chunks.get(&pos) {
            for id in ids {
                if let Some(&slot) = self.id_index.get(id) {
                    visit(slot);
                }
            }
        }
        if let Some(ids) = self.sensor_chunks.get(&pos) {
            for id in ids {
                if let Some(&slot) = self.id_index.get(id) {
                    visit(slot);
                }
            }
        }
    }

    pub(crate) fn brain_revision(&self) -> u64 {
        self.brain_revision
    }

    pub(crate) fn brain_changes(&self) -> &rustc_hash::FxHashSet<EntityId> {
        &self.brain_changes
    }

    pub(crate) fn swap_brain_changes(&mut self, changes: &mut rustc_hash::FxHashSet<EntityId>) {
        std::mem::swap(&mut self.brain_changes, changes);
    }

    /// One-row reference for [`Self::advance_admitted_ages`] differential tests.
    #[cfg(test)]
    pub(crate) fn advance_age(&mut self, slot: EntitySlot) -> Option<u32> {
        let index = slot.0 as usize;
        if !self.bodies[index].alive {
            return None;
        }
        let row = &mut self.cold[index];
        row.age = row.age.wrapping_add(1);
        Some(row.age)
    }

    /// Generic lifetime age is a record-tick projection, not a brain change.
    /// Advance every live row whose `admitted` byte is set (rows are
    /// independent, so the pool splits them without changing any result) and
    /// return, ascending, the item rows whose clock reached `item_limit`.
    /// Its owner still removes expired entities through the ordinary hook.
    pub(crate) fn advance_admitted_ages(
        &mut self,
        admitted: &[u8],
        item_limit: u32,
        min_parallel: usize,
    ) -> Vec<EntitySlot> {
        fn step(
            index: usize,
            body: &Body,
            row: &mut ColdRow,
            admitted: u8,
            item_limit: u32,
        ) -> Option<EntitySlot> {
            if admitted == 0 || !body.alive {
                return None;
            }
            row.age = row.age.wrapping_add(1);
            (row.age >= item_limit && matches!(body.kind, EntityKind::Item { .. }))
                .then_some(EntitySlot(index as u32))
        }
        debug_assert_eq!(admitted.len(), self.cold.len());
        debug_assert_eq!(self.bodies.len(), self.cold.len());
        if self.cold.len() < min_parallel {
            return self
                .cold
                .iter_mut()
                .zip(&self.bodies)
                .zip(admitted)
                .enumerate()
                .filter_map(|(index, ((row, body), &admitted))| {
                    step(index, body, row, admitted, item_limit)
                })
                .collect();
        }
        use rayon::prelude::*;
        self.cold
            .par_iter_mut()
            .zip(self.bodies.par_iter())
            .zip(admitted.par_iter())
            .with_min_len(2048)
            .enumerate()
            .filter_map(|(index, ((row, body), &admitted))| {
                step(index, body, row, admitted, item_limit)
            })
            .collect()
    }

    /// Clock projection is not a gameplay dirty mark. Schedule/event changes
    /// still enter through ordinary row_mut after full-brain admission.
    pub(crate) fn advance_villager_clocks(
        &mut self,
        slot: EntitySlot,
        from: u64,
        to: u64,
    ) -> Option<u64> {
        let index = slot.0 as usize;
        if !self.bodies[index].alive {
            return None;
        }
        let row = &mut self.cold[index];
        if row.controls.no_ai {
            return None;
        }
        let v = row.villager.as_mut()?;
        if v.wandering || v.conversion_time.is_some() {
            return None;
        }
        // Admission, not the last journal image, decides whether time runs.
        // Preserve the image flag so this does not create a gameplay mutation.
        let brain_clock = v.brain_clock;
        v.brain_clock = true;
        crate::villager::persistence::advance_clocks(v, from, to);
        v.brain_clock = brain_clock;
        Some(v.panic_until)
    }

    pub(crate) fn set_brain_impulse(&mut self, slot: EntitySlot, impulse: [f64; 2]) {
        let index = slot.0 as usize;
        if self.impulse[index] != impulse {
            self.impulse[index] = impulse;
            self.mark_motion_dirty(index);
        }
    }

    /// Following a committed route only consumes transient, unjournaled steps.
    /// It must not turn skipped-brain execution into a cold-row dirty mark.
    pub(crate) fn advance_villager_path(&mut self, slot: EntitySlot, reached: usize) {
        if let Some(v) = self.cold[slot.0 as usize].villager.as_mut() {
            v.navigation_path
                .truncate(v.navigation_path.len().saturating_sub(reached));
        }
    }

    /// Sampled route geometry is operational state, not a cold-row mutation.
    pub(crate) fn villager_motion_geometry_mut(
        &mut self,
        slot: EntitySlot,
    ) -> Option<&mut crate::villager::brain::MotionGeometryCache> {
        self.cold[slot.0 as usize]
            .villager
            .as_mut()
            .map(|v| &mut v.motion_geometry)
    }

    pub(crate) fn warden_ids(&self) -> &[EntityId] {
        &self.wardens
    }

    pub(crate) fn register_warden(&mut self, id: EntityId) {
        if let Err(at) = self.wardens.binary_search(&id) {
            self.wardens.insert(at, id);
        }
    }

    fn unregister_warden(&mut self, id: EntityId) -> bool {
        if let Ok(at) = self.wardens.binary_search(&id) {
            self.wardens.remove(at);
            true
        } else {
            false
        }
    }

    /// Allocate a wire id (i32, shared players+mobs space).
    pub fn alloc_id(&mut self) -> EntityId {
        self.next_wire_id += 1;
        EntityId(self.next_wire_id)
    }

    pub(crate) fn set_next_wire_id(&mut self, id: i32) {
        self.next_wire_id = self.next_wire_id.max(id);
    }

    pub(crate) fn wire_id_high_water(&self) -> i32 {
        self.next_wire_id
    }

    /// Spawn one entity; returns its dense slot.
    pub fn spawn(
        &mut self,
        id: EntityId,
        uuid: Uuid,
        kind: EntityKind,
        cold: ColdFields,
    ) -> EntitySlot {
        let slot = EntitySlot(self.bodies.len() as u32);
        debug_assert_eq!(self.bodies.len(), self.cold.len());
        debug_assert_eq!(self.impulse.len(), self.cold.len());
        self.max_half_width = self.max_half_width.max(cold.half_width);
        self.pos.push(cold.pos);
        self.vel.push(cold.vel);
        self.rot.push([cold.yaw, cold.pitch, cold.yaw]);
        self.ground.push(cold.ground);
        self.bodies.push(Body {
            id,
            kind,
            alive: true,
            half_width: cold.half_width,
            height: cold.height,
            burn_ticks: 0,
            movement: cold.movement,
            air_supply: 300,
            pickup_delay: cold.pickup_delay,
            attack_cooldown: 0,
            hurt_at: None,
        });
        self.impulse.push([0.0; 2]);
        self.cold.push(ColdRow {
            uuid,
            health: cold.health,
            active_effects: crate::effects::ActiveEffects::default(),
            villager: None,
            projectile: None,
            special_mob: None,
            bee: None,
            allay: None,
            falling: None,
            tnt: None,
            entity_life: crate::entity_life::EntityLifeState::default(),
            loot: crate::entity_loot::EntityLootState::default(),
            controls: Default::default(),
            persistent: false,
            no_action_time: 0,
            breed_cooldown: cold.breed_cooldown,
            love: 0,
            age: 0,
            item: cold.item.map(Box::new),
            appearance: cold.appearance,
            embedded_projectile_timers: [0; 2],
            speed: cold.speed,
            attack_damage: cold.attack_damage,
            sight_range: cold.sight_range,
            attack_reach: cold.attack_reach,
            last_hurt: 0.0,
            protected_until: 0,
            opaque_entity_fields: Vec::new(),
            last_player_hit: None,
        });
        self.touched_flag.push(true);
        debug_assert!(
            !self.wire_removed.contains(&id)
                || self.wire_pending.iter().any(|(pending, _)| *pending == id),
            "a re-spawned id in wire_removed must have a pending wire entry"
        );
        self.wire_marked.push(self.wire_removed.contains(&id));
        self.touched.push(id);
        self.flags.push(0);
        self.hints.push(MotionHint {
            special: None,
            swell_metadata_neutral: true,
        });
        self.dirty_flag.push(true);
        self.dirty_ids.push(id);
        self.set_next_wire_id(id.0);
        self.id_index.insert(id, slot);
        self.uuid_index.insert(uuid, slot);
        self.index_tick_members(slot);
        self.index_brain_body(id, kind, cold.pos, true, slot);
        if Self::resident_kind(kind) {
            self.brain_revision = self.brain_revision.wrapping_add(1);
        }
        if matches!(kind, EntityKind::ExperienceOrb { .. }) {
            self.orbs.insert(id);
        }
        slot
    }

    /// Remove by dense slot via swap-remove; returns the moved entity's slot fixup.
    pub fn remove(&mut self, slot: EntitySlot) -> Option<(EntityId, EntitySlot)> {
        let idx = slot.0 as usize;
        if idx >= self.bodies.len() {
            return None;
        }
        debug_assert_eq!(self.bodies.len(), self.cold.len());
        debug_assert_eq!(self.impulse.len(), self.cold.len());
        let last = self.bodies.len() - 1;
        let removed_id = self.bodies[idx].id;
        let removed_uuid = self.cold[idx].uuid;
        self.unindex_tick_members(slot, self.bodies[idx].kind);
        if idx != last {
            self.unindex_tick_members(EntitySlot(last as u32), self.bodies[last].kind);
        }
        self.index_brain_body(
            removed_id,
            self.bodies[idx].kind,
            self.pos[idx],
            false,
            slot,
        );
        self.wire_removed.insert(removed_id);
        if Self::resident_kind(self.bodies[idx].kind) {
            self.brain_revision = self.brain_revision.wrapping_add(1);
        }
        self.orbs.remove(&removed_id);
        self.tnt.remove(&removed_id);
        self.unregister_warden(removed_id);
        self.pos.swap_remove(idx);
        self.vel.swap_remove(idx);
        self.rot.swap_remove(idx);
        self.ground.swap_remove(idx);
        self.bodies.swap_remove(idx);
        self.impulse.swap_remove(idx);
        self.cold.swap_remove(idx);
        if idx != last {
            self.index_tick_members(slot);
        }
        self.touched_flag.swap_remove(idx);
        self.wire_marked.swap_remove(idx);
        self.flags.swap_remove(idx);
        self.hints.swap_remove(idx);
        self.dirty_flag.swap_remove(idx);
        self.removed.push(removed_id);
        self.id_index.remove(&removed_id);
        self.uuid_index.remove(&removed_uuid);
        if idx != last {
            let moved_id = self.bodies[idx].id;
            self.id_index.insert(moved_id, EntitySlot(idx as u32));
            if Self::resident_kind(self.bodies[idx].kind) {
                self.brain_revision = self.brain_revision.wrapping_add(1);
                self.entity_changed(moved_id, true, EntitySlot(idx as u32));
            }
            self.uuid_index
                .insert(self.cold[idx].uuid, EntitySlot(idx as u32));
            Some((moved_id, EntitySlot(idx as u32)))
        } else {
            None
        }
    }

    /// Move complete custody to another isolated world without cloning retained state.
    pub(crate) fn transfer_to(
        &mut self,
        slot: EntitySlot,
        destination: &mut Self,
        position: [f64; 3],
        rotation: [f32; 2],
    ) -> EntitySlot {
        let index = slot.0 as usize;
        debug_assert_eq!(self.bodies.len(), self.cold.len());
        debug_assert_eq!(self.impulse.len(), self.cold.len());
        let last = self.bodies.len() - 1;
        self.unindex_tick_members(slot, self.bodies[index].kind);
        if index != last {
            self.unindex_tick_members(EntitySlot(last as u32), self.bodies[last].kind);
        }
        let body = self.bodies.swap_remove(index);
        let impulse = self.impulse.swap_remove(index);
        let row = self.cold.swap_remove(index);
        if index != last {
            self.index_tick_members(slot);
        }
        let id = body.id;
        let kind = body.kind;
        let uuid = row.uuid;
        self.index_brain_body(id, kind, self.pos[index], false, slot);
        self.wire_removed.insert(id);
        if Self::resident_kind(kind) {
            self.brain_revision = self.brain_revision.wrapping_add(1);
        }
        self.pos.swap_remove(index);
        let velocity = self.vel.swap_remove(index);
        self.rot.swap_remove(index);
        self.ground.swap_remove(index);
        self.touched_flag.swap_remove(index);
        self.wire_marked.swap_remove(index);
        self.flags.swap_remove(index);
        self.hints.swap_remove(index);
        self.dirty_flag.swap_remove(index);
        self.removed.push(id);
        self.id_index.remove(&id);
        self.uuid_index.remove(&uuid);
        let orb = self.orbs.remove(&id);
        let tnt = self.tnt.remove(&id);
        let warden = self.unregister_warden(id);
        if index != self.bodies.len() {
            let moved = &self.bodies[index];
            self.id_index.insert(moved.id, EntitySlot(index as u32));
            self.uuid_index
                .insert(self.cold[index].uuid, EntitySlot(index as u32));
            if Self::resident_kind(moved.kind) {
                self.brain_revision = self.brain_revision.wrapping_add(1);
                self.entity_changed(moved.id, true, EntitySlot(index as u32));
            }
        }
        let target = EntitySlot(destination.bodies.len() as u32);
        debug_assert_eq!(destination.bodies.len(), destination.cold.len());
        debug_assert_eq!(destination.impulse.len(), destination.cold.len());
        destination.max_half_width = destination.max_half_width.max(body.half_width);
        destination.pos.push(position);
        destination.vel.push(velocity);
        destination
            .rot
            .push([rotation[0], rotation[1], rotation[0]]);
        destination.ground.push(false);
        destination.bodies.push(body);
        destination.impulse.push(impulse);
        destination.cold.push(row);
        destination.touched_flag.push(true);
        destination
            .wire_marked
            .push(destination.wire_removed.contains(&id));
        destination.touched.push(id);
        destination.flags.push(0);
        destination.hints.push(MotionHint {
            special: None,
            swell_metadata_neutral: true,
        });
        destination.dirty_flag.push(true);
        destination.dirty_ids.push(id);
        destination.id_index.insert(id, target);
        destination.uuid_index.insert(uuid, target);
        destination.set_next_wire_id(id.0);
        destination.index_brain_body(
            id,
            destination.bodies[target.0 as usize].kind,
            position,
            true,
            target,
        );
        if Self::resident_kind(destination.bodies[target.0 as usize].kind) {
            destination.brain_revision = destination.brain_revision.wrapping_add(1);
        }
        if orb {
            destination.orbs.insert(id);
        }
        if tnt {
            destination.tnt.insert(id);
        }
        if warden {
            destination.register_warden(id);
        }
        destination.index_tick_members(target);
        target
    }

    pub(crate) fn slot_of(&self, id: EntityId) -> Option<EntitySlot> {
        let slot = *self.id_index.get(&id)?;
        // Stale index entries are impossible (remove fixes up), but verify
        // cheaply: wrong slot means not found; dead rows are NOT found here
        // (live-target validation). Use `slot_of_any` for existing-row
        // lookup (respawn, persistence, death-removal).
        let body = self.bodies.get(slot.0 as usize)?;
        (body.id == id && body.alive).then_some(slot)
    }

    /// Existing-row lookup regardless of `alive`: dead entities stay
    /// addressable for respawn, death-drop persistence and removal.
    pub(crate) fn slot_of_any(&self, id: EntityId) -> Option<EntitySlot> {
        let slot = *self.id_index.get(&id)?;
        let body = self.bodies.get(slot.0 as usize)?;
        (body.id == id).then_some(slot)
    }

    /// Live UUID lookup for durable references (projectile owners, riders).
    #[must_use]
    pub fn slot_of_uuid(&self, uuid: Uuid) -> Option<EntitySlot> {
        self.slot_of_uuid_any(uuid)
            .filter(|slot| self.bodies[slot.0 as usize].alive)
    }

    /// Indexed durable identity lookup while a dead row awaits native removal.
    pub(crate) fn slot_of_uuid_any(&self, uuid: Uuid) -> Option<EntitySlot> {
        let slot = *self.uuid_index.get(&uuid)?;
        let row = self.cold.get(slot.0 as usize)?;
        (row.uuid == uuid).then_some(slot)
    }

    /// Replace a durable identity without leaving either UUID index stale.
    pub(crate) fn set_uuid(&mut self, slot: EntitySlot, uuid: Uuid) {
        let row = &mut self.cold[slot.0 as usize];
        if row.uuid == uuid {
            return;
        }
        let old = std::mem::replace(&mut row.uuid, uuid);
        let id = self.bodies[slot.0 as usize].id;
        self.uuid_index.remove(&old);
        self.uuid_index.insert(uuid, slot);
        self.entity_changed(id, false, slot);
    }

    /// Publish only mutation-owned candidates. Journal marks are not drained:
    /// durable cold/chunk-crossing policy is independent of the wire projection.
    pub fn views(&mut self) -> crate::boundary::EntityViews {
        if self.wire_pending.is_empty() {
            return self.wire_views.clone();
        }
        let mut pending = std::mem::take(&mut self.wire_pending);
        // Publish in identity order: lifecycle slot assignment stays deterministic.
        if pending.len() >= 8192 {
            use rayon::slice::ParallelSliceMut;
            pending.par_sort_unstable_by_key(|&(id, _)| id);
        } else {
            pending.sort_unstable_by_key(|&(id, _)| id);
        }
        for &(id, hint) in &pending {
            let slot = if self
                .bodies
                .get(hint.0 as usize)
                .is_some_and(|body| body.id == id)
            {
                Some(hint)
            } else {
                self.slot_of_any(id)
            };
            let Some(slot) = slot else {
                self.nameplates.remove(&id);
                continue;
            };
            let row = &self.cold[slot.0 as usize];
            if row.villager.is_some() {
                self.nameplates
                    .entry(id)
                    .and_modify(|plate| plate.refresh(row))
                    .or_insert_with(|| crate::villager::nameplates::Nameplate::new(row));
            } else {
                self.nameplates.remove(&id);
            }
        }
        // All queued identities are consumed, including removal tombstones.
        self.wire_marked.fill(false);
        self.wire_removed.clear();
        let mut views = std::mem::take(&mut self.wire_views);
        views.publish(
            &pending,
            |&(id, hint)| {
                // Compaction, transfer, or rebirth can invalidate a mark-time slot.
                // Stable slots avoid a second ID-index probe at publication.
                let slot = if self
                    .bodies
                    .get(hint.0 as usize)
                    .is_some_and(|body| body.id == id)
                {
                    Some(hint)
                } else {
                    self.slot_of_any(id)
                };
                (id, slot)
            },
            |slot| self.wire_view(slot),
        );
        self.wire_views = views;
        pending.clear();
        self.wire_pending = pending;
        self.wire_views.clone()
    }

    fn wire_view(&self, slot: EntitySlot) -> EntityView {
        let slot = slot.0 as usize;
        let body = &self.bodies[slot];
        let row = &self.cold[slot];
        let rot = self.rot[slot];
        EntityView {
            id: body.id,
            uuid: row.uuid,
            // Clients only know vanilla types: data-defined blueprints
            // present as theirs (ADR 0016).
            type_id: match body.kind {
                EntityKind::Player { .. } => player_type_id(),
                EntityKind::Mob { type_id } => crate::blueprint::wire_type_id(type_id),
                EntityKind::Item { type_id } | EntityKind::ExperienceOrb { type_id } => type_id,
            },
            position: self.pos[slot],
            velocity: self.vel[slot],
            yaw: rot[0],
            pitch: rot[1],
            head_yaw: rot[2],
            on_ground: self.ground[slot],
            appearance: if row.villager.is_some() {
                Arc::clone(
                    &self
                        .nameplates
                        .get(&body.id)
                        .expect("projected villager nameplate")
                        .appearance,
                )
            } else {
                Arc::clone(&row.appearance)
            },
        }
    }

    /// Number of entities in the store.
    pub fn len(&self) -> usize {
        self.bodies.len()
    }

    /// Whether the store holds no entity.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.bodies.is_empty()
    }

    pub fn pos(&self, slot: EntitySlot) -> [f64; 3] {
        self.pos[slot.0 as usize]
    }

    pub fn vel(&self, slot: EntitySlot) -> [f64; 3] {
        self.vel[slot.0 as usize]
    }

    pub fn row(&self, slot: EntitySlot) -> &ColdRow {
        &self.cold[slot.0 as usize]
    }

    pub fn body(&self, slot: EntitySlot) -> &Body {
        &self.bodies[slot.0 as usize]
    }

    /// Mutable body custody: same marks as [`Self::row_mut`] (motion-dirty,
    /// touched, wire/brain notification).
    #[cfg_attr(test, track_caller)]
    pub fn body_mut(&mut self, slot: EntitySlot) -> &mut Body {
        let index = slot.0 as usize;
        self.mark_motion_dirty(index);
        self.mark_touched(index);
        let body = &self.bodies[index];
        self.entity_changed(body.id, self.cold[index].villager.is_some(), slot);
        &mut self.bodies[index]
    }

    /// Mixed hot/cold custody for one slot: same marks as [`Self::row_mut`].
    /// The split borrow keeps both columns live without aliasing.
    pub fn parts_mut(&mut self, slot: EntitySlot) -> (&mut Body, &mut ColdRow) {
        let index = slot.0 as usize;
        self.mark_motion_dirty(index);
        self.mark_touched(index);
        let body = &self.bodies[index];
        self.entity_changed(body.id, self.cold[index].villager.is_some(), slot);
        (&mut self.bodies[index], &mut self.cold[index])
    }

    pub fn horizontal_impulse(&self, slot: EntitySlot) -> [f64; 2] {
        self.impulse[slot.0 as usize]
    }

    /// Combat momentum custody, with the same mutation marks as `row_mut`.
    /// The brain's transient projection uses `set_brain_impulse` instead.
    pub fn set_horizontal_impulse(&mut self, slot: EntitySlot, value: [f64; 2]) {
        let index = slot.0 as usize;
        self.mark_motion_dirty(index);
        self.mark_touched(index);
        let body = &self.bodies[index];
        self.entity_changed(body.id, self.cold[index].villager.is_some(), slot);
        self.impulse[index] = value;
    }

    pub fn bodies(&self) -> &[Body] {
        &self.bodies
    }

    pub fn positions(&self) -> &[[f64; 3]] {
        &self.pos
    }

    pub(crate) fn velocities(&self) -> &[[f64; 3]] {
        &self.vel
    }

    pub(crate) fn rotations(&self) -> &[[f32; 3]] {
        &self.rot
    }

    pub(crate) fn grounds(&self) -> &[bool] {
        &self.ground
    }

    pub fn impulses(&self) -> &[[f64; 2]] {
        &self.impulse
    }

    /// Vanilla `LivingEntity.hurt` invulnerability: for
    /// [`HURT_INVULNERABLE_TICKS`] after a hit only a stronger hit lands, and
    /// only its excess. `/kill` and the void bypass it. Returns the amount
    /// that lands (before armor). Mixed custody preserves the old `row_mut`
    /// boundary, including attempted hits that do not change the final image.
    pub fn land_hit(
        &mut self,
        slot: EntitySlot,
        tick: u64,
        amount: f32,
        kind: crate::survival::DamageKind,
    ) -> f32 {
        let (body, row) = self.parts_mut(slot);
        if kind == crate::survival::DamageKind::Void {
            return amount;
        }
        if row.controls.invulnerable || tick < row.protected_until {
            return 0.0;
        }
        if body
            .hurt_at
            .is_some_and(|at| tick.saturating_sub(at) < HURT_INVULNERABLE_TICKS)
        {
            if amount <= row.last_hurt {
                return 0.0;
            }
            let excess = amount - row.last_hurt;
            row.last_hurt = amount;
            return excess;
        }
        body.hurt_at = Some(tick);
        row.last_hurt = amount;
        amount
    }

    pub(crate) fn max_half_width(&self) -> f64 {
        self.max_half_width
    }

    /// Dimension changes must keep local body queries conservative, including
    /// growth, imported slime sizes, and armor-stand marker transitions.
    pub(crate) fn set_half_width(&mut self, slot: EntitySlot, half_width: f64) {
        self.max_half_width = self.max_half_width.max(half_width);
        if self.body(slot).half_width != half_width {
            self.body_mut(slot).half_width = half_width;
        }
    }

    /// Idle accrual is durable cold state, not a wire-image or motion change.
    pub(crate) fn set_no_action_time(&mut self, slot: EntitySlot, no_action_time: u32) {
        let index = slot.0 as usize;
        if self.cold[index].no_action_time == no_action_time {
            return;
        }
        self.mark_touched(index);
        let id = self.bodies[index].id;
        let row = &mut self.cold[index];
        row.no_action_time = no_action_time;
        if row.villager.is_some() {
            self.brain_changes.insert(id);
        }
    }

    /// Egg countdowns are deterministic replay projections, not hint or wire
    /// inputs. The lifecycle owner separately marks initialization/reset events.
    pub(crate) fn set_harvest_ticks(&mut self, slot: EntitySlot, ticks: u32) {
        self.cold[slot.0 as usize].entity_life.harvest_ticks = ticks;
    }

    /// Mutable row custody. Taking it marks the row touched for the journal:
    /// the journal compares/emits only touched rows. Equality of the row with
    /// its last journaled image stays the final arbiter, so a conservative
    /// (read-only) borrow costs a comparison, never correctness.
    #[cfg_attr(test, track_caller)]
    pub fn row_mut(&mut self, slot: EntitySlot) -> &mut ColdRow {
        let index = slot.0 as usize;
        #[cfg(test)]
        {
            static TRACE: std::sync::LazyLock<bool> =
                std::sync::LazyLock::new(|| std::env::var_os("SOLARIS_TRACE_ROW_MUT").is_some());
            if *TRACE {
                let caller = std::panic::Location::caller();
                let counts = self
                    .touch_sites
                    .entry((caller.file(), caller.line()))
                    .or_default();
                counts[0] += 1;
                counts[1] += usize::from(self.cold[index].villager.is_some());
                counts[2] += usize::from(!self.touched_flag[index]);
            }
        }
        self.mark_motion_dirty(index);
        self.mark_touched(index);
        let body = &self.bodies[index];
        self.entity_changed(body.id, self.cold[index].villager.is_some(), slot);
        &mut self.cold[index]
    }

    fn mark_motion_dirty(&mut self, index: usize) {
        if !std::mem::replace(&mut self.dirty_flag[index], true) {
            self.dirty_ids.push(self.bodies[index].id);
        }
    }

    fn mark_touched(&mut self, index: usize) {
        if !std::mem::replace(&mut self.touched_flag[index], true) {
            self.touched.push(self.bodies[index].id);
        }
    }

    /// Internal transactional custody; callers must publish actual durable changes
    /// through `row_mut`, while cache-only/no-op updates remain unmarked.
    pub(crate) fn row_mut_unmarked(&mut self, slot: EntitySlot) -> &mut ColdRow {
        let index = slot.0 as usize;
        self.mark_motion_dirty(index);
        &mut self.cold[index]
    }

    /// Serial movement writeback after the parallel entity step.
    /// Only air is a retained image key. Impulse and attack/pickup timers are
    /// transient or covered by full images, and none changes the wire view.
    pub(crate) fn set_motion_state(
        &mut self,
        slot: EntitySlot,
        horizontal_impulse: [f64; 2],
        air_supply: i32,
        pickup_delay: u32,
        attack_cooldown: u32,
    ) {
        let index = slot.0 as usize;
        let body = &self.bodies[index];
        if self.impulse[index] != horizontal_impulse
            || body.air_supply != air_supply
            || body.pickup_delay != pickup_delay
            || body.attack_cooldown != attack_cooldown
        {
            if body.air_supply != air_supply {
                let (body, _) = self.parts_mut(slot);
                body.air_supply = air_supply;
                body.pickup_delay = pickup_delay;
                body.attack_cooldown = attack_cooldown;
                self.impulse[index] = horizontal_impulse;
            } else {
                self.mark_motion_dirty(index);
                self.bodies[index].pickup_delay = pickup_delay;
                self.bodies[index].attack_cooldown = attack_cooldown;
                self.impulse[index] = horizontal_impulse;
            }
        }
    }

    /// Serial movement writeback for one entity step: pos/vel/rotation/ground
    /// and motion state land together under a single wire/brain notification.
    /// Equivalent to `set_pos; set_vel; set_ground; set_rot; set_motion_state`
    /// (pickup delay always decrements): a chunk-crossing move still reindexes
    /// and lists the row for the journal, and only an air change takes
    /// `row_mut` custody. Returns the previous position.
    pub(crate) fn apply_motion_result(
        &mut self,
        slot: EntitySlot,
        result: &MotionResult,
    ) -> [f64; 3] {
        let index = slot.0 as usize;
        let old = self.pos[index];
        let mut wire = false;
        if old != result.pos {
            if Self::brain_chunk(old) != Self::brain_chunk(result.pos) {
                self.reindex_moved_body(slot, old, result.pos);
            }
            self.pos[index] = result.pos;
            wire = true;
        }
        if self.vel[index] != result.vel {
            self.vel[index] = result.vel;
            wire = true;
        }
        let rot = [result.yaw, result.pitch, result.yaw];
        if self.rot[index] != rot {
            self.rot[index] = rot;
            wire = true;
        }
        if self.ground[index] != result.ground {
            self.ground[index] = result.ground;
            wire = true;
        }
        let (air_changed, motion_changed) =
            apply_motion_state(&mut self.bodies, &mut self.impulse, index, result);
        if motion_changed {
            self.mark_motion_dirty(index);
            if air_changed {
                self.mark_touched(index);
            }
        }
        if air_changed || wire {
            let body = &self.bodies[index];
            let resident = air_changed && self.cold[index].villager.is_some();
            self.entity_changed(body.id, resident, slot);
        }
        old
    }

    /// Shared read-only admission fence; the journal remains the sole drainer.
    pub(crate) fn touched_ids(&self) -> &[EntityId] {
        &self.touched
    }

    /// Rows spawned, transferred in or mutably borrowed since the last call.
    /// Every live row is either in the journal's baseline maps or in this set.
    pub(crate) fn take_touched(&mut self) -> Vec<EntityId> {
        let ids = std::mem::take(&mut self.touched);
        for id in &ids {
            if let Some(slot) = self.id_index.get(id) {
                self.touched_flag[slot.0 as usize] = false;
            }
        }
        ids
    }

    /// Return the drained journal scratch allocation instead of growing it
    /// again on the next tick. Journal extraction cannot mutate row custody.
    pub(crate) fn recycle_touched(&mut self, mut ids: Vec<EntityId>) {
        debug_assert!(self.touched.is_empty());
        ids.clear();
        self.touched = ids;
    }

    /// Opt-in bench-only attribution; no tracing exists in a production build.
    #[cfg(test)]
    pub(crate) fn take_touch_sites(
        &mut self,
    ) -> std::collections::BTreeMap<(&'static str, u32), [usize; 3]> {
        std::mem::take(&mut self.touch_sites)
    }

    /// Ids that left this store since the last call.
    pub(crate) fn take_removed(&mut self) -> Vec<EntityId> {
        std::mem::take(&mut self.removed)
    }

    /// Position column write. Any actual move dirties the wire projection;
    /// only a chunk-crossing move additionally lists the row for the journal
    /// (ordinary motion is checkpoint-only, see ADR 0013).
    /// The rare reindex branch stays out of line so ordinary moves can inline.
    #[inline]
    pub fn set_pos(&mut self, slot: EntitySlot, pos: [f64; 3]) {
        let index = slot.0 as usize;
        let old = self.pos[index];
        if old == pos {
            return;
        }
        if Self::brain_chunk(old) != Self::brain_chunk(pos) {
            self.reindex_moved_body(slot, old, pos);
        }
        self.pos[index] = pos;
        self.entity_changed(self.bodies[index].id, false, slot);
    }

    #[inline(never)]
    fn reindex_moved_body(&mut self, slot: EntitySlot, old: [f64; 3], pos: [f64; 3]) {
        let index = slot.0 as usize;
        let body = &self.bodies[index];
        let (id, kind) = (body.id, body.kind);
        self.index_brain_body(id, kind, old, false, slot);
        self.index_brain_body(id, kind, pos, true, slot);
        self.mark_touched(index);
    }

    pub fn set_vel(&mut self, slot: EntitySlot, vel: [f64; 3]) {
        let index = slot.0 as usize;
        if self.vel[index] != vel {
            self.vel[index] = vel;
            self.entity_changed(self.bodies[index].id, false, slot);
        }
    }

    /// Yaw/pitch for a slot.
    #[must_use]
    pub fn rot(&self, slot: EntitySlot) -> [f32; 3] {
        self.rot.get(slot.0 as usize).copied().unwrap_or([0.0; 3])
    }

    pub fn set_rot(&mut self, slot: EntitySlot, yaw: f32, pitch: f32) {
        let index = slot.0 as usize;
        let next = [yaw, pitch, yaw];
        if self.rot[index] != next {
            self.rot[index] = next;
            self.entity_changed(self.bodies[index].id, false, slot);
        }
    }

    /// On-ground flag for a slot.
    #[must_use]
    pub fn ground(&self, slot: EntitySlot) -> bool {
        self.ground.get(slot.0 as usize).copied().unwrap_or(false)
    }

    pub fn set_ground(&mut self, slot: EntitySlot, ground: bool) {
        let index = slot.0 as usize;
        if self.ground[index] != ground {
            self.ground[index] = ground;
            self.entity_changed(self.bodies[index].id, false, slot);
        }
    }

    /// Compact flag scan for exact-mask admission; never touches ColdRow or Body.
    pub(crate) fn motion_flags(&self, slot: EntitySlot) -> u16 {
        self.flags[slot.0 as usize]
    }

    /// Slots whose flags contain `mask`, in global slot order.
    pub(crate) fn motion_slots(&self, mask: u16) -> impl Iterator<Item = EntitySlot> + '_ {
        self.flags
            .iter()
            .enumerate()
            .filter(move |(_, flags)| **flags & mask == mask)
            .map(|(index, _)| EntitySlot(index as u32))
    }

    pub(crate) fn motion_hint(&self, slot: EntitySlot) -> &MotionHint {
        &self.hints[slot.0 as usize]
    }

    /// All live slots in order.
    pub fn slots(&self) -> impl Iterator<Item = EntitySlot> {
        (0..self.bodies.len() as u32).map(EntitySlot)
    }

    /// Native mob type members, including dead rows, in dense-slot order.
    pub(crate) fn mob_slots(&self, type_id: u32) -> impl Iterator<Item = EntitySlot> + '_ {
        self.mob_members
            .get(&type_id)
            .into_iter()
            .flat_map(|slots| slots.iter().copied())
    }

    pub(crate) fn player_slots(&self) -> impl Iterator<Item = EntitySlot> + '_ {
        self.player_members.iter().copied()
    }

    pub(crate) fn item_slots(&self) -> impl Iterator<Item = EntitySlot> + '_ {
        self.item_members.iter().copied()
    }

    /// Merge selected type lists without changing the old full-scan apply order.
    pub(crate) fn mob_slots_matching_into(
        &self,
        accepts: impl Fn(u32) -> bool,
        slots: &mut Vec<EntitySlot>,
    ) {
        slots.clear();
        for (&type_id, members) in &self.mob_members {
            if accepts(type_id) {
                slots.extend(members.iter().copied());
            }
        }
        slots.sort_unstable();
    }

    /// Merge selected type lists without changing the old full-scan apply order.
    pub(crate) fn mob_slots_matching(&self, accepts: impl Fn(u32) -> bool) -> Vec<EntitySlot> {
        let mut slots = Vec::new();
        self.mob_slots_matching_into(accepts, &mut slots);
        slots
    }

    fn index_tick_members(&mut self, slot: EntitySlot) {
        let body = &self.bodies[slot.0 as usize];
        match body.kind {
            EntityKind::Mob { type_id } => {
                self.mob_members.entry(type_id).or_default().insert(slot);
            }
            EntityKind::Player { .. } => {
                self.player_members.insert(slot);
            }
            EntityKind::Item { .. } => {
                self.item_members.insert(slot);
            }
            EntityKind::ExperienceOrb { .. } => {}
        }
        if self.cold[slot.0 as usize].entity_life.mount.rider.is_some() {
            self.rider_members.insert(slot);
        }
        if self.cold[slot.0 as usize].entity_life.leash.is_some() {
            self.leash_members.insert(slot);
        }
    }

    fn unindex_tick_members(&mut self, slot: EntitySlot, kind: EntityKind) {
        match kind {
            EntityKind::Mob { type_id } => {
                if let Some(slots) = self.mob_members.get_mut(&type_id) {
                    slots.remove(&slot);
                    if slots.is_empty() {
                        self.mob_members.remove(&type_id);
                    }
                }
            }
            EntityKind::Player { .. } => {
                self.player_members.remove(&slot);
            }
            EntityKind::Item { .. } => {
                self.item_members.remove(&slot);
            }
            EntityKind::ExperienceOrb { .. } => {}
        }
        self.rider_members.remove(&slot);
        self.leash_members.remove(&slot);
    }

    pub(crate) fn set_kind(&mut self, slot: EntitySlot, kind: EntityKind) {
        self.unindex_tick_members(slot, self.body(slot).kind);
        self.body_mut(slot).kind = kind;
        self.index_tick_members(slot);
    }

    pub(crate) fn rider_slots(&self) -> impl Iterator<Item = EntitySlot> + '_ {
        self.rider_members.iter().copied()
    }

    pub(crate) fn leashed_slots(&self) -> impl Iterator<Item = EntitySlot> + '_ {
        self.leash_members.iter().copied()
    }

    pub(crate) fn set_rider(&mut self, slot: EntitySlot, rider: Option<Uuid>) {
        self.row_mut(slot).entity_life.mount.rider = rider;
        if rider.is_some() {
            self.rider_members.insert(slot);
        } else {
            self.rider_members.remove(&slot);
        }
    }

    pub(crate) fn set_leash(&mut self, slot: EntitySlot, leash: Option<crate::leash::Holder>) {
        if leash.is_some() {
            self.leash_members.insert(slot);
        } else {
            self.leash_members.remove(&slot);
        }
        self.row_mut(slot).entity_life.leash = leash;
    }

    pub(crate) fn set_entity_life(
        &mut self,
        slot: EntitySlot,
        life: crate::entity_life::EntityLifeState,
    ) {
        self.rider_members.remove(&slot);
        self.leash_members.remove(&slot);
        if life.mount.rider.is_some() {
            self.rider_members.insert(slot);
        }
        if life.leash.is_some() {
            self.leash_members.insert(slot);
        }
        self.row_mut(slot).entity_life = life;
    }

    pub(crate) fn experience_orbs(&self) -> impl Iterator<Item = EntityId> + '_ {
        self.orbs.iter().copied()
    }

    pub(crate) fn track_tnt(&mut self, id: EntityId) {
        self.tnt.insert(id);
    }

    pub(crate) fn next_tnt(&self, after: Option<EntityId>) -> Option<EntityId> {
        match after {
            None => self.tnt.first().copied(),
            Some(after) => self
                .tnt
                .range((std::ops::Bound::Excluded(after), std::ops::Bound::Unbounded))
                .next()
                .copied(),
        }
    }

    /// Refresh motion flags for dirty slots only. Resolves each queued
    /// stable id to its current slot through `id_index` (removed ids are
    /// ignored) and retains the queue capacity across ticks.
    pub(crate) fn prepare_motion_hints(&mut self, registry: &crate::registry::CoreRegistry) {
        if self.dirty_ids.is_empty() {
            return;
        }
        static CHICKEN: std::sync::LazyLock<mc_data::Identifier> = std::sync::LazyLock::new(|| {
            mc_data::Identifier::parse("minecraft:chicken").expect("static identifier parses")
        });
        static TADPOLE: std::sync::LazyLock<mc_data::Identifier> = std::sync::LazyLock::new(|| {
            mc_data::Identifier::parse("minecraft:tadpole").expect("static identifier parses")
        });
        let chicken = registry.entity_types.id_of(&CHICKEN);
        let tadpole = registry.entity_types.id_of(&TADPOLE);
        let dirty = std::mem::take(&mut self.dirty_ids);
        for id in &dirty {
            let Some(slot) = self.id_index.get(id).copied() else {
                continue;
            };
            let index = slot.0 as usize;
            let Some(body) = self.bodies.get(index) else {
                continue;
            };
            if body.id != *id {
                continue;
            }
            self.dirty_flag[index] = false;
            let special = effective_special(registry, body, &self.cold[index]);
            self.flags[index] = motion_flags_for(
                registry,
                body,
                &self.cold[index],
                self.impulse[index],
                special,
                chicken,
                tadpole,
            );
            self.hints[index] = MotionHint {
                special,
                swell_metadata_neutral: crate::hostile::swell_metadata_neutral(&self.cold[index]),
            };
        }
        let mut dirty = dirty;
        dirty.clear();
        self.dirty_ids = dirty;
    }

    #[cfg(test)]
    pub(crate) fn dirty_ids_len_for_test(&self) -> usize {
        self.dirty_ids.len()
    }

    pub(crate) fn last_tnt(&self) -> Option<EntityId> {
        self.tnt.last().copied()
    }
}

/// Exact tick native-motion gate shared with the entity phase.
fn motion_owned(body: &Body, row: &ColdRow) -> bool {
    !crate::special_mobs::raiders::owns(body.kind)
        && row.falling.is_none()
        && row.tnt.is_none()
        && row.entity_life.vehicle.is_none()
        && !row
            .villager
            .as_ref()
            .is_some_and(|v| !v.wandering && v.conversion_time.is_none())
        && !matches!(body.kind, EntityKind::ExperienceOrb { .. })
}

/// Blueprint special visible to motion gating: mobs without NoAI only.
fn effective_special(
    registry: &crate::registry::CoreRegistry,
    body: &Body,
    row: &ColdRow,
) -> Option<crate::hostile::Special> {
    let EntityKind::Mob { type_id } = body.kind else {
        return None;
    };
    if row.controls.no_ai {
        return None;
    }
    registry
        .blueprint(type_id)
        .and_then(crate::hostile::special_of)
}

fn motion_rest_base(
    registry: &crate::registry::CoreRegistry,
    body: &Body,
    row: &ColdRow,
    impulse: [f64; 2],
    special: Option<crate::hostile::Special>,
) -> bool {
    if !body.alive
        || !matches!(body.kind, EntityKind::Mob { .. })
        || body.movement != MovementClass::Walker
        || impulse != [0.0; 2]
        || row.controls.no_ai
        || row.special_mob.is_some()
        || row.love != 0
        || body.burn_ticks != 0
        || crate::taming::suppress_ai(row)
    {
        return false;
    }
    let EntityKind::Mob { type_id } = body.kind else {
        return false;
    };
    let Some(blueprint) = registry.blueprint(type_id) else {
        return false;
    };
    if blueprint.goals.is_empty()
        || blueprint.name == "minecraft:creaking"
        || !matches!(
            special,
            None | Some(crate::hostile::Special::Ranged) | Some(crate::hostile::Special::Swell)
        )
    {
        return false;
    }
    true
}

fn motion_effects(body: &Body, row: &ColdRow) -> bool {
    if matches!(body.kind, EntityKind::Player { .. }) {
        return false;
    }
    !row.active_effects.entries.is_empty()
        || !row.appearance.effect_particles.is_empty()
        || row.appearance.absorption > 0.0
        || row.appearance.shared_flags & 0x60 != 0
}

fn motion_life(body: &Body, row: &ColdRow, chicken: Option<u32>, tadpole: Option<u32>) -> bool {
    let EntityKind::Mob { type_id } = body.kind else {
        return row.appearance.arrow_count > 0 || row.appearance.stinger_count > 0;
    };
    if !body.alive {
        return row.appearance.arrow_count > 0 || row.appearance.stinger_count > 0;
    }
    let mob_clock = row.breed_cooldown > 0
        || row.love > 0
        || (!row.entity_life.age_lock.locked
            && (row.entity_life.child_ticks > 0 || Some(type_id) == tadpole))
        || (Some(type_id) == chicken && crate::entity_life::adult(row));
    mob_clock || row.appearance.arrow_count > 0 || row.appearance.stinger_count > 0
}

fn motion_flags_for(
    registry: &crate::registry::CoreRegistry,
    body: &Body,
    row: &ColdRow,
    impulse: [f64; 2],
    special: Option<crate::hostile::Special>,
    chicken: Option<u32>,
    tadpole: Option<u32>,
) -> u16 {
    let phase = ((body.id.0 as u64 & 3) as u16) << MOTION_PHASE_SHIFT;
    let mut flags: u16 = phase;
    if motion_owned(body, row) {
        flags |= MOTION_OWNED;
    }
    if matches!(body.kind, EntityKind::Player { .. }) {
        flags |= MOTION_PLAYER;
    }
    if body.alive {
        flags |= MOTION_ALIVE;
    }
    if special.is_some() {
        flags |= MOTION_SPECIAL;
    }
    if motion_rest_base(registry, body, row, impulse, special) {
        flags |= MOTION_REST;
        // No alternate movement goal, standing override, breeding or panic.
        if let EntityKind::Mob { type_id } = body.kind
            && let Some(blueprint) = registry.blueprint(type_id)
            && !matches!(
                crate::piglin::name(type_id),
                "minecraft:hoglin" | "minecraft:piglin"
            )
            && !crate::dolphin::is_dolphin(type_id)
        {
            let chaser = blueprint.has_goal(crate::blueprint::GoalKind::MeleeChase)
                || matches!(
                    special,
                    Some(crate::hostile::Special::Ranged | crate::hostile::Special::Swell)
                );
            if !chaser {
                flags |= MOTION_WANDER_REST;
            } else if !matches!(
                blueprint.name.as_str(),
                "minecraft:zombie" | "minecraft:zombie_villager" | "minecraft:iron_golem"
            ) && crate::piglin::name(type_id) != "minecraft:zoglin"
            {
                // Other chasers only search players. If that whole target
                // table is empty, they reach the same final wander branch.
                flags |= MOTION_TARGETLESS_WANDER_REST;
            }
        }
    }
    if motion_effects(body, row) {
        flags |= MOTION_EFFECTS;
    }
    if motion_life(body, row, chicken, tadpole) {
        flags |= MOTION_LIFE;
    }
    if !matches!(body.kind, EntityKind::Player { .. })
        && row.appearance.equipment[2..6].iter().any(Option::is_some)
    {
        flags |= MOTION_ARMORED;
    }
    if body.pickup_delay == 0 && body.attack_cooldown == 0 && impulse == [0.0; 2] {
        flags |= MOTION_QUIET_TIMERS;
    }
    flags
}

/// Cold fields for [`EntityStore::spawn`].
#[derive(Debug, Clone)]
pub struct ColdFields {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub ground: bool,
    pub health: f32,
    pub breed_cooldown: u32,
    pub pickup_delay: u32,
    pub item: Option<StackRow>,
    pub appearance: Arc<EntityAppearance>,
    pub movement: MovementClass,
    pub half_width: f64,
    pub height: f64,
    pub speed: f32,
    pub attack_damage: f32,
    pub sight_range: f32,
    pub attack_reach: f32,
}

/// Protocol type id of `minecraft:player` for player views.
pub(crate) fn player_type_id() -> u32 {
    use std::sync::LazyLock;
    static ID: LazyLock<u32> = LazyLock::new(|| {
        mc_data::entity_types::solaris_required_entity_types()
            .id_of(&mc_data::Identifier::parse("minecraft:player").expect("static id parses"))
            .expect("player entity type present")
    });
    *ID
}

#[cfg(test)]
#[path = "entities_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "entity_motion_hint_tests.rs"]
mod motion_hint_tests;

#[cfg(test)]
#[path = "entity_body_tests.rs"]
mod body_tests;
