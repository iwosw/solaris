use super::*;
use crate::boundary::PlayerChange;
use crate::commands::Command;
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use mc_nbt::Tag;
use std::sync::Arc;

fn state() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).expect("registry"));
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn run(state: &mut TickState, line: &str) -> Reply {
    let Command::ForceLoad(command) = crate::commands::parse(line, true).unwrap() else {
        panic!("{line} is not /forceload");
    };
    execute(state, crate::command_console::EXECUTOR, &command)
}

fn reply(key: &'static str, args: &[&str], failure: bool) -> Reply {
    Reply {
        key,
        args: args.iter().map(ToString::to_string).collect(),
        failure,
    }
}

const OVERWORLD: &str = "minecraft:overworld";

#[test]
fn parse_follows_the_vanilla_column_syntax() {
    let absolute = |value| Coord {
        relative: false,
        local: false,
        value,
    };
    assert_eq!(
        parse(&["add", "0", "16"]),
        Ok(ForceLoadCommand::Add {
            from: [absolute(0.0), absolute(16.0)],
            to: None
        })
    );
    assert!(matches!(
        parse(&["remove", "~", "~", "31", "31"]),
        Ok(ForceLoadCommand::Remove { to: Some(_), .. })
    ));
    assert_eq!(parse(&["remove", "all"]), Ok(ForceLoadCommand::RemoveAll));
    assert_eq!(parse(&["query"]), Ok(ForceLoadCommand::Query { pos: None }));
    assert!(matches!(
        parse(&["query", "5", "5"]),
        Ok(ForceLoadCommand::Query { pos: Some(_) })
    ));
    for invalid in [
        &["add"][..],
        &["add", "1"],
        &["add", "1", "2", "3"],
        &["add", "^1", "^1"],
        &["add", "1.5", "2"],
        &["add", "x", "1"],
        &["query", "1"],
        &["remove"],
        &["wipe"],
        &[],
    ] {
        assert!(parse(invalid).is_err(), "{invalid:?}");
    }
}

#[test]
fn add_and_remove_report_vanilla_translations_and_persist_as_forced_tickets() {
    let mut state = state();
    assert_eq!(
        run(&mut state, "forceload add 0 0"),
        reply(
            "commands.forceload.added.single",
            &["[0, 0]", OVERWORLD],
            false
        )
    );
    assert_eq!(
        state.tickets["minecraft:forced:0:0"],
        (ChunkPos { x: 0, z: 0 }, 0)
    );
    assert_eq!(
        run(&mut state, "forceload add 5 5"),
        reply("commands.forceload.added.failure", &[], true),
        "same chunk is not a change"
    );
    assert_eq!(
        run(&mut state, "forceload add -1 -17"),
        reply(
            "commands.forceload.added.single",
            &["[-1, -2]", OVERWORLD],
            false
        )
    );
    assert_eq!(
        run(&mut state, "forceload add 0 0 31 31"),
        reply(
            "commands.forceload.added.multiple",
            &["3", OVERWORLD, "[0, 0]", "[1, 1]"],
            false
        ),
        "only the three new chunks count"
    );
    let durable = crate::core::checkpoint_tickets(&state, 7);
    assert_eq!(durable.len(), 5);
    assert_eq!(durable["minecraft:forced:1:1"], ([1, 1], (7, 0)));
    assert_eq!(
        run(&mut state, "forceload remove 16 16"),
        reply(
            "commands.forceload.removed.single",
            &["[1, 1]", OVERWORLD],
            false
        )
    );
    assert_eq!(
        run(&mut state, "forceload remove 16 16"),
        reply("commands.forceload.removed.failure", &[], true)
    );
    assert_eq!(
        run(&mut state, "forceload remove all"),
        reply("commands.forceload.removed.all", &[OVERWORLD], false)
    );
    assert!(state.tickets.is_empty());
    assert_eq!(
        run(&mut state, "forceload remove all"),
        reply("commands.forceload.removed.all", &[OVERWORLD], false),
        "remove all never fails"
    );
}

#[test]
fn relative_coordinates_use_the_source_position() {
    let mut state = state();
    // The console source stands at world spawn, block (0, 80, 0).
    assert_eq!(run(&mut state, "forceload add ~32 ~-1").args[0], "[2, -1]");
}

#[test]
fn one_command_may_not_exceed_256_chunks() {
    let mut state = state();
    assert_eq!(
        run(&mut state, "forceload add 0 0 255 255"),
        reply(
            "commands.forceload.added.multiple",
            &["256", OVERWORLD, "[0, 0]", "[15, 15]"],
            false
        ),
        "16 x 16 chunks is the maximum"
    );
    assert_eq!(
        run(&mut state, "forceload add 0 0 256 255"),
        reply("commands.forceload.toobig", &["256", "272"], true)
    );
    assert_eq!(state.tickets.len(), 256);
    assert_eq!(
        run(&mut state, "forceload add 30000000 0"),
        reply("argument.pos.outofworld", &[], true)
    );
}

#[test]
fn query_lists_in_vanilla_packed_order_and_answers_per_chunk() {
    let mut state = state();
    assert_eq!(
        run(&mut state, "forceload query"),
        reply("commands.forceload.added.none", &[OVERWORLD], true)
    );
    run(&mut state, "forceload add 16 0");
    assert_eq!(
        run(&mut state, "forceload query"),
        reply(
            "commands.forceload.list.single",
            &[OVERWORLD, "[1, 0]"],
            false
        )
    );
    run(&mut state, "forceload add -1 -1");
    run(&mut state, "forceload add 0 0");
    // Packed `x & 0xFFFFFFFF | z << 32` sorts by z, then unsigned x.
    assert_eq!(
        run(&mut state, "forceload query"),
        reply(
            "commands.forceload.list.multiple",
            &["3", OVERWORLD, "[-1, -1], [0, 0], [1, 0]"],
            false
        )
    );
    assert_eq!(
        run(&mut state, "forceload query 20 3"),
        reply(
            "commands.forceload.query.success",
            &["[1, 0]", OVERWORLD],
            false
        )
    );
    assert_eq!(
        run(&mut state, "forceload query 40 3"),
        reply(
            "commands.forceload.query.failure",
            &["[2, 0]", OVERWORLD],
            true
        )
    );
}

