//! Tick-owned boats and rail vehicles. Containers have one durable custody image.
mod containers;
mod loot;
pub(crate) mod motion;
mod persistence;
#[cfg(test)]
#[path = "../vehicles_tests.rs"]
mod tests;

use crate::boundary::{BlockPos, ChangeSet, EntityDataValue, EntityId, Hand};
use crate::entities::{EntityKind, EntitySlot};
use crate::tick::TickState;
use mc_data::statistics::StatType;
use mc_data::{Identifier, ItemStack};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

pub use containers::VehicleMenus;
pub(crate) use containers::{
    click, container_slots_for_player, insert_one, is_open, pull_into_hopper,
};
pub(crate) use persistence::{from_record, to_fields, validate_replay};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VehicleKind {
    Boat,
    Minecart,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "persistence::VehicleImage")]
pub struct VehicleState {
    pub kind: VehicleKind,
    pub slots: Vec<ItemStack>,
    pub loot_table: Option<Identifier>,
    pub loot_seed: i64,
    pub fuel: u16,
    pub push: [f64; 2],
    pub fuse: i32,
    pub explosion_power: f32,
    pub explosion_speed_factor: f32,
    pub enabled: bool,
    pub transfer_cooldown: u8,
    pub on_ground: bool,
    #[serde(skip)]
    pub paddles: [bool; 2],
}
impl VehicleState {
    fn new(name: &str) -> Option<Self> {
        Some(Self {
            kind: name_kind(name)?,
            slots: vec![ItemStack::EMPTY; container_size(name)],
            loot_table: None,
            loot_seed: 0,
            fuel: 0,
            push: [0.0; 2],
            fuse: -1,
            explosion_power: 4.0,
            explosion_speed_factor: 1.0,
            enabled: true,
            transfer_cooldown: 0,
            on_ground: false,
            paddles: [false; 2],
        })
    }
}
fn name_kind(name: &str) -> Option<VehicleKind> {
    let path = name.strip_prefix("minecraft:")?;
    if path.ends_with("_boat") || path == "bamboo_raft" || path == "bamboo_chest_raft" {
        Some(VehicleKind::Boat)
    } else if matches!(
        path,
        "minecart" | "chest_minecart" | "hopper_minecart" | "furnace_minecart" | "tnt_minecart"
    ) {
        Some(VehicleKind::Minecart)
    } else {
        None
    }
}
fn container_size(name: &str) -> usize {
    if name.contains("chest_boat")
        || name == "minecraft:bamboo_chest_raft"
        || name == "minecraft:chest_minecart"
    {
        27
    } else if name == "minecraft:hopper_minecart" {
        5
    } else {
        0
    }
}
pub(crate) fn name(state: &TickState, slot: EntitySlot) -> Option<&'static str> {
    let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return None;
    };
    mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id).map(|c| c.name)
}
pub(crate) fn kind(state: &TickState, slot: EntitySlot) -> Option<VehicleKind> {
    name_kind(name(state, slot)?)
}
pub(crate) fn rideable(state: &TickState, slot: EntitySlot) -> bool {
    kind(state, slot) == Some(VehicleKind::Boat) || name(state, slot) == Some("minecraft:minecart")
}
pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot) {
    let Some(name) = name(state, slot) else {
        return;
    };
    let Some(record) = VehicleState::new(name) else {
        return;
    };
    state.entities.row_mut(slot).entity_life.vehicle = Some(Box::new(record));
    restore(state, slot);
}
pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    if kind(state, slot).is_none() {
        return;
    }
    if state.entities.row(slot).entity_life.vehicle.is_none() {
        initialize(state, slot);
        return;
    }
    let (body, row) = state.entities.parts_mut(slot);
    body.movement = crate::blueprint::MovementClass::Stationary;
    row.persistent = true;
    sync_metadata(state, slot);
}
fn sync_metadata(state: &mut TickState, slot: EntitySlot) {
    let Some(v) = state.entities.row(slot).entity_life.vehicle.as_ref() else {
        return;
    };
    let mut extra = vec![
        (8, EntityDataValue::VarInt(0)),
        (9, EntityDataValue::VarInt(1)),
        (10, EntityDataValue::Float(0.0)),
    ];
    if v.kind == VehicleKind::Boat {
        // Entity: 0..7, VehicleEntity: 8..10, AbstractBoat: 11..13 (26.1.2 javap).
        extra.extend([
            (11, EntityDataValue::Boolean(v.paddles[0])),
            (12, EntityDataValue::Boolean(v.paddles[1])),
            (13, EntityDataValue::VarInt(0)),
        ]);
    } else if name(state, slot) == Some("minecraft:furnace_minecart") {
        // AbstractMinecart adds optional display 11 and offset 12; furnace fuel is 13.
        extra.push((13, EntityDataValue::Boolean(v.fuel > 0)));
    }
    if state.entities.row(slot).appearance.extra != extra {
        let (body, row) = state.entities.parts_mut(slot);
        Arc::make_mut(&mut row.appearance).extra = extra;
        state.appearance_changed.push(body.id);
    }
}
pub(crate) fn extend_blueprints(
    registry: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<(u32, crate::blueprint::MobBlueprint)>,
) {
    for type_id in 0..mc_data::entity_types::ENTITY_TYPE_COUNT as u32 {
        let Some(c) = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
        else {
            continue;
        };
        if name_kind(c.name).is_none() || out.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        let id = Identifier::parse(c.name).expect("canonical entity id");
        let Some(facts) = registry.facts_of(&id) else {
            continue;
        };
        out.push((
            type_id,
            crate::blueprint::MobBlueprint {
                name: c.name.to_owned(),
                type_id,
                movement: crate::blueprint::MovementClass::Stationary,
                goals: Vec::new(),
                sensors: crate::blueprint::Sensors {
                    sight_range: 0.0,
                    attack_reach: 0.0,
                },
                params: crate::blueprint::MobParams {
                    max_health: 4.0,
                    speed: 0.0,
                    attack_damage: 0.0,
                    pickup_delay: 0,
                    breed_cooldown: 0,
                    armor: 0.0,
                    knockback_resistance: 0.0,
                },
                half_width: facts.dimensions.half_width(),
                height: facts.dimensions.height,
                offspring: None,
                breed_food: Vec::new(),
                loot_table: None,
                category: crate::blueprint::MobCategory::Misc,
                wool: false,
                spawn_weight: 0,
                capabilities: crate::blueprint::Capabilities::default(),
            },
        ));
    }
}

