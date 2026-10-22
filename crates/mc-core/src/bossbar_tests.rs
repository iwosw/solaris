//! `/bossbar` execution tests: exact feedback keys and `PlayerChange::BossBar`
//! sequences for add/set players/value/max/visible/remove, join sync, errors.

use super::*;
use crate::boundary::{BossBarChange, ConnectionId, EntityId, PlayerChange, PlayerInput};
use crate::command_bossbar::BossBarCommand;
use std::sync::Arc;

fn fixture() -> (TickState, EntityId, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 1, 4096);
    crate::tick::run_tick(
        &mut state,
        &[
            PlayerInput::Join {
                connection: ConnectionId(1),
                uuid: uuid::Uuid::new_v4(),
                name: "Alice".into(),
                permission_level: 4,
            },
            PlayerInput::Join {
                connection: ConnectionId(2),
                uuid: uuid::Uuid::new_v4(),
                name: "Bob".into(),
                permission_level: 4,
            },
        ],
    );
    let alice = state.connections.entity_of(ConnectionId(1)).unwrap();
    let bob = state.connections.entity_of(ConnectionId(2)).unwrap();
    state.outbox.clear();
    (state, alice, bob)
}

fn run(state: &mut TickState, executor: EntityId, command: &BossBarCommand) -> ConnectionId {
    let connection = state.connections.entity_connection(executor).unwrap();
    execute(state, connection, executor, command);
    connection
}

fn translations(state: &TickState, connection: ConnectionId) -> Vec<String> {
    state
        .outbox
        .get(&connection)
        .map(|queue| {
            queue
                .iter()
                .filter_map(|change| match change {
                    PlayerChange::StyledChat {
                        content: mc_nbt::Tag::Compound(fields),
                        ..
                    } => fields.iter().find_map(|(name, value)| {
                        (name == "translate").then(|| match value {
                            mc_nbt::Tag::String(key) => key.clone(),
                            _ => String::new(),
                        })
                    }),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn boss_changes(state: &TickState, connection: ConnectionId) -> Vec<BossBarChange> {
    state
        .outbox
        .get(&connection)
        .map(|queue| {
            queue
                .iter()
                .filter_map(|change| match change {
                    PlayerChange::BossBar { change, .. } => Some(change.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

fn add_command(id: &str) -> BossBarCommand {
    BossBarCommand::Add {
        id: id.into(),
        name: r#"{"text":"Boss"}"#.into(),
    }
}

#[test]
fn uuid_is_stable_per_id() {
    assert_eq!(bar_uuid("minecraft:a"), bar_uuid("minecraft:a"));
    assert_ne!(bar_uuid("minecraft:a"), bar_uuid("minecraft:b"));
}

#[test]
fn add_defaults_and_duplicate_key() {
    let (mut state, alice, _) = fixture();
    let connection = run(&mut state, alice, &add_command("test:bar"));
    assert!(
        translations(&state, connection).contains(&"commands.bossbar.create.success".to_owned())
    );
    let bar = state.bossbars.bars.get("test:bar").unwrap();
    assert_eq!((bar.value, bar.max), (0, 100));
    assert_eq!(bar.color, BarColor::White);
    assert_eq!(bar.overlay, BarOverlay::Progress);
    assert!(bar.visible);
    assert!(bar.players.is_empty());
    // New bars have no members: nobody gets packets.
    assert!(boss_changes(&state, connection).is_empty());

    state.outbox.clear();
    run(&mut state, alice, &add_command("test:bar"));
    assert!(
        translations(&state, connection).contains(&"commands.bossbar.create.failed".to_owned())
    );
}

#[test]
fn set_players_add_sends_add_and_remove_sends_remove() {
    let (mut state, alice, bob) = fixture();
    run(&mut state, alice, &add_command("test:bar"));
    state.outbox.clear();

    let bob_connection = state.connections.entity_connection(bob).unwrap();
    run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: Some("Bob".into()),
        },
    );
    assert!(boss_changes(&state, bob_connection).iter().any(|change| matches!(
        change,
        BossBarChange::Add { progress, color: 6, overlay: 0, flags: 0, .. } if *progress == 0.0
    )));

    state.outbox.clear();
    run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: None,
        },
    );
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(change, BossBarChange::Remove))
    );
    // Clearing twice is unchanged (vanilla ERROR_NO_PLAYER_CHANGE).
    state.outbox.clear();
    let connection = run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: None,
        },
    );
    assert!(state.outbox[&connection].iter().any(|change| matches!(
        change,
        PlayerChange::StyledChat { content, .. }
        if format!("{content:?}").contains("commands.bossbar.set.players.unchanged")
    )));
}

#[test]
fn set_value_max_send_progress() {
    let (mut state, alice, bob) = fixture();
    run(&mut state, alice, &add_command("test:bar"));
    run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: Some("Bob".into()),
        },
    );
    state.outbox.clear();
    let bob_connection = state.connections.entity_connection(bob).unwrap();

    run(
        &mut state,
        alice,
        &BossBarCommand::SetValue {
            id: "test:bar".into(),
            value: 25,
        },
    );
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(
                change,
                BossBarChange::Progress(value) if (*value - 0.25).abs() < 1e-6
            ))
    );
    run(
        &mut state,
        alice,
        &BossBarCommand::SetMax {
            id: "test:bar".into(),
            max: 50,
        },
    );
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(
                change,
                BossBarChange::Progress(value) if (*value - 0.5).abs() < 1e-6
            ))
    );
    // Same value twice is unchanged.
    let connection = run(
        &mut state,
        alice,
        &BossBarCommand::SetValue {
            id: "test:bar".into(),
            value: 25,
        },
    );
    assert!(state.outbox[&connection].iter().any(|change| matches!(
        change,
        PlayerChange::StyledChat { content, .. }
        if format!("{content:?}").contains("commands.bossbar.set.value.unchanged")
    )));
}

