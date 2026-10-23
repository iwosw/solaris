//! Native ageable-animal lifecycle and merge-owned harvesting.
//!
//! Age is separate from an entity's lifetime/item despawn clock. The retained
//! image travels with the existing entity journal and checkpoint records.
use std::sync::Arc;

use mc_data::{Identifier, ItemStack};
use serde::{Deserialize, Serialize};

use crate::boundary::{EntityId, Hand};
use crate::entities::{ColdRow, EntityKind};
use crate::intent::{Footprint, Intent, IntentKind, InvClaim};
use crate::tick::{TickState, spawn_item_entity};

#[path = "entity_life_breeding.rs"]
pub(crate) mod breeding;

pub const CHILD_TICKS: u32 = 24_000;
pub const LOVE_TICKS: u32 = 600;
pub const PARENT_COOLDOWN: u32 = 6_000;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EntityLifeState {
    pub child_ticks: u32,
    pub age_lock: crate::golden_dandelion::AgeLock,
    /// Zero seeds a timer without producing an item.
    pub harvest_ticks: u32,
    pub tamed: bool,
    pub sitting: bool,
    #[serde(skip)]
    pub mounted: bool,
    pub taming: crate::taming::TamingState,
    pub mount: crate::mounts::MountState,
    pub leash: Option<crate::leash::Holder>,
    pub vehicle: Option<Box<crate::vehicles::VehicleState>>,
    pub brown_mooshroom: bool,
    pub stew_effects: Vec<mc_data::item_stack::StewEffect>,
    pub portal: crate::portals::PortalState,
    pub dolphin: Option<crate::dolphin::DolphinState>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Harvest {
    Milk,
    Stew,
    Flower(mc_data::item_stack::StewEffect),
    ShearWool,
    ShearMooshroom,
    Dye(u8),
}

/// The exact held stack is revalidated at merge, including tool damage and
/// components: two predictions cannot spend the same last durability point.
#[derive(Debug, Clone, PartialEq)]
pub struct HarvestPlan {
    pub target: EntityId,
    pub slot: usize,
    pub held: ItemStack,
    pub action: Harvest,
}

fn item(state: &TickState, name: &str) -> Option<u32> {
    state.registry.items.id_of(&Identifier::parse(name).ok()?)
}

pub fn adult(row: &ColdRow) -> bool {
    row.entity_life.child_ticks == 0 && !row.appearance.baby
}

pub fn can_feed(body: &crate::entities::Body, row: &ColdRow) -> bool {
    body.alive
        && (row.entity_life.child_ticks > 0
            || (adult(row) && row.love == 0 && row.breed_cooldown == 0))
}

pub fn breed_ready(body: &crate::entities::Body, row: &ColdRow) -> bool {
    body.alive
        && adult(row)
        && row.breed_cooldown == 0
        && row.love > 0
        && row.love <= LOVE_TICKS - 60
}

/// Species prerequisites from OLD passive_mobs authority; no session state.
pub fn can_breed(state: &TickState, body: &crate::entities::Body, row: &ColdRow) -> bool {
    can_breed_in(&state.registry, body, row)
}

/// [`can_breed`] over the registry alone, for workers without a `TickState`.
pub(crate) fn can_breed_in(
    registry: &crate::registry::CoreRegistry,
    body: &crate::entities::Body,
    row: &ColdRow,
) -> bool {
    let EntityKind::Mob { type_id } = body.kind else {
        return false;
    };
    let Some(blueprint) = registry.blueprint(type_id) else {
        return false;
    };
    match blueprint.name.as_str() {
        "minecraft:wolf" | "minecraft:cat" => row.entity_life.tamed && !row.entity_life.sitting,
        "minecraft:horse" | "minecraft:donkey" | "minecraft:llama" | "minecraft:trader_llama" => {
            row.entity_life.tamed
                && !row.entity_life.mounted
                && row.health >= blueprint.params.max_health
        }
        "minecraft:mule" | "minecraft:parrot" => false,
        _ => true,
    }
}

pub fn feed(state: &mut TickState, target: EntityId) {
    let Some(slot) = state.entities.slot_of(target) else {
        return;
    };
    let row = state.entities.row_mut(slot);
    if row.entity_life.age_lock.locked {
        return;
    }
    if row.entity_life.child_ticks > 0 {
        let growth = (row.entity_life.child_ticks / 20 / 10) * 20;
        row.entity_life.child_ticks = row.entity_life.child_ticks.saturating_sub(growth);
    } else {
        row.love = LOVE_TICKS;
    }
}

pub fn make_child(state: &mut TickState, id: EntityId) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    if !state.entities.row(slot).appearance.baby {
        let half_width = state.entities.body(slot).half_width * 0.5;
        state.entities.set_half_width(slot, half_width);
    }
    let (body, row) = state.entities.parts_mut(slot);
    row.entity_life.child_ticks = CHILD_TICKS;
    row.breed_cooldown = 0;
    row.love = 0;
    if !row.appearance.baby {
        body.height *= 0.5;
        Arc::make_mut(&mut row.appearance).baby = true;
    }
    state.appearance_changed.push(id);
}

