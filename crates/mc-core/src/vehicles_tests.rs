use super::*;
use crate::boundary::{ClickMode, ConnectionId, PlayerInput};
use crate::journal::{EntityStateImage, ReplayEntity};
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use mc_data::{Identifier, ItemStack};
use std::sync::Arc;

#[path = "boat_floating_tests.rs"]
mod boat_floating;

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.0),
            Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}
fn fixture() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 65, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    let wanted = crate::world::chunks_in_radius(mc_world::ChunkPos { x: 0, z: 0 }, 1)
        .into_iter()
        .map(|pos| (pos, 0))
        .collect();
    state.world.ensure_blocking(&wanted);
    for pos in state.world.resident_positions() {
        state
            .tickets
            .insert(format!("vehicle-test:{}:{}", pos.x, pos.z), (pos, 0));
    }
    state.refresh_active_chunks(state.tick);
    state
        .gamerules
        .insert("mob_spawning".into(), "false".into());
    state
}

#[test]
fn resting_vehicle_owners_do_not_touch_unchanged_retained_state() {
    let mut state = fixture();
    rail(&mut state, 0, 65, 0, "east_west", "rail", false);
    let cart = mob(&mut state, "chest_minecart", [0.5, 65.0625, 0.5]);
    run(&mut state, &mut Vec::new(), &mut ChangeSet::default());
    state.entities.take_touched();
    let slot = state.entities.slot_of(cart).unwrap();
    let position = state.entities.pos(slot);
    run(&mut state, &mut Vec::new(), &mut ChangeSet::default());
    assert_eq!(state.entities.pos(slot), position);
    assert!(
        !state.entities.take_touched().contains(&cart),
        "idle metadata, fuel/fuse and ground writeback remain read-only"
    );
}

fn item(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
}
fn mob(state: &mut TickState, name: &str, p: [f64; 3]) -> EntityId {
    let id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    state.spawn_mob(id, p, 4.0)
}
fn block(state: &TickState, name: &str, properties: &[(&str, &str)]) -> u32 {
    let id = Identifier::parse(format!("minecraft:{name}")).unwrap();
    let b = state.registry.blocks.block(&id).unwrap();
    b.states
        .iter()
        .copied()
        .find(|id| {
            let s = state.registry.blocks.by_id(*id).unwrap();
            properties
                .iter()
                .all(|(k, v)| s.properties.iter().any(|(key, val)| key == k && val == v))
        })
        .unwrap()
        .0
}
fn rail(state: &mut TickState, x: i32, y: i32, z: i32, shape: &str, name: &str, powered: bool) {
    let id = block(
        state,
        name,
        if name == "rail" {
            vec![("shape", shape)]
        } else {
            vec![
                ("shape", shape),
                ("powered", if powered { "true" } else { "false" }),
            ]
        }
        .as_slice(),
    );
    let stone = block(state, "stone", &[]);
    state.world.set_block(x, y - 1, z, stone);
    state.world.set_block(x, y, z, id);
}
fn player(state: &mut TickState, p: [f64; 3]) -> EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "vehicle-test".into(),
            permission_level: 0,
        }],
    );
    let id = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, p);
    id
}
fn native(state: &TickState, id: EntityId) -> mc_data::entity_nbt::VanillaEntity {
    let (_, regions, _) = crate::core::checkpoint_data_for(state, state.tick);
    regions
        .values()
        .flat_map(|r| &r.entities)
        .find(|e| e.wire_id == id.0)
        .unwrap()
        .clone()
}
fn restore(state: &mut TickState, record: &mc_data::entity_nbt::VanillaEntity) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(&record.id).unwrap())
        .unwrap();
    let image = EntityStateImage::from_record(&state.registry, record).unwrap();
    let replay = ReplayEntity {
        uuid: record.uuid.to_string(),
        type_id,
        pos: record.pos,
        vel: record.motion.map(|v| v * 20.0),
        yaw: record.yaw,
        pitch: record.pitch,
        ground: record.on_ground,
        health: record.health,
        alive: true,
        age: image.age,
        breed_cooldown: image.breed_cooldown,
        pickup_delay: 0,
        item: None,
        appearance: crate::journal::EntityAppearanceImage {
            health: record.health,
            ..Default::default()
        },
        active_effects: Default::default(),
        villager: None,
        state: image,
        projectile: None,
        tick: 0,
    };
    crate::core::restore_entity(state, record.wire_id, &replay).unwrap();
    EntityId(record.wire_id)
}

