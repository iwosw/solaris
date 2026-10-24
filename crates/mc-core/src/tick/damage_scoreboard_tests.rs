use super::{flat_player_fixture, join};
use crate::boundary::{ConnectionId, PlayerChange};
use crate::scoreboard::{self, ScoreboardCommand};
use crate::survival::DamageKind;
use crate::tick::DamagePiece;

#[test]
fn damage_scoreboard_auto_criteria_accept_authoritative_values() {
    let (mut state, player, _) = flat_player_fixture();
    let name = state.connections.name_of(player).unwrap().to_owned();
    state.players.get_mut(&player).unwrap().vitals.food = 7;
    state.players.get_mut(&player).unwrap().vitals.air = 123;
    state
        .players
        .get_mut(&player)
        .unwrap()
        .vitals
        .experience
        .add_points(42);
    for (criterion, expected) in [
        ("food", 7),
        ("air", 123),
        ("xp", 42),
        ("level", 4),
        ("armor", 0),
    ] {
        let command =
            ScoreboardCommand::parse("scoreboard", &["objectives", "add", criterion, criterion])
                .unwrap();
        scoreboard::execute(&mut state, player, &command);
        scoreboard::automatic(&mut state, &[], &[]);
        assert_eq!(state.scoreboard.scores[&name][criterion].value, expected);
    }
}

#[test]
fn damage_scoreboard_death_visibility_options_roundtrip() {
    let (mut state, player, _) = flat_player_fixture();
    for args in [
        vec!["add", "allies"],
        vec!["join", "allies"],
        vec!["modify", "allies", "deathMessageVisibility", "never"],
    ] {
        let command = ScoreboardCommand::parse("team", &args).unwrap();
        scoreboard::execute(&mut state, player, &command);
    }
    let saved = state.scoreboard.to_nbt();
    assert!(format!("{saved:?}").contains("DeathMessageVisibility"));
    assert_eq!(
        scoreboard::Scoreboard::from_nbt(&saved).unwrap(),
        state.scoreboard
    );
}

#[test]
fn damage_scoreboard_death_notification_ignores_broadcast_rule() {
    let (mut state, player, _) = flat_player_fixture();
    join(&mut state, 2, "observer");
    state
        .gamerules
        .insert("show_death_messages".into(), "false".into());
    state.outbox.clear();
    let slot = state.entities.slot_of(player).unwrap();
    state.merge_damage.push((
        slot,
        DamagePiece {
            amount: f32::MAX,
            kind: DamageKind::Void,
            source: None,
            physical: None,
            custom_armor: false,
        },
    ));
    state.commit_merge_effects();
    assert!(
        state.outbox[&ConnectionId(1)]
            .iter()
            .any(|change| matches!(change, PlayerChange::Died { .. }))
    );
    assert!(
        !state
            .outbox
            .get(&ConnectionId(2))
            .into_iter()
            .flatten()
            .any(|change| matches!(
                change,
                PlayerChange::Chat { .. } | PlayerChange::StyledChat { .. }
            ))
    );
}

#[test]
fn damage_scoreboard_generic_kill_notifies_victim_and_filters_all_team_modes() {
    for (visibility, teammate_sees, outsider_sees) in [
        ("always", true, true),
        ("never", false, false),
        ("hideForOtherTeams", true, false),
        ("hideForOwnTeam", false, true),
    ] {
        let (mut state, player, _) = flat_player_fixture();
        join(&mut state, 2, "ally");
        join(&mut state, 3, "outsider");
        for args in [
            vec!["add", "allies"],
            vec!["join", "allies"],
            vec!["join", "allies", "ally"],
            vec!["modify", "allies", "deathMessageVisibility", visibility],
        ] {
            scoreboard::execute(
                &mut state,
                player,
                &ScoreboardCommand::parse("team", &args).unwrap(),
            );
        }
        state.outbox.clear();
        let command = crate::commands::parse("kill @s", true).unwrap();
        crate::command_execution::execute(
            &mut state,
            ConnectionId(1),
            player,
            &command,
            &mut Default::default(),
        );
        state.commit_merge_effects();
        assert!(state.outbox[&ConnectionId(1)].iter().any(|change|
            matches!(change, PlayerChange::Died { message } if format!("{message:?}").contains("death.attack.genericKill"))));
        for (connection, sees) in [(2, teammate_sees), (3, outsider_sees)] {
            assert_eq!(
                state
                    .outbox
                    .get(&ConnectionId(connection))
                    .into_iter()
                    .flatten()
                    .any(|change| matches!(change, PlayerChange::StyledChat { .. })),
                sees,
                "{visibility}"
            );
        }
        assert_eq!(
            scoreboard::Scoreboard::from_nbt(&state.scoreboard.to_nbt()).unwrap(),
            state.scoreboard
        );
    }
}

