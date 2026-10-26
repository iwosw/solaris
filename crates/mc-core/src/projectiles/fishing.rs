//! Native fishing-bobber custody, bites, hooking and the vanilla fishing tables.

use std::sync::Arc;

use mc_data::ItemStack;
use uuid::Uuid;

use super::{ProjectileData, ProjectileKind, ProjectileState};
use crate::boundary::{EntityDataValue, EntityId, Hand, PlayerChange};
use crate::intent::Intent;
use crate::tick::TickState;

#[cfg(test)]
#[path = "fishing_floor_tests.rs"]
mod floor_tests;
#[cfg(test)]
#[path = "fishing_tests.rs"]
mod tests;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FishingState {
    pub hooked: Option<Uuid>,
    pub water_since: Option<u64>,
    pub bite_until: Option<u64>,
    pub lure: u32,
    pub delay: u64,
}

impl FishingState {
    pub fn new(lure: u32, seed: u64) -> Self {
        Self {
            hooked: None,
            water_since: None,
            bite_until: None,
            lure: lure.min(3),
            delay: (100 + random(seed) % 501)
                .saturating_sub(u64::from(lure.min(3)) * 100)
                .max(1),
        }
    }
}

fn random(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}

fn metadata(state: &mut TickState, id: EntityId, hooked: Option<EntityId>, biting: bool) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    let appearance = &mut state.entities.row_mut(slot).appearance;
    let values = [
        (
            8,
            EntityDataValue::VarInt(hooked.map_or(0, |id| id.0.saturating_add(1))),
        ),
        (9, EntityDataValue::Boolean(biting)),
    ];
    if values.iter().all(|(index, value)| {
        appearance
            .extra
            .iter()
            .any(|(at, old)| at == index && old == value)
    }) {
        return;
    }
    let extra = &mut Arc::make_mut(appearance).extra;
    for (index, value) in values {
        if let Some((_, old)) = extra.iter_mut().find(|(at, _)| *at == index) {
            *old = value;
        } else {
            extra.push((index, value));
        }
    }
    super::touch(state, id);
}

pub(crate) fn use_item(state: &mut TickState, actor: EntityId, hand: Hand) -> bool {
    let Some(held_slot) = super::weapons::held_slot(state, actor, hand) else {
        return false;
    };
    let held = &state.players[&actor].inventory.slots[held_slot];
    if held.is_empty() || super::item_name(state, held) != Some("minecraft:fishing_rod") {
        return false;
    }
    let held = held.clone();
    let Some(owner_slot) = state.entities.slot_of(actor) else {
        return true;
    };
    let owner = state.entities.row(owner_slot).uuid;
    let hook = state.entities.slots().find_map(|slot| {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        row.projectile
            .as_ref()
            .filter(|p| p.kind == ProjectileKind::FishingHook && p.owner == Some(owner))
            .map(|_| body.id)
    });
    if let Some(hook) = hook {
        reel(state, actor, held_slot, &held, hook);
        return true;
    }
    let Some((origin, velocity)) = super::weapons::geometry(state, actor, 1.5) else {
        return true;
    };
    let mut record = ProjectileState::new(ProjectileKind::FishingHook, Some(owner), held.clone());
    record.data = ProjectileData::Fishing(FishingState::new(
        crate::enchantments::level(&held, "lure"),
        owner.as_u128() as u64 ^ state.tick,
    ));
    if let Some(id) = super::launch(state, record, origin, velocity) {
        metadata(state, id, None, false);
        if let Some(player) = state.players.get_mut(&actor) {
            player
                .statistics
                .increment(mc_data::statistics::StatType::Used, held.item_id, 1);
        }
    }
    true
}

