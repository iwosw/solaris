use std::sync::Arc;

use mc_data::{Identifier, ItemStack};

use super::{ProjectileKind, ProjectileState, launch};
use crate::boundary::{ConnectionId, EntityId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};

pub(super) fn setup() -> (TickState, EntityId, [f64; 3]) {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let world = crate::horde_bench::flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "archer".into(),
            permission_level: 0,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    for pos in state.world.resident_positions() {
        state
            .tickets
            .insert(format!("projectile-test:{}:{}", pos.x, pos.z), (pos, 0));
    }
    state.refresh_active_chunks(state.tick);
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(player).unwrap();
    let at = state.entities.pos(slot);
    let centre = [at[0].floor() + 0.5, 201.0, at[2].floor() + 0.5];
    for x in -10..=10 {
        for z in -10..=10 {
            state.world.set_block(
                centre[0] as i32 + x,
                200,
                centre[2] as i32 + z,
                state.registry.states.stone,
            );
        }
    }
    state.world.fold_overlay();
    state.entities.set_pos(slot, centre);
    state.entities.row_mut(slot).protected_until = 0;
    state.day_time = 6000;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    (state, player, centre)
}

fn arrow_stack(state: &TickState) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:arrow").unwrap())
            .unwrap(),
        1,
    )
}

#[test]
fn flight_hits_first_body_and_commits_punch_along_horizontal_flight() {
    let (mut state, player, centre) = setup();
    let cow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let victim = state.spawn_mob(cow, [centre[0] + 2.0, centre[1], centre[2]], 20.0);
    // Keep the target stationary: vanilla knockback preserves half its prior
    // horizontal motion, so wandering would obscure the Punch direction.
    let victim_slot = state.entities.slot_of(victim).unwrap();
    state.entities.row_mut(victim_slot).speed = 0.0;
    let owner = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let mut record = ProjectileState::new(ProjectileKind::Arrow, Some(owner), arrow_stack(&state));
    record.punch = 2;
    record.damage = 2.0;
    let projectile = launch(
        &mut state,
        record,
        [centre[0], centre[1] + 0.7, centre[2]],
        [3.0, 0.0, 0.0],
    )
    .unwrap();
    run_tick(&mut state, &[]);
    assert!(state.entities.slot_of(projectile).is_none());
    let slot = state.entities.slot_of(victim).unwrap();
    assert_eq!(state.entities.row(slot).health, 14.0);
    assert!(
        state.entities.vel(slot)[0] > 20.0,
        "Punch is additional forward momentum"
    );
    assert!(state.entities.vel(slot)[2].abs() < 0.01);
}

#[test]
fn creative_and_infinity_arrows_cannot_be_picked_up_or_duplicate_ammunition() {
    let (mut state, player, centre) = setup();
    let mut record = ProjectileState::new(ProjectileKind::Arrow, None, arrow_stack(&state));
    record.stuck = true;
    let id = launch(&mut state, record, centre, [0.0; 3]).unwrap();
    for _ in 0..4 {
        run_tick(&mut state, &[]);
    }
    assert!(state.entities.slot_of(id).is_some());
    assert!(
        state.players[&player]
            .inventory
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    state
        .entities
        .row_mut(state.entities.slot_of(id).unwrap())
        .projectile
        .as_mut()
        .unwrap()
        .pickup = true;
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(state.entities.slot_of(id).is_none());
    assert!(output.changes.entities_removed.contains(&id));
    assert_eq!(
        state.players[&player]
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        1
    );
    run_tick(&mut state, &[]);
    assert_eq!(
        state.players[&player]
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        1
    );
}

#[test]
fn piercing_crossbow_arrow_hits_multiple_distinct_bodies_without_repeat_damage() {
    let (mut state, player, centre) = setup();
    let cow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let a = state.spawn_mob(cow, [centre[0] + 1.5, centre[1], centre[2]], 20.0);
    let b = state.spawn_mob(cow, [centre[0] + 3.0, centre[1], centre[2]], 20.0);
    let owner = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let mut record = ProjectileState::new(ProjectileKind::Arrow, Some(owner), arrow_stack(&state));
    record.piercing = 1;
    launch(
        &mut state,
        record,
        [centre[0], centre[1] + 0.7, centre[2]],
        [4.0, 0.0, 0.0],
    )
    .unwrap();
    run_tick(&mut state, &[]);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(a).unwrap())
            .health,
        12.0
    );
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(b).unwrap())
            .health,
        12.0
    );
}