/// Rebuild retained species appearance when a checkpoint/replay row is restored.
pub fn restore_appearance(body: &crate::entities::Body, row: &mut ColdRow) {
    row.entity_life.mounted = row.entity_life.mount.rider.is_some();
    let EntityKind::Mob { type_id } = body.kind else {
        return;
    };
    let Some(contract) = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
    else {
        return;
    };
    if old_ageable(contract.name) || contract.name == "minecraft:tadpole" {
        crate::golden_dandelion::sync(row);
    }
    crate::taming::restore(row, contract.name);
    if contract.behavior.metadata_schema.as_str() == "MushroomCow" {
        let appearance = Arc::make_mut(&mut row.appearance);
        appearance.extra.retain(|(index, _)| *index != 18);
        appearance.extra.push((
            18,
            crate::boundary::EntityDataValue::VarInt(i32::from(row.entity_life.brown_mooshroom)),
        ));
    }
}

pub fn breed(
    state: &mut TickState,
    actor: EntityId,
    partner: EntityId,
    pos: [f64; 3],
    type_id: u32,
) {
    // Vanilla credits the first parent's online love cause, falling back to
    // the partner's. Unattributed births do not invent a nearby beneficiary.
    let breeder = [actor, partner].into_iter().find_map(|id| {
        let parent = state.entities.row(state.entities.slot_of(id)?);
        let uuid = parent.entity_life.taming.love_cause?;
        let slot = state.entities.slot_of_uuid(uuid)?;
        let id = state.entities.body(slot).id;
        state.players.contains_key(&id).then_some(id)
    });
    let wool = state
        .registry
        .blueprint(type_id)
        .filter(|blueprint| blueprint.wool)
        .and_then(|_| {
            let first = state.entities.row(state.entities.slot_of(actor)?);
            let second = state.entities.row(state.entities.slot_of(partner)?);
            let a = crate::tick::wool_of(&first.appearance) & 15;
            let b = crate::tick::wool_of(&second.appearance) & 15;
            let pair = (a.min(b), a.max(b));
            Some(match pair {
                (4, 14) => 1,
                (6, 10) => 2,
                (0, 11) => 3,
                (0, 13) => 5,
                (0, 14) => 6,
                (0, 15) => 7,
                (0, 7) => 8,
                (11, 13) => 9,
                (11, 14) => 10,
                _ => {
                    let (low, high) = if actor < partner {
                        (actor, partner)
                    } else {
                        (partner, actor)
                    };
                    let seed = (low.0 as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        ^ (high.0 as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
                        ^ state.tick.wrapping_mul(0x1656_67B1_9E37_79F9);
                    if (seed.rotate_left(23) & 1 == 0) == (actor < partner) {
                        a
                    } else {
                        b
                    }
                }
            })
        });
    // Only a retained love cause credits the player; the breeding actors are mobs.
    let advancement_breeding = [actor, partner].into_iter().find_map(|parent| {
        let slot = state.entities.slot_of(parent)?;
        let cause = state.entities.row(slot).entity_life.taming.love_cause?;
        let player_slot = state.entities.slot_of_uuid(cause)?;
        let player = state.entities.body(player_slot).id;
        state.players.contains_key(&player).then(|| {
            (
                player,
                serde_json::json!({
                    "parent": crate::advancements::entity_snapshot(state, actor),
                    "partner": crate::advancements::entity_snapshot(state, partner),
                }),
            )
        })
    });
    for id in [actor, partner] {
        if let Some(slot) = state.entities.slot_of(id) {
            let row = state.entities.row_mut(slot);
            row.love = 0;
            row.breed_cooldown = PARENT_COOLDOWN;
        }
    }
    let health = state
        .registry
        .blueprint(type_id)
        .map_or(10.0, |blueprint| blueprint.params.max_health);
    state.merge_spawns.push(crate::tick::MergeSpawn {
        pos,
        type_id,
        health,
        taming: crate::taming::inherit(state, actor, partner, type_id),
        wool,
        advancement_breeding,
    });
    if let Some(player) = breeder.and_then(|id| state.players.get_mut(&id)) {
        player
            .statistics
            .increment_custom(mc_data::statistics::CustomStat::AnimalsBred, 1);
    }
}

fn flower_effect(name: &str) -> Option<mc_data::item_stack::StewEffect> {
    let (effect, duration) = match name.strip_prefix("minecraft:")? {
        "allium" => ("fire_resistance", 60),
        "azure_bluet" | "open_eyeblossom" => ("blindness", 220),
        "blue_orchid" | "dandelion" | "golden_dandelion" => ("saturation", 7),
        "closed_eyeblossom" => ("nausea", 140),
        "cornflower" => ("jump_boost", 100),
        "lily_of_the_valley" => ("poison", 220),
        "oxeye_daisy" => ("regeneration", 140),
        "poppy" | "torchflower" => ("night_vision", 100),
        "red_tulip" | "orange_tulip" | "white_tulip" | "pink_tulip" => ("weakness", 140),
        "wither_rose" => ("wither", 140),
        _ => return None,
    };
    Some(mc_data::item_stack::StewEffect {
        id: Identifier::parse(format!("minecraft:{effect}")).ok()?,
        duration,
    })
}

fn harvest(
    state: &TickState,
    body: &crate::entities::Body,
    row: &ColdRow,
    held: &ItemStack,
) -> Option<Harvest> {
    if !body.alive || held.is_empty() {
        return None;
    }
    let EntityKind::Mob { type_id } = body.kind else {
        return None;
    };
    let blueprint = state.registry.blueprint(type_id)?;
    let name = state.registry.items.name_of(held.item_id)?.as_str();
    if name == "minecraft:bucket"
        && adult(row)
        && matches!(
            blueprint.name.as_str(),
            "minecraft:cow" | "minecraft:goat" | "minecraft:mooshroom"
        )
        && item(state, "minecraft:milk_bucket").is_some()
    {
        return Some(Harvest::Milk);
    }
    if blueprint.name == "minecraft:mooshroom" {
        if name == "minecraft:bowl" && adult(row) {
            let soup = if row.entity_life.stew_effects.is_empty() {
                "minecraft:mushroom_stew"
            } else {
                "minecraft:suspicious_stew"
            };
            return item(state, soup).map(|_| Harvest::Stew);
        }
        if row.entity_life.brown_mooshroom
            && row.entity_life.stew_effects.is_empty()
            && let Some(effect) = flower_effect(name)
        {
            return Some(Harvest::Flower(effect));
        }
        if name == "minecraft:shears" && adult(row) {
            let mushroom = if row.entity_life.brown_mooshroom {
                "minecraft:brown_mushroom"
            } else {
                "minecraft:red_mushroom"
            };
            if item(state, mushroom).is_some()
                && state
                    .registry
                    .entity_types
                    .id_of(&Identifier::parse("minecraft:cow").ok()?)
                    .is_some()
            {
                return Some(Harvest::ShearMooshroom);
            }
        }
    }
    if blueprint.wool {
        let wool = crate::tick::wool_of(&row.appearance);
        if name == "minecraft:shears" && adult(row) && wool & 0x10 == 0 {
            return Some(Harvest::ShearWool);
        }
        if wool & 0x10 == 0 {
            for (color, dye) in crate::blueprint::DYE_COLORS.iter().enumerate() {
                if name
                    .strip_prefix("minecraft:")
                    .and_then(|name| name.strip_suffix("_dye"))
                    == Some(*dye)
                    && wool & 15 != color as u8
                {
                    return Some(Harvest::Dye(color as u8));
                }
            }
        }
    }
    None
}

pub fn valid(state: &TickState, actor: EntityId, plan: &HarvestPlan) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let Some(slot) = state.entities.slot_of(plan.target) else {
        return false;
    };
    !player.spectator()
        && !player.vitals.is_dead()
        && player.inventory.slots.get(plan.slot) == Some(&plan.held)
        && harvest(
            state,
            state.entities.body(slot),
            state.entities.row(slot),
            &plan.held,
        )
        .as_ref()
            == Some(&plan.action)
        && !below_harvest_cost_floors(state, actor, plan)
}

fn below_harvest_cost_floors(state: &TickState, actor: EntityId, plan: &HarvestPlan) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.creative() {
        return false;
    }
    let wear = matches!(plan.action, Harvest::ShearWool | Harvest::ShearMooshroom);
    let rules = state.registry.item_rules();
    let breaks = wear
        && rules
            .facts
            .facts_for_stack(&plan.held, rules.items)
            .and_then(|facts| facts.max_damage)
            .is_some_and(|max| {
                u32::try_from(plan.held.damage.unwrap_or(0).saturating_add(1))
                    .is_ok_and(|damage| damage >= max)
            });
    let debit = [(plan.slot, 1)];
    crate::plugin_inventories::below_player_debit_floors(
        state,
        actor,
        if wear { &[] } else { &debit },
        breaks.then_some(plan.slot),
    )
}

