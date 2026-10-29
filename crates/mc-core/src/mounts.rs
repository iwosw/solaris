//! Native mount admission, exact-stack equipment transactions and rider motion.
use crate::boundary::{ChangeSet, ConnectionId, EntityId, Hand, PlayerChange};
use crate::entities::{EntityKind, EntitySlot};
use crate::intent::{Footprint, Intent, IntentKind, InvClaim};
use crate::tick::TickState;
use mc_data::ItemStack;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use uuid::Uuid;
pub(crate) mod menus;
pub(crate) mod persistence;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MountState {
    pub rider: Option<Uuid>,
    #[serde(skip)]
    pub input: u8,
    /// Reverse link is restart-local; the retained vehicle owns its rider UUID.
    #[serde(skip)]
    pub vehicle: Option<EntityId>,
    pub cargo: Vec<ItemStack>,
    pub chested: bool,
    /// Llama cargo width; zero uses the ordinary one-column minimum.
    pub strength: u8,
}
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Taming(crate::taming::Action),
    Leash(crate::leash::Action),
    Saddle,
    Armor,
    Repair,
    Strip,
    Unsaddle,
    Ride,
    Dismount,
    Inventory,
    Chest,
}
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub target: EntityId,
    pub slot: usize,
    pub held: ItemStack,
    pub secondary: bool,
    pub action: Action,
}
pub(crate) fn type_id(body: &crate::entities::Body) -> Option<u32> {
    match body.kind {
        EntityKind::Mob { type_id } => Some(type_id),
        _ => None,
    }
}
pub(crate) fn name(state: &TickState, slot: EntitySlot) -> &'static str {
    type_id(state.entities.body(slot))
        .and_then(mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id)
        .map_or("", |contract| contract.name)
}
fn capabilities(state: &TickState, slot: EntitySlot) -> crate::blueprint::Capabilities {
    type_id(state.entities.body(slot))
        .and_then(|key| state.registry.blueprint(key))
        .map(|blueprint| blueprint.capabilities)
        .unwrap_or_default()
}
pub(crate) fn vehicle(state: &TickState, actor: EntityId) -> Option<EntitySlot> {
    let actor = state.entities.row(state.entities.slot_of(actor)?);
    let slot = state
        .entities
        .slot_of_any(actor.entity_life.mount.vehicle?)?;
    (state.entities.row(slot).entity_life.mount.rider == Some(actor.uuid)).then_some(slot)
}
pub(crate) fn join(state: &mut TickState, actor: EntityId) {
    let Some(actor_slot) = state.entities.slot_of(actor) else {
        return;
    };
    let uuid = state.entities.row(actor_slot).uuid;
    let vehicle = state
        .entities
        .slots()
        .find(|slot| state.entities.row(*slot).entity_life.mount.rider == Some(uuid));
    if let Some(vehicle) = vehicle {
        sync_rider(state, vehicle);
    }
}
pub(crate) fn rider(state: &TickState, slot: EntitySlot) -> Option<EntitySlot> {
    state
        .entities
        .slot_of_uuid(state.entities.row(slot).entity_life.mount.rider?)
}
pub(crate) fn vehicle_input(state: &TickState, slot: EntitySlot) -> u8 {
    rider(state, slot).map_or(0, |r| state.entities.row(r).entity_life.mount.input)
}
fn action(
    state: &TickState,
    actor: EntityId,
    target: EntitySlot,
    held: &ItemStack,
    secondary: bool,
) -> Option<Action> {
    use crate::blueprint::RidingProfile;
    let body = state.entities.body(target);
    let row = state.entities.row(target);
    if !body.alive {
        return None;
    }
    let uuid = state.entities.row(state.entities.slot_of(actor)?).uuid;
    let kind = name(state, target);
    let item = if held.is_empty() {
        ""
    } else {
        state.registry.items.name_of(held.item_id)?.as_str()
    };
    let capabilities = type_id(body)
        .and_then(|id| state.registry.blueprint(id))
        .map(|blueprint| blueprint.capabilities)
        .unwrap_or_default();
    if let Some(action) = crate::leash::action(row, uuid, capabilities.leashable, item, held) {
        return Some(Action::Leash(action));
    }
    let adult = crate::entity_life::adult(row);
    let owner = row.entity_life.taming.owner == Some(uuid);
    if adult && row.entity_life.tamed && menus::kind(state, target).is_some() {
        if secondary {
            return Some(Action::Inventory);
        }
        if item == "minecraft:chest"
            && matches!(
                kind,
                "minecraft:donkey"
                    | "minecraft:mule"
                    | "minecraft:llama"
                    | "minecraft:trader_llama"
            )
            && !row.entity_life.mount.chested
        {
            return Some(Action::Chest);
        }
    }
    if adult {
        if item == "minecraft:saddle"
            && row.loot.saddle.is_none()
            && ((capabilities.riding == RidingProfile::TamedSaddle && row.entity_life.tamed)
                || matches!(
                    capabilities.riding,
                    RidingProfile::Saddle | RidingProfile::CarrotStick | RidingProfile::FungusStick
                ))
        {
            return Some(Action::Saddle);
        }
        if item == "minecraft:wolf_armor"
            && kind == "minecraft:wolf"
            && row.entity_life.tamed
            && owner
            && row.loot.body_armor.is_none()
        {
            return Some(Action::Armor);
        }
        if item.ends_with("_horse_armor")
            && matches!(
                item,
                "minecraft:leather_horse_armor"
                    | "minecraft:copper_horse_armor"
                    | "minecraft:iron_horse_armor"
                    | "minecraft:golden_horse_armor"
                    | "minecraft:diamond_horse_armor"
                    | "minecraft:netherite_horse_armor"
            )
            && capabilities.equine_body_armor
            && row.entity_life.tamed
            && row.loot.body_armor.is_none()
        {
            return Some(Action::Armor);
        }
        if item == "minecraft:armadillo_scute"
            && kind == "minecraft:wolf"
            && owner
            && row
                .loot
                .body_armor
                .as_ref()
                .is_some_and(|s| s.damage.unwrap_or(0) > 0)
        {
            return Some(Action::Repair);
        }
        if item == "minecraft:shears"
            && row.loot.body_armor.is_some()
            && ((kind == "minecraft:wolf" && owner) || capabilities.equine_body_armor)
        {
            return Some(Action::Strip);
        }
        if secondary && item == "minecraft:shears" && row.loot.saddle.is_some() {
            return Some(Action::Unsaddle);
        }
    }
    if secondary && row.entity_life.mount.rider == Some(uuid) {
        return Some(Action::Dismount);
    }
    let food = !held.is_empty() && crate::taming::food(state, capabilities.food, held.item_id);
    if let Some(action) =
        crate::taming::action(state, uuid, body, row, capabilities.taming, item, food)
    {
        return Some(Action::Taming(action));
    }
    if adult
        && !secondary
        && !food
        && (capabilities.riding != RidingProfile::None || crate::vehicles::rideable(state, target))
        && row.entity_life.mount.rider.is_none()
        && vehicle(state, actor).is_none()
        && (!matches!(
            capabilities.riding,
            RidingProfile::CarrotStick | RidingProfile::FungusStick
        ) || row.loot.saddle.is_some())
    {
        return Some(Action::Ride);
    }
    None
}
pub(crate) fn interact(
    state: &mut TickState,
    actor: EntityId,
    target: EntityId,
    hand: Hand,
    secondary: bool,
    intents: &mut Vec<Intent>,
) -> bool {
    let Some(slot) = state.entities.slot_of(target) else {
        return false;
    };
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.spectator() || player.vitals.is_dead() {
        return false;
    }
    let inv_slot = match hand {
        Hand::Main => usize::from(player.inventory.selected.min(8)),
        Hand::Off => crate::inventory::OFF_HAND,
    };
    let held = player.inventory.slots[inv_slot].clone();
    if crate::leash::knot_interact(state, slot) {
        return true;
    }
    let Some(action) = action(state, actor, slot, &held, secondary) else {
        return false;
    };
    let (expect_item, expect_count) = (held.item_id, held.count);
    let plan = Plan {
        target,
        slot: inv_slot,
        held,
        secondary,
        action,
    };
    let seq = state.next_seq(actor);
    intents.push(Intent {
        actor,
        seq,
        kind: IntentKind::MountInteract { plan },
        footprint: Footprint {
            entity_reads: vec![actor],
            entity_writes: vec![target],
            inv: vec![InvClaim {
                owner: actor,
                slot: inv_slot as u16,
                expect_item,
                expect_count,
                take: 0,
                put_item: None,
                put_count: 0,
            }],
            ..Default::default()
        },
    });
    true
}
pub(crate) fn valid(state: &TickState, actor: EntityId, plan: &Plan) -> bool {
    state.players.get(&actor).is_some_and(|p| {
        !p.spectator()
            && !p.vitals.is_dead()
            && p.inventory.slots.get(plan.slot) == Some(&plan.held)
            && !below_native_cost_floors(state, actor, plan)
    }) && state.entities.slot_of(plan.target).is_some_and(|s| {
        action(state, actor, s, &plan.held, plan.secondary).as_ref() == Some(&plan.action)
    })
}

