use super::{flat_player_fixture, join};
use crate::boundary::{ChangeSet, ConnectionId, EntityId, Hand, PlayerChange, PlayerInput};
use crate::commands::GameMode;
use crate::intent::DamageSource;
use crate::survival::{DamageKind, Difficulty};
use crate::tick::{DamagePiece, TickState, run_tick};

fn hit(state: &mut TickState, target: EntityId, kind: DamageKind, source: Option<DamageSource>) {
    let slot = state.entities.slot_of(target).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    state.merge_damage.push((
        slot,
        DamagePiece {
            amount: 100.0,
            kind,
            source,
            physical: None,
            custom_armor: false,
        },
    ));
}

fn shield(state: &mut TickState, player: EntityId) {
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:shield").unwrap())
        .unwrap();
    let actor = state.players.get_mut(&player).unwrap();
    actor.inventory.slots[0] = mc_data::ItemStack::new(item, 1);
    actor.use_item(&state.registry.item_rules(), Hand::Main);
    for _ in 0..5 {
        actor.tick_item_use(&state.registry.item_rules());
    }
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    state.entities.set_rot(slot, 0.0, 0.0);
}

#[test]
fn parity_creative_and_spectator_ignore_damage_but_not_void() {
    for mode in [GameMode::Creative, GameMode::Spectator] {
        let (mut state, player, _) = flat_player_fixture();
        state.players.get_mut(&player).unwrap().game_mode = mode;
        for kind in [DamageKind::Melee, DamageKind::Projectile, DamageKind::Lava] {
            hit(&mut state, player, kind, None);
            state.commit_merge_effects();
            assert_eq!(
                state.players[&player].vitals.health, 20.0,
                "{mode:?} {kind:?}"
            );
            assert_eq!(
                state
                    .entities
                    .body(state.entities.slot_of(player).unwrap())
                    .hurt_at,
                None
            );
        }
        hit(&mut state, player, DamageKind::Void, None);
        assert_eq!(state.commit_merge_effects().0, vec![player]);
    }
}

#[test]
fn parity_death_orbs_once_keep_inventory_and_fire_reset() {
    for keep in [false, true] {
        let (mut state, player, _) = flat_player_fixture();
        state
            .gamerules
            .insert("keep_inventory".into(), keep.to_string());
        state
            .players
            .get_mut(&player)
            .unwrap()
            .vitals
            .experience
            .level = 10;
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.body_mut(slot).burn_ticks = 300;
        std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags |= 1;
        hit(&mut state, player, DamageKind::Void, None);
        let (_, _, spawned, _) = state.commit_merge_effects();
        let xp: i32 = spawned
            .iter()
            .filter_map(|id| state.entities.slot_of(*id))
            .filter_map(|slot| state.entities.row(slot).appearance.experience_orb)
            .map(|(value, count)| value * count)
            .sum();
        assert_eq!(xp, if keep { 0 } else { 70 });
        assert!(state.commit_merge_effects().2.is_empty());
        assert!(crate::tick::respawn_player(
            &mut state,
            ConnectionId(1),
            &mut ChangeSet::default()
        ));
        assert_eq!(state.entities.body(slot).burn_ticks, 0);
        assert_eq!(state.entities.row(slot).appearance.shared_flags & 1, 0);
    }
}

#[test]
fn parity_spectator_moves_through_wall() {
    let (mut state, player, pos) = flat_player_fixture();
    let slot = state.entities.slot_of(player).unwrap();
    state.players.get_mut(&player).unwrap().game_mode = GameMode::Spectator;
    let x = pos[0].floor() as i32 + 1;
    for y in pos[1].floor() as i32..=pos[1].floor() as i32 + 2 {
        state
            .world
            .set_block(x, y, pos[2].floor() as i32, state.registry.states.stone);
    }
    let dest = [pos[0] + 2.0, pos[1], pos[2]];
    run_tick(
        &mut state,
        &[PlayerInput::Move {
            connection: ConnectionId(1),
            position: dest,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }],
    );
    assert_eq!(state.entities.pos(slot), dest);
}

#[test]
fn parity_peaceful_recovers_without_starvation() {
    let (mut state, player, _) = flat_player_fixture();
    state.difficulty = Difficulty::Peaceful;
    let p = state.players.get_mut(&player).unwrap();
    p.vitals.food = 0;
    p.vitals.saturation = 0.0;
    p.vitals.health = 10.0;
    for _ in 0..80 {
        run_tick(&mut state, &[]);
    }
    assert!(state.players[&player].vitals.health > 10.0);
    assert!(state.players[&player].vitals.food >= 8);
}