/// Called from the existing InteractEntity path, after its geometry check.
pub fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
    intents: &mut Vec<Intent>,
) {
    let Some(slot) = state.entities.slot_of(target) else {
        return;
    };
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    let EntityKind::Mob { type_id } = body.kind else {
        return;
    };
    let Some(blueprint) = state.registry.blueprint(type_id) else {
        return;
    };
    let Some(player) = state.players.get(&actor) else {
        return;
    };
    if player.spectator() || player.vitals.is_dead() {
        return;
    }
    let held_slot = match hand {
        Hand::Main => usize::from(player.inventory.selected.min(8)),
        Hand::Off => crate::inventory::OFF_HAND,
    };
    let held = player.inventory.slots[held_slot].clone();
    let creative = player.creative();
    let kind = if let Some(action) = harvest(state, body, row, &held) {
        IntentKind::HarvestAnimal {
            plan: HarvestPlan {
                target,
                slot: held_slot,
                held: held.clone(),
                action,
            },
        }
    } else if can_feed(body, row)
        && can_breed(state, body, row)
        && blueprint.has_goal(crate::blueprint::GoalKind::Breed)
        && !held.is_empty()
        && blueprint.breed_food.contains(&held.item_id)
    {
        IntentKind::FeedAnimal { target }
    } else {
        return;
    };
    let take = if matches!(kind, IntentKind::FeedAnimal { .. }) {
        i32::from(!creative)
    } else {
        0
    };
    let seq = state.next_seq(actor);
    intents.push(Intent {
        actor,
        seq,
        kind,
        footprint: Footprint {
            entity_reads: vec![actor],
            entity_writes: vec![target],
            inv: vec![InvClaim {
                owner: actor,
                slot: held_slot as u16,
                expect_item: held.item_id,
                expect_count: held.count,
                take,
                put_item: None,
                put_count: 0,
            }],
            ..Default::default()
        },
    });
}

