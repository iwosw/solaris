use super::*;
use crate::tick::run_tick;

fn fixture(
    range: f64,
) -> (
    TickState,
    crate::boundary::EntityId,
    crate::boundary::EntityId,
    [f64; 3],
) {
    let (mut state, player, pos) = crate::hostile_tests::setup(range);
    state.difficulty = crate::survival::Difficulty::Normal;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.players.get_mut(&player).unwrap().vitals.food = 17;
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:creaking").unwrap())
        .unwrap();
    let blueprint = state.registry.blueprint(type_id).unwrap();
    assert_eq!(blueprint.params.max_health, 1.0);
    assert_eq!(blueprint.params.attack_damage, 3.0);
    let id = state.spawn_mob(type_id, pos, 1.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_ground(slot, true);
    let viewer = state.entities.slot_of(player).unwrap();
    state.entities.set_rot(viewer, -90.0, 0.0);
    (state, id, player, pos)
}

#[test]
fn creaking_freezes_by_gaze_and_chases_when_player_looks_away() {
    let (mut state, id, player, pos) = fixture(8.0);
    let slot = state.entities.slot_of(id).unwrap();
    assert!(observed(&state, slot));
    // An already executing movement decision must stop on the first gaze tick.
    state.entities.set_vel(slot, [3.0, 0.0, 0.0]);
    state.entities.set_horizontal_impulse(slot, [0.5, 0.0]);
    for _ in 0..10 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.entities.pos(slot), pos);
    let viewer = state.entities.slot_of(player).unwrap();
    state.entities.set_rot(viewer, 90.0, 0.0);
    assert!(!observed(&state, slot));
    for _ in 0..20 {
        run_tick(&mut state, &[]);
    }
    assert!(
        state.entities.pos(slot)[0] < pos[0],
        "unobserved creaking must chase"
    );
}

#[test]
fn creaking_gaze_uses_pitch_occlusion_and_survival_observers() {
    let (mut state, id, player, pos) = fixture(8.0);
    let slot = state.entities.slot_of(id).unwrap();
    let viewer = state.entities.slot_of(player).unwrap();
    state.entities.set_rot(viewer, -90.0, -90.0);
    assert!(!observed(&state, slot));
    state.entities.set_rot(viewer, -90.0, 0.0);
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Creative;
    assert!(!observed(&state, slot));
    state.players.get_mut(&player).unwrap().game_mode = crate::commands::GameMode::Survival;
    for y in pos[1] as i32..=pos[1] as i32 + 4 {
        for z in pos[2] as i32 - 2..=pos[2] as i32 + 2 {
            state
                .world
                .set_block(pos[0] as i32 - 4, y, z, state.registry.states.stone);
        }
    }
    assert!(
        !observed(&state, slot),
        "view vector alone cannot see through a wall"
    );
}

#[test]
fn creaking_cannot_attack_while_observed_but_hits_for_three_when_unobserved() {
    let (mut state, id, player, _) = fixture(1.5);
    for _ in 0..10 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.players[&player].vitals.health, 20.0);
    let viewer = state.entities.slot_of(player).unwrap();
    state.entities.set_rot(viewer, 90.0, 0.0);
    for _ in 0..10 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.players[&player].vitals.health, 17.0);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .health,
        1.0
    );
}
