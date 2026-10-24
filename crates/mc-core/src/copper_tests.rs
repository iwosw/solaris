use super::*;
use crate::boundary::{ChunkPos, ConnectionId, Hand, PlayerInput};
use crate::core::{checkpoint_data_for, restore_entity};
use crate::journal::{
    EntityStateImage, JournalEntry, JournalWriter, Replay, ReplayEntity,
    read_complete_batches_after, replay_batches,
};
use crate::registry::CoreRegistry;
use crate::special_mobs::Retained;
use crate::tick::{BlockEntityState, run_tick};
use crate::world::ChunkWorld;
use mc_data::ItemStack;
use mc_nbt::Tag;
use std::sync::{Arc, LazyLock};

const A: BlockPos = BlockPos { x: 1, y: 201, z: 1 };
const B: BlockPos = BlockPos { x: 2, y: 201, z: 1 };
struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            BlockStateId(self.0),
            Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}
fn registry() -> Arc<CoreRegistry> {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    Arc::clone(&REGISTRY)
}
fn fixture(dir: Option<std::path::PathBuf>) -> TickState {
    let registry = registry();
    let generator = Arc::new(Empty(registry.states.air));
    let mut world = if let Some(dir) = dir {
        ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&crate::CoreConfig::new(dir, 42))
                .unwrap(),
            0,
            generator,
            &registry,
            BlockStateId(registry.states.air),
            [1, 201, 3],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            42,
            generator,
            &registry,
            BlockStateId(registry.states.air),
            [1, 201, 3],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    for x in 0..10 {
        for z in 0..7 {
            world.set_block(x, 200, z, registry.states.stone);
        }
    }
    world.fold_overlay();
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .tickets
        .insert("copper-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    state
}
fn block(state: &TickState, path: &str, props: &[(&str, &str)]) -> u32 {
    let name = Identifier::parse(format!("minecraft:{path}")).unwrap();
    let default = state.registry.blocks.block(&name).unwrap().default;
    let mut values = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (name, value) in props {
        values.iter_mut().find(|(key, _)| key == name).unwrap().1 = (*value).into();
    }
    state
        .registry
        .blocks
        .by_name_and_props(&name, &values)
        .unwrap()
        .0
}
fn install(state: &mut TickState, pos: BlockPos, path: &str, kind: &str, items: Vec<ItemStack>) {
    let id = block(state, path, &[("facing", "north"), ("type", kind)]);
    state.world.set_block(pos.x, pos.y, pos.z, id);
    state.world.fold_overlay();
    let mut slots = vec![ItemStack::EMPTY; 27];
    slots[..items.len()].clone_from_slice(&items);
    state.block_entities.insert(
        (pos.x, pos.y, pos.z),
        BlockEntityState {
            slots,
            furnace: None,
        },
    );
    state.world.mark_block_entity_dirty(pos);
}
fn stack(path: &str, count: i32) -> ItemStack {
    ItemStack::new(
        registry()
            .items
            .id_of(&Identifier::parse(format!("minecraft:{path}")).unwrap())
            .unwrap(),
        count,
    )
}
fn at(state: &TickState, pos: BlockPos) -> &mc_world::BlockState {
    state
        .registry
        .blocks
        .by_id(BlockStateId(
            state.world.block_at(pos.x, pos.y, pos.z).unwrap(),
        ))
        .unwrap()
}
fn join(state: &mut TickState) -> EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "copper".into(),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [1.5, 201.0, 3.5]);
    player
}
fn step(state: &mut TickState) -> (u64, Vec<JournalEntry>) {
    let (output, _, _) = run_tick(state, &[]);
    (
        output.tick,
        crate::core::journal_entries_for_tick(state, &output),
    )
}
fn golem(state: &mut TickState, pos: [f64; 3]) -> EntityId {
    let kind = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:copper_golem").unwrap())
        .unwrap();
    state.spawn_mob(kind, pos, 12.0)
}

#[test]
fn copper_eight_chest_and_statue_variants_preserve_all_properties() {
    let state = fixture(None);
    for family_name in ["copper_chest", "copper_golem_statue"] {
        let initial = block(
            &state,
            family_name,
            &[("facing", "east"), ("waterlogged", "true")],
        );
        for age in 0..4 {
            for waxed in [false, true] {
                let id = transform(&state.registry.blocks, initial, age, waxed).unwrap();
                let result = state.registry.blocks.by_id(BlockStateId(id)).unwrap();
                assert_eq!(
                    family(result.block.id.path()),
                    Some((family_name, age, waxed))
                );
                assert_eq!(
                    result.properties,
                    state
                        .registry
                        .blocks
                        .by_id(BlockStateId(initial))
                        .unwrap()
                        .properties
                );
            }
        }
    }
}

