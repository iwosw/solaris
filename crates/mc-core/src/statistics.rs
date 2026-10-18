//! Statistic awards at authoritative gameplay outcomes, never client predictions.
use crate::boundary::EntityId;
use crate::entities::{EntityKind, EntitySlot};
use crate::tick::TickState;
use mc_data::statistics::{CustomStat, StatType};
use mc_physics::mth;

static CLIMBABLE_TAG: std::sync::LazyLock<(mc_data::Identifier, mc_data::Identifier)> =
    std::sync::LazyLock::new(|| {
        (
            mc_data::Identifier::parse("minecraft:block").expect("static registry identifier"),
            mc_data::Identifier::parse("minecraft:climbable").expect("static tag identifier"),
        )
    });

pub(crate) fn climbable(state: &TickState, raw_id: i32) -> bool {
    let (registry, tag) = &*CLIMBABLE_TAG;
    state
        .registry
        .tags
        .registries
        .get(registry)
        .and_then(|tags| tags.get(tag))
        .is_some_and(|ids| ids.contains(&raw_id))
}

/// Vanilla casts the Euclidean length to float, then rounds centimeters.
fn centimeters(distance: f64) -> i32 {
    ((distance as f32) * 100.0).round() as i32
}

pub(crate) fn tick(state: &mut TickState, tick: u64) {
    for (entity, player) in &mut state.players {
        let sneaking = state
            .entities
            .slot_of_any(*entity)
            .is_some_and(|slot| state.entities.row(slot).appearance.shared_flags & 0x02 != 0);
        player.statistics.set_active(
            true,
            !player.vitals.is_dead(),
            state.sleepers.contains_key(entity),
            sneaking,
        );
        player.statistics.advance_tick(tick);
    }
}

pub(crate) fn movement(
    state: &mut TickState,
    entity: EntityId,
    slot: EntitySlot,
    old: [f64; 3],
    new: [f64; 3],
    on_ground: bool,
) {
    let delta = std::array::from_fn::<_, 3, _>(|axis| new[axis] - old[axis]);
    let horizontal = mth::length2(delta[0], delta[2]);
    let full = mth::length2(horizontal, delta[1]);
    let row = state.entities.row(slot);
    let grounded = on_ground || state.entities.ground(slot);
    let pose = row.appearance.pose;
    let crouching = row.appearance.shared_flags & 0x02 != 0 || pose == 5;
    let water_at = |y: f64| {
        state
            .world
            .block_at(
                new[0].floor() as i32,
                y.floor() as i32,
                new[2].floor() as i32,
            )
            .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
            .is_some_and(|block| {
                block.block.id.path() == "water"
                    || block
                        .properties
                        .iter()
                        .any(|(name, value)| name == "waterlogged" && value == "true")
            })
    };
    let eye_water = water_at(new[1] + crate::tick::player_eye_height(pose));
    let feet_water = water_at(new[1]);
    let climbable = state
        .world
        .block_at(
            new[0].floor() as i32,
            new[1].floor() as i32,
            new[2].floor() as i32,
        )
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| climbable(state, block.block.raw_id));
    let Some(player) = state.players.get_mut(&entity) else {
        return;
    };
    let was_grounded = player.statistics_grounded;
    player.statistics_grounded = grounded;
    if !player.spectator()
        && was_grounded
        && !grounded
        && delta[1] > 0.0
        && !feet_water
        && !climbable
    {
        player.statistics.increment_custom(CustomStat::Jump, 1);
    }
    let (stat, amount) = if (pose == 3 || player.sprinting && eye_water) && feet_water {
        (CustomStat::SwimOneCm, centimeters(full))
    } else if eye_water {
        (CustomStat::WalkUnderWaterOneCm, centimeters(full))
    } else if feet_water {
        (CustomStat::WalkOnWaterOneCm, centimeters(horizontal))
    } else if climbable {
        (
            CustomStat::ClimbOneCm,
            (delta[1].max(0.0) * 100.0).round() as i32,
        )
    } else if grounded {
        (
            if player.sprinting {
                CustomStat::SprintOneCm
            } else if crouching {
                CustomStat::CrouchOneCm
            } else {
                CustomStat::WalkOneCm
            },
            centimeters(horizontal),
        )
    } else if pose == 1 {
        (CustomStat::AviateOneCm, centimeters(full))
    } else {
        let cm = centimeters(horizontal);
        (CustomStat::FlyOneCm, if cm > 25 { cm } else { 0 })
    };
    if amount > 0 {
        player.statistics.increment_custom(stat, amount);
    }
}