/// Placement is serialized on the native input owner: reserve no copies of held custody.
#[allow(clippy::too_many_arguments)]
pub(crate) fn use_on(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    clicked: BlockPos,
    face: u8,
    changes: &mut ChangeSet,
) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let index = if hand == Hand::Off {
        crate::inventory::OFF_HAND
    } else {
        usize::from(player.inventory.selected.min(8))
    };
    let held = &player.inventory.slots[index];
    let Some(item) = state.registry.items.name_of(held.item_id) else {
        return false;
    };
    let name = item.as_str();
    let Some(kind) = name_kind(name) else {
        return false;
    };
    if held.is_empty() || player.spectator() || player.vitals.is_dead() {
        return true;
    }
    let Some(type_id) = state.registry.entity_types.id_of(item) else {
        return true;
    };
    let offset = crate::placement::Face::from_code(face).offset();
    let pos = if kind == VehicleKind::Minecart {
        let Some(rail) = motion::rail_at(state, clicked) else {
            return true;
        };
        rail.point(0.5)
    } else {
        let Some(block) = state.world.block_at(clicked.x, clicked.y, clicked.z) else {
            return true;
        };
        let water = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(block))
            .is_some_and(|b| b.block.id.path() == "water");
        [
            f64::from(clicked.x + if water { 0 } else { offset[0] }) + 0.5,
            f64::from(clicked.y + if water { 0 } else { offset[1] })
                + if water { 0.8 } else { 0.0 },
            f64::from(clicked.z + if water { 0 } else { offset[2] }) + 0.5,
        ]
    };
    let Some(bp) = state.registry.blueprint(type_id) else {
        return true;
    };
    if !motion::clear(state, pos, bp.half_width, bp.height)
        || state.entities.slots().any(|slot| {
            let body = state.entities.body(slot);
            let p = state.entities.pos(slot);
            body.alive
                && body.id != actor
                && !matches!(body.kind, EntityKind::Item { .. })
                && (p[0] - pos[0]).abs() < body.half_width + bp.half_width
                && (p[2] - pos[2]).abs() < body.half_width + bp.half_width
                && p[1] < pos[1] + bp.height
                && pos[1] < p[1] + body.height
        })
    {
        return true;
    }
    if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
        return true;
    }
    let custom_name = held.custom_name.as_deref().map(str::to_owned);
    let item_id = held.item_id;
    let yaw = state
        .entities
        .slot_of(actor)
        .map(|slot| state.entities.rot(slot)[0])
        .unwrap_or(0.0);
    let id = state.spawn_mob(type_id, pos, 4.0);
    if let Some(slot) = state.entities.slot_of(id) {
        state.entities.set_rot(slot, yaw, 0.0);
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name = custom_name;
    }
    let player = state.players.get_mut(&actor).expect("same input owner");
    if !player.creative() {
        mc_data::inventory_semantics_26_1_2::decrement_stack(&mut player.inventory.slots[index]);
    }
    player.statistics.increment(StatType::Used, item_id, 1);
    let contents = player.inventory_contents();
    let connection = player.connection;
    state.push_player(connection, contents);
    changes.entities_spawned.push(id);
    true
}