pub fn apply(state: &mut TickState, actor: EntityId, plan: &HarvestPlan) {
    let Some(slot) = state.entities.slot_of(plan.target) else {
        return;
    };
    let pos = state.entities.pos(slot);
    let mut output = None;
    let mut wear = false;
    let mut consume = true;
    match &plan.action {
        Harvest::Milk => {
            output = item(state, "minecraft:milk_bucket").map(|id| ItemStack::new(id, 1))
        }
        Harvest::Stew => {
            let life = &state.entities.row(slot).entity_life;
            let name = if life.stew_effects.is_empty() {
                "minecraft:mushroom_stew"
            } else {
                "minecraft:suspicious_stew"
            };
            output = item(state, name).map(|id| ItemStack::new(id, 1));
            if let Some(soup) = &mut output {
                soup.stew_effects =
                    std::mem::take(&mut state.entities.row_mut(slot).entity_life.stew_effects);
            }
        }
        Harvest::Flower(effect) => state
            .entities
            .row_mut(slot)
            .entity_life
            .stew_effects
            .push(effect.clone()),
        Harvest::ShearWool => {
            let wool = crate::tick::wool_of(&state.entities.row(slot).appearance);
            let name = format!(
                "minecraft:{}_wool",
                crate::blueprint::DYE_COLORS[usize::from(wool & 15)]
            );
            if let Some(id) = item(state, &name) {
                let count =
                    1 + ((actor.0 as u64) ^ state.tick).wrapping_mul(0x9E37_79B9_7F4A_7C15) % 3;
                state.merge_life_drops.push((
                    [pos[0], pos[1] + 1.0, pos[2]],
                    ItemStack::new(id, count as i32),
                ));
            }
            crate::tick::set_wool(
                Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
                wool | 0x10,
            );
            state.appearance_changed.push(plan.target);
            wear = true;
            consume = false;
        }
        Harvest::ShearMooshroom => {
            let brown = state.entities.row(slot).entity_life.brown_mooshroom;
            if let Some(id) = item(
                state,
                if brown {
                    "minecraft:brown_mushroom"
                } else {
                    "minecraft:red_mushroom"
                },
            ) {
                state.merge_life_drops.push((pos, ItemStack::new(id, 5)));
            }
            let cow = state
                .registry
                .entity_types
                .id_of(&Identifier::parse("minecraft:cow").expect("static identifier"))
                .expect("validated cow");
            let blueprint = state.registry.blueprint(cow).expect("cow blueprint");
            state.entities.set_half_width(slot, blueprint.half_width);
            state
                .entities
                .set_kind(slot, EntityKind::Mob { type_id: cow });
            let (body, row) = state.entities.parts_mut(slot);
            row.entity_life.brown_mooshroom = false;
            row.entity_life.stew_effects.clear();
            let appearance = Arc::make_mut(&mut row.appearance);
            appearance.extra.retain(|(index, _)| *index != 18);
            body.height = blueprint.height;
            row.speed = blueprint.params.speed;
            state.appearance_changed.push(plan.target);
            wear = true;
            consume = false;
        }
        Harvest::Dye(color) => {
            crate::tick::set_wool(
                Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
                *color,
            );
            state.appearance_changed.push(plan.target);
        }
    }
    let rules = state.registry.item_rules();
    let player = state.players.get_mut(&actor).expect("validated actor");
    let creative = player.creative();
    if wear && !creative {
        let held = &mut player.inventory.slots[plan.slot];
        if let Some(max) = rules
            .facts
            .facts_for_stack(held, rules.items)
            .and_then(|facts| facts.max_damage)
        {
            let damage = held.damage.unwrap_or(0).saturating_add(1);
            if u32::try_from(damage).is_ok_and(|damage| damage >= max) {
                *held = ItemStack::EMPTY;
            } else {
                held.damage = Some(damage);
            }
        }
    }
    if consume && !creative {
        let held = &mut player.inventory.slots[plan.slot];
        held.count -= 1;
        if held.count <= 0 {
            *held = output.take().unwrap_or(ItemStack::EMPTY);
        }
    }
    if let Some(mut stack) = output {
        // Creative retains the input and only adds a missing result.
        if !creative
            || !player.inventory.slots.iter().any(|held| {
                !held.is_empty()
                    && held.item_id == stack.item_id
                    && held.stew_effects == stack.stew_effects
            })
        {
            player.inventory.add(&rules, &mut stack);
            if !stack.is_empty() {
                state.merge_life_drops.push((pos, stack));
            }
        }
    }
    let contents = player.inventory_contents();
    if let Some(connection) = state.connections.entity_connection(actor) {
        state.push_player(connection, contents);
    }
}

