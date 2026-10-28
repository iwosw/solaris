use super::*;
use crate::boundary::PlayerChange;
use crate::journal::{
    JournalEntry, JournalWriter, Replay, read_complete_batches_after, replay_batches,
};
fn fixture(can_summon: bool) -> (TickState, EntityId, BlockPos) {
    let (mut state, player, at) = crate::hostile_tests::setup(8.0);
    state.difficulty = crate::survival::Difficulty::Normal;
    let pos = BlockPos {
        x: at[0] as i32,
        y: at[1] as i32,
        z: at[2] as i32,
    };
    let block = state
        .registry
        .blocks
        .states()
        .find(|b| {
            b.block.id.path() == "sculk_shrieker"
                && b.properties.iter().any(|(k, v)| {
                    k == "can_summon" && v == if can_summon { "true" } else { "false" }
                })
                && b.properties
                    .iter()
                    .any(|(k, v)| k == "shrieking" && v == "false")
                && b.properties
                    .iter()
                    .any(|(k, v)| k == "waterlogged" && v == "false")
        })
        .unwrap()
        .id
        .0;
    state.world.set_block(pos.x, pos.y, pos.z, block);
    changed(&mut state, pos);
    (state, player, pos)
}
fn cycle(state: &mut TickState, player: EntityId, pos: BlockPos, tick: u64) {
    state.tick = tick;
    state
        .players
        .get_mut(&player)
        .unwrap()
        .warden_warning
        .cooldown_ticks = 0;
    assert!(shriek(state, pos, player));
    state.tick = tick + 90;
    run(state);
}
fn wardens(state: &TickState) -> usize {
    state
        .entities
        .slots()
        .filter(|s| {
            matches!(
                state.entities.row(*s).special_mob.as_deref(),
                Some(crate::special_mobs::Retained::Warden(_))
            )
        })
        .count()
}

