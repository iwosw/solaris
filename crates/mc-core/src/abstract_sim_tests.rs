use super::abstract_sim::*;
use crate::boundary::{BlockPos, ChunkPos};
use crate::journal::{JournalEntry, Replay, SimDebtImage};
use crate::tick::{BlockEntityState, TickState};
use mc_data::{Identifier, ItemStack};
use std::sync::Arc;

const CHUNK: ChunkPos = ChunkPos { x: 0, z: 0 };
const CELL: (i32, i32, i32) = (8, 100, 8);

fn fixture(chunks: &[ChunkPos]) -> TickState {
    let registry = Arc::new(crate::tick_tests::full_registry());
    let mut world = crate::horde_bench::flat_world(&registry);
    world.ensure_blocking(&chunks.iter().map(|&pos| (pos, 0)).collect());
    let mut state = TickState::new(registry, world, 2, 512);
    state.tick = 10000;
    for (index, &chunk) in chunks.iter().enumerate() {
        state
            .tickets
            .insert(format!("abstract-{index}"), (chunk, 0));
    }
    state.refresh_active_chunks(state.tick);
    state
}

fn native_fixture(directory: &std::path::Path, chunks: &[ChunkPos]) -> TickState {
    let registry = Arc::new(crate::tick_tests::full_registry());
    let mut world = crate::world::world_tests::test_world(Some(directory.to_path_buf()));
    world.ensure_blocking(&chunks.iter().map(|&pos| (pos, 0)).collect());
    let mut state = TickState::new(registry, world, 2, 512);
    state.tick = 10000;
    for (index, &chunk) in chunks.iter().enumerate() {
        state
            .tickets
            .insert(format!("abstract-{index}"), (chunk, 0));
    }
    state.refresh_active_chunks(state.tick);
    state
}