fn below_native_cost_floors(state: &TickState, actor: EntityId, plan: &Plan) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.creative() {
        return false;
    }
    let consume = match &plan.action {
        Action::Taming(action) => !matches!(action, crate::taming::Action::Sit),
        Action::Leash(action) => !matches!(action, crate::leash::Action::Detach),
        Action::Saddle | Action::Armor | Action::Repair | Action::Chest => true,
        _ => false,
    };
    let wear = matches!(plan.action, Action::Strip | Action::Unsaddle);
    let rules = state.registry.item_rules();
    let breaks = wear
        && rules
            .facts
            .facts_for_stack(&plan.held, rules.items)
            .and_then(|facts| facts.max_damage)
            .is_some_and(|max| plan.held.damage.unwrap_or(0).saturating_add(1) as u32 >= max);
    let debit = [(plan.slot, 1)];
    crate::plugin_inventories::below_player_debit_floors(
        state,
        actor,
        if consume { &debit } else { &[] },
        breaks.then_some(plan.slot),
    )
}
pub(crate) fn apply(state: &mut TickState, actor: EntityId, plan: &Plan) {
    let slot = state
        .entities
        .slot_of(plan.target)
        .expect("validated mount");
    let uuid = state
        .entities
        .row(state.entities.slot_of(actor).expect("validated actor"))
        .uuid;
    let mut consume = false;
    let mut wear = false;
    match &plan.action {
        Action::Inventory => {
            menus::open(state, actor, plan.target);
            return;
        }
        Action::Chest => {
            let columns = if matches!(
                name(state, slot),
                "minecraft:llama" | "minecraft:trader_llama"
            ) {
                state
                    .entities
                    .row(slot)
                    .entity_life
                    .mount
                    .strength
                    .clamp(1, 5)
            } else {
                5
            };
            let mount = &mut state.entities.row_mut(slot).entity_life.mount;
            mount.chested = true;
            mount
                .cargo
                .resize(usize::from(columns) * 3, ItemStack::EMPTY);
            consume = true;
        }
        Action::Taming(action) => {
            consume = !matches!(action, crate::taming::Action::Sit);
            crate::taming::apply(state, slot, uuid, action);
        }
        Action::Leash(action) => {
            consume = !matches!(action, crate::leash::Action::Detach);
            crate::leash::apply(state, slot, uuid, action, &plan.held);
        }
        Action::Saddle | Action::Armor => {
            let mut stack = plan.held.clone();
            stack.count = 1;
            let row = state.entities.row_mut(slot);
            if plan.action == Action::Saddle {
                crate::entities::set_boxed(&mut row.loot.saddle, stack);
            } else {
                crate::entities::set_boxed(&mut row.loot.body_armor, stack);
            }
            consume = true;
        }
        Action::Repair => {
            let armor = state
                .entities
                .row_mut(slot)
                .loot
                .body_armor
                .as_mut()
                .expect("validated armor");
            armor.damage = Some(armor.damage.unwrap_or(0).saturating_sub(8)).filter(|d| *d > 0);
            consume = true;
        }
        Action::Strip | Action::Unsaddle => {
            let row = state.entities.row_mut(slot);
            let stack = if plan.action == Action::Strip {
                row.loot.body_armor.take()
            } else {
                row.loot.saddle.take()
            };
            if let Some(stack) = stack {
                state
                    .merge_life_drops
                    .push((state.entities.pos(slot), *stack));
            }
            wear = true;
        }
        Action::Ride => {
            state.entities.set_rider(slot, Some(uuid));
            state.entities.row_mut(slot).entity_life.mounted = true;
            state.entities.set_vel(slot, [0.0; 3]);
            sync_rider(state, slot);
            crate::advancements::trigger(
                state,
                actor,
                "minecraft:started_riding",
                serde_json::json!({}),
            );
        }
        Action::Dismount => dismount(state, actor),
    }
    let rules = state.registry.item_rules();
    let player = state.players.get_mut(&actor).expect("validated player");
    if wear {
        player.statistics.increment(
            mc_data::statistics::StatType::Used,
            player.inventory.slots[plan.slot].item_id,
            1,
        );
    }
    if !player.creative() {
        let held = &mut player.inventory.slots[plan.slot];
        if consume {
            held.count -= 1;
            if held.count <= 0 {
                *held = ItemStack::EMPTY;
            }
        }
        if wear
            && let Some(max) = rules
                .facts
                .facts_for_stack(held, rules.items)
                .and_then(|f| f.max_damage)
        {
            let damage = held.damage.unwrap_or(0).saturating_add(1);
            if damage as u32 >= max {
                player
                    .statistics
                    .increment(mc_data::statistics::StatType::Broken, held.item_id, 1);
                *held = ItemStack::EMPTY;
            } else {
                held.damage = Some(damage);
            }
        }
    }
    let contents = player.inventory_contents();
    if let Some(c) = state.connections.entity_connection(actor) {
        state.push_player(c, contents);
    }
    let row = state.entities.row_mut(slot);
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.saddle = row.loot.saddle.as_deref().cloned();
    appearance.body_armor = row.loot.body_armor.as_deref().cloned();
    appearance.health = row.health;
    state.appearance_changed.push(plan.target);
    if menus::kind(state, slot).is_some() {
        menus::sync(state, plan.target);
    }
}
pub(crate) fn death(state: &mut TickState, slot: EntitySlot) {
    if let Some(rider) = rider(state, slot) {
        dismount(state, state.entities.body(rider).id);
    }
    let position = state.entities.pos(slot);
    let mount = &mut state.entities.row_mut(slot).entity_life.mount;
    let chested = std::mem::take(&mut mount.chested);
    let cargo = std::mem::take(&mut mount.cargo);
    state.merge_life_drops.extend(
        cargo
            .into_iter()
            .filter(|stack| !stack.is_empty())
            .map(|stack| (position, stack)),
    );
    if chested
        && let Some(item) = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:chest").unwrap())
    {
        state
            .merge_life_drops
            .push((position, ItemStack::new(item, 1)));
    }
}
pub(crate) fn status(state: &mut TickState, entity: EntityId, event: i8) {
    let connections: Vec<_> = state.connections.all_connections().collect();
    for connection in connections {
        state.push_player(connection, PlayerChange::ProjectileStatus { entity, event });
    }
}
fn clear_rider(state: &mut TickState, slot: EntitySlot) {
    let vehicle = state.entities.row(slot);
    let uuid = vehicle.entity_life.mount.rider;
    let rider = rider(state, slot).or_else(|| {
        let passenger = *vehicle.appearance.passengers.first()?;
        let slot = state.entities.slot_of_any(passenger)?;
        (Some(state.entities.row(slot).uuid) == uuid).then_some(slot)
    });
    if let Some(rider) = rider {
        if let Some(player) = state.players.get_mut(&state.entities.body(rider).id) {
            player.vehicle_airtime_ticks = 0;
            player.vehicle_last_reported_y = None;
        }
        let mount = &mut state.entities.row_mut(rider).entity_life.mount;
        mount.vehicle = None;
        mount.input = 0;
    }
    state.entities.set_rider(slot, None);
    let (body, row) = state.entities.parts_mut(slot);
    row.entity_life.mounted = false;
    Arc::make_mut(&mut row.appearance).passengers.clear();
    state.appearance_changed.push(body.id);
}