/// Vanilla `Chicken.eggTime = random.nextInt(6000) + 6000`. The roll is mixed
/// through SplitMix: a bare multiplicative hash of sequential ids is an
/// arithmetic progression mod 6001 that clusters the first layings early.
fn egg_interval(id: EntityId, tick: u64) -> u32 {
    6000 + (crate::farming::hash(&[id.0 as i64 as u64, tick, 0xE66_1A71]) % 6000) as u32
}

/// Every living animal advances even when its AI decision cadence is thinned.
pub fn advance(state: &mut TickState) {
    static IDS: std::sync::LazyLock<(Identifier, Identifier)> = std::sync::LazyLock::new(|| {
        (
            Identifier::parse("minecraft:chicken").expect("static identifier"),
            Identifier::parse("minecraft:egg").expect("static identifier"),
        )
    });
    let chicken = state.registry.entity_types.id_of(&IDS.0);
    let egg = state.registry.items.id_of(&IDS.1);
    static TADPOLE: std::sync::LazyLock<Identifier> = std::sync::LazyLock::new(|| {
        Identifier::parse("minecraft:tadpole").expect("static identifier")
    });
    let tadpole = state.registry.entity_types.id_of(&TADPOLE);
    state.entities.prepare_motion_hints(&state.registry);
    let mut slots = std::mem::take(&mut state.serial_entity_slots);
    slots.clear();
    slots.extend(state.entities.motion_slots(crate::entities::MOTION_LIFE));
    let mut active_column = None;
    for &slot in &slots {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        let living_mob = body.alive && matches!(body.kind, EntityKind::Mob { .. });
        let life_clock = living_mob
            && (row.breed_cooldown > 0
                || row.love > 0
                || (!row.entity_life.age_lock.locked
                    && (row.entity_life.child_ticks > 0
                        || matches!(body.kind, EntityKind::Mob { type_id }
                            if Some(type_id) == tadpole)))
                || (matches!(body.kind, EntityKind::Mob { type_id }
                        if Some(type_id) == chicken)
                    && adult(row)
                    && egg.is_some()));
        if !life_clock && row.appearance.arrow_count <= 0 && row.appearance.stinger_count <= 0 {
            continue;
        }
        let pos = state.entities.pos(slot);
        let column = (
            mc_physics::mth::floor_i32(pos[0]).div_euclid(16),
            mc_physics::mth::floor_i32(pos[2]).div_euclid(16),
        );
        let active = match active_column {
            Some((previous, active)) if previous == column => active,
            _ => {
                let active = state.world.is_active(column.0, column.1);
                active_column = Some((column, active));
                active
            }
        };
        if !active {
            continue;
        }
        // Deterministic lifecycle clocks are replay projections, not dirty
        // state. Only events that differ from that projection need custody.
        if row.appearance.arrow_count > 0 || row.appearance.stinger_count > 0 {
            let id = state.entities.body(slot).id;
            let row = state.entities.row_mut_unmarked(slot);
            if crate::projectiles::embedded_projectiles::advance(row) {
                state.appearance_changed.push(id);
                let _ = state.entities.row_mut(slot);
            }
        }
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if !body.alive || !matches!(body.kind, EntityKind::Mob { .. }) {
            continue;
        }
        let tadpole_clock = matches!(body.kind, EntityKind::Mob { type_id }
            if Some(type_id) == tadpole)
            && !row.entity_life.age_lock.locked;
        let mut changed = false;
        let mut half_width = None;
        if row.breed_cooldown > 0
            || row.love > 0
            || (row.entity_life.child_ticks > 0 && !row.entity_life.age_lock.locked)
            || tadpole_clock
        {
            let row = state.entities.row_mut_unmarked(slot);
            if tadpole_clock {
                row.entity_life.age_lock.tadpole_age =
                    row.entity_life.age_lock.tadpole_age.saturating_add(1);
            }
            row.breed_cooldown = row.breed_cooldown.saturating_sub(1);
            row.love = row.love.saturating_sub(1);
            if row.entity_life.child_ticks > 0 && !row.entity_life.age_lock.locked {
                row.entity_life.child_ticks -= 1;
                row.love = 0;
                if row.entity_life.child_ticks == 0 {
                    Arc::make_mut(&mut row.appearance).baby = false;
                    let body = state.entities.body_mut(slot);
                    half_width = Some(body.half_width * 2.0);
                    body.height *= 2.0;
                    state.appearance_changed.push(body.id);
                    changed = true;
                }
            }
        }
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if let EntityKind::Mob { type_id } = body.kind
            && Some(type_id) == chicken
            && adult(row)
            && let Some(egg) = egg
        {
            let id = body.id;
            let remaining = row.entity_life.harvest_ticks;
            let next = if remaining <= 1 {
                egg_interval(id, state.tick)
            } else {
                remaining - 1
            };
            state.entities.set_harvest_ticks(slot, next);
            if remaining <= 1 {
                if remaining == 1 {
                    state.merge_life_drops.push((pos, ItemStack::new(egg, 1)));
                }
                changed = true;
            }
        }
        if let Some(half_width) = half_width {
            state.entities.set_half_width(slot, half_width);
        }
        if changed {
            let _ = state.entities.row_mut(slot);
        }
    }
    slots.clear();
    state.serial_entity_slots = slots;
}

