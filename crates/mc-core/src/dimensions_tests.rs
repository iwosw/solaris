use super::*;

#[test]
fn execute_in_commits_native_block_change_and_wal_in_destination_only() {
    let (mut state, alice, _) = worlds();
    let (output, _) = command(
        &mut state,
        1,
        "execute positioned 64 150 64 in minecraft:the_nether run setblock ~ ~ ~ minecraft:diamond_block",
    );
    let pos = crate::boundary::BlockPos { x: 8, y: 150, z: 8 };
    let nether = &output.snapshot.dimensions[&Dimension::Nether];
    assert!(
        nether
            .changes
            .blocks
            .iter()
            .any(|change| change.position == pos)
    );
    assert!(
        !output
            .changes
            .blocks
            .iter()
            .any(|change| change.position == pos)
    );
    assert_eq!(
        travel::locate(&state, alice).unwrap().0,
        Dimension::Overworld
    );
    assert!(
        state.other_dimensions[&Dimension::Nether]
            .merge_block_writes
            .is_empty()
    );
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|entry| matches!(entry, crate::journal::JournalEntry::Dimension { dimension: Dimension::Nether, entries }
        if entries.iter().any(|entry| matches!(entry, crate::journal::JournalEntry::Block { pos: [8,150,8], .. })))));
}
#[test]
fn integrity_same_tick_dimension_round_trip_survives_wal_recovery() {
    let directory = TempWorld::new();
    let (mut state, _, _) = worlds();
    let stone = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let item = crate::tick::spawn_item_entity(
        &mut state,
        [8.0, 150.0, 8.0],
        mc_data::ItemStack::new(stone, 7),
    );
    let uuid = state
        .entities
        .row(state.entities.slot_of(item).unwrap())
        .uuid;
    let inputs = [Dimension::Nether, Dimension::Overworld].map(|dimension| PlayerInput::Command {
        connection: ConnectionId(1),
        command: format!("execute in {} run tp {uuid} 8 150 8", dimension.name()),
        permission_level: 2,
    });
    let (output, _, _) = crate::tick::run_tick(&mut state, &inputs);
    assert_eq!(
        travel::locate(&state, item).unwrap().0,
        Dimension::Overworld
    );
    assert!(output.changes.entities_removed.contains(&item));
    assert!(output.changes.entities_spawned.contains(&item));
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let mut writer = crate::journal::JournalWriter::open(&directory.0, 1).unwrap();
    writer.append(1, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches =
        crate::journal::read_complete_batches_after(&directory.0.join("journal"), 0).unwrap();
    let replay = crate::journal::replay_batches(
        Default::default(),
        &journal_for_dimension(&batches, Dimension::Overworld),
    );
    assert_eq!(
        replay
            .entities
            .get(&item.0)
            .and_then(|entity| entity.item.as_ref())
            .map(|stack| stack.count),
        Some(7)
    );
    let nether = crate::journal::replay_batches(
        Default::default(),
        &journal_for_dimension(&batches, Dimension::Nether),
    );
    assert!(!nether.entities.contains_key(&item.0));
}

#[test]
fn advancement_global_dispatch_mutates_players_in_their_own_dimensions() {
    use mc_data::advancements::{AdvancementDefinition, AdvancementTree, Criterion};
    let (mut state, alice, bob) = worlds();
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    let mut tree = AdvancementTree::default();
    tree.definitions.insert(
        "test:global".into(),
        AdvancementDefinition {
            criteria: [(
                "command".into(),
                Criterion {
                    trigger: "minecraft:impossible".into(),
                    ..Default::default()
                },
            )]
            .into(),
            requirements: vec![vec!["command".into()]],
            ..Default::default()
        },
    );
    registry.advancements = Arc::new(tree);
    let registry = Arc::new(registry);
    state.registry = Arc::clone(&registry);
    for world in state.other_dimensions.values_mut() {
        world.registry = Arc::clone(&registry);
    }
    command(&mut state, 1, "advancement grant @a only test:global");
    for entity in [alice, bob] {
        let (dimension, _) = travel::locate(&state, entity).unwrap();
        assert_eq!(
            travel::world(&state, dimension).unwrap().players[&entity].advancement_progress["test:global"]
                ["done"],
            true,
        );
    }
    command(
        &mut state,
        1,
        "advancement revoke Bob only test:global command",
    );
    let nether = travel::world(&state, Dimension::Nether).unwrap();
    assert_ne!(
        nether.players[&bob].advancement_progress["test:global"]["done"],
        true
    );
    assert_eq!(
        state.players[&alice].advancement_progress["test:global"]["done"],
        true
    );
}

#[test]
fn vanilla_dimension_keys_and_region_directories_are_closed() {
    let root = Path::new("world");
    for dimension in Dimension::ALL {
        assert_eq!(Dimension::parse(dimension.name()), Some(dimension));
        assert_eq!(
            serde_json::from_str::<Dimension>(&serde_json::to_string(&dimension).unwrap()).unwrap(),
            dimension
        );
    }
    assert_eq!(
        Dimension::Overworld.directory(root),
        root.join("dimensions/minecraft/overworld")
    );
    assert_eq!(
        Dimension::Nether.directory(root),
        root.join("dimensions/minecraft/the_nether")
    );
    assert_eq!(
        Dimension::End.directory(root),
        root.join("dimensions/minecraft/the_end")
    );
    assert!(serde_json::from_str::<Dimension>("\"minecraft:unknown\"").is_err());
    assert!(Dimension::Nether.water_evaporates());
    assert!(!Dimension::End.beds_work());
    assert!(!Dimension::Nether.has_weather());
}

#[test]
fn global_tick_budget_follows_dimension_swaps_without_refilling() {
    use crate::tick_budget::{Consumer, DECISIONS_PER_TICK, Kind, PATH_NODES_PER_TICK};

    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut root = dimension_state(Dimension::Overworld, Arc::clone(&registry), None);
    let mut children = [
        dimension_state(Dimension::Nether, Arc::clone(&registry), None),
        dimension_state(Dimension::End, registry, None),
    ];
    root.tick_budget.reset(7);
    assert!(
        root.tick_budget
            .try_charge(root.dimension, Consumer::Brain, Kind::Decision, 4,)
    );
    assert!(
        root.tick_budget
            .try_charge(root.dimension, Consumer::Brain, Kind::PathNode, 4,)
    );

    for (entry, index) in [0, 1, 0].into_iter().enumerate() {
        let world = &mut children[index];
        swap_globals(&mut root, world);
        assert_eq!(
            world.tick_budget.remaining(Kind::Decision),
            DECISIONS_PER_TICK - 4 - entry,
        );
        assert_eq!(
            world.tick_budget.remaining(Kind::PathNode),
            PATH_NODES_PER_TICK - 4 - entry
        );
        assert!(
            world
                .tick_budget
                .try_charge(world.dimension, Consumer::Brain, Kind::Decision, 1)
        );
        assert!(
            world
                .tick_budget
                .try_charge(world.dimension, Consumer::Brain, Kind::PathNode, 1)
        );
        swap_globals(&mut root, world);
    }

    root.tick_budget.reset(7);
    assert_eq!(
        root.tick_budget.remaining(Kind::Decision),
        DECISIONS_PER_TICK - 7
    );
    assert_eq!(
        root.tick_budget.remaining(Kind::PathNode),
        PATH_NODES_PER_TICK - 7
    );
    root.tick_budget.exhaust_for_test(Kind::Decision);
    root.tick_budget.exhaust_for_test(Kind::PathNode);
    assert_eq!(root.tick_budget.remaining(Kind::Decision), 0);
    assert_eq!(root.tick_budget.remaining(Kind::PathNode), 0);
    assert!(
        !root
            .tick_budget
            .try_charge(root.dimension, Consumer::Brain, Kind::Decision, 1)
    );
    assert!(
        !root
            .tick_budget
            .try_charge(root.dimension, Consumer::Brain, Kind::PathNode, 1)
    );
    root.tick_budget.reset(8);
    swap_globals(&mut root, &mut children[0]);
    assert_eq!(
        children[0].tick_budget.remaining(Kind::Decision),
        DECISIONS_PER_TICK,
    );
    assert_eq!(
        children[0].tick_budget.remaining(Kind::PathNode),
        PATH_NODES_PER_TICK,
    );
    swap_globals(&mut root, &mut children[0]);
}

use crate::boundary::{ConnectionId, EntityId, PlayerChange, PlayerInput};
use crate::tick::TickState;
use std::sync::Arc;

fn dimension_state(
    dimension: Dimension,
    registry: Arc<crate::registry::CoreRegistry>,
    directory: Option<std::path::PathBuf>,
) -> TickState {
    let generator: Arc<dyn mc_world::ChunkGenerator> = match dimension {
        Dimension::Overworld => Arc::new(mc_worldgen::TerrainGenerator::new(
            42,
            Arc::clone(&registry.blocks),
        )),
        Dimension::Nether => Arc::new(
            mc_worldgen::NetherGenerator::try_new(42, Arc::clone(&registry.blocks)).unwrap(),
        ),
        Dimension::End => {
            Arc::new(mc_worldgen::EndGenerator::try_new(42, Arc::clone(&registry.blocks)).unwrap())
        }
    };
    let world = if let Some(directory) = directory {
        crate::world::ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&import_config(&directory)).unwrap(),
            crate::native_checkpoint::dimension_id(dimension),
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 150, 8],
            dimension.geometry(),
        )
    } else {
        crate::world::ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 150, 8],
            dimension.geometry(),
        )
    };
    let mut state = TickState::new(registry, world, 0, 4096);
    state.dimension = dimension;
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let want = [(mc_world::ChunkPos { x: 0, z: 0 }, 0)]
        .into_iter()
        .collect();
    state.world.ensure_blocking(&want);
    for x in 0..16 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 149, z, state.registry.states.stone);
        }
    }
    state.world.fold_overlay();
    state
}