fn block(state: &TickState, path: &str) -> u32 {
    state
        .registry
        .blocks
        .block(&Identifier::parse(path).unwrap())
        .unwrap()
        .default
        .0
}
fn item(state: &TickState, path: &str) -> u32 {
    state
        .registry
        .items
        .id_of(&Identifier::parse(path).unwrap())
        .unwrap()
}
fn furnace(state: &mut TickState, cell: (i32, i32, i32), input: i32) {
    let id = block(state, "minecraft:furnace");
    state.world.set_block(cell.0, cell.1, cell.2, id);
    let slots = vec![
        ItemStack::new(item(state, "minecraft:iron_ore"), input),
        ItemStack::new(item(state, "minecraft:coal"), 4),
        ItemStack::EMPTY,
    ];
    state.block_entities.insert(
        cell,
        BlockEntityState {
            slots,
            furnace: Some(Default::default()),
        },
    );
    state.world.fold_overlay();
    cancel_external_writes(state);
}
fn owe(state: &mut TickState, chunks: &[ChunkPos], debt: u64) {
    state
        .world
        .restore_sim_debt(
            &Replay {
                tick: state.tick,
                sim_debt: chunks
                    .iter()
                    .map(|chunk| {
                        (
                            [chunk.x, chunk.z],
                            SimDebtImage {
                                chunk: [chunk.x, chunk.z],
                                last_sim: state.tick,
                                remaining: debt,
                                active: false,
                                tick: state.tick,
                            },
                        )
                    })
                    .collect(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    state.refresh_active_chunks(state.tick);
}
fn step(state: &mut TickState) {
    cancel_external_writes(state);
    run(state, &mut Vec::new());
}

#[test]
fn whole_chunk_step_cooks_items_ages_items_and_spends_debt_once() {
    let mut state = fixture(&[CHUNK]);
    furnace(&mut state, CELL, 8);
    let item_id = item(&state, "minecraft:stone");
    let mut spawned = Vec::new();
    state.spawn_block_loot(
        BlockPos { x: 9, y: 100, z: 8 },
        vec![ItemStack::new(item_id, 1)],
        1,
        &mut spawned,
    );
    let dropped = spawned[0];
    let slot = state.entities.slot_of(dropped).unwrap();
    state.entities.row_mut(slot).age = 5800;
    state.journaled_entity_state.insert(
        dropped,
        (
            state.tick,
            CHUNK,
            Arc::new(crate::journal::EntityStateImage::from_row(
                state.entities.body(slot),
                state.entities.row(slot),
            )),
        ),
    );
    owe(&mut state, &[CHUNK], 600);
    step(&mut state);
    assert_eq!(state.world.simulation_debt(CHUNK), 400);
    assert_eq!(state.block_entities[&CELL].slots[2].count, 1);
    assert_eq!(state.entities.row(slot).age, 6000);
    assert!(state.merge_consumed_items.contains(&slot));
    // The ordinary tick commit owns removal and its WAL tombstone.
    let (_, _, _, removed) = state.commit_merge_effects();
    assert!(removed.contains(&dropped));
    assert!(state.entities.slot_of(dropped).is_none());
    for _ in 0..4 {
        step(&mut state);
    }
    assert_eq!(state.world.simulation_debt(CHUNK), 0);
    assert_eq!(state.block_entities[&CELL].slots[2].count, 3);
    let cooked = state.block_entities[&CELL].slots[2].count;
    step(&mut state);
    assert_eq!(state.block_entities[&CELL].slots[2].count, cooked);
}

#[test]
fn empty_loaded_furnace_post_load_insert_never_spends_old_debt() {
    let mut state = fixture(&[CHUNK]);
    furnace(&mut state, CELL, 0);
    owe(&mut state, &[CHUNK], 600);
    let ore = item(&state, "minecraft:iron_ore");
    state.block_entities.get_mut(&CELL).unwrap().slots[0] = ItemStack::new(ore, 3);
    step(&mut state);
    assert_eq!(state.world.simulation_debt(CHUNK), 0);
    assert!(state.block_entities[&CELL].slots[2].is_empty());
}

fn late_insert_batch(directory: &std::path::Path) -> (TickState, Vec<JournalEntry>) {
    let chunks = [CHUNK, ChunkPos { x: 1, z: 0 }, ChunkPos { x: 2, z: 0 }];
    let mut state = native_fixture(directory, &chunks);
    let target = (40, 100, 8);
    furnace(&mut state, target, 0);
    state.tick = 10001;
    // Obtain a real tick output for the production WAL image owner.
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    crate::core::journal_entries_for_tick(&mut state, &output);
    // At tick % 3 == 0 the first two chunks precede the target.
    owe(&mut state, &chunks, 600);
    step(&mut state);
    assert_eq!(
        state.world.simulation_debt(chunks[2]),
        600,
        "target not reached by the slice budget"
    );
    let ore = item(&state, "minecraft:iron_ore");
    state.block_entities.get_mut(&target).unwrap().slots[0] = ItemStack::new(ore, 3);
    // No more catch-up: unload/crash may happen before the next slice. The
    // real WAL finalizer must cancel debt in the same batch as these slots.
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|entry| matches!(
        entry,
        JournalEntry::BlockEntity {
            pos: [40, 100, 8],
            ..
        }
    )));
    assert!(entries.iter().any(|entry| matches!(entry, JournalEntry::SimDebt { image } if image.chunk == [2, 0] && image.remaining == 0)));
    (state, entries)
}

fn checkpoint_and_unload(state: &mut TickState) {
    let target = ChunkPos { x: 2, z: 0 };
    assert!(
        !state
            .world
            .evict_outside(&Default::default())
            .contains(&target),
        "debt zero pins until matching ACK"
    );
    let images = state
        .block_entities
        .iter()
        .map(|(&pos, record)| {
            (
                pos,
                crate::journal::ReplayBlockEntity {
                    slots: record.slots.clone(),
                    furnace: record.furnace_image(),
                    tick: state.tick,
                },
            )
        })
        .collect();
    state.world.flush_dirty(&images).unwrap();
    assert!(
        state
            .world
            .evict_outside(&Default::default())
            .contains(&target),
        "unload occurs before any slice reaches the target"
    );
}

fn assert_reloaded_no_cooking(directory: &std::path::Path) {
    let batches =
        crate::journal::read_complete_batches_after(&directory.join("journal"), 0).unwrap();
    let replay = crate::journal::replay_batches(Replay::default(), &batches);
    assert_eq!(replay.sim_debt[&[2, 0]].remaining, 0);
    let mut state = native_fixture(directory, &[ChunkPos { x: 2, z: 0 }]);
    let target = (40, 100, 8);
    let saved = &replay.block_entities[&target];
    state.block_entities.hydrate(target, || BlockEntityState {
        slots: saved.slots.clone(),
        furnace: Some(Default::default()),
    });
    state.tick = replay.tick;
    state.world.restore_sim_debt(&replay, None).unwrap();
    state.refresh_active_chunks(state.tick);
    step(&mut state);
    assert!(state.block_entities[&target].slots[2].is_empty());
    assert_eq!(state.world.simulation_debt(ChunkPos { x: 2, z: 0 }), 0);
}

#[test]
fn post_load_insert_unload_restart_reload_does_not_reuse_debt() {
    let directory = tempfile::tempdir().unwrap();
    let (mut state, entries) = late_insert_batch(directory.path());
    let mut writer = crate::journal::JournalWriter::open(directory.path(), 1).unwrap();
    writer.append(1, state.tick, &entries).unwrap();
    writer.flush().unwrap();
    checkpoint_and_unload(&mut state);
    drop(state);
    drop(writer);
    assert_reloaded_no_cooking(directory.path());
}

#[test]
#[ignore = "child process of post_load_insert_kill9_recovery_does_not_reuse_debt"]
fn crash_child() {
    use std::io::Write;
    let directory = std::env::var_os("SOLARIS_ABSTRACT_CRASH_DIR").expect("parent directory");
    let (mut state, entries) = late_insert_batch(std::path::Path::new(&directory));
    let mut writer =
        crate::journal::JournalWriter::open(std::path::Path::new(&directory), 1).unwrap();
    writer.append(1, state.tick, &entries).unwrap();
    writer.flush().unwrap();
    checkpoint_and_unload(&mut state);
    println!("ABSTRACT_WAL_READY");
    std::io::stdout().flush().unwrap();
    let mut line = String::new();
    std::io::stdin().read_line(&mut line).unwrap();
    panic!("parent must kill child before any graceful shutdown");
}

#[test]
fn post_load_insert_kill9_recovery_does_not_reuse_debt() {
    use std::io::BufRead;
    let directory = tempfile::tempdir().unwrap();
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "abstract_sim_tests::crash_child",
            "--ignored",
            "--nocapture",
        ])
        .env("SOLARIS_ABSTRACT_CRASH_DIR", directory.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
    let mut ready = false;
    let mut line = String::new();
    while reader.read_line(&mut line).unwrap() != 0 {
        if line.contains("ABSTRACT_WAL_READY") {
            ready = true;
            break;
        }
        line.clear();
    }
    if !ready {
        let status = child.wait().unwrap();
        panic!("child failed before durable WAL: {status}");
    }
    child.kill().unwrap();
    assert!(!child.wait().unwrap().success());
    assert_reloaded_no_cooking(directory.path());
}

