//! All portal custody changes run after local commits, before the shared WAL.
use super::named;
use super::nether::{self, Frame};
use crate::boundary::{BlockPos, ChangeSet, EntityId};
use crate::dimensions::{Dimension, travel};
use crate::entities::EntitySlot;
use crate::tick::TickState;
use mc_physics::mth::{self, floor_i32};
use std::collections::BTreeMap;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Nether,
    End,
    Gateway,
}

type ContactBox = ((i32, i32, i32), (i32, i32, i32));

fn world_mut(state: &mut TickState, dimension: Dimension) -> &mut TickState {
    if state.dimension == dimension {
        state
    } else {
        state
            .other_dimensions
            .get_mut(&dimension)
            .expect("vanilla world")
    }
}

/// Numeric portal prefilter resolved once per [`finish`]: borrowed state-id
/// slices plus boss type ids. Per-pass palette verdicts are shared across bodies;
/// exact scans use only numeric state IDs, not per-mob strings.
struct Filter<'a> {
    dragon: Option<u32>,
    wither: Option<u32>,
    nether: &'a [mc_world::BlockStateId],
    end: &'a [mc_world::BlockStateId],
    gateway: &'a [mc_world::BlockStateId],
}

fn portal_states<'a>(
    registry: &'a crate::registry::CoreRegistry,
    id: &mc_data::Identifier,
) -> &'a [mc_world::BlockStateId] {
    registry
        .blocks
        .block(id)
        .map(|block| block.states.as_slice())
        .unwrap_or(&[])
}

impl<'a> Filter<'a> {
    fn resolve(registry: &'a crate::registry::CoreRegistry) -> Self {
        static DRAGON: std::sync::LazyLock<mc_data::Identifier> = std::sync::LazyLock::new(|| {
            mc_data::Identifier::parse("minecraft:ender_dragon").expect("vanilla entity id")
        });
        static WITHER: std::sync::LazyLock<mc_data::Identifier> = std::sync::LazyLock::new(|| {
            mc_data::Identifier::parse("minecraft:wither").expect("vanilla entity id")
        });
        static NETHER: std::sync::LazyLock<mc_data::Identifier> = std::sync::LazyLock::new(|| {
            mc_data::Identifier::parse("minecraft:nether_portal").expect("vanilla block id")
        });
        static END: std::sync::LazyLock<mc_data::Identifier> = std::sync::LazyLock::new(|| {
            mc_data::Identifier::parse("minecraft:end_portal").expect("vanilla block id")
        });
        static GATEWAY: std::sync::LazyLock<mc_data::Identifier> = std::sync::LazyLock::new(|| {
            mc_data::Identifier::parse("minecraft:end_gateway").expect("vanilla block id")
        });
        Self {
            dragon: registry.entity_types.id_of(&DRAGON),
            wither: registry.entity_types.id_of(&WITHER),
            nether: portal_states(registry, &NETHER),
            end: portal_states(registry, &END),
            gateway: portal_states(registry, &GATEWAY),
        }
    }
}

/// Inclusive contact box for one entity body, shared by the idle-skip gate
/// and the exact scan so both always cover the same cells.
fn contact_box(entities: &crate::entities::EntityStore, slot: EntitySlot) -> ContactBox {
    let row = entities.body(slot);
    let pos = entities.pos(slot);
    let w = row.half_width;
    (
        (
            floor_i32(pos[0] - w + 0.0001),
            floor_i32(pos[1]),
            floor_i32(pos[2] - w + 0.0001),
        ),
        (
            floor_i32(pos[0] + w - 0.0001),
            floor_i32(pos[1] + row.height - 0.0001),
            floor_i32(pos[2] + w - 0.0001),
        ),
    )
}

/// Whether one resident state id can be portal contact in this dimension.
/// Same predicate as the exact scan's palette gate (dimension-gated
/// nether/gateway slices plus the always-checked end slice).
fn contact_candidate(
    filter: &Filter<'_>,
    dimension: Dimension,
    seen: mc_world::BlockStateId,
) -> bool {
    (dimension != Dimension::End && filter.nether.contains(&seen))
        || filter.end.contains(&seen)
        || (dimension == Dimension::End && filter.gateway.contains(&seen))
}

