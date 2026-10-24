use super::*;
use crate::boundary::{ChunkPos, ConnectionId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::world::ChunkWorld;
use std::sync::{Arc, LazyLock};

pub(crate) const POS: BlockPos = BlockPos { x: 4, y: 201, z: 4 };
pub(crate) fn state(dir: Option<std::path::PathBuf>) -> TickState {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let mut world = match dir {
        Some(directory) => ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&crate::config::CoreConfig::new(
                directory, 42,
            ))
            .unwrap(),
            0,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [4, 201, 6],
            mc_world::OVERWORLD_GEOMETRY,
        ),
        None => ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [4, 201, 6],
            mc_world::OVERWORLD_GEOMETRY,
        ),
    };
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 512);
    state.day_time = 1000;
    // Strict radius split: fixture intent carries a ticket; activation flows
    // through the same refresh run_tick uses (tickets simulate, Main binding).
    state
        .tickets
        .insert("bees-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    state
}
pub(crate) fn place(state: &mut TickState, p: BlockPos, name: &str) {
    let id = state
        .registry
        .blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(p.x, p.y, p.z, id);
    state.world.fold_overlay();
}
pub(crate) fn item(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap(),
        count,
    )
}
pub(crate) fn joined(state: &mut TickState) -> EntityId {
    crate::tick::run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "beekeeper".into(),
            permission_level: 0,
        }],
    );
    let id = state.connections.entity_of(ConnectionId(1)).unwrap();
    state
        .entities
        .set_pos(state.entities.slot_of(id).unwrap(), [4.5, 201.0, 6.5]);
    id
}
pub(crate) fn mob(state: &mut TickState, name: &str, p: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(name).unwrap())
        .unwrap();
    state.spawn_mob(type_id, p, 20.0)
}
#[test]
fn native_hive_keys_round_trip_and_corruption_fails_closed() {
    let mut data = to_fields(&BeeState {
        nectar: true,
        anger_end: -1,
        ..Default::default()
    });
    data.push(("id".into(), Tag::String("minecraft:bee".into())));
    let hive = Hive {
        bees: vec![Occupant {
            data,
            ticks: 137,
            minimum: 2400,
        }],
        flower: Some(POS),
        pending_placement: false,
    };
    let bytes = hive.encode(POS);
    assert_eq!(Hive::decode(&bytes).unwrap(), hive);
    let mut bad = hive.clone();
    bad.bees[0].ticks = -1;
    assert!(
        Hive::decode(&bad.encode(POS))
            .unwrap_err()
            .to_string()
            .contains("ticks_in_hive: expected nonnegative int")
    );
    let mut bytes = bytes;
    bytes.push(0);
    assert!(
        Hive::decode(&bytes)
            .unwrap_err()
            .to_string()
            .contains("Beehive: trailing NBT bytes")
    );
}
#[test]
fn pollination_enters_hive_then_exits_once_with_honey() {
    let mut state = state(None);
    place(&mut state, POS, "minecraft:beehive");
    let flower = BlockPos { x: 5, ..POS };
    place(&mut state, flower, "minecraft:poppy");
    let id = mob(&mut state, "minecraft:bee", centre(flower));
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).bee.as_mut().unwrap().hive = Some(POS);
    state.entities.row_mut(slot).bee.as_mut().unwrap().flower = Some(flower);
    let mut changes = ChangeSet::default();
    let mut writes = Vec::new();
    for _ in 0..440 {
        state.tick += 1;
        run(&mut state, &mut writes, &mut changes);
        commit(&mut state, &mut changes);
    }
    assert!(state.entities.slot_of(id).is_none());
    assert_eq!(state.hives[&key(POS)].bees.len(), 1);
    for _ in 0..2401 {
        state.tick += 1;
        run(&mut state, &mut writes, &mut changes);
        commit(&mut state, &mut changes);
    }
    assert_eq!(honey(&state, POS), Some(1));
    assert_eq!(changes.entities_removed, vec![id]);
    assert_eq!(changes.entities_spawned.len(), 1);
}
#[test]
fn hive_checkpoint_preserves_occupation_and_single_release() {
    let dir = std::env::temp_dir().join(format!("bees-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    place(&mut original, POS, "minecraft:bee_nest");
    let mut data = to_fields(&BeeState {
        nectar: true,
        anger_end: -1,
        ..Default::default()
    });
    data.push(("id".into(), Tag::String("minecraft:bee".into())));
    original.hives.insert(
        key(POS),
        Hive {
            bees: vec![Occupant {
                data,
                ticks: 137,
                minimum: 2400,
            }],
            flower: Some(POS),
            pending_placement: false,
        },
    );
    let expected = original.hives[&key(POS)].clone();
    checkpoint(&mut original);
    original
        .world
        .capture_dirty(&BTreeMap::new())
        .write()
        .unwrap();
    drop(original);
    let mut restored = state(Some(dir.clone()));
    hydrate_chunk(
        &mut restored.hives,
        &restored.registry,
        restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.hives[&key(POS)], expected);
    let mut changes = ChangeSet::default();
    let mut writes = Vec::new();
    for _ in 137..2401 {
        restored.tick += 1;
        run(&mut restored, &mut writes, &mut changes);
        commit(&mut restored, &mut changes);
    }
    assert_eq!(honey(&restored, POS), Some(1));
    assert!(restored.hives[&key(POS)].bees.is_empty());
    assert_eq!(changes.entities_spawned.len(), 1);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn bottle_and_shears_harvest_and_smoke_prevents_anger() {
    let mut state = state(None);
    let actor = joined(&mut state);
    place(&mut state, POS, "minecraft:beehive");
    let bee = mob(&mut state, "minecraft:bee", centre(POS));
    let fire = BlockPos {
        y: POS.y - 3,
        ..POS
    };
    place(&mut state, fire, "minecraft:campfire");
    assert!(smoke(&state, POS));
    let next = property(&state, POS, "honey_level", "5").unwrap();
    state.world.set_block(POS.x, POS.y, POS.z, next);
    let bottle = item(&state, "minecraft:glass_bottle", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bottle.clone();
    assert!(use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &bottle,
        &mut ChangeSet::default()
    ));
    assert_eq!(honey(&state, POS), Some(0));
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        item(&state, "minecraft:honey_bottle", 1)
    );
    assert!(
        state
            .entities
            .row(state.entities.slot_of(bee).unwrap())
            .bee
            .as_ref()
            .unwrap()
            .angry_at
            .is_none()
    );
    place(&mut state, fire, "minecraft:air");
    state.world.set_block(POS.x, POS.y, POS.z, next);
    let shears = item(&state, "minecraft:shears", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = shears.clone();
    use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &shears,
        &mut ChangeSet::default(),
    );
    assert!(
        state.players[&actor]
            .inventory
            .slots
            .iter()
            .any(|s| *s == item(&state, "minecraft:honeycomb", 3))
    );
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
    assert!(
        state
            .entities
            .row(state.entities.slot_of(bee).unwrap())
            .bee
            .as_ref()
            .unwrap()
            .angry_at
            .is_some()
    );
}

#[test]
fn nectar_grows_crops_and_sting_poisons_then_bee_dies() {
    let mut state = state(None);
    let actor = joined(&mut state);
    state.difficulty = crate::survival::Difficulty::Normal;
    place(&mut state, POS, "minecraft:wheat");
    let id = mob(&mut state, "minecraft:bee", [4.5, 202.5, 4.5]);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).bee.as_mut().unwrap().nectar = true;
    state.tick = 20;
    run(&mut state, &mut Vec::new(), &mut ChangeSet::default());
    assert!(
        block(&state, POS)
            .unwrap()
            .properties
            .iter()
            .any(|(k, v)| k == "age" && v == "1")
    );
    let victim = state.entities.slot_of(actor).unwrap();
    let source = Some(crate::intent::DamageSource::Melee {
        attacker: id,
        position: state.entities.pos(slot),
        yaw: 0.0,
        bonus_knockback: 0.0,
        sweep: false,
        fire_ticks: 0,
        bane_ticks: 0,
    });
    accepted_hit(&mut state, victim, source);
    assert!(
        state.players[&actor]
            .active_effects
            .has(crate::effects::EffectId::Poison)
    );
    assert!(state.entities.row(slot).bee.as_ref().unwrap().stung);
    state
        .entities
        .row_mut(slot)
        .bee
        .as_mut()
        .unwrap()
        .sting_ticks = 1199;
    let mut changes = ChangeSet::default();
    run(&mut state, &mut Vec::new(), &mut changes);
    commit(&mut state, &mut changes);
    assert!(state.entities.slot_of_any(id).is_none());
    assert_eq!(changes.entities_removed, vec![id]);
}

#[test]
fn wal_hive_entry_tombstones_source_and_replays_one_full_bee() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let dir = std::env::temp_dir().join(format!("bees-wal-{}", uuid::Uuid::new_v4()));
    let mut original = state(None);
    place(&mut original, POS, "minecraft:beehive");
    let id = mob(&mut original, "minecraft:bee", centre(POS));
    let slot = original.entities.slot_of(id).unwrap();
    original.entities.row_mut(slot).controls.no_ai = true;
    let uuid = original.entities.row(slot).uuid;
    let (mut output, _, _) = crate::tick::run_tick(&mut original, &[]);
    Arc::get_mut(&mut output.changes)
        .expect("tick output uniquely owns its changes")
        .entities_spawned
        .push(id);
    let birth = crate::core::journal_entries_for_tick(&mut original, &output);
    let bee = original.entities.row_mut(slot).bee.as_mut().unwrap();
    bee.nectar = true;
    bee.hive = Some(POS);
    let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
    assert!(original.entities.slot_of_any(id).is_none());
    let entry = crate::core::journal_entries_for_tick(&mut original, &output);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, 1, &birth).unwrap();
    writer.append(2, 2, &entry).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let replay = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&dir.join("journal"), 0).unwrap(),
    );
    assert!(!replay.entities.contains_key(&id.0));
    let bytes = replay.opaque_block_entities[&key(POS)].as_ref().unwrap();
    let mut restored = state(None);
    // Recovery resumes the world clock; derived uuids are keyed on it.
    restored.tick = original.tick;
    place(&mut restored, POS, "minecraft:beehive");
    restored
        .world
        .set_opaque_block_entity(POS, Some(bytes.clone()));
    hydrate_chunk(
        &mut restored.hives,
        &restored.registry,
        restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.hives[&key(POS)].bees.len(), 1);
    restored.hives.get_mut(&key(POS)).unwrap().bees[0].ticks = 2400;
    let mut changes = ChangeSet::default();
    run(&mut restored, &mut Vec::new(), &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    let slot = restored
        .entities
        .slot_of(changes.entities_spawned[0])
        .unwrap();
    assert_ne!(restored.entities.row(slot).uuid, uuid);
    assert!(restored.entities.row(slot).controls.no_ai);
    assert_eq!(honey(&restored, POS), Some(1));
    run(&mut restored, &mut Vec::new(), &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn native_elapsed_pollination_timer_is_not_flower_progress() {
    let mut original = state(None);
    let fields = vec![
        ("TicksSincePollination".into(), Tag::Int(1234)),
        ("solaris:BeePollinationTicks".into(), Tag::Int(399)),
        ("flower_pos".into(), pos_tag(POS)),
    ];
    let value = from_fields(&fields).unwrap();
    assert_eq!(value.ticks_since_pollination, 1234);
    assert_eq!(value.pollination, 399);
    assert_eq!(
        field(&to_fields(&value), "TicksSincePollination").unwrap(),
        Some(&Tag::Int(1234))
    );
    place(&mut original, POS, "minecraft:dandelion");
    let id = mob(&mut original, "minecraft:bee", centre(POS));
    let slot = original.entities.slot_of(id).unwrap();
    original.entities.row_mut(slot).bee = Some(Box::new(value));
    original.tick = 1;
    run(&mut original, &mut Vec::new(), &mut ChangeSet::default());
    let value = original.entities.row_mut(slot).bee.as_mut().unwrap();
    assert!(value.nectar);
    assert_eq!(value.pollination, 0);
    assert_eq!(value.ticks_since_pollination, 0);
    value.nectar = false;
    value.flower = None;
    value.pollination = 17;
    value.ticks_since_pollination = 42;
    run(&mut original, &mut Vec::new(), &mut ChangeSet::default());
    let value = original.entities.row(slot).bee.as_ref().unwrap();
    assert_eq!(value.pollination, 17);
    assert_eq!(value.ticks_since_pollination, 43);
    assert_eq!(from_fields(&to_fields(value)).unwrap(), **value);
    let error = from_fields(&[("solaris:BeePollinationTicks".into(), Tag::Int(401))])
        .unwrap_err()
        .to_string();
    assert!(error.contains("Bee.solaris:BeePollinationTicks: expected 0..=400"));
}

#[test]
fn idle_occupied_hive_skips_wal_and_checkpoint_restores_elapsed_ticks() {
    let dir = std::env::temp_dir().join(format!("bees-idle-clock-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    place(&mut original, POS, "minecraft:beehive");
    let mut data = to_fields(&BeeState {
        nectar: true,
        anger_end: -1,
        ..Default::default()
    });
    data.push(("id".into(), Tag::String("minecraft:bee".into())));
    original.hives.insert(
        key(POS),
        Hive {
            bees: vec![Occupant {
                data,
                ticks: 137,
                minimum: 2400,
            }],
            flower: None,
            pending_placement: false,
        },
    );
    for _ in 0..100 {
        let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut original, &output);
        assert!(!entries.iter().any(|entry| matches!(
            entry,
            crate::journal::JournalEntry::OpaqueBlockEntity { pos, .. }
                if *pos == [POS.x, POS.y, POS.z]
        )));
    }
    assert_eq!(original.hives[&key(POS)].bees[0].ticks, 237);
    checkpoint(&mut original);
    original
        .world
        .capture_dirty(&BTreeMap::new())
        .write()
        .unwrap();
    drop(original);
    let mut restored = state(Some(dir.clone()));
    hydrate_chunk(
        &mut restored.hives,
        &restored.registry,
        restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.hives[&key(POS)].bees[0].ticks, 237);
    run(&mut restored, &mut Vec::new(), &mut ChangeSet::default());
    assert_eq!(restored.hives[&key(POS)].bees[0].ticks, 238);
    assert!(restored.opaque_changes.is_empty());
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn failed_hive_and_flower_search_waits_before_rescanning() {
    let mut original = state(None);
    let id = mob(&mut original, "minecraft:bee", centre(POS));
    let slot = original.entities.slot_of(id).unwrap();
    original.tick = 20;
    run(&mut original, &mut Vec::new(), &mut ChangeSet::default());
    let bee = original.entities.row(slot).bee.as_ref().unwrap();
    assert_eq!(bee.search_cooldown_until, 220);
    assert!(bee.hive.is_none());
    assert!(bee.flower.is_none());
    place(&mut original, POS, "minecraft:beehive");
    let flower = BlockPos {
        x: POS.x + 1,
        ..POS
    };
    place(&mut original, flower, "minecraft:dandelion");
    original.tick = 40;
    run(&mut original, &mut Vec::new(), &mut ChangeSet::default());
    let bee = original.entities.row(slot).bee.as_ref().unwrap();
    assert!(bee.hive.is_none());
    assert!(bee.flower.is_none());
    original.tick = 220;
    run(&mut original, &mut Vec::new(), &mut ChangeSet::default());
    let bee = original.entities.row(slot).bee.as_ref().unwrap();
    assert_eq!(bee.hive, Some(POS));
    assert_eq!(bee.flower, Some(flower));
}

fn occupied_hive(state: &mut TickState) -> Occupant {
    place(state, POS, "minecraft:beehive");
    let level = property(state, POS, "honey_level", "4").unwrap();
    state.world.set_block(POS.x, POS.y, POS.z, level);
    let mut data = to_fields(&BeeState {
        nectar: true,
        flower: Some(BlockPos { x: 9, ..POS }),
        ..Default::default()
    });
    data.push(("id".into(), Tag::String("minecraft:bee".into())));
    data.push(("Health".into(), Tag::Float(7.5)));
    let occupant = Occupant {
        data,
        ticks: 137,
        minimum: 2400,
    };
    let mut hive = Hive {
        bees: vec![occupant.clone()],
        ..Default::default()
    };
    // Chunk bytes deliberately lag the authoritative owner's occupation clock.
    hive.bees[0].ticks = 17;
    persist(state, POS, &hive);
    hive.bees[0].ticks = 137;
    state.hives.insert(key(POS), hive);
    occupant
}

fn component(stack: &mut ItemStack, name: &str, tag: Tag) {
    let mut value = Vec::new();
    mc_nbt::write_network_value(&mut value, &tag).unwrap();
    stack
        .extended_mut()
        .preserved_components
        .push(mc_data::item_stack::PreservedItemComponent {
            name: name.into(),
            value,
            baseline: None,
        });
}

fn hive_place_input(sequence: i32) -> PlayerInput {
    PlayerInput::UseItemOn {
        connection: ConnectionId(1),
        position: BlockPos {
            y: POS.y - 1,
            ..POS
        },
        hand: Hand::Main,
        face: 1,
        cursor: [0.5, 1.0, 0.5],
        sequence,
    }
}

#[test]
fn silk_touch_break_pickup_place_disk_wal_and_checkpoint_have_one_bee_owner() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let dir = std::env::temp_dir().join(format!("bees-item-custody-{}", uuid::Uuid::new_v4()));
    let mut original = state(None);
    let actor = joined(&mut original);
    place(
        &mut original,
        BlockPos {
            y: POS.y - 1,
            ..POS
        },
        "minecraft:stone",
    );
    let occupant = occupied_hive(&mut original);
    let mut tool = item(&original, "minecraft:diamond_axe", 1);
    tool.enchantments.push(mc_data::ItemEnchantment {
        id: Identifier::parse("minecraft:silk_touch").unwrap(),
        level: 1,
    });
    original.players.get_mut(&actor).unwrap().inventory.slots[0] = tool;
    crate::tick::mining_tests::ready_to_finish(&mut original, ConnectionId(1), POS);
    let (seed_output, _, _) = crate::tick::run_tick(&mut original, &[]);
    let seed_entries = crate::core::journal_entries_for_tick(&mut original, &seed_output);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, seed_output.tick, &seed_entries).unwrap();
    let (output, _, _) = crate::tick::run_tick(
        &mut original,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: POS,
            action: crate::boundary::DigAction::Finish,
            sequence: 2,
        }],
    );
    let entries = crate::core::journal_entries_for_tick(&mut original, &output);
    writer.append(output.tick, output.tick, &entries).unwrap();
    assert!(!original.hives.contains_key(&key(POS)));
    assert!(
        original
            .entities
            .slots()
            .all(|slot| original.entities.row(slot).bee.is_none())
    );
    let hive_item = output
        .changes
        .entities_spawned
        .iter()
        .copied()
        .find(|id| {
            original.entities.slot_of(*id).is_some_and(|slot| {
                original
                    .entities
                    .row(slot)
                    .item
                    .as_ref()
                    .and_then(|row| row.stack.as_ref())
                    .is_some_and(|stack| item_component(stack, "minecraft:bees").unwrap().is_some())
            })
        })
        .expect("silk-touch hive drop carries its bees");
    let slot = original.entities.slot_of(hive_item).unwrap();
    let stack = original
        .entities
        .row(slot)
        .item
        .as_ref()
        .unwrap()
        .stack
        .as_ref()
        .unwrap();
    let carried =
        occupants_from_tag(item_component(stack, "minecraft:bees").unwrap().unwrap()).unwrap();
    assert_eq!(carried.len(), 1);
    assert_eq!(carried[0].data, occupant.data);
    assert!(carried[0].ticks >= occupant.ticks);
    assert_eq!(carried[0].minimum, occupant.minimum);
    assert_eq!(
        item_component(stack, "minecraft:block_state").unwrap(),
        Some(Tag::Compound(vec![(
            "honey_level".into(),
            Tag::String("4".into())
        )]))
    );
    original.entities.body_mut(slot).pickup_delay = 0;
    let actor_pos = original
        .entities
        .pos(original.entities.slot_of(actor).unwrap());
    original.entities.set_pos(slot, actor_pos);
    let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut original, &output);
    writer.append(output.tick, output.tick, &entries).unwrap();
    assert!(original.entities.slot_of_any(hive_item).is_none());
    let selected = original.players[&actor]
        .inventory
        .slots
        .iter()
        .position(|stack| item_component(stack, "minecraft:bees").unwrap().is_some())
        .expect("player picked up occupied hive");
    assert!(selected < 9);
    original.players.get_mut(&actor).unwrap().inventory.selected = selected as u8;
    let (output, _, _) = crate::tick::run_tick(&mut original, &[hive_place_input(3)]);
    let entries = crate::core::journal_entries_for_tick(&mut original, &output);
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    assert_eq!(honey(&original, POS), Some(4));
    assert_eq!(original.hives[&key(POS)].bees.len(), 1);
    assert_eq!(original.hives[&key(POS)].bees[0].data, occupant.data);
    assert!(original.players[&actor].inventory.slots[selected].is_empty());
    assert!(
        original
            .entities
            .slots()
            .all(|slot| original.entities.row(slot).bee.is_none())
    );
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let once = replay_batches(Replay::default(), &batches);
    let twice = replay_batches(once, &batches);
    assert!(
        twice
            .entities
            .values()
            .all(|entity| entity.state.bee.is_none())
    );
    assert!(twice.entities.values().all(|entity| {
        entity
            .item
            .as_ref()
            .is_none_or(|stack| item_component(stack, "minecraft:bees").unwrap().is_none())
    }));
    assert!(twice.players.values().all(|player| {
        player
            .inventory
            .iter()
            .all(|stack| item_component(stack, "minecraft:bees").unwrap().is_none())
    }));
    let recovered_hive =
        Hive::decode(twice.opaque_block_entities[&key(POS)].as_ref().unwrap()).unwrap();
    assert_eq!(recovered_hive.bees.len(), 1);
    assert_eq!(recovered_hive.bees[0], carried[0]);
    let mut restored = state(Some(dir.clone()));
    for (&(x, y, z), &block) in &twice.blocks {
        restored.world.set_block(x, y, z, block);
    }
    // Match production recovery: fold block post-images before opaque images.
    // Hydration reads the resident chunk, not the tick's block overlay.
    restored.world.fold_overlay();
    for (&(x, y, z), bytes) in &twice.opaque_block_entities {
        restored
            .world
            .set_opaque_block_entity(BlockPos { x, y, z }, bytes.clone());
    }
    hydrate_chunk(
        &mut restored.hives,
        &restored.registry,
        restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
    )
    .unwrap();
    checkpoint(&mut restored);
    let checkpoint_hive = restored.hives[&key(POS)].clone();
    restored
        .world
        .capture_dirty(&BTreeMap::new())
        .write()
        .unwrap();
    drop(restored);
    let mut restarted = state(Some(dir.clone()));
    hydrate_chunk(
        &mut restarted.hives,
        &restarted.registry,
        restarted.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
    )
    .unwrap();
    assert_eq!(restarted.hives[&key(POS)], checkpoint_hive);
    assert_eq!(honey(&restarted, POS), Some(4));
    assert!(
        restarted
            .entities
            .slots()
            .all(|slot| restarted.entities.row(slot).bee.is_none())
    );
    drop(restarted);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn non_silk_break_releases_nectar_bee_once_even_when_hive_is_replaced() {
    let mut original = state(None);
    let actor = joined(&mut original);
    occupied_hive(&mut original);
    let tool = item(&original, "minecraft:diamond_axe", 1);
    original.players.get_mut(&actor).unwrap().inventory.slots[0] = tool;
    crate::tick::mining_tests::ready_to_finish(&mut original, ConnectionId(1), POS);
    let (output, _, _) = crate::tick::run_tick(
        &mut original,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: POS,
            action: crate::boundary::DigAction::Finish,
            sequence: 2,
        }],
    );
    assert!(!original.hives.contains_key(&key(POS)));
    let bees: Vec<_> = output
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| {
            original
                .entities
                .slot_of(*id)
                .filter(|slot| original.entities.row(*slot).bee.is_some())
        })
        .collect();
    assert_eq!(bees.len(), 1);
    assert!(original.entities.row(bees[0]).bee.as_ref().unwrap().nectar);
    assert!(output.changes.entities_spawned.iter().all(|id| {
        original.entities.slot_of(*id).is_none_or(|slot| {
            original
                .entities
                .row(slot)
                .item
                .as_ref()
                .and_then(|row| row.stack.as_ref())
                .is_none_or(|stack| item_component(stack, "minecraft:bees").unwrap().is_none())
        })
    }));
    place(&mut original, POS, "minecraft:beehive");
    let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
    assert!(output.changes.entities_spawned.is_empty());
    assert_eq!(
        original
            .entities
            .slots()
            .filter(|slot| original.entities.row(*slot).bee.is_some())
            .count(),
        1
    );
    assert!(!original.hives.contains_key(&key(POS)));
}