pub(crate) fn riding(
    state: &mut TickState,
    entity: EntityId,
    vehicle: EntitySlot,
    old: [f64; 3],
    new: [f64; 3],
) {
    let name = crate::mounts::name(state, vehicle);
    let stat = if let Some(kind) = crate::vehicles::kind(state, vehicle) {
        match kind {
            crate::vehicles::VehicleKind::Boat => CustomStat::BoatOneCm,
            crate::vehicles::VehicleKind::Minecart => CustomStat::MinecartOneCm,
        }
    } else {
        match name {
            "minecraft:pig" => CustomStat::PigOneCm,
            "minecraft:strider" => CustomStat::StriderOneCm,
            "minecraft:horse"
            | "minecraft:donkey"
            | "minecraft:mule"
            | "minecraft:skeleton_horse"
            | "minecraft:zombie_horse"
            | "minecraft:camel"
            | "minecraft:camel_husk"
            | "minecraft:llama"
            | "minecraft:trader_llama" => CustomStat::HorseOneCm,
            "minecraft:happy_ghast" => CustomStat::HappyGhastOneCm,
            "minecraft:nautilus" | "minecraft:zombie_nautilus" => CustomStat::NautilusOneCm,
            _ => return,
        }
    };
    let cm = centimeters(mth::length3(
        new[0] - old[0],
        new[1] - old[1],
        new[2] - old[2],
    ));
    if cm > 0
        && let Some(player) = state.players.get_mut(&entity)
    {
        player.statistics.increment_custom(stat, cm);
    }
}

