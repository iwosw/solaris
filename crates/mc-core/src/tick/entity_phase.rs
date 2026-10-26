//! Entity phase: worker row preparation, parallel `step_entity` and pickup
//! intent collection. Moved verbatim from `tick.rs`; stage order unchanged.

use super::*;

#[cfg(test)]
#[path = "../entity_phase_tests.rs"]
mod entity_phase_tests;

#[cfg(test)]
#[path = "../sensor_bench_tests.rs"]
mod sensor_bench_tests;

#[cfg(test)]
#[path = "../entity_prephase_tests.rs"]
mod entity_prephase_tests;

/// Hot worker snapshot; rare state is read by slot while the store is immutable.
#[derive(Debug, Clone, Copy)]
pub(super) struct ReadRow {
    pub(super) slot: EntitySlot,
    pub(super) id: EntityId,
    /// Current column's ID-staggered activation decision.
    pub(super) think: bool,
    pub(super) pos: [f64; 3],
    pub(super) panic_multiplier: f64,
    /// Velocity written last tick: the standing decision between AI thinks.
    pub(super) vel: [f64; 3],
    pub(super) rotation: [f32; 3],
    pub(super) movement: crate::blueprint::MovementClass,
    pub(super) air_supply: i32,
    pub(super) horizontal_impulse: [f64; 2],
    pub(super) alive: bool,
    pub(super) ground: bool,
    pub(super) half_width: f64,
    pub(super) height: f64,
    pub(super) sight_range: f32,
    pub(super) attack_reach: f32,
    pub(super) attack_damage: f32,
    pub(super) melee_bonus: f32,
    pub(super) attack_cooldown: u32,
    pub(super) love: u32,
    pub(super) is_enderman: bool,
    pub(super) is_iron_golem: bool,
    pub(super) hunts_villagers: bool,
    pub(super) hunts_living: bool,
    pub(super) is_slime_size_one: bool,
    /// Blueprint locomotion capabilities read by the motion step.
    pub(super) climbs_walls: bool,
    pub(super) aquatic_control: crate::species::AquaticControl,
    /// Resolved from the blueprint's goal set (data, not type names).
    pub(super) chaser: bool,
    /// Breed goal, eligibility, adulthood, cooldown and love checked once.
    pub(super) breed_ready: bool,
    pub(super) temptable: bool,
    /// Blueprint key used for pair compatibility and following.
    pub(super) type_id: u32,
    /// Mature item snapshot (item kind, alive, zero pickup delay).
    pub(super) pickup_ready: bool,
    pub(super) item_entity: bool,
    pub(super) player: bool,
    pub(super) speed: f32,
    pub(super) enderman_target: Option<EntityId>,
    pub(super) levitation: f32,
    pub(super) slow_falling: bool,
    pub(super) special: Option<crate::hostile::Special>,
    /// Target acquisition uses local light, except recent retaliation.
    pub(super) dark_aggro: bool,
    /// A lit fuse holds the mob still.
    pub(super) standing: bool,
    pub(super) plugin_target: Option<[f64; 3]>,
    pub(super) leash_follow: bool,
    pub(super) gold_target: Option<[f64; 3]>,
    pub(super) has_retained_target: bool,
    pub(super) retain_target: bool,
}

/// Owned movement write from one worker.
pub(super) struct MoveWrite {
    pub(super) slot: EntitySlot,
    pub(super) pos: [f64; 3],
    pub(super) vel: [f64; 3],
    pub(super) rotation: [f32; 3],
    pub(super) air_supply: i32,
    pub(super) ground: bool,
    pub(super) horizontal_impulse: [f64; 2],
    /// Only a newly acquired target needs serial memory writeback.
    pub(super) chase_target: crate::target_grid::Nearest,
}

#[derive(Default)]
pub(super) struct EntityPhaseScratch {
    pub(super) rows: Vec<ReadRow>,
    pub(super) writes: Vec<Option<(MoveWrite, Vec<RawIntent>)>>,
    pub(super) specials: Vec<(EntityId, crate::hostile::Special)>,
    pub(super) players: Vec<(EntityId, [f64; 3])>,
    pub(super) activation: HashMap<(i32, i32), u64>,
    pub(super) periods: Vec<u8>,
    pub(super) resting: Vec<EntitySlot>,
}

pub(super) fn unchanged_entity_footprint(
    pos: [f64; 3],
    half_width: f64,
    chunk_revision: impl Fn(i32, i32) -> u64,
    unchanged_since: u64,
) -> bool {
    let half = half_width + 0.001;
    let chunk = |v: f64| mc_physics::mth::floor_i32(v).div_euclid(16);
    let (x0, x1) = (chunk(pos[0] - half), chunk(pos[0] + half));
    let (z0, z1) = (chunk(pos[2] - half), chunk(pos[2] + half));
    (x0..=x1).all(|cx| (z0..=z1).all(|cz| chunk_revision(cx, cz) <= unchanged_since))
}

pub(super) fn motion_thinks(tick: u64, flags: u16, period: u8) -> bool {
    let phase = ((flags & MOTION_PHASE_MASK) >> MOTION_PHASE_SHIFT) as u8;
    (tick as u8).wrapping_add(phase) & (period - 1) == 0
}

/// Rest before the wide snapshot. Decision ticks additionally prove that the
/// final wander branch's complete motion step is bit-identical to the input.
pub(super) fn early_entity_rest(
    view: &PrepView,
    slot: EntitySlot,
    period: u8,
    unchanged_since: u64,
    special_idle: bool,
    targets_empty: bool,
) -> bool {
    let flags = view.entities.motion_flags(slot);
    if flags & MOTION_REST == 0
        || !special_idle
        || (motion_thinks(view.tick, flags, period)
            && flags & crate::entities::MOTION_WANDER_REST == 0
            && !(targets_empty && flags & crate::entities::MOTION_TARGETLESS_WANDER_REST != 0))
        || !view.ground[slot.0 as usize]
        || view.velocities[slot.0 as usize] != [0.0; 3]
    {
        return false;
    }
    let body = &view.bodies[slot.0 as usize];
    if view.hostile.targets.contains_key(&body.id)
        || body
            .hurt_at
            .is_some_and(|at| view.tick.saturating_sub(at) < 100)
    {
        return false;
    }
    if motion_thinks(view.tick, flags, period)
        && let EntityKind::Mob { type_id } = body.kind
        && view.registry.blueprint(type_id).is_some_and(|blueprint| {
            (blueprint.capabilities.temptable && view.has_held_items)
                || (blueprint.capabilities.follows_adults
                    && !crate::entity_life::adult(view.entities.row(slot)))
        })
    {
        // Player food and adult positions may change inside the same chunk.
        // Decision ticks must observe them even when geometry is unchanged.
        return false;
    }
    if motion_thinks(view.tick, flags, period) {
        // The proof samples current geometry, so unrelated edits anywhere in
        // the footprint's chunk need not reject an otherwise exact no-op.
        return idle_wander_motion_unchanged(view, slot);
    }
    unchanged_entity_footprint(
        view.positions[slot.0 as usize],
        body.half_width,
        |cx, cz| view.world.chunk_revision(cx, cz),
        unchanged_since,
    )
}