fn worlds() -> (TickState, EntityId, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = dimension_state(Dimension::Overworld, Arc::clone(&registry), None);
    for dimension in [Dimension::Nether, Dimension::End] {
        state.other_dimensions.insert(
            dimension,
            Box::new(dimension_state(dimension, Arc::clone(&registry), None)),
        );
    }
    crate::tick::run_tick(
        &mut state,
        &[
            PlayerInput::Join {
                connection: ConnectionId(1),
                uuid: uuid::Uuid::from_u128(1),
                name: "Alice".into(),
                permission_level: 0,
            },
            PlayerInput::Join {
                connection: ConnectionId(2),
                uuid: uuid::Uuid::from_u128(2),
                name: "Bob".into(),
                permission_level: 0,
            },
        ],
    );
    let alice = state.connections.entity_of(ConnectionId(1)).unwrap();
    let bob = state.connections.entity_of(ConnectionId(2)).unwrap();
    command(&mut state, 1, "execute in the_nether run tp Bob 8 150 8");
    assert_eq!(travel::locate(&state, bob).unwrap().0, Dimension::Nether);
    (state, alice, bob)
}

fn command(
    state: &mut TickState,
    connection: u64,
    command: &str,
) -> (
    crate::boundary::TickOutput,
    Vec<(ConnectionId, Vec<PlayerChange>)>,
) {
    let (output, drains, _) = crate::tick::run_tick(
        state,
        &[PlayerInput::Command {
            connection: ConnectionId(connection),
            command: command.into(),
            permission_level: 2,
        }],
    );
    (output, drains)
}

fn translation(change: &PlayerChange, key: &str) -> bool {
    matches!(change, PlayerChange::StyledChat { content: mc_nbt::Tag::Compound(fields), .. }
        if fields.iter().any(|(name, value)| name == "translate" && value == &mc_nbt::Tag::String(key.into())))
}

#[test]
fn shared_command_execution_reports_missing_player_and_entity_targets() {
    let (mut state, _, _) = worlds();
    for connection in [1, 2] {
        for (input, key) in [
            (
                "gamemode creative Missing",
                "argument.entity.notfound.player",
            ),
            (
                "give Missing minecraft:stone",
                "argument.entity.notfound.player",
            ),
            ("kill Missing", "argument.entity.notfound.entity"),
            (
                "effect give Missing minecraft:speed",
                "argument.entity.notfound.entity",
            ),
        ] {
            let (_, drains) = command(&mut state, connection, input);
            let feedback: Vec<_> = drains
                .iter()
                .filter(|(recipient, _)| *recipient == ConnectionId(connection))
                .flat_map(|(_, changes)| changes)
                .collect();
            assert!(
                format!("{feedback:?}").contains(key),
                "{input}: {feedback:?}"
            );
        }
    }
}

#[test]
fn global_give_and_message_reach_the_nether_player_once() {
    let (mut state, alice, bob) = worlds();
    command(&mut state, 1, "give @a minecraft:stone 3");
    let stone = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    for entity in [alice, bob] {
        let (dimension, _) = travel::locate(&state, entity).unwrap();
        let player = &travel::world(&state, dimension).unwrap().players[&entity];
        assert_eq!(
            player
                .inventory
                .slots
                .iter()
                .filter(|stack| stack.item_id == stone)
                .map(|stack| stack.count)
                .sum::<i32>(),
            3
        );
    }
    let (_, drains) = command(&mut state, 1, "msg Bob hello");
    let incoming: Vec<_> = drains
        .iter()
        .filter(|(connection, _)| *connection == ConnectionId(2))
        .flat_map(|(_, queue)| queue)
        .filter(|change| translation(change, "commands.message.display.incoming"))
        .collect();
    assert_eq!(incoming.len(), 1);
    assert!(format!("{:?}", incoming[0]).contains("Alice"));
    let (_, drains) = command(&mut state, 2, "msg Alice reply");
    assert_eq!(
        drains
            .iter()
            .filter(|(connection, _)| *connection == ConnectionId(1))
            .flat_map(|(_, queue)| queue)
            .filter(|change| translation(change, "commands.message.display.incoming"))
            .count(),
        1
    );
}

#[test]
fn global_selector_limit_is_applied_once_across_worlds() {
    let (mut state, alice, bob) = worlds();
    command(
        &mut state,
        1,
        "give @a[limit=1,sort=arbitrary] minecraft:diamond 1",
    );
    let diamond = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond").unwrap())
        .unwrap();
    let total: i32 = [alice, bob]
        .into_iter()
        .map(|entity| {
            let (dimension, _) = travel::locate(&state, entity).unwrap();
            travel::world(&state, dimension).unwrap().players[&entity]
                .inventory
                .slots
                .iter()
                .filter(|stack| stack.item_id == diamond)
                .map(|stack| stack.count)
                .sum::<i32>()
        })
        .sum();
    assert_eq!(total, 1);
}

#[test]
fn weather_command_from_the_nether_only_changes_the_overworld() {
    let (mut state, _, _) = worlds();
    let (output, _) = command(&mut state, 2, "weather rain 60");
    assert!(output.snapshot.raining);
    for dimension in [Dimension::Nether, Dimension::End] {
        let snapshot = &output.in_dimension(dimension).unwrap().snapshot;
        assert!(!snapshot.raining);
        assert_eq!(snapshot.rain_level, 0.0);
        assert_eq!(snapshot.thunder_level, 0.0);
        assert_eq!(
            state.other_dimensions[&dimension].weather.0,
            crate::commands::Weather::Clear
        );
    }
    assert_eq!(state.weather.0, crate::commands::Weather::Rain);
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    assert!(output.snapshot.rain_level > 0.0);
}

#[test]
fn scoreboard_and_chat_are_global_but_entity_visibility_is_not() {
    let (mut state, _, bob) = worlds();
    let (_, drains) = command(&mut state, 1, "scoreboard objectives add points dummy");
    assert!(
        drains
            .iter()
            .filter(|(connection, _)| *connection == ConnectionId(2))
            .flat_map(|(_, queue)| queue)
            .any(|change| matches!(change, PlayerChange::Scoreboard { .. }))
    );
    command(&mut state, 1, "scoreboard players set @a points 7");
    assert_eq!(state.scoreboard.scores["Alice"]["points"].value, 7);
    assert_eq!(state.scoreboard.scores["Bob"]["points"].value, 7);
    let (output, drains, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Chat {
            connection: ConnectionId(2),
            message: "nether chat".into(),
        }],
    );
    assert!(
        !output
            .snapshot
            .entities
            .iter()
            .any(|entity| entity.id == bob)
    );
    for connection in [ConnectionId(1), ConnectionId(2)] {
        assert_eq!(
            drains
                .iter()
                .filter(|(recipient, _)| *recipient == connection)
                .flat_map(|(_, queue)| queue)
                .filter(
                    |change| matches!(change, PlayerChange::StyledChat { content, .. }
                if format!("{content:?}").contains("nether chat"))
                )
                .count(),
            1
        );
    }
}

#[test]
fn private_and_team_messages_cross_dimensions() {
    let (mut state, _, _) = worlds();
    for alias in ["msg", "tell", "w"] {
        let (_, drains) = command(&mut state, 1, &format!("{alias} Bob hello"));
        assert!(
            drains
                .iter()
                .filter(|(id, _)| *id == ConnectionId(2))
                .flat_map(|(_, changes)| changes)
                .any(|change| translation(change, "commands.message.display.incoming"))
        );
    }
    command(&mut state, 1, "team add red");
    command(&mut state, 1, "team join red Alice");
    command(&mut state, 1, "team join red Bob");
    for alias in ["teammsg", "tm"] {
        let (_, drains) = command(&mut state, 1, &format!("{alias} hello"));
        assert!(
            drains
                .iter()
                .filter(|(id, _)| *id == ConnectionId(2))
                .flat_map(|(_, changes)| changes)
                .any(|change| translation(change, "chat.type.team.text"))
        );
    }
}

#[test]
fn roster_spans_dimensions_without_changing_on_travel() {
    let (mut state, _, _) = worlds();
    assert_eq!(state.dimension_globals.roster.len(), 2);
    let roster = Arc::clone(&state.dimension_globals.roster);
    command(&mut state, 2, "execute in the_end run tp @s 8 150 8");
    assert!(Arc::ptr_eq(&roster, &state.dimension_globals.roster));
    let (_, drains, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: ConnectionId(2),
        }],
    );
    assert_eq!(state.dimension_globals.roster.len(), 1);
    assert!(
        drains
            .iter()
            .filter(|(connection, _)| *connection == ConnectionId(1))
            .flat_map(|(_, queue)| queue)
            .any(
                |change| matches!(change, PlayerChange::Roster { removed, .. }
            if removed.as_ref() == [uuid::Uuid::from_u128(2)])
            )
    );
}

