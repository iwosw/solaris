use super::*;
use crate::boundary::PlayerInput;
use crate::commands::parse;
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
                permission_level: 0,
            },
            PlayerInput::Join {
                connection: ConnectionId(2),
                uuid: uuid::Uuid::new_v4(),
                name: "Bob".into(),
                permission_level: 0,
            },
        ],
    );
    let alice = state.connections.entity_of(ConnectionId(1)).unwrap();
    let bob = state.connections.entity_of(ConnectionId(2)).unwrap();
    for (entity, position) in [(alice, [0.0, 80.0, 0.0]), (bob, [10.0, 80.0, 0.0])] {
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.set_pos(slot, position);
    }
    state.outbox.clear();
    (state, alice, bob)
}

fn command(state: &mut TickState, executor: EntityId, input: &str) -> ChangeSet {
    let command = parse(input, true).unwrap();
    let mut changes = ChangeSet::default();
    execute(
        state,
        state.connections.entity_connection(executor).unwrap(),
        executor,
        &command,
        &mut changes,
    );
    changes
}

fn has_translation(state: &TickState, connection: ConnectionId, key: &str) -> bool {
    state.outbox.get(&connection).is_some_and(|queue| {
        queue.iter().any(|change| {
            matches!(change,
        PlayerChange::StyledChat { content:Tag::Compound(fields),.. }
        if fields.iter().any(|(name,value)|name=="translate" && value==&Tag::String(key.into())))
        })
    })
}

fn advancement_fixture() -> mc_data::advancements::AdvancementTree {
    use mc_data::advancements::{AdvancementDefinition, AdvancementTree, Criterion};
    let mut tree = AdvancementTree::default();
    for (id, parent) in [
        ("root", None),
        ("branch", Some("root")),
        ("leaf", Some("branch")),
        ("sibling", Some("root")),
        ("other", None),
    ] {
        tree.definitions.insert(
            format!("test:{id}"),
            AdvancementDefinition {
                parent: parent.map(|parent| format!("test:{parent}")),
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
    }
    tree
}

#[test]
fn advancement_tree_scopes_follow_parents_and_descendants_not_siblings() {
    use crate::commands::AdvancementScope;
    let tree = advancement_fixture();
    for (scope, expected) in [
        (
            AdvancementScope::Everything,
            vec!["branch", "leaf", "other", "root", "sibling"],
        ),
        (AdvancementScope::Only, vec!["branch"]),
        (AdvancementScope::From, vec!["branch", "leaf"]),
        (AdvancementScope::Through, vec!["branch", "leaf", "root"]),
        (AdvancementScope::Until, vec!["branch", "root"]),
    ] {
        let mut ids = advancement_ids(&tree, scope, Some("test:branch")).unwrap();
        ids.sort();
        assert_eq!(
            ids,
            expected
                .into_iter()
                .map(|id| format!("test:{id}"))
                .collect::<Vec<_>>()
        );
    }
    assert!(advancement_ids(&tree, AdvancementScope::Only, Some("test:missing")).is_err());
}

#[test]
fn advancement_dispatch_validates_before_mutating_and_filters_player_targets() {
    let (mut state, alice, bob) = fixture();
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    registry.advancements = Arc::new(advancement_fixture());
    state.registry = Arc::new(registry);
    state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    let before = state.players[&bob].advancement_progress.clone();
    command(&mut state, alice, "advancement grant Bob only test:missing");
    command(
        &mut state,
        alice,
        "advancement grant Bob only test:branch missing",
    );
    assert_eq!(state.players[&bob].advancement_progress, before);
    command(&mut state, alice, "advancement grant @e from test:branch");
    for entity in [alice, bob] {
        let progress = &state.players[&entity].advancement_progress;
        assert_eq!(progress["test:branch"]["done"], true);
        assert_eq!(progress["test:leaf"]["done"], true);
        assert!(progress.get("test:root").is_none());
    }
    command(
        &mut state,
        alice,
        "advancement revoke Bob only test:leaf command",
    );
    assert_ne!(
        state.players[&bob].advancement_progress["test:leaf"]["done"],
        true
    );
    assert_eq!(
        state.players[&alice].advancement_progress["test:leaf"]["done"],
        true
    );
}

#[test]
fn selectors_cover_nearest_all_random_self_entities_and_filters() {
    let (mut state, alice, bob) = fixture();
    let cow = state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    assert_eq!(
        resolve_target(&state, bob, &crate::commands::target_of("@p").unwrap()),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[sort=furthest,limit=1]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@e[type=cow,distance=..3]").unwrap()
        ),
        vec![cow]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@e[type=!player,distance=1..3]").unwrap()
        ),
        vec![cow]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[name=!Alice]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@s[distance=0]").unwrap()
        ),
        vec![alice]
    );
    assert_eq!(
        resolve_target(&state, alice, &crate::commands::target_of("@r").unwrap()).len(),
        1
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@e[sort=nearest,limit=2]").unwrap()
        ),
        vec![alice, cow]
    );
}