fn contact_alive(
    entities: &crate::entities::EntityStore,
    slot: EntitySlot,
    filter: &Filter<'_>,
) -> bool {
    let row = entities.row(slot);
    entities.body(slot).alive
        && row.entity_life.mount.rider.is_none()
        && row.entity_life.mount.vehicle.is_none()
        && !matches!(entities.body(slot).kind, crate::entities::EntityKind::Mob { type_id } if Some(type_id) == filter.dragon || Some(type_id) == filter.wither)
}

/// Shared warm-proof check for the classifier and serial body. Raw equality
/// avoids cell math; drift computes coverage once and refreshes the baseline
/// only if the exact same cells still prove absence. Stale coverage is returned
/// for the palette gate and exact scan.
fn quiet_proof(
    entities: &crate::entities::EntityStore,
    chunks: &crate::world::ChunkWorld,
    cache: &crate::world::SectionMatchCache,
    slot: EntitySlot,
    stamp: &mut Option<crate::world::ContactStamp>,
) -> (bool, Option<ContactBox>) {
    let row = entities.body(slot);
    let pos = entities.pos(slot);
    let shape = [row.half_width, row.height];
    let Some(kept) = stamp.as_mut().filter(|kept| kept.entity == row.id) else {
        // Slot compaction/id reuse: never trust a dead slot's proof.
        *stamp = None;
        return (false, Some(contact_box(entities, slot)));
    };
    let raw_same = pos == kept.position && shape == kept.shape;
    let covered = if raw_same {
        (kept.min, kept.max)
    } else {
        contact_box(entities, slot)
    };
    let quiet = (raw_same || covered == (kept.min, kept.max))
        && chunks.contact_unchanged_since(covered.0, covered.1, kept.revision, cache);
    if quiet && !raw_same {
        kept.position = pos;
        kept.shape = shape;
    }
    (quiet, (!quiet).then_some(covered))
}

/// Pure read-only filter for one row of the portal pass. Returns `true` when
/// the serial body must run; `false` rows are fully resolved here (cleared or
/// refreshed stamp, nothing else). Players always go serial (spectator/dead
/// fence and timers); an idle row with a still-valid warm proof, or an idle
/// ineligible row, would only `continue` in the serial body.
fn classify_row(
    entities: &crate::entities::EntityStore,
    chunks: &crate::world::ChunkWorld,
    filter: &Filter<'_>,
    cache: &crate::world::SectionMatchCache,
    index: usize,
    stamp: &mut Option<crate::world::ContactStamp>,
) -> Option<()> {
    let slot = EntitySlot(index as u32);
    let row = entities.row(slot);
    if matches!(
        entities.body(slot).kind,
        crate::entities::EntityKind::Player { .. }
    ) {
        return Some(());
    }
    let timer = &row.entity_life.portal;
    if timer.cooldown != 0 || timer.elapsed != 0 {
        return Some(());
    }
    if !contact_alive(entities, slot, filter) {
        *stamp = None;
        return None;
    }
    // Cold/mismatched rows need the serial body; leave their geometry lazy.
    if stamp
        .as_ref()
        .is_none_or(|stamp| stamp.entity != entities.body(slot).id)
    {
        return Some(());
    }
    (!quiet_proof(entities, chunks, cache, slot, stamp).0).then_some(())
}

fn contact_gate(
    state: &TickState,
    filter: &Filter<'_>,
    box_: ContactBox,
    cache: &mut crate::world::SectionMatchCache,
) -> Option<ContactBox> {
    // Caller guarantees an eligible body and passes its covered box, computed
    // once and shared with the exact scan and the stamp. Memoize each
    // section's palette for all bodies in this read-only pass. Overlay writes
    // fail open, so stationary entities still observe edits.
    state
        .world
        .may_hold_matching_state_cached(box_.0, box_.1, cache, |seen| {
            contact_candidate(filter, state.dimension, seen)
        })
        .then_some(box_)
}