#[test]
fn global_spawnpoint_uses_source_dimension_and_cross_dimension_death_returns_there() {
    let (mut state, alice, bob) = worlds();
    command(&mut state, 2, "spawnpoint Alice 8 150 8");
    assert_eq!(state.players[&alice].respawn_dimension, Dimension::Nether);
    let want = crate::world::chunks_in_radius(mc_world::ChunkPos { x: 0, z: 0 }, 1)
        .into_iter()
        .map(|position| (position, 0))
        .collect();
    state.world.ensure_blocking(&want);
    command(&mut state, 1, "kill Bob");
    assert!(
        !state.other_dimensions[&Dimension::Nether]
            .entities
            .body(travel::locate(&state, bob).unwrap().1)
            .alive
    );
    let (output, drains, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Respawn {
            connection: ConnectionId(2),
        }],
    );
    assert_eq!(travel::locate(&state, bob).unwrap().0, Dimension::Overworld);
    assert!(
        output
            .snapshot
            .entities
            .iter()
            .any(|entity| entity.id == bob)
    );
    assert!(
        drains
            .iter()
            .filter(|(connection, _)| *connection == ConnectionId(2))
            .flat_map(|(_, queue)| queue)
            .any(|change| matches!(
                change,
                PlayerChange::Respawned {
                    dimension: Dimension::Overworld,
                    ..
                }
            ))
    );
}

struct TempWorld(std::path::PathBuf);

impl TempWorld {
    fn new() -> Self {
        let directory =
            std::env::temp_dir().join(format!("mc-core-dimensions-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        Self(directory)
    }
}

impl Drop for TempWorld {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn real_generators_and_native_roundtrip_keep_same_coordinates_isolated() {
    let directory = TempWorld::new();
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    for (dimension, marker) in Dimension::ALL.into_iter().zip([
        "minecraft:stone",
        "minecraft:glowstone",
        "minecraft:obsidian",
    ]) {
        let id = registry
            .blocks
            .block(&mc_data::Identifier::parse(marker).unwrap())
            .unwrap()
            .default
            .0;
        let mut state =
            dimension_state(dimension, Arc::clone(&registry), Some(directory.0.clone()));
        assert_eq!(
            state.world.chunk(position).unwrap().geometry(),
            dimension.geometry()
        );
        state.world.set_block(8, 150, 8, id);
        state.world.fold_overlay();
        let capture = state.world.capture_dirty(&Default::default());
        assert_eq!(capture.write().unwrap().len(), 1);
        drop(state);
        let reopened = dimension_state(dimension, Arc::clone(&registry), Some(directory.0.clone()));
        assert_eq!(reopened.world.block_at(8, 150, 8), Some(id));
        assert_eq!(
            reopened.world.chunk(position).unwrap().geometry(),
            dimension.geometry()
        );
        assert!(directory.0.join("native/state.redb").is_file());
    }
}

#[test]
fn dimension_contract_rejects_changed_geometry_and_missing_history_registry() {
    for missing in [false, true] {
        let directory = TempWorld::new();
        let config = crate::CoreConfig::new(directory.0.clone(), 42);
        let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
        let mut image = store.read_checkpoint().unwrap();
        let mut dimensions: serde_json::Value =
            serde_json::from_slice(&image.globals["dimensions"].payload).unwrap();
        if missing {
            dimensions.as_object_mut().unwrap().remove("1");
        } else {
            dimensions["1"]["min_y"] = serde_json::json!(-64);
        }
        image.globals.get_mut("dimensions").unwrap().payload =
            serde_json::to_vec(&dimensions).unwrap();
        for global in image.globals.values_mut() {
            global.captured_tick = 1;
        }
        let mut manifest: crate::journal::CheckpointManifest =
            serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
        manifest.tick = 1;
        for child in manifest.dimensions.values_mut() {
            child.tick = 1;
        }
        image.globals.get_mut("checkpoint").unwrap().payload =
            serde_json::to_vec(&manifest).unwrap();
        let registry = crate::registry::CoreRegistry::load(None).unwrap();
        let committed = store.commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick: 1,
                previous_tick: 0,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &image.globals,
            },
            &registry.blocks,
            &registry.items,
        );
        let error = match committed {
            Ok(()) => {
                drop(store);
                crate::Core::open(config).unwrap_err().to_string()
            }
            Err(error) => error.to_string(),
        };
        assert!(
            error.contains("generation") || error.contains("dimension"),
            "{error}",
        );
    }
}

#[test]
fn void_collision_and_damage_use_each_world_minimum() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    for dimension in Dimension::ALL {
        let mut state = dimension_state(dimension, Arc::clone(&registry), None);
        let minimum = dimension.geometry().min_y();
        assert_eq!(state.world.block_at(8, minimum - 1, 8), None);
        assert_eq!(
            state.world.physics_block_at(8, minimum - 1, 8),
            Some(state.registry.states.air)
        );
        assert_eq!(state.world.physics_block_at(1000, minimum - 1, 1000), None);
        let entity = state.spawn_mob(
            state.registry.cow_type_id,
            [8.0, f64::from(minimum - 65), 8.0],
            10.0,
        );
        state
            .entities
            .row_mut(state.entities.slot_of(entity).unwrap())
            .protected_until = 0;
        state.tick = 10;
        let mut intents = Vec::new();
        rules::void_intents(&mut state, &mut intents);
        assert_eq!(intents.len(), 1);
        state.merge(&mut intents);
        state.commit_merge_effects();
        let row = state.entities.row(state.entities.slot_of(entity).unwrap());
        assert_eq!(row.health, 6.0);
    }
}

#[test]
fn nether_lava_retains_short_delay_and_longer_horizontal_flow() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    for (dimension, delay, expected_level) in [
        (Dimension::Overworld, 30, 2),
        (Dimension::Nether, 10, 1),
        (Dimension::End, 30, 2),
    ] {
        let mut state = dimension_state(dimension, Arc::clone(&registry), None);
        let position = crate::boundary::BlockPos { x: 8, y: 150, z: 8 };
        let source = state.registry.states.lava_levels[0];
        state.world.set_block(8, 150, 8, source);
        crate::fluids::schedule(&mut state, &[(position, source)]);
        assert_eq!(state.fluid_ticks.image().due[0].deadline, delay);
        state.tick = delay;
        // Strict empty default: residency is not activity, and fluid ticks
        // only drain in explicitly activated chunks.
        state.world.set_active_chunks(
            [(mc_world::ChunkPos { x: 0, z: 0 })].into_iter().collect(),
            state.tick,
        );
        let mut writes = Vec::new();
        crate::fluids::run(&mut state, &mut writes, &mut Vec::new());
        assert_eq!(
            crate::fluids::fluid_at(
                &state.registry.states,
                state.world.block_at(9, 150, 8).unwrap()
            ),
            Some((crate::fluids::Fluid::Lava, expected_level))
        );
    }
}