#[test]
fn malformed_hive_item_placement_is_rejected_without_debit_or_owner_mutation() {
    let mut original = state(None);
    let actor = joined(&mut original);
    place(
        &mut original,
        BlockPos {
            y: POS.y - 1,
            ..POS
        },
        "minecraft:stone",
    );
    let mut stack = item(&original, "minecraft:beehive", 1);
    component(
        &mut stack,
        "minecraft:bees",
        Tag::List(ListTag {
            element_type: 10,
            elements: vec![Tag::Compound(Vec::new()); 4],
        }),
    );
    original.players.get_mut(&actor).unwrap().inventory.slots[0] = stack.clone();
    crate::tick::run_tick(&mut original, &[hive_place_input(1)]);
    assert_eq!(
        original.world.block_at(POS.x, POS.y, POS.z),
        Some(original.registry.states.air)
    );
    assert!(!original.hives.contains_key(&key(POS)));
    assert_eq!(original.players[&actor].inventory.slots[0], stack);
    let error = occupants_from_tag(item_component(&stack, "minecraft:bees").unwrap().unwrap())
        .unwrap_err()
        .to_string();
    assert!(
        error.ends_with("Beehive.bees: expected at most three occupants"),
        "{error}"
    );
    stack.extended_mut().preserved_components[0].value.push(0);
    let error = item_component(&stack, "minecraft:bees")
        .unwrap_err()
        .to_string();
    assert!(
        error.ends_with("minecraft:bees: trailing NBT bytes"),
        "{error}"
    );
}