/// Keep the same physics, breathing and effect integration as `step_entity`.
/// In particular, an imported on-ground flag is not itself a support proof.
pub(super) fn idle_wander_motion_unchanged(view: &PrepView, slot: EntitySlot) -> bool {
    let body = &view.bodies[slot.0 as usize];
    let row = view.entities.row(slot);
    if crate::entity_motion::wander(body.movement, body.id.0, view.tick, row.speed) != [0.0; 3] {
        return false;
    }
    let pos = view.positions[slot.0 as usize];
    let vel = view.velocities[slot.0 as usize];
    let rotation = view.rotations[slot.0 as usize];
    let physics_body = mc_physics::EntityBody {
        position: mc_physics::Vec3::new(pos[0], pos[1], pos[2]),
        velocity: mc_physics::Vec3::new(vel[0], vel[1], vel[2]),
        aabb: mc_physics::Aabb {
            half_width: body.half_width,
            height: body.height,
        },
        on_ground: view.ground[slot.0 as usize],
    };
    let reader = view.world.physics_reader();
    let sampler = WorkerSampler {
        block_at: &|x, y, z| reader.block_at(x, y, z),
        registry: view.registry,
    };
    let capabilities = match body.kind {
        EntityKind::Mob { type_id } => view
            .registry
            .blueprint(type_id)
            .map(|blueprint| blueprint.capabilities)
            .unwrap_or_default(),
        _ => return false,
    };
    let effects =
        (view.entities.motion_flags(slot) & MOTION_EFFECTS != 0).then_some(&row.active_effects);
    let result = crate::entity_motion::step(
        crate::entity_motion::MotionInput {
            body: physics_body,
            movement: body.movement,
            drive: [0.0; 3],
            impulse: view.impulses[slot.0 as usize],
            rotation,
            look_target: None,
            climbs_walls: capabilities.climbs_walls,
            aquatic_control: capabilities.aquatic,
            air_supply: body.air_supply,
            levitation: effects.map_or(0.0, |effects| {
                effects.level(crate::effects::EffectId::Levitation)
            }),
            slow_falling: effects
                .is_some_and(|effects| effects.has(crate::effects::EffectId::SlowFalling)),
            id: body.id.0,
            tick: view.tick,
        },
        &sampler,
        |x, z| view.world.has_chunk(x.div_euclid(16), z.div_euclid(16)),
    );
    result.body.position.x.to_bits() == pos[0].to_bits()
        && result.body.position.y.to_bits() == pos[1].to_bits()
        && result.body.position.z.to_bits() == pos[2].to_bits()
        && result.body.velocity.x.to_bits() == vel[0].to_bits()
        && result.body.velocity.y.to_bits() == vel[1].to_bits()
        && result.body.velocity.z.to_bits() == vel[2].to_bits()
        && result.body.on_ground == physics_body.on_ground
        && result.rotation.map(f32::to_bits) == rotation.map(f32::to_bits)
        && result.impulse.map(f64::to_bits) == view.impulses[slot.0 as usize].map(f64::to_bits)
        && result.air_supply == body.air_supply
        && !result.drowning
}

/// Live, non-converting resident bodies (villager, wandering trader) in slot
/// order. The resident index replaces a dense scan; slot order is the
/// `TargetGrid` tie-break, so it must match the dense order exactly.
pub(super) fn resident_villager_targets(entities: &EntityStore) -> Vec<(EntityId, [f64; 3])> {
    let mut slots = Vec::new();
    entities.resident_slots_sorted(&mut slots);
    slots
        .into_iter()
        .filter(|&slot| {
            let body = entities.body(slot);
            body.alive
                && entities
                    .row(slot)
                    .villager
                    .as_ref()
                    .is_some_and(|v| v.conversion_time.is_none())
        })
        .map(|slot| (entities.body(slot).id, entities.pos(slot)))
        .collect()
}

/// Prepare every admitted slot (`periods[slot] != 0`) into `out`, in slot
/// order. The parallel path admits disjoint ascending slot ranges over an
/// immutable view (specials, resting and the slots needing rows concatenate in
/// chunk order, which reproduces the serial order), then builds every row
/// straight into `out.rows` with an indexed parallel collect: no per-chunk row
/// vectors and no row copy. Rows needing the `TickState`-only helpers are then
/// rebuilt serially before the combined projection reduction.
pub(super) fn prepare_rows(
    state: &TickState,
    periods: &[u8],
    env: &PrepEnv,
    parallel: bool,
    out: &mut PrepChunk,
) {
    // `out` arrives empty (entity-phase scratch is empty between ticks; only
    // its capacity is reused): the parallel collect replaces `out.rows`.
    debug_assert!(out.rows.is_empty() && out.rare.is_empty());
    let view = PrepView::of(state);
    if parallel {
        use rayon::prelude::*;
        #[derive(Default)]
        struct Admission {
            kept: Vec<(EntitySlot, u16)>,
            specials: Vec<(EntityId, crate::hostile::Special)>,
            resting: Vec<EntitySlot>,
        }
        let parts: Vec<Admission> = periods
            .par_chunks(PREP_CHUNK)
            .enumerate()
            .map(|(chunk, periods)| {
                let base = chunk * PREP_CHUNK;
                let mut part = Admission::default();
                for (offset, &period) in periods.iter().enumerate() {
                    if period != 0
                        && let Some(kept) = admit_slot(
                            &view,
                            base + offset,
                            period,
                            env,
                            &mut part.specials,
                            &mut part.resting,
                        )
                    {
                        part.kept.push(kept);
                    }
                }
                part
            })
            .collect();
        let mut kept = Vec::with_capacity(parts.iter().map(|part| part.kept.len()).sum());
        for part in parts {
            kept.extend(part.kept);
            out.specials.extend(part.specials);
            out.resting.extend(part.resting);
        }
        let mut is_rare = Vec::new();
        kept.par_iter()
            .map(|&(slot, flags)| {
                read_row(
                    &view,
                    slot,
                    flags,
                    periods[slot.0 as usize],
                    &RarePrep::default(),
                )
            })
            .unzip_into_vecs(&mut out.rows, &mut is_rare);
        out.rare.extend(
            is_rare
                .iter()
                .enumerate()
                .filter_map(|(index, &rare): (usize, &bool)| rare.then_some(index)),
        );
    } else {
        for (index, &period) in periods.iter().enumerate() {
            if period != 0 {
                prepare_slot(&view, index, period, env, out);
            }
        }
    }
    for index in std::mem::take(&mut out.rare) {
        let slot = out.rows[index].slot;
        let flags = view.entities.motion_flags(slot);
        let period = periods[slot.0 as usize];
        out.rows[index] = read_row(&view, slot, flags, period, &RarePrep::of(state, slot)).0;
    }
    // Rare helpers can change `chaser` (notably enderman anger), so project
    // only final rows. Chunk collection and concatenation retain row order.
    out.summary = if parallel {
        use rayon::prelude::*;
        let parts: Vec<_> = out
            .rows
            .par_chunks(PREP_CHUNK)
            .enumerate()
            .map(|(chunk, rows)| PrepSummary::of(rows, chunk * PREP_CHUNK))
            .collect();
        let mut summary = PrepSummary::default();
        for part in parts {
            summary.has_dolphin |= part.has_dolphin;
            summary.temptation_consumers += part.temptation_consumers;
            for (total, count) in summary.consumers.iter_mut().zip(part.consumers) {
                *total += count;
            }
            for (cell, indices) in part.cow_cells {
                summary.cow_cells.entry(cell).or_default().extend(indices);
            }
        }
        summary
    } else {
        PrepSummary::of(&out.rows, 0)
    };
}

/// Slot ranges larger than this admission count prepare rows in parallel.
pub(super) const PARALLEL_PREP_MIN: usize = 4096;
pub(super) const PREP_CHUNK: usize = 2048;

/// Read-only components row preparation needs. `TickState` owns channel
/// endpoints and is not `Sync`, so workers borrow only these.
pub(super) struct PrepView<'a> {
    pub(super) entities: &'a EntityStore,
    pub(super) bodies: &'a [Body],
    pub(super) positions: &'a [[f64; 3]],
    pub(super) velocities: &'a [[f64; 3]],
    pub(super) rotations: &'a [[f32; 3]],
    pub(super) ground: &'a [bool],
    pub(super) impulses: &'a [[f64; 2]],
    pub(super) registry: &'a crate::registry::CoreRegistry,
    pub(super) world: &'a crate::world::ChunkWorld,
    pub(super) hostile: &'a crate::hostile::HostileState,
    pub(super) tick: u64,
    pub(super) has_held_items: bool,
}