#[test]
fn gamemode_give_clear_and_experience_apply_and_feedback() {
    let (mut state, alice, bob) = fixture();
    command(&mut state, alice, "gamemode creative Bob");
    assert_eq!(
        state.players[&bob].game_mode,
        crate::commands::GameMode::Creative
    );
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.gamemode.success.other"
    ));
    command(&mut state, alice, "give Bob stone 67");
    let stone = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    assert_eq!(
        state.players[&bob]
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == stone)
            .map(|stack| stack.count)
            .sum::<i32>(),
        67
    );
    command(&mut state, alice, "clear Bob stone 0");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.clear.test.single"
    ));
    assert_eq!(
        state.players[&bob]
            .inventory
            .slots
            .iter()
            .map(|stack| stack.count.max(0))
            .sum::<i32>(),
        67
    );
    command(&mut state, alice, "clear Bob stone 7");
    assert_eq!(
        state.players[&bob]
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == stone)
            .map(|stack| stack.count)
            .sum::<i32>(),
        60
    );
    command(&mut state, alice, "experience add Bob 5 levels");
    command(&mut state, alice, "xp set Bob 3 points");
    assert_eq!(state.players[&bob].vitals.experience.level, 5);
    assert!((state.players[&bob].vitals.experience.progress - 3.0 / 17.0).abs() < 0.001);
    command(&mut state, alice, "xp query Bob levels");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.experience.query.levels"
    ));
    command(&mut state, alice, "xp set Bob 100 points");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.experience.set.points.invalid"
    ));
    command(&mut state, alice, "give Bob stone 6401");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.give.failed.toomanyitems"
    ));
}

#[test]
fn teleport_uses_executor_relative_origin_for_every_target_and_rejects_multiple_destination() {
    let (mut state, alice, bob) = fixture();
    command(&mut state, alice, "tp @a ~2 ~1 ~3");
    for entity in [alice, bob] {
        assert_eq!(
            state.entities.pos(state.entities.slot_of(entity).unwrap()),
            [2.0, 81.0, 3.0]
        );
    }
    assert!(state.outbox[&ConnectionId(2)].iter().any(|change|matches!(change,PlayerChange::PositionCorrected{position,..}if *position==[2.0,81.0,3.0])));
    command(&mut state, alice, "tp Bob @a");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "argument.entity.toomany"
    ));
}

