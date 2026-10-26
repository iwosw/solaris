//! Builder navigation and physical hauling. The shared structure executor is
//! the sole placement/progress authority; resident carry is real native stock.
use super::{Activity, VillagerProfession, VillagerState};
use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntitySlot;
use crate::plugin_structures::settlement_builds as executor;
use crate::settlements::{SettlementId, blueprints, construction};
use crate::tick::TickState;

/// Perimeter goals sit this many cells outside the future footprint. The
/// brain accepts arrival within 0.3 of a cell centre, and the body half-width
/// is 0.3, so the adjacent cell (centre 0.5 from the edge) can leave the body
/// straddling the footprint and the builder would never place.
const MARGIN: i32 = 2;
/// Placement reach from the builder's feet to the next cell.
const REACH: f64 = 12.0;

/// Containers are approached; a perimeter cell must actually be reached.
pub(crate) struct WorkTarget {
    pub(crate) pos: BlockPos,
    pub(crate) exact: bool,
}

fn block_pos([x, y, z]: [i32; 3]) -> BlockPos {
    BlockPos { x, y, z }
}

pub(crate) fn working(state: &TickState, v: &VillagerState) -> bool {
    v.data.profession == VillagerProfession::Builder
        && v.age >= 0
        && !v.sleeping
        && v.conversion_time.is_none()
        && v.activity == Activity::Work
        && (2_000..9_000).contains(&state.day_time.rem_euclid(24_000))
        && crate::command_rules::boolean(state, crate::command_rules::BoolRule::MOB_GRIEFING)
}

fn carried(v: &VillagerState, item: u32, count: u32) -> bool {
    v.inventory
        .iter()
        .flatten()
        .filter(|stack| stack.item_id == item && stack.count > 0)
        .map(|stack| stack.count as u32)
        .sum::<u32>()
        >= count
}

fn storage_target(
    state: &TickState,
    slot: EntitySlot,
    id: SettlementId,
    item: u32,
) -> Option<BlockPos> {
    executor::pickup_point(state, id, item, state.entities.pos(slot)).map(block_pos)
}

fn site_target(
    state: &TickState,
    project: &construction::Project,
    next: [i32; 3],
) -> Option<BlockPos> {
    let blueprint = blueprints::get(&state.registry, &project.blueprint)?;
    let size = blueprint.footprint(blueprints::Rotation::from_quarter_turns(project.rotation));
    let min = project.origin;
    let max = [
        min[0] + i32::from(size[0]),
        min[1],
        min[2] + i32::from(size[2]),
    ];
    [
        [min[0] - MARGIN, min[1] + 1, next[2]],
        [max[0] - 1 + MARGIN, min[1] + 1, next[2]],
        [next[0], min[1] + 1, min[2] - MARGIN],
        [next[0], min[1] + 1, max[2] - 1 + MARGIN],
    ]
    .into_iter()
    .min_by_key(|point| point[0].abs_diff(next[0]) + point[2].abs_diff(next[2]))
    .map(block_pos)
}

/// Brain hook only selects a navigation target while villager state is detached.
/// Inventory/world transactions run below with the real entity row attached.
pub(crate) fn work(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    _changes: &mut ChangeSet,
) -> Option<WorkTarget> {
    if !working(state, v) {
        return None;
    }
    let id = v.settlement?;
    let project = state.settlements.get(id)?.project.as_ref()?;
    let next = executor::next_cell(state, id, &project.structure).ok()??;
    if let Some((item, count)) = executor::next_material(state, id, &project.structure).ok()?
        && !carried(v, item, count)
    {
        return Some(WorkTarget {
            pos: storage_target(state, slot, id, item)?,
            exact: false,
        });
    }
    // Already outside the footprint and in reach of the next cell: keep
    // standing here rather than walking around the site for every cell.
    if in_reach(state, slot, project, next) {
        let here = crate::settlements::allocator::at(state.entities.pos(slot));
        return Some(WorkTarget {
            pos: here,
            exact: true,
        });
    }
    Some(WorkTarget {
        pos: site_target(state, project, next)?,
        exact: true,
    })
}

/// Whole body outside every future cell and within placement reach of `next`.
/// The 11×9×11 maximum footprint needs sqrt(7.5² + 8²) ≈ 11 blocks from the
/// nearest perimeter point to its farthest roof cell, plus arrival tolerance.
fn in_reach(
    state: &TickState,
    slot: EntitySlot,
    project: &construction::Project,
    next: [i32; 3],
) -> bool {
    let Some(blueprint) = blueprints::get(&state.registry, &project.blueprint) else {
        return false;
    };
    let size = blueprint.footprint(blueprints::Rotation::from_quarter_turns(project.rotation));
    let pos = state.entities.pos(slot);
    let width = state.entities.body(slot).half_width;
    // Choosing a perimeter goal does not mean the builder has reached it.
    // Keep the full body outside every future cell, not just the next one:
    // placing a wall around a builder can otherwise strand it inside the site.
    let outside = [0, 2].into_iter().any(|axis| {
        let min = f64::from(project.origin[axis]);
        pos[axis] + width <= min || pos[axis] - width >= min + f64::from(size[axis])
    });
    outside && super::mining::near(state, slot, block_pos(next), REACH)
}

/// Called once per construction admission, before the brain detaches carry.
/// One transfer OR one canonical one-cell advance, never both in an attempt.
pub(crate) fn tick(state: &mut TickState, id: SettlementId) {
    let Some(project) = state
        .settlements
        .get(id)
        .and_then(|record| record.project.clone())
    else {
        return;
    };
    // A paused footprint is re-snapshotted and resumed on the next admission.
    if executor::recover(state, id, &project.structure) == Ok(true) {
        return;
    }
    let next = match executor::next_cell(state, id, &project.structure) {
        Ok(Some(next)) => next,
        Ok(None) => {
            construction::finish_project(state, id);
            return;
        }
        Err(_) => return,
    };
    let Some(slot) = construction::residents(state, id).find(|slot| {
        state.entities.body(*slot).alive
            && state
                .entities
                .row(*slot)
                .villager
                .as_ref()
                .is_some_and(|v| v.settlement == Some(id) && working(state, v))
    }) else {
        return;
    };
    let Ok(item) = executor::next_material(state, id, &project.structure) else {
        return;
    };
    if let Some((item, count)) = item
        && !carried(
            state.entities.row(slot).villager.as_ref().expect("builder"),
            item,
            count,
        )
    {
        if let Some(point) = storage_target(state, slot, id, item)
            && super::mining::near(state, slot, point, 2.5)
        {
            let uuid = state.entities.row(slot).uuid;
            let _ = executor::fetch(state, uuid, [point.x, point.y, point.z], item);
        }
        return;
    }
    if !in_reach(state, slot, &project, next) {
        return;
    }
    let uuid = state.entities.row(slot).uuid;
    if executor::advance(state, id, &project.structure, uuid) == Ok(true) {
        construction::finish_project(state, id);
    }
}
