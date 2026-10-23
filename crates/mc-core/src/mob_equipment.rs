//! Spawn-finalized vanilla held items and regional-difficulty equipment rolls.
use crate::entities::EntitySlot;
use crate::natural_spawn::Rng;
use crate::survival::Difficulty;
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use std::sync::Arc;

fn stack(state: &TickState, name: &str) -> Option<ItemStack> {
    state
        .registry
        .items
        .id_of(&Identifier::parse(name).expect("equipment identifier"))
        .map(|id| ItemStack::new(id, 1))
}

/// Vanilla DifficultyInstance special multiplier (the 0..1 equipment factor).
/// InhabitedTime is read from the retained chunk, including imported worlds.
fn difficulty_factor(difficulty: Difficulty, game_time: u64, day_time: i64, inhabited: i64) -> f64 {
    if difficulty == Difficulty::Peaceful {
        return 0.0;
    }
    let day = ((game_time as f64 - 72_000.0) / 1_440_000.0).clamp(0.0, 1.0) * 0.25;
    let moon = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75]
        [day_time.div_euclid(24_000).rem_euclid(8) as usize];
    let mut local = (inhabited as f64 / 3_600_000.0).clamp(0.0, 1.0)
        * if difficulty == Difficulty::Hard {
            1.0
        } else {
            0.75
        };
    local += (moon * 0.25_f64).clamp(0.0, day);
    if difficulty == Difficulty::Easy {
        local *= 0.5;
    }
    let effective = f64::from(difficulty.id()) * (0.75 + day + local);
    ((effective - 2.0) / 2.0).clamp(0.0, 1.0)
}

fn factor(state: &TickState, slot: EntitySlot) -> f64 {
    let pos = state.entities.pos(slot);
    let chunk = mc_world::ChunkPos {
        x: (pos[0].floor() as i32).div_euclid(16),
        z: (pos[2].floor() as i32).div_euclid(16),
    };
    difficulty_factor(
        state.difficulty,
        state.tick,
        state.day_time,
        state.world.inhabited_time(chunk),
    )
}

