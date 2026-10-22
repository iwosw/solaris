//! Buffers retain their high-water capacity across publishes.
use super::*;

#[derive(Debug, Default)]
pub(super) struct PlanScratch {
    pub visible: Vec<EntityId>,
    pub spawns: Vec<(EntityId, f64)>,
    pub bosses: Vec<EntityId>,
    pub removed: Vec<i32>,
    pub tracking_ranges: HashSet<i32>,
    pub chunks: Vec<(i32, i32)>,
    pub touched: HashSet<(i32, i32)>,
    pub blocks: Vec<mc_world::chunk::BlockPos>,
    pub events: Vec<usize>,
    /// Held, in-window chunks named by this publish's delta.
    pub delta_chunks: Vec<(i32, i32)>,
}

#[derive(Debug, Default)]
pub(super) struct ViewWindow {
    key: Option<((i32, i32), i32)>,
    pub order: Vec<(i32, i32)>,
    pub want: HashSet<(i32, i32)>,
}

impl ViewWindow {
    /// Returns true when the window moved (or was never computed).
    pub fn update(&mut self, center: (i32, i32), distance: i32) -> bool {
        let distance = distance.max(0);
        if self.key == Some((center, distance)) {
            return false;
        }
        self.key = Some((center, distance));
        self.order = spiral_square(center, distance);
        self.want.clear();
        self.want.extend(self.order.iter().copied());
        true
    }
}

#[cfg(test)]
/// Query only the viewer's square through the persistent chunk index.
pub(super) fn visible_entity_ids(
    snapshot: &WorldSnapshot,
    center: (i32, i32),
    distance: i32,
    viewer: Option<&ViewerState>,
    visible: &mut Vec<EntityId>,
) {
    visible.clear();
    let distance = distance.max(0);
    for x in center.0 - distance..=center.0 + distance {
        for z in center.1 - distance..=center.1 + distance {
            if viewer.is_some_and(|viewer| !viewer.loaded_chunks.contains_key(&(x, z))) {
                continue;
            }
            visible.extend(
                snapshot
                    .entities
                    .in_chunk(mc_core::boundary::ChunkPos { x, z })
                    .map(|entity| entity.id),
            );
        }
    }
    // Self metadata is allowed before its chunk admission, without scanning
    // the unadmitted population to find the player's row.
    if let Some(viewer) = viewer
        && let Some(own) = snapshot.entities.get(viewer.entity_id)
    {
        let chunk = entity_chunk(own.position);
        if !viewer.loaded_chunks.contains_key(&chunk)
            && (chunk.0 - center.0).abs() <= distance
            && (chunk.1 - center.1).abs() <= distance
        {
            visible.push(own.id);
        }
    }
}