#[test]
fn parity_mob_melee_cannot_cross_solid_wall() {
    let (mut state, player, pos) = flat_player_fixture();
    let kind = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let mob = state.spawn_mob(kind, [pos[0], pos[1], pos[2] + 1.8], 20.0);
    let z = pos[2].floor() as i32 + 1;
    for y in pos[1].floor() as i32..=pos[1].floor() as i32 + 2 {
        state
            .world
            .set_block(pos[0].floor() as i32, y, z, state.registry.states.stone);
    }
    let source = DamageSource::Melee {
        attacker: mob,
        position: [pos[0], pos[1], pos[2] + 1.8],
        yaw: 0.0,
        bonus_knockback: 0.0,
        sweep: false,
        fire_ticks: 0,
        bane_ticks: 0,
    };
    hit(&mut state, player, DamageKind::Melee, Some(source));
    state.commit_merge_effects();
    assert_eq!(state.players[&player].vitals.health, 20.0);
}

#[test]
fn parity_death_announced_to_other_players() {
    let (mut state, player, _) = flat_player_fixture();
    join(&mut state, 2, "observer");
    state.outbox.clear();
    hit(&mut state, player, DamageKind::Void, None);
    state.commit_merge_effects();
    assert!(state.outbox[&ConnectionId(2)].iter().any(
        |change| matches!(change, PlayerChange::Chat { message } if message.contains("void"))
    ));
}

#[test]
fn parity_shield_blocks_front_explosion() {
    let (mut state, player, pos) = flat_player_fixture();
    state.difficulty = Difficulty::Normal;
    shield(&mut state, player);
    crate::explosion::apply_no_blocks(&mut state, [pos[0], pos[1], pos[2] + 2.0], 3.0, &[]);
    state.commit_merge_effects();
    assert_eq!(state.players[&player].vitals.health, 20.0);
}

#[test]
fn parity_crit_requires_fall_distance_not_air_below() {
    for (height, fall, expected) in [(2.0, 0.0, 19.0), (0.2, 0.2, 18.5)] {
        let (mut state, player, mut pos) = flat_player_fixture();
        pos[1] += height;
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.set_pos(slot, pos);
        state.entities.set_ground(slot, false);
        state.players.get_mut(&player).unwrap().fall_distance = fall;
        state.tick = 30;
        let target = state.spawn_mob(
            state.registry.cow_type_id,
            [pos[0] + 1.0, pos[1], pos[2]],
            20.0,
        );
        run_tick(
            &mut state,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target,
            }],
        );
        let slot = state.entities.slot_of(target).unwrap();
        assert_eq!(
            state.entities.row(slot).health,
            expected,
            "height {height} fall {fall}"
        );
    }
}

#[test]
fn parity_zero_and_large_respawn_radius_are_safe() {
    let (mut state, _, _) = flat_player_fixture();
    let spawn = state.world.world_spawn();
    state.gamerules.insert("respawn_radius".into(), "0".into());
    for seed in 0..20 {
        let pos = crate::natural_spawn::player_spawn(&state, seed);
        assert_eq!(pos[0].floor() as i32, spawn[0]);
        assert_eq!(pos[2].floor() as i32, spawn[2]);
    }
    state
        .gamerules
        .insert("respawn_radius".into(), i32::MAX.to_string());
    assert_eq!(
        crate::natural_spawn::player_spawn(&state, 7),
        crate::natural_spawn::player_spawn(&state, 7)
    );
}

#[test]
fn parity_invalid_overworld_spawn_is_cleared() {
    let (mut state, player, pos) = flat_player_fixture();
    state.players.get_mut(&player).unwrap().respawn_point = Some(pos.map(|v| v.floor() as i32));
    hit(&mut state, player, DamageKind::Void, None);
    state.commit_merge_effects();
    assert!(crate::tick::respawn_player(
        &mut state,
        ConnectionId(1),
        &mut ChangeSet::default()
    ));
    assert_eq!(state.players[&player].respawn_point, None);
}

#[test]
fn parity_piercing_bypasses_shield_without_wear_and_arrows_rebound() {
    use crate::projectiles::{ProjectileKind, ProjectileState};
    for piercing in [0, 1] {
        let (mut state, player, pos) = flat_player_fixture();
        shield(&mut state, player);
        let mut record =
            ProjectileState::new(ProjectileKind::Arrow, None, mc_data::ItemStack::EMPTY);
        record.piercing = piercing;
        let arrow = crate::projectiles::launch(
            &mut state,
            record,
            [pos[0], pos[1] + 1.0, pos[2] + 2.0],
            [0.0, 0.0, -3.0],
        )
        .unwrap();
        run_tick(&mut state, &[]);
        if piercing == 0 {
            assert_eq!(state.players[&player].vitals.health, 20.0);
            let slot = state.entities.slot_of(arrow).unwrap();
            assert!(state.entities.vel(slot)[2] > 0.0);
            assert!(state.players[&player].inventory.slots[0].damage.is_some());
        } else {
            assert!(state.players[&player].vitals.health < 20.0);
            assert!(state.players[&player].inventory.slots[0].damage.is_none());
        }
    }
}