pub(crate) fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
    secondary: bool,
    changes: &mut ChangeSet,
) -> bool {
    let Some(slot) = state.entities.slot_of(target) else {
        return false;
    };
    let Some(name) = name(state, slot) else {
        return false;
    };
    if kind(state, slot).is_none() {
        return false;
    }
    if state
        .players
        .get(&actor)
        .is_none_or(|p| p.spectator() || p.vitals.is_dead())
    {
        return true;
    }
    if container_size(name) > 0
        && (name.ends_with("minecart") || secondary || crate::mounts::rider(state, slot).is_some())
    {
        containers::open(state, actor, target, changes);
        return true;
    }
    if name == "minecraft:furnace_minecart" {
        let p = &state.players[&actor];
        let index = if hand == Hand::Off {
            crate::inventory::OFF_HAND
        } else {
            usize::from(p.inventory.selected.min(8))
        };
        let held = &p.inventory.slots[index];
        let fuel = state
            .registry
            .items
            .name_of(held.item_id)
            .is_some_and(|i| matches!(i.path(), "coal" | "charcoal"));
        let pos = state.entities.pos(slot);
        let Some(actor_slot) = state.entities.slot_of(actor) else {
            return true;
        };
        let actor_pos = state.entities.pos(actor_slot);
        if !fuel
            || state
                .entities
                .row(slot)
                .entity_life
                .vehicle
                .as_ref()
                .expect("vehicle initialized")
                .fuel
                > 28400
        {
            return true;
        }
        if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
            return false;
        }
        let v = state
            .entities
            .row_mut(slot)
            .entity_life
            .vehicle
            .as_mut()
            .expect("vehicle initialized");
        v.fuel += 3600;
        v.push = [pos[0] - actor_pos[0], pos[2] - actor_pos[2]];
        let p = state.players.get_mut(&actor).unwrap();
        // Vanilla MinecartFurnace::interact consumes fuel without an ITEM_USED award.
        if !p.creative() {
            mc_data::inventory_semantics_26_1_2::decrement_stack(&mut p.inventory.slots[index]);
        }
        let change = p.inventory_contents();
        let connection = p.connection;
        state.push_player(connection, change);
        return true;
    }
    if name == "minecraft:tnt_minecart" {
        let p = &state.players[&actor];
        let index = if hand == Hand::Off {
            crate::inventory::OFF_HAND
        } else {
            usize::from(p.inventory.selected.min(8))
        };
        let held = &p.inventory.slots[index];
        if state
            .registry
            .items
            .name_of(held.item_id)
            .is_some_and(|i| matches!(i.path(), "flint_and_steel" | "fire_charge"))
            && crate::command_rules::boolean(state, "tnt_explodes")
        {
            if state
                .entities
                .row(slot)
                .entity_life
                .vehicle
                .as_ref()
                .unwrap()
                .fuse
                >= 0
            {
                return true;
            }
            if !p.creative() {
                let fire_charge = state
                    .registry
                    .items
                    .name_of(held.item_id)
                    .is_some_and(|item| item.path() == "fire_charge");
                let refuses = if fire_charge {
                    crate::tick::held_debit_below_floor(state, actor, hand, 1)
                } else {
                    let rules = state.registry.item_rules();
                    let max = rules
                        .facts
                        .facts_for_stack(held, rules.items)
                        .and_then(|facts| facts.max_damage)
                        .unwrap_or(64);
                    let breaks = (held.damage.unwrap_or(0) + 1) as u32 >= max;
                    crate::plugin_inventories::below_player_debit_floors(
                        state,
                        actor,
                        &[],
                        breaks.then_some(index),
                    )
                };
                if refuses {
                    return false;
                }
            }
            let v = state
                .entities
                .row_mut(slot)
                .entity_life
                .vehicle
                .as_mut()
                .unwrap();
            v.fuse = 80;
            changes
                .entity_events
                .push(crate::boundary::EntityEvent::Status {
                    entity: target,
                    event: 10,
                });
            let rules = state.registry.item_rules();
            let p = state.players.get_mut(&actor).unwrap();
            let item_id = p.inventory.slots[index].item_id;
            if !p.creative() {
                let held = &mut p.inventory.slots[index];
                if rules
                    .items
                    .name_of(held.item_id)
                    .is_some_and(|i| i.path() == "fire_charge")
                {
                    mc_data::inventory_semantics_26_1_2::decrement_stack(held);
                } else {
                    let max = rules
                        .facts
                        .facts_for_stack(held, rules.items)
                        .and_then(|f| f.max_damage)
                        .unwrap_or(64);
                    let damage = held.damage.unwrap_or(0) + 1;
                    if damage as u32 >= max {
                        *held = ItemStack::EMPTY;
                        p.statistics.increment(StatType::Broken, item_id, 1);
                    } else {
                        held.damage = Some(damage);
                    }
                }
            }
            p.statistics.increment(StatType::Used, item_id, 1);
            let change = p.inventory_contents();
            let connection = p.connection;
            state.push_player(connection, change);
        }
        return true;
    }
    false // Admission/rider links have exactly one owner: mounts.
}