pub fn commit_drops(state: &mut TickState, spawned: &mut Vec<EntityId>) {
    for (pos, stack) in std::mem::take(&mut state.merge_life_drops) {
        spawned.push(spawn_item_entity(state, pos, stack));
    }
}

/// Vanilla retained keys established by the old animal state and item codec.
pub fn to_fields(life: &EntityLifeState, love: u32) -> Vec<(String, mc_nbt::Tag)> {
    use mc_nbt::Tag;
    let mut fields = vec![
        ("InLove".into(), Tag::Int(love as i32)),
        ("EggLayTime".into(), Tag::Int(life.harvest_ticks as i32)),
        ("Tame".into(), Tag::Byte(i8::from(life.tamed))),
        ("Sitting".into(), Tag::Byte(i8::from(life.sitting))),
        (
            "AgeLocked".into(),
            Tag::Byte(i8::from(life.age_lock.locked)),
        ),
        (
            "solaris:AgeLockCooldownUntil".into(),
            Tag::Long(life.age_lock.cooldown_until.min(i64::MAX as u64) as i64),
        ),
    ];
    if life.brown_mooshroom {
        fields.push(("Type".into(), Tag::String("brown".into())));
    }
    if !life.stew_effects.is_empty() {
        fields.push((
            "stew_effects".into(),
            mc_data::item_stack::encode_stew_effects(&life.stew_effects),
        ));
    }
    fields.extend(crate::mounts::persistence::to_fields(life));
    fields.extend(crate::portals::fields(life.portal));
    fields
}