pub(crate) fn reset_player(state: &mut TickState, actor: EntityId) {
    let Some(actor_slot) = state.entities.slot_of_any(actor) else {
        return;
    };
    let row = state.entities.row(actor_slot);
    if let Some(vehicle) = row.entity_life.mount.vehicle
        && let Some(slot) = state.entities.slot_of_any(vehicle)
        && state.entities.row(slot).entity_life.mount.rider == Some(row.uuid)
    {
        clear_rider(state, slot);
    }
    let mount = &mut state.entities.row_mut(actor_slot).entity_life.mount;
    mount.vehicle = None;
    mount.input = 0;
}

pub(crate) fn dismount(state: &mut TickState, actor: EntityId) {
    let Some(slot) = vehicle(state, actor) else {
        return;
    };
    clear_rider(state, slot);
    if let Some(rider) = state.entities.slot_of(actor) {
        let pos = state.entities.pos(slot);
        let width = state.entities.body(slot).half_width + 0.8;
        let sampler = crate::tick::TickSampler {
            world: &state.world,
            registry: &state.registry,
        };
        let resolved = mc_physics::resolve_collision_displacement(
            &sampler,
            mc_physics::Vec3::new(pos[0], pos[1], pos[2]),
            mc_physics::Aabb {
                half_width: 0.3,
                height: 1.8,
            },
            mc_physics::Vec3::new(width, 0.0, 0.0),
            state.entities.ground(slot),
            mc_physics::STEP_HEIGHT,
        );
        let p = resolved.map_or(
            [pos[0], pos[1] + state.entities.body(slot).height, pos[2]],
            |r| [pos[0] + r.delta.x, pos[1] + r.delta.y, pos[2] + r.delta.z],
        );
        state.entities.set_pos(rider, p);
        if let Some(connection) = state.connections.entity_connection(actor) {
            let rot = state.entities.rot(rider);
            state.correct_position(connection, p, rot[0], rot[1]);
        }
    }
}
pub(crate) fn sync_rider(state: &mut TickState, slot: EntitySlot) {
    let Some(rider) = rider(state, slot) else {
        return;
    };
    let id = state.entities.body(rider).id;
    if state.entity_session_locked(id) {
        return;
    }
    let old = state.entities.pos(rider);
    let already_mounted = state.entities.row(slot).appearance.passengers.as_slice() == [id];
    state.entities.row_mut(rider).entity_life.mount.vehicle = Some(state.entities.body(slot).id);
    let pos = state.entities.pos(slot);
    let (height, forward) =
        if crate::vehicles::kind(state, slot) == Some(crate::vehicles::VehicleKind::Boat) {
            // 26.3 Boat/ChestBoat.rideHeight and Raft/ChestRaft.rideHeight,
            // minus the player's VEHICLE attachment (0.6). Only players ride here.
            let kind = name(state, slot);
            let body_height = state.entities.body(slot).height as f32;
            let attachment = if kind.ends_with("_raft") {
                body_height * 0.888_888_9_f32
            } else {
                body_height / 3.0
            };
            let forward = if kind.contains("_chest_") {
                f64::from(0.15_f32)
            } else {
                0.0
            };
            (f64::from(attachment) - f64::from(0.6_f32), forward)
        } else {
            (state.entities.body(slot).height * 0.75, 0.0)
        };
    let yaw = f64::from(state.entities.rot(slot)[0]).to_radians();
    state.entities.set_pos(
        rider,
        [
            pos[0] - forward * f64::from(mc_physics::mth::sin(yaw)),
            pos[1] + height,
            pos[2] + forward * f64::from(mc_physics::mth::cos(yaw)),
        ],
    );
    state.entities.set_vel(rider, [0.0; 3]);
    if already_mounted {
        let new = state.entities.pos(rider);
        crate::statistics::riding(state, id, slot, old, new);
    }
    let (body, row) = state.entities.parts_mut(slot);
    if row.appearance.passengers.as_slice() != [id] {
        Arc::make_mut(&mut row.appearance).passengers = vec![id];
        state.appearance_changed.push(body.id);
    }
}
fn controllable(state: &TickState, slot: EntitySlot, actor: EntityId) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    if player.spectator() || player.vitals.is_dead() {
        return false;
    }
    if let Some(kind) = crate::vehicles::kind(state, slot) {
        return kind == crate::vehicles::VehicleKind::Boat;
    }
    let row = state.entities.row(slot);
    let capabilities = capabilities(state, slot);
    if row.loot.saddle.is_none()
        || capabilities.riding == crate::blueprint::RidingProfile::Unsteered
        || (capabilities.taming.equine() && !row.entity_life.tamed)
    {
        return false;
    }
    let stick = match capabilities.riding {
        crate::blueprint::RidingProfile::CarrotStick => "minecraft:carrot_on_a_stick",
        crate::blueprint::RidingProfile::FungusStick => "minecraft:warped_fungus_on_a_stick",
        _ => return true,
    };
    [
        usize::from(player.inventory.selected.min(8)),
        crate::inventory::OFF_HAND,
    ]
    .into_iter()
    .any(|i| {
        let held = &player.inventory.slots[i];
        !held.is_empty()
            && state
                .registry
                .items
                .name_of(held.item_id)
                .is_some_and(|n| n.as_str() == stick)
    })
}
/// Vanilla's vehicle floating scan: inflated body, extended 0.55 blocks down.
/// Any non-air block (including water) interrupts floating; missing cells are
/// conservative obstacles, as they are for native movement collision.
fn no_blocks_around(state: &TickState, slot: EntitySlot) -> bool {
    let p = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let min = [
        (p[0] - body.half_width - 0.0625).floor() as i32,
        (p[1] - 0.0625 - 0.55).floor() as i32,
        (p[2] - body.half_width - 0.0625).floor() as i32,
    ];
    let max = [
        (p[0] + body.half_width + 0.0625).floor() as i32,
        (p[1] + body.height + 0.0625).floor() as i32,
        (p[2] + body.half_width + 0.0625).floor() as i32,
    ];
    for x in min[0]..=max[0] {
        for y in min[1]..=max[1] {
            for z in min[2]..=max[2] {
                let air = state
                    .world
                    .block_at(x, y, z)
                    .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                    .is_some_and(|block| {
                        matches!(block.block.id.path(), "air" | "cave_air" | "void_air")
                    });
                if !air {
                    return false;
                }
            }
        }
    }
    true
}

