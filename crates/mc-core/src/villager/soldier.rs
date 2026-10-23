//! Barracks residents use ordinary villager pathfinding and native melee hits.
//! Sensing is local (16 blocks, bounded candidate budget) and never loads chunks.
use super::{Activity, VillagerState};
use crate::boundary::{BlockPos, ChangeSet, EntityEvent, Hand};
use crate::entities::{EntityKind, EntitySlot};
use crate::intent::{Footprint, Intent, IntentKind};
use crate::settlements::army::Order;
use crate::tick::TickState;

const GUARD_ENGAGEMENT_RADIUS: f64 = 12.0;
const MELEE_RANGE_SQUARED: f64 = 4.0;

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}
fn block(p: [f64; 3]) -> BlockPos {
    BlockPos {
        x: p[0].floor() as i32,
        y: p[1].floor() as i32,
        z: p[2].floor() as i32,
    }
}
fn hostile(state: &TickState, slot: EntitySlot) -> bool {
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return false;
    };
    state.entities.body(slot).alive
        && state
            .registry
            .blueprint(type_id)
            .is_some_and(|b| b.capabilities.guard_hostile)
}

/// Order goal is recomputed without random allocation or unbounded scans. A
/// missing/offline follow target falls back to the settlement anchor.
pub(super) fn goal(state: &TickState, slot: EntitySlot, v: &VillagerState) -> Option<[f64; 3]> {
    let military = v.soldier.as_ref()?;
    let record = state.settlements.get(v.settlement?)?;
    let anchor = record.anchor.map(f64::from);
    Some(match &military.order {
        Order::Follow(uuid) => state
            .entities
            .slot_of_uuid(*uuid)
            .filter(|&s| state.players.contains_key(&state.entities.body(s).id))
            .map_or(anchor, |s| state.entities.pos(s)),
        Order::Guard(pos) => [
            f64::from(pos[0]) + 0.5,
            f64::from(pos[1]),
            f64::from(pos[2]) + 0.5,
        ],
        Order::Return => record
            .buildings
            .get(&military.barracks)
            .map_or(anchor, |b| {
                [
                    f64::from(b.bounds.min[0]) + 1.5,
                    f64::from(b.bounds.min[1]) + 1.0,
                    f64::from(b.bounds.min[2]) + 1.5,
                ]
            }),
        Order::Patrol => {
            let radius = f64::from(record.radius.saturating_sub(2).min(10));
            let phase =
                (state.tick / 160 + (state.entities.body(slot).id.0.unsigned_abs() as u64)) % 4;
            let offset = match phase {
                0 => [radius, 0.0],
                1 => [0.0, radius],
                2 => [-radius, 0.0],
                _ => [0.0, -radius],
            };
            [anchor[0] + offset[0], anchor[1], anchor[2] + offset[1]]
        }
    })
}

fn target(
    state: &TickState,
    slot: EntitySlot,
    v: &VillagerState,
    order_goal: [f64; 3],
) -> Option<EntitySlot> {
    let record = state.settlements.get(v.settlement?)?;
    let military = v.soldier.as_ref()?;
    if matches!(military.order, Order::Return) {
        return None;
    }
    let anchor = record.anchor.map(f64::from);
    let radius = f64::from(record.radius);
    let pos = state.entities.pos(slot);
    let tether = match military.order {
        Order::Guard(_) => GUARD_ENGAGEMENT_RADIUS,
        Order::Follow(_) => 8.0,
        _ => radius,
    };
    super::brain::bodies_near(state, pos, 16.0, false)
        .take(256)
        .filter(|&s| hostile(state, s))
        .filter(|&s| {
            let p = state.entities.pos(s);
            distance(p, anchor) <= radius * radius
                && distance(p, order_goal) <= tether * tether
                && distance(p, pos) <= 16.0 * 16.0
        })
        .min_by(|&a, &b| {
            distance(state.entities.pos(a), pos).total_cmp(&distance(state.entities.pos(b), pos))
        })
}

