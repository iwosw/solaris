//! Native contact damage, ignition and extinguishing through merge admission.
//!
//! Multi-cell scans borrow a single resident column when overlays are empty;
//! seam and overlay scans retain the exact overlay-first world reader.

use crate::boundary::EntityId;
use crate::intent::{Footprint, Intent, IntentKind};
use crate::registry::{
    HAZARD_BERRY, HAZARD_CACTUS, HAZARD_CAMPFIRE, HAZARD_FIRE, HAZARD_LAVA, HAZARD_MAGMA,
    HAZARD_WATER,
};
use crate::survival::DamageKind;
use crate::tick::TickState;
use mc_physics::mth::floor_i32;
use std::sync::{Arc, LazyLock};

/// Reused per-pass section-predicate caches for the contact run.
///
/// `hazard` gates body/foot-support samples on any nonzero hazard flag;
/// `suffocation` gates the eye-cell `block_at` on any suffocating state.
/// Both hold one fixed predicate's verdicts for a single read-only pass and
/// are `reset` once per `run`; `TickState` retains their capacity between runs.
///
/// `negative` memoizes proven raw geometric absence per dense entity slot as
/// `world::ContactStamp<2>` (`probes` are the foot-support center cell and the
/// exact eye cell from the actual height, which can exceed the clamped body
/// envelope). Slots are fenced by `EntityId` against swap-remove compaction;
/// the `Vec` retains capacity across ticks. Only true raw absence is stored:
/// any suffocating eye, any nonzero body hazard flag, or magma support blocks
/// the memo even when sneaking, frost-walker, or stationary-berry rules would
/// suppress the damage intent. A hit bypasses geometric reads only; retained
/// burn timers still decay below.
#[derive(Default)]
pub(crate) struct Scratch {
    pub(crate) hazard: crate::world::SectionMatchCache,
    pub(crate) suffocation: crate::world::SectionMatchCache,
    pub(crate) negative: Vec<Option<crate::world::ContactStamp<2>>>,
}

/// Set the retained timer and publish only the shared burning bit.
pub(crate) fn ignite(state: &mut TickState, entity: EntityId, ticks: u32) {
    let Some(slot) = state.entities.slot_of(entity) else {
        return;
    };
    let (body, row) = state.entities.parts_mut(slot);
    body.burn_ticks = body.burn_ticks.max(ticks);
    if ticks > 0 && row.appearance.shared_flags & 1 == 0 {
        Arc::make_mut(&mut row.appearance).shared_flags |= 1;
        state.appearance_changed.push(entity);
    }
}

fn damage(
    state: &mut TickState,
    entity: EntityId,
    kind: DamageKind,
    amount: f32,
    intents: &mut Vec<Intent>,
) {
    let seq = state.next_seq(entity);
    intents.push(Intent {
        actor: entity,
        seq,
        kind: IntentKind::DamageEntity {
            target: entity,
            amount,
            kind,
            source: None,
        },
        footprint: Footprint {
            entity_reads: vec![entity],
            entity_writes: vec![entity],
            ..Default::default()
        },
    });
}

// The contract table is generated and immutable: resolve living once.
static LIVING_BY_TYPE: LazyLock<[bool; mc_data::entity_types::ENTITY_TYPE_COUNT]> =
    LazyLock::new(|| {
        let mut living = [false; mc_data::entity_types::ENTITY_TYPE_COUNT];
        for (index, slot) in living.iter_mut().enumerate() {
            *slot = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(index as u32)
                .is_some_and(|contract| {
                    contract.behavior.archetype == mc_data::entity_types::EntityArchetype::Living
                });
        }
        living
    });

type ContactCells = ((i32, i32, i32), (i32, i32, i32), [[i32; 3]; 2]);

#[inline]
fn contact_cells(pos: [f64; 3], shape: [f64; 2], foot_xz: [i32; 2]) -> ContactCells {
    let width = shape[0].min(2.0);
    let height = shape[1].min(6.0);
    let foot_y = floor_i32(pos[1]);
    (
        (floor_i32(pos[0] - width), foot_y, floor_i32(pos[2] - width)),
        (
            floor_i32(pos[0] + width),
            floor_i32(pos[1] + height - 1.0e-6),
            floor_i32(pos[2] + width),
        ),
        [
            [foot_xz[0], foot_y - 1, foot_xz[1]],
            [foot_xz[0], floor_i32(pos[1] + shape[1] * 0.9), foot_xz[1]],
        ],
    )
}

