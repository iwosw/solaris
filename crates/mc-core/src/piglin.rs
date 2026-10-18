//! Piglin gold custody and delayed barter rewards; nether-family targeting helpers.
//! Loot weights/counts/components are the 26.1.2 piglin_bartering table, read
//! from the local server jar. Piglin.getArmPose (javap) uses loved offhand items
//! for ADMIRING_ITEM, so no extra animation metadata or packet is needed.
use crate::boundary::{ChangeSet, EntityId, Hand};
use crate::entities::{ColdRow, EntityKind, EntitySlot};
use crate::special_mobs::Retained;
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use mc_physics::mth;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;

const ADMIRE_TICKS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Barter {
    pub until: u64,
    pub player: Option<Uuid>,
    pub target: [i32; 3],
}

pub(crate) fn name(type_id: u32) -> &'static str {
    mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
        .map_or("", |contract| contract.name)
}

/// Protocol id of an entity-type name in the immutable contract table, for
/// resolving a type-id constant once instead of comparing names per row.
pub(crate) fn type_id_of(wanted: &str) -> Option<u32> {
    (0..mc_data::entity_types::ENTITY_TYPE_COUNT as u32).find(|&id| name(id) == wanted)
}

static PIGLIN_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| type_id_of("minecraft:piglin"));

pub(crate) fn is_piglin_type(type_id: u32) -> bool {
    Some(type_id) == *PIGLIN_TYPE
}

static ZOGLIN_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| type_id_of("minecraft:zoglin"));
static HOGLIN_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| type_id_of("minecraft:hoglin"));
static IRON_GOLEM_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| type_id_of("minecraft:iron_golem"));
static ZOMBIE_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| type_id_of("minecraft:zombie"));
static ZOMBIE_VILLAGER_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| type_id_of("minecraft:zombie_villager"));

pub(crate) fn is_zoglin_type(type_id: u32) -> bool {
    Some(type_id) == *ZOGLIN_TYPE
}

pub(crate) fn is_hoglin_type(type_id: u32) -> bool {
    Some(type_id) == *HOGLIN_TYPE
}

pub(crate) fn is_iron_golem_type(type_id: u32) -> bool {
    Some(type_id) == *IRON_GOLEM_TYPE
}

/// Zombies and zombie villagers hunt villagers as well as players.
pub(crate) fn hunts_villagers_type(type_id: u32) -> bool {
    Some(type_id) == *ZOMBIE_TYPE || Some(type_id) == *ZOMBIE_VILLAGER_TYPE
}

/// Whether a row of the species exists in the store (the type lane, dead rows
/// included): a conservative presence gate for the per-species target lists
/// built each entity phase, which only that species' rows consume.
pub(crate) fn piglin_present(state: &TickState) -> bool {
    (*PIGLIN_TYPE).is_some_and(|id| state.entities.mob_slots(id).next().is_some())
}

pub(crate) fn zoglin_present(state: &TickState) -> bool {
    (*ZOGLIN_TYPE).is_some_and(|id| state.entities.mob_slots(id).next().is_some())
}

pub(crate) fn iron_golem_present(state: &TickState) -> bool {
    (*IRON_GOLEM_TYPE).is_some_and(|id| state.entities.mob_slots(id).next().is_some())
}

/// Cheap kind gate: entity-dense reject before gamerule/activity/registry work.
/// Worlds are ~all non-piglin, so both `run` and `gold_target` test this first.
fn is_piglin(body: &crate::entities::Body) -> bool {
    matches!(body.kind, EntityKind::Mob { type_id } if is_piglin_type(type_id))
}

pub(crate) fn eligible(body: &crate::entities::Body, row: &ColdRow) -> bool {
    is_piglin(body)
        && body.alive
        && !row.controls.no_ai
        && crate::entity_life::adult(row)
        && row.special_mob.is_none()
        && row.appearance.equipment[1].is_none()
}

pub(crate) fn admiring(row: &ColdRow) -> bool {
    matches!(row.special_mob.as_deref(), Some(Retained::Piglin(_)))
}