#[test]
fn copper_single_and_mixed_double_use_native_menus_and_comparator() {
    let mut state = fixture(None);
    let player = join(&mut state);
    for age in 0..4 {
        for waxed in [false, true] {
            let name = name("copper_chest", age, waxed);
            install(
                &mut state,
                A,
                name.strip_prefix("minecraft:").unwrap(),
                "single",
                vec![stack("stone", 64); 27],
            );
            run_tick(
                &mut state,
                &[PlayerInput::UseItemOn {
                    connection: ConnectionId(1),
                    hand: Hand::Main,
                    position: A,
                    face: 1,
                    cursor: [0.5; 3],
                    sequence: 1,
                }],
            );
            assert_eq!(
                state.players[&player]
                    .open
                    .as_ref()
                    .unwrap()
                    .kind
                    .container_size(),
                27
            );
            let comparator = BlockPos { z: A.z + 1, ..A };
            let id = block(&state, "comparator", &[("facing", "north")]);
            state
                .world
                .set_block(comparator.x, comparator.y, comparator.z, id);
            // Seed the native block-update owner; merge clears its scratch
            // write vector, so preloading that vector is not a placement.
            crate::redstone::run(&mut state, &mut vec![(comparator, id)], &mut Vec::new());
            for _ in 0..4 {
                step(&mut state);
            }
            assert!(
                at(&state, comparator)
                    .properties
                    .iter()
                    .any(|(key, value)| key == "powered" && value == "true")
            );
            assert_eq!(
                crate::redstone::output(
                    &state,
                    comparator,
                    BlockPos {
                        z: comparator.z + 1,
                        ..comparator
                    }
                ),
                15
            );
        }
    }
    install(
        &mut state,
        A,
        "copper_chest",
        "left",
        vec![stack("stone", 3)],
    );
    install(
        &mut state,
        B,
        "waxed_oxidized_copper_chest",
        "right",
        vec![stack("iron_ingot", 5)],
    );
    let cells = crate::chests::storage_cells(&state.registry.blocks, A, &|p| {
        state.world.block_at(p.x, p.y, p.z)
    });
    assert_eq!(cells.len(), 2);
    run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: A,
            face: 1,
            cursor: [0.5; 3],
            sequence: 2,
        }],
    );
    assert_eq!(
        state.players[&player]
            .open
            .as_ref()
            .unwrap()
            .kind
            .container_size(),
        54
    );
}

#[test]
fn copper_paired_weathering_wax_and_offhand_scrape_keep_inventory() {
    let mut state = fixture(None);
    let player = join(&mut state);
    install(
        &mut state,
        A,
        "weathered_copper_chest",
        "left",
        vec![stack("diamond", 9)],
    );
    install(
        &mut state,
        B,
        "weathered_copper_chest",
        "right",
        vec![stack("iron_ingot", 11)],
    );
    let before = state.block_entities.clone();
    let writes = random_tick(&state, A, at(&state, A).id.0, 0).unwrap();
    assert_eq!(writes.len(), 2);
    for (pos, id) in writes {
        state.world.set_block(pos.x, pos.y, pos.z, id);
    }
    state.players.get_mut(&player).unwrap().inventory.slots[0] = stack("honeycomb", 2);
    assert!(use_on(&mut state, player, Hand::Main, A));
    assert!(at(&state, A).block.id.path().starts_with("waxed_"));
    assert!(at(&state, B).block.id.path().starts_with("waxed_"));
    assert!(random_tick(&state, A, at(&state, A).id.0, 0).is_none());
    state.players.get_mut(&player).unwrap().inventory.slots[crate::inventory::OFF_HAND] =
        stack("iron_axe", 1);
    assert!(use_on(&mut state, player, Hand::Off, A));
    assert_eq!(family(at(&state, A).block.id.path()).unwrap().1, 3);
    assert!(use_on(&mut state, player, Hand::Off, B));
    assert_eq!(family(at(&state, A).block.id.path()).unwrap().1, 2);
    assert_eq!(
        state.players[&player].inventory.slots[crate::inventory::OFF_HAND].damage,
        Some(2)
    );
    assert_eq!(state.players[&player].inventory.slots[0].damage, None);
    state.world.fold_overlay();
    for (key, record) in before {
        assert_eq!(state.block_entities[&key].slots, record.slots);
    }
}