/// Geometry absence never suspends this retained clock or its damage cadence.
#[inline]
fn retained_burn(
    state: &mut TickState,
    slot: crate::entities::EntitySlot,
    water: bool,
    burning_contact: bool,
    intents: &mut Vec<Intent>,
) {
    let body = state.entities.body(slot);
    if body.burn_ticks == 0 && state.entities.row(slot).appearance.shared_flags & 1 == 0 {
        return;
    }
    let entity = body.id;
    let (body, row) = state.entities.parts_mut(slot);
    if water {
        body.burn_ticks = 0;
    } else {
        body.burn_ticks = body.burn_ticks.saturating_sub(1);
    }
    let burning = body.burn_ticks > 0;
    if (row.appearance.shared_flags & 1 != 0) != burning {
        let flags = &mut Arc::make_mut(&mut row.appearance).shared_flags;
        *flags = (*flags & !1) | u8::from(burning);
        state.appearance_changed.push(entity);
    }
    if burning && state.tick.is_multiple_of(20) && !burning_contact {
        damage(state, entity, DamageKind::OnFire, 1.0, intents);
    }
}

/// A bounded body scan; missing chunks never manufacture hazard or support.
/// Berry damage requires actual horizontal displacement, not packet cadence.
pub(crate) fn run(state: &mut TickState, intents: &mut Vec<Intent>) {
    run_with(state, intents, MIN_PARALLEL_ROWS);
}

/// Rows per rayon job; sized so a job amortizes its small result vector.
const CLASSIFY_JOB_ROWS: usize = 2048;
/// Below this row count the classifier runs inline: a rayon hand-off costs more.
pub(crate) const MIN_PARALLEL_ROWS: usize = 8192;

/// Run a pure per-row classifier over `rows` (jobs of [`CLASSIFY_JOB_ROWS`] on
/// the rayon pool once `rows.len() >= min_parallel`, otherwise inline) and
/// return ascending `(row index, work)` pairs for nonempty results, grouped
/// per job. `classify` may only mutate its own row and must not depend on any
/// other row, so the result is identical for every thread count.
pub(crate) fn classify_rows<S: Send, R: Send>(
    rows: &mut [S],
    min_parallel: usize,
    classify: impl Fn(usize, &mut S) -> Option<R> + Sync,
) -> Vec<Vec<(u32, R)>> {
    let job = |first: usize, rows: &mut [S]| {
        let mut flagged = Vec::new();
        for (offset, row) in rows.iter_mut().enumerate() {
            if let Some(work) = classify(first + offset, row) {
                flagged.push(((first + offset) as u32, work));
            }
        }
        flagged
    };
    if rows.len() < min_parallel {
        return vec![job(0, rows)];
    }
    use rayon::prelude::*;
    rows.par_chunks_mut(CLASSIFY_JOB_ROWS)
        .enumerate()
        .map(|(chunk, rows)| job(chunk * CLASSIFY_JOB_ROWS, rows))
        .collect()
}

/// Reuse a geometric-absence proof, refreshing raw drift only while the same
/// cells and world revision still prove quiet. Return computed coverage for a
/// stale proof so the serial scan never repeats the cell math.
fn quiet_proof(
    world: &crate::world::ChunkWorld,
    hazard: &crate::world::SectionMatchCache,
    stamp: &mut crate::world::ContactStamp<2>,
    pos: [f64; 3],
    shape: [f64; 2],
    foot_xz: [i32; 2],
) -> (bool, Option<ContactCells>) {
    let raw_same = stamp.position == pos && stamp.shape == shape;
    let cells = if raw_same {
        (stamp.min, stamp.max, stamp.probes)
    } else {
        contact_cells(pos, shape, foot_xz)
    };
    let quiet = (raw_same || cells == (stamp.min, stamp.max, stamp.probes))
        && world.contact_unchanged_since(stamp.min, stamp.max, stamp.revision, hazard);
    if quiet && !raw_same {
        stamp.position = pos;
        stamp.shape = shape;
    }
    (quiet, (!quiet).then_some(cells))
}

/// Work left after the cold-free geometric classifier.
enum ContactWork {
    Scan,
    Quiet,
}