pub(crate) fn wears_gold(state: &TickState, player: EntityId) -> bool {
    state.players.get(&player).is_some_and(|player| {
        player.inventory.slots[36..40].iter().any(|stack| {
            !stack.is_empty()
                && state
                    .registry
                    .items
                    .name_of(stack.item_id)
                    .is_some_and(|id| {
                        matches!(
                            id.path(),
                            "golden_helmet"
                                | "golden_chestplate"
                                | "golden_leggings"
                                | "golden_boots"
                        )
                    })
        })
    })
}

fn gold_ingot(state: &TickState) -> u32 {
    static ID: std::sync::LazyLock<Identifier> =
        std::sync::LazyLock::new(|| Identifier::parse("minecraft:gold_ingot").unwrap());
    state.registry.items.id_of(&ID).expect("gold item")
}

fn gold(state: &TickState, stack: &ItemStack) -> bool {
    !stack.is_empty() && stack.item_id == gold_ingot(state)
}

fn begin(state: &mut TickState, slot: EntitySlot, player: Option<EntityId>, target: [f64; 3]) {
    let player = player
        .and_then(|id| state.entities.slot_of(id))
        .map(|slot| state.entities.row(slot).uuid);
    state
        .entities
        .row_mut(slot)
        .set_special_mob(Retained::Piglin(Barter {
            until: state.tick + ADMIRE_TICKS,
            player,
            target: target.map(|value| value.floor() as i32),
        }));
    state.entities.row_mut(slot).persistent = true;
    restore(state, slot);
    state.appearance_changed.push(state.entities.body(slot).id);
}

pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    if !admiring(state.entities.row(slot)) {
        return;
    }
    let gold = gold_ingot(state);
    let row = state.entities.row_mut(slot);
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.equipment[1] = Some(ItemStack::new(gold, 1));
    row.loot.drop_chances[1] = 1.0;
}

pub(crate) fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
) -> bool {
    let Some(slot) = state.entities.slot_of(target) else {
        return false;
    };
    if !eligible(state.entities.body(slot), state.entities.row(slot)) {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.spectator()
        || !gold(
            state,
            &player.inventory.slots[crate::tick::held_slot_index(player, hand)],
        )
    {
        return false;
    }
    if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
        return false;
    }
    let connection = player.connection;
    let evidence = (!state.registry.advancements.definitions.is_empty()).then(|| {
        serde_json::json!({
            "item": crate::advancements::item_snapshot(
                state,
                &player.inventory.slots[crate::tick::held_slot_index(player, hand)],
            ),
            "entity": crate::advancements::entity_snapshot(state, target),
        })
    });
    let effects = state.players.get_mut(&actor).unwrap().consume_held(hand, 1);
    for change in effects.changes {
        state.push_player(connection, change);
    }
    let pos = state.entities.pos(state.entities.slot_of(actor).unwrap());
    begin(state, slot, Some(actor), pos);
    if let Some(evidence) = evidence {
        crate::advancements::trigger(
            state,
            actor,
            "minecraft:player_interacted_with_entity",
            evidence,
        );
    }
    true
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|axis| (a[axis] - b[axis]).powi(2)).sum()
}

pub(crate) fn gold_target(state: &TickState, slot: EntitySlot) -> Option<(EntityId, [f64; 3])> {
    // Order: `eligible` tests kind first (rejects ~all of the dense population),
    // then the alive/NoAI/adult/busy/geared row, and only then the gamerule
    // string lookup. No entity scan happens until a real candidate needs gold.
    if !eligible(state.entities.body(slot), state.entities.row(slot))
        || !crate::command_rules::boolean(state, "mob_griefing")
    {
        return None;
    }
    let pos = state.entities.pos(slot);
    let gold = gold_ingot(state);
    state
        .entities
        .slots()
        .filter_map(|item_slot| {
            let row = state.entities.body(item_slot);
            // Order: scalar kind/pickup/alive fields before the carried stack,
            // with the gold id hoisted out of the per-item registry lookup.
            if !matches!(row.kind, EntityKind::Item { .. })
                || row.pickup_delay != 0
                || !row.alive
                || !state
                    .entities
                    .row(item_slot)
                    .appearance
                    .item
                    .as_ref()
                    .is_some_and(|stack| stack.item_id == gold && !stack.is_empty())
            {
                return None;
            }
            let target = state.entities.pos(item_slot);
            let distance = distance(pos, target);
            (distance <= 64.0).then_some((row.id, target, distance))
        })
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(id, pos, _)| (id, pos))
}