#[test]
fn cross_dimension_wal_then_checkpoint_restart_preserves_player_and_world_custody() {
    let directory = TempWorld::new();
    let (mut state, _, bob) = worlds();
    command(&mut state, 2, "gamemode creative");
    command(&mut state, 2, "give @s minecraft:diamond 5");
    command(&mut state, 2, "spawnpoint @s 8 150 8");
    let (output, _) = command(&mut state, 2, "setblock 8 150 8 minecraft:glowstone");
    let items = Arc::clone(&state.registry.items);
    let diamond = items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond").unwrap())
        .unwrap();
    let glowstone = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:glowstone").unwrap())
        .unwrap()
        .default
        .0;
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let config = import_config(&directory.0);
    let initial = crate::journal::CheckpointManifest {
        world_spawn: Some([8, 150, 8]),
        dimensions: [Dimension::Nether, Dimension::End]
            .into_iter()
            .map(|dimension| {
                (
                    dimension,
                    Box::new(crate::journal::CheckpointManifest {
                        world_spawn: Some([8, 150, 8]),
                        ..Default::default()
                    }),
                )
            })
            .collect(),
        ..Default::default()
    };
    crate::native_checkpoint::tests::write_manifest(&config, &initial).unwrap();
    let mut writer = crate::journal::JournalWriter::open(&directory.0, 1).unwrap();
    // Manifest and player fixture publication captures tick 2. Split the
    // empty earlier interval from the tail that runtime recovery must replay.
    writer.append(1, 2, &[]).unwrap();
    writer.append(3, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    let batches =
        crate::journal::read_complete_batches_after(&directory.0.join("journal"), 0).unwrap();
    let replay = crate::journal::replay_batches(
        Default::default(),
        &journal_for_dimension(&batches, Dimension::Nether),
    );
    let uuid = uuid::Uuid::from_u128(2);
    let record = &replay.players[&uuid.to_string()];
    assert_eq!(record.dimension, Dimension::Nether);
    assert_eq!(record.respawn_dimension, Dimension::Nether);
    assert_eq!(record.respawn, Some([8, 150, 8]));
    assert_eq!(replay.blocks[&(8, 150, 8)], glowstone);
    assert_eq!(travel::locate(&state, bob).unwrap().0, Dimension::Nether);
    // The WAL changes custody away from an older, required Overworld
    // checkpoint record. Recovery must not hydrate that UUID in both worlds.
    let mut base = record.clone();
    base.dimension = Dimension::Overworld;
    base.respawn_dimension = Dimension::Overworld;
    base.tick = 0;
    crate::native_checkpoint::tests::write_player(&config, &uuid.to_string(), &base, &items)
        .unwrap();
    drop(writer);
    drop(state);
    for restart in 0..2 {
        let core = crate::Core::open(config.clone()).unwrap();
        let mut receiver = core.subscribe();
        assert!(core.submit(PlayerInput::Join {
            permission_level: 0,
            connection: ConnectionId(9),
            uuid,
            name: "Bob".into()
        }));
        let mut joined = false;
        let mut inventory = false;
        let mut resident = false;
        for _ in 0..100 {
            let output = receiver.blocking_recv().unwrap();
            let child = output.in_dimension(Dimension::Nether).unwrap();
            resident |= child
                .snapshot
                .chunks
                .iter()
                .any(|chunk| chunk.pos == mc_world::ChunkPos { x: 0, z: 0 });
            for (connection, queue) in core.try_drain_outbox() {
                if connection != ConnectionId(9) {
                    continue;
                }
                for change in queue {
                    match change {
                        PlayerChange::Joined { dimension, .. } => {
                            assert_eq!(dimension, Dimension::Nether);
                            joined = true;
                        }
                        PlayerChange::MenuContents {
                            window: 0, slots, ..
                        } => {
                            inventory |= slots
                                .iter()
                                .flatten()
                                .filter(|stack| stack.item_id == diamond)
                                .map(|stack| stack.count)
                                .sum::<i32>()
                                == 5;
                        }
                        _ => {}
                    }
                }
            }
            if joined && inventory && resident {
                break;
            }
        }
        assert!(
            joined && inventory && resident,
            "restart {restart}: join={joined}, inventory={inventory}, resident={resident}"
        );
        assert!(core.checkpoint(), "all-world checkpoint");
        core.shutdown();
        let manifest = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
        assert_eq!(manifest.dimensions.len(), 2);
        let record =
            crate::native_checkpoint::tests::read_player(&config, &uuid.to_string(), &items)
                .unwrap();
        assert_eq!(record.dimension, Dimension::Nether);
        assert_eq!(record.respawn_dimension, Dimension::Nether);
        assert_eq!(record.respawn, Some([8, 150, 8]));
        assert_eq!(
            record
                .inventory
                .iter()
                .filter(|stack| stack.item_id == diamond)
                .map(|stack| stack.count)
                .sum::<i32>(),
            5
        );
    }
    assert!(directory.0.join("native/state.redb").is_file());
}

fn native_import_fixture(root: &std::path::Path) -> mc_data::items::ItemRegistry {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let config = import_config(root);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let mut image = store.read_checkpoint().unwrap();
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.world_spawn = Some([8, 150, 8]);
    image.globals.insert(
        "checkpoint".into(),
        crate::native_checkpoint::record(0, serde_json::to_vec(&manifest).unwrap()),
    );
    let mut entities = Vec::new();
    for (dimension, records) in [
        (Dimension::Overworld, vec![(1, 101), (2, 102)]),
        (Dimension::Nether, vec![(3, 103)]),
        (Dimension::End, vec![(4, 100)]),
    ] {
        let chunks = [(
            (0, 0),
            mc_world::anvil::EntityChunk {
                x: 0,
                z: 0,
                entities: records
                    .into_iter()
                    .map(|(uuid, wire_id)| mc_data::entity_nbt::VanillaEntity {
                        id: "minecraft:cow".into(),
                        uuid: uuid::Uuid::from_u128(uuid),
                        pos: [8.0, 150.0, 8.0],
                        health: 10.0,
                        wire_id,
                        ..Default::default()
                    })
                    .collect(),
            },
        )]
        .into_iter()
        .collect();
        entities.extend(
            crate::native_checkpoint::encode_entities(dimension, &chunks, &registry.items, 0)
                .unwrap(),
        );
    }
    store
        .commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick: 1,
                previous_tick: image.tick,
                chunks: &[],
                entities: &entities,
                players: &[],
                globals: &{
                    manifest.tick = 1;
                    for child in manifest.dimensions.values_mut() {
                        child.tick = 1;
                    }
                    for value in image.globals.values_mut() {
                        value.captured_tick = 1;
                    }
                    image.globals.insert(
                        "checkpoint".into(),
                        crate::native_checkpoint::record(1, serde_json::to_vec(&manifest).unwrap()),
                    );
                    image.globals
                },
            },
            &registry.blocks,
            &registry.items,
        )
        .unwrap();
    (*registry.items).clone()
}

fn native_ids(
    root: &std::path::Path,
    _items: &mc_data::items::ItemRegistry,
) -> std::collections::BTreeMap<uuid::Uuid, i32> {
    crate::native_checkpoint::recover_offline(&import_config(root))
        .unwrap()
        .dimensions
        .values()
        .flat_map(|replay| &replay.entities)
        .map(|(&id, entity)| (entity.uuid.parse().unwrap(), id))
        .collect()
}

fn import_config(root: &std::path::Path) -> crate::config::CoreConfig {
    let mut config = crate::config::CoreConfig::new(root.to_owned(), 42);
    config.view_distance = 1;
    config.simulation_distance = 0;
    config.checkpoint_every_ticks = u64::MAX;
    config.journal_fsync_every_batches = 1;
    config
}

#[test]
fn native_checkpoint_warden_receives_nearby_step_after_core_open() {
    let directory = TempWorld::new();
    let items = native_import_fixture(&directory.0);
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut floor = dimension_state(Dimension::Overworld, registry, Some(directory.0.clone()));
    floor
        .world
        .capture_dirty(&Default::default())
        .write()
        .unwrap();
    drop(floor);
    let config = import_config(&directory.0);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let mut image = store.read_checkpoint().unwrap();
    let warden_uuid = uuid::Uuid::from_u128(1);
    let warden = image
        .entities
        .iter_mut()
        .find(|entity| entity.identity == *warden_uuid.as_bytes())
        .unwrap();
    let Some(mc_nbt::Tag::Compound(fields)) = &mut warden.payload else {
        panic!()
    };
    for (name, value) in fields.iter_mut() {
        match name.as_str() {
            "id" => *value = mc_nbt::Tag::String("minecraft:warden".into()),
            "Pos" => {
                *value = mc_nbt::Tag::List(mc_nbt::ListTag {
                    element_type: mc_nbt::tag_type::DOUBLE,
                    elements: [12.5, 150.0, 8.5]
                        .into_iter()
                        .map(mc_nbt::Tag::Double)
                        .collect(),
                })
            }
            "Health" => *value = mc_nbt::Tag::Float(500.0),
            _ => {}
        }
    }
    fields.push(("NoAI".into(), mc_nbt::Tag::Byte(1)));
    let tick = image.tick + 1;
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.tick = tick;
    for child in manifest.dimensions.values_mut() {
        child.tick = tick;
    }
    for value in image.globals.values_mut() {
        value.captured_tick = tick;
    }
    image.globals.insert(
        "checkpoint".into(),
        crate::native_checkpoint::record(tick, serde_json::to_vec(&manifest).unwrap()),
    );
    store
        .commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &image.entities,
                players: &[],
                globals: &image.globals,
            },
            &crate::registry::CoreRegistry::load(None).unwrap().blocks,
            &items,
        )
        .unwrap();
    drop(store);

    let core = crate::Core::open(config).unwrap();
    let final_id = EntityId(101);
    let mut receiver = core.subscribe();
    let connection = ConnectionId(500);
    let player_uuid = uuid::Uuid::from_u128(500);
    core.submit(PlayerInput::Join {
        connection,
        uuid: player_uuid,
        name: "warden-import-probe".into(),
        permission_level: 0,
    });
    let mut player_pos = None;
    for _ in 0..8 {
        let output = receiver.blocking_recv().unwrap();
        let _ = core.try_drain_outbox();
        if let Some(player) = output
            .snapshot
            .entities
            .iter()
            .find(|entity| entity.uuid == player_uuid)
        {
            player_pos = Some(player.position);
            break;
        }
    }
    let mut position = player_pos.expect("joined player is published");
    position[0] += 0.3;
    position[1] -= 0.01;
    core.submit(PlayerInput::ConfirmTeleport {
        connection,
        teleport_id: 1,
    });
    core.submit(PlayerInput::Move {
        connection,
        position,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
    });
    let mut heard = false;
    for _ in 0..8 {
        let output = receiver.blocking_recv().unwrap();
        let _ = core.try_drain_outbox();
        heard |= output.changes.entity_events.iter().any(|event| {
            matches!(
                event,
                crate::boundary::EntityEvent::Status { entity, event: 61 } if *entity == final_id
            )
        });
        if heard {
            break;
        }
    }
    core.shutdown();
    assert!(
        heard,
        "imported warden with final wire ID did not hear nearby Step"
    );
}

