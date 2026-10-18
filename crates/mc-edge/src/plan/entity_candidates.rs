//! Fused per-viewer entity candidate pass: one walk fills `visible`,
//! `spawns` (nearest-distance tuples) and `bosses`, preserving the exact
//! `visible_entity_ids` order (x-major, z-minor, own row last).
use super::*;

/// Collect trackable candidates for one viewer tick.
///
/// Clears and reuses `scratch.visible/spawns/bosses` (no per-row
/// allocation). Marks `tracked.seen` for known same-type rows. A tracked
/// row with a mismatched type stays unmarked but IS a spawn candidate:
/// the root removes the stale type before spawning. Health policy stays in
/// `BossBaseline`.
pub(super) fn collect(
    snapshot: &WorldSnapshot,
    viewer: &mut ViewerState,
    view_distance: i32,
    scratch: &mut PlanScratch,
    spawn_credit: usize,
) {
    scratch.visible.clear();
    scratch.spawns.clear();
    scratch.bosses.clear();
    scratch.tracking_ranges.clear();
    if viewer.plan_stamp == u64::MAX {
        // Reset only on wrap so no old mark can equal the new plan stamp.
        for tracked in viewer.tracked_entities.values_mut() {
            tracked.seen = 0;
        }
        viewer.plan_stamp = 1;
    } else {
        viewer.plan_stamp += 1;
    }
    let stamp = viewer.plan_stamp;
    let own = viewer.entity_id;
    let [vx, vy, vz] = viewer.position;
    let center = super::tick_index::entity_chunk(viewer.position);
    let distance = view_distance.max(0);
    let want_spawn = spawn_credit > 0;

    for x in center.0 - distance..=center.0 + distance {
        for z in center.1 - distance..=center.1 + distance {
            if !viewer.loaded_chunks.contains_key(&(x, z)) {
                continue;
            }
            for entity in snapshot
                .entities
                .in_chunk(mc_core::boundary::ChunkPos { x, z })
            {
                if let Some(range) = entity_interest::tracking_range(entity.type_id, distance) {
                    scratch.tracking_ranges.insert(range);
                }
                if !super::entity_trackable(entity, &*viewer, view_distance) {
                    continue;
                }
                scratch.visible.push(entity.id);
                let known_same = match viewer.tracked_entities.get_mut(&entity.id) {
                    Some(tracked) if tracked.type_id == entity.type_id => {
                        tracked.seen = stamp;
                        true
                    }
                    _ => false,
                };
                if want_spawn && entity.id != own && !known_same {
                    let dist = (entity.position[0] - vx).powi(2)
                        + (entity.position[1] - vy).powi(2)
                        + (entity.position[2] - vz).powi(2);
                    scratch.spawns.push((entity.id, dist));
                }
                if entity_type_flags::is_boss(entity.type_id) {
                    scratch.bosses.push(entity.id);
                }
            }
        }
    }

    // Self metadata is allowed before its chunk admission, without scanning
    // the unadmitted population to find the player's row.
    if let Some(mine) = snapshot.entities.get(own) {
        let chunk = super::tick_index::entity_chunk(mine.position);
        if !viewer.loaded_chunks.contains_key(&chunk)
            && (chunk.0 - center.0).abs() <= distance
            && (chunk.1 - center.1).abs() <= distance
            && super::entity_trackable(mine, &*viewer, view_distance)
        {
            scratch.visible.push(mine.id);
            if let Some(tracked) = viewer.tracked_entities.get_mut(&mine.id)
                && tracked.type_id == mine.type_id
            {
                tracked.seen = stamp;
            }
            if entity_type_flags::is_boss(mine.type_id) {
                scratch.bosses.push(mine.id);
            }
        }
    }
}
