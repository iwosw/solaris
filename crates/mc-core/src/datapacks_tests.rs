use super::*;

fn pack(root: &Path, name: &str, resources: &[(&str, &str)]) {
    let pack = root.join(name);
    std::fs::create_dir_all(&pack).unwrap();
    std::fs::write(
        pack.join("pack.mcmeta"),
        r#"{"pack":{"pack_format":101,"description":"test"}}"#,
    )
    .unwrap();
    for (path, text) in resources {
        let path = pack.join("data").join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
}

fn fixture(packs: PackSet) -> TickState {
    let (mut state, _, _) = crate::hostile_tests::setup(8.0);
    state.datapacks = Runtime::new(packs, PathBuf::new());
    state
}

fn run(state: &mut TickState, line: &str) {
    crate::command_console::execute(
        state,
        crate::command_console::CONNECTION,
        line,
        4,
        &mut ChangeSet::default(),
    );
}

#[test]
fn command_parity_configured_sequence_budget_is_per_invocation() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "test",
        &[(
            "test/function/three.mcfunction",
            "time add 1\ntime add 1\ntime add 1",
        )],
    );
    let mut state = fixture(PackSet::load(directory.path(), Selection::default()).unwrap());
    state.day_time = 0;
    run(
        &mut state,
        "gamerule minecraft:max_command_sequence_length 1",
    );
    run(&mut state, "function test:three");
    assert_eq!(
        state.day_time, 0,
        "the function call itself costs one command"
    );
    run(
        &mut state,
        "gamerule minecraft:max_command_sequence_length 2",
    );
    run(&mut state, "function test:three");
    assert_eq!(state.day_time, 1);
    run(&mut state, "function test:three");
    assert_eq!(
        state.day_time, 2,
        "another invocation in the same tick has its own quota"
    );
}

#[test]
fn function_tag_charges_each_called_function() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "test",
        &[
            ("test/function/a.mcfunction", "time add 1"),
            ("test/function/b.mcfunction", "time add 1"),
            ("test/function/c.mcfunction", "time add 1"),
            (
                "test/tags/function/all.json",
                r#"{"values":["test:a","test:b","test:c"]}"#,
            ),
        ],
    );
    let mut state = fixture(PackSet::load(directory.path(), Selection::default()).unwrap());
    state.day_time = 0;
    run(
        &mut state,
        "gamerule minecraft:max_command_sequence_length 4",
    );
    run(&mut state, "function #test:all");
    assert_eq!(state.day_time, 2);
    assert!(state.datapacks.pending.is_empty());
}

#[test]
fn deferred_tick_function_preserves_one_sequence_budget() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "test",
        &[
            (
                "test/function/three.mcfunction",
                "time add 1\ntime add 1\ntime add 1",
            ),
            (
                "minecraft/tags/function/tick.json",
                r#"{"values":["test:three"]}"#,
            ),
        ],
    );
    let mut state = fixture(PackSet::load(directory.path(), Selection::default()).unwrap());
    state.day_time = 0;
    run(
        &mut state,
        "gamerule minecraft:max_command_sequence_length 2",
    );
    state.dimension_globals.active = true;
    tick(&mut state, &mut ChangeSet::default());
    let jobs = std::mem::take(&mut state.command_context.jobs);
    assert_eq!(
        jobs.len(),
        1,
        "defer the invocation, not independently budgeted lines"
    );
    assert_eq!(jobs[0].text, "function #minecraft:tick");
    state.dimension_globals.active = false;
    for job in jobs {
        crate::command_context::run(
            &mut state,
            &job.text,
            job.context,
            &mut ChangeSet::default(),
        );
    }
    assert_eq!(state.day_time, 1);
}

#[test]
fn directory_functions_load_tick_nested_and_schedule_use_native_commands() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "test",
        &[
            (
                "test/function/load.mcfunction",
                "time set 100\nfunction test:nested",
            ),
            ("test/function/nested.mcfunction", "time add 2"),
            ("test/function/tick.mcfunction", "time add 3"),
            (
                "minecraft/tags/function/load.json",
                r#"{"values":["test:load"]}"#,
            ),
            (
                "minecraft/tags/function/tick.json",
                r#"{"values":["test:tick"]}"#,
            ),
        ],
    );
    let mut state = fixture(PackSet::load(directory.path(), Selection::default()).unwrap());
    tick(&mut state, &mut ChangeSet::default());
    assert_eq!(state.day_time, 105);
    run(&mut state, "schedule function test:nested 2t");
    state.tick += 1;
    tick(&mut state, &mut ChangeSet::default());
    assert_eq!(state.day_time, 108);
    state.tick += 1;
    tick(&mut state, &mut ChangeSet::default());
    assert_eq!(state.day_time, 113);
    run(&mut state, "function test:nested");
    assert_eq!(state.day_time, 115);
}