impl<'a> PrepView<'a> {
    pub(super) fn of(state: &'a TickState) -> Self {
        Self {
            entities: &state.entities,
            bodies: state.entities.bodies(),
            positions: state.entities.positions(),
            velocities: state.entities.velocities(),
            rotations: state.entities.rotations(),
            ground: state.entities.grounds(),
            impulses: state.entities.impulses(),
            registry: &state.registry,
            world: &state.world,
            hostile: &state.hostile,
            tick: state.tick,
            has_held_items: state.players.values().any(|player| {
                !player.spectator()
                    && [Hand::Main, Hand::Off].into_iter().any(|hand| {
                        !player.inventory.slots[held_slot_index(player, hand)].is_empty()
                    })
            }),
        }
    }
}

/// Helper results that need the whole `TickState` (creaking observers,
/// enderman anger, plugin routes, piglin gold). Only rows passing the cheap
/// gates in `read_row` ever carry them; workers leave the default and the
/// serial fix-up pass rebuilds those rows.
#[derive(Default)]
pub(super) struct RarePrep {
    pub(super) observed: bool,
    pub(super) enderman_target: Option<EntityId>,
    pub(super) plugin_target: Option<[f64; 3]>,
    pub(super) gold_target: Option<[f64; 3]>,
}

impl RarePrep {
    pub(super) fn of(state: &TickState, slot: EntitySlot) -> Self {
        Self {
            observed: crate::creaking::observed(state, slot),
            enderman_target: crate::enderman::target(state, slot),
            plugin_target: crate::plugin_npcs::motion_target(state, slot),
            gold_target: crate::piglin::gold_target(state, slot).map(|(_, pos)| pos),
        }
    }
}