#[test]
fn player_arrow_damage_is_not_scaled_by_peaceful_difficulty() {
    let (mut state, player, centre) = setup();
    state.difficulty = crate::survival::Difficulty::Peaceful;
    let cow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let victim = state.spawn_mob(cow, [centre[0] + 2.0, centre[1], centre[2]], 20.0);
    let owner = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let record = ProjectileState::new(ProjectileKind::Arrow, Some(owner), arrow_stack(&state));
    launch(
        &mut state,
        record,
        [centre[0], centre[1] + 0.7, centre[2]],
        [3.0, 0.0, 0.0],
    )
    .unwrap();
    run_tick(&mut state, &[]);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(victim).unwrap())
            .health,
        14.0
    );
}

#[test]
fn loyalty_returns_the_retained_worn_trident_once_and_projects_no_physics() {
    let (mut state, player, centre) = setup();
    let owner_slot = state.entities.slot_of(player).unwrap();
    let owner = state.entities.row(owner_slot).uuid;
    let mut stack = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:trident").unwrap())
            .unwrap(),
        1,
    )
    .with_enchantment(Identifier::parse("minecraft:loyalty").unwrap(), 3);
    stack.damage = Some(73);
    let mut record = ProjectileState::new(ProjectileKind::Trident, Some(owner), stack.clone());
    record.stuck = true;
    record.pickup = true;
    let origin = [centre[0] + 6.0, centre[1] + 1.0, centre[2]];
    let id = launch(&mut state, record, origin, [0.0; 3]).unwrap();
    let mut intents = Vec::new();
    super::flight::run(&mut state, &mut intents, std::iter::once(id));
    let slot = state.entities.slot_of(id).unwrap();
    assert!(state.entities.pos(slot)[0] < origin[0]);
    let record = state.entities.row(slot).projectile.as_ref().unwrap();
    assert!(record.returning && !record.stuck);
    assert!(
        state
            .entities
            .row(slot)
            .appearance
            .extra
            .contains(&(8, crate::boundary::EntityDataValue::Byte(2)),)
    );
    for _ in 0..80 {
        super::flight::run(&mut state, &mut intents, std::iter::once(id));
    }
    assert!(state.entities.slot_of(id).is_none());
    assert_eq!(
        state.players[&player]
            .inventory
            .slots
            .iter()
            .filter(|item| **item == stack)
            .count(),
        1
    );
    super::flight::run(&mut state, &mut intents, std::iter::once(id));
    assert_eq!(
        state.players[&player]
            .inventory
            .slots
            .iter()
            .filter(|item| **item == stack)
            .count(),
        1
    );
    assert!(
        intents.is_empty(),
        "returning tridents do not damage bodies"
    );
}

#[test]
fn loyalty_keeps_custody_when_inventory_is_full_and_retries_without_duplication() {
    let (mut state, player, centre) = setup();
    let stone = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap(),
        64,
    );
    state
        .players
        .get_mut(&player)
        .unwrap()
        .inventory
        .slots
        .fill(stone);
    let owner = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let stack = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:trident").unwrap())
            .unwrap(),
        1,
    )
    .with_enchantment(Identifier::parse("minecraft:loyalty").unwrap(), 1);
    let mut record = ProjectileState::new(ProjectileKind::Trident, Some(owner), stack.clone());
    record.stuck = true;
    record.pickup = true;
    let id = launch(
        &mut state,
        record,
        [centre[0], centre[1] + 1.0, centre[2]],
        [0.0; 3],
    )
    .unwrap();
    let mut intents = Vec::new();
    super::flight::run(&mut state, &mut intents, std::iter::once(id));
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(
        state.entities.row(slot).projectile.as_ref().unwrap().stack,
        stack
    );
    state.players.get_mut(&player).unwrap().inventory.slots[9] = ItemStack::EMPTY;
    super::flight::run(&mut state, &mut intents, std::iter::once(id));
    assert!(state.entities.slot_of(id).is_none());
    assert_eq!(state.players[&player].inventory.slots[9], stack);
    super::flight::run(&mut state, &mut intents, std::iter::once(id));
    assert_eq!(state.players[&player].inventory.slots[9], stack);
}

