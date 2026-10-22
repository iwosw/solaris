//! ItemEntity motion: retain launch momentum through air, fluids, and collision.

use crate::entities::EntitySlot;
use crate::tick::TickState;
use mc_data::ItemStack;
use mc_data::inventory_semantics_26_1_2::can_stack;
use mc_physics::mth;
use mc_physics::{BlockMaterial, BlockSampler, EntityBody, Vec3};

pub(crate) fn step<S: BlockSampler>(mut body: EntityBody, sampler: &S) -> EntityBody {
    let material = sampler.material_at(
        body.position.x.floor() as i32,
        (body.position.y + 0.1).floor() as i32,
        body.position.z.floor() as i32,
    );
    // ItemEntity.tick / setFluidMovement (26.1.2): velocities here are blocks/s.
    match material {
        BlockMaterial::Water | BlockMaterial::Lava => {
            let drag = if material == BlockMaterial::Water {
                0.99
            } else {
                0.95
            };
            body.velocity.x *= drag;
            body.velocity.z *= drag;
            if body.velocity.y < 0.06 * 20.0 {
                body.velocity.y += 0.0005 * 20.0;
            }
        }
        _ => body.velocity.y -= 0.04 * 20.0,
    }
    let requested = Vec3::new(
        body.velocity.x * mc_physics::TICK_SECONDS,
        body.velocity.y * mc_physics::TICK_SECONDS,
        body.velocity.z * mc_physics::TICK_SECONDS,
    );
    let Some(resolved) = mc_physics::resolve_collision_displacement(
        sampler,
        body.position,
        body.aabb,
        requested,
        body.on_ground,
        0.0,
    ) else {
        body.velocity = Vec3::ZERO;
        return body;
    };
    body.position.x += resolved.delta.x;
    body.position.y += resolved.delta.y;
    body.position.z += resolved.delta.z;
    body.on_ground = resolved.grounded;
    let horizontal_drag = if body.on_ground { 0.6 * 0.98 } else { 0.98 };
    body.velocity.x = if (resolved.delta.x - requested.x).abs() > 1e-7 {
        0.0
    } else {
        body.velocity.x * horizontal_drag
    };
    body.velocity.y = if (resolved.delta.y - requested.y).abs() > 1e-7 {
        0.0
    } else {
        body.velocity.y * 0.98
    };
    body.velocity.z = if (resolved.delta.z - requested.z).abs() > 1e-7 {
        0.0
    } else {
        body.velocity.z * horizontal_drag
    };
    body
}

/// Vanilla `ItemEntity.mergeWithNeighbours`: every twentieth entity tick,
/// merge compatible item boxes expanded by `.5, 0, .5`.
pub(crate) fn merge_neighbours(state: &mut TickState) {
    if !state.tick.is_multiple_of(20) {
        return;
    }
    let mut slots = std::mem::take(&mut state.serial_entity_slots);
    slots.clear();
    slots.extend(state.entities.item_slots().filter(|slot| {
        let body = state.entities.body(*slot);
        body.alive
            && state
                .entities
                .row(*slot)
                .item
                .as_ref()
                .is_some_and(|item| item.count > 0)
    }));
    slots.sort_unstable_by_key(|slot| item_cell(state, *slot));

    for receiver in slots.iter().copied() {
        if !state.entities.body(receiver).alive {
            continue;
        }
        let position = state.entities.pos(receiver);
        let receiver_id = state.entities.body(receiver).id;
        let cell = item_cell(state, receiver);
        let mut receiver_changed = false;
        for y in cell.1 - 1..=cell.1 + 1 {
            for z in cell.2 - 1..=cell.2 + 1 {
                for x in cell.0 - 1..=cell.0 + 1 {
                    let candidate_cell = (x, y, z);
                    let start =
                        slots.partition_point(|slot| item_cell(state, *slot) < candidate_cell);
                    let end =
                        slots.partition_point(|slot| item_cell(state, *slot) <= candidate_cell);
                    for source in slots[start..end].iter().copied() {
                        if source.0 <= receiver.0 || !state.entities.body(source).alive {
                            continue;
                        }
                        if !merge_boxes_overlap(
                            position,
                            state.entities.pos(source),
                            state.entities.body(receiver).half_width,
                            state.entities.body(receiver).height,
                            state.entities.body(source).half_width,
                            state.entities.body(source).height,
                        ) {
                            continue;
                        }
                        let Some((moved, pickup_delay, age, source_id)) =
                            merge_amount(state, receiver, source)
                        else {
                            continue;
                        };
                        {
                            let row = state.entities.row_mut(receiver);
                            let item = row.item.as_mut().unwrap();
                            item.count += moved;
                            item.stack.as_mut().unwrap().count += moved;
                            row.age = age;
                        }
                        let source_empty = {
                            let item = state.entities.row_mut(source).item.as_mut().unwrap();
                            item.count -= moved;
                            item.stack.as_mut().unwrap().count = item.count;
                            if item.count == 0 {
                                item.stack = Some(ItemStack::EMPTY);
                            }
                            item.count == 0
                        };
                        state.entities.body_mut(receiver).pickup_delay = pickup_delay;
                        receiver_changed = true;
                        if source_empty {
                            state.entities.body_mut(source).alive = false;
                            state.merge_consumed_items.insert(source);
                        } else {
                            let row = state.entities.row_mut(source);
                            std::sync::Arc::make_mut(&mut row.appearance).item =
                                row.item.as_ref().map(|item| item.full_stack());
                            state.appearance_changed.push(source_id);
                        }
                    }
                }
            }
        }
        if receiver_changed {
            let row = state.entities.row_mut(receiver);
            std::sync::Arc::make_mut(&mut row.appearance).item =
                row.item.as_ref().map(|item| item.full_stack());
            state.appearance_changed.push(receiver_id);
        }
    }
    slots.clear();
    state.serial_entity_slots = slots;
}