#[test]
fn shrieker_level_event_reaches_only_nearby_same_dimension() {
    use crate::boundary::{ConnectionId, PlayerInput};
    let (mut state, player, pos) = fixture(false);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: uuid::Uuid::new_v4(),
            name: "far".into(),
            permission_level: 0,
        }],
    );
    let far = state.connections.entity_of(ConnectionId(2)).unwrap();
    state.entities.set_pos(
        state.entities.slot_of(far).unwrap(),
        [f64::from(pos.x) + 200.0, f64::from(pos.y), f64::from(pos.z)],
    );
    let (mut other, _, _) = fixture(false);
    other.dimension = crate::dimensions::Dimension::Nether;
    crate::tick::run_tick(
        &mut other,
        &[PlayerInput::Join {
            connection: ConnectionId(3),
            uuid: uuid::Uuid::new_v4(),
            name: "other".into(),
            permission_level: 0,
        }],
    );
    other.outbox.clear();
    state
        .other_dimensions
        .insert(crate::dimensions::Dimension::Nether, Box::new(other));
    state.outbox.clear();
    assert!(shriek(&mut state, pos, player));
    let received = |events: Option<&Vec<PlayerChange>>| {
        events.is_some_and(|events| {
        events.iter().any(|event| matches!(event, PlayerChange::ProjectileLevelEvent { event: 3007, position, .. } if *position == pos))
    })
    };
    let nearby = state.players[&player].connection;
    assert!(received(state.outbox.get(&nearby)));
    assert!(!received(state.outbox.get(&ConnectionId(2))));
    assert!(!received(
        state.other_dimensions[&crate::dimensions::Dimension::Nether]
            .outbox
            .get(&ConnectionId(3))
    ));
}
#[test]
fn three_warnings_darkness_fourth_spawns_emerging_once() {
    let (mut state, player, pos) = fixture(true);
    for n in 0..3 {
        cycle(&mut state, player, pos, n * 201);
        assert_eq!(
            state.players[&player].warden_warning.warning_level(),
            n as u32 + 1
        );
        assert_eq!(wardens(&state), 0);
    }
    assert!(
        state.players[&player]
            .active_effects
            .has(crate::effects::EffectId::Darkness)
    );
    cycle(&mut state, player, pos, 603);
    assert_eq!(wardens(&state), 1);
    let row = state
        .entities
        .slots()
        .map(|s| state.entities.row(s))
        .find(|r| {
            matches!(
                r.special_mob.as_deref(),
                Some(crate::special_mobs::Retained::Warden(_))
            )
        })
        .unwrap();
    assert_eq!((row.appearance.pose, row.appearance.spawn_data), (13, 1));
    state.tick += 1;
    run(&mut state);
    assert_eq!(wardens(&state), 1);
}
#[test]
fn spawn_rule_and_player_placed_shrieker_do_not_warn_or_spawn() {
    for natural in [true, false] {
        let (mut state, player, pos) = fixture(natural);
        if natural {
            state
                .gamerules
                .insert("spawn_wardens".into(), "false".into());
        }
        for n in 0..4 {
            cycle(&mut state, player, pos, n * 201);
        }
        assert_eq!(state.players[&player].warden_warning.warning_level(), 0);
        assert_eq!(wardens(&state), 0);
        assert!(
            !state.players[&player]
                .active_effects
                .has(crate::effects::EffectId::Darkness)
        );
    }
}
#[test]
fn warning_cooldown_and_nearby_warden_refuse_repeated_warning() {
    let (mut state, player, pos) = fixture(true);
    cycle(&mut state, player, pos, 0);
    assert!(!shriek(&mut state, pos, player));
    state
        .players
        .get_mut(&player)
        .unwrap()
        .warden_warning
        .cooldown_ticks = 0;
    let ty = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:warden").unwrap())
        .unwrap();
    state.spawn_mob(ty, vibration::centre(pos), 500.0);
    assert!(!shriek(&mut state, pos, player));
    assert_eq!(state.players[&player].warden_warning.warning_level(), 1);
}
#[test]
fn tendrils_vibration_activates_with_player_provenance() {
    let (mut state, player, pos) = fixture(true);
    vibration::emit(
        &mut state,
        vibration::centre(pos),
        GameEvent::TendrilsClicking,
        Some(player),
    );
    state.tick += 1;
    run(&mut state);
    assert_eq!(state.players[&player].warden_warning.warning_level(), 1);
    assert!(property(&state, pos, "shrieking"));
}
#[test]
fn warning_decays_every_twelve_thousand_ticks_and_exact_nbt_fields() {
    let mut warning = Warning {
        ticks_since_last_warning: 12000,
        warning_level: 3,
        cooldown_ticks: 2,
        active: true,
        ..Warning::default()
    };
    let anchor = warning;
    warning.advance_tick(1);
    assert_eq!(warning, anchor, "tick cursor is not durable state");
    assert_eq!(warning.ticks_since_last_warning(), 0);
    assert_eq!(warning.warning_level(), 2);
    assert_eq!(warning.cooldown_ticks(), 1);
    let fields = vec![("warden_spawn_tracker".into(), warning.nbt())];
    let imported = Warning::read(&fields).unwrap();
    assert_eq!(imported.nbt(), warning.nbt());
    let Tag::Compound(fields) = warning.nbt() else {
        panic!()
    };
    assert_eq!(
        fields.iter().map(|(k, _)| k.as_str()).collect::<Vec<_>>(),
        [
            "ticks_since_last_warning",
            "warning_level",
            "cooldown_ticks"
        ]
    );
    assert!(fields.iter().all(|(_, tag)| matches!(tag, Tag::Int(_))));
    assert!(
        serde_json::from_str::<Warning>(
            r#"{"ticks_since_last_warning":0,"warning_level":5,"cooldown_ticks":0}"#
        )
        .unwrap_err()
        .to_string()
        .contains("warden_spawn_tracker.warning_level: expected 0..=4")
    );
}
#[test]
fn warning_and_inflight_shriek_survive_wal_checkpoint_restart_without_duplicate_birth() {
    let (mut state, player, pos) = fixture(true);
    for n in 0..3 {
        cycle(&mut state, player, pos, n * 201);
    }
    state
        .players
        .get_mut(&player)
        .unwrap()
        .warden_warning
        .cooldown_ticks = 0;
    state.tick = 603;
    assert!(shriek(&mut state, pos, player));
    let uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid
        .to_string();
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let root = std::env::temp_dir().join(format!("solaris-shrieker-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&root).unwrap();
    let mut writer = JournalWriter::open(&root, 1).unwrap();
    writer.append(1, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    let replay = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&root.join("journal"), 0).unwrap(),
    );
    let record = &replay.players[&uuid];
    assert_eq!(record.warden_warning.warning_level(), 4);
    let config = crate::CoreConfig::new(root.clone(), 42);
    crate::native_checkpoint::tests::write_player(&config, &uuid, record, &state.registry.items)
        .unwrap();
    let saved = crate::native_checkpoint::tests::read_player(&config, &uuid, &state.registry.items)
        .unwrap();
    assert_eq!(saved.warden_warning, record.warden_warning);
    let bytes = replay.opaque_block_entities[&(pos.x, pos.y, pos.z)]
        .as_ref()
        .unwrap();
    assert!(entries.iter().any(|e|matches!(e,JournalEntry::OpaqueBlockEntity{pos:p,nbt:Some(_)} if *p==[pos.x,pos.y,pos.z])));
    let (mut resumed, other, other_pos) = fixture(true);
    assert_eq!(other_pos.y, pos.y);
    for dx in -5..=5 {
        for dz in -5..=5 {
            resumed.world.set_block(
                pos.x + dx,
                pos.y - 1,
                pos.z + dz,
                resumed.registry.states.stone,
            );
        }
    }
    // The saved BE is adopted at its original position, keeping its absolute response deadline.
    resumed.world.set_block(
        pos.x,
        pos.y,
        pos.z,
        state.world.block_at(pos.x, pos.y, pos.z).unwrap(),
    );
    resumed
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    resumed.shriekers = Shriekers::default();
    resumed.tick = record.tick;
    resumed.players.get_mut(&other).unwrap().warden_warning = saved.warden_warning;
    changed(&mut resumed, pos);
    resumed.tick = 693;
    run(&mut resumed);
    assert_eq!(wardens(&resumed), 1);
    let after = encode(&resumed, pos, &resumed.shriekers.entries[&key(pos)]);
    let (decoded, _) = decode(&after, resumed.tick).unwrap();
    assert_eq!(decoded.shriek_until, 0);
    resumed.shriekers = Shriekers::default();
    resumed.tick += 1;
    changed(&mut resumed, pos);
    run(&mut resumed);
    assert_eq!(wardens(&resumed), 1);
    drop(writer);
    std::fs::remove_dir_all(root).unwrap();
}
#[test]
fn corrupt_shrieker_records_fail_with_named_field_diagnostics() {
    let (state, _, pos) = fixture(true);
    let mut input = encode(&state, pos, &state.shriekers.entries[&key(pos)]);
    let Tag::Compound(mut fields) = mc_nbt::read_network(&mut input.as_slice()).unwrap() else {
        panic!()
    };
    fields
        .iter_mut()
        .find(|(k, _)| k == "warning_level")
        .unwrap()
        .1 = Tag::Int(5);
    let mut bad = Vec::new();
    mc_nbt::write_network(&mut bad, &Tag::Compound(fields)).unwrap();
    assert_eq!(
        validate_record(&bad).unwrap_err(),
        "warning_level: expected int 0..=4"
    );
    input.push(0);
    assert_eq!(
        validate_record(&input).unwrap_err(),
        "sculk_shrieker: trailing NBT bytes"
    );
}

#[test]
fn lazy_warning_matches_eager_ticks_including_zero_level_and_imported_large_counter() {
    for ticks in [0, 11999, 12000, 12001, i32::MAX as u32] {
        let mut warning = Warning {
            ticks_since_last_warning: ticks,
            warning_level: 4,
            cooldown_ticks: 200,
            active: true,
            ..Warning::default()
        };
        let durable = serde_json::to_vec(&warning).unwrap();
        let (mut since, mut level, mut cooldown) = (ticks, 4_u32, 200_u32);
        for tick in 1..=60010 {
            if since >= 12000 {
                level = level.saturating_sub(1);
                since = 0;
            } else {
                since += 1;
            }
            cooldown = cooldown.saturating_sub(1);
            warning.advance_tick(tick);
            assert_eq!(
                (
                    warning.ticks_since_last_warning(),
                    warning.warning_level(),
                    warning.cooldown_ticks()
                ),
                (since, level, cooldown),
                "initial counter {ticks}, tick {tick}"
            );
        }
        assert_eq!(serde_json::to_vec(&warning).unwrap(), durable);
        warning.freeze_at_tick(60010);
        let frozen = warning.nbt();
        warning.advance_tick(90000);
        assert_eq!(
            warning.nbt(),
            frozen,
            "offline time must not advance warnings"
        );
        warning.set_active(true);
        warning.advance_tick(90001);
        assert_eq!(warning.ticks_since_last_warning(), since + 1);
    }
}

#[test]
fn lazy_warning_decay_survives_native_checkpoint_wal_restart_and_anvil_export() {
    use crate::dimensions::Dimension;
    let directory = tempfile::tempdir().unwrap();
    let native = directory.path().join("world");
    let config = crate::CoreConfig::new(native.clone(), 42);
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let uuid = uuid::Uuid::from_u128(0x5741524e);
    let mut player =
        crate::journal::player_from_tag(&Tag::Compound(Vec::new()), &registry.items).unwrap();
    player.warden_warning = Warning::warned(3, 10);
    player.warden_warning.advance_tick(6010);
    player.tick = 6010;
    crate::native_checkpoint::tests::write_player(
        &config,
        &uuid.to_string(),
        &player,
        &registry.items,
    )
    .unwrap();
    let saved =
        crate::native_checkpoint::tests::read_player(&config, &uuid.to_string(), &registry.items)
            .unwrap();
    assert_eq!(saved.warden_warning.nbt(), player.warden_warning.nbt());
    let check_restart = |tick: u64, since, level| {
        let recovered =
            crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
        let mut warning = recovered.offline_players[&uuid].warden_warning;
        assert_eq!(
            (
                warning.ticks_since_last_warning(),
                warning.warning_level(),
                warning.cooldown_ticks()
            ),
            (since, level, 0)
        );
        warning.advance_tick(tick + 1000);
        assert_eq!(
            warning.ticks_since_last_warning(),
            since,
            "restart freezes offline timers"
        );
    };
    check_restart(6010, 6000, 3);
    {
        let mut writer = JournalWriter::open(&native, 1).unwrap();
        writer.append(6011, 12010, &[]).unwrap();
        writer.flush().unwrap();
    }
    check_restart(12010, 12000, 3);
    {
        let mut writer = JournalWriter::open(&native, 1).unwrap();
        writer.append(12011, 12011, &[]).unwrap();
        writer.flush().unwrap();
    }
    check_restart(12011, 0, 2);
    let batches = read_complete_batches_after(&native.join("journal"), 6010).unwrap();
    let mut replay = Replay::default();
    replay.players.insert(uuid.to_string(), saved);
    let replay = replay_batches(replay, &batches);
    assert_eq!(
        replay.players[&uuid.to_string()]
            .warden_warning
            .warning_level(),
        2
    );
    assert_eq!(
        replay.players[&uuid.to_string()]
            .warden_warning
            .ticks_since_last_warning(),
        0
    );
    let exported = directory.path().join("export");
    crate::world_conversion::export_anvil(
        &config,
        &native,
        &exported,
        &directory.path().join("export.json"),
    )
    .unwrap();
    let tag = crate::world_conversion::read_nbt(&exported.join(format!("players/data/{uuid}.dat")))
        .unwrap();
    let Tag::Compound(fields) = tag else {
        panic!("exported player compound")
    };
    assert!(!fields.iter().any(|(key, _)| key == "SolarisWardenWarning"));
    let warning = Warning::read(&fields).unwrap();
    assert_eq!(
        (
            warning.ticks_since_last_warning(),
            warning.warning_level(),
            warning.cooldown_ticks()
        ),
        (0, 2, 0)
    );
}

#[test]
fn disconnected_warning_anchor_is_journaled_and_rejoin_does_not_count_offline_ticks() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::dimensions::Dimension;
    let directory = tempfile::tempdir().unwrap();
    let mut config = crate::CoreConfig::new(directory.path().to_path_buf(), 42);
    config.view_distance = 1;
    config.simulation_distance = 0;
    let _store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let mut state = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let uuid = uuid::Uuid::from_u128(0x57414c);
    let connection = ConnectionId(1);
    let join = PlayerInput::Join {
        connection,
        uuid,
        name: "Warning".into(),
        permission_level: 0,
    };
    let (output, _, _) = crate::tick::run_tick(&mut state, std::slice::from_ref(&join));
    let actor = state.connections.entity_of(connection).unwrap();
    state.players.get_mut(&actor).unwrap().warden_warning = Warning::warned(3, output.tick);
    let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    writer.append(output.tick, output.tick, &entries).unwrap();
    for _ in 0..5 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        assert!(!entries.iter().any(|entry| matches!(
            entry,
            JournalEntry::Player { .. } | JournalEntry::Statistics { .. }
        )));
        writer.append(output.tick, output.tick, &entries).unwrap();
    }
    let (output, _, _) = crate::tick::run_tick(&mut state, &[PlayerInput::Leave { connection }]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(entry, JournalEntry::Player { .. }))
            .count(),
        1
    );
    let frozen = state.offline_players[&uuid].warden_warning;
    assert_eq!(
        (
            frozen.ticks_since_last_warning(),
            frozen.warning_level(),
            frozen.cooldown_ticks()
        ),
        (5, 3, 195)
    );
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer
        .append(output.tick + 1, output.tick + 12002, &[])
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let mut recovered =
        crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(
        recovered.offline_players[&uuid].warden_warning.nbt(),
        frozen.nbt()
    );
    let (output, _, _) = crate::tick::run_tick(&mut recovered, &[join]);
    let actor = recovered.connections.entity_of(connection).unwrap();
    let warning = recovered.players[&actor].warden_warning;
    assert_eq!(
        (
            warning.ticks_since_last_warning(),
            warning.warning_level(),
            warning.cooldown_ticks()
        ),
        (6, 3, 194)
    );
    let entries = crate::core::journal_entries_for_tick(&mut recovered, &output);
    let replay = replay_batches(
        Replay::default(),
        &[
            (output.tick, output.tick, entries),
            (output.tick + 1, output.tick + 11995, Vec::new()),
        ],
    );
    let warning = replay.players[&uuid.to_string()].warden_warning;
    assert_eq!(
        (
            warning.ticks_since_last_warning(),
            warning.warning_level(),
            warning.cooldown_ticks()
        ),
        (0, 2, 0)
    );
}