#[test]
fn copper_build_pattern_replaces_body_with_matching_chest_exactly_once() {
    for age in 0..4 {
        for waxed in [false, true] {
            let mut state = fixture(None);
            let body_name = [
                "copper_block",
                "exposed_copper",
                "weathered_copper",
                "oxidized_copper",
            ][age];
            let body_name = if waxed {
                format!("waxed_{body_name}")
            } else {
                body_name.into()
            };
            let body = block(&state, &body_name, &[]);
            let pumpkin = BlockPos { y: A.y + 1, ..A };
            let head = block(&state, "carved_pumpkin", &[("facing", "east")]);
            state.world.set_block(A.x, A.y, A.z, body);
            state.world.set_block(pumpkin.x, pumpkin.y, pumpkin.z, head);
            let mut writes = vec![(pumpkin, head)];
            crate::copper_golem::after_blocks(&mut state, &mut writes);
            crate::copper_golem::after_blocks(&mut state, &mut writes);
            assert_eq!(
                family(at(&state, A).block.id.path()),
                Some(("copper_chest", age as u8, waxed))
            );
            assert_eq!(
                state.world.block_at(pumpkin.x, pumpkin.y, pumpkin.z),
                Some(state.registry.states.air)
            );
            let found: Vec<_> = state
                .entities
                .slots()
                .filter(|slot| {
                    matches!(
                        state.entities.row(*slot).special_mob.as_deref(),
                        Some(Retained::Copper(_))
                    )
                })
                .collect();
            assert_eq!(found.len(), 1);
            assert_eq!(
                state.entities.row(found[0]).special_mob.as_deref().cloned(),
                Some(Retained::Copper(crate::copper_golem::CopperState {
                    weather: age as u8,
                    next_weather: if waxed { -2 } else { -1 }
                }))
            );
        }
    }
}