fn reel(state: &mut TickState, actor: EntityId, held_slot: usize, rod: &ItemStack, id: EntityId) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    let Some(record) = state.entities.row(slot).projectile.as_ref() else {
        return;
    };
    let ProjectileData::Fishing(fishing) = &record.data else {
        return;
    };
    let fishing = fishing.clone();
    let pos = state.entities.pos(slot);
    let stuck = record.stuck;
    let seed = state.entities.row(slot).uuid.as_u128() as u64 ^ state.tick;
    let Some(owner_slot) = state.entities.slot_of(actor) else {
        return;
    };
    let target = state.entities.pos(owner_slot);
    let hooked_slot = fishing
        .hooked
        .and_then(|uuid| state.entities.slot_of_uuid(uuid));
    let biting = fishing.bite_until.is_some_and(|until| state.tick < until);
    // Resolve the actual surviving catch once, before native wear admission.
    // A missing loot-table result does not invent a durability cost.
    let catch = if hooked_slot.is_none() && biting {
        let open = open_water(state, pos);
        let jungle = state
            .world
            .biome_at(
                pos[0].floor() as i32,
                pos[1].floor() as i32,
                pos[2].floor() as i32,
            )
            .is_some_and(|biome| {
                matches!(biome.path(), "jungle" | "sparse_jungle" | "bamboo_jungle")
            });
        let effects = &state.players[&actor].active_effects;
        let luck = crate::enchantments::level(rod, "luck_of_the_sea").min(255) as i32
            + effects.level(crate::effects::EffectId::Luck) as i32
            - effects.level(crate::effects::EffectId::Unluck) as i32;
        loot(&state.registry, luck, open, jungle, seed)
    } else {
        None
    };
    // Vanilla durability: reeling a living entity costs 5; pulling a hooked
    // dropped item costs 3 (mob retrieval is the 5-cost control).
    let hooked_is_item = hooked_slot.is_some_and(|slot| {
        matches!(
            state.entities.body(slot).kind,
            crate::entities::EntityKind::Item { .. }
        )
    });
    let amount = if hooked_is_item {
        3
    } else if hooked_slot.is_some() {
        5
    } else if catch.is_some() {
        1
    } else if !biting && stuck {
        2
    } else {
        0
    };
    let wear = super::weapons::plan_wear(state, actor, held_slot, amount);
    if !super::weapons::costs_allowed(state, actor, &[], held_slot, wear.as_ref()) {
        return;
    }
    if let Some(hooked_slot) =
        hooked_slot.filter(|slot| !state.entity_session_locked(state.entities.body(*slot).id))
    {
        let victim = state.entities.body(hooked_slot).id;
        let at = state.entities.pos(hooked_slot);
        let old = state.entities.vel(hooked_slot);
        let velocity = std::array::from_fn(|axis| old[axis] + (target[axis] - at[axis]) * 2.0);
        state.entities.set_vel(hooked_slot, velocity);
        if let Some(player) = state.players.get(&victim) {
            let connection = player.connection;
            state.push_player(
                connection,
                PlayerChange::Motion {
                    entity: victim,
                    velocity,
                },
            );
        }
        super::touch(state, victim);
        let entity = crate::advancements::entity_snapshot(state, victim);
        let mut event = serde_json::json!({
            "rod": crate::advancements::item_snapshot(state, rod),
            "entity": entity,
        });
        if let Some(item) = &state.entities.row(hooked_slot).item {
            event["item"] = if let Some(stack) = &item.stack {
                crate::advancements::item_snapshot(state, stack)
            } else {
                crate::advancements::item_snapshot(state, &ItemStack::new(item.item_id, item.count))
            };
        }
        crate::advancements::trigger(state, actor, "minecraft:fishing_rod_hooked", event);
    } else if let Some(loot) = catch {
        static FISH_TAG: std::sync::LazyLock<(mc_data::Identifier, mc_data::Identifier)> =
            std::sync::LazyLock::new(|| {
                (
                    mc_data::Identifier::parse("minecraft:item").expect("static registry"),
                    mc_data::Identifier::parse("minecraft:fishes").expect("static fish tag"),
                )
            });
        let fish = state
            .registry
            .tags
            .registries
            .get(&FISH_TAG.0)
            .and_then(|tags| tags.get(&FISH_TAG.1))
            .is_some_and(|items| items.contains(&(loot.item_id as i32)));
        let item = crate::advancements::item_snapshot(state, &loot);
        let entity = crate::advancements::entity_snapshot(state, id);
        let drop = crate::tick::spawn_item_entity(state, pos, loot);
        if let Some(slot) = state.entities.slot_of(drop) {
            let delta = std::array::from_fn::<_, 3, _>(|axis| target[axis] - pos[axis]);
            let mut velocity = delta.map(|v| v * 2.0);
            velocity[1] += delta.iter().map(|v| v * v).sum::<f64>().sqrt().sqrt() * 1.6;
            state.entities.set_vel(slot, velocity);
        }
        state.projectiles.spawned.push(drop);
        if fish && let Some(player) = state.players.get_mut(&actor) {
            player
                .statistics
                .increment_custom(mc_data::statistics::CustomStat::FishCaught, 1);
        }
        let mut spawned = Vec::new();
        crate::experience_orbs::award(state, target, (random(seed) % 6 + 1) as i32, &mut spawned);
        state.projectiles.spawned.extend(spawned);
        let rod = crate::advancements::item_snapshot(state, rod);
        crate::advancements::trigger(
            state,
            actor,
            "minecraft:fishing_rod_hooked",
            serde_json::json!({ "rod": rod, "entity": entity, "item": item }),
        );
    }
    if amount > 0 {
        super::weapons::apply_wear(state, actor, held_slot, wear);
        super::weapons::sync(state, actor);
    }
    super::remove(state, id);
}

