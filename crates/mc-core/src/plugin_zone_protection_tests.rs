//! Native enforcement of protected zones: containers, ignition, explosions,
//! fire spread, entity interaction, damage (PvP + animals) and the public-use
//! flag, for a listed member against a visitor.

use std::sync::Arc;

use super::*;
use crate::boundary::{DigAction, Hand};
use crate::tick::run_tick;

const VISITOR: ConnectionId = ConnectionId(1);
const MEMBER: ConnectionId = ConnectionId(2);
const CHEST: BlockPos = BlockPos { x: 10, y: 64, z: 8 };
const LEVER: BlockPos = BlockPos { x: 12, y: 64, z: 8 };

struct World {
    state: TickState,
    visitor: EntityId,
    member: EntityId,
}

fn join(state: &mut TickState, connection: ConnectionId, name: &str) -> EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection,
            uuid: uuid::Uuid::from_u128(u128::from(connection.0)),
            name: name.into(),
            permission_level: 0,
        }],
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    run_tick(
        state,
        &[PlayerInput::ConfirmTeleport {
            connection,
            teleport_id: 1,
        }],
    );
    state.connections.entity_of(connection).expect("joined")
}

fn set(state: &mut TickState, pos: BlockPos, name: &str) -> u32 {
    let id = state
        .registry
        .blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(pos.x, pos.y, pos.z, id);
    id
}

fn give(state: &mut TickState, entity: EntityId, name: &str) {
    let item = state
        .registry
        .items
        .id_of(&Identifier::parse(name).unwrap())
        .unwrap();
    let slot = crate::tick::held_slot_index(&state.players[&entity], Hand::Main);
    state.players.get_mut(&entity).unwrap().inventory.slots[slot] = ItemStack::new(item, 1);
}

/// Two survival players inside a protected claim `[0,15]x[-64,319]x[0,15]`
/// that lists only the member; a chest and a lever stand in it.
fn world(public_use: bool) -> World {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let flat = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, flat, 2, 512);
    let visitor = join(&mut state, VISITOR, "visitor");
    let member = join(&mut state, MEMBER, "member");
    for (entity, x) in [(visitor, 8.5), (member, 9.5)] {
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.set_pos(slot, [x, 64.0, 8.5]);
        state.entities.set_ground(slot, true);
        state.entities.row_mut(slot).protected_until = 0;
    }
    set(&mut state, CHEST, "minecraft:chest");
    set(&mut state, LEVER, "minecraft:lever");
    state.world.fold_overlay();
    install(&mut state, public_use);
    World {
        state,
        visitor,
        member,
    }
}

fn install(state: &mut TickState, public_use: bool) {
    state.plugins.set_domain(
        "towns",
        ZONES,
        serde_json::json!({
            "claim": {
                "dimension": "minecraft:overworld",
                "minimum": [0.0, -64.0, 0.0],
                "maximum": [15.0, 319.0, 15.0],
                "allowed_actors": [uuid::Uuid::from_u128(2).simple().to_string()],
                "public_use": public_use
            }
        }),
    );
    restore(state).unwrap();
}

fn use_on(state: &mut TickState, connection: ConnectionId, position: BlockPos) {
    run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection,
            hand: Hand::Main,
            position,
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
}

fn attack(state: &mut TickState, connection: ConnectionId, target: EntityId) {
    run_tick(state, &[PlayerInput::Attack { connection, target }]);
}

fn mob_health(state: &TickState, mob: EntityId) -> f32 {
    state
        .entities
        .row(state.entities.slot_of(mob).unwrap())
        .health
}

fn player_health(state: &TickState, player: EntityId) -> f32 {
    state.players[&player].vitals.health
}

#[test]
fn visitor_cannot_open_a_zone_container_but_the_member_can() {
    let mut w = world(false);
    use_on(&mut w.state, VISITOR, CHEST);
    assert!(w.state.players[&w.visitor].open.is_none());
    use_on(&mut w.state, MEMBER, CHEST);
    assert!(w.state.players[&w.member].open.is_some());
}

#[test]
fn public_use_opens_doors_and_levers_but_never_containers() {
    let mut w = world(false);
    let before = w.state.world.block_at(LEVER.x, LEVER.y, LEVER.z);
    use_on(&mut w.state, VISITOR, LEVER);
    assert_eq!(w.state.world.block_at(LEVER.x, LEVER.y, LEVER.z), before);

    let mut w = world(true);
    use_on(&mut w.state, VISITOR, LEVER);
    assert_ne!(w.state.world.block_at(LEVER.x, LEVER.y, LEVER.z), before);
    use_on(&mut w.state, VISITOR, CHEST);
    assert!(w.state.players[&w.visitor].open.is_none());
    // Clicking the lever with a block in hand flips it and places nothing.
    give(&mut w.state, w.visitor, "minecraft:stone");
    use_on(&mut w.state, VISITOR, LEVER);
    assert_eq!(w.state.world.block_at(LEVER.x, LEVER.y, LEVER.z), before);
    assert_eq!(
        w.state.world.block_at(LEVER.x, LEVER.y + 1, LEVER.z),
        Some(w.state.registry.states.air)
    );
    // Placing stone on the floor elsewhere in the claim stays denied.
    use_on(&mut w.state, VISITOR, BlockPos { x: 11, y: 63, z: 8 });
    assert_eq!(
        w.state.world.block_at(11, 64, 8),
        Some(w.state.registry.states.air)
    );
}