#[test]
fn time_weather_difficulty_gamerule_and_seed_have_real_mutations() {
    let (mut state, alice, _) = fixture();
    state.day_time = 72000;
    command(&mut state, alice, "time set 30000");
    assert_eq!(state.day_time, 30000);
    command(&mut state, alice, "time add -2s");
    assert_eq!(state.day_time, 29960);
    command(&mut state, alice, "time query gametime");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.time.query"
    ));
    command(&mut state, alice, "weather thunder 5s");
    assert_eq!(state.weather, (Weather::Thunder, 100));
    assert_eq!(state.rain_level, 0.0);
    assert_eq!(state.thunder_level, 0.0);
    assert!(
        !state
            .outbox
            .values()
            .flatten()
            .any(|change| { matches!(change, PlayerChange::WeatherChanged { .. }) })
    );
    let (_, drains, _) = crate::tick::run_tick(&mut state, &[]);
    assert!(drains.iter().any(|(connection, changes)| {
        *connection == ConnectionId(2)
            && changes.iter().any(|change| {
                matches!(
                    change,
                    PlayerChange::WeatherChanged { raining: true, thunder_level, .. }
                        if *thunder_level == 0.01
                )
            })
    }));
    command(&mut state, alice, "difficulty hard");
    assert_eq!(state.difficulty, crate::survival::Difficulty::Hard);
    command(&mut state, alice, "difficulty");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.difficulty.query"
    ));
    command(&mut state, alice, "gamerule doDaylightCycle false");
    assert_eq!(
        state.gamerules.get("advance_time").map(String::as_str),
        Some("false")
    );
    command(&mut state, alice, "gamerule minecraft:advance_time");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.gamerule.query"
    ));
    command(&mut state, alice, "seed");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.seed.success"
    ));
}

#[test]
fn summon_uses_executor_position_and_kill_stages_canonical_damage() {
    let (mut state, alice, _) = fixture();
    let changes = command(&mut state, alice, "summon cow ~2 ~ ~");
    assert_eq!(changes.entities_spawned.len(), 1);
    let cow = changes.entities_spawned[0];
    assert_eq!(
        state.entities.pos(state.entities.slot_of(cow).unwrap()),
        [2.0, 80.0, 0.0]
    );
    command(&mut state, alice, "kill @e[type=cow]");
    assert!(
        state
            .merge_damage
            .iter()
            .any(|(slot, damage)| state.entities.body(*slot).id == cow
                && damage.kind == crate::survival::DamageKind::Void)
    );
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.kill.success.single"
    ));
}

#[test]
fn chat_routing_preserves_message_and_unprivileged_selector_literals() {
    let (mut state, alice, _) = fixture();
    command(&mut state, alice, "say hello ] \"quoted\" @a");
    assert!(has_translation(
        &state,
        ConnectionId(2),
        "chat.type.announcement"
    ));
    let parsed = parse("me waves @a ]", false).unwrap();
    execute(
        &mut state,
        ConnectionId(1),
        alice,
        &parsed,
        &mut ChangeSet::default(),
    );
    assert!(has_translation(&state, ConnectionId(2), "chat.type.emote"));
    command(&mut state, alice, "tell Bob hello  world @é");
    assert!(has_translation(
        &state,
        ConnectionId(2),
        "commands.message.display.incoming"
    ));
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.message.display.outgoing"
    ));
    assert!(parse("msg @a hello", false).is_err());
    assert_eq!(expand_message(&state, alice, "mail @é @"), "mail @é @");
}

#[test]
fn command_boundary_enforces_operator_level() {
    let (mut state, alice, _) = fixture();
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            command: "gamemode creative".into(),
            permission_level: 0,
        }],
    );
    assert_eq!(
        state.players[&alice].game_mode,
        crate::commands::GameMode::Survival
    );
}

#[test]
fn selector_position_overrides_distance_sort_and_old_axis_volumes() {
    let (mut state, alice, bob) = fixture();
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@p[x=10,y=80,z=0]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[x=10,distance=..1]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[x=10,dx=-2]").unwrap()
        ),
        vec![bob]
    );
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("@a[x=0,dx=9]").unwrap()
        ),
        vec![alice],
        "the upper axis boundary is exclusive"
    );
    command(
        &mut state,
        alice,
        "give @a[x=10,dx=0,y=80,dy=0,z=0,dz=0] stone 2",
    );
    assert!(
        state.players[&alice]
            .inventory
            .slots
            .iter()
            .all(mc_data::ItemStack::is_empty)
    );
    assert_eq!(
        state.players[&bob]
            .inventory
            .slots
            .iter()
            .map(|item| item.count)
            .sum::<i32>(),
        2
    );
    for options in ["x=NaN", "dy=inf", "x=1,x=2", "dz=1,dz=2"] {
        assert!(crate::commands::target_of(&format!("@a[{options}]")).is_err());
    }
}