#[test]
fn burning_arrow_primes_exact_tnt_cell_once_on_negative_axis_impact() {
    let (mut state, _, centre) = setup();
    let tnt = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:tnt").unwrap())
        .unwrap()
        .default
        .0;
    let target = crate::boundary::BlockPos {
        x: centre[0].floor() as i32 + 3,
        y: centre[1] as i32,
        z: centre[2].floor() as i32,
    };
    state.world.set_block(target.x, target.y, target.z, tnt);
    let mut record = ProjectileState::new(ProjectileKind::Arrow, None, arrow_stack(&state));
    record.flame = true;
    let id = launch(
        &mut state,
        record,
        [
            f64::from(target.x) + 3.0,
            f64::from(target.y) + 0.5,
            centre[2],
        ],
        [-4.0, 0.0, 0.0],
    )
    .unwrap();
    let mut intents = Vec::new();
    super::flight::run(&mut state, &mut intents, std::iter::once(id));
    assert!(state.entities.slot_of(id).is_none());
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(state.registry.states.air)
    );
    assert_eq!(
        state
            .entities
            .slots()
            .filter(|slot| state.entities.row(*slot).tnt.is_some())
            .count(),
        1
    );
    super::flight::run(&mut state, &mut intents, std::iter::once(id));
    assert_eq!(
        state
            .entities
            .slots()
            .filter(|slot| state.entities.row(*slot).tnt.is_some())
            .count(),
        1
    );
    assert!(
        state
            .merge_block_writes
            .contains(&(target, state.registry.states.air))
    );
}

#[test]
fn swept_sphere_uses_rounded_box_corners_not_expanded_slabs() {
    let min = [0.0, 0.0, 0.0];
    let max = [1.0, 1.0, 1.0];
    let origin = [-1.0, 1.10, 0.5];
    let delta = [3.0, 0.0, 0.0];
    let radius = 0.12;
    let contact = super::flight::swept_sphere_enters(origin, delta, min, max, radius).unwrap();
    let rounded_x = -(radius * radius - 0.10_f64.powi(2)).sqrt();
    assert!((contact - (rounded_x + 1.0) / 3.0).abs() < 1.0e-12);
    let expanded_slab = super::flight::segment_enters(
        origin,
        delta,
        min.map(|value| value - radius),
        max.map(|value| value + radius),
    )
    .unwrap();
    assert!(contact > expanded_slab);
    assert!(
        super::flight::swept_sphere_enters([-1.0, 1.10, 1.10], delta, min, max, radius,).is_none()
    );
}

#[test]
fn swept_sphere_retains_inside_tangent_and_zero_radius_contacts() {
    let min = [0.0; 3];
    let max = [1.0; 3];
    assert_eq!(
        super::flight::swept_sphere_enters([0.5; 3], [0.0; 3], min, max, 0.25),
        Some(0.0),
    );
    assert_eq!(
        super::flight::swept_sphere_enters([-1.0, 1.5, 0.5], [3.0, 0.0, 0.0], min, max, 0.5),
        Some(1.0 / 3.0),
    );
    let origin = [-1.0, 0.5, 0.5];
    let delta = [3.0, 0.0, 0.0];
    assert_eq!(
        super::flight::swept_sphere_enters(origin, delta, min, max, 0.0),
        super::flight::segment_enters(origin, delta, min, max),
    );
    assert!(super::flight::swept_sphere_enters(origin, delta, min, max, -0.1).is_none());
}

#[test]
fn attached_firework_rides_owner_and_detaches_safely_when_owner_lost() {
    let (mut state, player, _) = setup();
    let owner_uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let player_slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(player_slot, [0.5, 201.0, 0.5]);
    let mut record = ProjectileState::new(
        ProjectileKind::Firework,
        Some(owner_uuid),
        arrow_stack(&state),
    );
    record.attached_to = Some(owner_uuid);
    let id = super::launch(&mut state, record, [1.5, 201.0, 0.5], [1.0, 0.0, 0.0]).expect("launch");
    let mut intents = Vec::new();
    super::flight::run(&mut state, &mut intents, std::iter::once(id));
    assert_eq!(
        state.entities.pos(state.entities.slot_of(id).unwrap()),
        [0.5, 201.0, 0.5]
    );
    state.entities.body_mut(player_slot).alive = false;
    super::flight::run(&mut state, &mut intents, std::iter::once(id));
    let record = state
        .entities
        .row(state.entities.slot_of(id).unwrap())
        .projectile
        .clone()
        .unwrap();
    assert!(record.attached_to.is_none());
    assert!(
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .appearance
            .extra
            .contains(&(
                9,
                crate::boundary::EntityDataValue::OptionalUnsignedInt(None)
            ))
    );
}