fn rng(state: &TickState, slot: EntitySlot) -> Rng {
    Rng(state.world.seed() as u64
        ^ state.tick.rotate_left(23)
        ^ (state.entities.body(slot).id.0 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
}

/// Returns the armor tier and number of consecutive slots, helmet first.
fn armor_roll(difficulty: Difficulty, factor: f64, rng: &mut Rng) -> Option<(usize, usize)> {
    if rng.unit() >= 0.15 * factor {
        return None;
    }
    let mut tier = rng.below(3) as usize;
    for _ in 0..3 {
        if rng.unit() < 0.1087 {
            tier += 1;
        }
    }
    let mut slots = 1;
    while slots < 4
        && rng.unit()
            >= if difficulty == Difficulty::Hard {
                0.1
            } else {
                0.25
            }
    {
        slots += 1;
    }
    Some((tier, slots))
}

fn enchant(state: &TickState, stack: &mut ItemStack, factor: f64, chance: f64, rng: &mut Rng) {
    if rng.unit() < chance * factor {
        let level = 5 + (factor * rng.below(18) as f64) as i32;
        mc_data::stations::random_loot_enchantments(
            &state.registry.items,
            stack,
            level,
            rng.next(),
        );
    }
}

pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot, type_id: u32) {
    let registry = std::sync::Arc::clone(&state.registry);
    use crate::blueprint::EquipmentLoadout;
    let Some(loadout) = registry
        .blueprint(type_id)
        .map(|b| b.capabilities.equipment)
    else {
        return;
    };
    let skeleton = loadout == EquipmentLoadout::BowArmor;
    let zombie = loadout == EquipmentLoadout::RareToolArmor;
    if loadout == EquipmentLoadout::None {
        return;
    }
    let factor = factor(state, slot);
    let mut rng = rng(state, slot);
    let held = match loadout {
        EquipmentLoadout::BowArmor => Some("minecraft:bow"),
        EquipmentLoadout::StoneSword => Some("minecraft:stone_sword"),
        EquipmentLoadout::GoldSwordOrCrossbow => Some(if rng.below(2) == 0 {
            "minecraft:golden_sword"
        } else {
            "minecraft:crossbow"
        }),
        EquipmentLoadout::GoldSword => Some("minecraft:golden_sword"),
        EquipmentLoadout::GoldAxe => Some("minecraft:golden_axe"),
        EquipmentLoadout::IronAxe => Some("minecraft:iron_axe"),
        EquipmentLoadout::Crossbow => Some("minecraft:crossbow"),
        EquipmentLoadout::BoundIronSword => Some("minecraft:iron_sword"),
        _ if zombie
            && rng.unit()
                < if state.difficulty == Difficulty::Hard {
                    0.05
                } else {
                    0.01
                } =>
        {
            Some(if rng.below(3) == 0 {
                "minecraft:iron_sword"
            } else {
                "minecraft:iron_shovel"
            })
        }
        _ => None,
    };
    let mut equipment: [Option<ItemStack>; 6] = std::array::from_fn(|_| None);
    equipment[0] = held.and_then(|name| stack(state, name));
    if skeleton || zombie {
        if let Some((tier, count)) = armor_roll(state.difficulty, factor, &mut rng) {
            const ARMOR: [[&str; 4]; 6] = [
                [
                    "minecraft:leather_helmet",
                    "minecraft:leather_chestplate",
                    "minecraft:leather_leggings",
                    "minecraft:leather_boots",
                ],
                [
                    "minecraft:copper_helmet",
                    "minecraft:copper_chestplate",
                    "minecraft:copper_leggings",
                    "minecraft:copper_boots",
                ],
                [
                    "minecraft:golden_helmet",
                    "minecraft:golden_chestplate",
                    "minecraft:golden_leggings",
                    "minecraft:golden_boots",
                ],
                [
                    "minecraft:chainmail_helmet",
                    "minecraft:chainmail_chestplate",
                    "minecraft:chainmail_leggings",
                    "minecraft:chainmail_boots",
                ],
                [
                    "minecraft:iron_helmet",
                    "minecraft:iron_chestplate",
                    "minecraft:iron_leggings",
                    "minecraft:iron_boots",
                ],
                [
                    "minecraft:diamond_helmet",
                    "minecraft:diamond_chestplate",
                    "minecraft:diamond_leggings",
                    "minecraft:diamond_boots",
                ],
            ];
            for (index, name) in ARMOR[tier].iter().take(count).enumerate() {
                equipment[5 - index] = stack(state, name);
            }
        }
        if let Some(held) = equipment[0].as_mut() {
            enchant(state, held, factor, 0.25, &mut rng);
        }
        for armor in equipment[2..].iter_mut().flatten() {
            enchant(state, armor, factor, 0.5, &mut rng);
        }
    }
    let shell = loadout == EquipmentLoadout::Aquatic && rng.unit() < 0.03;
    if shell {
        equipment[1] = stack(state, "minecraft:nautilus_shell");
    }
    let pickup = loadout == EquipmentLoadout::GoldSwordOrCrossbow
        || ((skeleton
            || zombie
            || matches!(
                loadout,
                EquipmentLoadout::Aquatic
                    | EquipmentLoadout::GoldSword
                    | EquipmentLoadout::StoneSword
            ))
            && rng.unit() < 0.55 * factor);
    let row = state.entities.row_mut(slot);
    row.loot.can_pick_up_loot = pickup;
    let appearance = Arc::make_mut(&mut row.appearance);
    for (index, item) in equipment.into_iter().enumerate() {
        if item.is_some() {
            appearance.equipment[index] = item;
        }
    }
    if shell {
        row.loot.drop_chances[1] = 2.0;
    }
    if loadout == EquipmentLoadout::BoundIronSword {
        row.loot.drop_chances[0] = 0.0;
    }
    crate::entity_loot::restore(state, slot);
}

/// Drowned weapons are exclusive to natural spawns, not eggs/commands/conversions.
pub(crate) fn natural(state: &mut TickState, slot: EntitySlot) {
    let crate::entities::EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return;
    };
    if state
        .registry
        .blueprint(type_id)
        .is_none_or(|b| b.capabilities.equipment != crate::blueprint::EquipmentLoadout::Aquatic)
    {
        return;
    }
    let mut rng = rng(state, slot);
    // Separate the weapon draw from the independent shell/pickup draws.
    rng.0 ^= 0xD20A_4E0D_1A63_5EED;
    let roll = rng.unit();
    let name = if roll < 0.0625 {
        "minecraft:trident"
    } else if roll < 0.1 {
        "minecraft:fishing_rod"
    } else {
        return;
    };
    let item = stack(state, name);
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] = item;
    crate::entity_loot::restore(state, slot);
}

#[cfg(test)]
#[path = "mob_equipment_tests.rs"]
mod tests;