#[test]
fn copper_golem_walks_and_sorts_source_into_matching_or_empty_chest() {
    let mut state = fixture(None);
    let target = BlockPos { x: 6, ..A };
    install(
        &mut state,
        A,
        "copper_chest",
        "single",
        vec![stack("iron_ingot", 23)],
    );
    install(&mut state, target, "chest", "single", vec![]);
    let id = golem(&mut state, [2.5, 201.0, 1.5]);
    for tick in 0..180 {
        state.tick = tick;
        crate::copper_golem::tick(&mut state, &mut Vec::new());
    }
    assert!(
        state.block_entities[&(A.x, A.y, A.z)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    assert_eq!(
        state.block_entities[&(target.x, target.y, target.z)]
            .slots
            .iter()
            .map(|s| s.count)
            .sum::<i32>(),
        23
    );
    assert!(
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .appearance
            .equipment[0]
            .is_none()
    );
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .loot
            .drop_chances[0],
        crate::entity_loot::EntityLootState::default().drop_chances[0]
    );
}

#[test]
fn copper_oxidation_statue_keeps_facing_pose_name_and_checkpoint() {
    let dir = std::env::temp_dir().join(format!("copper-statue-{}", uuid::Uuid::new_v4()));
    let mut state = fixture(Some(dir.clone()));
    let id = golem(&mut state, [1.5, 201.0, 1.5]);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_rot(slot, 90.0, 0.0);
    state.entities.row_mut(slot).special_mob = Some(Box::new(Retained::Copper(
        crate::copper_golem::CopperState {
            weather: 2,
            next_weather: 0,
        },
    )));
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name = Some("Sorter".into());
    let mut writes = Vec::new();
    crate::copper_golem::tick(&mut state, &mut writes);
    assert!(state.entities.slot_of(id).is_none());
    let statue = at(&state, A);
    assert_eq!(statue.block.id.path(), "oxidized_copper_golem_statue");
    assert!(
        statue
            .properties
            .iter()
            .any(|(key, value)| key == "facing" && value == "west")
    );
    assert!(
        statue
            .properties
            .iter()
            .any(|(key, value)| key == "copper_golem_pose"
                && value == POSES[(id.0.unsigned_abs() % 4) as usize])
    );
    let expected = state
        .world
        .chunk(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities[&A]
        .clone();
    let (_, _, inventory) = checkpoint_data_for(&state, state.tick);
    state.world.capture_dirty(&inventory).write().unwrap();
    drop(state);
    let mut restored = fixture(Some(dir.clone()));
    assert_eq!(
        at(&restored, A).block.id.path(),
        "oxidized_copper_golem_statue"
    );
    assert_eq!(
        restored
            .world
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities[&A],
        expected
    );
    let player = join(&mut restored);
    restored.players.get_mut(&player).unwrap().inventory.slots[0] = stack("iron_axe", 1);
    for _ in 0..4 {
        assert!(use_on(&mut restored, player, Hand::Main, A));
    }
    assert_eq!(
        restored.world.block_at(A.x, A.y, A.z),
        Some(restored.registry.states.air)
    );
    let revived = restored
        .entities
        .slots()
        .find(|slot| {
            matches!(
                restored.entities.row(*slot).special_mob.as_deref(),
                Some(Retained::Copper(_))
            )
        })
        .unwrap();
    assert_eq!(
        restored
            .entities
            .row(revived)
            .appearance
            .custom_name
            .as_deref(),
        Some("Sorter")
    );
    assert_eq!(restored.entities.rot(revived)[0], 90.0);
    assert_eq!(restored.players[&player].inventory.slots[0].damage, Some(4));
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copper_native_fields_are_exact_and_corruption_fails_closed() {
    use crate::copper_golem::{CopperState, from_fields, to_fields};
    for weather in 0..4 {
        for next_weather in [-2, -1, 0, 123456] {
            let state = CopperState {
                weather,
                next_weather,
            };
            let fields = to_fields(&state);
            assert_eq!(
                fields[0],
                (
                    "weather_state".into(),
                    Tag::String(AGES[weather as usize].into())
                )
            );
            assert_eq!(
                fields[1],
                ("next_weather_age".into(), Tag::Long(next_weather))
            );
            assert_eq!(from_fields(&fields).unwrap(), state);
        }
    }
    assert_eq!(
        from_fields(&[("weather_state".into(), Tag::Int(2))]).unwrap_err(),
        "Copper.weather_state: expected string"
    );
    assert_eq!(
        from_fields(&[("next_weather_age".into(), Tag::Long(-3))]).unwrap_err(),
        "Copper.next_weather: expected -2, -1 or nonnegative game time"
    );
    assert_eq!(
        from_fields(&[("weather_state".into(), Tag::String("bogus".into()))]).unwrap_err(),
        "Copper.weather_state: expected unaffected, exposed, weathered or oxidized"
    );
    assert_eq!(
        from_fields(&[
            ("weather_state".into(), Tag::String("exposed".into())),
            ("weather_state".into(), Tag::String("exposed".into()))
        ])
        .unwrap_err(),
        "Copper.weather_state: duplicate field"
    );
}

#[test]
fn copper_antenna_shear_offhand_and_spawn_egg_have_single_custody() {
    let mut state = fixture(None);
    let player = join(&mut state);
    let id = golem(&mut state, [1.5, 201.0, 1.5]);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = stack("poppy", 2);
    assert!(crate::copper_golem::interact(
        &mut state,
        player,
        id,
        Hand::Main
    ));
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(
        state.entities.row(slot).loot.saddle.as_ref().unwrap().count,
        1
    );
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
    state.players.get_mut(&player).unwrap().inventory.slots[crate::inventory::OFF_HAND] =
        stack("shears", 1);
    assert!(crate::copper_golem::interact(
        &mut state,
        player,
        id,
        Hand::Off
    ));
    assert!(state.entities.row(slot).loot.saddle.is_none());
    assert!(state.entities.row(slot).appearance.saddle.is_none());
    assert_eq!(
        state
            .merge_life_drops
            .iter()
            .filter(|(_, s)| s.item_id == stack("poppy", 1).item_id)
            .count(),
        1
    );
    assert_eq!(
        state.players[&player].inventory.slots[crate::inventory::OFF_HAND].damage,
        Some(1)
    );
    state.players.get_mut(&player).unwrap().inventory.slots[0] = stack("copper_golem_spawn_egg", 2);
    assert!(crate::spawn_egg::use_on(
        &mut state,
        player,
        Hand::Main,
        B,
        1
    ));
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
    assert_eq!(
        state
            .entities
            .slots()
            .filter(|slot| matches!(
                state.entities.row(*slot).special_mob.as_deref(),
                Some(Retained::Copper(_))
            ))
            .count(),
        2
    );
}

fn native_record(state: &TickState, native: &mc_data::entity_nbt::VanillaEntity) -> ReplayEntity {
    let image = EntityStateImage::from_record(&state.registry, native).unwrap();
    ReplayEntity {
        uuid: native.uuid.to_string(),
        type_id: state
            .registry
            .entity_types
            .id_of(&Identifier::parse(&native.id).unwrap())
            .unwrap(),
        pos: native.pos,
        vel: native.motion.map(|component| component * 20.0),
        yaw: native.yaw,
        pitch: native.pitch,
        ground: native.on_ground,
        health: native.health,
        alive: true,
        age: image.age,
        breed_cooldown: image.breed_cooldown,
        pickup_delay: native.pickup_delay as u32,
        item: native.item.clone(),
        appearance: crate::journal::EntityAppearanceImage {
            health: native.health,
            equipment: crate::entity_loot::equipment_from_fields(
                &state.registry.items,
                &native.extra_fields,
            )
            .unwrap()
            .to_vec(),
            ..Default::default()
        },
        active_effects: Default::default(),
        villager: None,
        state: image,
        projectile: None,
        tick: 1,
    }
}
#[test]
fn copper_wal_replay_and_native_checkpoint_keep_carried_custody_once() {
    let dir = std::env::temp_dir().join(format!("copper-replay-{}", uuid::Uuid::new_v4()));
    let mut state = fixture(Some(dir.clone()));
    install(
        &mut state,
        A,
        "waxed_copper_chest",
        "single",
        vec![stack("diamond", 16)],
    );
    let id = golem(&mut state, [2.5, 201.0, 1.5]);
    crate::copper_golem::tick(&mut state, &mut Vec::new());
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(
        state.entities.row(slot).appearance.equipment[0]
            .as_ref()
            .unwrap()
            .count,
        16
    );
    let (tick, entries) = step(&mut state);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(tick, tick, &entries).unwrap();
    writer.gate(0, tick).unwrap();
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    drop(writer);
    let once = replay_batches(Replay::default(), &batches);
    let expected_entities = serde_json::to_value(&once.entities).unwrap();
    let expected_inventories = once.block_entities.clone();
    let twice = replay_batches(once, &batches);
    assert_eq!(
        expected_entities,
        serde_json::to_value(&twice.entities).unwrap()
    );
    assert_eq!(expected_inventories, twice.block_entities);
    let (_, regions, inventories) = checkpoint_data_for(&state, state.tick);
    let store =
        crate::native_checkpoint::tests::fixture_store(&crate::CoreConfig::new(dir.clone(), 42))
            .unwrap();
    let mut image = store.read_checkpoint().unwrap();
    let checkpoint_tick = image.tick + 1;
    let entities = crate::native_checkpoint::encode_entities(
        crate::dimensions::Dimension::Overworld,
        &regions,
        &state.registry.items,
        checkpoint_tick,
    )
    .unwrap();
    let capture = state.world.capture_dirty(&inventories);
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.tick = checkpoint_tick;
    for child in manifest.dimensions.values_mut() {
        child.tick = checkpoint_tick;
    }
    image.globals.insert(
        "checkpoint".into(),
        crate::native_checkpoint::record(checkpoint_tick, serde_json::to_vec(&manifest).unwrap()),
    );
    for global in image.globals.values_mut() {
        global.captured_tick = checkpoint_tick;
    }
    let entity_iter = entities.iter().map(Ok);
    store
        .commit_checkpoint_stream(
            mc_world::native::NativeCheckpoint {
                tick: checkpoint_tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &image.globals,
            },
            |write| {
                capture
                    .visit_native(|snapshot| {
                        write(snapshot)
                            .map_err(|error| crate::config::CoreError::Storage(error.to_string()))
                    })
                    .map_err(|error| mc_world::native::NativeError::Source(error.to_string()))
            },
            entity_iter,
            &state.registry.blocks,
            &state.registry.items,
        )
        .unwrap();
    let loaded = store.read_checkpoint().unwrap().entities;
    drop(state);
    let mut restored = fixture(Some(dir.clone()));
    for snapshot in loaded {
        let Some(Tag::Compound(fields)) = snapshot.payload else {
            panic!("missing copper entity payload")
        };
        let native = mc_data::entity_nbt::entity_from_fields(
            &restored.registry.items,
            Default::default(),
            &fields,
        )
        .unwrap();
        let record = native_record(&restored, &native);
        restore_entity(&mut restored, native.wire_id, &record).unwrap();
    }
    step(&mut restored);
    assert!(
        restored.block_entities[&(A.x, A.y, A.z)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    let slot = restored.entities.slot_of(id).unwrap();
    assert_eq!(
        restored.entities.row(slot).appearance.equipment[0]
            .as_ref()
            .unwrap(),
        &stack("diamond", 16)
    );
    assert!(matches!(
        restored.entities.row(slot).special_mob.as_deref(),
        Some(Retained::Copper(_))
    ));
    for (&wire, record) in &twice.entities {
        if let Some(slot) = restored.entities.slot_of(EntityId(wire)) {
            restored.remove_entity(slot);
        }
        restore_entity(&mut restored, wire, record).unwrap();
    }
    for (key, record) in twice.block_entities {
        restored.block_entities.insert(
            key,
            BlockEntityState {
                slots: record.slots,
                furnace: None,
            },
        );
    }
    assert_eq!(
        restored
            .entities
            .row(restored.entities.slot_of(id).unwrap())
            .appearance
            .equipment[0]
            .as_ref()
            .unwrap()
            .count,
        16
    );
    assert_eq!(
        restored
            .entities
            .slots()
            .filter(|slot| matches!(
                restored.entities.row(*slot).special_mob.as_deref(),
                Some(Retained::Copper(_))
            ))
            .count(),
        1
    );
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn copper_native_chest_metadata_and_seeded_loot_roundtrip_once() {
    let mut state = fixture(None);
    install(&mut state, A, "exposed_copper_chest", "single", vec![]);
    let mut chunk = state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap().clone();
    let mut fields = vec![
        ("id".into(), Tag::String("minecraft:chest".into())),
        ("x".into(), Tag::Int(A.x)),
        ("y".into(), Tag::Int(A.y)),
        ("z".into(), Tag::Int(A.z)),
        ("CustomName".into(), Tag::String("Copper custody".into())),
        (
            "Lock".into(),
            Tag::Compound(vec![("components".into(), Tag::Compound(vec![]))]),
        ),
        (
            "LootTable".into(),
            Tag::String("minecraft:chests/simple_dungeon".into()),
        ),
        ("LootTableSeed".into(), Tag::Long(123)),
    ];
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields.clone())).unwrap();
    chunk.block_entities.insert(A, bytes);
    let roundtrip = |chunk: &mc_world::Chunk| {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::CoreConfig::new(directory.path().to_owned(), 42);
        let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
        store
            .import_chunk(0, chunk, &state.registry.blocks, &state.registry.items)
            .unwrap();
        store
            .read_chunk(
                0,
                chunk.pos,
                &state.registry.blocks,
                &state.registry.items,
                &state.registry.chest_loot,
                &Empty(state.registry.states.air),
            )
            .map(|chunk| chunk.unwrap())
    };
    let imported = roundtrip(&chunk).unwrap();
    assert!(imported.chests[&A].loot_table.is_none());
    assert!(
        imported.chests[&A]
            .slots
            .iter()
            .any(|slot| !slot.is_empty())
    );
    let twice = roundtrip(&imported).unwrap();
    assert_eq!(imported.chests[&A], twice.chests[&A]);
    let native =
        mc_nbt::read_network(&mut std::io::Cursor::new(&twice.block_entities[&A])).unwrap();
    let Tag::Compound(saved) = native else {
        panic!("chest compound")
    };
    assert!(
        saved
            .iter()
            .any(|(key, value)| key == "CustomName"
                && value == &Tag::String("Copper custody".into()))
    );
    assert!(saved.iter().any(|(key, _)| key == "Lock"));
    assert!(!saved.iter().any(|(key, _)| key == "LootTable"));
    fields
        .iter_mut()
        .find(|(key, _)| key == "LootTable")
        .unwrap()
        .1 = Tag::String("minecraft:chests/not_supported".into());
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).unwrap();
    chunk.block_entities.insert(A, bytes);
    assert!(roundtrip(&chunk).unwrap_err().to_string().contains(
        "copper chest LootTable minecraft:chests/not_supported: unsupported saved container table"
    ));
}

#[test]
fn copper_carried_items_drop_on_non_player_death_once_after_double_wal_replay() {
    let mut state = fixture(None);
    let cargo = stack("diamond", 16);
    install(&mut state, A, "copper_chest", "single", vec![cargo.clone()]);
    let id = golem(&mut state, [2.5, 201.0, 1.5]);
    crate::copper_golem::tick(&mut state, &mut Vec::new());
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(
        state.entities.row(slot).appearance.equipment[0],
        Some(cargo.clone())
    );
    assert_eq!(state.entities.row(slot).loot.drop_chances[0], 2.0);
    let (carry_tick, carry_entries) = step(&mut state);
    assert_eq!(carry_tick, 1);
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: crate::survival::DamageKind::Void,
            source: None,
        },
    ));
    let (output, _, _) = run_tick(&mut state, &[]);
    assert_eq!(output.tick, 2);
    assert!(output.changes.entity_events.iter().any(|event| {
        matches!(event, crate::boundary::EntityEvent::Death { entity } if *entity == id)
    }));
    let death_entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(death_entries.iter().any(|entry| match entry {
        JournalEntry::Entity {
            id: dead,
            alive: false,
            ..
        }
        | JournalEntry::EntityVitals {
            id: dead,
            alive: false,
            ..
        }
        | JournalEntry::RemoveEntity { id: dead, .. } => *dead == id.0,
        _ => false,
    }));
    let cargo_drops: Vec<_> = output
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| {
            state
                .entities
                .row(slot)
                .item
                .as_ref()
                .map(|item| item.full_stack())
        })
        .filter(|stack| stack.item_id == cargo.item_id)
        .collect();
    assert_eq!(cargo_drops, vec![cargo.clone()]);
    assert!(
        state.block_entities[&(A.x, A.y, A.z)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    let dir = std::env::temp_dir().join(format!("copper-death-replay-{}", uuid::Uuid::new_v4()));
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, 1, &carry_entries).unwrap();
    writer.append(2, 2, &death_entries).unwrap();
    writer.gate(0, 2).unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let once = replay_batches(Replay::default(), &batches);
    assert!(!once.entities.contains_key(&id.0));
    let expected = serde_json::to_value(&once.entities).unwrap();
    let twice = replay_batches(once, &batches);
    assert!(!twice.entities.contains_key(&id.0));
    assert_eq!(expected, serde_json::to_value(&twice.entities).unwrap());
    let mut restored = fixture(None);
    restored.tick = 2;
    for (&id, image) in &twice.entities {
        restore_entity(&mut restored, id, image).unwrap();
    }
    assert!(restored.entities.slot_of_any(id).is_none());
    let restored_cargo: Vec<_> = restored
        .entities
        .slots()
        .filter(|slot| restored.entities.body(*slot).alive)
        .filter_map(|slot| {
            restored
                .entities
                .row(slot)
                .item
                .as_ref()
                .map(|item| item.full_stack())
        })
        .filter(|stack| stack.item_id == cargo.item_id)
        .collect();
    assert_eq!(restored_cargo, vec![cargo]);
    drop(state);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn paired_copper_waxing_checks_real_consumption_before_either_block_changes() {
    let mut state = fixture(None);
    let actor = join(&mut state);
    install(
        &mut state,
        A,
        "weathered_copper_chest",
        "left",
        vec![stack("diamond", 9)],
    );
    install(
        &mut state,
        B,
        "weathered_copper_chest",
        "right",
        vec![stack("iron_ingot", 11)],
    );
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack("honeycomb", 2);
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "honeycomb",
        2,
    );
    let blocks = [
        state.world.block_at(A.x, A.y, A.z),
        state.world.block_at(B.x, B.y, B.z),
    ];
    let custody = state.block_entities.clone();
    let writes = state.merge_block_writes.len();
    assert!(use_on(&mut state, actor, Hand::Main, A));
    assert_eq!(
        [
            state.world.block_at(A.x, A.y, A.z),
            state.world.block_at(B.x, B.y, B.z)
        ],
        blocks
    );
    for (key, record) in &custody {
        assert_eq!(state.block_entities[key].slots, record.slots);
    }
    assert_eq!(state.merge_block_writes.len(), writes);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0].count = 3;
    assert!(use_on(&mut state, actor, Hand::Main, A));
    assert!(at(&state, A).block.id.path().starts_with("waxed_"));
    assert!(at(&state, B).block.id.path().starts_with("waxed_"));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
    for (key, record) in &custody {
        assert_eq!(state.block_entities[key].slots, record.slots);
    }
}

