//! Settlement shepherd: shears adult unsheared sheep inside the settlement
//! radius and carries the vanilla-colored wool like any other gathered stock.
//!
//! A bound [`VillagerProfession::Shepherd`] scans the settlement chunks
//! through the entity chunk index (never the whole world), walks to the
//! nearest adult unsheared living sheep, and shears it once within reach.
//! The whole yield must fit the real villager inventory first; otherwise the
//! shepherd routes to physical settlement storage through
//! [`super::mining::deposit`] and the sheep keeps its wool. No shears tool is
//! required: the owner explicitly wants an attainable autonomous wool source.
//! Regrowth reuses the native grazing logic untouched: only the sheared bit
//! is set, the color byte is preserved.
use super::mining;
use super::{Activity, VillagerProfession, VillagerState};
use crate::boundary::{BlockPos, ChangeSet, ChunkPos};
use crate::entities::{EntityKind, EntitySlot};
use crate::tick::TickState;
use mc_data::ItemStack;

/// Ticks between shear attempts of one working shepherd.
const PERIOD: u64 = 100;
/// Shear reach in blocks, matching the interact geometry other workers use.
const REACH: f64 = 3.0;

/// Nearest eligible sheep among at most 256 chunk-local sensor entries.
/// The chunk range is settlement-bounded; unrelated world populations never
/// contribute to this work attempt.
fn find_sheep(
    state: &TickState,
    anchor: [i32; 3],
    radius: u32,
    from: [f64; 3],
) -> Option<EntitySlot> {
    let sheep = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").ok()?)?;
    let span = radius as i32;
    let limit = f64::from(radius) * f64::from(radius);
    let (min_cx, max_cx) = (
        anchor[0].saturating_sub(span).div_euclid(16),
        anchor[0].saturating_add(span).div_euclid(16),
    );
    let (min_cz, max_cz) = (
        anchor[2].saturating_sub(span).div_euclid(16),
        anchor[2].saturating_add(span).div_euclid(16),
    );
    let mut best: Option<(EntitySlot, f64)> = None;
    let mut examined = 0;
    for cx in min_cx..=max_cx {
        for cz in min_cz..=max_cz {
            let Some(ids) = state
                .entities
                .brain_sensor_chunks()
                .get(&ChunkPos { x: cx, z: cz })
            else {
                continue;
            };
            for id in ids {
                if examined == 256 {
                    return best.map(|(slot, _)| slot);
                }
                examined += 1;
                let Some(slot) = state.entities.slot_of(*id) else {
                    continue;
                };
                let body = state.entities.body(slot);
                if !matches!(body.kind, EntityKind::Mob { type_id } if type_id == sheep) {
                    continue;
                }
                let row = state.entities.row(slot);
                if !crate::entity_life::adult(row)
                    || crate::tick::wool_of(&row.appearance) & 0x10 != 0
                {
                    continue;
                }
                let p = state.entities.pos(slot);
                let inside = (p[0] - f64::from(anchor[0])).powi(2)
                    + (p[1] - f64::from(anchor[1])).powi(2)
                    + (p[2] - f64::from(anchor[2])).powi(2);
                if inside > limit {
                    continue;
                }
                let distance =
                    (p[0] - from[0]).powi(2) + (p[1] - from[1]).powi(2) + (p[2] - from[2]).powi(2);
                if best.is_none_or(|(_, kept)| distance < kept) {
                    best = Some((slot, distance));
                }
            }
        }
    }
    best.map(|(slot, _)| slot)
}

fn deposit(state: &mut TickState, slot: EntitySlot, v: &mut VillagerState) -> Option<BlockPos> {
    let destination = mining::deposit(state, slot, v);
    if destination.is_none() && v.inventory.iter().any(Option::is_some) {
        v.harvest_at = state.tick + PERIOD;
    }
    destination
}

pub(super) fn work(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
    _changes: &mut ChangeSet,
) -> Option<BlockPos> {
    if v.data.profession != VillagerProfession::Shepherd
        || v.age < 0
        || v.sleeping
        || v.conversion_time.is_some()
        || v.activity == Activity::Panic
    {
        return None;
    }
    let settlement = v.settlement?;
    let record = state.settlements.get(settlement)?;
    let (anchor, radius) = (record.anchor, record.radius);
    let working = v.activity == Activity::Work
        && (2_000..9_000).contains(&state.day_time.rem_euclid(24_000))
        && crate::command_rules::boolean(state, crate::command_rules::BoolRule::MOB_GRIEFING);
    if !working || v.inventory.iter().any(Option::is_some) {
        return deposit(state, slot, v);
    }
    if state.tick < v.harvest_at {
        return None;
    }
    let from = state.entities.pos(slot);
    let target = find_sheep(state, anchor, radius, from)?;
    let feet = state.entities.pos(target);
    let block = BlockPos {
        x: feet[0].floor() as i32,
        y: feet[1].floor() as i32,
        z: feet[2].floor() as i32,
    };
    if !mining::near(state, slot, block, REACH) {
        // Walk without consuming the shear cooldown: the next attempt
        // re-scans and keeps navigating until within reach.
        return Some(block);
    }
    // The whole yield must fit before the sheep is touched; otherwise haul
    // first and the wool stays on the sheep.
    let wool = crate::tick::wool_of(&state.entities.row(target).appearance);
    let color = usize::from(wool & 15);
    let name = format!(
        "{}_wool",
        crate::blueprint::DYE_COLORS
            .get(color)
            .copied()
            .unwrap_or("white")
    );
    let Some(item) = mining::item_id(state, &name) else {
        v.harvest_at = state.tick + PERIOD;
        return None;
    };
    let count = 1 + (u64::from(target.0) ^ state.tick).wrapping_mul(0x9E37_79B9_7F4A_7C15) % 3;
    let stack = ItemStack::new(item, count as i32);
    if mining::room(state, v, &stack) < count as i32
        || !mining::collect(state, v, item, count as i32)
    {
        return deposit(state, slot, v);
    }
    let id = state.entities.body(target).id;
    let appearance = std::sync::Arc::make_mut(&mut state.entities.row_mut(target).appearance);
    crate::tick::set_wool(appearance, wool | 0x10);
    state.appearance_changed.push(id);
    v.harvest_at = state.tick + PERIOD;
    None
}