#[test]
fn restock_counters_catch_up_without_body_or_chunk_load() {
    let mut state = fixture(&[CHUNK]);
    state.day_time = 3000;
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, [8.5, 100.5, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    let site = BlockPos { x: 9, y: 100, z: 8 };
    state
        .world
        .set_block(site.x, site.y, site.z, block(&state, "minecraft:composter"));
    let mut v = crate::villager::VillagerState::default();
    v.data.profession = crate::villager::VillagerProfession::Farmer;
    v.job_site = Some(site);
    v.job_site_dimension = state.dimension.name().into();
    crate::villager::trades::generate(&mut v, &state.registry);
    assert!(!v.offers.is_empty());
    v.offers[0].uses = v.offers[0].max_uses;
    state.entities.row_mut(slot).villager = Some(Box::new(v));
    owe(&mut state, &[CHUNK], 200);
    let pos = state.entities.pos(slot);
    let resident = state.world.chunks().count();
    step(&mut state);
    let v = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(v.offers[0].uses, 0);
    assert_eq!(v.restocks_today, 1);
    assert_eq!(state.entities.pos(slot), pos);
    assert_eq!(state.world.chunks().count(), resident);
    step(&mut state);
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .restocks_today,
        1
    );
}

#[test]
fn accepted_block_write_including_same_state_cancels_whole_chunk_debt() {
    let mut state = fixture(&[CHUNK]);
    furnace(&mut state, CELL, 5);
    owe(&mut state, &[CHUNK], 600);
    let current = state.world.block_at(1, 100, 1).unwrap();
    state.world.set_block(1, 100, 1, current);
    assert_eq!(state.world.simulation_debt(CHUNK), 0);
    step(&mut state);
    assert!(state.block_entities[&CELL].slots[2].is_empty());
    let mut entries = Vec::new();
    state.world.journal_debt(&mut entries);
    assert!(entries.iter().any(|entry| matches!(entry, JournalEntry::SimDebt { image } if image.chunk == [0, 0] && image.remaining == 0)));
}

#[test]
fn dormant_debt_never_loads_a_chunk() {
    let mut state = fixture(&[CHUNK]);
    let dormant = ChunkPos { x: 64, z: 64 };
    owe(&mut state, &[dormant], 10000);
    let resident = state.world.chunks().count();
    step(&mut state);
    assert_eq!(state.world.chunks().count(), resident);
    assert!(!state.world.has_chunk(dormant.x, dormant.z));
    assert_eq!(state.world.simulation_debt(dormant), 10000);
}