/// OLD default_breeding_state includes sterile mules as ageable animals.
pub(crate) fn old_ageable(name: &str) -> bool {
    matches!(
        name,
        "minecraft:sheep"
            | "minecraft:cow"
            | "minecraft:mooshroom"
            | "minecraft:chicken"
            | "minecraft:pig"
            | "minecraft:goat"
            | "minecraft:rabbit"
            | "minecraft:camel"
            | "minecraft:armadillo"
            | "minecraft:axolotl"
            | "minecraft:bee"
            | "minecraft:cat"
            | "minecraft:wolf"
            | "minecraft:horse"
            | "minecraft:donkey"
            | "minecraft:mule"
            | "minecraft:llama"
            | "minecraft:trader_llama"
            | "minecraft:ocelot"
            | "minecraft:fox"
            | "minecraft:hoglin"
            | "minecraft:strider"
            | "minecraft:dolphin"
            | "minecraft:nautilus"
            | "minecraft:zombie_nautilus"
            | "minecraft:happy_ghast"
            | "minecraft:sniffer"
            | "minecraft:panda"
            | "minecraft:polar_bear"
            | "minecraft:turtle"
    )
}

pub fn from_record(
    record: &mc_data::entity_nbt::VanillaEntity,
    ageable: bool,
) -> (EntityLifeState, u32) {
    use mc_nbt::Tag;
    let mut life = EntityLifeState::default();
    if ageable && record.age < 0 {
        life.child_ticks = record.age.unsigned_abs();
    }
    let mut love = 0;
    for (key, tag) in &record.extra_fields {
        match (key.as_str(), tag) {
            ("InLove", Tag::Int(ticks)) => love = (*ticks).max(0) as u32,
            ("EggLayTime", Tag::Int(ticks)) => life.harvest_ticks = (*ticks).max(0) as u32,
            ("Type", Tag::String(kind)) => life.brown_mooshroom = kind == "brown",
            ("Tame", Tag::Byte(tamed)) => life.tamed = *tamed != 0,
            ("Owner", Tag::IntArray(owner)) if owner.len() == 4 => life.tamed = true,
            ("Sitting", Tag::Byte(sitting)) => life.sitting = *sitting != 0,
            ("stew_effects", tag) => {
                if let Ok(effects) = mc_data::item_stack::decode_stew_effects(tag) {
                    life.stew_effects = effects;
                }
            }
            _ => {}
        }
    }
    (life, love)
}