#[test]
fn selectors_match_plain_custom_name_component_for_summoned_entities() {
    let (mut state, alice, _) = fixture();
    state.world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    for nbt in ["{CustomName:'Foo'}", "{CustomName:{text:'Foo'}}"] {
        let before: std::collections::HashSet<_> = state
            .entities
            .slots()
            .map(|slot| state.entities.body(slot).id)
            .collect();
        command(&mut state, alice, &format!("summon cow 2 200 2 {nbt}"));
        let cow = state
            .entities
            .slots()
            .map(|slot| state.entities.body(slot).id)
            .find(|id| !before.contains(id))
            .expect("summoned named cow");
        let target = |selector: &str| crate::commands::target_of(selector).unwrap();
        assert!(resolve_target(&state, alice, &target("@e[name=Foo]")).contains(&cow));
        assert!(!resolve_target(&state, alice, &target("@e[name=!Foo]")).contains(&cow));
        assert!(!resolve_target(&state, alice, &target("@e[name=Bar]")).contains(&cow));
        assert!(resolve_target(&state, alice, &target("@e[name=!Bar]")).contains(&cow));
    }
}

#[test]
fn json_shaped_summon_custom_name_stays_literal_in_vanilla_26() {
    let (mut state, alice, _) = fixture();
    state.world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: 0, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    command(
        &mut state,
        alice,
        r#"summon cow 2 200 2 {CustomName:'{"text":"Foo"}'}"#,
    );
    let target = crate::commands::target_of("@e[type=cow,name=Foo]").unwrap();
    assert!(resolve_target(&state, alice, &target).is_empty());
    let Target::Selector(mut selector) = target else {
        panic!("entity selector");
    };
    selector.name = Some((r#"{"text":"Foo"}"#.into(), false));
    assert_eq!(
        resolve_target(&state, alice, &Target::Selector(selector)).len(),
        1
    );
}

#[test]
fn named_and_uuid_entity_targets_preserve_player_only_command_filtering() {
    let (mut state, alice, _) = fixture();
    let cow = state.spawn_mob(state.registry.cow_type_id, [2.0, 80.0, 0.0], 10.0);
    let slot = state.entities.slot_of(cow).unwrap();
    let uuid = state.entities.row(slot).uuid.to_string();
    assert_eq!(
        resolve_target(
            &state,
            alice,
            &crate::commands::target_of("minecraft:cow").unwrap()
        ),
        vec![cow]
    );
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name =
        Some("Bessie".into());
    for name in ["Bessie", uuid.as_str()] {
        assert_eq!(
            resolve_target(&state, alice, &crate::commands::target_of(name).unwrap()),
            vec![cow]
        );
    }
    command(&mut state, alice, "give Bessie stone");
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "argument.entity.notfound.player"
    ));
    command(&mut state, alice, &format!("kill {uuid}"));
    assert!(state.merge_damage.iter().any(|(target, _)| *target == slot));
}