#[test]
fn failure_replies_are_red_translations_through_the_command_pipeline() {
    let mut state = state();
    let mut changes = crate::boundary::ChangeSet::default();
    crate::command_console::execute(
        &mut state,
        crate::command_console::CONNECTION,
        "forceload remove 0 0",
        4,
        &mut changes,
    );
    let sent = state
        .outbox
        .get(&crate::command_console::CONNECTION)
        .expect("console reply");
    let Some(PlayerChange::StyledChat {
        content: Tag::Compound(fields),
    }) = sent.last()
    else {
        panic!("styled reply expected: {sent:?}");
    };
    assert!(fields.contains(&(
        "translate".into(),
        Tag::String("commands.forceload.removed.failure".into())
    )));
    assert!(fields.contains(&("color".into(), Tag::String("red".into()))));
    // Non-operators never see the command.
    assert!(crate::commands::parse("forceload add 0 0", false).is_err());
}

#[test]
fn forced_chunks_stay_loaded_and_simulated_with_no_player() {
    let mut state = state();
    assert!(state.players.is_empty());
    run(&mut state, "forceload add 160 160");
    let chunk = ChunkPos { x: 10, z: 10 };
    for _ in 0..3 {
        run_tick(&mut state, &[]);
        let want = state.desired_chunks();
        state.world.ensure_blocking(&want);
    }
    run_tick(&mut state, &[]);
    // Level 31: the chunk and one ring simulate, a two-chunk ring loads.
    for dz in -2..=2_i32 {
        for dx in -2..=2_i32 {
            assert!(
                state.world.has_chunk(chunk.x + dx, chunk.z + dz),
                "loaded ({dx}, {dz})"
            );
            assert_eq!(
                state.world.is_active(chunk.x + dx, chunk.z + dz),
                dx.abs().max(dz.abs()) <= 1,
                "simulated ({dx}, {dz})"
            );
        }
    }
    assert!(!state.world.has_chunk(chunk.x + 3, chunk.z));
    run(&mut state, "forceload remove 160 160");
    run_tick(&mut state, &[]);
    assert!(!state.world.is_active(chunk.x, chunk.z));
}

#[test]
fn forced_neighbour_ticks_blocks_but_freezes_entity_motion_and_age() {
    let mut state = state();
    run(&mut state, "forceload add 160 160");
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state.refresh_active_chunks(state.tick);
    let centre = state.spawn_mob(state.registry.cow_type_id, [168.5, 200.0, 168.5], 10.0);
    let neighbour = state.spawn_mob(state.registry.cow_type_id, [184.5, 200.0, 168.5], 10.0);
    let neighbour_slot = state.entities.slot_of(neighbour).unwrap();
    let before = state.entities.pos(neighbour_slot);
    let age = state.entities.row(neighbour_slot).age;
    for _ in 0..4 {
        run_tick(&mut state, &[]);
    }
    let neighbour_slot = state.entities.slot_of(neighbour).unwrap();
    assert_eq!(state.entities.pos(neighbour_slot), before);
    assert_eq!(state.entities.row(neighbour_slot).age, age);
    assert!(state.world.is_active(11, 10), "level 32 still ticks blocks");
    let centre_slot = state.entities.slot_of(centre).unwrap();
    assert!(state.entities.pos(centre_slot)[1] < 200.0);
}

#[test]
fn forced_tickets_are_journaled_once_and_removal_leaves_a_tombstone() {
    use crate::journal::JournalEntry;
    let mut state = state();
    run(&mut state, "forceload add 0 0");
    let mut entries = Vec::new();
    crate::chunk_tickets::journal(&mut state, 5, &mut entries);
    assert_eq!(
        entries,
        [JournalEntry::Ticket {
            owner: "minecraft:forced:0:0".into(),
            chunk: [0, 0],
            tick: 5,
            radius: 0
        }]
    );
    entries.clear();
    crate::chunk_tickets::journal(&mut state, 6, &mut entries);
    assert!(entries.is_empty(), "unchanged tickets are not re-journaled");
    run(&mut state, "forceload remove all");
    crate::chunk_tickets::journal(&mut state, 7, &mut entries);
    assert_eq!(
        entries,
        [JournalEntry::TicketRemoved {
            owner: "minecraft:forced:0:0".into(),
            tick: 7
        }]
    );
    // Replay: the tombstone wins over the earlier post-image and the base.
    let mut replay = crate::journal::Replay::default();
    replay
        .tickets
        .insert("minecraft:forced:0:0".into(), ([0, 0], (1, 0)));
    let replay = crate::journal::apply_batch_images(
        replay,
        &[(
            2,
            7,
            vec![
                JournalEntry::Ticket {
                    owner: "minecraft:forced:9:9".into(),
                    chunk: [9, 9],
                    tick: 3,
                    radius: 0,
                },
                JournalEntry::TicketRemoved {
                    owner: "minecraft:forced:0:0".into(),
                    tick: 7,
                },
            ],
        )],
    );
    assert_eq!(
        replay.tickets.keys().collect::<Vec<_>>(),
        ["minecraft:forced:9:9"]
    );
}