/// Prepared rows, serial special pass entries and resting slots of a slot
/// range, plus indexes (into `rows`) of rows that need the rare helpers.
#[derive(Default)]
pub(super) struct PrepChunk {
    pub(super) rows: Vec<ReadRow>,
    pub(super) specials: Vec<(EntityId, crate::hostile::Special)>,
    pub(super) resting: Vec<EntitySlot>,
    pub(super) rare: Vec<usize>,
    pub(super) summary: PrepSummary,
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct PrepSummary {
    pub(super) has_dolphin: bool,
    pub(super) consumers: [usize; 5],
    pub(super) temptation_consumers: usize,
    pub(super) cow_cells: HashMap<(i32, i32), Vec<usize>>,
}

impl PrepSummary {
    pub(super) fn of(rows: &[ReadRow], base: usize) -> Self {
        let mut summary = Self::default();
        for (offset, row) in rows.iter().enumerate() {
            summary.temptation_consumers += usize::from(row.temptable && row.alive && row.think);
            summary.has_dolphin |= row.movement == crate::blueprint::MovementClass::Swimmer
                && crate::dolphin::is_dolphin(row.type_id);
            if row.chaser && row.alive {
                if row.is_iron_golem {
                    summary.consumers[3] += 1;
                } else if crate::piglin::is_piglin_type(row.type_id) {
                    summary.consumers[1] += 1;
                } else {
                    summary.consumers[0] += 1;
                }
                if row.hunts_villagers {
                    summary.consumers[2] += 1;
                }
                if row.hunts_living {
                    summary.consumers[4] += 1;
                }
            }
            if row.breed_ready && row.alive {
                let cell = (
                    mc_physics::mth::floor_i32(row.pos[0]) / 4,
                    mc_physics::mth::floor_i32(row.pos[2]) / 4,
                );
                summary
                    .cow_cells
                    .entry(cell)
                    .or_default()
                    .push(base + offset);
            }
        }
        summary
    }
}

pub(super) struct PrepEnv {
    pub(super) targets_empty: bool,
    pub(super) villagers_empty: bool,
    pub(super) unchanged_since: u64,
}

/// Admit one slot: record its serial special entry and either rest it
/// (`None`) or return the slot and its motion flags for a worker row.
/// Reads the immutable store only; the caller owns ordering.
pub(super) fn admit_slot(
    view: &PrepView,
    index: usize,
    period: u8,
    env: &PrepEnv,
    specials: &mut Vec<(EntityId, crate::hostile::Special)>,
    resting: &mut Vec<EntitySlot>,
) -> Option<(EntitySlot, u16)> {
    let PrepEnv {
        targets_empty,
        villagers_empty,
        unchanged_since,
    } = *env;
    let slot = EntitySlot(index as u32);
    let flags = view.entities.motion_flags(slot);
    let mut special_idle = true;
    if flags & MOTION_SPECIAL != 0 {
        let hint = view.entities.motion_hint(slot);
        if let Some(special) = hint.special {
            let id = view.bodies[slot.0 as usize].id;
            special_idle = targets_empty
                && !view.hostile.targets.contains_key(&id)
                && view
                    .hostile
                    .idle_without_targets(id, special, hint.swell_metadata_neutral);
            if !special_idle {
                specials.push((id, special));
            }
        }
    }
    if early_entity_rest(
        view,
        slot,
        period,
        unchanged_since,
        special_idle,
        targets_empty
            && (villagers_empty
                || !matches!(
                    view.bodies[index].kind,
                    EntityKind::Mob { type_id }
                        if view.registry.blueprint(type_id).is_some_and(|bp| bp.capabilities.hunts_villagers)
                )),
    ) {
        resting.push(slot);
        return None;
    }
    Some((slot, flags))
}

/// Serial admission: admit one slot and snapshot its worker row into `out`.
pub(super) fn prepare_slot(
    view: &PrepView,
    index: usize,
    period: u8,
    env: &PrepEnv,
    out: &mut PrepChunk,
) {
    let Some((slot, flags)) = admit_slot(
        view,
        index,
        period,
        env,
        &mut out.specials,
        &mut out.resting,
    ) else {
        return;
    };
    let (row, rare) = read_row(view, slot, flags, period, &RarePrep::default());
    if rare {
        out.rare.push(out.rows.len());
    }
    out.rows.push(row);
}

/// The worker snapshot of one admitted slot, and whether the row needs the
/// `TickState`-only helpers in `RarePrep` (every helper below returns its
/// default when that is false).
pub(super) fn read_row(
    view: &PrepView,
    slot: EntitySlot,
    flags: u16,
    period: u8,
    rare: &RarePrep,
) -> (ReadRow, bool) {
    let think = motion_thinks(view.tick, flags, period);
    let body = &view.bodies[slot.0 as usize];
    let row = view.entities.row(slot);
    let observed = rare.observed;
    // Snapshot predicates share one immutable blueprint lookup per mob.
    let blueprint = match body.kind {
        EntityKind::Mob { type_id } => view.registry.blueprint(type_id),
        _ => None,
    };
    let needs_rare = crate::enderman::needs_state(row)
        || crate::plugin_npcs::needs_state(row)
        || crate::creaking::needs_state(body)
        || crate::piglin::eligible(body, row);
    let (chaser, breeder, item_entity, player, special, dark_aggro) = match body.kind {
        EntityKind::Mob { .. } => {
            let special = blueprint.and_then(crate::hostile::special_of);
            (
                blueprint.is_some_and(|blueprint| {
                    blueprint.has_goal(crate::blueprint::GoalKind::MeleeChase)
                }) || matches!(
                    special,
                    Some(crate::hostile::Special::Ranged | crate::hostile::Special::Swell)
                ),
                blueprint
                    .is_some_and(|blueprint| blueprint.has_goal(crate::blueprint::GoalKind::Breed)),
                false,
                false,
                special,
                blueprint.is_some_and(|blueprint| blueprint.capabilities.dark_aggro_only),
            )
        }
        EntityKind::Item { .. } => (false, false, true, false, None, false),
        EntityKind::ExperienceOrb { .. } => (false, false, false, false, None, false),
        EntityKind::Player { .. } => (false, false, false, true, None, false),
    };
    // Kind classification and scalar combat/defense inputs are snapshotted
    // here so `step_entity` never touches the cold row for the moved fields.
    let is_enderman = crate::enderman::is_enderman(body);
    let type_id = match body.kind {
        EntityKind::Mob { type_id }
        | EntityKind::Item { type_id }
        | EntityKind::ExperienceOrb { type_id } => type_id,
        EntityKind::Player { .. } => u32::MAX,
    };
    let is_iron_golem = blueprint.is_some_and(|bp| bp.capabilities.hunts_monsters);
    let is_slime_size_one = row.loot.slime_size == Some(1)
        && blueprint
            .is_some_and(|bp| bp.capabilities.size_profile == crate::blueprint::SizeProfile::Slime);
    let has_effects = flags & MOTION_EFFECTS != 0;
    let chaser = chaser
        && !row.controls.no_ai
        && (!matches!(body.kind, EntityKind::Mob { type_id } if crate::piglin::is_piglin_type(type_id))
            || crate::entity_life::adult(row))
        && (!is_enderman || rare.enderman_target.is_some());
    let breeder = breeder && !row.controls.no_ai;
    let special = special.filter(|_| !row.controls.no_ai);
    let panic_multiplier = if (body.burn_ticks > 0
        || body
            .hurt_at
            .is_some_and(|at| view.tick.saturating_sub(at) < 100))
        && blueprint
            .is_some_and(|blueprint| blueprint.has_goal(crate::blueprint::GoalKind::FleeFire))
    {
        if body.burn_ticks > 0 { 2.0 } else { 1.25 }
    } else {
        0.0
    };
    let impulse = view.impulses[slot.0 as usize];
    let (speed, melee_bonus, levitation, slow_falling) = if has_effects {
        let effects = &row.active_effects;
        (
            row.speed
                * ((1.0 + 0.2 * effects.level(crate::effects::EffectId::Speed))
                    * (1.0 - 0.15 * effects.level(crate::effects::EffectId::Slowness)).max(0.0)),
            effects.melee_bonus(),
            effects.level(crate::effects::EffectId::Levitation),
            effects.has(crate::effects::EffectId::SlowFalling),
        )
    } else {
        (row.speed, 0.0, 0.0, false)
    };
    // Panic source stays a rare-branch worker read (see `step_entity`).
    let read = ReadRow {
        think,
        has_retained_target: view.hostile.targets.contains_key(&body.id),
        retain_target: blueprint.is_some_and(crate::hostile::retains_target),
        enderman_target: rare.enderman_target,
        plugin_target: rare.plugin_target,
        gold_target: rare.gold_target,
        panic_multiplier,
        slot,
        id: body.id,
        pos: view.positions[slot.0 as usize],
        vel: view.velocities[slot.0 as usize],
        rotation: view.rotations[slot.0 as usize],
        movement: body.movement,
        air_supply: body.air_supply,
        horizontal_impulse: if observed { [0.0; 2] } else { impulse },
        alive: body.alive,
        ground: view.ground[slot.0 as usize],
        half_width: body.half_width,
        height: body.height,
        sight_range: row.sight_range,
        attack_reach: row.attack_reach,
        attack_damage: row.attack_damage,
        melee_bonus,
        attack_cooldown: body.attack_cooldown,
        love: row.love,
        is_enderman,
        is_iron_golem,
        hunts_villagers: blueprint.is_some_and(|bp| bp.capabilities.hunts_villagers),
        hunts_living: blueprint.is_some_and(|bp| bp.capabilities.hunts_living),
        is_slime_size_one,
        climbs_walls: blueprint.is_some_and(|blueprint| blueprint.capabilities.climbs_walls),
        aquatic_control: blueprint.map_or(crate::species::AquaticControl::Other, |blueprint| {
            blueprint.capabilities.aquatic
        }),
        chaser,
        breed_ready: breeder
            && crate::entity_life::can_breed_in(view.registry, body, row)
            && row.entity_life.child_ticks == 0
            && row.breed_cooldown == 0
            && row.love > 0,
        temptable: blueprint.is_some_and(|blueprint| blueprint.capabilities.temptable),
        type_id,
        pickup_ready: item_entity && body.alive && body.pickup_delay == 0,
        item_entity,
        player,
        speed,
        levitation,
        slow_falling,
        special,
        dark_aggro,
        standing: (special == Some(crate::hostile::Special::Swell)
            && view.hostile.fuse(body.id) > 0)
            || matches!(row.special_mob.as_deref(), Some(crate::special_mobs::Retained::Wither { invulnerable }) if *invulnerable > 0)
            || (matches!(
                special,
                Some(crate::hostile::Special::Native(
                    crate::blueprint::GoalKind::Beam | crate::blueprint::GoalKind::ElderBeam
                ))
            ) && view
                .hostile
                .native
                .attacks
                .get(&body.id)
                .is_some_and(|attack| attack.stage > 0))
            || row.controls.no_ai
            || observed
            || blueprint.is_some_and(|blueprint| blueprint.goals.is_empty())
            || crate::taming::suppress_ai(row)
            || crate::warden::suppress_ai(row)
            || crate::piglin::admiring(row),
        leash_follow: row.entity_life.leash.is_some()
            && !row.controls.no_ai
            && !row.entity_life.sitting
            && row.entity_life.mount.rider.is_none(),
    };
    (read, needs_rare)
}

/// Entity phase: every simulated entity reads the snapshot and emits its own
/// next state plus intents. Parallel by data over scoped threads; workers
/// return owned writes applied in slot order afterwards. No locks: the only
/// synchronization is the scope join.
pub(super) fn run_entity_phase(state: &mut TickState, sim: bool) -> Vec<Intent> {
    #[cfg(all(test, debug_assertions))]
    sensor_bench_tests::COUNTS.set((0, 0));
    if sim {
        crate::contact_hazards::hurt_by_water(state);
        crate::enderman::tick(state);
    }
    let prep_start = std::time::Instant::now();
    state.entities.prepare_motion_hints(&state.registry);
    let unchanged_since = state.entity_phase_revision;
    state.entity_phase_revision = state.world.revision_now();
    let EntityPhaseScratch {
        mut rows,
        writes: mut ordered,
        mut specials,
        mut players,
        mut activation,
        mut periods,
        mut resting,
    } = std::mem::take(&mut state.entity_phase_scratch);
    periods.resize(state.entities.len(), 0);
    let mut admitted = 0;
    // Compact flags preserve synthetic player bodies without PlayerState,
    // including global motion admission for players outside active chunks.
    for slot in state.entities.motion_slots(MOTION_OWNED | MOTION_PLAYER) {
        periods[slot.0 as usize] = 1;
        admitted += 1;
        if state.entities.motion_flags(slot) & MOTION_ALIVE != 0 {
            players.push((state.entities.body(slot).id, state.entities.pos(slot)));
        }
    }
    // Mob decisions run every 1/2/4 ticks at near/mid/far chunk distances.
    for (_, pos) in &players {
        let cx = mc_physics::mth::floor_i32(pos[0]).div_euclid(16);
        let cz = mc_physics::mth::floor_i32(pos[2]).div_euclid(16);
        for dz in -MOB_MID_CHUNKS..=MOB_MID_CHUNKS {
            for dx in -MOB_MID_CHUNKS..=MOB_MID_CHUNKS {
                let period = if dx.abs().max(dz.abs()) <= MOB_NEAR_CHUNKS {
                    1
                } else {
                    2
                };
                activation
                    .entry((cx + dx, cz + dz))
                    .and_modify(|best| *best = (*best).min(period))
                    .or_insert(period);
            }
        }
    }
    // Exact, mostly active membership lets hot flags/positions replace per-ID
    // index traversal even when a few occupied chunks are inactive.
    if !sim {
        // Frozen (`/tick freeze`): only player bodies step; every other
        // entity keeps its position, velocity and timers.
    } else if prefer_dense_entity_admission(state) {
        let mut previous: Option<(ChunkPos, u8)> = None;
        for slot in state.entities.motion_slots(MOTION_OWNED) {
            if state.entities.motion_flags(slot) & MOTION_PLAYER != 0 {
                continue;
            }
            let chunk = state.entities.chunk_of(slot);
            let period = match previous {
                Some((last, period)) if last == chunk => period,
                _ => {
                    let period = if state.world.is_active_position(chunk.x, chunk.z) {
                        activation
                            .get(&(chunk.x, chunk.z))
                            .copied()
                            .unwrap_or(MOB_FAR_PERIOD) as u8
                    } else {
                        0
                    };
                    previous = Some((chunk, period));
                    period
                }
            };
            if period != 0 {
                periods[slot.0 as usize] = period;
                admitted += 1;
            }
        }
    } else {
        // Preserve the indexed union and period order for old type-transition
        // entries rather than silently replacing them with current positions.
        for chunk in state.world.active_positions() {
            if !state.world.is_active(chunk.x, chunk.z) {
                continue;
            }
            let period = activation
                .get(&(chunk.x, chunk.z))
                .copied()
                .unwrap_or(MOB_FAR_PERIOD) as u8;
            state.entities.for_each_slot_in_chunk(chunk, |slot| {
                let flags = state.entities.motion_flags(slot);
                if flags & MOTION_OWNED != 0 && flags & MOTION_PLAYER == 0 {
                    let mark = &mut periods[slot.0 as usize];
                    admitted += usize::from(*mark == 0);
                    *mark = period;
                }
            });
        }
    }
    crate::hostile::refresh_targets(state);
    let targets_empty = !crate::hostile::has_live_targets(state);
    // Reuse the sensor's current resident table before admission. Any
    // candidate conservatively wakes decision ticks, even out of range or
    // occluded: normal sensor cadence catches same-chunk range crossings,
    // spawn, teleport and dimension travel without a cached wake dependency.
    let villagers = if admitted == 0 {
        Vec::new()
    } else {
        resident_villager_targets(&state.entities)
    };
    rows.reserve(admitted);
    let mut prepared = PrepChunk {
        rows: std::mem::take(&mut rows),
        specials: std::mem::take(&mut specials),
        resting: std::mem::take(&mut resting),
        rare: Vec::new(),
        summary: PrepSummary::default(),
    };
    prepare_rows(
        state,
        &periods,
        &PrepEnv {
            targets_empty,
            villagers_empty: villagers.is_empty(),
            unchanged_since,
        },
        admitted >= PARALLEL_PREP_MIN,
        &mut prepared,
    );
    let PrepSummary {
        has_dolphin,
        consumers,
        temptation_consumers,
        cow_cells,
    } = std::mem::take(&mut prepared.summary);
    PrepChunk {
        rows,
        specials,
        resting,
        ..
    } = prepared;
    if rows.is_empty() && resting.is_empty() {
        periods.clear();
        activation.clear();
        state.entity_phase_scratch = EntityPhaseScratch {
            rows,
            writes: ordered,
            specials,
            players,
            activation,
            periods,
            resting,
        };
        return Vec::new();
    }
    let golem_targets = if consumers[3] != 0 {
        crate::villager::defense::targets(state)
    } else {
        Vec::new()
    };
    let world = &state.world;
    let registry = &state.registry;
    let chunk_loaded = |x: i32, z: i32| world.has_chunk(x.div_euclid(16), z.div_euclid(16));
    let tick = state.tick;
    let chunk_revision = |cx: i32, cz: i32| world.chunk_revision(cx, cz);
    let rows_ref = &rows;
    let dolphin_toys: Vec<[f64; 3]> = if has_dolphin {
        rows.iter()
            .filter(|row| row.item_entity && row.alive && row.pickup_ready)
            .map(|row| row.pos)
            .collect()
    } else {
        Vec::new()
    };
    // Chase targets: vanilla monsters ignore creative and spectator players.
    let targets: Vec<(EntityId, [f64; 3])> = players
        .iter()
        .filter(|(id, _)| {
            state.players.get(id).is_some_and(|player| {
                !player.creative()
                    && !player.spectator()
                    && !state.session_locked(player.connection)
            })
        })
        .copied()
        .collect();
    let piglin_targets: Vec<_> = if crate::piglin::piglin_present(state) {
        targets
            .iter()
            .filter(|(id, _)| !crate::piglin::wears_gold(state, *id))
            .copied()
            .collect()
    } else {
        Vec::new()
    };
    let zoglin_targets: Vec<_> = if consumers[4] != 0 {
        state
            .entities
            .slots()
            .filter_map(|slot| {
                let body = state.entities.body(slot);
                let EntityKind::Mob { type_id } = body.kind else {
                    return None;
                };
                (body.alive
                    && state.entities.row(slot).health > 0.0
                    && mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(
                        crate::blueprint::wire_type_id(type_id),
                    )
                    .is_some_and(|contract| {
                        contract.behavior.archetype
                            == mc_data::entity_types::EntityArchetype::Living
                    })
                    && !state
                        .registry
                        .blueprint(type_id)
                        .is_some_and(|bp| bp.capabilities.ignored_by_living_hunters))
                .then(|| (body.id, state.entities.pos(slot)))
            })
            .collect()
    } else {
        Vec::new()
    };
    // Chase searches per class this tick (over-counted: not every chaser
    // thinks). Only a class with many consumers pays for an index; the rest
    // scan linearly as before (see target_grid.rs for boundary and fence).
    let grid = crate::target_grid::TargetGrid::for_consumers;
    let mut player_grid = grid(&targets, consumers[0]);
    let mut piglin_grid = grid(&piglin_targets, consumers[1]);
    let mut villager_grid = grid(&villagers, consumers[2]);
    let mut golem_grid = grid(&golem_targets, consumers[3]);
    let mut zoglin_grid = grid(&zoglin_targets, consumers[4]);
    let searches = rows.iter().filter(|row| {
        row.alive
            && row.chaser
            && row.think
            && !row.breed_ready
            && !row.standing
            && row.plugin_target.is_none()
            && row.panic_multiplier == 0.0
            && !row.has_retained_target
    });
    let query = |row: &ReadRow| {
        (
            row.pos,
            if row.is_enderman {
                64.0
            } else {
                f64::from(row.sight_range)
            },
        )
    };
    player_grid.batch(
        searches
            .clone()
            .filter(|row| !row.is_iron_golem && !crate::piglin::is_piglin_type(row.type_id))
            .map(query),
    );
    piglin_grid.batch(
        searches
            .clone()
            .filter(|row| crate::piglin::is_piglin_type(row.type_id))
            .map(query),
    );
    villager_grid.batch(
        searches
            .clone()
            .filter(|row| row.hunts_villagers)
            .map(query),
    );
    golem_grid.batch(searches.clone().filter(|row| row.is_iron_golem).map(query));
    zoglin_grid.batch(searches.filter(|row| row.hunts_living).map(query));
    let cows_ref = &cow_cells;
    let scope_start = std::time::Instant::now();
    // Work-stealing over the core's long-lived rayon workers (no threads
    // spawned per tick); `collect` keeps slot order.
    let ctx = StepCtx {
        rows: rows_ref,
        entities: &state.entities,
        retained_targets: &state.hostile.targets,
        players: player_grid,
        passive: passive_follow::Context {
            players: grid(&players, temptation_consumers),
            states: &state.players,
            locked: state.require_plugin_login.then_some(&state.locked_sessions),
        },
        dolphin_toys: &dolphin_toys,
        villagers: villager_grid,
        golem_targets: golem_grid,
        piglin_targets: piglin_grid,
        zoglin_targets: zoglin_grid,
        cow_cells: cows_ref,
        world,
        chunk_loaded: &chunk_loaded,
        registry,
        tick,
        chunk_revision: &chunk_revision,
        unchanged_since,
        difficulty: state.difficulty,
        sky_darken: if state.dimension.has_weather() {
            crate::light::sky_darken(state.day_time, state.rain_level, state.thunder_level)
        } else {
            15
        },
    };
    system_graph::run(&ctx, &mut ordered);
    let scope_time = scope_start.elapsed();
    #[cfg(all(test, debug_assertions))]
    {
        let grids = [
            &ctx.players,
            &ctx.piglin_targets,
            &ctx.villagers,
            &ctx.golem_targets,
            &ctx.zoglin_targets,
        ];
        sensor_bench_tests::COUNTS.set((
            grids.iter().map(|grid| grid.query_count()).sum(),
            grids.iter().map(|grid| grid.batch_count()).sum(),
        ));
    }
    let prep_time = scope_start.duration_since(prep_start);
    let commit_start = std::time::Instant::now();
    let resting_count = resting.len();
    let mut seq = std::mem::take(&mut state.seq_counter);
    let mut intents = Vec::new();
    let physical_profiles = state.registry.item_facts.has_physical_profiles();
    for slot in resting.drain(..) {
        // A quiet row's writeback is a no-op; the hot flag spares the cold
        // row (a cache miss per resting body).
        if state.entities.motion_flags(slot) & crate::entities::MOTION_QUIET_TIMERS != 0 {
            continue;
        }
        let body = state.entities.body(slot);
        state.entities.set_motion_state(
            slot,
            [0.0; 2],
            body.air_supply,
            body.pickup_delay.saturating_sub(1),
            body.attack_cooldown.saturating_sub(1),
        );
    }
    for (write, mut raw) in ordered.drain(..).flatten() {
        if let Some(target) = write.chase_target {
            state
                .hostile
                .targets
                .insert(state.entities.body(write.slot).id, target);
        }
        let attack_cooldown = raw
            .iter()
            .any(|intent| {
                matches!(
                    intent.kind,
                    IntentKind::DamageEntity {
                        kind: crate::survival::DamageKind::Melee,
                        ..
                    }
                )
            })
            .then_some(MELEE_ATTACK_INTERVAL);
        let was_ground = state.entities.ground(write.slot);
        let old = state.entities.apply_motion_result(
            write.slot,
            &crate::entities::MotionResult {
                pos: write.pos,
                vel: write.vel,
                yaw: write.rotation[0],
                pitch: write.rotation[1],
                ground: write.ground,
                horizontal_impulse: write.horizontal_impulse,
                air_supply: write.air_supply,
                attack_cooldown,
            },
        );
        if matches!(
            state.entities.body(write.slot).kind,
            EntityKind::Item { .. }
        ) {
            crate::redstone::containers::item_moved(&mut state.redstone_containers, write.pos);
        }
        let fall = crate::mob_fall::after_move(
            state,
            write.slot,
            old,
            write.pos,
            was_ground,
            write.ground,
        );
        if fall > 0.0 {
            let actor = state.entities.body(write.slot).id;
            raw.push(RawIntent {
                actor,
                kind: IntentKind::DamageEntity {
                    target: actor,
                    amount: fall,
                    kind: crate::survival::DamageKind::Fall,
                    source: None,
                },
                footprint: Footprint {
                    entity_writes: vec![actor],
                    ..Default::default()
                },
            });
        }
        // Only a player or a body wearing armor can spend movement stamina;
        // the hot flags answer that without dereferencing the row's
        // appearance (a cache miss per unarmored mob).
        if physical_profiles
            && state.entities.motion_flags(write.slot)
                & (crate::entities::MOTION_PLAYER | crate::entities::MOTION_ARMORED)
                != 0
        {
            crate::physical_defense::accepted_movement(
                state,
                write.slot,
                mth::length2(write.pos[0] - old[0], write.pos[2] - old[2]),
            );
        }
        for raw_intent in raw {
            let next = seq.get(&raw_intent.actor).copied().unwrap_or(0);
            seq.insert(raw_intent.actor, next + 1);
            intents.push(Intent {
                actor: raw_intent.actor,
                seq: next,
                kind: raw_intent.kind,
                footprint: raw_intent.footprint,
            });
        }
    }
    state.seq_counter = seq;
    crate::item_motion::merge_neighbours(state);
    let commit_time = commit_start.elapsed();
    let post_start = std::time::Instant::now();
    // Pickup intents: players collect mature item entities in reach.
    // Exact kind lanes, authoritative maturity and post-move positions.
    if sim {
        collect_pickup_intents(state, &mut intents);
    }
    // Shooting skeletons, flying arrows, swelling creepers (serial: they
    // read the world and every entity).
    if sim {
        crate::hostile::run(state, &mut intents, &specials);
        crate::projectiles::run(state, &mut intents, &specials);
    }
    if std::env::var("MC_CORE_PHASE_LOG").is_ok() {
        eprintln!(
            "  entity detail ms: scope={:.1} n={} prep={:.1} commit={:.1} post={:.1} rest={}",
            scope_time.as_secs_f64() * 1000.0,
            rows.len(),
            prep_time.as_secs_f64() * 1000.0,
            commit_time.as_secs_f64() * 1000.0,
            post_start.elapsed().as_secs_f64() * 1000.0,
            resting_count,
        );
    }
    rows.clear();
    specials.clear();
    players.clear();
    activation.clear();
    periods.clear();
    state.entity_phase_scratch = EntityPhaseScratch {
        rows,
        writes: ordered,
        specials,
        players,
        activation,
        periods,
        resting,
    };
    let _ = tick;
    intents
}

/// Ticks between a mob's melee hits (vanilla `MeleeAttackGoal`).
pub(super) const MELEE_ATTACK_INTERVAL: u32 = 20;

/// Intent without a sequence number (assigned at drain time).
pub(super) struct RawIntent {
    pub(super) actor: EntityId,
    pub(super) kind: IntentKind,
    pub(super) footprint: Footprint,
}

/// Step one entity: returns its movement write plus raw intents.
/// Shared read-only context for entity stepping (no locks, no copies).
pub(super) struct StepCtx<'a> {
    pub(super) rows: &'a [ReadRow],
    /// Immutable through the indexed worker join; writes/compaction follow it.
    pub(super) entities: &'a crate::entities::EntityStore,
    pub(super) retained_targets:
        &'a rustc_hash::FxHashMap<EntityId, (crate::target_grid::Target, f64)>,
    pub(super) players: crate::target_grid::TargetGrid<'a>,
    pub(super) passive: passive_follow::Context<'a>,
    pub(super) dolphin_toys: &'a [[f64; 3]],
    pub(super) villagers: crate::target_grid::TargetGrid<'a>,
    pub(super) golem_targets: crate::target_grid::TargetGrid<'a>,
    pub(super) piglin_targets: crate::target_grid::TargetGrid<'a>,
    pub(super) zoglin_targets: crate::target_grid::TargetGrid<'a>,
    pub(super) cow_cells: &'a HashMap<(i32, i32), Vec<usize>>,
    pub(super) world: &'a ChunkWorld,
    pub(super) chunk_loaded: &'a (dyn Fn(i32, i32) -> bool + Sync),
    pub(super) registry: &'a CoreRegistry,
    pub(super) tick: u64,
    /// Content revision of a resident chunk (`u64::MAX` when absent).
    pub(super) chunk_revision: &'a (dyn Fn(i32, i32) -> u64 + Sync),
    /// World revision the previous entity phase read.
    pub(super) unchanged_since: u64,
    pub(super) difficulty: crate::survival::Difficulty,
    pub(super) sky_darken: u8,
}