#[test]
fn copper_unbreaking_admission_preserves_denied_rng_and_applies_one_allowed_roll() {
    let mut state = fixture(None);
    let actor = join(&mut state);
    install(&mut state, A, "waxed_copper_chest", "single", vec![]);
    let waxed = state.world.block_at(A.x, A.y, A.z).unwrap();
    let mut axe = stack("iron_axe", 1);
    let max = state
        .registry
        .item_facts
        .facts_for_stack(&axe, &state.registry.items)
        .unwrap()
        .max_damage
        .unwrap();
    axe.damage = Some(max as i32 - 1);
    axe.enchantments.push(mc_data::ItemEnchantment {
        id: Identifier::parse("minecraft:unbreaking").unwrap(),
        level: 3,
    });
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = axe.clone();
    let player_id = state.players[&actor].connection.0;
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id },
        "iron_axe",
        1,
    );
    let breaking = (0..1024)
        .find(|seed| {
            let mut seed = *seed;
            crate::enchantments::durability_damage(&axe, 1, false, &mut seed) == 1
        })
        .unwrap();
    state.players.get_mut(&actor).unwrap().enchantment_seed = breaking;
    let writes = state.merge_block_writes.len();
    assert!(use_on(&mut state, actor, Hand::Main, A));
    assert_eq!(state.world.block_at(A.x, A.y, A.z), Some(waxed));
    assert_eq!(state.players[&actor].inventory.slots[0], axe);
    assert_eq!(state.players[&actor].enchantment_seed, breaking);
    assert_eq!(state.merge_block_writes.len(), writes);
    let skipping = (0..1024)
        .find(|seed| {
            let mut seed = *seed;
            crate::enchantments::durability_damage(&axe, 1, false, &mut seed) == 0
        })
        .unwrap();
    let mut expected_seed = skipping;
    assert_eq!(
        crate::enchantments::durability_damage(&axe, 1, false, &mut expected_seed),
        0
    );
    state.players.get_mut(&actor).unwrap().enchantment_seed = skipping;
    assert!(use_on(&mut state, actor, Hand::Main, A));
    assert_eq!(at(&state, A).block.id.path(), "copper_chest");
    assert_eq!(state.players[&actor].inventory.slots[0], axe);
    assert_eq!(state.players[&actor].enchantment_seed, expected_seed);
}