#[test]
fn accepted_break_and_replacement_in_one_merge_do_not_keep_old_occupants() {
    use crate::intent::{Footprint, Intent, IntentKind, InvClaim};
    let mut original = state(None);
    let actor = joined(&mut original);
    occupied_hive(&mut original);
    let stack = item(&original, "minecraft:beehive", 1);
    original.players.get_mut(&actor).unwrap().inventory.slots[0] = stack.clone();
    let block_id = original
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:beehive").unwrap())
        .unwrap()
        .default
        .0;
    // These operations have the same ordering as two accepted merge intents.
    break_custody(&mut original, POS, &mut Vec::new()).unwrap();
    assert!(!original.hives.contains_key(&key(POS)));
    assert_eq!(original.hostile.spawned.len(), 1);
    let bee = original.hostile.spawned[0];
    original.merge_block_writes.push((POS, block_id));
    place_from_item(
        &mut original,
        &Intent {
            actor,
            seq: 2,
            kind: IntentKind::PlaceBlock {
                pos: POS,
                state: block_id,
            },
            footprint: Footprint {
                block_write: vec![(POS, block_id)],
                inv: vec![InvClaim {
                    owner: actor,
                    slot: 0,
                    expect_item: stack.item_id,
                    expect_count: 1,
                    take: 1,
                    put_item: None,
                    put_count: 0,
                }],
                ..Default::default()
            },
        },
    )
    .unwrap();
    original.world.set_block(POS.x, POS.y, POS.z, block_id);
    let mut changes = ChangeSet::default();
    run(&mut original, &mut Vec::new(), &mut changes);
    assert!(changes.entities_spawned.is_empty());
    assert!(original.hives[&key(POS)].bees.is_empty());
    let live = original
        .entities
        .row(original.entities.slot_of(bee).unwrap());
    assert!(live.bee.as_ref().unwrap().nectar);
    let bytes = original
        .world
        .chunk(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities
        .get(&POS)
        .unwrap();
    assert!(Hive::decode(bytes).unwrap().bees.is_empty());
    assert_eq!(
        original
            .entities
            .slots()
            .filter(|slot| original.entities.row(*slot).bee.is_some())
            .count(),
        1
    );
}

#[test]
fn break_custody_normalizes_zero_multiple_and_malformed_bee_carriers() {
    for carrier_counts in [vec![], vec![0], vec![1, 1], vec![3, 2], vec![0, 3, 1]] {
        let mut original = state(None);
        let occupant = occupied_hive(&mut original);
        let mut drops: Vec<_> = carrier_counts
            .iter()
            .map(|count| {
                let mut stack = item(&original, "minecraft:beehive", *count);
                // Malformed copied bytes are not trusted or decoded: custody comes
                // from the validated authoritative owner and replaces this payload.
                stack.extended_mut().preserved_components.push(
                    mc_data::item_stack::PreservedItemComponent {
                        name: "minecraft:bees".into(),
                        value: vec![0xff],
                        baseline: None,
                    },
                );
                stack
            })
            .collect();
        if carrier_counts.is_empty() {
            drops.push(item(&original, "minecraft:beehive", 1));
        }
        let total: i32 = drops.iter().map(|stack| stack.count).sum();
        break_custody(&mut original, POS, &mut drops).unwrap();
        assert!(!original.hives.contains_key(&key(POS)));
        assert_eq!(drops.iter().map(|stack| stack.count).sum::<i32>(), total);
        let carriers: Vec<_> = drops
            .iter()
            .filter(|stack| item_component(stack, "minecraft:bees").unwrap().is_some())
            .collect();
        if carrier_counts.iter().any(|count| *count > 0) {
            assert_eq!(carriers.len(), 1);
            assert_eq!(carriers[0].count, 1);
            assert_eq!(
                occupants_from_tag(
                    item_component(carriers[0], "minecraft:bees")
                        .unwrap()
                        .unwrap()
                )
                .unwrap(),
                vec![occupant]
            );
            assert!(original.hostile.spawned.is_empty());
            assert!(
                original
                    .entities
                    .slots()
                    .all(|slot| original.entities.row(slot).bee.is_none())
            );
        } else {
            assert!(carriers.is_empty());
            assert_eq!(original.hostile.spawned.len(), 1);
            let released = original.hostile.spawned[0];
            assert!(
                original
                    .entities
                    .row(original.entities.slot_of(released).unwrap())
                    .bee
                    .as_ref()
                    .unwrap()
                    .nectar
            );
        }
    }
}

#[test]
fn corrupt_break_owner_returns_typed_error_without_mutating_custody() {
    let mut original = state(None);
    place(&mut original, POS, "minecraft:beehive");
    let mut bytes = Vec::new();
    mc_nbt::write_network(
        &mut bytes,
        &Tag::Compound(vec![("bees".into(), Tag::Int(1))]),
    )
    .unwrap();
    original
        .world
        .set_opaque_block_entity(POS, Some(bytes.clone()));
    let mut drops = vec![item(&original, "minecraft:beehive", 1)];
    let before = drops.clone();
    let error = break_custody(&mut original, POS, &mut drops).unwrap_err();
    assert!(matches!(&error, CoreError::Storage(message)
        if message == "Beehive.bees: expected compound list"));
    assert_eq!(drops, before);
    assert_eq!(
        original
            .world
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities[&POS],
        bytes
    );
    assert!(!original.hives.contains_key(&key(POS)));
    assert!(original.hostile.spawned.is_empty());
}

#[test]
fn advancement_inhabited_count_reads_owner_then_saved_image_and_rejects_corruption() {
    let mut state = state(None);
    assert_eq!(inhabited_bees(&state, POS).unwrap(), None);
    let occupant = occupied_hive(&mut state);
    state.hives.get_mut(&key(POS)).unwrap().bees.push(occupant);
    assert_eq!(inhabited_bees(&state, POS).unwrap(), Some(2));
    state.hives.remove(&key(POS));
    assert_eq!(inhabited_bees(&state, POS).unwrap(), Some(1));
    let mut bytes = Vec::new();
    mc_nbt::write_network(
        &mut bytes,
        &Tag::Compound(vec![("bees".into(), Tag::Int(1))]),
    )
    .unwrap();
    state.world.set_opaque_block_entity(POS, Some(bytes));
    assert!(inhabited_bees(&state, POS).is_err());
}

#[test]
fn pending_item_placement_checkpoint_before_run_restarts_with_one_owner() {
    use crate::intent::{Footprint, Intent, IntentKind, InvClaim};
    let dir =
        std::env::temp_dir().join(format!("bees-pending-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    let actor = joined(&mut original);
    let occupant = occupied_hive(&mut original);
    let mut stack = item(&original, "minecraft:beehive", 1);
    component(
        &mut stack,
        "minecraft:bees",
        occupants_tag(std::slice::from_ref(&occupant)),
    );
    component(
        &mut stack,
        "minecraft:block_state",
        Tag::Compound(vec![("honey_level".into(), Tag::String("4".into()))]),
    );
    break_custody(&mut original, POS, &mut vec![stack.clone()]).unwrap();
    original
        .world
        .set_block(POS.x, POS.y, POS.z, original.registry.states.air);
    original.world.fold_overlay();
    original.players.get_mut(&actor).unwrap().inventory.slots[0] = stack.clone();
    let block_id = original
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:beehive").unwrap())
        .unwrap()
        .default
        .0;
    original.merge_block_writes.push((POS, block_id));
    let intent = Intent {
        actor,
        seq: 2,
        kind: IntentKind::PlaceBlock {
            pos: POS,
            state: block_id,
        },
        footprint: Footprint {
            block_write: vec![(POS, block_id)],
            inv: vec![InvClaim {
                owner: actor,
                slot: 0,
                expect_item: stack.item_id,
                expect_count: 1,
                take: 1,
                put_item: None,
                put_count: 0,
            }],
            ..Default::default()
        },
    };
    place_from_item(&mut original, &intent).unwrap();
    original.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
    assert!(original.hives[&key(POS)].pending_placement);
    assert_eq!(original.hives[&key(POS)].bees, vec![occupant.clone()]);
    assert_eq!(
        original.world.block_at(POS.x, POS.y, POS.z),
        Some(original.registry.states.air)
    );
    // No bees::run or block phase has occurred after accepted placement.
    checkpoint(&mut original);
    original
        .world
        .capture_dirty(&BTreeMap::new())
        .write()
        .unwrap();
    drop(original);
    let mut restarted = state(Some(dir.clone()));
    hydrate_chunk(
        &mut restarted.hives,
        &restarted.registry,
        restarted.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
    )
    .unwrap();
    assert_eq!(restarted.hives[&key(POS)].bees, vec![occupant]);
    assert!(!restarted.hives[&key(POS)].pending_placement);
    assert_eq!(honey(&restarted, POS), Some(4));
    assert!(
        restarted
            .entities
            .slots()
            .all(|slot| restarted.entities.row(slot).bee.is_none())
    );
    assert!(restarted.players.values().all(|player| {
        player
            .inventory
            .slots
            .iter()
            .all(|stack| item_component(stack, "minecraft:bees").unwrap().is_none())
    }));
    drop(restarted);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn corrupt_resident_hive_is_quarantined_without_panicking_the_tick() {
    let mut original = state(None);
    joined(&mut original);
    place(&mut original, POS, "minecraft:beehive");
    let mut bytes = Vec::new();
    mc_nbt::write_network(
        &mut bytes,
        &Tag::Compound(vec![("bees".into(), Tag::Int(1))]),
    )
    .unwrap();
    original.world.set_opaque_block_entity(POS, Some(bytes));
    let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
    let chunk = ChunkPos { x: 0, z: 0 };
    let error = original
        .world
        .load_error(chunk)
        .expect("invalid hive quarantines its chunk");
    assert_eq!(
        error.reason,
        "storage: Beehive.bees: expected compound list"
    );
    assert!(!original.world.has_chunk(0, 0));
    assert!(!original.hives.contains_key(&key(POS)));
    assert!(output.changes.entities_spawned.is_empty());
}

#[test]
fn tnt_break_releases_occupied_hive_once_without_bee_bearing_loot() {
    use crate::journal::{Replay, replay_batches};
    let mut original = state(None);
    joined(&mut original);
    occupied_hive(&mut original);
    original
        .gamerules
        .insert("tnt_explosion_drop_decay".into(), "false".into());
    let tnt = crate::tnt::prime_at(
        &mut original,
        [
            f64::from(POS.x) + 0.5,
            f64::from(POS.y),
            f64::from(POS.z) + 0.5,
        ],
        1,
    )
    .unwrap();
    let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
    assert_eq!(output.changes.explosions.len(), 1);
    assert!(original.entities.slot_of_any(tnt).is_none());
    assert_eq!(
        original.world.block_at(POS.x, POS.y, POS.z),
        Some(original.registry.states.air)
    );
    assert!(!original.hives.contains_key(&key(POS)));
    assert!(
        !original
            .world
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities
            .contains_key(&POS)
    );
    let released: Vec<_> = original
        .entities
        .slots()
        .filter(|slot| original.entities.row(*slot).bee.is_some())
        .map(|slot| original.entities.body(slot).id)
        .collect();
    assert_eq!(released.len(), 1);
    let bee = released[0];
    let slot = original.entities.slot_of(bee).unwrap();
    assert!(original.entities.row(slot).bee.as_ref().unwrap().nectar);
    assert_eq!(original.entities.row(slot).health, 7.5);
    let hive_item = item(&original, "minecraft:beehive", 1).item_id;
    let dropped: Vec<_> = original
        .entities
        .slots()
        .filter_map(|slot| original.entities.row(slot).item.as_ref())
        .collect();
    assert!(dropped.iter().any(|stack| stack.item_id == hive_item));
    assert!(dropped.iter().all(|row| {
        row.stack
            .as_ref()
            .is_none_or(|stack| item_component(stack, "minecraft:bees").unwrap().is_none())
    }));
    let first = crate::core::journal_entries_for_tick(&mut original, &output);
    let (next, _, _) = crate::tick::run_tick(&mut original, &[]);
    assert!(next.changes.explosions.is_empty());
    assert!(!next.changes.entities_spawned.contains(&bee));
    assert_eq!(
        original
            .entities
            .slots()
            .filter(|slot| original.entities.row(*slot).bee.is_some())
            .count(),
        1
    );
    let second = crate::core::journal_entries_for_tick(&mut original, &next);
    let batches = vec![
        (output.tick, output.tick, first),
        (next.tick, next.tick, second),
    ];
    let replay = replay_batches(Replay::default(), &batches);
    let twice = replay_batches(replay, &batches);
    assert_eq!(twice.opaque_block_entities[&key(POS)], None);
    assert_eq!(
        twice
            .entities
            .iter()
            .filter(|(_, image)| image.state.bee.is_some())
            .map(|(&id, _)| id)
            .collect::<Vec<_>>(),
        vec![bee.0]
    );
    assert!(twice.entities.values().all(|image| {
        image
            .item
            .as_ref()
            .is_none_or(|stack| item_component(stack, "minecraft:bees").unwrap().is_none())
    }));
}

#[test]
fn hive_bottle_floor_refusal_keeps_honey_bees_and_item_custody() {
    let mut state = state(None);
    let actor = joined(&mut state);
    place(&mut state, POS, "minecraft:beehive");
    let bee = mob(&mut state, "minecraft:bee", centre(POS));
    let full = property(&state, POS, "honey_level", "5").unwrap();
    state.world.set_block(POS.x, POS.y, POS.z, full);
    let bottle = item(&state, "minecraft:glass_bottle", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bottle.clone();
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "glass_bottle",
        1,
    );
    let mut changes = ChangeSet::default();
    assert!(use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &bottle,
        &mut changes
    ));
    assert_eq!(honey(&state, POS), Some(5));
    assert_eq!(state.players[&actor].inventory.slots[0], bottle);
    assert!(changes.entities_spawned.is_empty());
    assert!(
        state
            .entities
            .row(state.entities.slot_of(bee).unwrap())
            .bee
            .as_ref()
            .unwrap()
            .angry_at
            .is_none()
    );
    state.players.get_mut(&actor).unwrap().inventory.slots[0].count = 2;
    let bottle = state.players[&actor].inventory.slots[0].clone();
    assert!(use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &bottle,
        &mut changes
    ));
    assert_eq!(honey(&state, POS), Some(0));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(
        state.players[&actor]
            .inventory
            .slots
            .iter()
            .any(|stack| { *stack == item(&state, "minecraft:honey_bottle", 1) })
    );
}

#[test]
fn hive_unbreaking_plan_refuses_break_without_spending_rng_and_admits_skip() {
    let mut state = state(None);
    let actor = joined(&mut state);
    place(&mut state, POS, "minecraft:beehive");
    let full = property(&state, POS, "honey_level", "5").unwrap();
    state.world.set_block(POS.x, POS.y, POS.z, full);
    let mut shears = item(&state, "minecraft:shears", 1);
    let max = state
        .registry
        .item_facts
        .facts_for_stack(&shears, &state.registry.items)
        .unwrap()
        .max_damage
        .unwrap();
    shears.damage = Some(max as i32 - 1);
    shears.enchantments.push(mc_data::ItemEnchantment {
        id: Identifier::parse("minecraft:unbreaking").unwrap(),
        level: 3,
    });
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = shears.clone();
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "shears",
        1,
    );
    let breaking = (0..1024)
        .find(|seed| {
            let mut seed = *seed;
            crate::enchantments::durability_damage(&shears, 1, false, &mut seed) == 1
        })
        .unwrap();
    state.players.get_mut(&actor).unwrap().enchantment_seed = breaking;
    let before = state.players[&actor].inventory.slots.clone();
    let mut changes = ChangeSet::default();
    assert!(use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &shears,
        &mut changes
    ));
    assert_eq!(state.players[&actor].inventory.slots, before);
    assert_eq!(state.players[&actor].enchantment_seed, breaking);
    assert_eq!(honey(&state, POS), Some(5));
    assert!(changes.entities_spawned.is_empty());
    let skipping = (0..1024)
        .find(|seed| {
            let mut seed = *seed;
            crate::enchantments::durability_damage(&shears, 1, false, &mut seed) == 0
        })
        .unwrap();
    let mut expected_seed = skipping;
    assert_eq!(
        crate::enchantments::durability_damage(&shears, 1, false, &mut expected_seed),
        0
    );
    state.players.get_mut(&actor).unwrap().enchantment_seed = skipping;
    assert!(use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &shears,
        &mut changes
    ));
    assert_eq!(honey(&state, POS), Some(0));
    assert_eq!(state.players[&actor].inventory.slots[0], shears);
    assert_eq!(state.players[&actor].enchantment_seed, expected_seed);
}