#[test]
fn every_boat_and_raft_places_exact_type_debits_once_and_breaks_to_one_item() {
    let mut state = fixture();
    let actor = player(&mut state, [-3.0, 65.0, 0.5]);
    let vehicles: Vec<_> = state
        .registry
        .items
        .iter()
        .filter(|(name, _)| name_kind(name.as_str()) == Some(VehicleKind::Boat))
        .map(|(name, id)| (name.clone(), id))
        .collect();
    assert!(
        vehicles
            .iter()
            .any(|(name, _)| name.path() == "bamboo_raft")
    );
    assert!(
        vehicles
            .iter()
            .any(|(name, _)| name.path() == "bamboo_chest_raft")
    );
    for (name, item_id) in vehicles {
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(item_id, 1);
        let mut changes = ChangeSet::default();
        assert!(use_on(
            &mut state,
            actor,
            Hand::Main,
            BlockPos { x: 0, y: 64, z: 0 },
            1,
            &mut changes
        ));
        assert_eq!(changes.entities_spawned.len(), 1, "{name}");
        let id = changes.entities_spawned[0];
        let slot = state.entities.slot_of(id).unwrap();
        assert_eq!(super::name(&state, slot), Some(name.as_str()));
        assert!(state.players[&actor].inventory.slots[0].is_empty());
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Used, item_id),
            1
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Broken, item_id),
            0
        );
        state.entities.body_mut(slot).alive = false;
        let mut drops = Vec::new();
        assert!(death(&mut state, slot, &mut drops));
        assert_eq!(drops.len(), 1);
        assert_eq!(
            state
                .entities
                .row(state.entities.slot_of(drops[0]).unwrap())
                .item
                .as_ref()
                .unwrap()
                .item_id,
            item_id
        );
        assert!(death(&mut state, slot, &mut drops));
        assert_eq!(drops.len(), 1, "repeat destruction duplicates drop");
        state.remove_entity(slot);
        if let Some(slot) = state.entities.slot_of(drops[0]) {
            state.remove_entity(slot);
        }
    }
}
#[test]
fn occupied_or_solid_boat_placement_never_spends_item() {
    let mut s = fixture();
    let actor = player(&mut s, [-3.0, 65.0, 0.5]);
    let boat = item(&s, "oak_boat");
    s.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(boat, 2);
    let stone = block(&s, "stone", &[]);
    s.world.set_block(0, 65, 0, stone);
    let mut c = ChangeSet::default();
    use_on(
        &mut s,
        actor,
        Hand::Main,
        BlockPos { x: 0, y: 64, z: 0 },
        1,
        &mut c,
    );
    assert!(c.entities_spawned.is_empty());
    assert_eq!(s.players[&actor].inventory.slots[0].count, 2);
    s.world.set_block(0, 65, 0, s.registry.states.air);
    mob(&mut s, "oak_boat", [0.5, 65.0, 0.5]);
    use_on(
        &mut s,
        actor,
        Hand::Main,
        BlockPos { x: 0, y: 64, z: 0 },
        1,
        &mut c,
    );
    assert!(c.entities_spawned.is_empty());
    assert_eq!(s.players[&actor].inventory.slots[0].count, 2);
    assert_eq!(s.players[&actor].statistics.get(StatType::Used, boat), 0);
}
#[test]
fn boat_rejected_movement_does_not_suppress_native_gravity() {
    let mut state = fixture();
    let actor = player(&mut state, [0.5, 65.0, -0.5]);
    let boat = mob(&mut state, "oak_boat", [0.5, 65.0, 0.5]);
    run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: boat,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    let slot = state.entities.slot_of(boat).unwrap();
    assert_eq!(
        crate::mounts::rider(&state, slot),
        state.entities.slot_of(actor)
    );
    state.entities.set_pos(slot, [0.5, 70.0, 0.5]);
    state.entities.set_vel(slot, [0.0; 3]);
    run_tick(
        &mut state,
        &[PlayerInput::MoveVehicle {
            connection: ConnectionId(1),
            position: [50.0, 70.0, 50.0],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }],
    );
    assert!(
        state.entities.pos(slot)[1] < 70.0,
        "a rejected packet cannot freeze a boat in midair"
    );
}

#[test]
fn boat_client_water_moves_are_accepted_without_correction_feedback() {
    let mut state = fixture();
    let actor = player(&mut state, [0.5, 65.8, -0.5]);
    let water = block(&state, "water", &[("level", "0")]);
    for x in -2..=2 {
        for z in -2..=12 {
            state.world.set_block(x, 65, z, water);
        }
    }
    let boat = mob(&mut state, "oak_boat", [0.5, 65.8, 0.5]);
    run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: boat,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    let slot = state.entities.slot_of(boat).unwrap();
    assert_eq!(
        crate::mounts::rider(&state, slot),
        state.entities.slot_of(actor)
    );
    let start = state.entities.pos(slot);
    for step in 1..=30 {
        let position = [start[0], start[1], start[2] + f64::from(step) * 0.1];
        let (_, changes, _) = run_tick(
            &mut state,
            &[
                PlayerInput::VehicleInput {
                    connection: ConnectionId(1),
                    flags: 1,
                },
                PlayerInput::MoveVehicle {
                    connection: ConnectionId(1),
                    position,
                    yaw: 0.0,
                    pitch: 0.0,
                    on_ground: false,
                },
            ],
        );
        assert!(
            !changes
                .iter()
                .flat_map(|(_, changes)| changes)
                .any(|change| matches!(
                    change,
                    crate::boundary::PlayerChange::VehicleCorrected { .. }
                )),
            "accepted water movement must not trigger correction/echo feedback"
        );
        assert_eq!(
            state.entities.pos(slot),
            position,
            "do not integrate a client-steered boat twice"
        );
    }
    assert!(state.entities.pos(slot)[2] > start[2] + 2.9);
    let stone = state.registry.states.stone;
    for y in 65..=67 {
        state.world.set_block(0, y, 5, stone);
    }
    let (_, changes, _) = run_tick(
        &mut state,
        &[PlayerInput::MoveVehicle {
            connection: ConnectionId(1),
            position: [start[0], start[1], 5.5],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }],
    );
    assert!(
        state.entities.pos(slot)[2] < 5.0,
        "boat cannot pass through a wall"
    );
    assert!(
        changes
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(
                change,
                crate::boundary::PlayerChange::VehicleCorrected { .. }
            )),
        "clipped vehicle moves still require correction"
    );
    let (_, changes, _) = run_tick(
        &mut state,
        &[PlayerInput::MoveVehicle {
            connection: ConnectionId(1),
            position: [50.0, 65.8, 50.0],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }],
    );
    assert!(
        changes
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(
                change,
                crate::boundary::PlayerChange::VehicleCorrected { .. }
            )),
        "invalid vehicle moves still require correction"
    );
}

#[test]
fn boat_and_raft_player_seats_use_vehicle_attachment_points() {
    for (name, height_fraction, forward) in [
        ("oak_boat", 1.0 / 3.0, 0.0),
        ("oak_chest_boat", 1.0 / 3.0, f64::from(0.15_f32)),
        ("bamboo_raft", f64::from(0.888_888_9_f32), 0.0),
        (
            "bamboo_chest_raft",
            f64::from(0.888_888_9_f32),
            f64::from(0.15_f32),
        ),
    ] {
        let mut state = fixture();
        let actor = player(&mut state, [0.5, 65.0, -0.5]);
        let boat = mob(&mut state, name, [0.5, 65.0, 0.5]);
        run_tick(
            &mut state,
            &[PlayerInput::InteractEntity {
                connection: ConnectionId(1),
                target: boat,
                hand: Hand::Main,
                location: [0.0; 3],
            }],
        );
        let slot = state.entities.slot_of(boat).unwrap();
        let rider = state.entities.slot_of(actor).unwrap();
        assert_eq!(crate::mounts::rider(&state, slot), Some(rider));
        let at = state.entities.pos(slot);
        let player_at = state.entities.pos(rider);
        let expected_y =
            at[1] + state.entities.body(slot).height * height_fraction - f64::from(0.6_f32);
        assert!(
            (player_at[1] - expected_y).abs() < 1e-6,
            "{name}: {player_at:?}, expected y={expected_y}"
        );
        assert!(
            (player_at[2] - at[2] - forward).abs() < 1e-6,
            "{name}: chest seat offset"
        );
    }
}

