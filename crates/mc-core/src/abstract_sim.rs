//! Decision 001-f: bounded resident catch-up on the single tick merge owner.
//! Each transaction spends one time step for the entire chunk, not one target.
use crate::boundary::{BlockPos, ChunkPos};
use crate::entities::{EntityKind, EntitySlot};
use crate::intent::Footprint;
use crate::tick::TickState;

const CHUNKS_PER_TICK: usize = 2;
const EFFECTS_PER_TICK: usize = 4096;
const STEP_TICKS: u64 = 200;

/// Reuse existing custody/touched marks, before eviction, catch-up and journal
/// baseline updates. No new queue or load-time target fingerprint is needed.
pub(crate) fn cancel_external_writes(state: &mut TickState) {
    for (x, _, z) in state.block_entities.take_external_writes() {
        state.world.cancel_simulation_debt(ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        });
    }
    for id in state.entities.touched_ids() {
        let Some(slot) = state.entities.slot_of(*id) else {
            continue;
        };
        if !matches!(state.entities.body(slot).kind, EntityKind::Item { .. }) {
            continue;
        }
        let pos = state.entities.pos(slot);
        let chunk = ChunkPos {
            x: (pos[0].floor() as i32).div_euclid(16),
            z: (pos[2].floor() as i32).div_euclid(16),
        };
        // Hydration seeds this existing WAL baseline in restore_entity.
        // Only a birth or an accepted chunk crossing invalidates old time.
        if !state
            .journaled_entity_state
            .get(id)
            .is_some_and(|(_, old_chunk, _)| *old_chunk == chunk)
        {
            state.world.cancel_simulation_debt(chunk);
        }
    }
}

pub(crate) fn run(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>) {
    let mut chunks = state.world.catch_up_chunks();
    if chunks.is_empty() {
        return;
    }
    let rotation = state.tick as usize % chunks.len();
    chunks.rotate_left(rotation);
    let mut budget = EFFECTS_PER_TICK;
    for chunk in chunks.into_iter().take(CHUNKS_PER_TICK) {
        let elapsed = state.world.simulation_debt(chunk).min(STEP_TICKS);
        let crops = crate::farming::catch_up_crops(state, chunk, elapsed);
        let mut entities = Vec::new();
        state.entities.for_each_slot_in_chunk(chunk, |slot| {
            let body = state.entities.body(slot);
            let row = state.entities.row(slot);
            let pos = state.entities.pos(slot);
            if body.alive
                && (matches!(body.kind, EntityKind::Item { .. }) || row.villager.is_some())
                && (pos[0].floor() as i32).div_euclid(16) == chunk.x
                && (pos[2].floor() as i32).div_euclid(16) == chunk.z
            {
                entities.push(slot);
            }
        });
        // Dense order is part of catch-up staging; merged chunk indexes are
        // ordered by id, not slot, and type transitions can leave duplicate IDs.
        entities.sort_unstable();
        entities.dedup();
        let containers = state
            .block_entities
            .keys()
            .filter(|&&(x, _, z)| x.div_euclid(16) == chunk.x && z.div_euclid(16) == chunk.z)
            .count();
        let cost = crops.len() + entities.len() + containers;
        if cost > EFFECTS_PER_TICK {
            // A whole-chunk step cannot be split by target. Drop an oversized
            // interval rather than violate the hard effect cap or replay time.
            state.world.cancel_simulation_debt(chunk);
            continue;
        }
        if cost > budget {
            continue;
        }
        budget -= cost;
        for (at, expected, next) in crops {
            let footprint = Footprint {
                block_expect: vec![(at, expected)],
                ..Default::default()
            };
            if state.footprint_valid(&footprint) {
                state.world.set_block_catch_up(at.x, at.y, at.z, next);
                writes.push((at, next));
            }
        }
        crate::tick::tick_furnaces(state, writes, Some((chunk, elapsed)));
        for slot in entities {
            if matches!(state.entities.body(slot).kind, EntityKind::Item { .. }) {
                if state.merge_consumed_items.contains(&slot)
                    || state.merge_pickups.iter().any(|(item, _, _)| *item == slot)
                {
                    continue;
                }
                let (body, row) = state.entities.parts_mut(slot);
                row.age = row
                    .age
                    .saturating_add(elapsed.min(u64::from(u32::MAX)) as u32);
                body.pickup_delay = body
                    .pickup_delay
                    .saturating_sub(elapsed.min(u64::from(u32::MAX)) as u32);
                if row.age >= 6000 {
                    if let Some(item) = row.item.as_mut() {
                        item.count = 0;
                    }
                    // Dense slots remain stable until the existing commit owner
                    // has consumed all previously admitted slot-based intents.
                    state.merge_consumed_items.insert(slot);
                }
            } else {
                restock(state, slot, chunk, elapsed);
            }
        }
        state.world.consume_simulation_debt(chunk, elapsed);
    }
}