pub(super) fn step_entity(row: ReadRow, ctx: &StepCtx) -> (MoveWrite, Vec<RawIntent>) {
    let rows = ctx.rows;
    let players = ctx.players.table();
    let cow_cells = ctx.cow_cells;
    let reader = ctx.world.physics_reader();
    let block_at = &|x, y, z| reader.block_at(x, y, z);
    let chunk_loaded = ctx.chunk_loaded;
    let registry = ctx.registry;
    let tick = ctx.tick;
    let mut intents = Vec::new();
    let mut chase_target = None;
    if row.item_entity {
        let sampler = WorkerSampler { block_at, registry };
        let body = crate::item_motion::step(
            mc_physics::EntityBody {
                position: mc_physics::Vec3::new(row.pos[0], row.pos[1], row.pos[2]),
                velocity: mc_physics::Vec3::new(row.vel[0], row.vel[1], row.vel[2]),
                aabb: mc_physics::Aabb {
                    half_width: row.half_width,
                    height: row.height,
                },
                on_ground: row.ground,
            },
            &sampler,
        );
        return (
            MoveWrite {
                slot: row.slot,
                chase_target: None,
                pos: [body.position.x, body.position.y, body.position.z],
                vel: [body.velocity.x, body.velocity.y, body.velocity.z],
                ground: body.on_ground,
                horizontal_impulse: [0.0; 2],
                rotation: row.rotation,
                air_supply: row.air_supply,
            },
            intents,
        );
    }
    if row.special == Some(crate::hostile::Special::Projectile) {
        // Arrows fly in `hostile::run`: they read the world and every entity.
        return (
            MoveWrite {
                slot: row.slot,
                chase_target: None,
                pos: row.pos,
                vel: row.vel,
                ground: row.ground,
                horizontal_impulse: [0.0; 2],
                rotation: row.rotation,
                air_supply: row.air_supply,
            },
            intents,
        );
    }
    if row.player {
        return (
            MoveWrite {
                chase_target: None,
                slot: row.slot,
                pos: row.pos,
                vel: [0.0; 3],
                ground: row.ground,
                horizontal_impulse: [0.0; 2],
                rotation: row.rotation,
                air_supply: row.air_supply,
            },
            intents,
        );
    }
    // Mobs decide on their activation period, phase-shifted by id so each
    // tick carries an even share. Between decisions the last one keeps
    // executing (its horizontal velocity); physics and collision run every
    // tick either way.
    let think = row.think;
    let mut look_target = None;
    let mut vel = [0.0; 3];
    if let Some(target) = row.plugin_target {
        vel = crate::entity_motion::steer(row.movement, row.pos, target, f64::from(row.speed));
        look_target = Some(target);
    } else if row.standing {
        // Riding/sitting suppress decisions, never gravity or collision.
        if row.leash_follow {
            vel = row.vel;
        }
    } else if row.panic_multiplier > 0.0 {
        // Rare branch only: the one `last_player_hit` cold read on the
        // worker path.
        let panic_source = ctx
            .entities
            .row(row.slot)
            .last_player_hit
            .filter(|(_, at)| tick.saturating_sub(*at) < 100)
            .and_then(|(attacker, _)| ctx.entities.slot_of(attacker))
            .map(|attacker| ctx.entities.pos(attacker));
        vel = crate::entity_motion::panic(
            row.movement,
            row.id.0,
            tick,
            f64::from(row.speed) * row.panic_multiplier,
            row.pos,
            panic_source,
        );
    } else if !think {
        // A mob standing on the ground with no standing decision has nothing
        // to integrate while every chunk under its footprint is unchanged
        // since the last entity phase read it (a block edit, load or
        // eviction there wakes it this tick).
        if row.movement == crate::blueprint::MovementClass::Walker
            && row.ground
            && row.vel == [0.0; 3]
            && row.horizontal_impulse == [0.0; 2]
            && unchanged_entity_footprint(
                row.pos,
                row.half_width,
                ctx.chunk_revision,
                ctx.unchanged_since,
            )
        {
            return (
                MoveWrite {
                    chase_target: None,
                    slot: row.slot,
                    pos: row.pos,
                    vel: [0.0; 3],
                    ground: true,
                    horizontal_impulse: [0.0; 2],
                    rotation: row.rotation,
                    air_supply: row.air_supply,
                },
                intents,
            );
        }
        vel = [
            row.vel[0] - row.horizontal_impulse[0],
            row.vel[1],
            row.vel[2] - row.horizontal_impulse[1],
        ];
    } else if let Some(target) = row.gold_target {
        vel = crate::entity_motion::steer(row.movement, row.pos, target, f64::from(row.speed));
        look_target = Some(target);
    } else if crate::piglin::is_hoglin_type(row.type_id)
        && let Some(target) = crate::piglin::warped_fungus(row.pos, registry, block_at)
    {
        let away = [
            row.pos[0] * 2.0 - target[0],
            row.pos[1],
            row.pos[2] * 2.0 - target[2],
        ];
        vel = crate::entity_motion::steer(row.movement, row.pos, away, f64::from(row.speed));
    } else if row.chaser && !row.breed_ready {
        // Goals by priority: MeleeAttack, MeleeChase, Wander. Only chasers
        // need the nearest live player (compact table, no full-row scan).
        let mut nearest = if row.has_retained_target {
            ctx.retained_targets.get(&row.id).copied()
        } else {
            None
        };
        let sight = f64::from(if row.is_enderman {
            64.0
        } else {
            row.sight_range
        });
        let hunts_villagers = row.hunts_villagers;
        let golem = row.is_iron_golem;
        let players = if crate::piglin::is_piglin_type(row.type_id) {
            &ctx.piglin_targets
        } else {
            &ctx.players
        };
        // Chain order (players, villagers, golem targets, zoglin targets)
        // and table order break distance ties, as one linear scan did.
        if nearest.is_none() {
            if !golem {
                players.consider(&mut nearest, row.pos, sight, |(id, _)| {
                    row.enderman_target.is_none_or(|target| target == *id)
                });
            }
            if hunts_villagers {
                ctx.villagers
                    .consider(&mut nearest, row.pos, sight, |_| true);
            }
            if golem {
                // Vanilla IronGolem.canAttackType: a player-created golem never
                // targets players, even ones it was hit by. Player creation is
                // rare cold state; read it only on this branch.
                let spared = matches!(
                    ctx.entities.row(row.slot).special_mob.as_deref(),
                    Some(crate::special_mobs::Retained::PlayerIronGolem)
                );
                ctx.golem_targets
                    .consider(&mut nearest, row.pos, sight, |(id, _)| {
                        !(spared
                            && ctx.entities.slot_of(*id).is_some_and(|slot| {
                                matches!(ctx.entities.body(slot).kind, EntityKind::Player { .. })
                            }))
                    });
            }
            if row.hunts_living {
                ctx.zoglin_targets
                    .consider(&mut nearest, row.pos, sight, |(id, _)| *id != row.id);
            }
        }
        let target_admitted = !row.dark_aggro
            || ctx
                .entities
                .row(row.slot)
                .last_player_hit
                .is_some_and(|(_, tick)| ctx.tick.saturating_sub(tick) < 100)
            || ctx
                .world
                .light_at(
                    row.pos[0].floor() as i32,
                    row.pos[1].floor() as i32,
                    row.pos[2].floor() as i32,
                )
                .is_some_and(|(sky, block)| block.max(sky.saturating_sub(ctx.sky_darken)) < 8);
        if row.retain_target && !row.has_retained_target && target_admitted {
            chase_target = nearest;
        }
        let mut acted = false;
        if let Some(((target, target_pos), dist)) = nearest
            && target_admitted
        {
            look_target = Some(target_pos);
            if row.standing {
                // A creeper mid-fuse stands still.
                acted = true;
            } else if dist <= f64::from(row.attack_reach) {
                // In reach the mob stands and swings once per interval; a
                // bow user backs off from a close target instead.
                let ranged = matches!(
                    row.special,
                    Some(crate::hostile::Special::Ranged | crate::hostile::Special::Native(_))
                ) && !matches!(
                    row.special,
                    Some(crate::hostile::Special::Native(
                        crate::blueprint::GoalKind::SonicBoom | crate::blueprint::GoalKind::VexLife
                    ))
                );
                let attack_damage = if row.is_slime_size_one {
                    0.0
                } else {
                    (row.attack_damage + row.melee_bonus).max(0.0)
                };
                if !ranged && attack_damage > 0.0 && row.attack_cooldown == 0 {
                    intents.push(RawIntent {
                        actor: row.id,
                        kind: IntentKind::DamageEntity {
                            target,
                            amount: if ctx.entities.slot_of(target).is_some_and(|slot| {
                                matches!(ctx.entities.body(slot).kind, EntityKind::Player { .. })
                            }) {
                                ctx.difficulty.scale_mob_damage(attack_damage)
                            } else {
                                attack_damage
                            },
                            kind: crate::survival::DamageKind::Melee,
                            source: Some(crate::intent::DamageSource::Melee {
                                attacker: row.id,
                                position: row.pos,
                                yaw: 0.0,
                                bonus_knockback: 0.0,
                                sweep: false,
                                fire_ticks: 0,
                                bane_ticks: 0,
                            }),
                        },
                        footprint: Footprint {
                            entity_reads: vec![row.id],
                            entity_writes: vec![target],
                            ..Default::default()
                        },
                    });
                }
                if ranged && dist < crate::hostile::BOW_BACKOFF {
                    vel = crate::entity_motion::steer(
                        row.movement,
                        row.pos,
                        target_pos,
                        f64::from(row.speed) * crate::hostile::BACKOFF_SPEED,
                    )
                    .map(|value| -value);
                }
                acted = true;
            } else if dist > 0.5 {
                vel = crate::entity_motion::steer(
                    row.movement,
                    row.pos,
                    target_pos,
                    f64::from(row.speed),
                );
                acted = true;
            }
        }
        if !acted {
            vel = passive_follow::idle(row, ctx, &mut look_target);
        }
    } else if row.breed_ready {
        // Goals by priority: Breed, Wander.
        let mut bred = false;
        let cell = (
            mc_physics::mth::floor_i32(row.pos[0]) / 4,
            mc_physics::mth::floor_i32(row.pos[2]) / 4,
        );
        let mut found: Option<&ReadRow> = None;
        'cells: for dx in -2..=2 {
            for dz in -2..=2 {
                let Some(members) = cow_cells.get(&(cell.0 + dx, cell.1 + dz)) else {
                    continue;
                };
                for index in members {
                    let other = &rows[*index];
                    if other.id == row.id
                        || crate::entity_life::breeding::offspring(
                            registry,
                            row.type_id,
                            other.type_id,
                        )
                        .is_none()
                    {
                        continue;
                    }
                    let ox = other.pos[0] - row.pos[0];
                    let oy = other.pos[1] - row.pos[1];
                    let oz = other.pos[2] - row.pos[2];
                    if ox * ox + oy * oy + oz * oz < 64.0 {
                        found = Some(other);
                        break 'cells;
                    }
                }
            }
        }
        if let Some(other) = found {
            let delta = [
                other.pos[0] - row.pos[0],
                other.pos[1] - row.pos[1],
                other.pos[2] - row.pos[2],
            ];
            let distance = delta
                .into_iter()
                .map(|component| component * component)
                .sum::<f64>()
                .sqrt();
            look_target = Some(other.pos);
            vel = [
                delta[0] / distance.max(0.001) * f64::from(row.speed),
                0.0,
                delta[2] / distance.max(0.001) * f64::from(row.speed),
            ];
            bred = true;
            if distance < 3.0
                && row.id < other.id
                && row.love <= crate::entity_life::LOVE_TICKS - 60
                && other.love <= crate::entity_life::LOVE_TICKS - 60
                && let Some(child_type) =
                    crate::entity_life::breeding::offspring(registry, row.type_id, other.type_id)
            {
                let partner = other.id;
                intents.push(RawIntent {
                    actor: row.id,
                    kind: IntentKind::Breed {
                        partner,
                        child_pos: std::array::from_fn(|axis| {
                            (row.pos[axis] + other.pos[axis]) * 0.5
                        }),
                        child_type,
                    },
                    footprint: Footprint {
                        entity_reads: vec![row.id, partner],
                        entity_writes: vec![row.id, partner],
                        ..Default::default()
                    },
                });
            }
        }
        if !bred {
            vel = passive_follow::idle(row, ctx, &mut look_target);
        }
    } else {
        vel = passive_follow::idle(row, ctx, &mut look_target);
    }
    if !row.standing
        && crate::dolphin::is_dolphin(row.type_id)
        && let Some(drive) = crate::dolphin::drive(
            row.pos,
            row.air_supply,
            row.speed,
            players.iter().map(|(_, pos)| *pos),
            ctx.dolphin_toys.iter().copied(),
            block_at,
            registry,
        )
    {
        vel = drive;
    }
    let sampler = WorkerSampler { block_at, registry };
    let motion = crate::entity_motion::step(
        crate::entity_motion::MotionInput {
            body: mc_physics::EntityBody {
                position: mc_physics::Vec3::new(row.pos[0], row.pos[1], row.pos[2]),
                velocity: mc_physics::Vec3::new(row.vel[0], row.vel[1], row.vel[2]),
                aabb: mc_physics::Aabb {
                    half_width: row.half_width,
                    height: row.height,
                },
                on_ground: row.ground,
            },
            movement: row.movement,
            drive: vel,
            impulse: row.horizontal_impulse,
            rotation: row.rotation,
            look_target,
            climbs_walls: row.climbs_walls,
            aquatic_control: row.aquatic_control,
            air_supply: row.air_supply,
            levitation: row.levitation,
            slow_falling: row.slow_falling,
            id: row.id.0,
            tick,
        },
        &sampler,
        chunk_loaded,
    );
    if motion.drowning {
        intents.push(RawIntent {
            actor: row.id,
            kind: IntentKind::DamageEntity {
                target: row.id,
                amount: 2.0,
                kind: crate::survival::DamageKind::Drowning,
                source: None,
            },
            footprint: Footprint {
                entity_writes: vec![row.id],
                ..Default::default()
            },
        });
    }
    (
        MoveWrite {
            chase_target,
            slot: row.slot,
            pos: [
                motion.body.position.x,
                motion.body.position.y,
                motion.body.position.z,
            ],
            vel: [
                motion.body.velocity.x,
                motion.body.velocity.y,
                motion.body.velocity.z,
            ],
            ground: motion.body.on_ground,
            horizontal_impulse: motion.impulse,
            rotation: motion.rotation,
            air_supply: motion.air_supply,
        },
        intents,
    )
}

