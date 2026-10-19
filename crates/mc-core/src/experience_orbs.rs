//! Vanilla experience orb lifecycle, denominations, collection and Mending.

use std::sync::Arc;

use crate::boundary::{ChangeSet, EntityAppearance, EntityEvent, EntityId};
use crate::entities::{ColdFields, EntityKind};
use crate::tick::{TickSampler, TickState};

/// Vanilla `ExperienceOrb.getExperienceValue`.
#[must_use]
pub fn experience_value(amount: i32) -> i32 {
    [2477, 1237, 617, 307, 149, 73, 37, 17, 7, 3, 1]
        .into_iter()
        .find(|value| amount >= *value)
        .unwrap_or(1)
}

fn random(seed: u64) -> u64 {
    let mut value = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// Spawn denominations; the vanilla randomized 40-group merge bounds orb density.
pub(crate) fn award(
    state: &mut TickState,
    pos: [f64; 3],
    mut amount: i32,
    spawned: &mut Vec<EntityId>,
) {
    let Some(type_id) = state.registry.entity_types.id_of(
        &mc_data::Identifier::parse("minecraft:experience_orb").expect("built-in identifier"),
    ) else {
        return;
    };
    while amount > 0 {
        let value = experience_value(amount);
        amount -= value;
        let seed =
            random(state.tick ^ (state.entities.len() as u64).rotate_left(32) ^ amount as u64);
        let group = (seed % 40) as i32;
        let existing = state
            .entities
            .experience_orbs()
            .filter_map(|id| state.entities.slot_of(id))
            .find(|slot| {
                let body = state.entities.body(*slot);
                let row = state.entities.row(*slot);
                row.appearance
                    .experience_orb
                    .is_some_and(|(v, _)| v == value)
                    && (body.id.0 - group) % 40 == 0
                    && state
                        .entities
                        .pos(*slot)
                        .into_iter()
                        .zip(pos)
                        .all(|(a, b)| (a - b).abs() <= 0.75)
            });
        if let Some(slot) = existing {
            let (body, row) = state.entities.parts_mut(slot);
            let data = Arc::make_mut(&mut row.appearance)
                .experience_orb
                .as_mut()
                .expect("orb");
            data.1 = data.1.saturating_add(1);
            row.age = 0;
            state.appearance_changed.push(body.id);
            continue;
        }
        let id = state.entities.alloc_id();
        let unit = |salt| random(seed ^ salt) as f64 / u64::MAX as f64;
        state.entities.spawn(
            id,
            uuid::Uuid::new_v4(),
            EntityKind::ExperienceOrb { type_id },
            ColdFields {
                pos,
                // Core velocities are blocks/second; vanilla uses blocks/tick.
                vel: [
                    (unit(1) * 0.4 - 0.2) * 20.0,
                    unit(2) * 8.0,
                    (unit(3) * 0.4 - 0.2) * 20.0,
                ],
                yaw: (unit(4) * 360.0) as f32,
                pitch: 0.0,
                ground: false,
                health: 5.0,
                breed_cooldown: 0,
                pickup_delay: 0,
                item: None,
                appearance: Arc::new(EntityAppearance {
                    experience_orb: Some((value, 1)),
                    health: 5.0,
                    ..Default::default()
                }),
                movement: crate::blueprint::MovementClass::Item,
                half_width: 0.25,
                height: 0.5,
                speed: 0.0,
                attack_damage: 0.0,
                sight_range: 0.0,
                attack_reach: 0.0,
            },
        );
        spawned.push(id);
    }
}

fn distance_sq(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

fn merge_cell(pos: [f64; 3]) -> [i32; 3] {
    pos.map(|component| (component / 4.0).floor() as i32)
}

/// Player-attributed mob XP. Animals roll 1–3; baby animals drop none.
#[must_use]
pub(crate) fn mob_experience(name: &str, baby: bool, seed: u64) -> i32 {
    match name.strip_prefix("minecraft:").unwrap_or(name) {
        "cow" | "mooshroom" | "pig" | "sheep" | "chicken" | "rabbit" | "horse" | "donkey"
        | "mule" | "llama" | "trader_llama" | "wolf" | "cat" | "ocelot" | "fox" | "panda"
        | "polar_bear" | "goat" | "camel" | "armadillo" | "bee" | "turtle" | "axolotl" | "frog"
        | "sniffer" | "dolphin" | "nautilus" => {
            if baby {
                0
            } else {
                1 + (random(seed) % 3) as i32
            }
        }
        "cod" | "salmon" | "pufferfish" | "tropical_fish" => 1 + (random(seed) % 3) as i32,
        "blaze" | "breeze" | "elder_guardian" | "guardian" => 10,
        "ravager" | "piglin_brute" => 20,
        "wither" => 50,
        "ender_dragon" => 12000,
        "slime" | "magma_cube" => 1,
        "zombie" | "husk" | "drowned" | "zombie_villager" | "zombified_piglin" if baby => 12,
        "zombie" | "husk" | "drowned" | "zombie_villager" | "zombified_piglin" | "skeleton"
        | "stray" | "bogged" | "wither_skeleton" | "creeper" | "spider" | "cave_spider"
        | "enderman" | "endermite" | "silverfish" | "witch" | "ghast" | "phantom" | "shulker"
        | "pillager" | "vindicator" | "evoker" | "vex" | "piglin" | "hoglin" | "zoglin"
        | "warden" | "parched" | "camel_husk" | "zombie_nautilus" => 5,
        _ => 0,
    }
}

/// Mending selects a damaged equipped item, repairs two durability per XP,
/// then repeats with the remainder before crediting experience.
fn mend(player: &mut crate::player::PlayerState, mut amount: i32, mut seed: u64) -> (i32, bool) {
    let slots = [
        usize::from(player.inventory.selected.min(8)),
        crate::inventory::OFF_HAND,
        36,
        37,
        38,
        39,
    ];
    let mut changed = false;
    while amount > 0 {
        let mut candidates = [0; 6];
        let mut count = 0;
        for slot in slots {
            let stack = &player.inventory.slots[slot];
            if !stack.is_empty()
                && stack.damage.is_some_and(|damage| damage > 0)
                && stack.enchantments.iter().any(|enchantment| {
                    enchantment.id.as_str() == "minecraft:mending" && enchantment.level > 0
                })
            {
                candidates[count] = slot;
                count += 1;
            }
        }
        if count == 0 {
            break;
        }
        seed = random(seed);
        let stack = &mut player.inventory.slots[candidates[(seed % count as u64) as usize]];
        let damage = stack.damage.unwrap_or(0);
        let repaired = damage.min(amount.saturating_mul(2));
        stack.damage = Some(damage - repaired);
        changed = true;
        amount -= repaired / 2;
    }
    (amount, changed)
}

/// Runs after entity merge: physics, periodic local merges, expiry and collection.
pub(crate) fn run(state: &mut TickState, changes: &mut ChangeSet, events: &mut Vec<EntityEvent>) {
    if state.entities.experience_orbs().next().is_none() {
        return;
    }
    let spawned: std::collections::HashSet<_> = changes.entities_spawned.iter().copied().collect();
    let orbs: Vec<_> = state
        .entities
        .experience_orbs()
        .filter(|id| !spawned.contains(id))
        .collect();
    if orbs.is_empty() {
        return;
    }
    let mut players: Vec<_> = state
        .players
        .iter()
        .filter_map(|(id, player)| {
            if player.spectator() || state.entity_session_locked(*id) {
                return None;
            }
            let slot = state.entities.slot_of(*id)?;
            Some((*id, state.entities.pos(slot)))
        })
        .collect();
    players.sort_unstable_by_key(|(id, _)| *id);
    let mut merge_cells =
        std::collections::HashMap::<(i32, i32, i32, i32, i32), Vec<EntityId>>::new();
    if state.tick.is_multiple_of(20) {
        for id in &orbs {
            let slot = state.entities.slot_of(*id).expect("orb index");
            let pos = state.entities.pos(slot);
            if !state.world.is_active(
                (pos[0].floor() as i32).div_euclid(16),
                (pos[2].floor() as i32).div_euclid(16),
            ) {
                continue;
            }
            let value = state
                .entities
                .row(slot)
                .appearance
                .experience_orb
                .expect("orb")
                .0;
            let [x, y, z] = merge_cell(pos);
            merge_cells
                .entry((value, id.0.rem_euclid(40), x, y, z))
                .or_default()
                .push(*id);
        }
    }
    for id in orbs {
        let Some(slot) = state.entities.slot_of(id) else {
            continue;
        };
        let pos = state.entities.pos(slot);
        if !state.world.is_active(
            (pos[0].floor() as i32).div_euclid(16),
            (pos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let mut velocity = state.entities.vel(slot).map(|component| component / 20.0);
        velocity[1] -= 0.03;
        let nearest = players
            .iter()
            .filter(|(_, at)| distance_sq(*at, pos) <= 64.0)
            .min_by(|a, b| distance_sq(a.1, pos).total_cmp(&distance_sq(b.1, pos)));
        if let Some((_, target)) = nearest {
            let delta = [
                target[0] - pos[0],
                target[1] + 0.81 - pos[1],
                target[2] - pos[2],
            ];
            let distance = distance_sq(delta, [0.0; 3]).sqrt();
            if distance > 0.0 {
                let power = (1.0 - distance / 8.0).max(0.0).powi(2) * 0.1;
                for axis in 0..3 {
                    velocity[axis] += delta[axis] / distance * power;
                }
            }
        }
        let sampler = TickSampler {
            world: &state.world,
            registry: &state.registry,
        };
        if let Some(resolved) = mc_physics::resolve_collision_displacement(
            &sampler,
            mc_physics::Vec3::new(pos[0], pos[1], pos[2]),
            mc_physics::Aabb {
                half_width: 0.25,
                height: 0.5,
            },
            mc_physics::Vec3::new(velocity[0], velocity[1], velocity[2]),
            state.entities.ground(slot),
            0.0,
        ) {
            state.entities.set_pos(
                slot,
                [
                    pos[0] + resolved.delta.x,
                    pos[1] + resolved.delta.y,
                    pos[2] + resolved.delta.z,
                ],
            );
            if (resolved.delta.x - velocity[0]).abs() > 1e-8 {
                velocity[0] = 0.0;
            }
            if (resolved.delta.z - velocity[2]).abs() > 1e-8 {
                velocity[2] = 0.0;
            }
            if resolved.grounded && velocity[1] < -0.03 {
                velocity[1] *= -0.4;
            }
            let friction = if resolved.grounded { 0.6 * 0.98 } else { 0.98 };
            state
                .entities
                .set_vel(slot, velocity.map(|component| component * friction * 20.0));
            state.entities.set_ground(slot, resolved.grounded);
        }
        if state.entities.row(slot).age >= 6000 {
            changes.entities_removed.push(id);
            state.remove_entity(slot);
            continue;
        }
        if state.tick.is_multiple_of(20) {
            let pos = state.entities.pos(slot);
            let value = state
                .entities
                .row(slot)
                .appearance
                .experience_orb
                .expect("orb")
                .0;
            let [x, y, z] = merge_cell(pos);
            let mut nearby = Vec::new();
            for dx in -1..=1 {
                for dy in -1..=1 {
                    for dz in -1..=1 {
                        let Some(candidates) =
                            merge_cells.get(&(value, id.0.rem_euclid(40), x + dx, y + dy, z + dz))
                        else {
                            continue;
                        };
                        for other in candidates {
                            if *other == id {
                                continue;
                            }
                            let Some(other_slot) = state.entities.slot_of(*other) else {
                                continue;
                            };
                            if state
                                .entities
                                .pos(other_slot)
                                .into_iter()
                                .zip(pos)
                                .all(|(a, b)| (a - b).abs() <= 1.0)
                            {
                                nearby.push(*other);
                            }
                        }
                    }
                }
            }
            for other in nearby {
                let other_slot = state.entities.slot_of(other).expect("merge target");
                let count = state
                    .entities
                    .row(other_slot)
                    .appearance
                    .experience_orb
                    .expect("orb")
                    .1;
                let age = state.entities.row(other_slot).age;
                state.remove_entity(other_slot);
                changes.entities_removed.push(other);
                let slot = state.entities.slot_of(id).expect("merge owner");
                let row = state.entities.row_mut(slot);
                row.age = row.age.min(age);
                let orb = Arc::make_mut(&mut row.appearance)
                    .experience_orb
                    .as_mut()
                    .expect("orb");
                orb.1 = orb.1.saturating_add(count);
                state.appearance_changed.push(id);
            }
        }
        let slot = state.entities.slot_of(id).expect("orb owner");
        let pos = state.entities.pos(slot);
        let collector = players.iter().find(|(player, at)| {
            (at[0] - pos[0]).abs() <= 0.8
                && (at[2] - pos[2]).abs() <= 0.8
                && pos[1] >= at[1] - 0.5
                && pos[1] <= at[1] + 2.0
                && state
                    .entities
                    .slot_of(*player)
                    .is_some_and(|slot| state.entities.body(slot).pickup_delay == 0)
        });
        let Some((collector, _)) = collector else {
            continue;
        };
        let collector = *collector;
        let (value, count) = state
            .entities
            .row(slot)
            .appearance
            .experience_orb
            .expect("orb");
        let player = state.players.get_mut(&collector).expect("player");
        let (remaining, repaired) = mend(player, value, state.tick ^ id.0 as u64);
        crate::commands::add_experience(&mut player.vitals.experience, remaining, false);
        let connection = player.connection;
        let xp = player.experience_change();
        let inventory = repaired.then(|| player.inventory_contents());
        state.push_player(connection, xp);
        if let Some(inventory) = inventory {
            state.push_player(connection, inventory);
        }
        let collector_slot = state.entities.slot_of(collector).expect("collector");
        state.entities.body_mut(collector_slot).pickup_delay = 2;
        events.push(EntityEvent::Pickup {
            item: id,
            collector,
            count: 1,
        });
        if count == 1 {
            changes.entities_removed.push(id);
            state.remove_entity(slot);
        } else {
            Arc::make_mut(&mut state.entities.row_mut(slot).appearance).experience_orb =
                Some((value, count - 1));
            state.appearance_changed.push(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::experience_value;

    #[test]
    fn vanilla_experience_denominations_and_boundaries() {
        for (amount, expected) in [
            (1, 1),
            (2, 1),
            (3, 3),
            (5, 3),
            (7, 7),
            (16, 7),
            (17, 17),
            (37, 37),
            (73, 73),
            (149, 149),
            (307, 307),
            (617, 617),
            (1237, 1237),
            (2477, 2477),
            (12000, 2477),
        ] {
            assert_eq!(experience_value(amount), expected);
        }
    }
}