#[test]
fn riding_boat_steers_paddles_collides_and_cannot_client_teleport() {
    let mut s = fixture();
    let actor = player(&mut s, [0.5, 65.8, -0.5]);
    let water = block(&s, "water", &[("level", "0")]);
    for x in -2..=2 {
        for z in -2..=5 {
            s.world.set_block(x, 65, z, water);
        }
    }
    let boat = mob(&mut s, "bamboo_raft", [0.5, 65.8, 0.5]);
    run_tick(
        &mut s,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: boat,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    let slot = s.entities.slot_of(boat).unwrap();
    assert_eq!(crate::mounts::rider(&s, slot), s.entities.slot_of(actor));
    let p = s.entities.pos(slot);
    let rider = s.entities.slot_of(actor).unwrap();
    s.entities.row_mut(rider).entity_life.mount.input = 1;
    for _ in 0..10 {
        run(&mut s, &mut Vec::new(), &mut ChangeSet::default());
        crate::mounts::sync_rider(&mut s, slot);
    }
    assert!(s.entities.pos(slot)[2] > p[2] + 0.5);
    assert_eq!(
        s.entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .paddles,
        [true, true]
    );
    let before = s.entities.pos(slot);
    crate::mounts::move_vehicle(&mut s, ConnectionId(1), [50.0, 65.8, 50.0], 0.0, 0.0, false);
    assert_eq!(s.entities.pos(slot), before);
    s.entities.row_mut(rider).entity_life.mount.input = 4;
    run(&mut s, &mut Vec::new(), &mut ChangeSet::default());
    assert!(s.entities.rot(slot)[0] < 0.0);
}
#[test]
fn chest_boat_clicks_share_custody_stale_click_and_break_do_not_duplicate() {
    let mut s = fixture();
    let actor = player(&mut s, [-1.0, 65.0, 0.5]);
    let boat = mob(&mut s, "oak_chest_boat", [0.5, 65.0, 0.5]);
    let slot = s.entities.slot_of(boat).unwrap();
    let diamond = item(&s, "diamond");
    s.entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap()
        .slots[0] = ItemStack::new(diamond, 3);
    let mut changes = ChangeSet::default();
    assert!(interact(
        &mut s,
        actor,
        boat,
        Hand::Main,
        true,
        &mut changes
    ));
    let m = s.players[&actor].open.as_ref().unwrap();
    let window = m.window;
    let revision = m.state_id;
    click(
        &mut s,
        actor,
        window,
        revision,
        0,
        0,
        ClickMode::Pickup,
        &mut changes,
    );
    assert!(
        s.entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .slots[0]
            .is_empty()
    );
    assert_eq!(s.players[&actor].open.as_ref().unwrap().carried.count, 3);
    click(
        &mut s,
        actor,
        window,
        revision,
        0,
        0,
        ClickMode::Pickup,
        &mut changes,
    );
    // Stale client intent either resyncs or swaps the same cursor; conservation holds.
    let stored = s
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap()
        .slots
        .iter()
        .map(|s| s.count)
        .sum::<i32>();
    let carried = s.players[&actor].open.as_ref().unwrap().carried.count;
    assert_eq!(stored + carried, 3);
    s.entities.body_mut(slot).alive = false;
    let mut drops = Vec::new();
    death(&mut s, slot, &mut drops);
    containers::maintain(&mut s, &mut changes);
    assert!(s.players[&actor].open.is_none());
    let inventory = s.players[&actor]
        .inventory
        .slots
        .iter()
        .filter(|s| s.item_id == diamond)
        .map(|s| s.count)
        .sum::<i32>();
    let dropped = drops
        .iter()
        .filter_map(|id| s.entities.slot_of(*id))
        .filter_map(|slot| s.entities.row(slot).item.as_ref())
        .filter(|s| s.item_id == diamond)
        .map(|s| s.count)
        .sum::<i32>();
    assert_eq!(inventory + dropped, 3);
}
#[test]
fn minecart_places_only_on_rail_and_follows_curves_and_slopes() {
    let mut s = fixture();
    let actor = player(&mut s, [-2.0, 65.0, 0.5]);
    let cart_item = item(&s, "minecart");
    s.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(cart_item, 1);
    let mut c = ChangeSet::default();
    use_on(
        &mut s,
        actor,
        Hand::Main,
        BlockPos { x: 0, y: 65, z: 0 },
        1,
        &mut c,
    );
    assert!(c.entities_spawned.is_empty());
    assert_eq!(s.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(
        s.players[&actor].statistics.get(StatType::Used, cart_item),
        0
    );
    rail(&mut s, 0, 65, 0, "east_west", "rail", false);
    use_on(
        &mut s,
        actor,
        Hand::Main,
        BlockPos { x: 0, y: 65, z: 0 },
        1,
        &mut c,
    );
    assert_eq!(c.entities_spawned.len(), 1);
    let cart = c.entities_spawned[0];
    let slot = s.entities.slot_of(cart).unwrap();
    assert!(s.players[&actor].inventory.slots[0].is_empty());
    assert_eq!(
        s.players[&actor].statistics.get(StatType::Used, cart_item),
        1
    );
    rail(&mut s, 1, 65, 0, "south_west", "rail", false);
    for z in 1..=4 {
        rail(&mut s, 1, 65, z, "north_south", "rail", false);
    }
    s.entities.set_vel(slot, [8.0, 0.0, 0.0]);
    for _ in 0..9 {
        run(&mut s, &mut Vec::new(), &mut c);
    }
    let p = s.entities.pos(slot);
    assert!((p[0] - 1.5).abs() < 0.1, "{p:?}");
    assert!(p[2] > 1.0, "curve must redirect cart: {p:?}");
    rail(&mut s, 3, 65, 0, "ascending_east", "rail", false);
    rail(&mut s, 4, 66, 0, "east_west", "rail", false);
    rail(&mut s, 5, 66, 0, "east_west", "rail", false);
    s.entities.set_pos(slot, [3.1, 65.1625, 0.5]);
    s.entities.set_vel(slot, [8.0, 0.0, 0.0]);
    for _ in 0..5 {
        run(&mut s, &mut Vec::new(), &mut c);
    }
    assert!(
        s.entities.pos(slot)[1] >= 66.0,
        "slope elevation: {:?}",
        s.entities.pos(slot)
    );
}
#[test]
fn powered_rail_accelerates_unpowered_brakes_and_obstacle_stops_cart() {
    let mut s = fixture();
    for x in 0..6 {
        rail(&mut s, x, 65, 0, "east_west", "powered_rail", true);
    }
    let cart = mob(&mut s, "minecart", [0.5, 65.0625, 0.5]);
    let slot = s.entities.slot_of(cart).unwrap();
    s.entities.set_vel(slot, [1.0, 0.0, 0.0]);
    run(&mut s, &mut Vec::new(), &mut ChangeSet::default());
    assert!(s.entities.vel(slot)[0] > 1.0);
    for x in 0..6 {
        rail(&mut s, x, 65, 0, "east_west", "powered_rail", false);
    }
    for _ in 0..8 {
        run(&mut s, &mut Vec::new(), &mut ChangeSet::default());
    }
    assert!(s.entities.vel(slot)[0].abs() < 0.01);
    rail(&mut s, 0, 65, 0, "east_west", "rail", false);
    let stone = block(&s, "stone", &[]);
    s.world.set_block(1, 65, 0, stone);
    s.world.set_block(1, 66, 0, stone);
    s.entities.set_pos(slot, [0.5, 65.0625, 0.5]);
    s.entities.set_vel(slot, [8.0, 0.0, 0.0]);
    run(&mut s, &mut Vec::new(), &mut ChangeSet::default());
    assert!(s.entities.pos(slot)[0] < 0.6);
    assert_eq!(s.entities.vel(slot)[0], 0.0);
}
#[test]
fn hopper_cart_transfers_components_conservatively_and_full_destination_preserves_source() {
    let mut s = fixture();
    rail(&mut s, 0, 65, 0, "east_west", "rail", false);
    let hopper = mob(&mut s, "hopper_minecart", [0.5, 65.0625, 0.5]);
    let slot = s.entities.slot_of(hopper).unwrap();
    let diamond = item(&s, "diamond");
    let chest = block(&s, "chest", &[]);
    s.world.set_block(0, 66, 0, chest);
    let mut stack = ItemStack::new(diamond, 2);
    stack.custom_name = Some("custody".into());
    s.block_entities.insert(
        (0, 66, 0),
        crate::tick::BlockEntityState {
            slots: vec![stack.clone(); 27],
            furnace: None,
        },
    );
    containers::tick_hopper(&mut s, slot, &mut ChangeSet::default());
    assert_eq!(
        s.entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .slots[0]
            .count,
        1
    );
    assert_eq!(s.block_entities[&(0, 66, 0)].slots[0].count, 1);
    assert_eq!(
        s.entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .slots[0]
            .custom_name,
        stack.custom_name
    );
    let v = s
        .entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap();
    v.slots = vec![ItemStack::new(diamond, 64); 5];
    v.transfer_cooldown = 0;
    let before = s.block_entities[&(0, 66, 0)].slots.clone();
    containers::tick_hopper(&mut s, slot, &mut ChangeSet::default());
    assert_eq!(s.block_entities[&(0, 66, 0)].slots, before);
}
#[test]
fn hopper_cart_item_suction_credits_once_and_block_hopper_can_empty_cart() {
    let mut s = fixture();
    rail(&mut s, 0, 65, 0, "east_west", "rail", false);
    let hopper = mob(&mut s, "hopper_minecart", [0.5, 65.0625, 0.5]);
    let slot = s.entities.slot_of(hopper).unwrap();
    let apple = item(&s, "apple");
    let ground = crate::tick::spawn_item_entity(&mut s, [0.5, 65.5, 0.5], ItemStack::new(apple, 7));
    let mut changes = ChangeSet::default();
    containers::tick_hopper(&mut s, slot, &mut changes);
    assert!(s.entities.slot_of(ground).is_none());
    let (_, _, _, removed) = s.commit_merge_effects();
    assert_eq!(removed, vec![ground]);
    assert!(s.entities.slot_of_any(ground).is_none());
    assert_eq!(
        s.entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .slots[0]
            .count,
        7
    );
    for _ in 0..8 {
        containers::tick_hopper(&mut s, slot, &mut changes);
    }
    assert_eq!(
        s.entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .slots[0]
            .count,
        7
    );
    s.block_entities.insert(
        (0, 64, 0),
        crate::tick::BlockEntityState {
            slots: vec![ItemStack::EMPTY; 5],
            furnace: None,
        },
    );
    assert!(pull_into_hopper(
        &mut s,
        BlockPos { x: 0, y: 65, z: 0 },
        BlockPos { x: 0, y: 64, z: 0 }
    ));
    assert_eq!(s.block_entities[&(0, 64, 0)].slots[0].count, 1);
    assert_eq!(
        s.entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .slots[0]
            .count,
        6
    );
}
#[test]
fn furnace_fuel_and_activator_tnt_fuse_restart_without_reset() {
    let mut s = fixture();
    let actor = player(&mut s, [-1.0, 65.0, 0.5]);
    rail(&mut s, 0, 65, 0, "east_west", "rail", false);
    let furnace = mob(&mut s, "furnace_minecart", [0.5, 65.0625, 0.5]);
    let slot = s.entities.slot_of(furnace).unwrap();
    let coal = item(&s, "coal");
    s.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(coal, 1);
    interact(
        &mut s,
        actor,
        furnace,
        Hand::Main,
        false,
        &mut ChangeSet::default(),
    );
    assert_eq!(
        s.entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .fuel,
        3600
    );
    assert_eq!(s.players[&actor].statistics.get(StatType::Used, coal), 0);
    run(&mut s, &mut Vec::new(), &mut ChangeSet::default());
    assert!(s.entities.vel(slot)[0] > 0.0);
    let saved = native(&s, furnace);
    let mut restored = fixture();
    let id = restore(&mut restored, &saved);
    let v = restored
        .entities
        .row(restored.entities.slot_of(id).unwrap())
        .entity_life
        .vehicle
        .as_ref()
        .unwrap();
    assert_eq!(v.fuel, 3599);
    assert!(v.push[0] > 0.0);
    rail(&mut s, 3, 65, 0, "east_west", "activator_rail", true);
    let tnt = mob(&mut s, "tnt_minecart", [3.5, 65.0625, 0.5]);
    let t = s.entities.slot_of(tnt).unwrap();
    run(&mut s, &mut Vec::new(), &mut ChangeSet::default());
    assert_eq!(
        s.entities.row(t).entity_life.vehicle.as_ref().unwrap().fuse,
        79
    );
    let saved = native(&s, tnt);
    let id = restore(&mut restored, &saved);
    let slot = restored.entities.slot_of(id).unwrap();
    assert_eq!(
        restored
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .fuse,
        79
    );
}

#[test]
fn primed_tnt_minecart_defuses_without_deletion_when_explosions_are_disabled() {
    let mut state = fixture();
    rail(&mut state, 0, 65, 0, "east_west", "activator_rail", true);
    let cart = mob(&mut state, "tnt_minecart", [0.5, 65.0625, 0.5]);
    let slot = state.entities.slot_of(cart).unwrap();
    let mut changes = ChangeSet::default();
    run(&mut state, &mut Vec::new(), &mut changes);
    assert!(changes.entity_events.iter().any(|event| matches!(
        event,
        crate::boundary::EntityEvent::Status { entity, event: 10 } if *entity == cart
    )));
    state
        .gamerules
        .insert("tnt_explodes".into(), "false".into());
    state
        .entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap()
        .fuse = 1;
    let mut intents = Vec::new();
    run(&mut state, &mut intents, &mut ChangeSet::default());
    let row = state.entities.row(slot);
    assert!(state.entities.body(slot).alive);
    assert!(!row.loot.death_handled);
    assert_eq!(row.entity_life.vehicle.as_ref().unwrap().fuse, -1);
    assert!(
        !intents
            .iter()
            .any(|intent| matches!(intent.kind, crate::intent::IntentKind::Explode { .. }))
    );
    let mut expired = Vec::new();
    queue_expired_removals(&state, &mut expired);
    assert!(expired.is_empty());
    run(&mut state, &mut Vec::new(), &mut ChangeSet::default());
    assert_eq!(
        state
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .fuse,
        -1
    );
}
#[test]
fn checkpoint_and_journal_preserve_chest_custody_and_reject_corrupt_records() {
    let mut s = fixture();
    let boat = mob(&mut s, "bamboo_chest_raft", [0.5, 65.0, 0.5]);
    let slot = s.entities.slot_of(boat).unwrap();
    let diamond = item(&s, "diamond");
    let mut stack = ItemStack::new(diamond, 9);
    stack.custom_name = Some("restart".into());
    s.entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap()
        .slots[26] = stack;
    let saved = native(&s, boat);
    let fields = mc_data::entity_nbt::entity_to_fields(&s.registry.items, &saved).unwrap();
    let decoded =
        mc_data::entity_nbt::entity_from_fields(&s.registry.items, Default::default(), &fields)
            .unwrap();
    let mut resumed = fixture();
    let id = restore(&mut resumed, &decoded);
    assert_eq!(
        resumed
            .entities
            .row(resumed.entities.slot_of(id).unwrap())
            .entity_life
            .vehicle,
        s.entities.row(slot).entity_life.vehicle
    );
    let image = EntityStateImage::from_row(s.entities.body(slot), s.entities.row(slot));
    let bytes = serde_json::to_vec(&image).unwrap();
    let replay: EntityStateImage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(image, replay);
    let mut malformed = saved.clone();
    malformed.extra_fields.retain(|(key, _)| key != "Items");
    malformed
        .extra_fields
        .push(("Items".into(), mc_nbt::Tag::Int(1)));
    assert!(
        EntityStateImage::from_record(&s.registry, &malformed)
            .unwrap_err()
            .to_string()
            .contains("vehicle Items: expected compound list")
    );
    let mut malformed = saved;
    malformed
        .extra_fields
        .push(("Fuel".into(), mc_nbt::Tag::String("corrupt".into())));
    assert!(
        EntityStateImage::from_record(&s.registry, &malformed)
            .unwrap_err()
            .to_string()
            .contains("vehicle Fuel: expected short")
    );
    let mut json = serde_json::to_value(&image).unwrap();
    json["entity_life"]["vehicle"]["slots"] = serde_json::json!([null]);
    assert!(serde_json::from_value::<EntityStateImage>(json).is_err());
}

#[test]
fn expired_hostile_and_tnt_cart_each_emit_one_tombstone() {
    let mut s = fixture();
    let vex = mob(&mut s, "vex", [4.5, 65.0, 0.5]);
    let slot = s.entities.slot_of(vex).unwrap();
    s.entities.row_mut(slot).special_mob = Some(Box::new(crate::special_mobs::Retained::Vex {
        owner: None,
        life: 1,
    }));
    crate::special_mobs::run(
        &mut s,
        &mut Vec::new(),
        vex,
        crate::blueprint::GoalKind::VexLife,
        &[],
    );
    assert!(s.entities.slot_of_any(vex).is_none());
    rail(&mut s, 0, 65, 0, "east_west", "rail", false);
    let tnt = mob(&mut s, "tnt_minecart", [0.5, 65.0625, 0.5]);
    let slot = s.entities.slot_of(tnt).unwrap();
    s.entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap()
        .fuse = 1;
    let (output, _, _) = run_tick(&mut s, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut s, &output);
    for id in [vex, tnt] {
        assert!(s.entities.slot_of_any(id).is_none());
        assert_eq!(
            output
                .changes
                .entities_removed
                .iter()
                .filter(|&&removed| removed == id)
                .count(),
            1
        );
        assert_eq!(entries.iter().filter(|entry| matches!(entry, crate::journal::JournalEntry::RemoveEntity { id: removed, .. } if *removed == id.0)).count(), 1);
    }
}

#[test]
fn entity_region_and_wal_restart_preserve_vehicle_motion_and_custody() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let dir =
        std::env::temp_dir().join(format!("solaris-vehicle-restart-{}", uuid::Uuid::new_v4()));
    let mut s = fixture();
    for x in 0..10 {
        rail(&mut s, x, 65, 0, "east_west", "rail", false);
    }
    let cart = mob(&mut s, "chest_minecart", [0.5, 65.0625, 0.5]);
    let furnace = mob(&mut s, "furnace_minecart", [4.5, 65.0625, 0.5]);
    let tnt = mob(&mut s, "tnt_minecart", [8.5, 65.0625, 0.5]);
    let raft = mob(&mut s, "bamboo_chest_raft", [0.5, 67.0, 4.5]);
    let diamond = item(&s, "diamond");
    for id in [cart, raft] {
        let slot = s.entities.slot_of(id).unwrap();
        let mut stack = ItemStack::new(diamond, 9);
        stack.custom_name = Some("disk custody".into());
        s.entities
            .row_mut(slot)
            .entity_life
            .vehicle
            .as_mut()
            .unwrap()
            .slots[26] = stack;
    }
    let slot = s.entities.slot_of(furnace).unwrap();
    let v = s
        .entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap();
    v.fuel = 100;
    v.push = [1.0, 0.0];
    let slot = s.entities.slot_of(tnt).unwrap();
    s.entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap()
        .fuse = 70;
    let (output, _, _) = run_tick(&mut s, &[]);
    let initial_entries = crate::core::journal_entries_for_tick(&mut s, &output);
    let base = replay_batches(
        Replay::default(),
        &[(output.tick, output.tick, initial_entries)],
    );
    let (_, chunks, _) = crate::core::checkpoint_data_for(&s, output.tick);
    mc_world::anvil::write_entity_regions(
        &dir.join("entities"),
        &s.registry.items,
        &chunks,
        u32::try_from(output.tick).unwrap(),
    )
    .unwrap();
    let chunks =
        mc_world::anvil::read_entity_regions(&dir.join("entities"), &s.registry.items).unwrap();
    let mut checkpoint = fixture();
    for record in chunks.values().flat_map(|chunk| &chunk.entities) {
        let id = restore(&mut checkpoint, record);
        let original = s.entities.slot_of(id).unwrap();
        let restored = checkpoint.entities.slot_of(id).unwrap();
        assert_eq!(
            checkpoint.entities.row(restored).entity_life.vehicle,
            s.entities.row(original).entity_life.vehicle
        );
        assert_eq!(checkpoint.entities.pos(restored), s.entities.pos(original));
        assert_eq!(checkpoint.entities.vel(restored), s.entities.vel(original));
    }
    let slot = s.entities.slot_of(cart).unwrap();
    s.entities.set_pos(slot, [2.5, 65.0625, 0.5]);
    s.entities.set_vel(slot, [4.0, 0.0, 0.0]);
    s.entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap()
        .slots[26]
        .count = 4;
    let (output, _, _) = run_tick(&mut s, &[]);
    let tail = crate::core::journal_entries_for_tick(&mut s, &output);
    {
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer.append(output.tick, output.tick, &tail).unwrap();
        writer.flush().unwrap();
    }
    let base_position = base.entities[&cart.0].pos;
    let batches = read_complete_batches_after(&dir.join("journal"), base.tick).unwrap();
    let replay = replay_batches(base, &batches);
    let mut resumed = fixture();
    for (id, record) in &replay.entities {
        validate_replay(&resumed.registry, record).unwrap();
        crate::core::restore_entity(&mut resumed, *id, record).unwrap();
    }
    for id in [cart, furnace, tnt, raft] {
        let original = s.entities.slot_of(id).unwrap();
        let restored = resumed.entities.slot_of(id).unwrap();
        assert_eq!(
            resumed.entities.row(restored).entity_life.vehicle,
            s.entities.row(original).entity_life.vehicle
        );
        assert_eq!(resumed.entities.pos(restored), s.entities.pos(original));
        assert_eq!(resumed.entities.vel(restored), s.entities.vel(original));
        assert_eq!(
            resumed.entities.ground(restored),
            s.entities.ground(original)
        );
    }
    assert_ne!(replay.entities[&cart.0].pos, base_position);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unopened_mineshaft_cart_loot_survives_restart_and_materializes_once() {
    let mut s = fixture();
    let cart = mob(&mut s, "chest_minecart", [0.5, 65.0, 0.5]);
    let mut record = native(&s, cart);
    record.extra_fields.push((
        "LootTable".into(),
        mc_nbt::Tag::String("minecraft:chests/abandoned_mineshaft".into()),
    ));
    record
        .extra_fields
        .push(("LootTableSeed".into(), mc_nbt::Tag::Long(-17)));
    let mut resumed = fixture();
    let id = restore(&mut resumed, &record);
    let slot = resumed.entities.slot_of(id).unwrap();
    let saved = native(&resumed, id);
    assert!(!saved.extra_fields.iter().any(|(key, _)| key == "Items"));
    assert_eq!(
        from_record(&resumed.registry, &saved).unwrap(),
        resumed
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_deref()
            .cloned()
    );
    loot::materialize(&mut resumed, slot);
    let populated = resumed
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .clone()
        .unwrap();
    assert!(populated.loot_table.is_none());
    assert!(populated.slots.iter().any(|stack| !stack.is_empty()));
    loot::materialize(&mut resumed, slot);
    assert_eq!(
        resumed
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap(),
        &populated
    );
    let saved = native(&resumed, id);
    assert!(
        !saved
            .extra_fields
            .iter()
            .any(|(key, _)| key == "LootTable" || key == "LootTableSeed")
    );
    let mut bad = record;
    bad.extra_fields.retain(|(key, _)| key != "LootTable");
    bad.extra_fields.push((
        "LootTable".into(),
        mc_nbt::Tag::String("minecraft:chests/unknown".into()),
    ));
    assert!(
        from_record(&s.registry, &bad)
            .unwrap_err()
            .to_string()
            .contains(
                "vehicle LootTable minecraft:chests/unknown: unsupported saved container table"
            )
    );
}

#[test]
fn container_loot_advancement_requires_first_accepted_player_open() {
    let mut state = fixture();
    let mut tree = mc_data::advancements::AdvancementTree::default();
    tree.definitions.insert(
        "test:vehicle_loot".into(),
        serde_json::from_value(serde_json::json!({
            "criteria": {"loot": {"trigger": "minecraft:player_generates_container_loot"}},
            "requirements": [["loot"]],
        }))
        .unwrap(),
    );
    Arc::get_mut(&mut state.registry).unwrap().advancements = Arc::new(tree);
    let actor = player(&mut state, [0.5, 65.0, 0.5]);
    let cart = mob(&mut state, "chest_minecart", [1.5, 65.0, 0.5]);
    let slot = state.entities.slot_of(cart).unwrap();
    let vehicle = state
        .entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap();
    vehicle.loot_table = Some(Identifier::parse("minecraft:chests/abandoned_mineshaft").unwrap());
    vehicle.loot_seed = -17;
    let actor_slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(actor_slot, [20.5, 65.0, 0.5]);
    containers::open(&mut state, actor, cart, &mut Default::default());
    assert!(state.advancement_events.is_empty());
    assert!(
        state
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .loot_table
            .is_some()
    );
    state.entities.set_pos(actor_slot, [0.5, 65.0, 0.5]);
    containers::open(&mut state, actor, cart, &mut Default::default());
    assert_eq!(state.advancement_events.len(), 1);
    let event = &state.advancement_events[0];
    assert_eq!(event.entity, actor);
    assert_eq!(event.trigger, "minecraft:player_generates_container_loot");
    assert_eq!(
        event.evidence["loot_table"],
        "minecraft:chests/abandoned_mineshaft"
    );
    assert!(
        state
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap()
            .loot_table
            .is_none()
    );
    containers::open(&mut state, actor, cart, &mut Default::default());
    assert_eq!(state.advancement_events.len(), 1);
    state.advancement_events.clear();
    state
        .entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap()
        .loot_table = Some(Identifier::parse("minecraft:chests/abandoned_mineshaft").unwrap());
    loot::materialize(&mut state, slot);
    assert!(state.advancement_events.is_empty());
    containers::open(&mut state, actor, cart, &mut Default::default());
    assert!(state.advancement_events.is_empty());
}

#[test]
fn creative_offhand_vehicle_placement_credits_used_without_spending() {
    for name in ["oak_boat", "furnace_minecart"] {
        let mut state = fixture();
        let actor = player(&mut state, [-3.0, 65.0, 0.5]);
        let item_id = item(&state, name);
        let main_item = item(&state, "stone");
        let held = ItemStack::new(item_id, 1);
        let main = ItemStack::new(main_item, 2);
        let p = state.players.get_mut(&actor).unwrap();
        p.game_mode = crate::commands::GameMode::Creative;
        p.inventory.slots[0] = main.clone();
        p.inventory.slots[crate::inventory::OFF_HAND] = held.clone();
        let clicked = if name.ends_with("minecart") {
            rail(&mut state, 0, 65, 0, "east_west", "rail", false);
            BlockPos { x: 0, y: 65, z: 0 }
        } else {
            BlockPos { x: 0, y: 64, z: 0 }
        };
        let mut changes = ChangeSet::default();
        assert!(use_on(
            &mut state,
            actor,
            Hand::Off,
            clicked,
            1,
            &mut changes
        ));
        assert_eq!(changes.entities_spawned.len(), 1, "{name}");
        assert_eq!(state.players[&actor].inventory.slots[0], main);
        assert_eq!(
            state.players[&actor].inventory.slots[crate::inventory::OFF_HAND],
            held
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Used, item_id),
            1
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Broken, item_id),
            0
        );
        use_on(&mut state, actor, Hand::Off, clicked, 1, &mut changes);
        assert_eq!(changes.entities_spawned.len(), 1, "occupied placement");
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Used, item_id),
            1
        );
    }
}