#[test]
fn native_checkpoint_keeps_ids_stable_for_wal_only_removal_and_health_replay() {
    let directory = TempWorld::new();
    let items = native_import_fixture(&directory.0);
    let original = native_ids(&directory.0, &items);
    assert!(original.values().all(|id| *id > 0));
    let config = import_config(&directory.0);
    let core = crate::Core::open(config.clone()).unwrap();
    core.shutdown();
    let assigned = native_ids(&directory.0, &items);
    assert_eq!(assigned.len(), 4);
    assert!(assigned.values().all(|id| *id > 0));
    assert_eq!(
        assigned
            .values()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(assigned[&uuid::Uuid::from_u128(4)], 100);
    let base = crate::native_checkpoint::tests::read_manifest(&config)
        .unwrap()
        .tick;
    let removed = assigned[&uuid::Uuid::from_u128(1)];
    let modified = assigned[&uuid::Uuid::from_u128(2)];
    let mut writer = crate::journal::JournalWriter::open(&directory.0, 1).unwrap();
    writer
        .append(
            base + 1,
            base + 1,
            &[
                crate::journal::JournalEntry::RemoveEntity {
                    id: removed,
                    tick: base + 1,
                },
                crate::journal::JournalEntry::EntityVitals {
                    id: modified,
                    health: 5.0,
                    alive: true,
                    tick: base + 1,
                },
            ],
        )
        .unwrap();
    writer.flush().unwrap();
    drop(writer); // Crash image: durable WAL tail, no checkpoint of these changes.
    assert_eq!(
        crate::native_checkpoint::tests::read_manifest(&config)
            .unwrap()
            .tick,
        base
    );
    let core = crate::Core::open(config).unwrap();
    let mut receiver = core.subscribe();
    let output = receiver.blocking_recv().unwrap();
    let entities: Vec<_> = std::iter::once(&output)
        .chain(output.snapshot.dimensions.values())
        .flat_map(|world| &world.snapshot.entities)
        .collect();
    assert_eq!(entities.len(), 3);
    assert!(
        entities
            .iter()
            .all(|entity| entity.uuid != uuid::Uuid::from_u128(1))
    );
    assert_eq!(
        entities
            .iter()
            .map(|entity| entity.uuid)
            .collect::<std::collections::BTreeSet<_>>()
            .len(),
        3
    );
    for entity in entities {
        assert_eq!(entity.id.0, assigned[&entity.uuid]);
        if entity.uuid == uuid::Uuid::from_u128(2) {
            assert_eq!(entity.appearance.health, 5.0);
        }
    }
    core.shutdown();
}

fn join_one(state: &mut TickState) -> EntityId {
    crate::tick::run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::from_u128(1),
            name: "Alice".into(),
            permission_level: 0,
        }],
    );
    state.connections.entity_of(ConnectionId(1)).unwrap()
}

fn block(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
        .default
        .0
}

fn held(state: &mut TickState, actor: EntityId, name: &str, count: i32) -> usize {
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    let player = state.players.get_mut(&actor).unwrap();
    let slot = crate::tick::held_slot_index(player, crate::boundary::Hand::Main);
    player.inventory.slots[slot] = mc_data::ItemStack::new(item, count);
    slot
}

fn load_spawn_area(state: &mut TickState) {
    let want = crate::world::chunks_in_radius(mc_world::ChunkPos { x: 0, z: 0 }, 1)
        .into_iter()
        .map(|position| (position, 0))
        .collect();
    state.world.ensure_blocking(&want);
}

#[test]
fn duplicate_join_same_batch_and_across_worlds_never_duplicates_inventory() {
    let (mut state, alice, bob) = worlds();
    command(&mut state, 1, "give @a minecraft:diamond 3");
    let before = state.players[&alice].inventory.slots.clone();
    let nether_before = state.other_dimensions[&Dimension::Nether].players[&bob]
        .inventory
        .slots
        .clone();
    let (_, drains, _) = crate::tick::run_tick(
        &mut state,
        &[
            PlayerInput::Join {
                connection: ConnectionId(3),
                uuid: uuid::Uuid::from_u128(1),
                name: "DuplicateAlice".into(),
                permission_level: 0,
            },
            PlayerInput::Join {
                connection: ConnectionId(4),
                uuid: uuid::Uuid::from_u128(2),
                name: "DuplicateBob".into(),
                permission_level: 0,
            },
            PlayerInput::Join {
                connection: ConnectionId(5),
                uuid: uuid::Uuid::from_u128(5),
                name: "Fresh".into(),
                permission_level: 0,
            },
            PlayerInput::Join {
                connection: ConnectionId(6),
                uuid: uuid::Uuid::from_u128(5),
                name: "DuplicateFresh".into(),
                permission_level: 0,
            },
        ],
    );
    for connection in [3, 4, 6].map(ConnectionId) {
        assert!(drains.iter().filter(|(id, _)| *id == connection).flat_map(|(_, queue)| queue)
            .any(|change| matches!(change, PlayerChange::Disconnect { reason, .. } if reason == "UUID is already joined")));
        assert!(
            !std::iter::once(&state)
                .chain(state.other_dimensions.values().map(|world| &**world))
                .any(|world| world.connections.entity_of(connection).is_some())
        );
    }
    assert_eq!(state.players[&alice].inventory.slots, before);
    assert_eq!(
        state.other_dimensions[&Dimension::Nether].players[&bob]
            .inventory
            .slots,
        nether_before
    );
    assert_eq!(state.dimension_globals.roster.len(), 3);
    let uuids: Vec<_> = std::iter::once(&state)
        .chain(state.other_dimensions.values().map(|world| &**world))
        .flat_map(|world| {
            world
                .connections
                .all_connections()
                .map(|connection| world.connections.uuid_of(connection).unwrap())
        })
        .collect();
    assert_eq!(
        uuids.len(),
        uuids
            .iter()
            .collect::<std::collections::BTreeSet<_>>()
            .len()
    );
}

#[test]
fn wrong_dimension_beds_and_anchors_explode_without_source_loot() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    for (dimension, name) in [
        (Dimension::Nether, "red_bed"),
        (Dimension::End, "red_bed"),
        (Dimension::Overworld, "respawn_anchor"),
        (Dimension::End, "respawn_anchor"),
    ] {
        let mut state = dimension_state(dimension, Arc::clone(&registry), None);
        let actor = join_one(&mut state);
        let slot = state.entities.slot_of(actor).unwrap();
        state.entities.set_pos(slot, [8.5, 150.0, 8.5]);
        state.entities.row_mut(slot).protected_until = 0;
        let position = crate::boundary::BlockPos { x: 8, y: 150, z: 8 };
        let mut source = block(&state, name);
        if name == "respawn_anchor" {
            source = rules::anchor_state(&state, source, 1).unwrap();
        }
        state.world.set_block(8, 150, 8, source);
        let other = if name == "red_bed" {
            let (other, _) =
                crate::placement::other_half(&state.registry.blocks, position, source).unwrap();
            state.world.set_block(other.x, other.y, other.z, source);
            Some(other)
        } else {
            None
        };
        let mut intents = Vec::new();
        assert!(rules::use_block(
            &mut state,
            actor,
            position,
            crate::boundary::Hand::Main,
            &mut intents
        ));
        state.merge(&mut intents);
        state.commit_merge_effects();
        assert_eq!(
            state.world.block_at(8, 150, 8),
            Some(state.registry.states.air)
        );
        if let Some(other) = other {
            assert_eq!(
                state.world.block_at(other.x, other.y, other.z),
                Some(state.registry.states.air)
            );
        }
        assert!(state.entities.row(slot).health < 20.0);
        let source_item = registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap();
        assert!(!state.entities.slots().any(|slot| {
            state
                .entities
                .row(slot)
                .item
                .as_ref()
                .is_some_and(|item| item.item_id == source_item)
        }));
    }
}

#[test]
fn simultaneous_anchor_charges_claim_one_glowstone_once() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = dimension_state(Dimension::Nether, registry, None);
    let first = join_one(&mut state);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: uuid::Uuid::from_u128(2),
            name: "Bob".into(),
            permission_level: 0,
        }],
    );
    let second = state.connections.entity_of(ConnectionId(2)).unwrap();
    let first_slot = held(&mut state, first, "glowstone", 2);
    let second_slot = held(&mut state, second, "glowstone", 2);
    let anchor = block(&state, "respawn_anchor");
    let position = crate::boundary::BlockPos { x: 8, y: 150, z: 8 };
    state.world.set_block(8, 150, 8, anchor);
    let input = |connection| PlayerInput::UseItemOn {
        connection: ConnectionId(connection),
        hand: crate::boundary::Hand::Main,
        position,
        face: 1,
        cursor: [0.5; 3],
        sequence: 1,
    };
    for (actor, position) in [(first, [7.5, 150.0, 8.5]), (second, [9.5, 150.0, 8.5])] {
        state
            .entities
            .set_pos(state.entities.slot_of(actor).unwrap(), position);
    }
    crate::tick::run_tick(&mut state, &[input(1), input(2)]);
    assert_eq!(
        rules::anchor_charge(&state, state.world.block_at(8, 150, 8).unwrap()),
        Some(1)
    );
    assert_eq!(
        state.players[&first].inventory.slots[first_slot].count
            + state.players[&second].inventory.slots[second_slot].count,
        3
    );
    let full = rules::anchor_state(&state, anchor, 4).unwrap();
    state.world.set_block(8, 150, 8, full);
    let counts = [
        state.players[&first].inventory.slots[first_slot].count,
        state.players[&second].inventory.slots[second_slot].count,
    ];
    crate::tick::run_tick(&mut state, &[input(1)]);
    assert_eq!(
        counts[0],
        state.players[&first].inventory.slots[first_slot].count
    );
    assert_eq!(
        rules::anchor_charge(&state, state.world.block_at(8, 150, 8).unwrap()),
        Some(4)
    );
    assert_eq!(state.players[&first].respawn_point, Some([8, 150, 8]));
    assert_eq!(state.players[&first].respawn_dimension, Dimension::Nether);
}

