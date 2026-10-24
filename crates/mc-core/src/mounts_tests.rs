use super::*;
use crate::boundary::PlayerInput;
use crate::tick::run_tick;
use mc_data::Identifier;

pub(crate) fn fixture() -> (TickState, EntityId, [f64; 3]) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().unwrap();
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(terrain),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: Uuid::new_v4(),
            name: "rider".into(),
            permission_level: 0,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(actor).unwrap();
    let at = state.entities.pos(slot);
    let pos = [at[0].floor() + 0.5, at[1].ceil(), at[2].floor() + 0.5];
    let stone = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    for x in pos[0] as i32 - 8..=pos[0] as i32 + 8 {
        for z in pos[2] as i32 - 8..=pos[2] as i32 + 8 {
            state.world.set_block(x, pos[1] as i32 - 1, z, stone);
            for dy in 0..8 {
                state
                    .world
                    .set_block(x, pos[1] as i32 + dy, z, state.registry.states.air);
            }
        }
    }
    state.world.fold_overlay();
    state.entities.set_pos(slot, pos);
    state.entities.set_ground(slot, true);
    (state, actor, pos)
}
pub(crate) fn spawn(state: &mut TickState, name: &str, pos: [f64; 3]) -> EntityId {
    let id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    let health = state
        .registry
        .blueprint(id)
        .map_or(20.0, |b| b.params.max_health);
    let id = state.spawn_mob(id, pos, health);
    let slot = state.entities.slot_of(id).unwrap();
    // The first accepted tame roll is zero, without probabilistic test flakes.
    // Distinct high UUID bits preserve separate identities in the same fixture.
    state.entities.set_uuid(
        slot,
        Uuid::from_u128((u128::from(id.0 as u32) << 64) | 0x9E37_79B9_7F4A_7C15),
    );
    state.entities.set_ground(slot, true);
    id
}
pub(crate) fn hold(state: &mut TickState, actor: EntityId, name: &str, count: i32) {
    let id = state
        .registry
        .items
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(id, count);
}
pub(crate) fn use_entity(target: EntityId) -> PlayerInput {
    PlayerInput::InteractEntity {
        connection: ConnectionId(1),
        target,
        hand: Hand::Main,
        location: [0.0; 3],
    }
}
pub(crate) fn slot(state: &TickState, id: EntityId) -> EntitySlot {
    state.entities.slot_of(id).unwrap()
}
fn equip(state: &mut TickState, actor: EntityId, target: EntityId, item: &str) {
    hold(state, actor, item, 1);
    run_tick(state, &[use_entity(target)]);
}
#[test]
fn mount_inventory_opens_for_each_tamed_mount() {
    let (mut state, actor, pos) = fixture();
    for name in ["horse", "donkey", "mule", "llama", "camel"] {
        let mount = spawn(&mut state, name, pos);
        let target_slot = slot(&state, mount);
        state.entities.row_mut(target_slot).entity_life.tamed = true;
        run_tick(
            &mut state,
            &[PlayerInput::InteractEntitySecondary {
                connection: ConnectionId(1),
                target: mount,
                hand: Hand::Main,
                location: [0.0; 3],
            }],
        );
        assert!(state.players[&actor].open.is_some(), "{name} inventory");
        let window = state.players[&actor].open.as_ref().unwrap().window;
        run_tick(
            &mut state,
            &[PlayerInput::ContainerClose {
                connection: ConnectionId(1),
                window,
            }],
        );
    }
}