pub(crate) fn opened(state: &mut TickState, entity: EntityId, block: mc_world::BlockPos) {
    let Some(name) = state
        .world
        .block_at(block.x, block.y, block.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .map(|block| block.block.id.path())
    else {
        return;
    };
    let stat = match name {
        "chest" => CustomStat::OpenChest,
        "trapped_chest" => CustomStat::TriggerTrappedChest,
        "barrel" => CustomStat::OpenBarrel,
        "hopper" => CustomStat::InspectHopper,
        "dispenser" => CustomStat::InspectDispenser,
        "dropper" => CustomStat::InspectDropper,
        "furnace" => CustomStat::InteractWithFurnace,
        "blast_furnace" => CustomStat::InteractWithBlastFurnace,
        "smoker" => CustomStat::InteractWithSmoker,
        "brewing_stand" => CustomStat::InteractWithBrewingStand,
        "crafting_table" => CustomStat::InteractWithCraftingTable,
        _ if crate::shulker::is_block(name) => CustomStat::OpenShulkerBox,
        _ if crate::copper::chest(name) => CustomStat::OpenChest,
        _ => return,
    };
    if let Some(player) = state.players.get_mut(&entity) {
        player.statistics.increment_custom(stat, 1);
    }
}

/// Only LivingEntity sources/victims participate in vanilla kill statistics.
pub(crate) fn living_type(state: &TickState, entity: EntityId) -> Option<u32> {
    let slot = state.entities.slot_of_any(entity)?;
    match state.entities.body(slot).kind {
        EntityKind::Player { .. } => {
            mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:player")
                .map(|kind| kind.protocol_id)
        }
        EntityKind::Mob { type_id } => {
            mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                .filter(|kind| {
                    kind.behavior.archetype == mc_data::entity_types::EntityArchetype::Living
                })
                .map(|kind| kind.protocol_id)
        }
        _ => None,
    }
}

pub(crate) fn kill_credit(
    state: &TickState,
    source: Option<crate::intent::DamageSource>,
) -> Option<(EntityId, u32)> {
    if let Some(crate::intent::DamageSource::Explosion { kill_credit, .. }) = source {
        return kill_credit;
    }
    let entity = attacker(source)?;
    Some((entity, living_type(state, entity)?))
}

pub(crate) fn kill(state: &mut TickState, killer: Option<(EntityId, u32)>, victim: EntityId) {
    let Some(victim_slot) = state.entities.slot_of_any(victim) else {
        return;
    };
    let kind = state.entities.body(victim_slot).kind;
    let victim_type = living_type(state, victim);
    // LivingEntity.getKillCredit prefers the retained recent player hit, even
    // when another living entity or an environmental source finishes the kill.
    let recent_player = state
        .entities
        .row(victim_slot)
        .loot
        .player_credit
        .as_ref()
        .filter(|credit| state.tick.saturating_sub(credit.tick) < 100)
        .and_then(|credit| {
            let entity = EntityId(credit.id);
            state.players.contains_key(&entity).then_some(entity)
        })
        .and_then(|entity| living_type(state, entity).map(|type_id| (entity, type_id)));
    let killer = recent_player.or(killer);
    if let Some(player) = state.players.get_mut(&victim)
        && let Some((_, type_id)) = killer
    {
        player.statistics.increment(StatType::KilledBy, type_id, 1);
    }
    if let Some((killer, _)) = killer.filter(|(id, _)| *id != victim)
        && let Some(player) = state.players.get_mut(&killer)
        && let Some(type_id) = victim_type
    {
        player.statistics.increment(StatType::Killed, type_id, 1);
        player.statistics.increment_custom(
            if matches!(kind, EntityKind::Player { .. }) {
                CustomStat::PlayerKills
            } else {
                CustomStat::MobKills
            },
            1,
        );
    }
}

pub(crate) fn attacker(source: Option<crate::intent::DamageSource>) -> Option<EntityId> {
    Some(match source? {
        crate::intent::DamageSource::Melee { attacker, .. }
        | crate::intent::DamageSource::Mace { attacker, .. }
        | crate::intent::DamageSource::Sweep { attacker, .. } => attacker,
        crate::intent::DamageSource::Projectile {
            projectile, owner, ..
        } if projectile != owner => owner,
        crate::intent::DamageSource::Projectile { .. } => return None,
        crate::intent::DamageSource::Explosion { kill_credit, .. } => {
            return kill_credit.map(|(entity, _)| entity);
        }
    })
}

pub(crate) fn damage_dealt(
    players: &mut rustc_hash::FxHashMap<EntityId, crate::player::PlayerState>,
    source: Option<crate::intent::DamageSource>,
    dealt: f32,
    absorbed: f32,
    resisted: f32,
) {
    let Some(source) = source else {
        return;
    };
    let Some(attacker) = attacker(Some(source)) else {
        return;
    };
    if let Some(player) = players.get_mut(&attacker) {
        if !matches!(
            source,
            crate::intent::DamageSource::Projectile { .. }
                | crate::intent::DamageSource::Explosion { .. }
                | crate::intent::DamageSource::Sweep { .. }
        ) {
            player
                .statistics
                .increment_custom(CustomStat::DamageDealt, (dealt * 10.0).round() as i32);
        }
        player.statistics.increment_custom(
            CustomStat::DamageDealtAbsorbed,
            (absorbed * 10.0).round() as i32,
        );
        player.statistics.increment_custom(
            CustomStat::DamageDealtResisted,
            (resisted * 10.0).round() as i32,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn distance_is_rounded_integer_centimeters() {
        assert_eq!(centimeters(1.234), 123);
        assert_eq!(centimeters(0.005), 1);
        assert_eq!(centimeters(0.0049), 0);
        assert_eq!(centimeters(5.0), 500);
    }
}