#[test]
fn furnace_fuel_consumption_does_not_award_item_used_in_survival_or_creative() {
    let mut state = fixture();
    let actor = player(&mut state, [-1.0, 65.0, 0.5]);
    let furnace = mob(&mut state, "furnace_minecart", [0.5, 65.0625, 0.5]);
    let slot = state.entities.slot_of(furnace).unwrap();
    for (mode, name) in [
        (crate::commands::GameMode::Survival, "coal"),
        (crate::commands::GameMode::Creative, "charcoal"),
    ] {
        let item_id = item(&state, name);
        let p = state.players.get_mut(&actor).unwrap();
        p.game_mode = mode;
        p.inventory.slots[crate::inventory::OFF_HAND] = ItemStack::new(item_id, 2);
        state
            .entities
            .row_mut(slot)
            .entity_life
            .vehicle
            .as_mut()
            .unwrap()
            .fuel = 0;
        assert!(interact(
            &mut state,
            actor,
            furnace,
            Hand::Off,
            false,
            &mut ChangeSet::default()
        ));
        assert_eq!(
            state
                .entities
                .row(slot)
                .entity_life
                .vehicle
                .as_ref()
                .unwrap()
                .fuel,
            3600
        );
        assert_eq!(
            state.players[&actor].inventory.slots[crate::inventory::OFF_HAND].count,
            if mode == crate::commands::GameMode::Creative {
                2
            } else {
                1
            }
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Used, item_id),
            0
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Broken, item_id),
            0
        );
    }
}