/// Exact contact scan over one gated box. Returns the touch plus whether any
/// covered cell holds a portal block at all: an End portal block outside the
/// inside shape (26.1 `Block.column(16, 6, 12)`) is contact `None` but real
/// geometric presence, so only a `false` second half proves absence.
fn contact_from_gate(
    state: &TickState,
    slot: EntitySlot,
    filter: &Filter<'_>,
    gate: Option<ContactBox>,
) -> (Option<(Kind, BlockPos)>, bool) {
    let Some((min, max)) = gate else {
        return (None, false);
    };
    let row = state.entities.body(slot);
    let pos = state.entities.pos(slot);
    let mut saw_block = false;
    for y in min.1..=max.1 {
        for x in min.0..=max.0 {
            for z in min.2..=max.2 {
                let cell = BlockPos { x, y, z };
                let Some(seen) = state.world.block_at(cell.x, cell.y, cell.z) else {
                    continue;
                };
                let seen = mc_world::BlockStateId(seen);
                if filter.nether.contains(&seen) && state.dimension != Dimension::End {
                    return (Some((Kind::Nether, cell)), true);
                }
                // 26.1 EndPortalBlock's inside shape is Block.column(16,6,12).
                if filter.end.contains(&seen) {
                    saw_block = true;
                    if pos[1] < f64::from(y) + 0.75 && pos[1] + row.height > f64::from(y) + 0.375 {
                        return (Some((Kind::End, cell)), true);
                    }
                }
                if state.dimension == Dimension::End && filter.gateway.contains(&seen) {
                    return (Some((Kind::Gateway, cell)), true);
                }
            }
        }
    }
    (None, saw_block)
}

/// Bank or drop one slot's geometric-absence proof after a probe.
///
/// Caller guarantees an eligible body (`contact_alive`): only a box whose
/// full probe found no portal block keeps a stamp. Any live touch, or any
/// portal block the End inside-shape test merely missed, clears it. The
/// `revision` baseline is the world's current revision; reuse needs the
/// same coverage plus `contact_unchanged_since`, so overlay writes, folded
/// edits and loads (all bump revisions or mark the overlay) wake the probe.
fn stamp_negative(
    world: &mut TickState,
    index: usize,
    id: EntityId,
    box_: ContactBox,
    touched: bool,
    saw_block: bool,
) {
    if touched || saw_block {
        world.portal_contact_stamps[index] = None;
        return;
    }
    let slot = EntitySlot(index as u32);
    let row = world.entities.body(slot);
    let pos = world.entities.pos(slot);
    world.portal_contact_stamps[index] = Some(crate::world::ContactStamp {
        entity: id,
        position: pos,
        shape: [row.half_width, row.height],
        min: box_.0,
        max: box_.1,
        probes: [],
        revision: world.world.revision_now(),
    });
}

/// Tickets are derived from live admission, so a restart/cancel cannot strand a
/// permanent forced-load. Missing destination chunks postpone without custody loss.
fn loaded(state: &mut TickState, id: EntityId, center: [f64; 3], radius: i32) -> bool {
    let center = mc_world::ChunkPos {
        x: floor_i32(center[0]).div_euclid(16),
        z: floor_i32(center[2]).div_euclid(16),
    };
    state
        .tickets
        .insert(format!("solaris:portal:{}", id.0), (center, radius));
    (-radius..=radius)
        .all(|dx| (-radius..=radius).all(|dz| state.world.has_chunk(center.x + dx, center.z + dz)))
}

fn arrival(
    shape: Frame,
    source: Option<Frame>,
    pos: [f64; 3],
    width: f64,
    height: f64,
) -> [f64; 3] {
    let (fraction, vertical) = source.map_or((0.5, 0.0), |source| {
        let along = if source.x_axis {
            pos[0] - f64::from(source.bottom.x)
        } else {
            pos[2] - f64::from(source.bottom.z)
        };
        let free = f64::from(source.width) - 2.0 * width;
        let horizontal = if free > 0.0 {
            ((along - width) / free).clamp(0.0, 1.0)
        } else {
            0.5
        };
        let free_height = f64::from(source.height) - height;
        let vertical = if free_height > 0.0 {
            ((pos[1] - f64::from(source.bottom.y)) / free_height).clamp(0.0, 1.0)
        } else {
            0.0
        };
        (horizontal, vertical)
    });
    let along = width + (f64::from(shape.width) - 2.0 * width).max(0.0) * fraction;
    [
        f64::from(shape.bottom.x) + if shape.x_axis { along } else { 0.5 },
        f64::from(shape.bottom.y) + (f64::from(shape.height) - height).max(0.0) * vertical,
        f64::from(shape.bottom.z) + if shape.x_axis { 0.5 } else { along },
    ]
}