fn item_cell(state: &TickState, slot: EntitySlot) -> (i32, i32, i32) {
    let pos = state.entities.pos(slot);
    (
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    )
}

fn merge_boxes_overlap(
    a: [f64; 3],
    b: [f64; 3],
    a_half_width: f64,
    a_height: f64,
    b_half_width: f64,
    b_height: f64,
) -> bool {
    (a[0] - b[0]).abs() < a_half_width + b_half_width + 0.5
        && (a[2] - b[2]).abs() < a_half_width + b_half_width + 0.5
        && a[1] < b[1] + b_height
        && b[1] < a[1] + a_height
}

fn merge_amount(
    state: &TickState,
    receiver: EntitySlot,
    source: EntitySlot,
) -> Option<(i32, u32, u32, crate::boundary::EntityId)> {
    let receiver_body = state.entities.body(receiver);
    let source_body = state.entities.body(source);
    let receiver_item = state.entities.row(receiver).item.as_ref()?;
    let source_item = state.entities.row(source).item.as_ref()?;
    let receiver_stack = receiver_item.stack.as_ref()?;
    let source_stack = source_item.stack.as_ref()?;
    if receiver_item.count <= 0
        || source_item.count <= 0
        || !can_stack(receiver_stack, source_stack)
    {
        return None;
    }
    let moved = (state.registry.item_rules().max_stack(receiver_stack) - receiver_item.count)
        .min(source_item.count);
    (moved > 0).then_some((
        moved,
        receiver_body.pickup_delay.max(source_body.pickup_delay),
        state
            .entities
            .row(receiver)
            .age
            .min(state.entities.row(source).age),
        source_body.id,
    ))
}

pub(crate) fn throw_selected(
    state: &mut crate::tick::TickState,
    actor: crate::boundary::EntityId,
    drops: &[crate::boundary::EntityId],
) {
    let Some(actor_slot) = state.entities.slot_of(actor) else {
        return;
    };
    let mut pos = state.entities.pos(actor_slot);
    let rotation = state.entities.rot(actor_slot);
    let eye = crate::tick::player_eye_height(state.entities.row(actor_slot).appearance.pose);
    pos[1] += eye - 0.3;
    let yaw = f64::from(rotation[0]).to_radians();
    let pitch = f64::from(rotation[1]).to_radians();
    let velocity = [
        -f64::from(mth::sin(yaw)) * f64::from(mth::cos(pitch)) * 6.0,
        -f64::from(mth::sin(pitch)) * 6.0 + 2.0,
        f64::from(mth::cos(yaw)) * f64::from(mth::cos(pitch)) * 6.0,
    ];
    for id in drops {
        if let Some(slot) = state.entities.slot_of(*id) {
            state.entities.set_pos(slot, pos);
            state.entities.set_vel(slot, velocity);
            state.entities.body_mut(slot).pickup_delay = 40;
        }
    }
}