#[test]
fn tnt_cart_ignition_credits_used_once_and_broken_only_on_actual_durability_break() {
    let mut state = fixture();
    let actor = player(&mut state, [-1.0, 65.0, 0.5]);
    for (mode, name, damage, breaks) in [
        (
            crate::commands::GameMode::Survival,
            "fire_charge",
            None,
            false,
        ),
        (
            crate::commands::GameMode::Creative,
            "fire_charge",
            None,
            false,
        ),
        (
            crate::commands::GameMode::Survival,
            "flint_and_steel",
            Some(62),
            false,
        ),
        (
            crate::commands::GameMode::Survival,
            "flint_and_steel",
            Some(63),
            true,
        ),
        (
            crate::commands::GameMode::Creative,
            "flint_and_steel",
            Some(63),
            false,
        ),
    ] {
        let item_id = item(&state, name);
        let cart = mob(&mut state, "tnt_minecart", [0.5, 65.0625, 0.5]);
        let slot = state.entities.slot_of(cart).unwrap();
        let mut held = ItemStack::new(item_id, 1);
        held.damage = damage;
        let p = state.players.get_mut(&actor).unwrap();
        p.game_mode = mode;
        p.inventory.slots[crate::inventory::OFF_HAND] = held.clone();
        let used_before = p.statistics.get(StatType::Used, item_id);
        let broken_before = p.statistics.get(StatType::Broken, item_id);
        state
            .gamerules
            .insert("tnt_explodes".into(), "false".into());
        let mut changes = ChangeSet::default();
        assert!(interact(
            &mut state,
            actor,
            cart,
            Hand::Off,
            false,
            &mut changes
        ));
        assert_eq!(
            state
                .entities
                .row(slot)
                .entity_life
                .vehicle
                .as_ref()
                .unwrap()
                .fuse,
            -1
        );
        assert_eq!(
            state.players[&actor].inventory.slots[crate::inventory::OFF_HAND],
            held
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Used, item_id),
            used_before
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Broken, item_id),
            broken_before
        );
        state.gamerules.insert("tnt_explodes".into(), "true".into());
        assert!(interact(
            &mut state,
            actor,
            cart,
            Hand::Off,
            false,
            &mut changes
        ));
        assert_eq!(
            state
                .entities
                .row(slot)
                .entity_life
                .vehicle
                .as_ref()
                .unwrap()
                .fuse,
            80
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Used, item_id),
            used_before + 1
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Broken, item_id),
            broken_before + i32::from(breaks)
        );
        let after = &state.players[&actor].inventory.slots[crate::inventory::OFF_HAND];
        if mode == crate::commands::GameMode::Creative {
            assert_eq!(after, &held);
        } else if name == "fire_charge" || breaks {
            assert!(after.is_empty());
        } else {
            assert_eq!(after.item_id, item_id);
            assert_eq!(after.damage, Some(63));
        }
        let after = after.clone();
        assert!(interact(
            &mut state,
            actor,
            cart,
            Hand::Off,
            false,
            &mut changes
        ));
        assert_eq!(
            state.players[&actor].inventory.slots[crate::inventory::OFF_HAND],
            after
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Used, item_id),
            used_before + 1
        );
        assert_eq!(
            state.players[&actor]
                .statistics
                .get(StatType::Broken, item_id),
            broken_before + i32::from(breaks)
        );
    }
}