#[test]
fn visitor_cannot_light_a_fire_in_a_zone_but_the_member_can() {
    let mut w = world(false);
    let floor = BlockPos { x: 11, y: 63, z: 6 };
    let fire = BlockPos { x: 11, y: 64, z: 6 };
    let air = w.state.registry.states.air;
    give(&mut w.state, w.visitor, "minecraft:flint_and_steel");
    use_on(&mut w.state, VISITOR, floor);
    assert_eq!(w.state.world.block_at(fire.x, fire.y, fire.z), Some(air));
    give(&mut w.state, w.member, "minecraft:flint_and_steel");
    use_on(&mut w.state, MEMBER, floor);
    assert_ne!(w.state.world.block_at(fire.x, fire.y, fire.z), Some(air));
}

#[test]
fn visitor_cannot_hurt_animals_or_players_in_a_zone_but_can_fight_hostiles() {
    let mut w = world(false);
    let cow = w
        .state
        .spawn_mob(w.state.registry.cow_type_id, [10.5, 64.0, 9.5], 10.0);
    attack(&mut w.state, VISITOR, cow);
    assert_eq!(mob_health(&w.state, cow), 10.0, "visitor spared the cow");
    attack(&mut w.state, VISITOR, w.member);
    assert_eq!(
        player_health(&w.state, w.member),
        20.0,
        "no PvP on a visitor's part"
    );
    let zombie = w
        .state
        .spawn_mob(w.state.registry.zombie_type_id, [8.5, 64.0, 10.0], 20.0);
    attack(&mut w.state, VISITOR, zombie);
    assert!(
        mob_health(&w.state, zombie) < 20.0,
        "hostiles stay fair game"
    );
    attack(&mut w.state, MEMBER, cow);
    assert!(
        mob_health(&w.state, cow) < 10.0,
        "the member may hurt the cow"
    );
    attack(&mut w.state, MEMBER, w.visitor);
    assert!(
        player_health(&w.state, w.visitor) < 20.0,
        "members defend their land"
    );
}

#[test]
fn the_same_visitor_hits_land_once_the_zone_is_gone() {
    let mut w = world(false);
    let cow = w
        .state
        .spawn_mob(w.state.registry.cow_type_id, [10.5, 64.0, 9.5], 10.0);
    w.state
        .plugins
        .set_domain("towns", ZONES, serde_json::json!({}));
    restore(&mut w.state).unwrap();
    attack(&mut w.state, VISITOR, cow);
    assert!(mob_health(&w.state, cow) < 10.0);
    attack(&mut w.state, VISITOR, w.member);
    assert!(player_health(&w.state, w.member) < 20.0);
}

#[test]
fn visitor_cannot_interact_with_zone_entities() {
    let mut w = world(false);
    let cow = w
        .state
        .spawn_mob(w.state.registry.cow_type_id, [10.5, 64.0, 9.5], 10.0);
    assert!(allows_entity(&w.state, w.member, cow));
    assert!(!allows_entity(&w.state, w.visitor, cow));
    let outside = w
        .state
        .spawn_mob(w.state.registry.cow_type_id, [40.5, 64.0, 9.5], 10.0);
    assert!(allows_entity(&w.state, w.visitor, outside));
}

#[test]
fn explosions_spare_zone_blocks() {
    let mut w = world(false);
    crate::explosion::apply(&mut w.state, None, [8.5, 63.5, 8.5], 4.0, None, None);
    assert!(
        w.state.merge_block_writes.is_empty(),
        "a blast inside the claim broke blocks"
    );
    w.state
        .plugins
        .set_domain("towns", ZONES, serde_json::json!({}));
    restore(&mut w.state).unwrap();
    crate::explosion::apply(&mut w.state, None, [8.5, 63.5, 8.5], 4.0, None, None);
    assert!(
        !w.state.merge_block_writes.is_empty(),
        "the same blast breaks blocks without a claim"
    );
}

#[test]
fn zone_blocks_stay_protected_against_dig_by_a_visitor() {
    let mut w = world(false);
    let before = w.state.world.block_at(CHEST.x, CHEST.y, CHEST.z);
    for (sequence, action) in [(1, DigAction::Start), (2, DigAction::Finish)] {
        run_tick(
            &mut w.state,
            &[PlayerInput::Dig {
                connection: VISITOR,
                position: CHEST,
                action,
                sequence,
            }],
        );
    }
    assert_eq!(w.state.world.block_at(CHEST.x, CHEST.y, CHEST.z), before);
}