#[test]
fn creative_copper_waxing_has_no_debit_and_can_use_reserved_honeycomb() {
    let mut state = fixture(None);
    let actor = join(&mut state);
    install(&mut state, A, "copper_chest", "single", vec![]);
    let player = state.players.get_mut(&actor).unwrap();
    player.game_mode = crate::commands::GameMode::Creative;
    player.inventory.slots[0] = stack("honeycomb", 1);
    let player_id = player.connection.0;
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id },
        "honeycomb",
        1,
    );
    assert!(use_on(&mut state, actor, Hand::Main, A));
    assert_eq!(at(&state, A).block.id.path(), "waxed_copper_chest");
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        stack("honeycomb", 1)
    );
}

#[test]
fn all_weathering_families_oxidize_wax_and_scrape() {
    // COPPER-WEATHERING group: every full 8-variant family weathers one age
    // on its random tick, waxes with honeycomb, and scrapes back with an axe.
    let mut state = fixture(None);
    let player = join(&mut state);
    let families = [
        "chiseled_copper",
        "copper_bars",
        "copper_bulb",
        "copper_chain",
        "copper_door",
        "copper_grate",
        "copper_lantern",
        "copper_trapdoor",
        "cut_copper",
        "cut_copper_slab",
        "cut_copper_stairs",
        "lightning_rod",
    ];
    for (i, fam) in families.iter().enumerate() {
        let pos = BlockPos { x: 20 + i as i32, y: 201, z: 1 };
        // Stateless families plant bare; stateful ones need explicit
        // props (registry defaults are attached/active states).
        let bare: &[(&str, &str)] = &[];
        let id = block(
            &state,
            fam,
            match *fam {
                "copper_bars" => &[("east", "false"), ("north", "false"), ("south", "false"), ("waterlogged", "false"), ("west", "false")][..],
                "copper_bulb" => &[("lit", "false"), ("powered", "false")][..],
                "copper_chain" => &[("axis", "y"), ("waterlogged", "false")][..],
                "copper_door" => &[("facing", "north"), ("half", "lower"), ("hinge", "left"), ("open", "false"), ("powered", "false")][..],
                "copper_grate" => &[("waterlogged", "false")][..],
                "copper_lantern" => &[("hanging", "false"), ("waterlogged", "false")][..],
                "copper_trapdoor" => &[("facing", "north"), ("half", "bottom"), ("open", "false"), ("powered", "false"), ("waterlogged", "false")][..],
                "cut_copper_slab" => &[("type", "bottom"), ("waterlogged", "false")][..],
                "cut_copper_stairs" => &[("facing", "north"), ("half", "bottom"), ("shape", "straight"), ("waterlogged", "false")][..],
                "lightning_rod" => &[("facing", "up"), ("powered", "false"), ("waterlogged", "false")][..],
                _ => bare,
            },
        );
        state.world.set_block(pos.x, pos.y, pos.z, id);
        state.world.fold_overlay();
        // Weather one age via the random-tick path (roll 0 always passes).
        let writes = random_tick(&state, pos, at(&state, pos).id.0, 0)
            .unwrap_or_else(|| panic!("{fam} never weathered"));
        assert_eq!(family(at(&state, pos).block.id.path()).unwrap().1, 0);
        for (wpos, wid) in writes {
            state.world.set_block(wpos.x, wpos.y, wpos.z, wid);
        }
        assert_eq!(family(at(&state, pos).block.id.path()).unwrap().1, 1);
        // Wax with honeycomb, then scrape back with an axe.
        state.players.get_mut(&player).unwrap().inventory.slots[0] = stack("honeycomb", 2);
        assert!(use_on(&mut state, player, Hand::Main, pos));
        assert!(family(at(&state, pos).block.id.path()).unwrap().2);
        state.players.get_mut(&player).unwrap().inventory.slots[0] = stack("iron_axe", 1);
        assert!(use_on(&mut state, player, Hand::Main, pos));
        let (name, age, waxed) = family(at(&state, pos).block.id.path()).unwrap();
        assert!(!waxed && age == 1, "{fam} scraped to unwaxed age 1, got {name}");
    }
}