#[test]
fn set_visible_hides_and_restores() {
    let (mut state, alice, bob) = fixture();
    run(&mut state, alice, &add_command("test:bar"));
    run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: Some("Bob".into()),
        },
    );
    state.outbox.clear();
    let bob_connection = state.connections.entity_connection(bob).unwrap();

    let connection = run(
        &mut state,
        alice,
        &BossBarCommand::SetVisible {
            id: "test:bar".into(),
            visible: false,
        },
    );
    assert!(
        translations(&state, connection)
            .contains(&"commands.bossbar.set.visible.success.hidden".to_owned())
    );
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(change, BossBarChange::Remove))
    );

    state.outbox.clear();
    run(
        &mut state,
        alice,
        &BossBarCommand::SetVisible {
            id: "test:bar".into(),
            visible: true,
        },
    );
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(change, BossBarChange::Add { .. }))
    );
}

#[test]
fn set_name_color_style_send_updates() {
    let (mut state, alice, bob) = fixture();
    run(&mut state, alice, &add_command("test:bar"));
    run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: Some("Bob".into()),
        },
    );
    state.outbox.clear();
    let bob_connection = state.connections.entity_connection(bob).unwrap();

    run(
        &mut state,
        alice,
        &BossBarCommand::SetColor {
            id: "test:bar".into(),
            color: BarColor::Red,
        },
    );
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(
                change,
                BossBarChange::Style {
                    color: 2,
                    overlay: 0
                }
            ))
    );
    run(
        &mut state,
        alice,
        &BossBarCommand::SetStyle {
            id: "test:bar".into(),
            overlay: BarOverlay::Notched10,
        },
    );
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(
                change,
                BossBarChange::Style {
                    color: 2,
                    overlay: 2
                }
            ))
    );
    run(
        &mut state,
        alice,
        &BossBarCommand::SetName {
            id: "test:bar".into(),
            name: r#"{"text":"Renamed"}"#.into(),
        },
    );
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(change, BossBarChange::Name(_)))
    );
}

#[test]
fn remove_drops_members_and_unknown_keys_error() {
    let (mut state, alice, bob) = fixture();
    run(&mut state, alice, &add_command("test:bar"));
    run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: Some("Bob".into()),
        },
    );
    state.outbox.clear();
    let bob_connection = state.connections.entity_connection(bob).unwrap();

    let connection = run(
        &mut state,
        alice,
        &BossBarCommand::Remove {
            id: "test:bar".into(),
        },
    );
    assert!(
        translations(&state, connection).contains(&"commands.bossbar.remove.success".to_owned())
    );
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(change, BossBarChange::Remove))
    );
    assert!(!state.bossbars.bars.contains_key("test:bar"));

    state.outbox.clear();
    run(
        &mut state,
        alice,
        &BossBarCommand::GetValue {
            id: "test:bar".into(),
        },
    );
    assert!(translations(&state, connection).contains(&"commands.bossbar.unknown".to_owned()));
}

#[test]
fn get_and_list_keys() {
    let (mut state, alice, _) = fixture();
    let connection = run(&mut state, alice, &BossBarCommand::List);
    assert!(
        translations(&state, connection).contains(&"commands.bossbar.list.bars.none".to_owned())
    );
    run(&mut state, alice, &add_command("test:bar"));
    state.outbox.clear();
    run(&mut state, alice, &BossBarCommand::List);
    assert!(
        translations(&state, connection).contains(&"commands.bossbar.list.bars.some".to_owned())
    );
    for command in [
        BossBarCommand::GetValue {
            id: "test:bar".into(),
        },
        BossBarCommand::GetMax {
            id: "test:bar".into(),
        },
        BossBarCommand::GetVisible {
            id: "test:bar".into(),
        },
        BossBarCommand::GetPlayers {
            id: "test:bar".into(),
        },
    ] {
        state.outbox.clear();
        run(&mut state, alice, &command);
        let keys = translations(&state, connection);
        assert_eq!(keys.len(), 1, "one feedback per get, got {keys:?}");
        assert!(keys[0].starts_with("commands.bossbar.get."), "{keys:?}");
    }
    state.outbox.clear();
    run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: Some("Nobody".into()),
        },
    );
    assert!(
        translations(&state, connection).contains(&"argument.entity.notfound.player".to_owned())
    );
}