/// No movement, POI acquisition or brain execution for inactive time. Only the
/// existing work/restock counters advance, with a resident reachable job site.
fn restock(state: &mut TickState, slot: EntitySlot, chunk: ChunkPos, elapsed: u64) {
    let Some(v) = state.entities.row(slot).villager.as_ref() else {
        return;
    };
    let Some(job) = v
        .job_site
        .filter(|_| v.job_site_dimension == state.dimension.name())
    else {
        return;
    };
    let pos = state.entities.pos(slot);
    if crate::settlements::allocator::distance(pos, crate::settlements::allocator::centre(job))
        > crate::settlements::allocator::JOB_SITE_REACH_SQUARED
    {
        return;
    }
    let Some(block) = state.world.block_at(job.x, job.y, job.z) else {
        return;
    };
    if crate::settlements::allocator::poi_kind(&state.registry, block)
        != Some(crate::settlements::allocator::PoiKind::Job(
            v.data.profession,
        ))
    {
        return;
    }
    let footprint = Footprint {
        block_expect: vec![(job, block)],
        ..Default::default()
    };
    if !state.footprint_valid(&footprint) {
        return;
    }
    let debt = state.world.simulation_debt(chunk);
    let start_tick = state.tick.saturating_sub(debt);
    // A live work pass may already have observed a newer counter image while
    // the chunk still owes other targets time. Never rewind that image.
    let skipped = u64::try_from(v.last_work)
        .unwrap_or(0)
        .saturating_add(1)
        .saturating_sub(start_tick)
        .min(elapsed);
    let elapsed = elapsed - skipped;
    if elapsed == 0 {
        return;
    }
    let start_tick = start_tick.saturating_add(skipped);
    let start_day = state
        .day_time
        .saturating_sub(i64::try_from(debt).unwrap_or(i64::MAX))
        .saturating_add(skipped as i64);
    let phase = start_day.rem_euclid(24_000);
    let first_offset = if phase < 2000 {
        2000 - phase
    } else if phase < 9000 {
        0
    } else {
        24_000 - phase + 2000
    } as u64;
    if first_offset >= elapsed {
        return;
    }
    let first_day = start_day.saturating_add(first_offset as i64);
    let last_offset =
        (elapsed - 1).min(first_offset + (8999 - first_day.rem_euclid(24_000)) as u64);
    let v = state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .expect("resident villager");
    crate::villager::trades::work(
        v,
        &state.registry,
        start_tick.saturating_add(first_offset),
        first_day,
    );
    // A second restock can become due inside the interval. There are no
    // abstract customer trades, so no further per-tick work is necessary.
    let due_tick = u64::try_from(v.last_restock)
        .unwrap_or(0)
        .saturating_add(2401);
    let due_offset = due_tick.saturating_sub(start_tick).max(first_offset);
    if due_offset > first_offset && due_offset <= last_offset {
        crate::villager::trades::work(
            v,
            &state.registry,
            start_tick.saturating_add(due_offset),
            start_day.saturating_add(due_offset as i64),
        );
    }
    if last_offset > first_offset {
        crate::villager::trades::work(
            v,
            &state.registry,
            start_tick.saturating_add(last_offset),
            start_day.saturating_add(last_offset as i64),
        );
    }
}