#[test]
fn hopper_pulling_from_a_container_vehicle_refreshes_its_open_menu() {
    let mut s = fixture();
    let viewer = player(&mut s, [2.5, 64.0, 0.5]);
    let stone = block(&s, "stone", &[]);
    s.world.set_block(2, 63, 0, stone);
    s.world.set_block(0, 63, 0, stone);
    let hopper = BlockPos { x: 0, y: 64, z: 0 };
    let hopper_block = block(&s, "hopper", &[("facing", "down")]);
    s.world
        .set_block(hopper.x, hopper.y, hopper.z, hopper_block);
    s.world.fold_overlay();
    s.block_entities.insert(
        (0, 64, 0),
        crate::tick::BlockEntityState {
            slots: vec![ItemStack::EMPTY; 5],
            furnace: None,
        },
    );
    run_tick(
        &mut s,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: hopper,
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    let window = s.players[&viewer]
        .open
        .as_ref()
        .expect("hopper menu")
        .window;
    let cart = mob(&mut s, "chest_minecart", [0.5, 65.0, 0.5]);
    let apple = item(&s, "apple");
    let slot = s.entities.slot_of(cart).unwrap();
    s.entities
        .row_mut(slot)
        .entity_life
        .vehicle
        .as_mut()
        .unwrap()
        .slots[0] = ItemStack::new(apple, 3);
    let mut refreshed = false;
    for _ in 0..10 {
        let (_, drains, _) = run_tick(&mut s, &[]);
        refreshed |= drains
            .iter()
            .filter(|(connection, _)| *connection == ConnectionId(1))
            .flat_map(|(_, changes)| changes)
            .any(|change| match change {
                crate::boundary::PlayerChange::MenuSlot {
                    window: w, item, ..
                } => *w == window && item.as_ref().is_some_and(|i| i.item_id == apple),
                crate::boundary::PlayerChange::MenuContents {
                    window: w, slots, ..
                } => *w == window && slots.iter().flatten().any(|stack| stack.item_id == apple),
                _ => false,
            });
        if s.block_entities[&(0, 64, 0)].slots[0].item_id == apple {
            break;
        }
    }
    assert_eq!(s.block_entities[&(0, 64, 0)].slots[0].item_id, apple);
    assert!(refreshed, "the hopper viewer sees the pulled apple");
}