/// Mob activation rings in chunks around each player (see the entity
/// phase): every tick within [`MOB_NEAR_CHUNKS`], every 2 ticks within
/// [`MOB_MID_CHUNKS`], every [`MOB_FAR_PERIOD`] ticks beyond.
pub(super) const MOB_NEAR_CHUNKS: i32 = 2;
pub(super) const MOB_MID_CHUNKS: i32 = 4;
pub(super) const MOB_FAR_PERIOD: u64 = 4;

/// Item pickup intents: live players collect mature item entities in reach.
pub(super) fn collect_pickup_intents(state: &TickState, intents: &mut Vec<Intent>) {
    collect_pickup_intents_from(
        state.entities.bodies(),
        state.entities.positions(),
        state
            .entities
            .player_slots()
            .filter(|slot| !state.entity_session_locked(state.entities.body(*slot).id)),
        state.entities.item_slots(),
        &state.seq_counter,
        intents,
    );
}

/// Inner pickup matcher over borrowed Body/position slices: exact kind lanes
/// (players, then items), authoritative maturity and post-move positions.
pub(super) fn collect_pickup_intents_from(
    bodies: &[Body],
    positions: &[[f64; 3]],
    player_slots: impl Iterator<Item = EntitySlot>,
    item_slots: impl Iterator<Item = EntitySlot>,
    seq_counter: &HashMap<EntityId, u32>,
    intents: &mut Vec<Intent>,
) {
    let mut players: Vec<(EntityId, [f64; 3])> = Vec::new();
    let mut items: Vec<(EntityId, [f64; 3])> = Vec::new();
    for slot in player_slots {
        let body = &bodies[slot.0 as usize];
        if body.alive {
            players.push((body.id, positions[slot.0 as usize]));
        }
    }
    for slot in item_slots {
        let body = &bodies[slot.0 as usize];
        if body.alive && body.pickup_delay == 0 {
            items.push((body.id, positions[slot.0 as usize]));
        }
    }
    for (player, player_pos) in &players {
        for (item, item_pos) in &items {
            let dx = item_pos[0] - player_pos[0];
            let dy = item_pos[1] - player_pos[1];
            let dz = item_pos[2] - player_pos[2];
            // Vanilla Player.aiStep checks the player's box inflated by
            // (1, .5, 1), intersecting the item's .25-wide/.25-high box.
            if dx.abs() >= 1.425 || dz.abs() >= 1.425 || dy <= -0.75 || dy >= 2.3 {
                continue;
            }
            let next = seq_counter.get(player).copied().unwrap_or(0);
            intents.push(Intent {
                actor: *player,
                seq: next,
                kind: IntentKind::PickupItem {
                    item: *item,
                    count: i32::MAX,
                },
                footprint: Footprint {
                    entity_reads: vec![*player],
                    item_take: Some(*item),
                    ..Default::default()
                },
            });
            break;
        }
    }
}