/// The post-commit brain owns navigation only, never raw-slot damage.
pub(crate) fn work(
    state: &mut TickState,
    slot: EntitySlot,
    v: &mut VillagerState,
) -> Option<(BlockPos, bool)> {
    let order_goal = goal(state, slot, v)?;
    v.activity = Activity::Work;
    v.sleeping = false;
    v.panic_until = 0;
    let pos = state.entities.pos(slot);
    if let Some(target) = target(state, slot, v, order_goal) {
        let p = state.entities.pos(target);
        return (distance(pos, p) > MELEE_RANGE_SQUARED)
            .then(|| (block(p), !crate::special_mobs::visible(state, slot, p)));
    }
    let stop = if matches!(v.soldier.as_ref()?.order, Order::Follow(_)) {
        2.5
    } else {
        1.0
    };
    (distance(pos, order_goal) > stop * stop).then(|| (block(order_goal), false))
}

/// Produce normal identity-based combat intents before this tick's merge and
/// sole damage commit. No dense target slot survives into another tick.
pub(crate) fn attacks(state: &mut TickState, intents: &mut Vec<Intent>, changes: &mut ChangeSet) {
    if state.entities.villager_chunks().is_empty() {
        return;
    }
    // Reuse the existing serial-admission scratch, visiting only resident
    // indexes rather than unrelated mobs. These slots cannot compact in this
    // pass: attack only stages intents and advances the resident's cooldown.
    let mut slots = std::mem::take(&mut state.serial_entity_slots);
    slots.clear();
    slots.extend(
        state
            .entities
            .villager_chunks()
            .values()
            .flat_map(|ids| ids.iter())
            .filter_map(|id| state.entities.slot_of(*id))
            .filter(|&slot| {
                crate::settlements::allocator::active(state, slot)
                    && state.entities.row(slot).villager.as_ref().is_some_and(|v| {
                        v.data.profession == super::VillagerProfession::Soldier
                            && v.soldier
                                .as_ref()
                                .is_some_and(|soldier| state.tick >= soldier.next_attack)
                    })
            }),
    );
    slots.sort_unstable_by_key(|&slot| state.entities.body(slot).id);
    for &slot in &slots {
        attack(state, slot, intents, changes);
    }
    state.serial_entity_slots = slots;
}

fn attack(
    state: &mut TickState,
    slot: EntitySlot,
    intents: &mut Vec<Intent>,
    changes: &mut ChangeSet,
) {
    let v = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .expect("admitted soldier");
    let Some(order_goal) = goal(state, slot, v) else {
        return;
    };
    let Some(target) = target(state, slot, v, order_goal) else {
        return;
    };
    let pos = state.entities.pos(slot);
    let target_pos = state.entities.pos(target);
    if distance(pos, target_pos) > MELEE_RANGE_SQUARED
        || !crate::special_mobs::visible(state, slot, target_pos)
    {
        return;
    }
    let empty = mc_data::ItemStack::EMPTY;
    let held = state.entities.row(slot).appearance.equipment[0]
        .as_ref()
        .unwrap_or(&empty);
    let hit = crate::combat::melee_hit(
        &state.registry.item_rules(),
        held,
        crate::combat::Swing {
            ticks_since_reset: 100,
            falling: false,
            on_ground: true,
            sprinting: false,
            crit_blocked: false,
            melee_bonus: 0.0,
            attack_speed_multiplier: 1.0,
        },
    );
    let target_id = state.entities.body(target).id;
    let damage = hit.damage + crate::enchantments::target_damage(state, target_id, held);
    let fire_ticks = crate::combat::fire_aspect_ticks(held);
    let attacker = state.entities.body(slot).id;
    let seq = state.next_seq(attacker);
    intents.push(Intent {
        actor: attacker,
        seq,
        kind: IntentKind::DamageEntity {
            target: target_id,
            amount: damage,
            kind: crate::survival::DamageKind::Melee,
            source: Some(crate::intent::DamageSource::Melee {
                attacker,
                position: pos,
                yaw: state.entities.rot(slot)[0],
                bonus_knockback: hit.bonus_knockback,
                sweep: false,
                fire_ticks,
                bane_ticks: 0,
            }),
        },
        footprint: Footprint {
            entity_reads: vec![attacker, target_id],
            entity_writes: vec![target_id],
            ..Default::default()
        },
    });
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .soldier
        .as_mut()
        .unwrap()
        .next_attack = state.tick.saturating_add(20);
    changes.entity_events.push(EntityEvent::Swing {
        entity: attacker,
        hand: Hand::Main,
    });
}

#[cfg(test)]
#[path = "soldier_tests.rs"]
mod tests;