#[test]
fn item_admission_fresh_drop_does_not_borrow_old_chunk_time() {
    for block_loot in [false, true] {
        let mut state = fixture(&[CHUNK]);
        owe(&mut state, &[CHUNK], 6000);
        let stack = ItemStack::new(item(&state, "minecraft:stone"), 1);
        let id = if block_loot {
            let mut spawned = Vec::new();
            state.spawn_block_loot(
                BlockPos { x: 8, y: 100, z: 8 },
                vec![stack],
                1,
                &mut spawned,
            );
            spawned[0]
        } else {
            crate::tick::spawn_item_entity(&mut state, [8.5, 100.5, 8.5], stack)
        };
        step(&mut state);
        let slot = state.entities.slot_of(id).expect("fresh item survives");
        assert_eq!(
            state.entities.row(slot).age,
            0,
            "new content cannot inherit dormant age"
        );
        assert_eq!(state.world.simulation_debt(CHUNK), 0);
    }
}

#[test]
fn item_admission_chunk_crossing_cancels_destination_time() {
    let target = ChunkPos { x: 1, z: 0 };
    let mut state = fixture(&[CHUNK, target]);
    let stack = ItemStack::new(item(&state, "minecraft:stone"), 1);
    let id = crate::tick::spawn_item_entity(&mut state, [15.5, 100.5, 8.5], stack);
    let slot = state.entities.slot_of(id).unwrap();
    state.journaled_entity_state.insert(
        id,
        (
            state.tick,
            CHUNK,
            Arc::new(crate::journal::EntityStateImage::from_row(
                state.entities.body(slot),
                state.entities.row(slot),
            )),
        ),
    );
    owe(&mut state, &[target], 6000);
    state.entities.set_pos(slot, [16.5, 100.5, 8.5]);
    step(&mut state);
    assert_eq!(
        state.entities.row(slot).age,
        0,
        "crossing carries the item's own clock, not destination debt"
    );
    assert_eq!(state.world.simulation_debt(target), 0);
}

#[test]
fn item_admission_replay_hydration_keeps_preexisting_debt() {
    let mut source = fixture(&[CHUNK]);
    let stack = ItemStack::new(item(&source, "minecraft:stone"), 1);
    let id = crate::tick::spawn_item_entity(&mut source, [8.5, 100.5, 8.5], stack);
    source.hostile.spawned.push(id);
    let (output, _, _) = crate::tick::run_tick(&mut source, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut source, &output);
    let replay = crate::journal::replay_batches(Replay::default(), &[(1, output.tick, entries)]);
    let mut record = replay.entities[&id.0].clone();
    record.age = 5800;
    let mut state = fixture(&[CHUNK]);
    crate::core::restore_entity(&mut state, id.0, &record).unwrap();
    owe(&mut state, &[CHUNK], 200);
    step(&mut state);
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(state.entities.row(slot).age, 6000);
    assert!(state.merge_consumed_items.contains(&slot));
}

#[test]
fn item_admission_after_slice_cancels_debt_in_its_spawn_wal_batch() {
    let mut state = fixture(&[CHUNK]);
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    crate::core::journal_entries_for_tick(&mut state, &output);
    owe(&mut state, &[CHUNK], 6000);
    let stack = ItemStack::new(item(&state, "minecraft:stone"), 1);
    let id = crate::tick::spawn_item_entity(&mut state, [8.5, 100.5, 8.5], stack);
    Arc::get_mut(&mut output.changes)
        .expect("unpublished fixture changes")
        .entities_spawned
        .push(id);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(
        entries
            .iter()
            .any(|entry| matches!(entry, JournalEntry::Entity { id: saved, .. } if *saved == id.0))
    );
    assert!(entries.iter().any(|entry| matches!(entry, JournalEntry::SimDebt { image } if image.chunk == [0, 0] && image.remaining == 0)));
}

fn plant_crop(state: &mut TickState, x: i32) -> (u32, u32) {
    let farmland = state
        .registry
        .blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:farmland").unwrap(),
            &[("moisture".into(), "7".into())],
        )
        .unwrap()
        .0;
    let wheat = Identifier::parse("minecraft:wheat").unwrap();
    let young = state
        .registry
        .blocks
        .by_name_and_props(&wheat, &[("age".into(), "0".into())])
        .unwrap()
        .0;
    let grown = state
        .registry
        .blocks
        .by_name_and_props(&wheat, &[("age".into(), "7".into())])
        .unwrap()
        .0;
    for dx in -1..=1 {
        for dz in -1..=1 {
            if state.world.has_chunk((x + dx).div_euclid(16), 0) {
                state.world.set_block(x + dx, 99, 8 + dz, farmland);
            }
        }
    }
    state.world.set_block(x, 100, 8, young);
    state.world.fold_overlay();
    state
        .gamerules
        .insert("random_tick_speed".into(), "4096".into());
    (young, grown)
}