#[test]
fn anchor_respawn_waits_for_loaded_area_and_consumes_one_charge() {
    let (mut state, _, bob) = worlds();
    let nether = state.other_dimensions.get_mut(&Dimension::Nether).unwrap();
    let anchor = rules::anchor_state(nether, block(nether, "respawn_anchor"), 2).unwrap();
    nether.world.set_block(8, 150, 8, anchor);
    let player = nether.players.get_mut(&bob).unwrap();
    player.respawn_point = Some([8, 150, 8]);
    player.respawn_dimension = Dimension::Nether;
    player.respawn_forced = false;
    command(&mut state, 2, "kill");
    let nether = state.other_dimensions.get_mut(&Dimension::Nether).unwrap();
    let mut changes = crate::boundary::ChangeSet::default();
    assert!(!crate::tick::respawn_player(
        nether,
        ConnectionId(2),
        &mut changes
    ));
    assert_eq!(
        rules::anchor_charge(nether, nether.world.block_at(8, 150, 8).unwrap()),
        Some(2)
    );
    load_spawn_area(nether);
    assert!(crate::tick::respawn_player(
        nether,
        ConnectionId(2),
        &mut changes
    ));
    assert_eq!(
        rules::anchor_charge(nether, nether.world.block_at(8, 150, 8).unwrap()),
        Some(1)
    );
    assert!(crate::tick::respawn_player(
        nether,
        ConnectionId(2),
        &mut changes
    ));
    assert_eq!(
        rules::anchor_charge(nether, nether.world.block_at(8, 150, 8).unwrap()),
        Some(1)
    );
    let slot = nether.entities.slot_of(bob).unwrap();
    assert_eq!(nether.entities.pos(slot), [9.5, 150.0, 8.5]);
    assert!(
        changes
            .blocks
            .iter()
            .any(|change| change.position == crate::boundary::BlockPos { x: 8, y: 150, z: 8 })
    );
    // A depleted/destroyed personal anchor returns through the same native
    // publication boundary to the Overworld, never an unsafe Nether fallback.
    nether
        .world
        .set_block(8, 150, 8, nether.registry.states.air);
    load_spawn_area(&mut state);
    command(&mut state, 2, "kill");
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Respawn {
            connection: ConnectionId(2),
        }],
    );
    assert_eq!(travel::locate(&state, bob).unwrap().0, Dimension::Overworld);
    assert_eq!(state.players[&bob].respawn_point, None);
}

#[test]
fn nether_bucket_water_evaporates_but_cauldron_retains_it() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = dimension_state(Dimension::Nether, registry, None);
    let actor = join_one(&mut state);
    let actor_slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(actor_slot, [8.5, 150.0, 8.5]);
    for ray in [false, true] {
        let inventory_slot = held(&mut state, actor, "water_bucket", 1);
        let input = if ray {
            PlayerInput::UseItem {
                connection: ConnectionId(1),
                hand: crate::boundary::Hand::Main,
                yaw: 0.0,
                pitch: 90.0,
                sequence: 2,
            }
        } else {
            PlayerInput::UseItemOn {
                connection: ConnectionId(1),
                hand: crate::boundary::Hand::Main,
                position: crate::boundary::BlockPos { x: 8, y: 149, z: 8 },
                face: 1,
                cursor: [0.5; 3],
                sequence: 1,
            }
        };
        crate::tick::run_tick(&mut state, &[input]);
        assert_eq!(
            state.world.block_at(8, 150, 8),
            Some(state.registry.states.air)
        );
        let stack = &state.players[&actor].inventory.slots[inventory_slot];
        assert_eq!(
            state.registry.items.name_of(stack.item_id).unwrap().path(),
            "bucket"
        );
        assert_eq!(stack.count, 1);
    }
    let inventory_slot = held(&mut state, actor, "water_bucket", 1);
    let cauldron = block(&state, "cauldron");
    state.world.set_block(9, 150, 8, cauldron);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: crate::boundary::Hand::Main,
            position: crate::boundary::BlockPos { x: 9, y: 150, z: 8 },
            face: 1,
            cursor: [0.5; 3],
            sequence: 3,
        }],
    );
    let current = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(
            state.world.block_at(9, 150, 8).unwrap(),
        ))
        .unwrap();
    assert_eq!(current.block.id.path(), "water_cauldron");
    assert!(
        current
            .properties
            .iter()
            .any(|(key, value)| key == "level" && value == "3")
    );
    assert_eq!(
        state
            .registry
            .items
            .name_of(state.players[&actor].inventory.slots[inventory_slot].item_id)
            .unwrap()
            .path(),
        "bucket"
    );
}

#[test]
fn native_dimension_spawn_tables_and_census_are_local() {
    let (mut state, _, _) = worlds();
    for dimension in [Dimension::Nether, Dimension::End] {
        let world = state.other_dimensions.get_mut(&dimension).unwrap();
        let biome = world.world.biome_at(8, 64, 8).unwrap();
        match dimension {
            Dimension::Nether => assert!(matches!(
                biome.as_str(),
                "minecraft:nether_wastes"
                    | "minecraft:soul_sand_valley"
                    | "minecraft:crimson_forest"
                    | "minecraft:warped_forest"
                    | "minecraft:basalt_deltas"
            )),
            Dimension::End => assert_eq!(biome.as_str(), "minecraft:the_end"),
            Dimension::Overworld => unreachable!(),
        }
        let entries = world.registry.biome_spawns.exact_entries(biome, "monster");
        let selected = crate::natural_spawn::choose_spawn(&world.registry, entries, 0).unwrap();
        let id = world
            .registry
            .entity_types
            .id_of(&selected.entity_type)
            .unwrap();
        let blueprint = world.registry.blueprint(id).unwrap();
        assert!(entries.iter().all(|entry| {
            world
                .registry
                .entity_types
                .id_of(&entry.entity_type)
                .is_some()
        }));
        world.spawn_mob(id, [8.5, 64.0, 8.5], blueprint.params.max_health);
        assert_eq!(
            crate::natural_spawn::census(world, &[[8.5, 64.0, 8.5]])[0],
            1
        );
    }
    assert_eq!(
        crate::natural_spawn::census(&mut state, &[[8.5, 150.0, 8.5]])[0],
        0
    );
}

struct SpawnBiome {
    biome: mc_data::Identifier,
    dimension: Dimension,
    air: mc_world::BlockStateId,
    floor: mc_world::BlockStateId,
}

impl mc_world::ChunkGenerator for SpawnBiome {
    fn generate(&self, position: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty_with_geometry(
            position,
            self.air,
            self.biome.clone(),
            self.dimension.geometry(),
        );
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 100, z, self.floor);
            }
        }
        chunk
    }
}

fn spawn_biome_state(
    registry: Arc<crate::registry::CoreRegistry>,
    biome: &str,
    dimension: Dimension,
) -> TickState {
    let generator = Arc::new(SpawnBiome {
        biome: mc_data::Identifier::parse(format!("minecraft:{biome}")).unwrap(),
        dimension,
        air: mc_world::BlockStateId(registry.states.air),
        floor: mc_world::BlockStateId(registry.states.stone),
    });
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 101, 0],
        dimension.geometry(),
    );
    let mut state = TickState::new(registry, world, 0, 4096);
    state.dimension = dimension;
    let want = crate::world::chunks_in_radius(mc_world::ChunkPos { x: 0, z: 0 }, 4)
        .into_iter()
        .map(|position| (position, 0))
        .collect();
    state.world.ensure_blocking(&want);
    state
}

#[test]
fn every_embedded_nether_and_end_biome_spawns_its_characteristic_mob() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    for (biome, characteristic, dimension) in [
        (
            "nether_wastes",
            "minecraft:zombified_piglin",
            Dimension::Nether,
        ),
        ("soul_sand_valley", "minecraft:skeleton", Dimension::Nether),
        ("crimson_forest", "minecraft:hoglin", Dimension::Nether),
        ("warped_forest", "minecraft:enderman", Dimension::Nether),
        ("basalt_deltas", "minecraft:magma_cube", Dimension::Nether),
        ("the_end", "minecraft:enderman", Dimension::End),
        ("end_highlands", "minecraft:enderman", Dimension::End),
        ("end_midlands", "minecraft:enderman", Dimension::End),
        ("small_end_islands", "minecraft:enderman", Dimension::End),
        ("end_barrens", "minecraft:enderman", Dimension::End),
    ] {
        let mut state = spawn_biome_state(Arc::clone(&registry), biome, dimension);
        let observer = [0.5, 101.0, 0.5];
        crate::natural_spawn::census(&mut state, &[observer]);
        let mut found = false;
        for seed in 0..64 {
            let mut changes = crate::boundary::ChangeSet::default();
            crate::natural_spawn::spawn_group(
                &mut state,
                &[observer],
                observer,
                mc_data::entity_types::MobCategory::Monster,
                1,
                &mut crate::natural_spawn::Rng(seed),
                &mut changes,
            );
            found |= changes.entities_spawned.iter().any(|id| {
                let slot = state.entities.slot_of(*id).unwrap();
                let crate::entities::EntityKind::Mob { type_id } = state.entities.body(slot).kind
                else {
                    return false;
                };
                state.registry.blueprint(type_id).unwrap().name == characteristic
            });
            if found {
                break;
            }
        }
        assert!(
            found,
            "{biome} must physically admit {characteristic} through the native pack path"
        );
        // Local cap admission uses only this world's census, not any other
        // world's same-coordinate population.
        for _ in 0..70 {
            state.spawn_mob(registry.zombie_type_id, [0.5, 101.0, 0.5], 20.0);
        }
        crate::natural_spawn::census(&mut state, &[observer]);
        assert_eq!(
            crate::natural_spawn::spawn_group(
                &mut state,
                &[observer],
                observer,
                mc_data::entity_types::MobCategory::Monster,
                70,
                &mut crate::natural_spawn::Rng(0),
                &mut crate::boundary::ChangeSet::default()
            ),
            0
        );
    }
}

