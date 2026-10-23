//! Shared food temptation and juvenile following over immutable tick inputs.
//! No retained target, allocation, or world-wide animal scan: children query
//! the existing chunk membership, players share the entity-phase target grid.
use super::{ReadRow, StepCtx};
use crate::boundary::{ChunkPos, ConnectionId, EntityId, Hand};
use crate::entities::EntityKind;
use crate::player::PlayerState;
use crate::target_grid::{TargetGrid, distance};
use rustc_hash::FxHashMap;
use std::collections::BTreeSet;

pub(super) struct Context<'a> {
    pub players: TargetGrid<'a>,
    pub states: &'a FxHashMap<EntityId, PlayerState>,
    pub locked: Option<&'a BTreeSet<ConnectionId>>,
}

pub(super) fn idle(row: ReadRow, ctx: &StepCtx, look: &mut Option<[f64; 3]>) -> [f64; 3] {
    if let Some((target, stop)) = target(row, ctx) {
        *look = Some(target);
        if distance(row.pos, target) <= stop {
            return [0.0; 3];
        }
        return crate::entity_motion::steer(row.movement, row.pos, target, f64::from(row.speed));
    }
    crate::entity_motion::wander(row.movement, row.id.0, ctx.tick, row.speed)
}

fn target(row: ReadRow, ctx: &StepCtx) -> Option<([f64; 3], f64)> {
    let blueprint = ctx.registry.blueprint(row.type_id)?;
    if blueprint.capabilities.temptable
        && !blueprint.breed_food.is_empty()
        && let Some((index, _)) = ctx
            .passive
            .players
            .nearest_within(row.pos, 10.0, |(id, _)| {
                let Some(player) = ctx.passive.states.get(id) else {
                    return false;
                };
                !player.spectator()
                    && !ctx
                        .passive
                        .locked
                        .is_some_and(|locked| locked.contains(&player.connection))
                    && [Hand::Main, Hand::Off].into_iter().any(|hand| {
                        let held = &player.inventory.slots[super::held_slot_index(player, hand)];
                        !held.is_empty() && blueprint.breed_food.contains(&held.item_id)
                    })
            })
    {
        return Some((ctx.passive.players.table()[index].1, 2.5));
    }
    if !blueprint.capabilities.follows_adults
        || crate::entity_life::adult(ctx.entities.row(row.slot))
    {
        return None;
    }
    let chunk = |v: f64| mc_physics::mth::floor_i32(v).div_euclid(16);
    let mut nearest: Option<(EntityId, [f64; 3], f64)> = None;
    for z in chunk(row.pos[2] - 8.0)..=chunk(row.pos[2] + 8.0) {
        for x in chunk(row.pos[0] - 8.0)..=chunk(row.pos[0] + 8.0) {
            ctx.entities
                .for_each_slot_in_chunk(ChunkPos { x, z }, |slot| {
                    let body = ctx.entities.body(slot);
                    if !body.alive
                        || body.id == row.id
                        || body.kind
                            != (EntityKind::Mob {
                                type_id: row.type_id,
                            })
                        || !crate::entity_life::adult(ctx.entities.row(slot))
                    {
                        return;
                    }
                    let pos = ctx.entities.pos(slot);
                    let dist = distance(row.pos, pos);
                    if dist <= 8.0
                        && nearest.is_none_or(|(id, _, best)| {
                            dist < best || (dist == best && body.id < id)
                        })
                    {
                        nearest = Some((body.id, pos, dist));
                    }
                });
        }
    }
    nearest.map(|(_, pos, _)| (pos, 3.0))
}