/// A hook's contact remains its own retained state, not a damaging throwable.
pub(crate) fn impact(
    state: &mut TickState,
    id: EntityId,
    record: &mut ProjectileState,
    hit: Option<EntityId>,
    position: [f64; 3],
) {
    let ProjectileData::Fishing(fishing) = &mut record.data else {
        return;
    };
    let hooked = hit.and_then(|target| {
        state
            .entities
            .slot_of(target)
            .map(|slot| state.entities.row(slot).uuid)
    });
    fishing.hooked = hooked;
    record.stuck = hit.is_none();
    if let Some(slot) = state.entities.slot_of(id) {
        state.entities.set_pos(slot, position);
        state.entities.set_vel(slot, [0.0; 3]);
        state.entities.set_ground(slot, record.stuck);
    }
    metadata(state, id, hit, false);
}

pub(crate) fn step(
    state: &mut TickState,
    _intents: &mut Vec<Intent>,
    id: EntityId,
    record: &mut ProjectileState,
) -> bool {
    let Some(owner) = super::owner_id(state, record) else {
        super::remove(state, id);
        return true;
    };
    let Some(owner_slot) = state.entities.slot_of(owner) else {
        super::remove(state, id);
        return true;
    };
    let Some(slot) = state.entities.slot_of(id) else {
        return true;
    };
    let position = state.entities.pos(slot);
    let owner_position = state.entities.pos(owner_slot);
    let holds = state.players.get(&owner).is_some_and(|player| {
        !player.spectator()
            && !player.vitals.is_dead()
            && [
                usize::from(player.inventory.selected.min(8)),
                crate::inventory::OFF_HAND,
            ]
            .into_iter()
            .any(|slot| {
                super::item_name(state, &player.inventory.slots[slot])
                    == Some("minecraft:fishing_rod")
            })
    });
    if !holds
        || position
            .iter()
            .zip(owner_position)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            > 1024.0
    {
        super::remove(state, id);
        return true;
    }
    if !matches!(record.data, ProjectileData::Fishing(_)) {
        record.data = ProjectileData::Fishing(FishingState::new(
            0,
            state.entities.row(slot).uuid.as_u128() as u64,
        ));
    }
    let ProjectileData::Fishing(fishing) = &mut record.data else {
        unreachable!()
    };
    if let Some(target_slot) = fishing
        .hooked
        .and_then(|uuid| state.entities.slot_of_uuid(uuid))
    {
        let target = state.entities.body(target_slot).id;
        let mut at = state.entities.pos(target_slot);
        at[1] += 0.8;
        state.entities.set_pos(slot, at);
        state.entities.set_vel(slot, [0.0; 3]);
        metadata(state, id, Some(target), false);
        return true;
    }
    fishing.hooked = None;
    let in_water = state
        .world
        .block_at(
            position[0].floor() as i32,
            position[1].floor() as i32,
            position[2].floor() as i32,
        )
        .is_some_and(|block| {
            state.registry.materials.classify(block) == mc_physics::BlockMaterial::Water
        });
    if !in_water {
        fishing.water_since = None;
        fishing.bite_until = None;
        metadata(state, id, None, false);
        return record.stuck;
    }
    record.stuck = false;
    // Underwater casts rise toward the surface until the block above is not
    // water (vanilla bobber buoyancy); a ceiling keeps the hook in place.
    let above_clear = state
        .world
        .block_at(
            position[0].floor() as i32,
            position[1].floor() as i32 + 1,
            position[2].floor() as i32,
        )
        .is_none_or(|block| {
            state.registry.materials.classify(block) != mc_physics::BlockMaterial::Water
        });
    if !above_clear {
        let rise = state.entities.pos(slot);
        // Move one block per tick would tunnel through the surface check; a
        // gentle per-tick rise keeps the hook inside loaded water.
        state
            .entities
            .set_pos(slot, [rise[0], rise[1] + 0.2, rise[2]]);
        state.entities.set_vel(slot, [0.0; 3]);
        metadata(state, id, None, false);
        return true;
    }
    let since = *fishing.water_since.get_or_insert(state.tick);
    if fishing.bite_until.is_some_and(|until| state.tick >= until) {
        fishing.water_since = Some(state.tick);
        fishing.bite_until = None;
        fishing.delay = FishingState::new(
            fishing.lure,
            state.entities.row(slot).uuid.as_u128() as u64 ^ state.tick,
        )
        .delay;
    } else if fishing.bite_until.is_none() {
        // Rain advances the wait by an extra tick 25% of the time: roughly
        // 20% shorter waits. Retain it in the existing persisted delay field.
        if state.tick > since
            && random(state.entities.row(slot).uuid.as_u128() as u64 ^ state.tick).is_multiple_of(4)
            && crate::conduit::rain_exposed(state, position)
        {
            fishing.delay = fishing.delay.saturating_sub(1);
        }
        if state.tick.saturating_sub(since) >= fishing.delay {
            fishing.bite_until = Some(state.tick.saturating_add(30));
        }
    }
    state.entities.set_vel(slot, [0.0; 3]);
    metadata(state, id, None, fishing.bite_until.is_some());
    true
}

