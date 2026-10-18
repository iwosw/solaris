//! Gameplay hooks for equipment enchantments; item components remain the authority.
use mc_data::ItemStack;

pub(crate) fn level(stack: &ItemStack, name: &str) -> u32 {
    stack
        .enchantments
        .iter()
        .filter(|entry| entry.id.namespace() == "minecraft" && entry.id.path() == name)
        .map(|entry| entry.level.max(0) as u32)
        .max()
        .unwrap_or(0)
}

/// The player's already-retained XpSeed supplies deterministic, restart-safe draws.
pub(crate) fn random(seed: &mut i32) -> f64 {
    *seed = crate::inventory::enchanting::next_seed(*seed);
    f64::from((*seed as u32) >> 8) / 16_777_216.0
}

/// Resolve each damage point independently. Armor retains the vanilla 60% floor.
pub(crate) fn durability_damage(
    stack: &ItemStack,
    amount: i32,
    armor: bool,
    seed: &mut i32,
) -> i32 {
    let level = level(stack, "unbreaking");
    if level == 0 || amount <= 0 {
        return amount.max(0);
    }
    let skip = f64::from(level) / (f64::from(level) + 1.0) * if armor { 0.4 } else { 1.0 };
    (0..amount).filter(|_| random(seed) >= skip).count() as i32
}

pub(crate) fn consumes_air(head: &ItemStack, seed: &mut i32) -> bool {
    let level = level(head, "respiration");
    level == 0 || random(seed) >= f64::from(level) / (f64::from(level) + 1.0)
}

pub(crate) fn equipment_attributes(
    equipment: &[ItemStack; 6],
    soul_active: bool,
    attributes: &mut [crate::boundary::PlayerAttribute],
) {
    for (slot, name, kind, amount, operation) in [
        (
            2,
            "depth_strider",
            "water_movement_efficiency",
            f64::from(1.0_f32 / 3.0),
            "add_value",
        ),
        (
            3,
            "swift_sneak",
            "sneaking_speed",
            f64::from(0.15_f32),
            "add_value",
        ),
        (5, "respiration", "oxygen_bonus", 1.0, "add_value"),
        (
            5,
            "aqua_affinity",
            "submerged_mining_speed",
            4.0,
            "add_multiplied_total",
        ),
    ] {
        let level = level(&equipment[slot], name);
        if level == 0 || equipment[slot].is_empty() {
            continue;
        }
        if let Some(attribute) = attributes.iter_mut().find(|entry| entry.kind == kind) {
            attribute
                .modifiers
                .push(mc_data::item_components::ItemAttributeModifier {
                    kind: format!("minecraft:{kind}"),
                    id: format!("minecraft:enchantment.{name}"),
                    amount: amount * f64::from(level),
                    operation: operation.into(),
                    slot: None,
                });
        }
    }
    let soul = level(&equipment[2], "soul_speed");
    if soul_active && soul > 0 {
        for (kind, amount) in [
            ("movement_speed", 0.0405 + 0.0105 * f64::from(soul - 1)),
            ("movement_efficiency", 1.0),
        ] {
            if let Some(attribute) = attributes.iter_mut().find(|entry| entry.kind == kind) {
                attribute
                    .modifiers
                    .push(mc_data::item_components::ItemAttributeModifier {
                        kind: format!("minecraft:{kind}"),
                        id: "minecraft:enchantment.soul_speed".into(),
                        amount,
                        operation: "add_value".into(),
                        slot: None,
                    });
            }
        }
    }
}

fn sensitive(state: &crate::tick::TickState, target: crate::boundary::EntityId, tag: &str) -> bool {
    state.entities.slot_of(target).is_some_and(|slot| {
        let crate::entities::EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
            return false;
        };
        state
            .registry
            .tags
            .contains_raw_id("minecraft:entity_type", tag, type_id as i32)
    })
}

pub(crate) fn target_damage(
    state: &crate::tick::TickState,
    target: crate::boundary::EntityId,
    stack: &ItemStack,
) -> f32 {
    [
        ("smite", "minecraft:sensitive_to_smite"),
        (
            "bane_of_arthropods",
            "minecraft:sensitive_to_bane_of_arthropods",
        ),
        ("impaling", "minecraft:sensitive_to_impaling"),
    ]
    .into_iter()
    .filter(|(_, tag)| sensitive(state, target, tag))
    .map(|(name, _)| 2.5 * level(stack, name) as f32)
    .sum()
}

pub(crate) fn bane_level(
    state: &crate::tick::TickState,
    target: crate::boundary::EntityId,
    stack: &ItemStack,
) -> u32 {
    if sensitive(state, target, "minecraft:sensitive_to_bane_of_arthropods") {
        level(stack, "bane_of_arthropods")
    } else {
        0
    }
}

pub(crate) fn bane_ticks(level: u32, seed: &mut i32) -> u32 {
    if level == 0 {
        return 0;
    }
    30 + (random(seed) * f64::from(level.saturating_sub(1).saturating_mul(10))) as u32
}

/// Authored 26.1.2 density: half a damage point per level and fallen block.
pub(crate) fn density_damage(stack: &ItemStack, distance: f64) -> f32 {
    (0.5 * f64::from(level(stack, "density")) * distance) as f32
}

/// Breach subtracts from armor's damage fraction, not from armor points.
pub(crate) fn breach_damage(amount: f32, armored: f32, level: u32) -> f32 {
    if level == 0 {
        return armored;
    }
    let amount = amount.max(0.0);
    (armored + amount * 0.15 * level as f32).min(amount)
}

pub(crate) fn wind_burst_multiplier(level: u32) -> f64 {
    match level {
        0 => 0.0,
        1 => 1.2,
        2 => 1.75,
        3 => 2.2,
        _ => 1.5 + 0.35 * f64::from(level - 1),
    }
}

#[cfg(test)]
#[path = "enchantments_tests.rs"]
mod tests;