/// Called before generic death loot; contents are taken, never cloned back into custody.
pub(crate) fn death(state: &mut TickState, slot: EntitySlot, spawned: &mut Vec<EntityId>) -> bool {
    if kind(state, slot).is_none() {
        return false;
    }
    if state.entities.row(slot).loot.death_handled {
        return true;
    }
    let name = name(state, slot).unwrap();
    loot::materialize(state, slot);
    let pos = state.entities.pos(slot);
    let row = state.entities.row_mut(slot);
    row.loot.death_handled = true;
    let slots = row
        .entity_life
        .vehicle
        .as_mut()
        .map(|v| {
            let count = v.slots.len();
            std::mem::replace(&mut v.slots, vec![ItemStack::EMPTY; count])
        })
        .unwrap_or_default();
    if crate::command_rules::boolean(state, "entity_drops") {
        if let Some(item) = state
            .registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
        {
            spawned.push(crate::tick::spawn_item_entity(
                state,
                pos,
                ItemStack::new(item, 1),
            ));
        }
        for stack in slots.into_iter().filter(|s| !s.is_empty()) {
            spawned.push(crate::tick::spawn_item_entity(state, pos, stack));
        }
    }
    true
}

pub(crate) fn run(
    state: &mut TickState,
    intents: &mut Vec<crate::intent::Intent>,
    changes: &mut ChangeSet,
) {
    // Module-owned bodies are excluded from parallel living motion.
    // Admission is the exact vehicle type-membership list (`name_kind`
    // classifier); only the dynamic alive filter stays per slot.
    let owned: Vec<EntitySlot> = state.entities.mob_slots_matching(|type_id| {
        mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
            .is_some_and(|contract| name_kind(contract.name).is_some())
    });
    for slot in owned {
        if !state.entities.body(slot).alive {
            continue;
        }
        let pos = state.entities.pos(slot);
        if !state.world.is_active(
            (pos[0].floor() as i32).div_euclid(16),
            (pos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let old_fuse = state
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .fuse;
        motion::step(state, slot, intents);
        let v = state
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap();
        if old_fuse < 0 && v.fuse > 0 {
            changes
                .entity_events
                .push(crate::boundary::EntityEvent::Status {
                    entity: state.entities.body(slot).id,
                    event: 10,
                });
        }
        containers::tick_hopper(state, slot, changes);
        sync_metadata(state, slot);
    }
    containers::maintain(state, changes);
}

/// Defer swap-removal until the merge owner has finished resolving slot-based intents.
pub(crate) fn queue_expired_removals(state: &TickState, slots: &mut Vec<EntitySlot>) {
    for index in 0..state.entities.len() {
        let slot = EntitySlot(index as u32);
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if !body.alive
            && row
                .entity_life
                .vehicle
                .as_ref()
                .is_some_and(|v| v.fuse == 0)
        {
            slots.push(slot);
        }
    }
}