fn excessive_boat_floating(
    state: &mut TickState,
    actor: EntityId,
    slot: EntitySlot,
    old_y: f64,
    reported_y: f64,
) -> bool {
    if crate::vehicles::kind(state, slot) != Some(crate::vehicles::VehicleKind::Boat) {
        return false;
    }
    // Native gravity may have moved the body since the previous report. Compare
    // consecutive client heights, not a descending report to that lower body.
    let dy = reported_y
        - state.players[&actor]
            .vehicle_last_reported_y
            .unwrap_or(old_y);
    let floating = state.max_airtime_ticks != 0 && dy >= -0.03125 && no_blocks_around(state, slot);
    let player = state.players.get_mut(&actor).expect("live rider");
    player.vehicle_last_reported_y = Some(reported_y);
    player.vehicle_airtime_ticks = if floating {
        player.vehicle_airtime_ticks.saturating_add(1)
    } else {
        0
    };
    // 26.3 getMaximumFlyingTicks: base * max(0.08 / gravity, 1).
    // Native boats, like AbstractBoat.getDefaultGravity, use 0.04.
    player.vehicle_airtime_ticks > state.max_airtime_ticks.saturating_mul(2)
}

pub(crate) fn move_vehicle(
    state: &mut TickState,
    connection: ConnectionId,
    position: [f64; 3],
    yaw: f32,
    pitch: f32,
    on_ground: bool,
) {
    let Some(actor) = state.connections.entity_of(connection) else {
        return;
    };
    let Some(slot) = vehicle(state, actor) else {
        return;
    };
    let Some(player) = state.players.get_mut(&actor) else {
        return;
    };
    // Fence before validation too: malformed packet floods must not multiply
    // collision work or correction messages, nor reset the displacement budget.
    if player.last_vehicle_move_tick == Some(state.tick) {
        return;
    }
    player.last_vehicle_move_tick = Some(state.tick);
    let old = state.entities.pos(slot);
    let delta = [
        position[0] - old[0],
        position[1] - old[1],
        position[2] - old[2],
    ];
    let valid = position.iter().all(|x| x.is_finite())
        && yaw.is_finite()
        && pitch.is_finite()
        && delta.iter().map(|d| d * d).sum::<f64>() <= 100.0
        && controllable(state, slot, actor);
    let mut floating_too_long = false;
    if valid {
        let body = state.entities.body(slot);
        let sampler = crate::tick::TickSampler {
            world: &state.world,
            registry: &state.registry,
        };
        if let Some(r) = mc_physics::resolve_collision_displacement(
            &sampler,
            mc_physics::Vec3::new(old[0], old[1], old[2]),
            mc_physics::Aabb {
                half_width: body.half_width,
                height: body.height,
            },
            mc_physics::Vec3::new(delta[0], delta[1], delta[2]),
            on_ground,
            mc_physics::STEP_HEIGHT,
        ) {
            state.entities.set_pos(
                slot,
                [old[0] + r.delta.x, old[1] + r.delta.y, old[2] + r.delta.z],
            );
            state.entities.set_rot(slot, yaw, pitch);
            state.entities.set_ground(slot, r.grounded);
            // Airborne boats still run native gravity after client admission.
            // Retain its accumulated vertical momentum across accepted packets.
            let velocity_y =
                if crate::vehicles::kind(state, slot) == Some(crate::vehicles::VehicleKind::Boat) {
                    state.entities.vel(slot)[1]
                } else {
                    0.0
                };
            state.entities.set_vel(slot, [0.0, velocity_y, 0.0]);
            // Accepted client steering is not a correction. Echoing every
            // accepted boat packet resets prediction and generates an echo loop.
            if r.delta.x == delta[0] && r.delta.y == delta[1] && r.delta.z == delta[2] {
                if excessive_boat_floating(state, actor, slot, old[1], position[1]) {
                    state.entities.set_pos(slot, old);
                    floating_too_long = true;
                } else {
                    state
                        .players
                        .get_mut(&actor)
                        .expect("live rider")
                        .last_vehicle_motion_tick = Some(state.tick);
                    sync_rider(state, slot);
                    return;
                }
            }
        }
    }
    sync_rider(state, slot);
    let position = state.entities.pos(slot);
    let rot = state.entities.rot(slot);
    state.push_player(
        connection,
        PlayerChange::VehicleCorrected {
            position,
            yaw: rot[0],
            pitch: rot[1],
        },
    );
    if floating_too_long {
        state.push_player(
            connection,
            PlayerChange::Disconnect {
                reason: "Flying is not enabled on this server".into(),
            },
        );
    }
}
pub(crate) fn tick(state: &mut TickState, changes: &mut ChangeSet) {
    let riders: Vec<_> = state.entities.rider_slots().collect();
    for slot in riders {
        let Some(uuid) = state.entities.row(slot).entity_life.mount.rider else {
            continue;
        };
        let mpos = state.entities.pos(slot);
        if !state.world.is_active(
            (mpos[0].floor() as i32).div_euclid(16),
            (mpos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let live = rider(state, slot).is_some_and(|r| {
            let body = state.entities.body(r);
            body.alive
                && state
                    .players
                    .get(&body.id)
                    .is_some_and(|p| !p.spectator() && !p.vitals.is_dead())
        });
        if !state.entities.body(slot).alive || !live {
            clear_rider(state, slot);
            continue;
        }
        let equine = capabilities(state, slot).taming.equine();
        let row = state.entities.row_mut(slot);
        row.entity_life.mounted = true;
        if equine && !row.entity_life.tamed && crate::taming::roll(row, 50) == 0 {
            let success = crate::taming::roll(row, 100) < u32::from(row.entity_life.taming.temper);
            if success {
                row.entity_life.tamed = true;
                row.entity_life.taming.owner = Some(uuid);
                row.persistent = true;
            } else {
                row.entity_life.taming.temper =
                    row.entity_life.taming.temper.saturating_add(5).min(100);
                let actor = state
                    .entities
                    .body(rider(state, slot).expect("live rider"))
                    .id;
                dismount(state, actor);
            }
            crate::taming::sync(state, slot);
            status(
                state,
                state.entities.body(slot).id,
                if success { 7 } else { 6 },
            );
            if success {
                let player = state
                    .entities
                    .body(rider(state, slot).expect("live rider"))
                    .id;
                let entity =
                    crate::advancements::entity_snapshot(state, state.entities.body(slot).id);
                crate::advancements::trigger(
                    state,
                    player,
                    "minecraft:tame_animal",
                    serde_json::json!({ "entity": entity }),
                );
            }
        }
        sync_rider(state, slot);
    }
    crate::leash::tick(state, changes);
}

#[cfg(test)]
#[path = "mounts_tests.rs"]
pub(crate) mod tests;