#[test]
fn recursion_is_iterative_and_shares_one_invocation_budget() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "test",
        &[(
            "test/function/loop.mcfunction",
            "time add 1\nfunction test:loop",
        )],
    );
    let mut state = fixture(PackSet::load(directory.path(), Selection::default()).unwrap());
    state.day_time = 0;
    run(
        &mut state,
        "gamerule minecraft:max_command_sequence_length 6",
    );
    run(&mut state, "function test:loop");
    assert_eq!(state.day_time, 3);
    assert!(state.datapacks.pending.is_empty());
    run(&mut state, "function test:loop");
    assert_eq!(state.day_time, 6);
    state.tick += 1;
    run(&mut state, "function test:loop");
    assert_eq!(state.day_time, 9);
}

#[test]
fn tags_merge_replace_optional_and_cycle_validation() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "a",
        &[
            ("test/function/a.mcfunction", "time set 1"),
            ("test/tags/function/entry.json", r#"{"values":["test:a"]}"#),
        ],
    );
    pack(
        directory.path(),
        "b",
        &[
            ("test/function/b.mcfunction", "time set 2"),
            (
                "test/tags/function/entry.json",
                r#"{"replace":true,"values":["test:b",{"id":"test:absent","required":false}]}"#,
            ),
        ],
    );
    let mut state = fixture(PackSet::load(directory.path(), Selection::default()).unwrap());
    run(&mut state, "function #test:entry");
    assert_eq!(state.day_time, 2);
    pack(
        directory.path(),
        "c",
        &[(
            "test/tags/function/entry.json",
            r##"{"values":["#test:entry"]}"##,
        )],
    );
    assert!(
        PackSet::load(directory.path(), Selection::default())
            .err()
            .unwrap()
            .contains("Cyclic")
    );
}

#[test]
fn custom_recipe_and_block_loot_override_builtins() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "a",
        &[
            (
                "minecraft/recipe/bone_meal.json",
                r#"{"type":"minecraft:crafting_shapeless","ingredients":["minecraft:bone"],"result":{"id":"minecraft:diamond","count":2}}"#,
            ),
            (
                "minecraft/loot_table/blocks/stone.json",
                r#"{"type":"minecraft:block","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:diamond"}]}]}"#,
            ),
        ],
    );
    let packs = PackSet::load(directory.path(), Selection::default()).unwrap();
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    packs.install_content(&mut registry).unwrap();
    let recipes = registry
        .recipes
        .iter()
        .filter(|recipe| recipe.id.as_str() == "minecraft:bone_meal")
        .collect::<Vec<_>>();
    assert_eq!(recipes.len(), 1);
    assert_eq!(recipes[0].result.item.as_str(), "minecraft:diamond");
    let block = mc_data::Identifier::parse("minecraft:stone").unwrap();
    let tool = mc_data::loot::LootContextItem::empty();
    let context = mc_data::loot::BlockLootContext::try_new(
        &block,
        &[],
        &tool,
        mc_data::loot::LootRandomBinding::new(None, 1),
    )
    .unwrap();
    let drops = registry.loot.evaluate_block(&context).unwrap().unwrap();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].item.as_str(), "minecraft:diamond");
}

#[test]
fn worldgen_and_unknown_resources_fail_with_explicit_error() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "a",
        &[("test/worldgen/biome/a.json", "{}")],
    );
    assert!(
        PackSet::load(directory.path(), Selection::default())
            .err()
            .unwrap()
            .contains("Worldgen datapacks are not supported")
    );
}

#[test]
fn nonoperator_cannot_invoke_function() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "a",
        &[("test/function/a.mcfunction", "time set 1234")],
    );
    let mut state = fixture(PackSet::load(directory.path(), Selection::default()).unwrap());
    let before = state.day_time;
    crate::command_console::execute(
        &mut state,
        crate::command_console::CONNECTION,
        "function test:a",
        0,
        &mut ChangeSet::default(),
    );
    assert_eq!(state.day_time, before);
}

#[test]
fn schedule_checkpoint_wal_recovery_and_clear_tombstone() {
    let root = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(root.path().to_owned(), 42);
    crate::native_checkpoint::tests::write_manifest(
        &config,
        &crate::journal::CheckpointManifest {
            scheduled_functions: vec![(100, "test:a".into())],
            ..Default::default()
        },
    )
    .unwrap();
    let mut state =
        crate::core::recover_dimension_state(&config, crate::dimensions::Dimension::Overworld)
            .unwrap();
    assert_eq!(state.datapacks.image(), vec![(100, "test:a".into())]);
    run(&mut state, "schedule clear test:a");
    let mut entries = Vec::new();
    state.datapacks.journal(&mut entries);
    assert!(
        matches!(&entries[..],[crate::journal::JournalEntry::ScheduledFunctions{value}] if value.is_empty())
    );
    let mut writer = crate::journal::JournalWriter::open(root.path(), 1).unwrap();
    writer
        .append(state.tick + 1, state.tick + 1, &entries)
        .unwrap();
    drop(writer);
    let recovered =
        crate::core::recover_dimension_state(&config, crate::dimensions::Dimension::Overworld)
            .unwrap();
    assert!(recovered.datapacks.image().is_empty());
    assert!(
        Runtime::default()
            .restore(vec![(1, "bad name".into())])
            .is_err()
    );
}