#[test]
fn gamerule_wal_images_preserve_canonical_aliases_and_absent_subsystem_overrides() {
    use crate::journal::{
        JournalEntry, JournalWriter, Replay, read_complete_batches_after, replay_batches,
    };
    let (mut state, alice, _) = fixture();
    command(&mut state, alice, "gamerule keepInventory true");
    command(&mut state, alice, "gamerule randomTickSpeed 0");
    command(
        &mut state,
        alice,
        "gamerule allowEnteringNetherUsingPortals false",
    );
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let values = entries
        .iter()
        .find_map(|entry| match entry {
            JournalEntry::GameRules { values } => Some(values.clone()),
            _ => None,
        })
        .expect("one absolute gamerule postimage");
    assert_eq!(
        values.get("keep_inventory").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        values.get("random_tick_speed").map(String::as_str),
        Some("0")
    );
    assert_eq!(
        values
            .get("allow_entering_nether_using_portals")
            .map(String::as_str),
        Some("false")
    );
    assert!(!values.contains_key("keepInventory"));
    let dir = std::env::temp_dir().join(format!("mc-core-gamerules-{}", uuid::Uuid::new_v4()));
    {
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer.append(1, output.tick, &entries).unwrap();
        writer.flush().unwrap();
    }
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let recovered = replay_batches(Replay::default(), &batches);
    assert_eq!(recovered.gamerules, values);
    let replayed_again = replay_batches(recovered, &batches);
    assert_eq!(
        replayed_again.gamerules, values,
        "absolute images are idempotent"
    );
    command(&mut state, alice, "gamerule minecraft:keep_inventory true");
    let (unchanged, _, _) = crate::tick::run_tick(&mut state, &[]);
    assert!(
        !crate::core::journal_entries_for_tick(&mut state, &unchanged)
            .iter()
            .any(|entry| matches!(entry, JournalEntry::GameRules { .. })),
        "queries and unchanged writes do not copy the rule map each tick"
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn summon_snbt_initializes_native_flags_motion_sheep_age_and_restart_image() {
    let (mut state, actor, _) = fixture();
    let changes = command(
        &mut state,
        actor,
        "summon sheep 2 80 3 {Health:7f, NoAI:1b, Invulnerable:1b, Age:-1234, Color:14b, Sheared:1b, Fire:120s, FallDistance:2.5f, Motion:[0.1d,0d,0d], Rotation:[90f,7f], CustomName:'Template Sheep'}",
    );
    assert_eq!(changes.entities_spawned.len(), 1);
    let id = changes.entities_spawned[0];
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(state.entities.vel(slot), [2.0, 0.0, 0.0]);
    assert_eq!(state.entities.rot(slot)[..2], [90.0, 7.0]);
    let row = state.entities.row(slot);
    assert_eq!(row.health, 7.0);
    assert_eq!(row.entity_life.child_ticks, 1234);
    assert!(row.controls.no_ai && row.controls.invulnerable && row.appearance.baby);
    assert_eq!(
        row.appearance.custom_name.as_deref(),
        Some("Template Sheep")
    );
    assert_eq!(crate::tick::wool_of(&row.appearance), 30);
    assert_eq!(
        state
            .entities
            .land_hit(slot, 1000, 5.0, crate::survival::DamageKind::Melee),
        0.0
    );
    assert_eq!(
        state
            .entities
            .land_hit(slot, 1000, 5.0, crate::survival::DamageKind::Void),
        5.0
    );
    let image = crate::journal::EntityStateImage::from_row(
        state.entities.body(slot),
        state.entities.row(slot),
    );
    let (_, regions, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let native = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == state.entities.row(slot).uuid)
        .unwrap();
    let recovered = crate::journal::EntityStateImage::from_record(&state.registry, native).unwrap();
    assert_eq!(recovered.controls, image.controls);
    assert_eq!(recovered.entity_life.child_ticks, 1234);
    let wal: crate::journal::EntityStateImage =
        serde_json::from_slice(&serde_json::to_vec(&image).unwrap()).unwrap();
    assert_eq!(wal, image);
}

#[test]
fn summon_rejects_malformed_unsupported_and_duplicate_identity_before_spawn() {
    let (mut state, actor, _) = fixture();
    let baseline = state.entities.len();
    for raw in [
        "summon cow 2 80 3 {Color:1b}",
        "summon cow 2 80 3 {Health:NaNf}",
        "summon cow 2 80 3 {Motion:[0d,0d]}",
        "summon cow 2 80 3 {Motion:[NaNd,0d,0d]}",
        "summon cow 2 80 3 {NoAI:'bad'}",
        "summon cow 2 80 3 {Unknown:1}",
        "summon wolf 2 80 3 {Owner:[I;1,2]}",
        "summon horse 2 80 3 {equipment:{unknown:{id:'minecraft:iron_horse_armor',count:1}}}",
    ] {
        assert!(
            command(&mut state, actor, raw).entities_spawned.is_empty(),
            "{raw}"
        );
        assert_eq!(state.entities.len(), baseline, "{raw}");
    }
    let uuid = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    assert!(
        command(
            &mut state,
            actor,
            &format!("summon cow 2 80 3 {{UUID:'{uuid}'}}")
        )
        .entities_spawned
        .is_empty()
    );
    assert!(parse("summon cow 2 80 3 {NoAI:1b,NoAI:0b}", true).is_err());
}

#[test]
fn summon_mule_age_is_retained_despite_no_breeding_offspring() {
    let (mut state, actor, _) = fixture();
    let changes = command(&mut state, actor, "summon mule 2 80 3 {Age:-321}");
    let slot = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    assert_eq!(row.entity_life.child_ticks, 321);
    assert!(row.appearance.baby);
    let image = crate::journal::EntityStateImage::from_row(body, row);
    let record = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:mule".into(),
        age: -321,
        extra_fields: image.to_fields(&state.registry.blocks),
        ..Default::default()
    };
    assert_eq!(
        crate::journal::EntityStateImage::from_record(&state.registry, &record)
            .unwrap()
            .entity_life
            .child_ticks,
        321
    );
}

#[test]
fn summon_uses_native_owner_leash_and_exact_saddle_stack_images() {
    let (mut state, actor, _) = fixture();
    let changes = command(
        &mut state,
        actor,
        "summon horse 2 80 3 {Owner:[I;1,2,3,4], Tame:1b, Temper:72, leash:{UUID:[I;5,6,7,8]}, equipment:{saddle:{id:'minecraft:saddle',count:1}}}",
    );
    assert_eq!(changes.entities_spawned.len(), 1);
    let slot = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    assert!(row.entity_life.tamed);
    assert_eq!(row.entity_life.taming.temper, 72);
    assert_eq!(
        row.entity_life.taming.owner,
        Some(uuid::Uuid::from_u128(
            (1u128 << 96) | (2u128 << 64) | (3u128 << 32) | 4
        ))
    );
    assert_eq!(
        row.entity_life.leash,
        Some(crate::leash::Holder::Entity(uuid::Uuid::from_u128(
            (5u128 << 96) | (6u128 << 64) | (7u128 << 32) | 8
        )))
    );
    assert_eq!(
        state
            .registry
            .items
            .name_of(row.loot.saddle.as_ref().unwrap().item_id)
            .unwrap()
            .path(),
        "saddle"
    );
    let image = crate::journal::EntityStateImage::from_row(body, row);
    let (_, regions, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let native = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|record| record.uuid == row.uuid)
        .unwrap();
    let restored = crate::journal::EntityStateImage::from_record(&state.registry, native).unwrap();
    assert_eq!(restored.entity_life, image.entity_life);
    assert_eq!(restored.loot.saddle, image.loot.saddle);
}

#[test]
fn component_item_commands_publish_native_inventory_and_selective_clear_feedback() {
    let (mut state, alice, bob) = fixture();
    let (_, deliveries, _) = crate::tick::run_tick(&mut state, &[
        PlayerInput::Command {
            connection: ConnectionId(1), permission_level: 4,
            command: r#"give @a potion[potion_contents={potion:"minecraft:water"},custom_name={text:"QA water",color:"gold"}] 1"#.into(),
        },
        PlayerInput::Command {
            connection: ConnectionId(1), permission_level: 4,
            command: r#"give Bob potion[potion_contents={potion:"minecraft:poison"}] 1"#.into(),
        },
    ]);
    for player in [alice, bob] {
        assert!(state.players[&player].inventory.slots.iter().any(|stack| {
            stack.custom_name.as_deref() == Some("QA water")
                && stack
                    .extended()
                    .potion
                    .as_ref()
                    .is_some_and(|potion| potion.as_str() == "minecraft:water")
        }));
    }
    assert!(deliveries.iter().find_map(|(connection, queue)| (*connection == ConnectionId(1)).then_some(queue))
        .is_some_and(|queue| queue.iter().any(|change| matches!(change,
            PlayerChange::StyledChat { content: Tag::Compound(fields), .. }
            if fields.iter().any(|(name, value)| name == "translate" && value == &Tag::String("commands.give.success.multiple".into()))
        ))));
    let before = state.players[&bob].inventory.clone();
    command(
        &mut state,
        alice,
        r#"clear Bob potion[potion_contents={potion:"minecraft:water"}] 0"#,
    );
    assert_eq!(state.players[&bob].inventory, before);
    command(
        &mut state,
        alice,
        r#"clear Bob potion[potion_contents={potion:"minecraft:water"}]"#,
    );
    assert!(state.players[&bob].inventory.slots.iter().all(|stack| {
        stack
            .extended()
            .potion
            .as_ref()
            .is_none_or(|potion| potion.as_str() != "minecraft:water")
    }));
    assert!(state.players[&bob].inventory.slots.iter().any(|stack| {
        stack
            .extended()
            .potion
            .as_ref()
            .is_some_and(|potion| potion.as_str() == "minecraft:poison")
    }));
    assert!(has_translation(
        &state,
        ConnectionId(1),
        "commands.clear.success.single"
    ));
}

#[test]
fn malformed_item_component_reports_error_without_any_inventory_write() {
    let (mut state, alice, _) = fixture();
    let before = state.players[&alice].inventory.clone();
    command(
        &mut state,
        alice,
        r#"give @s potion[potion_contents={potion:"minecraft:no_such_potion"}]"#,
    );
    assert_eq!(state.players[&alice].inventory, before);
    assert!(state.outbox[&ConnectionId(1)].iter().any(|change| matches!(change, PlayerChange::Chat { message, .. } if message.contains("unknown potion"))));
    state.outbox.clear();
    let (_, deliveries, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            permission_level: 4,
            command: "give @s stone[not_a_component=1]".into(),
        }],
    );
    assert_eq!(state.players[&alice].inventory, before);
    assert!(deliveries.iter().find_map(|(connection, queue)| (*connection == ConnectionId(1)).then_some(queue))
        .is_some_and(|queue| queue.iter().any(|change| matches!(change,
            PlayerChange::Chat { message, .. } if message.contains("Unsupported item component minecraft:not_a_component")
        ))));
}