fn safe_gateway(state: &TickState, exit: BlockPos, exact: bool) -> [f64; 3] {
    if exact {
        return [
            f64::from(exit.x) + 0.5,
            f64::from(exit.y),
            f64::from(exit.z) + 0.5,
        ];
    }
    let mut best = None;
    for dx in -5..=5 {
        for dz in -5..=5 {
            for y in
                (state.dimension.geometry().min_y()..state.dimension.geometry().max_y() - 2).rev()
            {
                if dx == 0 && dz == 0 {
                    continue;
                }
                let cell = BlockPos {
                    x: exit.x + dx,
                    y,
                    z: exit.z + dz,
                };
                if named(state, cell, "bedrock") {
                    continue;
                }
                if state.world.block_at(cell.x, y, cell.z).is_some_and(|id| {
                    state.registry.materials.classify(id) == mc_physics::BlockMaterial::Solid
                }) && named(state, BlockPos { y: y + 1, ..cell }, "air")
                    && named(state, BlockPos { y: y + 2, ..cell }, "air")
                {
                    if best.is_none_or(|(_, by, _, _)| y > by) {
                        best = Some((dx, y, dz, cell));
                    }
                    break;
                }
            }
        }
    }
    let cell = best
        .map(|(_, y, _, cell)| BlockPos { y: y + 1, ..cell })
        .unwrap_or(exit);
    [
        f64::from(cell.x) + 0.5,
        f64::from(cell.y),
        f64::from(cell.z) + 0.5,
    ]
}

pub(crate) fn finish(state: &mut TickState, changes: &mut BTreeMap<Dimension, ChangeSet>) {
    finish_with(
        state,
        changes,
        Some(crate::contact_hazards::MIN_PARALLEL_ROWS),
    );
}