#[test]
fn warped_forest_spawn_energy_is_local_and_rebuilt_without_stale_charges() {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let mut state = spawn_biome_state(registry, "warped_forest", Dimension::Nether);
    let name = "minecraft:enderman";
    let id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap();
    let entity = state.spawn_mob(id, [30.5, 101.0, 0.5], 40.0);
    crate::natural_spawn::census(&mut state, &[]);
    assert!(!spawning::admitted(&state, name, [32.5, 101.0, 0.5]));
    assert!(spawning::admitted(&state, name, [62.5, 101.0, 0.5]));
    state
        .entities
        .remove(state.entities.slot_of(entity).unwrap());
    crate::natural_spawn::census(&mut state, &[]);
    assert!(spawning::admitted(&state, name, [30.5, 101.0, 0.5]));
}

#[test]
fn native_checkpoint_preserves_chunks_and_entities_in_all_dimensions() {
    let directory = TempWorld::new();
    let items = native_import_fixture(&directory.0);
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let marker = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:diamond_block").unwrap())
        .unwrap()
        .default;
    for dimension in Dimension::ALL {
        let mut state =
            dimension_state(dimension, Arc::clone(&registry), Some(directory.0.clone()));
        state.world.set_block(8, 150, 8, marker.0);
        state.world.fold_overlay();
        state
            .world
            .capture_dirty(&Default::default())
            .write()
            .unwrap();
    }
    let core = crate::Core::open(import_config(&directory.0)).unwrap();
    let mut receiver = core.subscribe();
    let output = receiver.blocking_recv().unwrap();
    assert_eq!(
        std::iter::once(&output)
            .chain(output.snapshot.dimensions.values())
            .map(|world| world.snapshot.entities.len())
            .sum::<usize>(),
        4
    );
    core.submit(PlayerInput::Join {
        connection: ConnectionId(262),
        uuid: uuid::Uuid::from_u128(262),
        name: "LayoutProbe".into(),
        permission_level: 0,
    });
    let mut output = output;
    for _ in 0..20 {
        output = receiver.blocking_recv().unwrap();
        core.try_drain_outbox();
        if output
            .snapshot
            .chunks
            .iter()
            .any(|chunk| chunk.pos == mc_world::ChunkPos { x: 0, z: 0 })
        {
            break;
        }
    }
    let chunk = output
        .snapshot
        .chunks
        .iter()
        .find(|chunk| chunk.pos == mc_world::ChunkPos { x: 0, z: 0 })
        .unwrap();
    assert_eq!(
        chunk.chunk.get_block(8, 150, 8),
        Some(marker),
        "imported chunk must not be regenerated"
    );
    assert!(core.checkpoint());
    core.shutdown();
    for dimension in Dimension::ALL {
        let state = dimension_state(dimension, Arc::clone(&registry), Some(directory.0.clone()));
        assert_eq!(state.world.block_at(8, 150, 8), Some(marker.0));
    }
    assert_eq!(native_ids(&directory.0, &items).len(), 4);
    for legacy in ["region", "entities", "DIM-1", "DIM1"] {
        assert!(!directory.0.join(legacy).exists());
    }
}

#[test]
fn legacy_only_dimension_layout_is_rejected_without_creating_modern_world() {
    for legacy in ["region", "DIM-1", "DIM1"] {
        let directory = TempWorld::new();
        std::fs::create_dir_all(directory.0.join(legacy)).unwrap();
        let error = crate::Core::open(import_config(&directory.0))
            .unwrap_err()
            .to_string();
        assert!(error.contains("solaris-v2 world import-anvil"), "{error}");
        assert!(!directory.0.join("dimensions").exists());
        assert!(!directory.0.join("CHECKPOINT-MANIFEST.json").exists());
    }
}