#[test]
fn unsupported_loot_is_refused_and_empty_table_is_an_override() {
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    let id = mc_data::Identifier::parse("minecraft:stone").unwrap();
    assert!(registry.loot.override_datapack_block(id.clone(),r#"{"type":"minecraft:block","pools":[{"rolls":1,"conditions":[{"condition":"custom:unsupported"}],"entries":[{"type":"minecraft:item","name":"minecraft:diamond"}]}]}"#).is_err());
    registry
        .loot
        .override_datapack_block(id.clone(), r#"{"type":"minecraft:block"}"#)
        .unwrap();
    let tool = mc_data::loot::LootContextItem::empty();
    let context = mc_data::loot::BlockLootContext::try_new(
        &id,
        &[],
        &tool,
        mc_data::loot::LootRandomBinding::new(None, 1),
    )
    .unwrap();
    assert!(
        registry
            .loot
            .evaluate_block(&context)
            .unwrap()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn nested_functions_preserve_execute_position_rotation_and_permission() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "a",
        &[
            ("test/function/outer.mcfunction", "function test:inner"),
            (
                "test/function/inner.mcfunction",
                "setblock ^ ^ ^1 minecraft:diamond_block",
            ),
        ],
    );
    let mut state = fixture(PackSet::load(directory.path(), Selection::default()).unwrap());
    run(
        &mut state,
        "execute positioned 5 60 5 rotated 0 0 run function test:outer",
    );
    let block = state.world.block_at(5, 60, 6).unwrap();
    assert_eq!(
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(block))
            .unwrap()
            .block
            .id
            .path(),
        "diamond_block"
    );
    assert!(state.command_context.current.is_none());
}

#[test]
fn malformed_delay_is_rejected_without_scheduling() {
    let directory = tempfile::tempdir().unwrap();
    pack(
        directory.path(),
        "a",
        &[("test/function/a.mcfunction", "time add 1")],
    );
    let mut state = fixture(PackSet::load(directory.path(), Selection::default()).unwrap());
    run(&mut state, "schedule function test:a 2ss");
    assert!(state.datapacks.image().is_empty());
}

#[cfg(unix)]
#[test]
fn metadata_and_data_root_symlinks_are_rejected() {
    let directory = tempfile::tempdir().unwrap();
    pack(directory.path(), "a", &[]);
    let external = tempfile::NamedTempFile::new().unwrap();
    std::fs::remove_file(directory.path().join("a/pack.mcmeta")).unwrap();
    std::os::unix::fs::symlink(external.path(), directory.path().join("a/pack.mcmeta")).unwrap();
    assert!(PackSet::load(directory.path(), Selection::default()).is_err());
    std::fs::remove_file(directory.path().join("a/pack.mcmeta")).unwrap();
    pack(directory.path(), "a", &[]);
    let external = tempfile::tempdir().unwrap();
    std::os::unix::fs::symlink(external.path(), directory.path().join("a/data")).unwrap();
    assert!(PackSet::load(directory.path(), Selection::default()).is_err());
}

#[test]
#[ignore = "release microbenchmark; ordinary lane, approximate contended-machine measurement"]
fn datapack_idle_and_execute_microbench() {
    let mut state = fixture(PackSet::default());
    let mut changes = ChangeSet::default();
    let mut idle = Vec::new();
    let mut execute = Vec::new();
    for _ in 0..7 {
        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            state.tick += 1;
            tick(std::hint::black_box(&mut state), &mut changes);
        }
        idle.push(start.elapsed().as_nanos() / 10_000);
        let start = std::time::Instant::now();
        for _ in 0..10_000 {
            state.tick += 1;
            run(
                std::hint::black_box(&mut state),
                "execute positioned 8 60 8 rotated 0 0 unless entity @s run time add 1",
            );
            state.outbox.clear();
        }
        execute.push(start.elapsed().as_nanos() / 10_000);
    }
    idle.sort_unstable();
    execute.sort_unstable();
    eprintln!(
        "approximate, contended machine: idle datapack ns/call median={} samples={idle:?}; execute ns/call median={} samples={execute:?}",
        idle[3], execute[3]
    );
}