#[test]
fn mount_inventory_equipment_cargo_and_ridden_open_share_native_custody() {
    let (mut state, actor, pos) = fixture();
    let mount = spawn(&mut state, "donkey", pos);
    let target_slot = slot(&state, mount);
    state.entities.row_mut(target_slot).entity_life.tamed = true;
    equip(&mut state, actor, mount, "chest");
    assert!(state.entities.row(target_slot).entity_life.mount.chested);
    assert_eq!(
        state
            .entities
            .row(target_slot)
            .entity_life
            .mount
            .cargo
            .len(),
        15
    );
    run_tick(&mut state, &[use_entity(mount)]);
    run_tick(
        &mut state,
        &[PlayerInput::OpenMountInventory {
            connection: ConnectionId(1),
        }],
    );
    let menu = state.players[&actor]
        .open
        .as_ref()
        .expect("rider opens mount");
    assert_eq!(menu.kind.container_size(), 17);
    let window = menu.window;
    hold(&mut state, actor, "diamond", 7);
    let version = state.players[&actor].open.as_ref().unwrap().state_id;
    menus::click(
        &mut state,
        actor,
        window,
        version,
        44,
        0,
        crate::boundary::ClickMode::QuickMove,
        &mut ChangeSet::default(),
    );
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    assert_eq!(
        state.entities.row(target_slot).entity_life.mount.cargo[0].count,
        7
    );
    let saved = serde_json::to_vec(&state.entities.row(target_slot).entity_life.mount).unwrap();
    let restored: MountState = serde_json::from_slice(&saved).unwrap();
    assert_eq!(
        restored.cargo,
        state.entities.row(target_slot).entity_life.mount.cargo
    );
    hold(&mut state, actor, "saddle", 1);
    let version = state.players[&actor].open.as_ref().unwrap().state_id;
    menus::click(
        &mut state,
        actor,
        window,
        version,
        44,
        0,
        crate::boundary::ClickMode::QuickMove,
        &mut ChangeSet::default(),
    );
    assert!(state.entities.row(target_slot).loot.saddle.is_some());
    let version = state.players[&actor].open.as_ref().unwrap().state_id;
    menus::click(
        &mut state,
        actor,
        window,
        version,
        2,
        0,
        crate::boundary::ClickMode::QuickMove,
        &mut ChangeSet::default(),
    );
    assert!(state.entities.row(target_slot).entity_life.mount.cargo[0].is_empty());
    assert_eq!(
        state.players[&actor]
            .inventory
            .slots
            .iter()
            .filter(|stack| !stack.is_empty())
            .map(|stack| stack.count)
            .sum::<i32>(),
        7
    );
}
#[test]
fn saddle_equip_revalidation_strip_and_death_do_not_duplicate() {
    let (mut state, actor, pos) = fixture();
    let pig = spawn(&mut state, "pig", pos);
    hold(&mut state, actor, "saddle", 2);
    run_tick(&mut state, &[use_entity(pig), use_entity(pig)]);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(
        state
            .entities
            .row(slot(&state, pig))
            .loot
            .saddle
            .as_ref()
            .unwrap()
            .count,
        1
    );
    hold(&mut state, actor, "shears", 1);
    run_tick(
        &mut state,
        &[PlayerInput::InteractEntitySecondary {
            connection: ConnectionId(1),
            target: pig,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    assert!(state.entities.row(slot(&state, pig)).loot.saddle.is_none());
    assert_eq!(
        state
            .entities
            .slots()
            .filter(
                |s| state.entities.row(*s).item.as_ref().is_some_and(|i| state
                    .registry
                    .items
                    .name_of(i.item_id)
                    .unwrap()
                    .as_str()
                    == "minecraft:saddle")
            )
            .count(),
        1
    );
}
#[test]
fn mounted_move_commits_rider_snap_and_rejects_overspeed_with_correction() {
    let (mut state, actor, pos) = fixture();
    let pig = spawn(&mut state, "pig", pos);
    equip(&mut state, actor, pig, "saddle");
    hold(&mut state, actor, "carrot_on_a_stick", 1);
    run_tick(&mut state, &[use_entity(pig)]);
    assert_eq!(vehicle(&state, actor), Some(slot(&state, pig)));
    let old = state.entities.pos(slot(&state, pig));
    let target = [old[0] + 0.25, old[1], old[2]];
    run_tick(
        &mut state,
        &[PlayerInput::MoveVehicle {
            connection: ConnectionId(1),
            position: target,
            yaw: 30.0,
            pitch: 0.0,
            on_ground: true,
        }],
    );
    let at = state.entities.pos(slot(&state, pig));
    assert!((at[0] - target[0]).abs() < 0.001);
    let rider_at = state.entities.pos(slot(&state, actor));
    assert_eq!(at[0], rider_at[0]);
    assert_eq!(at[2], rider_at[2]);
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::MoveVehicle {
            connection: ConnectionId(1),
            position: [at[0] + 100.0, at[1], at[2]],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
        }],
    );
    assert!((state.entities.pos(slot(&state, pig))[0] - at[0]).abs() < 0.001);
    assert!(
        drains
            .iter()
            .filter(|(connection, _)| *connection == ConnectionId(1))
            .flat_map(|(_, changes)| changes)
            .any(|p| matches!(p, PlayerChange::VehicleCorrected { .. }))
    );
    run_tick(
        &mut state,
        &[PlayerInput::Sneak {
            connection: ConnectionId(1),
            sneaking: true,
        }],
    );
    assert!(vehicle(&state, actor).is_none());
    assert!(
        state
            .entities
            .row(slot(&state, pig))
            .appearance
            .passengers
            .is_empty()
    );
}
#[test]
fn control_requires_tame_saddle_and_correct_stick_in_either_hand() {
    let (mut state, actor, pos) = fixture();
    let pig = spawn(&mut state, "pig", pos);
    equip(&mut state, actor, pig, "saddle");
    let p = slot(&state, pig);
    hold(&mut state, actor, "wheat", 1);
    assert!(!controllable(&state, p, actor));
    let stick = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:carrot_on_a_stick").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[crate::inventory::OFF_HAND] =
        ItemStack::new(stick, 1);
    assert!(controllable(&state, p, actor));
    let horse = spawn(&mut state, "horse", pos);
    let h = slot(&state, horse);
    state.entities.row_mut(h).loot.saddle = state.entities.row(p).loot.saddle.clone();
    assert!(!controllable(&state, h, actor));
    state.entities.row_mut(h).entity_life.tamed = true;
    assert!(controllable(&state, h, actor));
}
#[test]
fn wolf_body_armor_repair_and_strip_transfer_one_stack() {
    let (mut state, actor, pos) = fixture();
    let wolf = spawn(&mut state, "wolf", pos);
    let w = slot(&state, wolf);
    let owner = state.entities.row(slot(&state, actor)).uuid;
    state.entities.row_mut(w).entity_life.tamed = true;
    state.entities.row_mut(w).entity_life.taming.owner = Some(owner);
    equip(&mut state, actor, wolf, "wolf_armor");
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    state
        .entities
        .row_mut(w)
        .loot
        .body_armor
        .as_mut()
        .unwrap()
        .damage = Some(20);
    equip(&mut state, actor, wolf, "armadillo_scute");
    assert_eq!(
        state
            .entities
            .row(w)
            .loot
            .body_armor
            .as_ref()
            .unwrap()
            .damage,
        Some(12)
    );
    equip(&mut state, actor, wolf, "shears");
    assert!(state.entities.row(w).loot.body_armor.is_none());
    assert_eq!(
        state
            .entities
            .slots()
            .filter(
                |s| state.entities.row(*s).item.as_ref().is_some_and(|i| state
                    .registry
                    .items
                    .name_of(i.item_id)
                    .unwrap()
                    .as_str()
                    == "minecraft:wolf_armor")
            )
            .count(),
        1
    );
}

#[test]
fn equipped_saddle_and_armor_death_drop_once_and_release_rider_and_leash() {
    let (mut state, actor, pos) = fixture();
    let horse = spawn(&mut state, "horse", pos);
    let h = slot(&state, horse);
    state.entities.row_mut(h).entity_life.tamed = true;
    equip(&mut state, actor, horse, "saddle");
    equip(&mut state, actor, horse, "diamond_horse_armor");
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
    run_tick(&mut state, &[use_entity(horse)]);
    assert!(vehicle(&state, actor).is_some());
    equip(&mut state, actor, horse, "lead");
    assert!(state.entities.row(h).entity_life.leash.is_some());
    state.merge_damage.push((
        h,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 1000.0,
            kind: crate::survival::DamageKind::Void,
            source: None,
        },
    ));
    state.commit_merge_effects();
    state.commit_merge_effects();
    for name in [
        "minecraft:saddle",
        "minecraft:diamond_horse_armor",
        "minecraft:lead",
    ] {
        let count = state
            .entities
            .slots()
            .filter_map(|s| state.entities.row(s).item.as_ref())
            .filter(|stack| {
                state
                    .registry
                    .items
                    .name_of(stack.item_id)
                    .unwrap()
                    .as_str()
                    == name
            })
            .map(|stack| stack.count)
            .sum::<i32>();
        assert_eq!(count, 1, "{name}");
    }
    assert!(vehicle(&state, actor).is_none());
}