/// Driven by tools/harness/vanilla_world_roundtrip.py: real Java creation, offline
/// native import/mutation/export, Java save, then fresh native import/re-export.
#[test]
#[ignore = "requires the real vanilla 26.1.2 headless round-trip driver"]
fn vanilla_headless_world_roundtrip() {
    let root = std::path::PathBuf::from(
        std::env::var_os("SOLARIS_VANILLA_ROUNDTRIP_WORLD").expect("round-trip world path"),
    );
    let phase = std::env::var("SOLARIS_VANILLA_ROUNDTRIP_PHASE").expect("round-trip phase");
    assert!(matches!(phase.as_str(), "create" | "verify"));
    let verified = phase == "verify";
    let anvil = std::path::PathBuf::from(
        std::env::var_os("SOLARIS_VANILLA_ROUNDTRIP_ANVIL").expect("Anvil conversion path"),
    );
    let receipt = std::path::PathBuf::from(
        std::env::var_os("SOLARIS_VANILLA_ROUNDTRIP_RECEIPT").expect("conversion receipt path"),
    );
    let export = std::path::PathBuf::from(
        std::env::var_os("SOLARIS_VANILLA_ROUNDTRIP_EXPORT").expect("Anvil export destination"),
    );
    let export_receipt = std::path::PathBuf::from(
        std::env::var_os("SOLARIS_VANILLA_ROUNDTRIP_EXPORT_RECEIPT").expect("export receipt path"),
    );
    let config = import_config(&root);
    assert!(
        !root.exists(),
        "each phase requires a fresh native destination"
    );
    crate::world_conversion::import_anvil(&config, &anvil, &root, &receipt)
        .expect("offline Java Anvil import");
    let uuid = uuid::Uuid::parse_str("4d75256b-f3ff-3c0a-84fe-74c1a31dbd90").unwrap();
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let core = crate::Core::open(config.clone()).expect("open native round-trip world");
    let mut receiver = core.subscribe();
    let advance =
        |ticks: usize,
         receiver: &mut tokio::sync::broadcast::Receiver<crate::boundary::TickOutput>| {
            let mut last = None;
            for _ in 0..ticks {
                last = Some(
                    receiver
                        .blocking_recv()
                        .expect("live round-trip tick thread"),
                );
                for (_, changes) in core.try_drain_outbox() {
                    for change in changes {
                        if matches!(
                            change,
                            PlayerChange::Chat { .. } | PlayerChange::StyledChat { .. }
                        ) {
                            println!("{change:?}");
                        }
                    }
                }
            }
            last.unwrap()
        };
    if phase == "create" {
        core.submit(PlayerInput::Join {
            connection: ConnectionId(261),
            uuid,
            name: "LayoutPlayer".into(),
            permission_level: 0,
        });
        advance(20, &mut receiver);
        let run = |command: &str,
                   receiver: &mut tokio::sync::broadcast::Receiver<crate::boundary::TickOutput>| {
            core.submit(PlayerInput::Command {
                connection: ConnectionId(261),
                command: command.into(),
                permission_level: 2,
            });
            advance(5, receiver);
        };
        for command in [
            "gamemode creative LayoutPlayer",
            "gamerule advance_time false",
            "gamerule advance_weather false",
            "gamerule spawn_mobs false",
            "time set 18000",
            "setworldspawn 8 80 8",
            "scoreboard objectives add layout_gate dummy",
            "scoreboard players set LayoutPlayer layout_gate 37",
            "give LayoutPlayer minecraft:diamond 3",
        ] {
            run(command, &mut receiver);
        }
        for dimension in Dimension::ALL {
            run(
                &format!("execute in {} run tp LayoutPlayer 8 82 8", dimension.name()),
                &mut receiver,
            );
            advance(20, &mut receiver);
            run("setblock 4 80 4 minecraft:diamond_block", &mut receiver);
            run("setblock 8 79 8 minecraft:stone", &mut receiver);
        }
        run(
            "execute in overworld run tp LayoutPlayer 8 80 8",
            &mut receiver,
        );
        advance(20, &mut receiver);
        run("spawnpoint LayoutPlayer 8 80 8", &mut receiver);
        run(
            "summon minecraft:cow 8 80 8 {CustomName:\"LayoutMarker\",NoAI:1b,Invulnerable:1b}",
            &mut receiver,
        );
        core.submit(PlayerInput::Leave {
            connection: ConnectionId(261),
        });
        advance(5, &mut receiver);
        assert!(core.checkpoint(), "v2 create checkpoint");
    } else {
        assert_eq!(phase, "verify");
        let output = advance(5, &mut receiver);
        assert_eq!(output.snapshot.day_time, 19000, "vanilla-changed day clock");
        assert!(
            output.snapshot.entities.iter().any(|entity| {
                entity
                    .appearance
                    .custom_name
                    .as_deref()
                    .is_some_and(|name| name.contains("LayoutMarkerVanilla"))
                    && (entity.position[0] - 8.0).abs() < 0.01
                    && (entity.position[2] - 9.0).abs() < 0.01
            }),
            "vanilla-saved named entity must be visible in v2"
        );
    }
    core.shutdown();
    let marker = registry
        .blocks
        .block(
            &mc_data::Identifier::parse(if verified {
                "minecraft:emerald_block"
            } else {
                "minecraft:diamond_block"
            })
            .unwrap(),
        )
        .unwrap()
        .default;
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let recovered = crate::native_checkpoint::recover_with_reader(
        &config,
        Arc::clone(&registry),
        store.checkpoint_reader().unwrap(),
    )
    .unwrap();
    let mut imported_boundary_light = false;
    for dimension in Dimension::ALL {
        let world = crate::world::ChunkWorld::new_native(
            config.seed,
            Arc::clone(&store),
            crate::native_checkpoint::dimension_id(dimension),
            crate::native_checkpoint::generator(&config, &registry, dimension).unwrap(),
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 80, 8],
            dimension.geometry(),
        );
        let mut state = TickState::new(Arc::clone(&registry), world, 0, 4096);
        let want = [(mc_world::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect();
        state.world.ensure_blocking(&want);
        assert_eq!(
            state.world.block_at(4, 80, 4),
            Some(marker.0),
            "{dimension:?} marker"
        );
        if verified {
            let chunk = state
                .world
                .chunk(mc_world::ChunkPos { x: 0, z: 0 })
                .unwrap();
            assert_eq!(chunk.geometry(), dimension.geometry());
            // Java adds boundary light to adjacent generated chunks, not
            // necessarily the marker chunk that v2 originally checkpointed.
            let want = (0..=2)
                .flat_map(|x| (0..=2).map(move |z| (mc_world::ChunkPos { x, z }, 0)))
                .collect();
            state.world.ensure_blocking(&want);
            for x in 0..=2 {
                for z in 0..=2 {
                    let pos = mc_world::ChunkPos { x, z };
                    let chunk = state.world.chunk(pos).unwrap_or_else(|| {
                        panic!(
                            "{dimension:?} chunk {pos:?} failed admission: {:?}",
                            state.world.load_errors().collect::<Vec<_>>()
                        )
                    });
                    assert_eq!(chunk.geometry(), dimension.geometry());
                    imported_boundary_light |= chunk
                        .boundary_section_lights
                        .iter()
                        .any(|light| light.block.is_some() || light.sky.is_some());
                }
            }
        }
    }
    assert!(
        !verified || imported_boundary_light,
        "must exercise real Java boundary-light import"
    );
    let manifest = &recovered.manifest;
    assert_eq!(
        manifest.world_spawn,
        Some(if verified { [9, 81, 9] } else { [8, 80, 8] })
    );
    assert_eq!(
        manifest.climate.as_ref().unwrap().day_time,
        if verified { 19000 } else { 18000 }
    );
    assert_eq!(manifest.gamerules["advance_time"], "false");
    let board = recovered.dimensions[&Dimension::Overworld]
        .scoreboard
        .as_ref()
        .unwrap();
    assert!(board.objectives.contains_key("layout_gate"));
    assert_eq!(
        board.scores["LayoutPlayer"]["layout_gate"].value,
        if verified { 41 } else { 37 }
    );
    let player = &recovered.dimensions[&Dimension::Overworld].players[&uuid.to_string()];
    assert_eq!(player.respawn, Some([8, 80, 8]));
    assert_eq!(player.respawn_dimension, Dimension::Overworld);
    let diamond = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond").unwrap())
        .unwrap();
    assert_eq!(
        player
            .inventory
            .iter()
            .filter(|stack| stack.item_id == diamond)
            .map(|stack| stack.count)
            .sum::<i32>(),
        3,
        "offline player item custody survives Java save and v2 reopen"
    );
    drop(recovered);
    drop(store);
    crate::world_conversion::export_anvil(&config, &root, &export, &export_receipt)
        .expect("offline native Anvil export");
    println!("SOLARIS_VANILLA_ROUNDTRIP_{}_OK", phase.to_uppercase());
}

#[test]
fn summon_uuid_preflight_covers_other_worlds_offline_players_and_same_tick_claims() {
    let (mut state, alice, bob) = worlds();
    let alice_slot = state.entities.slot_of(alice).unwrap();
    let alice_uuid = state.entities.row(alice_slot).uuid;
    let cow = state.spawn_mob(state.registry.cow_type_id, [8.0, 150.0, 8.0], 10.0);
    let cow_uuid = state
        .entities
        .row(state.entities.slot_of(cow).unwrap())
        .uuid;
    for (connection, uuid) in [(2, alice_uuid), (2, cow_uuid), (1, cow_uuid)] {
        command(
            &mut state,
            connection,
            &format!("summon cow 8 150 8 {{UUID:\"{uuid}\"}}"),
        );
        assert!(
            state.other_dimensions[&Dimension::Nether]
                .entities
                .slot_of_uuid(uuid)
                .is_none()
        );
    }
    let bob_uuid = state.other_dimensions[&Dimension::Nether]
        .entities
        .row(
            state.other_dimensions[&Dimension::Nether]
                .entities
                .slot_of(bob)
                .unwrap(),
        )
        .uuid;
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: ConnectionId(2),
        }],
    );
    assert!(
        state.other_dimensions[&Dimension::Nether]
            .offline_players
            .contains_key(&bob_uuid)
    );
    command(
        &mut state,
        1,
        &format!("summon cow 8 150 8 {{UUID:\"{bob_uuid}\"}}"),
    );
    assert!(state.entities.slot_of_uuid(bob_uuid).is_none());
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: bob_uuid,
            name: "Bob".into(),
            permission_level: 0,
        }],
    );
    assert!(
        state.other_dimensions[&Dimension::Nether]
            .connections
            .entity_of(ConnectionId(2))
            .is_some()
    );
    let fresh = uuid::Uuid::from_u128(0x123456789);
    let authored = format!("summon cow 8 150 8 {{UUID:\"{fresh}\"}}");
    crate::tick::run_tick(
        &mut state,
        &[
            PlayerInput::Command {
                connection: ConnectionId(1),
                command: authored.clone(),
                permission_level: 2,
            },
            PlayerInput::Command {
                connection: ConnectionId(2),
                command: authored,
                permission_level: 2,
            },
        ],
    );
    assert!(state.entities.slot_of_uuid(fresh).is_some());
    assert!(
        state.other_dimensions[&Dimension::Nether]
            .entities
            .slot_of_uuid(fresh)
            .is_none()
    );
    validate_identity(&mut state).unwrap();
}

fn open_diamond_chest(state: &mut TickState, connection: u64) -> PlayerInput {
    let entity = state
        .connections
        .entity_of(ConnectionId(connection))
        .unwrap();
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [8.5, 150.0, 8.5]);
    let chest = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:chest").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(10, 150, 8, chest);
    state.world.fold_overlay();
    let mut slots = vec![mc_data::ItemStack::EMPTY; 27];
    slots[0] = mc_data::ItemStack::new(diamond_id(state), 5);
    state.block_entities.insert(
        (10, 150, 8),
        crate::tick::BlockEntityState {
            slots,
            furnace: None,
        },
    );
    crate::tick::run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(connection),
            hand: crate::boundary::Hand::Main,
            position: crate::boundary::BlockPos {
                x: 10,
                y: 150,
                z: 8,
            },
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    let menu = state.players[&entity].open.as_ref().expect("chest menu");
    PlayerInput::ContainerClick {
        connection: ConnectionId(connection),
        window: menu.window,
        state_id: menu.state_id,
        slot: 0,
        button: 0,
        mode: crate::boundary::ClickMode::QuickMove,
    }
}

fn diamond_id(state: &TickState) -> u32 {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond").unwrap())
        .unwrap()
}

/// Diamonds held, carried on the cursor, or lying on the ground.
fn diamonds(state: &TickState, player: EntityId) -> i32 {
    let diamond = diamond_id(state);
    let player = &state.players[&player];
    let held: i32 = player
        .inventory
        .slots
        .iter()
        .chain(player.open.as_ref().map(|menu| &menu.carried))
        .filter(|stack| stack.item_id == diamond)
        .map(|stack| stack.count)
        .sum();
    let ground: i32 = state
        .entities
        .slots()
        .filter(|slot| state.entities.body(*slot).alive)
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .filter(|item| item.item_id == diamond)
        .map(|item| item.count)
        .sum();
    held + ground
}

#[test]
fn shared_destroy_command_settles_open_chest_before_the_next_click() {
    let (mut state, alice, _) = worlds();
    let click = open_diamond_chest(&mut state, 1);
    command(&mut state, 1, "setblock 10 150 8 air destroy");
    assert!(
        !state.open_containers.contains_key(&alice),
        "the viewer closes with the command"
    );
    assert!(state.merge_command_broken.is_empty());
    assert!(state.merge_container_contents.is_empty());
    assert!(state.merge_closed_viewers.is_empty());
    assert!(!state.block_entities.contains_key(&(10, 150, 8)));
    crate::tick::run_tick(&mut state, &[click]);
    assert_eq!(diamonds(&state, alice), 5);
}