pub(crate) fn run(state: &mut TickState, changes: &mut ChangeSet) {
    // Consumed item rows use merge's existing reservation/tombstone path.
    // No dense slots move until commit, and newly thrown items are not visited.
    // Only the piglin type lane is visited (slot order, as the old full scan).
    // Retained::Piglin exists on piglins alone: `begin` runs for eligible
    // piglins, persistence rejects the variant on any other entity id, and
    // `set_kind` is never applied to a piglin. NoAI and inactive semantics and
    // the payout order are unchanged.
    let piglins: Vec<_> = PIGLIN_TYPE
        .iter()
        .flat_map(|&type_id| state.entities.mob_slots(type_id))
        .collect();
    for slot in piglins {
        let row = state.entities.row(slot);
        if !state.entities.body(slot).alive || row.controls.no_ai {
            continue;
        }
        let pos0 = state.entities.pos(slot);
        if !state.world.is_active(
            (pos0[0].floor() as i32).div_euclid(16),
            (pos0[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let id = state.entities.body(slot).id;
        if let Some(&Retained::Piglin(barter)) = state.entities.row(slot).special_mob.as_deref() {
            if state.tick < barter.until {
                continue;
            }
            let pos = state.entities.pos(slot);
            let target = barter
                .player
                .and_then(|uuid| state.entities.slot_of_uuid(uuid))
                .map_or_else(
                    || barter.target.map(|value| f64::from(value) + 0.5),
                    |slot| state.entities.pos(slot),
                );
            state.entities.row_mut(slot).special_mob = None;
            Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[1] = None;
            state.appearance_changed.push(id);
            let stack = reward(state, id.0 as u64 ^ state.tick.rotate_left(23));
            let origin = [pos[0], pos[1] + 1.0, pos[2]];
            let dropped = crate::tick::spawn_item_entity(state, origin, stack);
            let drop_slot = state.entities.slot_of(dropped).unwrap();
            let dx = target[0] - origin[0];
            let dz = target[2] - origin[2];
            let length = mth::length2(dx, dz).max(0.001);
            state
                .entities
                .set_vel(drop_slot, [dx / length * 6.0, 4.0, dz / length * 6.0]);
            changes.entities_spawned.push(dropped);
            continue;
        }
        let Some((item, target)) = gold_target(state, slot) else {
            continue;
        };
        let pos = state.entities.pos(slot);
        if distance(pos, target) > 2.25 {
            continue;
        }
        let player = state
            .players
            .keys()
            .filter_map(|id| {
                let slot = state.entities.slot_of(*id)?;
                let row = state.entities.body(slot);
                (row.alive && !state.players[id].spectator())
                    .then_some((*id, state.entities.pos(slot)))
            })
            .min_by(|a, b| distance(a.1, target).total_cmp(&distance(b.1, target)));
        let item_slot = state.entities.slot_of(item).unwrap();
        let mut stack = state
            .entities
            .row(item_slot)
            .appearance
            .item
            .clone()
            .unwrap();
        stack.count -= 1;
        let row = state.entities.row_mut(item_slot);
        let cargo = row.item.as_mut().unwrap();
        cargo.count = stack.count;
        cargo.stack = Some(stack.clone());
        Arc::make_mut(&mut row.appearance).item = Some(stack);
        state.appearance_changed.push(item);
        state.merge_consumed_items.insert(item_slot);
        begin(
            state,
            slot,
            player.map(|(id, _)| id),
            player.map_or(target, |(_, pos)| pos),
        );
    }
}

// Compact, typed projection of gameplay/piglin_bartering.json (total weight 469).
// Effects: 1 stored Soul Speed book; 2 Soul Speed boots; 3 fire resistance; 4 water.
const LOOT: &[(&str, u32, i32, i32, u8)] = &[
    ("enchanted_book", 5, 1, 1, 1),
    ("iron_boots", 8, 1, 1, 2),
    ("potion", 8, 1, 1, 3),
    ("splash_potion", 8, 1, 1, 3),
    ("potion", 10, 1, 1, 4),
    ("iron_nugget", 10, 10, 36, 0),
    ("ender_pearl", 10, 2, 4, 0),
    ("dried_ghast", 10, 1, 1, 0),
    ("string", 20, 3, 9, 0),
    ("quartz", 20, 5, 12, 0),
    ("obsidian", 40, 1, 1, 0),
    ("crying_obsidian", 40, 1, 3, 0),
    ("fire_charge", 40, 1, 1, 0),
    ("leather", 40, 2, 4, 0),
    ("soul_sand", 40, 2, 8, 0),
    ("nether_brick", 40, 2, 8, 0),
    ("spectral_arrow", 40, 6, 12, 0),
    ("gravel", 40, 8, 16, 0),
    ("blackstone", 40, 8, 16, 0),
];

fn reward(state: &TickState, seed: u64) -> ItemStack {
    use mc_data::loot::chest_26_1_2::ChestRng;
    let mut rng = ChestRng::new(seed);
    let mut roll = rng.next_bounded(LOOT.iter().map(|entry| u64::from(entry.1)).sum()) as u32;
    let &(name, _, min, max, effect) = LOOT
        .iter()
        .find(|entry| {
            if roll < entry.1 {
                true
            } else {
                roll -= entry.1;
                false
            }
        })
        .expect("weighted barter entry");
    let item = state
        .registry
        .items
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .expect("barter item");
    let count = rng.next_range_inclusive(min as u32, max as u32) as i32;
    let mut stack = ItemStack::new(item, count);
    match effect {
        1 | 2 => {
            let enchantment = mc_data::ItemEnchantment {
                id: Identifier::parse("minecraft:soul_speed").unwrap(),
                level: rng.next_range_inclusive(1, 3) as i32,
            };
            if effect == 1 {
                stack.extended_mut().stored_enchantments.push(enchantment);
            } else {
                stack.enchantments.push(enchantment);
            }
        }
        3 | 4 => {
            stack.extended_mut().potion = Some(
                Identifier::parse(if effect == 3 {
                    "minecraft:fire_resistance"
                } else {
                    "minecraft:water"
                })
                .unwrap(),
            )
        }
        _ => {}
    }
    stack
}

pub(crate) fn warped_fungus(
    pos: [f64; 3],
    registry: &crate::registry::CoreRegistry,
    block_at: &dyn Fn(i32, i32, i32) -> Option<u32>,
) -> Option<[f64; 3]> {
    let block = registry
        .blocks
        .block(&Identifier::parse("minecraft:warped_fungus").unwrap())?;
    let centre = pos.map(|value| value.floor() as i32);
    let mut nearest: Option<([f64; 3], f64)> = None;
    for dx in -8..=8 {
        for dz in -8..=8 {
            for dy in -4..=4 {
                let [x, y, z] = [centre[0] + dx, centre[1] + dy, centre[2] + dz];
                if block_at(x, y, z) != Some(block.default.0) {
                    continue;
                }
                let target = [f64::from(x) + 0.5, f64::from(y), f64::from(z) + 0.5];
                let dist = distance(pos, target);
                if dist <= 64.0 && nearest.is_none_or(|(_, best)| dist < best) {
                    nearest = Some((target, dist));
                }
            }
        }
    }
    nearest.map(|(pos, _)| pos)
}

#[cfg(test)]
#[path = "piglin_tests.rs"]
mod tests;
