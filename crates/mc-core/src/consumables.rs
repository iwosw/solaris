//! Item cooldowns and world-aware chorus fruit completion.

use crate::boundary::{EntityId, PlayerChange};
use crate::inventory::ItemRules;
use crate::player::PlayerState;
use crate::tick::TickState;

impl PlayerState {
    #[must_use]
    pub fn item_on_cooldown(&self, item_id: u32) -> bool {
        self.item_cooldowns
            .get(&item_id)
            .is_some_and(|ticks| *ticks > 0)
    }

    /// Start only after the corresponding action commits its inventory debit.
    pub fn start_item_cooldown(
        &mut self,
        rules: &ItemRules<'_>,
        item_id: u32,
        ticks: u32,
    ) -> Option<PlayerChange> {
        let group = rules.items.name_of(item_id)?.as_str().to_owned();
        if ticks == 0 {
            self.item_cooldowns.remove(&item_id);
        } else {
            self.item_cooldowns.insert(item_id, ticks);
        }
        Some(PlayerChange::ItemCooldown {
            group,
            ticks: i32::try_from(ticks).unwrap_or(i32::MAX),
        })
    }
}

fn splitmix(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e3779b97f4a7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d049bb133111eb);
    value ^ (value >> 31)
}

/// Sixteen vanilla-style attempts, never treating an unloaded cell as air.
/// The native collision table rejects obstruction across the complete body.
pub(crate) fn chorus_destination(
    state: &TickState,
    position: [f64; 3],
    seed: u64,
) -> Option<[f64; 3]> {
    let shapes = mc_data::collision_shapes::vanilla_collision_shapes();
    for attempt in 0..16_u64 {
        let a = splitmix(seed ^ attempt.wrapping_mul(0x9e3779b97f4a7c15));
        let b = splitmix(a);
        let c = splitmix(b);
        let unit = |value: u64| (value >> 11) as f64 / ((1_u64 << 53) as f64);
        let x = position[0] + (unit(a) - 0.5) * 16.0;
        let z = position[2] + (unit(b) - 0.5) * 16.0;
        let mut y = (position[1].floor() as i32 + (c % 16) as i32 - 8)
            .min(state.dimension.geometry().max_y() - 2);
        let mut supported = false;
        while y > state.dimension.geometry().min_y() {
            let Some(id) = state
                .world
                .block_at(x.floor() as i32, y - 1, z.floor() as i32)
            else {
                break;
            };
            let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
                break;
            };
            if !block.has_fluid() && shapes.get(id).is_some_and(|shape| !shape.is_empty()) {
                supported = true;
                break;
            }
            y -= 1;
        }
        if !supported {
            continue;
        }
        let destination = [x, f64::from(y), z];
        if clear_body(state, destination) {
            return Some(destination);
        }
    }
    None
}

fn clear_body(state: &TickState, position: [f64; 3]) -> bool {
    let [x, y, z] = position;
    let shapes = mc_data::collision_shapes::vanilla_collision_shapes();
    for bx in (x - 0.3).floor() as i32..=(x + 0.3).floor() as i32 {
        for by in y.floor() as i32..=(y + 1.8 - 1.0e-6).floor() as i32 {
            for bz in (z - 0.3).floor() as i32..=(z + 0.3).floor() as i32 {
                let Some(id) = state.world.block_at(bx, by, bz) else {
                    return false;
                };
                let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
                    return false;
                };
                if block.has_fluid() {
                    return false;
                }
                if let Some(shape) = shapes.get(id) {
                    for bounds in shape.iter() {
                        let [min_x, min_y, min_z, max_x, max_y, max_z] = bounds.as_blocks();
                        if x + 0.3 > f64::from(bx) + min_x
                            && x - 0.3 < f64::from(bx) + max_x
                            && y + 1.8 > f64::from(by) + min_y
                            && y < f64::from(by) + max_y
                            && z + 0.3 > f64::from(bz) + min_z
                            && z - 0.3 < f64::from(bz) + max_z
                        {
                            return false;
                        }
                    }
                }
            }
        }
    }
    true
}

pub(crate) fn finish_chorus(state: &mut TickState, entity: EntityId) {
    let Some(slot) = state.entities.slot_of(entity) else {
        return;
    };
    let seed = state.tick ^ u64::from(entity.0 as u32).wrapping_mul(0x9e3779b97f4a7c15);
    let Some(position) = chorus_destination(state, state.entities.pos(slot), seed) else {
        return;
    };
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    player.fall_distance = 0.0;
    player.release_use_item();
    let connection = player.connection;
    let [yaw, pitch, _] = state.entities.rot(slot);
    crate::mounts::dismount(state, entity);
    state.entities.set_pos(slot, position);
    state.entities.set_vel(slot, [0.0; 3]);
    state.correct_position(connection, position, yaw, pitch);
}