#[test]
fn command_component_inventory_survives_real_wal_replay_and_native_player_checkpoint() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    use crate::native_checkpoint::tests::{read_player, write_player};
    let (mut state, alice, _) = fixture();
    let (output, _, _) = crate::tick::run_tick(&mut state, &[
        PlayerInput::Command {
            connection: ConnectionId(1), permission_level: 4,
            command: r#"give @s potion[potion_contents={potion:"minecraft:water"},custom_name={text:"Saved water",color:"gold"},!enchantment_glint_override] 2"#.into(),
        },
        PlayerInput::Command {
            connection: ConnectionId(1), permission_level: 4,
            command: r#"give @s diamond_sword[enchantments={"minecraft:sharpness":5}]"#.into(),
        },
    ]);
    let uuid = state
        .entities
        .row(state.entities.slot_of(alice).unwrap())
        .uuid
        .to_string();
    let expected = state.players[&alice].inventory.slots.clone();
    assert_eq!(
        expected
            .iter()
            .filter(|stack| stack.custom_name.as_deref() == Some("Saved water"))
            .map(|stack| stack.count)
            .sum::<i32>(),
        2
    );
    assert!(expected.iter().any(|stack| {
        stack
            .enchantments
            .iter()
            .any(|entry| entry.id.as_str() == "minecraft:sharpness" && entry.level == 5)
    }));
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let directory = tempfile::tempdir().unwrap();
    {
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        writer.append(1, output.tick, &entries).unwrap();
        writer.flush().unwrap();
    }
    let batches = read_complete_batches_after(&directory.path().join("journal"), 0).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    assert_eq!(
        replay.players[&uuid].inventory.as_slice(),
        expected.as_slice()
    );
    let replay = replay_batches(replay, &batches);
    assert_eq!(
        replay.players[&uuid].inventory.as_slice(),
        expected.as_slice(),
        "absolute WAL images do not duplicate command grants"
    );
    let config = crate::CoreConfig::new(directory.path().to_owned(), 42);
    write_player(
        &config,
        &uuid,
        &replay.players[&uuid],
        &state.registry.items,
    )
    .unwrap();
    let restored = read_player(&config, &uuid, &state.registry.items).unwrap();
    assert_eq!(
        restored.inventory.as_slice(),
        expected.as_slice(),
        "native checkpoint reader retains styled text and explicit removals"
    );
    write_player(&config, &uuid, &restored, &state.registry.items).unwrap();
    let restored_again = read_player(&config, &uuid, &state.registry.items).unwrap();
    assert_eq!(restored_again.inventory.as_slice(), expected.as_slice());
}