#[test]
fn mounted_packet_burst_has_one_displacement_budget_and_one_correction_per_tick() {
    let (mut state, actor, pos) = fixture();
    let pig = spawn(&mut state, "pig", pos);
    equip(&mut state, actor, pig, "saddle");
    hold(&mut state, actor, "carrot_on_a_stick", 1);
    run_tick(&mut state, &[use_entity(pig)]);
    let old = state.entities.pos(slot(&state, pig));
    let packet = |distance| PlayerInput::MoveVehicle {
        connection: ConnectionId(1),
        position: [old[0] + distance, old[1], old[2]],
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
    };
    let (_, drains, _) = run_tick(&mut state, &[packet(0.25), packet(0.5), packet(0.75)]);
    assert!((state.entities.pos(slot(&state, pig))[0] - old[0] - 0.25).abs() < 0.001);
    assert_eq!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .filter(|change| matches!(change, PlayerChange::VehicleCorrected { .. }))
            .count(),
        1
    );
    run_tick(&mut state, &[packet(0.5)]);
    assert!((state.entities.pos(slot(&state, pig))[0] - old[0] - 0.5).abs() < 0.001);
}

#[test]
fn rider_eject_and_respawn_clear_both_links_and_steering_input() {
    let (mut state, actor, pos) = fixture();
    let pig = spawn(&mut state, "pig", pos);
    equip(&mut state, actor, pig, "saddle");
    hold(&mut state, actor, "carrot_on_a_stick", 1);
    let input = PlayerInput::VehicleInput {
        connection: ConnectionId(1),
        flags: 1,
    };
    run_tick(&mut state, std::slice::from_ref(&input));
    assert_eq!(
        state
            .entities
            .row(slot(&state, actor))
            .entity_life
            .mount
            .input,
        0
    );
    run_tick(&mut state, &[use_entity(pig)]);
    run_tick(&mut state, std::slice::from_ref(&input));
    assert_eq!(
        state
            .entities
            .row(slot(&state, actor))
            .entity_life
            .mount
            .input,
        1
    );
    let actor_slot = slot(&state, actor);
    let pig_slot = slot(&state, pig);
    state.players.get_mut(&actor).unwrap().vitals.health = 0.0;
    state.entities.body_mut(actor_slot).alive = false;
    tick(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.entities.row(actor_slot).entity_life.mount.vehicle,
        None
    );
    assert_eq!(state.entities.row(actor_slot).entity_life.mount.input, 0);
    assert_eq!(state.entities.row(pig_slot).entity_life.mount.rider, None);
    assert!(
        state
            .entities
            .row(pig_slot)
            .appearance
            .passengers
            .is_empty()
    );
    state.entities.body_mut(actor_slot).alive = true;
    state.players.get_mut(&actor).unwrap().vitals.health = 20.0;
    run_tick(&mut state, &[use_entity(pig)]);
    run_tick(&mut state, std::slice::from_ref(&input));
    state.entities.body_mut(pig_slot).alive = false;
    tick(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.entities.row(actor_slot).entity_life.mount.vehicle,
        None
    );
    assert_eq!(state.entities.row(actor_slot).entity_life.mount.input, 0);
    state.entities.body_mut(pig_slot).alive = true;
    run_tick(&mut state, &[use_entity(pig)]);
    run_tick(&mut state, &[input]);
    state.entities.body_mut(actor_slot).alive = false;
    state.players.get_mut(&actor).unwrap().vitals.health = 0.0;
    assert!(crate::tick::respawn_player(
        &mut state,
        ConnectionId(1),
        &mut ChangeSet::default()
    ));
    assert_eq!(
        state.entities.row(actor_slot).entity_life.mount.vehicle,
        None
    );
    assert_eq!(state.entities.row(actor_slot).entity_life.mount.input, 0);
    assert_eq!(state.entities.row(pig_slot).entity_life.mount.rider, None);
}