#[test]
fn damage_scoreboard_damage_identities_are_distinct_and_registry_backed() {
    for (kind, name) in [
        (DamageKind::GenericKill, "generic_kill"),
        (DamageKind::Void, "out_of_world"),
        (DamageKind::Freeze, "freeze"),
        (DamageKind::FlyIntoWall, "fly_into_wall"),
        (DamageKind::Cramming, "cramming"),
        (DamageKind::SonicBoom, "sonic_boom"),
        (DamageKind::IndirectMagic, "indirect_magic"),
        (DamageKind::Stalagmite, "stalagmite"),
    ] {
        let expected = format!("minecraft:{name}");
        assert_eq!(kind.type_name(None, false), expected);
        assert_eq!(
            mc_data::required_registry_entry("damage_type", kind.type_id(None, false) as u32)
                .unwrap()
                .as_str(),
            expected
        );
    }
    assert!(DamageKind::GenericKill.bypasses_invulnerability());
    assert!(!DamageKind::SonicBoom.bypasses_invulnerability());
}

#[test]
fn damage_scoreboard_counter_criteria_allow_writes_and_swap_sources() {
    let (mut state, player, _) = flat_player_fixture();
    for criterion in ["deathCount", "playerKillCount", "totalKillCount"] {
        for args in [
            vec!["objectives", "add", criterion, criterion],
            vec!["players", "set", "Alice", criterion, "2"],
            vec!["players", "add", "Alice", criterion, "3"],
            vec!["players", "remove", "Alice", criterion, "1"],
        ] {
            let command = ScoreboardCommand::parse("scoreboard", &args).unwrap();
            scoreboard::execute(&mut state, player, &command);
        }
        assert_eq!(state.scoreboard.scores["Alice"][criterion].value, 4);
    }
    for args in [
        vec!["objectives", "add", "scratch", "dummy"],
        vec!["players", "set", "Bob", "scratch", "9"],
        vec![
            "players",
            "operation",
            "Bob",
            "scratch",
            "><",
            "Alice",
            "deathCount",
        ],
    ] {
        let command = ScoreboardCommand::parse("scoreboard", &args).unwrap();
        scoreboard::execute(&mut state, player, &command);
    }
    assert_eq!(state.scoreboard.scores["Bob"]["scratch"].value, 4);
    assert_eq!(state.scoreboard.scores["Alice"]["deathCount"].value, 9);
}

#[test]
fn damage_scoreboard_actual_mob_and_player_deaths_credit_distinct_kill_counters_once() {
    let (mut state, player, position) = flat_player_fixture();
    let name = state.connections.name_of(player).unwrap().to_owned();
    for criterion in ["totalKillCount", "playerKillCount"] {
        for args in [
            vec!["objectives", "add", criterion, criterion],
            vec!["players", "set", "@s", criterion, "0"],
        ] {
            let command = ScoreboardCommand::parse("scoreboard", &args).unwrap();
            scoreboard::execute(&mut state, player, &command);
        }
    }
    let zombie_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let mob = state.spawn_mob(
        zombie_type,
        [position[0] + 4.0, position[1], position[2]],
        20.0,
    );
    join(&mut state, 2, "victim");
    let victim = state.connections.entity_of(ConnectionId(2)).unwrap();
    for (target, total, player_kills) in [(mob, 1, 0), (victim, 2, 1)] {
        let slot = state.entities.slot_of(target).unwrap();
        state.entities.row_mut(slot).protected_until = 0;
        state.merge_damage.push((
            slot,
            DamagePiece {
                amount: 100.0,
                kind: DamageKind::Melee,
                source: Some(crate::intent::DamageSource::Melee {
                    attacker: player,
                    position,
                    yaw: 0.0,
                    bonus_knockback: 0.0,
                    sweep: false,
                    fire_ticks: 0,
                    bane_ticks: 0,
                }),
                physical: None,
                custom_armor: false,
            },
        ));
        state.commit_merge_effects();
        assert_eq!(
            state.scoreboard.scores[&name]["totalKillCount"].value,
            total
        );
        assert_eq!(
            state.scoreboard.scores[&name]["playerKillCount"].value,
            player_kills
        );
        state.commit_merge_effects();
        assert_eq!(
            state.scoreboard.scores[&name]["totalKillCount"].value,
            total
        );
        assert_eq!(
            state.scoreboard.scores[&name]["playerKillCount"].value,
            player_kills
        );
    }
}