/// Vanilla four homogeneous 5x5 layers: source water below air/lily pads.
pub(crate) fn open_water(state: &TickState, position: [f64; 3]) -> bool {
    let [x, y, z] = position.map(|value| value.floor() as i32);
    let mut above = false;
    for dy in -1..=2 {
        let mut layer = None;
        for dx in -2..=2 {
            for dz in -2..=2 {
                let Some(id) = state.world.block_at(x + dx, y + dy, z + dz) else {
                    return false;
                };
                let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(id)) else {
                    return false;
                };
                let air = matches!(
                    block.block.id.path(),
                    "air" | "cave_air" | "void_air" | "lily_pad"
                );
                let source = (matches!(
                    block.block.id.path(),
                    "water"
                        | "bubble_column"
                        | "kelp"
                        | "kelp_plant"
                        | "seagrass"
                        | "tall_seagrass"
                ) && block
                    .properties
                    .iter()
                    .find(|(key, _)| key == "level")
                    .is_none_or(|(_, value)| value == "0"))
                    || block
                        .properties
                        .iter()
                        .any(|(key, value)| key == "waterlogged" && value == "true");
                let water = source
                    && state
                        .registry
                        .collision_boxes
                        .get(id as usize)
                        .is_some_and(|boxes| boxes.is_empty());
                if !air && !water || layer.is_some_and(|old| old != air) {
                    return false;
                }
                layer = Some(air);
            }
        }
        let air = layer == Some(true);
        if dy == -1 && air || above && !air {
            return false;
        }
        above |= air;
    }
    true
}

/// The shared vanilla 26.1.2 fishing-table evaluator returns canonical stacks.
pub(crate) fn loot(
    registry: &crate::registry::CoreRegistry,
    luck: i32,
    open: bool,
    jungle: bool,
    seed: u64,
) -> Option<ItemStack> {
    registry.fishing_loot.roll(
        &registry.items,
        &registry.item_facts,
        luck,
        open,
        jungle,
        seed,
    )
}