#[test]
fn flowerless_bee_finds_wither_rose_and_rejects_waterlogged() {
    // BEE-FLOWER-TABLE through the existing pollination search path
    // (nearby + flower predicate): wither_rose attracts; waterlogged
    // torchflower and tall upper halves do not.
    let mut state = state(None);
    place(&mut state, BlockPos { x: 6, y: 201, z: 4 }, "minecraft:wither_rose");
    let found: Option<BlockPos> = nearby(&state, [4.5, 201.0, 4.5], |b| flower(b));
    assert_eq!(found, Some(BlockPos { x: 6, y: 201, z: 4 }));
}

#[test]
fn flower_rules_exclude_waterlogged_and_tall_upper() {
    // BEE-FLOWER-TABLE waterlogged/tall-half rules (post-fix coverage).
    let mut state = state(None);
    let torch = state
        .registry
        .blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:torchflower").unwrap(),
            &[("waterlogged".into(), "true".into())],
        );
    if let Some(torch) = torch {
        state.world.set_block(2, 201, 4, torch.0);
        state.world.fold_overlay();
        assert!(!flower_at(&state, BlockPos { x: 2, y: 201, z: 4 }));
    }
    let lilac = state
        .registry
        .blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:lilac").unwrap(),
            &[("half".into(), "upper".into())],
        );
    if let Some(lilac) = lilac {
        state.world.set_block(4, 201, 6, lilac.0);
        state.world.fold_overlay();
        assert!(!flower_at(&state, BlockPos { x: 4, y: 201, z: 6 }));
    }
}