#[test]
fn settlement_create_refuses_without_authenticated_actor_and_creates_nothing() {
    let (mut state, alice, _) = fixture();
    // Simulate a connection whose UUID registration is gone: every mutation
    // must refuse rather than invent a shared owner.
    let ghost = crate::boundary::ConnectionId(99);
    let parsed = parse("settlement create ghost-town", true).unwrap();
    let mut changes = ChangeSet::default();
    super::execute(&mut state, ghost, alice, &parsed, &mut changes);
    assert!(state.settlements.find("ghost-town").is_none());
    assert!(state.settlements.records().next().is_none());
}

#[test]
fn settlement_commands_keep_native_identity_and_enforce_owner_or_operator() {
    let (mut state, alice, bob) = fixture();
    assert!(parse("settlement create town", false).is_ok());
    let alice_slot = state.entities.slot_of(alice).unwrap();
    state.entities.set_pos(alice_slot, [-0.5, 80.0, -0.5]);
    command(&mut state, alice, "settlement create town");
    let record = state.settlements.find("town").unwrap();
    let id = record.id;
    assert_eq!(
        record.anchor,
        [-1, 80, -1],
        "anchor is the containing block"
    );
    assert_eq!(
        record.owner,
        state
            .connections
            .uuid_of(ConnectionId(1))
            .unwrap()
            .to_string()
    );
    assert_eq!(record.radius, 48);
    let villager_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let resident = state.spawn_mob(villager_type, [1.0, 80.0, 1.0], 20.0);
    let slot = state.entities.slot_of(resident).unwrap();
    state.entities.row_mut(slot).villager =
        Some(Box::new(crate::villager::VillagerState::default()));
    let uuid = state.entities.row(slot).uuid;
    let bodies = state.entities.len();

    command(&mut state, bob, &format!("settlement bind town {uuid}"));
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        None
    );
    command(&mut state, alice, &format!("settlement bind town {uuid}"));
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        Some(id)
    );
    assert_eq!(crate::settlements::member_count(&state, id), 1);
    command(&mut state, bob, &format!("settlement unbind {uuid}"));
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        Some(id)
    );
    command(&mut state, bob, "settlement rename town stolen");
    command(&mut state, bob, "settlement remove town");
    assert_eq!(state.settlements.find("town").unwrap().id, id);
    assert!(state.settlements.find("stolen").is_none());

    state.outbox.clear();
    command(&mut state, bob, "settlement list");
    command(&mut state, bob, "settlement info town");
    assert!(
        !state.outbox[&ConnectionId(2)].is_empty(),
        "reads are public"
    );
    command(&mut state, alice, &format!("settlement unbind {uuid}"));
    assert_eq!(crate::settlements::member_count(&state, id), 0);
    command(&mut state, alice, &format!("settlement bind town {uuid}"));
    command(&mut state, alice, "settlement rename town borough");
    assert_eq!(state.settlements.find("borough").unwrap().id, id);
    state.plugin_players.observe_permission(ConnectionId(2), 2);
    command(&mut state, bob, "settlement rename borough city");
    assert_eq!(state.settlements.find("city").unwrap().id, id);
    command(&mut state, bob, "settlement remove city");
    assert!(state.settlements.get(id).is_none());
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .settlement,
        None
    );
    assert_eq!(state.entities.body(slot).id, resident);
    assert_eq!(state.entities.row(slot).uuid, uuid);
    assert_eq!(
        state.entities.len(),
        bodies,
        "binding never creates a replacement NPC"
    );
}