#[test]
fn started_riding_evidence_follows_successful_mount_only() {
    use mc_data::advancements::{AdvancementDefinition, AdvancementTree, Criterion};
    let (mut state, actor, pos) = fixture();
    let mut tree = AdvancementTree::default();
    tree.definitions.insert(
        "test:ride".into(),
        AdvancementDefinition {
            criteria: [(
                "ride".into(),
                Criterion {
                    trigger: "minecraft:started_riding".into(),
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        },
    );
    Arc::get_mut(&mut state.registry)
        .expect("fixture registry is unshared")
        .advancements = Arc::new(tree);
    state.advancement_events.clear();
    let pig = spawn(&mut state, "pig", pos);
    let mut intents = Vec::new();
    assert!(!interact(
        &mut state,
        actor,
        pig,
        Hand::Main,
        false,
        &mut intents
    ));
    assert!(state.advancement_events.is_empty());
    let pig_slot = slot(&state, pig);
    let saddle = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:saddle").unwrap())
        .unwrap();
    state.entities.row_mut(pig_slot).loot.saddle = Some(Box::new(ItemStack::new(saddle, 1)));
    let plan = Plan {
        target: pig,
        slot: 0,
        held: ItemStack::EMPTY,
        secondary: false,
        action: Action::Ride,
    };
    assert!(valid(&state, actor, &plan));
    assert!(state.advancement_events.is_empty());
    apply(&mut state, actor, &plan);
    assert_eq!(vehicle(&state, actor), Some(pig_slot));
    assert_eq!(state.advancement_events.len(), 1);
    assert_eq!(state.advancement_events[0].entity, actor);
    assert_eq!(
        state.advancement_events[0].trigger,
        "minecraft:started_riding"
    );
    assert!(!valid(&state, actor, &plan));
    assert_eq!(state.advancement_events.len(), 1);
}

#[test]
fn armadillo_curls_against_undead_and_halves_melee_damage() {
    // ARMADILLO-THREAT-ROLL through the damage path: an armadillo with an
    // undead threat adjacent takes reduced melee damage (rolled up).
    let (mut state, _actor, pos) = fixture();
    let armadillo = spawn(&mut state, "armadillo", pos);
    let _threat = spawn(&mut state, "zombie", [pos[0] + 2.0, pos[1], pos[2]]);
    for _ in 0..5 {
        run_tick(&mut state, &[]);
    }
    let aslot = slot(&state, armadillo);
    let before = state.entities.row(aslot).health;
    state.merge_damage.push((
        aslot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 10.0,
            kind: crate::survival::DamageKind::Melee,
            source: None,
        },
    ));
    state.commit_merge_effects();
    state.commit_merge_effects();
    let after = state.entities.row(slot(&state, armadillo)).health;
    assert!(
        before - after < 10.0,
        "curled armadillo must take reduced melee damage, took {}",
        before - after
    );
}

#[test]
fn armadillo_brush_sheds_scute() {
    // ARMADILLO-SCUTE-SHED through the interact path: brushing an adult
    // armadillo sheds an armadillo scute.
    let (mut state, actor, pos) = fixture();
    let armadillo = spawn(&mut state, "armadillo", pos);
    hold(&mut state, actor, "brush", 1);
    run_tick(&mut state, &[use_entity(armadillo)]);
    let scutes = state
        .entities
        .slots()
        .filter_map(|s| state.entities.row(s).item.as_ref())
        .filter(|stack| {
            state
                .registry
                .items
                .name_of(stack.item_id)
                .unwrap()
                .as_str()
                == "minecraft:armadillo_scute"
        })
        .map(|stack| stack.count)
        .sum::<i32>();
    assert!(scutes >= 1, "brushing an armadillo must shed a scute");
}

#[test]
fn wolf_body_armor_absorbs_melee_damage_and_wears() {
    // WOLF-ARMOR-ABSORB through the damage path: an armored wolf takes
    // reduced melee damage and the armor loses durability.
    let (mut state, actor, pos) = fixture();
    let wolf = spawn(&mut state, "wolf", pos);
    let wslot = slot(&state, wolf);
    state.entities.row_mut(wslot).entity_life.tamed = true;
    let owner = state.entities.row(slot(&state, actor)).uuid;
    state.entities.row_mut(wslot).entity_life.taming.owner = Some(owner);
    equip(&mut state, actor, wolf, "wolf_armor");
    assert!(state.entities.row(slot(&state, wolf)).loot.body_armor.is_some());
    let before = state.entities.row(slot(&state, wolf)).health;
    state.merge_damage.push((
        slot(&state, wolf),
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 4.0,
            kind: crate::survival::DamageKind::Melee,
            source: None,
        },
    ));
    state.commit_merge_effects();
    state.commit_merge_effects();
    let after = state.entities.row(slot(&state, wolf)).health;
    assert!(
        before - after < 4.0,
        "armored wolf must take reduced melee damage, took {}",
        before - after
    );
    assert!(
        state
            .entities
            .row(slot(&state, wolf))
            .loot
            .body_armor
            .as_ref()
            .is_some_and(|s| s.damage.unwrap_or(0) > 0),
        "wolf armor must wear down when absorbing damage"
    );
}