#[test]
fn crop_catch_up_grows_expected_stages_and_spends_chunk_time() {
    let mut state = fixture(&[CHUNK]);
    let (_, grown) = plant_crop(&mut state, 8);
    owe(&mut state, &[CHUNK], 200);
    step(&mut state);
    assert_eq!(state.world.block_at(8, 100, 8), Some(grown));
    assert_eq!(state.world.simulation_debt(CHUNK), 0);
}

#[test]
fn crop_catch_up_missing_neighbour_hydration_or_light_does_not_grow() {
    let mut state = fixture(&[CHUNK]);
    let neighbor = ChunkPos { x: 1, z: 0 };
    state
        .world
        .reject_resident(neighbor, "fixture missing crop-neighbour data".into());
    let (young, _) = plant_crop(&mut state, 15);
    owe(&mut state, &[CHUNK], 200);
    step(&mut state);
    assert_eq!(state.world.block_at(15, 100, 8), Some(young));
    assert!(!state.world.has_chunk(neighbor.x, neighbor.z));
    state
        .world
        .reject_resident(CHUNK, "fixture missing crop light data".into());
    assert!(crate::farming::catch_up_crops(&state, CHUNK, 200).is_empty());
}

#[test]
fn crop_catch_up_dark_loaded_crop_does_not_grow() {
    let mut state = fixture(&[CHUNK]);
    let (young, _) = plant_crop(&mut state, 8);
    // Vanilla crops gate on undarkened light (`getRawBrightness(pos, 0)`), so
    // night alone never stops an open-sky crop; sealing the crop and its
    // farmland inside stone does (light also seeps in through open sides).
    let wall = state.registry.states.stone;
    for x in 6..=10 {
        for y in 98..=102 {
            for z in 6..=10 {
                let inside = (7..=9).contains(&x)
                    && (7..=9).contains(&z)
                    && (y == 99 || (y == 100 && x == 8 && z == 8));
                if !inside {
                    state.world.set_block(x, y, z, wall);
                }
            }
        }
    }
    state.world.fold_overlay();
    assert!(state.raw_brightness(8, 100, 8) < 9);
    owe(&mut state, &[CHUNK], 200);
    step(&mut state);
    assert_eq!(state.world.block_at(8, 100, 8), Some(young));
}

#[test]
fn restock_catch_up_never_rewinds_already_observed_live_work() {
    let mut state = fixture(&[CHUNK]);
    state.day_time = 3000;
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, [8.5, 100.5, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    let site = BlockPos { x: 9, y: 100, z: 8 };
    state
        .world
        .set_block(site.x, site.y, site.z, block(&state, "minecraft:composter"));
    let mut v = crate::villager::VillagerState::default();
    v.data.profession = crate::villager::VillagerProfession::Farmer;
    v.job_site = Some(site);
    v.job_site_dimension = state.dimension.name().into();
    v.last_work = state.tick as i64;
    state.entities.row_mut(slot).villager = Some(Box::new(v));
    owe(&mut state, &[CHUNK], 200);
    step(&mut state);
    assert_eq!(
        state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .unwrap()
            .last_work,
        state.tick as i64
    );
}

#[test]
fn catch_up_uses_current_chunk_membership_after_item_moves() {
    let mut state = fixture(&[CHUNK]);
    let item_id = item(&state, "minecraft:stone");
    let arrived =
        crate::tick::spawn_item_entity(&mut state, [160.5, 100.0, 8.5], ItemStack::new(item_id, 1));
    let departed =
        crate::tick::spawn_item_entity(&mut state, [8.5, 100.0, 8.5], ItemStack::new(item_id, 1));
    let arrived_slot = state.entities.slot_of(arrived).unwrap();
    let departed_slot = state.entities.slot_of(departed).unwrap();
    state.entities.set_pos(arrived_slot, [8.5, 100.0, 8.5]);
    state.entities.set_pos(departed_slot, [160.5, 100.0, 8.5]);
    for index in 0..128 {
        state.spawn_mob(
            state.registry.cow_type_id,
            [1000.5 + f64::from(index) * 16.0, 100.0, 8.5],
            10.0,
        );
    }
    owe(&mut state, &[CHUNK], 200);
    run(&mut state, &mut Vec::new());
    assert_eq!(state.entities.row(arrived_slot).age, 200);
    assert_eq!(state.entities.row(departed_slot).age, 0);
    assert_eq!(state.world.simulation_debt(CHUNK), 0);
}