#[test]
fn join_sync_resends_member_bars() {
    let (mut state, alice, bob) = fixture();
    run(&mut state, alice, &add_command("test:bar"));
    run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: Some("Bob".into()),
        },
    );
    let bob_connection = state.connections.entity_connection(bob).unwrap();
    state.outbox.clear();
    sync_join(&mut state, bob_connection);
    assert!(
        boss_changes(&state, bob_connection)
            .iter()
            .any(|change| matches!(change, BossBarChange::Add { .. }))
    );
    // Non-members get nothing.
    let alice_connection = state.connections.entity_connection(alice).unwrap();
    state.outbox.clear();
    sync_join(&mut state, alice_connection);
    assert!(boss_changes(&state, alice_connection).is_empty());
}

fn populated() -> BossBars {
    let (mut state, alice, bob) = fixture();
    run(&mut state, alice, &add_command("test:bar"));
    run(
        &mut state,
        alice,
        &BossBarCommand::SetPlayers {
            id: "test:bar".into(),
            targets: Some("Bob".into()),
        },
    );
    run(
        &mut state,
        alice,
        &BossBarCommand::SetValue {
            id: "test:bar".into(),
            value: 25,
        },
    );
    run(
        &mut state,
        alice,
        &BossBarCommand::SetColor {
            id: "test:bar".into(),
            color: BarColor::Red,
        },
    );
    let _ = (alice, bob);
    state.bossbars.clone()
}

#[test]
fn vanilla_packed_roundtrip() {
    let bars = populated();
    assert_eq!(BossBars::from_nbt(&bars.to_nbt()).unwrap(), bars);
    assert_eq!(
        BossBars::from_nbt(&BossBars::default().to_nbt()).unwrap(),
        BossBars::default()
    );
}

#[test]
fn malformed_and_unsupported_records_are_errors() {
    let mut root = populated().to_nbt();
    let mc_nbt::Tag::Compound(root_fields) = &mut root else {
        unreachable!()
    };
    let mc_nbt::Tag::Compound(data) = &mut root_fields
        .iter_mut()
        .find(|(name, _)| name == "data")
        .unwrap()
        .1
    else {
        unreachable!()
    };
    let mc_nbt::Tag::List(bars) = &mut data.iter_mut().find(|(name, _)| name == "Bars").unwrap().1
    else {
        unreachable!()
    };
    let mc_nbt::Tag::Compound(bar) = &mut bars.elements[0] else {
        unreachable!()
    };
    // Duplicate field fails closed instead of silently winning.
    bar.push(("Value".into(), mc_nbt::Tag::Int(1)));
    assert!(BossBars::from_nbt(&root).is_err());
    // Non-default vanilla flags are unsupported, not silently dropped.
    let mut root = populated().to_nbt();
    let mc_nbt::Tag::Compound(root_fields) = &mut root else {
        unreachable!()
    };
    let mc_nbt::Tag::Compound(data) = &mut root_fields
        .iter_mut()
        .find(|(name, _)| name == "data")
        .unwrap()
        .1
    else {
        unreachable!()
    };
    let mc_nbt::Tag::List(bars) = &mut data.iter_mut().find(|(name, _)| name == "Bars").unwrap().1
    else {
        unreachable!()
    };
    let mc_nbt::Tag::Compound(bar) = &mut bars.elements[0] else {
        unreachable!()
    };
    for (name, value) in bar.iter_mut() {
        if name == "DarkenScreen" {
            *value = mc_nbt::Tag::Byte(1);
        }
    }
    assert!(BossBars::from_nbt(&root).is_err());
    assert!(BossBars::from_nbt(&mc_nbt::Tag::Compound(vec![])).is_err());
}

#[test]
fn checkpoint_and_journal_overlay_restore_bossbars() {
    let base = populated();
    let checkpoint = base.to_nbt();
    assert_eq!(BossBars::from_nbt(&checkpoint).unwrap(), base);
    assert_eq!(BossBars::from_bytes(&base.to_bytes()).unwrap(), base);
    let tail = BossBars::default();
    let replay = crate::journal::Replay {
        bossbars: Some(base.to_bytes()),
        ..Default::default()
    };
    // Journal images ride as bytes; decode on replay like checkpoint open does.
    let decoded = replay
        .bossbars
        .as_deref()
        .map(BossBars::from_bytes)
        .transpose()
        .unwrap()
        .unwrap_or_default();
    assert_eq!(decoded, base);
    let batches = vec![(
        2,
        2,
        vec![crate::journal::JournalEntry::Bossbars {
            value: tail.to_bytes(),
        }],
    )];
    let folded = crate::journal::replay_batches(replay, &batches).bossbars;
    assert_eq!(
        folded
            .as_deref()
            .map(BossBars::from_bytes)
            .transpose()
            .unwrap()
            .unwrap_or_default(),
        tail
    );
}