/// Pure body/position classifier. Quiet survivors need only the serial
/// appearance/burn check: appearance has a single owner in the cold column.
fn classify_row(
    bodies: &[crate::entities::Body],
    positions: &[[f64; 3]],
    world: &crate::world::ChunkWorld,
    hazard: &crate::world::SectionMatchCache,
    index: usize,
    stamp: &mut Option<crate::world::ContactStamp<2>>,
) -> Option<ContactWork> {
    let row = &bodies[index];
    let type_id = match row.kind {
        crate::entities::EntityKind::Mob { type_id } => type_id,
        crate::entities::EntityKind::Player { .. } | crate::entities::EntityKind::Item { .. } => {
            return Some(ContactWork::Scan);
        }
        _ => {
            *stamp = None;
            return None;
        }
    };
    let living = LIVING_BY_TYPE
        .get(type_id as usize)
        .copied()
        .unwrap_or(false);
    if !row.alive || !living {
        *stamp = None;
        return None;
    }
    let pos = positions[index];
    let shape = [row.half_width, row.height];
    let previous = stamp.as_ref().filter(|stamp| stamp.entity == row.id);
    let foot_xz = if let Some(stamp) = previous
        && stamp.position[0] == pos[0]
        && stamp.position[2] == pos[2]
    {
        [stamp.probes[0][0], stamp.probes[0][2]]
    } else {
        [floor_i32(pos[0]), floor_i32(pos[2])]
    };
    if !world.is_active(foot_xz[0].div_euclid(16), foot_xz[1].div_euclid(16)) {
        *stamp = None;
        return None;
    }
    let quiet = stamp
        .as_mut()
        .filter(|stamp| stamp.entity == row.id)
        .is_some_and(|stamp| quiet_proof(world, hazard, stamp, pos, shape, foot_xz).0);
    Some(if quiet {
        ContactWork::Quiet
    } else {
        ContactWork::Scan
    })
}

/// Classify every row on the pool (pure reads), then run the exact serial
/// scan, in row order, only for rows that need it, so intent, fire and
/// appearance sequences match the all-serial pass.
pub(crate) fn run_with(state: &mut TickState, intents: &mut Vec<Intent>, min_parallel: usize) {
    // Take the reused predicate caches out of tick state (restored at the end
    // so capacity survives across ticks); reset once for this read-only pass.
    // `negative` retains capacity; stale slots beyond the live length are ignored.
    let mut scratch = std::mem::take(&mut state.contact_hazards_scratch);
    scratch.hazard.reset(&state.world);
    scratch.suffocation.reset(&state.world);
    let rows = state.entities.len();
    if scratch.negative.len() < rows {
        scratch.negative.resize(rows, None);
    }
    let pending = {
        let (world, hazard) = (&state.world, &scratch.hazard);
        let bodies = state.entities.bodies();
        let positions = state.entities.positions();
        classify_rows(
            &mut scratch.negative[..rows],
            min_parallel,
            |index, stamp| classify_row(bodies, positions, world, hazard, index, stamp),
        )
    };
    // This pass cannot spawn/remove rows; indexing avoids an ID snapshot.
    for &(index, ref work) in pending.iter().flatten() {
        match work {
            ContactWork::Scan => scan_row(state, &mut scratch, intents, index as usize),
            ContactWork::Quiet => retained_burn(
                state,
                crate::entities::EntitySlot(index),
                false,
                false,
                intents,
            ),
        }
    }
    state.contact_hazards_scratch = scratch;
}

/// The pre-split behaviour: the exact serial scan over every row. Differential
/// tests compare it against [`run_with`] on any thread count.
#[cfg(test)]
pub(crate) fn run_all_serial(state: &mut TickState, intents: &mut Vec<Intent>) {
    let mut scratch = std::mem::take(&mut state.contact_hazards_scratch);
    scratch.hazard.reset(&state.world);
    scratch.suffocation.reset(&state.world);
    if scratch.negative.len() < state.entities.len() {
        scratch.negative.resize(state.entities.len(), None);
    }
    for index in 0..state.entities.len() {
        scan_row(state, &mut scratch, intents, index);
    }
    state.contact_hazards_scratch = scratch;
}