/// `min_parallel` is the smallest row count classified on the rayon pool;
/// `None` skips the classifier so every row runs the serial body (the
/// differential-test reference).
pub(crate) fn finish_with(
    state: &mut TickState,
    changes: &mut BTreeMap<Dimension, ChangeSet>,
    min_parallel: Option<usize>,
) {
    state
        .tickets
        .retain(|key, _| !key.starts_with("solaris:portal:"));
    for world in state.other_dimensions.values_mut() {
        world
            .tickets
            .retain(|key, _| !key.starts_with("solaris:portal:"));
    }
    let registry = std::sync::Arc::clone(&state.registry);
    let filter = Filter::resolve(&registry);
    let dimensions: Vec<_> = std::iter::once(state.dimension)
        .chain(state.other_dimensions.keys().copied())
        .collect();
    let mut due = Vec::new();
    for dimension in dimensions {
        let world = world_mut(state, dimension);
        // Portal/world writes happen only in the queued travel phase below.
        let mut match_cache = std::mem::take(&mut world.portal_match_cache);
        match_cache.reset(&world.world);
        if world.portal_contact_stamps.len() != world.entities.len() {
            world
                .portal_contact_stamps
                .resize(world.entities.len(), None);
        }
        // Pure parallel pre-pass: rows whose warm geometric-absence proof holds
        // and whose timer is idle need nothing else (the serial body below would
        // only `continue`). Everything else runs the exact body in row order.
        let mut stamps = std::mem::take(&mut world.portal_contact_stamps);
        let pending = {
            let (entities, chunks) = (&world.entities, &world.world);
            let (filter, cache) = (&filter, &match_cache);
            match min_parallel {
                Some(min_parallel) => crate::contact_hazards::classify_rows(
                    &mut stamps,
                    min_parallel,
                    |index, stamp| classify_row(entities, chunks, filter, cache, index, stamp),
                ),
                // Test reference: every row takes the exact serial body.
                None => vec![(0..stamps.len() as u32).map(|index| (index, ())).collect()],
            }
        };
        world.portal_contact_stamps = stamps;
        for &(index, ()) in pending.iter().flatten() {
            let index = index as usize;
            let slot = EntitySlot(index as u32);
            let row = world.entities.body(slot);
            let id = row.id;
            // Spectators and dead players never travel or arm timers; retain
            // this fence before block reads. Only Player-kind rows own `players`
            // entries, so non-player bodies need no map probe.
            let barred = matches!(row.kind, crate::entities::EntityKind::Player { .. })
                && world.players.get(&id).is_some_and(|player| {
                    player.spectator()
                        || player.vitals.is_dead()
                        || world.session_locked(player.connection)
                });
            if barred {
                world.portal_contact_stamps[index] = None;
                continue;
            }
            let alive = contact_alive(&world.entities, slot, &filter);
            // Idle timers need no exact scan: the bookkeeping below only
            // touches the timer when contact registers or the timer is
            // already armed. `contact_gate` is the same palette prefilter the
            // exact scan uses (fail-open on overlay writes and missing
            // columns), evaluated once and reused, so a stationary entity or
            // a freshly placed portal is still observed with no stale state.
            let idle = {
                let timer = &world.entities.row(slot).entity_life.portal;
                timer.cooldown == 0 && timer.elapsed == 0
            };
            // Quiet hits skip geometry, but armed timers still decay below.
            // Ineligible bodies keep the legacy path without banking proofs.
            let (quiet, covered) = if alive {
                quiet_proof(
                    &world.entities,
                    &world.world,
                    &match_cache,
                    slot,
                    &mut world.portal_contact_stamps[index],
                )
            } else {
                world.portal_contact_stamps[index] = None;
                (false, None)
            };
            let gate = match (quiet, covered) {
                (true, _) => None,
                (false, Some(covered)) => contact_gate(world, &filter, covered, &mut match_cache),
                (false, None) => None,
            };
            if idle && gate.is_none() {
                if let Some(covered) = covered.filter(|_| alive && !quiet) {
                    // Palette-proven absence under an eligible body: bank it so
                    // the next pass reuses instead of rescanning sections.
                    stamp_negative(&mut *world, index, id, covered, false, false);
                }
                continue;
            }
            let (touch, saw_block) = if quiet {
                (None, false)
            } else {
                contact_from_gate(world, slot, &filter, gate)
            };
            if let Some(covered) = covered.filter(|_| alive && !quiet) {
                // Full-cell exact scan (or the palette gate when it proves
                // absence on its own) found no portal block under an
                // eligible body: bank the geometric absence. An End block
                // outside the inside shape sets `saw_block` and never
                // banks, so a later sub-cell dip into the shape rescans.
                stamp_negative(&mut *world, index, id, covered, touch.is_some(), saw_block);
            }
            // `delay` only gates Nether travel and `denied` only resets Nether
            // contact in the Overworld: idle, decaying and End/Gateway entities
            // skip both gamerule scans (same values when read, only later).
            let delay = if matches!(touch, Some((Kind::Nether, _))) {
                if let Some(player) = world.players.get(&id) {
                    crate::command_rules::integer(
                        world,
                        if player.creative() {
                            "players_nether_portal_creative_delay"
                        } else {
                            "players_nether_portal_default_delay"
                        },
                    )
                    .max(0) as u32
                } else {
                    0
                }
            } else {
                0
            };
            let denied = matches!(touch, Some((Kind::Nether, _)))
                && dimension == Dimension::Overworld
                && !crate::command_rules::boolean(world, "allow_entering_nether_using_portals");
            // The reset only runs on contact during cooldown; without contact
            // the value is unused, so idle/decaying entities skip the map probe.
            let cooldown = if touch.is_some() {
                if world.players.contains_key(&id)
                    || world.entities.row(slot).entity_life.vehicle.is_some()
                {
                    10
                } else {
                    300
                }
            } else {
                0
            };
            let timer = &world.entities.row(slot).entity_life.portal;
            if touch.is_none() && timer.cooldown == 0 && timer.elapsed == 0 {
                continue;
            }
            let timer = &mut world.entities.row_mut(slot).entity_life.portal;
            if timer.cooldown > 0 {
                timer.cooldown = timer.cooldown.saturating_sub(1);
                if touch.is_some() {
                    timer.cooldown = cooldown;
                }
                timer.elapsed = 0;
                continue;
            }
            if denied {
                timer.elapsed = 0;
                continue;
            }
            if let Some((kind, cell)) = touch {
                let elapsed = timer.elapsed;
                timer.elapsed = timer.elapsed.saturating_add(1).min(i32::MAX as u32);
                if kind != Kind::Nether || elapsed >= delay {
                    due.push((id, dimension, kind, cell));
                }
            } else {
                timer.elapsed = timer.elapsed.saturating_sub(4);
            }
        }
        world.portal_match_cache = match_cache;
    }
    for (id, source_dimension, kind, cell) in due {
        let Some((current, slot)) = travel::locate(state, id) else {
            continue;
        };
        if current != source_dimension {
            continue;
        }
        let source = travel::world(state, current).expect("source world");
        let pos = source.entities.pos(slot);
        let rot = source.entities.rot(slot);
        let vel = source.entities.vel(slot);
        let row = source.entities.body(slot);
        let is_player = source.players.contains_key(&id);
        let (width, height) = (row.half_width, row.height);
        let mut rotation = [rot[0], rot[1]];
        let mut velocity = vel;
        let (dimension, position) = match kind {
            Kind::Nether => {
                let destination = if current == Dimension::Overworld {
                    Dimension::Nether
                } else {
                    Dimension::Overworld
                };
                let Some(destination_world) = travel::world(state, destination) else {
                    continue;
                };
                let scale = current.coordinate_scale() / destination.coordinate_scale();
                let border = destination_world.border;
                let clamp = |value: f64, center: f64| {
                    value.clamp(
                        (center - border.size / 2.0).clamp(-29_999_872.0, 29_999_872.0),
                        (center + border.size / 2.0).clamp(-29_999_872.0, 29_999_872.0),
                    )
                };
                let target = [
                    clamp(pos[0] * scale, border.center_x),
                    pos[1],
                    clamp(pos[2] * scale, border.center_z),
                ];
                let source_axis = source
                    .world
                    .block_at(cell.x, cell.y, cell.z)
                    .and_then(|id| source.registry.blocks.by_id(mc_world::BlockStateId(id)))
                    .is_some_and(|block| {
                        block
                            .properties
                            .iter()
                            .any(|(k, v)| k == "axis" && v == "x")
                    });
                let source_frame = nether::frame(source, cell, source_axis);
                let destination_world = world_mut(state, destination);
                let radius = if destination == Dimension::Nether {
                    16
                } else {
                    128
                };
                if !loaded(destination_world, id, target, radius / 16 + 1) {
                    continue;
                }
                let mut writes = Vec::new();
                let shape =
                    nether::search(destination_world, target, radius).unwrap_or_else(|| {
                        nether::create(destination_world, target, source_axis, &mut writes)
                    });
                destination_world.world.fold_overlay();
                super::commit_removals(destination_world, changes.entry(destination).or_default());
                changes.entry(destination).or_default().blocks.extend(
                    writes
                        .into_iter()
                        .map(|(position, state)| crate::boundary::BlockChange { position, state }),
                );
                if shape.x_axis != source_axis {
                    rotation[0] += 90.0;
                    velocity = [vel[2], vel[1], -vel[0]];
                }
                (
                    destination,
                    arrival(shape, source_frame, pos, width, height),
                )
            }
            Kind::End => {
                if current != Dimension::End {
                    if travel::world(state, Dimension::End).is_none() {
                        continue;
                    }
                    let end = world_mut(state, Dimension::End);
                    if !loaded(end, id, [100.0, 50.0, 0.0], 1) {
                        continue;
                    }
                    let mut writes = Vec::new();
                    crate::end_portals::platform(
                        end,
                        &mut writes,
                        changes.entry(Dimension::End).or_default(),
                    );
                    end.world.fold_overlay();
                    changes.entry(Dimension::End).or_default().blocks.extend(
                        writes
                            .into_iter()
                            .map(|(position, state)| crate::boundary::BlockChange {
                                position,
                                state,
                            }),
                    );
                    rotation = [90.0, rot[1]];
                    let y = if is_player { 49.0 } else { 50.0 };
                    (Dimension::End, [100.5, y, 0.5])
                } else {
                    let point = source.players.get(&id).and_then(|player| {
                        player
                            .respawn_point
                            .map(|point| (player.respawn_dimension, point))
                    });
                    let mut destination =
                        point.map_or(Dimension::Overworld, |(dimension, _)| dimension);
                    if travel::world(state, destination).is_none() {
                        continue;
                    }
                    let fallback = travel::world(state, destination)
                        .unwrap()
                        .world
                        .world_spawn_position();
                    let target = point.map_or(fallback, |(_, cell)| {
                        [
                            f64::from(cell[0]) + 0.5,
                            f64::from(cell[1]),
                            f64::from(cell[2]) + 0.5,
                        ]
                    });
                    if !loaded(world_mut(state, destination), id, target, 1) {
                        continue;
                    }
                    let personal = {
                        let target = travel::world(state, destination).expect("return world");
                        let player = travel::world(state, current).unwrap().players.get(&id);
                        let personal = player
                            .and_then(|player| crate::sleep::respawn_position_for(target, player));
                        if let Some(player) = player.filter(|_| personal.is_some()) {
                            rotation = [player.respawn_yaw, player.respawn_pitch];
                        }
                        personal
                    };
                    let end_position = if let Some(personal) = personal {
                        personal
                    } else {
                        destination = Dimension::Overworld;
                        let fallback = travel::world(state, destination)
                            .expect("Overworld")
                            .world
                            .world_spawn_position();
                        if !loaded(world_mut(state, destination), id, fallback, 1) {
                            continue;
                        }
                        rotation = travel::world(state, destination)
                            .unwrap()
                            .world
                            .world_spawn_rotation();
                        fallback
                    };
                    velocity = [0.0; 3];
                    (destination, end_position)
                }
            }
            Kind::Gateway => {
                let mut image = match crate::end_portals::gateway(source, cell) {
                    Ok(image) => image,
                    Err(message) => {
                        if let Some(connection) = source.connections.entity_connection(id) {
                            crate::command_execution::feedback(
                                world_mut(state, current),
                                connection,
                                &message,
                                Vec::new(),
                            );
                        } else {
                            eprintln!("{message}");
                        }
                        continue;
                    }
                };
                if image.cooldown_until > state.tick {
                    continue;
                }
                let target = image.exit.map_or_else(
                    || {
                        let angle = mth::atan2(f64::from(cell.z), f64::from(cell.x));
                        [
                            f64::from(mth::cos(angle)) * 1024.0,
                            75.0,
                            f64::from(mth::sin(angle)) * 1024.0,
                        ]
                    },
                    |cell| [f64::from(cell.x), f64::from(cell.y), f64::from(cell.z)],
                );
                let end = world_mut(state, current);
                if !loaded(end, id, target, 1) {
                    continue;
                }
                if image.exit.is_none() {
                    image = crate::end_portals::initialize_gateway(
                        end,
                        cell,
                        changes.entry(current).or_default(),
                        image,
                    );
                }
                let exit = image.exit.expect("initialized gateway");
                let position = safe_gateway(end, exit, image.exact);
                image.cooldown_until = end.tick.saturating_add(40);
                crate::end_portals::save_gateway(end, cell, image);
                (current, position)
            }
        };
        let source = world_mut(state, current);
        let cooldown = if is_player || source.entities.row(slot).entity_life.vehicle.is_some() {
            10
        } else {
            300
        };
        source.entities.row_mut(slot).entity_life.portal = super::PortalState {
            elapsed: 0,
            cooldown,
        };
        if travel::change_dimension(state, id, dimension, position, rotation, changes).is_ok() {
            let destination = world_mut(state, dimension);
            let slot = destination
                .entities
                .slot_of_any(id)
                .expect("transferred identity");
            destination.entities.set_vel(slot, velocity);
            destination.entities.row_mut(slot).entity_life.portal = super::PortalState {
                elapsed: 0,
                cooldown,
            };
        }
    }
}