fn scan_row(state: &mut TickState, scratch: &mut Scratch, intents: &mut Vec<Intent>, index: usize) {
    let living_by_type = &*LIVING_BY_TYPE;
    let slot = crate::entities::EntitySlot(index as u32);
    let row = state.entities.row(slot);
    let body = state.entities.body(slot);
    let kind = body.kind;
    let is_player = matches!(kind, crate::entities::EntityKind::Player { .. });
    let living = if is_player {
        true
    } else if let crate::entities::EntityKind::Mob { type_id } = kind {
        living_by_type
            .get(type_id as usize)
            .copied()
            .unwrap_or(false)
    } else {
        false
    };
    let item = matches!(kind, crate::entities::EntityKind::Item { .. });
    let entity = body.id;
    if !body.alive || (!living && !item) {
        scratch.negative[index] = None;
        return;
    }
    let pos = state.entities.pos(slot);
    let shape = [body.half_width, body.height];
    let previous = scratch.negative[index]
        .as_ref()
        .filter(|stamp| !item && stamp.entity == entity);
    // Reuse exact foot coordinates as well as body cells on a resting row.
    // Activity still gets checked every pass, even when content is unchanged.
    let foot_xz = if let Some(stamp) = previous
        && stamp.position[0] == pos[0]
        && stamp.position[2] == pos[2]
    {
        [stamp.probes[0][0], stamp.probes[0][2]]
    } else {
        [floor_i32(pos[0]), floor_i32(pos[2])]
    };
    if !state
        .world
        .is_active(foot_xz[0].div_euclid(16), foot_xz[1].div_euclid(16))
    {
        scratch.negative[index] = None;
        return;
    }
    let mut sneaking = row.appearance.shared_flags & 2 != 0;
    let mut moving = true;
    let mut immune = false;
    // Non-players keep `moving = true`; only players have player-map rows.
    if is_player && let Some(player) = state.players.get_mut(&entity) {
        immune = player.creative() || player.spectator() || player.vitals.is_dead();
        moving = player.last_contact_position.is_some_and(|old| {
            (old[0] - pos[0]).abs() > 1.0e-9 || (old[2] - pos[2]).abs() > 1.0e-9
        });
        player.last_contact_position = Some(pos);
        sneaking |= row.appearance.pose == 5;
    }
    if immune {
        scratch.negative[index] = None;
        return;
    }
    let (quiet, computed) = scratch.negative[index]
        .as_mut()
        .filter(|stamp| !item && stamp.entity == entity)
        .map(|stamp| quiet_proof(&state.world, &scratch.hazard, stamp, pos, shape, foot_xz))
        .unwrap_or((false, None));
    if quiet {
        retained_burn(state, slot, false, false, intents);
        return;
    }
    let (body_min, body_max, [support, eye]) =
        computed.unwrap_or_else(|| contact_cells(pos, shape, foot_xz));
    let eye_y = eye[1];
    // Gate the eye-cell read on resident sections holding any
    // suffocating state; overlay/missing chunks fail open to the exact
    // read so a fresh wall still suffocates.
    let eye_gate = state.world.may_hold_matching_state_cached(
        (eye[0], eye_y, eye[2]),
        (eye[0], eye_y, eye[2]),
        &mut scratch.suffocation,
        |seen| state.registry.light.suffocating(seen.0) == Some(true),
    );
    // Player-only: after the suffocation probe so mobs skip the lookup.
    let frost_walker = is_player
        && state.players.get(&entity).is_some_and(|p| {
            p.inventory.slots[36]
                .enchantments
                .iter()
                .any(|enchantment| {
                    enchantment.id.as_str() == "minecraft:frost_walker" && enchantment.level > 0
                })
        });
    // Gate body hazard samples (plus the magma support read) on section
    // palettes matching any nonzero hazard flag. The box conservatively
    // covers every scanned body cell plus the foot-center support cell
    // below; overlay/missing chunks fail open to the exact scan.
    let gate_min = (body_min.0, support[1], body_min.2);
    let gate_max = body_max;
    // A raw hit blocks memoization even when sneaking, frost-walker or
    // berry-motion rules suppress its current damage.
    let mut raw_body_hazard = false;
    let gated = {
        let flags = &state.registry.hazard_flags;
        state.world.may_hold_matching_state_cached(
            gate_min,
            gate_max,
            &mut scratch.hazard,
            |seen| flags.get(seen.0 as usize).copied().unwrap_or(0) != 0,
        )
    };
    // Borrow a resident column once for multi-cell hazard scans. Otherwise
    // retain the exact overlay-first reader; an eye-only pass needs just
    // one lookup and does not benefit from the column checks.
    let borrowed_chunk = if gated && state.world.overlay_is_empty() {
        let column = crate::boundary::ChunkPos {
            x: body_min.0.div_euclid(16),
            z: body_min.2.div_euclid(16),
        };
        (body_max.0.div_euclid(16) == column.x
            && body_max.2.div_euclid(16) == column.z
            && support[0].div_euclid(16) == column.x
            && support[2].div_euclid(16) == column.z
            && eye[0].div_euclid(16) == column.x
            && eye[2].div_euclid(16) == column.z)
            .then(|| state.world.chunk(column))
            .flatten()
    } else {
        None
    };
    let block_at = |x: i32, y: i32, z: i32| match borrowed_chunk {
        Some(chunk) => chunk
            .get_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8)
            .map(|id| id.0),
        None => state.world.block_at(x, y, z),
    };
    let flag_at = |x, y, z| {
        block_at(x, y, z)
            .and_then(|id| state.registry.hazard_flags.get(id as usize).copied())
            .unwrap_or(0)
    };
    let suffocates = eye_gate
        && block_at(eye[0], eye_y, eye[2])
            .is_some_and(|id| state.registry.light.suffocating(id) == Some(true));
    let raw_magma_support =
        gated && flag_at(support[0], support[1], support[2]) & HAZARD_MAGMA != 0;
    let hot = gated && !sneaking && !frost_walker && raw_magma_support;
    let (mut water, mut lava, mut fire, mut cactus, mut berry, mut campfire) =
        (false, false, false, false, false, false);
    if gated {
        for x in body_min.0..=body_max.0 {
            for y in body_min.1..=body_max.1 {
                for z in body_min.2..=body_max.2 {
                    let flags = flag_at(x, y, z);
                    if flags == 0 {
                        continue;
                    }
                    raw_body_hazard = true;
                    // State flags bake in age/lit properties; movement
                    // and posture still gate only the damage intent.
                    if flags & HAZARD_WATER != 0 {
                        water = true;
                    } else if flags & HAZARD_LAVA != 0 {
                        lava = true;
                    } else if flags & HAZARD_FIRE != 0 {
                        fire = true;
                    } else if flags & HAZARD_CACTUS != 0 {
                        cactus = true;
                    } else if flags & HAZARD_BERRY != 0 {
                        berry |= moving;
                    } else if flags & HAZARD_CAMPFIRE != 0 {
                        campfire |= !sneaking;
                    }
                }
            }
        }
    }
    if item {
        let kind = if cactus {
            Some(DamageKind::Cactus)
        } else if lava {
            Some(DamageKind::Lava)
        } else if fire {
            Some(DamageKind::InFire)
        } else {
            None
        };
        if let Some(kind) = kind
            && !row
                .item
                .as_ref()
                .is_some_and(|item| crate::item_hazards::resistant(state, item, kind))
        {
            damage(state, entity, kind, 1.0, intents);
        }
        scratch.negative[index] = None;
        return;
    }
    if suffocates {
        damage(state, entity, DamageKind::Suffocation, 1.0, intents)
    }
    if cactus {
        damage(state, entity, DamageKind::Cactus, 1.0, intents)
    }
    if berry {
        damage(state, entity, DamageKind::SweetBerryBush, 1.0, intents)
    }
    if hot {
        damage(state, entity, DamageKind::HotFloor, 1.0, intents)
    }
    if campfire {
        damage(state, entity, DamageKind::Campfire, 1.0, intents)
    }
    if lava {
        damage(state, entity, DamageKind::Lava, 4.0, intents)
    } else if fire {
        damage(state, entity, DamageKind::InFire, 1.0, intents)
    }
    // Evaluated only for burning contact so the common case skips the
    // player-map and gamerule probes; identical truth table to before.
    if (lava || fire)
        && !water
        && (!state.players.contains_key(&entity)
            || crate::command_rules::boolean(state, "fire_damage"))
    {
        ignite(state, entity, 160);
    }
    // Bank raw absence independently of the live burn clock. A newly
    // ignited body can remain geometrically quiet throughout its burn.
    scratch.negative[index] = if !suffocates && !raw_body_hazard && !raw_magma_support {
        Some(crate::world::ContactStamp {
            entity,
            position: pos,
            shape,
            min: body_min,
            max: body_max,
            probes: [support, eye],
            revision: state.world.revision_now(),
        })
    } else {
        None
    };
    retained_burn(state, slot, water, lava || fire, intents);
}
