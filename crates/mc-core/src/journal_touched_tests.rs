use super::*;
use crate::core::journal_entries_for_tick;
use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
use crate::tick::BlockEntityState;
use std::sync::Arc;

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.0),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}

fn fixture() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 65, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

fn villager_type(state: &TickState) -> u32 {
    state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap()
}

fn spawn_idle_villager(state: &mut TickState, x: f64) -> EntityId {
    let id = state.spawn_mob(villager_type(state), [x, 65.0, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    row.controls.no_ai = true;
    row.villager = Some(Box::new(crate::villager::VillagerState::default()));
    id
}

fn spawn_idle_cow(state: &mut TickState, x: f64) -> EntityId {
    let id = state.spawn_mob(state.registry.cow_type_id, [x, 65.0, 9.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    row.controls.no_ai = true;
    id
}

fn step(state: &mut TickState) -> Vec<JournalEntry> {
    let (output, _, _) = crate::tick::run_tick(state, &[]);
    let entries = journal_entries_for_tick(state, &output);
    assert_baselines_complete(state, output.tick);
    entries
}

fn image_ids(entries: &[JournalEntry]) -> Vec<i32> {
    let mut ids: Vec<i32> = entries
        .iter()
        .filter_map(|entry| match entry {
            JournalEntry::Entity { id, .. } | JournalEntry::EntityState { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    ids.sort_unstable();
    ids
}

fn spawn_vehicle(state: &mut TickState, name: &str, pos: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, pos, 4.0);
    let slot = state.entities.slot_of(id).unwrap();
    crate::vehicles::restore(state, slot);
    id
}

#[test]
fn idle_counter_progression_and_reset_do_not_emit_entity_state_images() {
    let mut state = fixture();
    let id = spawn_idle_cow(&mut state, 8.5);
    assert_eq!(image_ids(&step(&mut state)), vec![id.0]);
    let slot = state.entities.slot_of(id).unwrap();
    for elapsed in 1..=100 {
        let row = state.entities.row_mut(slot);
        row.no_action_time = elapsed;
        row.age = elapsed;
        assert!(
            image_ids(&step(&mut state)).is_empty(),
            "idle counter progression alone must not rewrite retained entity state"
        );
    }
    for expected in [100, 0] {
        state.entities.row_mut(slot).no_action_time = expected;
        assert!(
            image_ids(&step(&mut state)).is_empty(),
            "the despawn idle counter is checkpoint-only, including resets"
        );
        let (_, regions, _) = crate::core::checkpoint_data_for(&state, state.tick);
        let saved = regions
            .values()
            .flat_map(|chunk| &chunk.entities)
            .find(|entity| entity.wire_id == id.0)
            .unwrap();
        let image = crate::journal::EntityStateImage::from_record(&state.registry, saved).unwrap();
        assert_eq!(
            image.no_action_time, expected,
            "checkpoint capture retains the current idle counter, including resets"
        );
    }
    // Meaningful retained changes still carry the latest counters incidentally.
    let row = state.entities.row_mut(slot);
    row.persistent = false;
    row.no_action_time = 712;
    let entries = step(&mut state);
    assert!(entries.iter().any(|entry| matches!(
        entry,
        JournalEntry::EntityState { id: changed, state: image, .. }
            if *changed == id.0 && !image.persistent && image.no_action_time == 712
    )));
}

#[test]
fn resting_chest_minecarts_emit_no_entity_images_after_first_tick() {
    let mut state = fixture();
    let carts: Vec<_> = (0..100)
        .map(|index| {
            spawn_vehicle(
                &mut state,
                "chest_minecart",
                [4.5 + f64::from(index), 65.0, 8.5],
            )
        })
        .collect();
    assert_eq!(image_ids(&step(&mut state)).len(), carts.len());
    for _ in 0..8 {
        // Conservative marks must not turn an unchanged resting body into WAL.
        for &id in &carts {
            let slot = state.entities.slot_of(id).unwrap();
            let _ = state.entities.row_mut(slot);
        }
        assert!(
            image_ids(&step(&mut state)).is_empty(),
            "resting vehicles emit no repeated postimages"
        );
    }
}

#[test]
fn vehicle_motion_and_rotation_only_changes_survive_wal_replay() {
    let dir = std::env::temp_dir().join(format!("mc-core-vehicle-pose-{}", uuid::Uuid::new_v4()));
    let mut state = fixture();
    let cart = spawn_vehicle(&mut state, "chest_minecart", [8.5, 65.0, 8.5]);
    let boat = spawn_vehicle(&mut state, "oak_boat", [10.5, 65.0, 8.5]);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    let entries = step(&mut state);
    writer.append(state.tick, state.tick, &entries).unwrap();

    let cart_slot = state.entities.slot_of(cart).unwrap();
    state.entities.set_pos(cart_slot, [9.0, 65.0, 8.5]);
    state.entities.set_vel(cart_slot, [2.0, 0.0, 0.0]);
    let boat_slot = state.entities.slot_of(boat).unwrap();
    let boat_pos = state.entities.pos(boat_slot);
    let boat_vel = state.entities.vel(boat_slot);
    state.entities.set_rot(boat_slot, 90.0, 15.0);
    let entries = step(&mut state);
    let mut expected = vec![cart.0, boat.0];
    expected.sort_unstable();
    assert_eq!(image_ids(&entries), expected);
    writer.append(state.tick, state.tick, &entries).unwrap();
    let quiet = step(&mut state);
    assert!(image_ids(&quiet).is_empty());
    writer.append(state.tick, state.tick, &quiet).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    assert_eq!(replay.entities[&cart.0].pos, [9.0, 65.0, 8.5]);
    assert_eq!(replay.entities[&cart.0].vel, [2.0, 0.0, 0.0]);
    assert_eq!(replay.entities[&boat.0].pos, boat_pos);
    assert_eq!(replay.entities[&boat.0].vel, boat_vel);
    assert_eq!(replay.entities[&boat.0].yaw, 90.0);
    assert_eq!(replay.entities[&boat.0].pitch, 15.0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn row_mut_spawn_and_removal_are_the_only_journal_marks() {
    let mut state = fixture();
    let cow = spawn_idle_cow(&mut state, 8.5);
    let other = spawn_idle_cow(&mut state, 9.5);
    assert_eq!(state.entities.take_touched().len(), 2, "spawn marks");
    assert!(state.entities.take_touched().is_empty(), "marks drain once");
    let slot = state.entities.slot_of(cow).unwrap();
    state.entities.row_mut(slot).love = 5;
    state.entities.row_mut(slot).love = 6;
    assert_eq!(state.entities.take_touched(), vec![cow], "deduplicated");
    let _ = state.entities.row(slot);
    state.entities.set_pos(slot, [1.0, 65.0, 1.0]);
    assert!(
        state.entities.take_touched().is_empty(),
        "reads/columns do not mark"
    );
    state.entities.set_pos(slot, [40.0, 65.0, 1.0]);
    assert_eq!(
        state.entities.take_touched(),
        vec![cow],
        "a chunk-crossing move marks"
    );
    state.entities.remove(slot);
    assert_eq!(state.entities.take_removed(), vec![cow]);
    let moved = state.entities.slot_of(other).unwrap();
    state.entities.row_mut(moved).love = 1;
    assert_eq!(
        state.entities.take_touched(),
        vec![other],
        "swap-remove keeps flags aligned"
    );
}

#[test]
fn no_op_entity_writeback_and_transient_timer_changes_leave_journal_clean() {
    let mut state = fixture();
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state
        .tickets
        .insert("journal-writeback-fixture".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(chunk, 0)]));
    state.refresh_active_chunks(0);
    let quiet = spawn_idle_cow(&mut state, 8.5);
    let timed = spawn_idle_cow(&mut state, 10.5);
    let villager = spawn_idle_villager(&mut state, 12.5);
    let timed_slot = state.entities.slot_of(timed).unwrap();
    state.entities.body_mut(timed_slot).attack_cooldown = 2;
    state.entities.take_touched();

    crate::tick::run_tick(&mut state, &[]);
    let touched = state.entities.take_touched();
    assert!(
        !touched.contains(&quiet),
        "unchanged cold fields need no journal comparison"
    );
    assert!(
        !touched.contains(&villager),
        "an inactive brain needs no journal comparison"
    );
    assert!(
        !touched.contains(&timed),
        "transient attack cooldown changes need no journal comparison"
    );
    assert_eq!(state.entities.body(timed_slot).attack_cooldown, 1);
}

#[test]
fn only_touched_rows_whose_images_differ_are_emitted() {
    let mut state = fixture();
    let villagers: Vec<_> = (0..4)
        .map(|i| spawn_idle_villager(&mut state, 4.5 + f64::from(i)))
        .collect();
    let cows: Vec<_> = (0..4)
        .map(|i| spawn_idle_cow(&mut state, 4.5 + f64::from(i)))
        .collect();
    let baseline = step(&mut state);
    assert!(
        image_ids(&baseline).len() >= 8,
        "first batch baselines every row: {baseline:#?}"
    );
    assert!(
        image_ids(&step(&mut state)).is_empty(),
        "an untouched population emits nothing"
    );

    // A conservative borrow without a change costs a comparison, not an image.
    let slot = state.entities.slot_of(villagers[0]).unwrap();
    let _ = state.entities.row_mut(slot);
    let slot = state.entities.slot_of(cows[0]).unwrap();
    let _ = state.entities.row_mut(slot);
    assert!(image_ids(&step(&mut state)).is_empty());

    // A real change emits exactly that row.
    let slot = state.entities.slot_of(villagers[1]).unwrap();
    state.entities.row_mut(slot).villager.as_mut().unwrap().xp = 17;
    let slot = state.entities.slot_of(cows[2]).unwrap();
    state.entities.row_mut(slot).opaque_entity_fields = vec![1, 2, 3];
    assert_eq!(
        image_ids(&step(&mut state)),
        vec![villagers[1].0.min(cows[2].0), villagers[1].0.max(cows[2].0)]
    );
    assert!(image_ids(&step(&mut state)).is_empty());
}

#[test]
fn removed_rows_drop_their_baselines() {
    let mut state = fixture();
    let villager = spawn_idle_villager(&mut state, 8.5);
    let cow = spawn_idle_cow(&mut state, 9.5);
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    journal_entries_for_tick(&mut state, &output);
    assert!(state.journaled_villagers.contains_key(&villager));
    assert!(state.journaled_entity_state.contains_key(&cow));
    for id in [villager, cow] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.remove(slot);
    }
    journal_entries_for_tick(&mut state, &output);
    assert!(state.journaled_villagers.is_empty());
    assert!(state.journaled_entity_state.is_empty());
}

#[test]
fn deterministic_lifecycle_clocks_leave_journal_baselines_lazy() {
    let mut state = fixture();
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state.tickets.insert("lazy-life-clocks".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(chunk, 0)]));
    state.refresh_active_chunks(0);
    let cow = spawn_idle_cow(&mut state, 8.5);
    let chicken_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:chicken").unwrap())
        .unwrap();
    let chicken = state.spawn_mob(chicken_type, [10.5, 65.0, 8.5], 4.0);
    let cow_slot = state.entities.slot_of(cow).unwrap();
    state.entities.row_mut(cow_slot).breed_cooldown = 100;
    let chicken_slot = state.entities.slot_of(chicken).unwrap();
    let row = state.entities.row_mut(chicken_slot);
    row.persistent = true;
    row.controls.no_ai = true;
    row.entity_life.harvest_ticks = 5;
    step(&mut state);
    let anchor = state.tick;

    for _ in 0..3 {
        assert!(image_ids(&step(&mut state)).is_empty());
        for id in [cow, chicken] {
            assert_eq!(
                state.journaled_entity_state[&id].0, anchor,
                "deterministic clock ticks must not compare the full retained image"
            );
        }
    }
    assert_eq!(state.entities.row(cow_slot).breed_cooldown, 96);
    assert_eq!(
        state.entities.row(chicken_slot).entity_life.harvest_ticks,
        1
    );

    // A random egg reseed is not a projection: it and the new egg are journaled.
    assert!(image_ids(&step(&mut state)).contains(&chicken.0));
    assert!(state.entities.row(chicken_slot).entity_life.harvest_ticks >= 6000);
}

#[test]
fn only_dead_clock_sensitive_images_are_rechecked_every_batch() {
    let mut state = fixture();
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state.tickets.insert("dead-life-clocks".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(chunk, 0)]));
    state.refresh_active_chunks(0);
    let cooling = spawn_idle_cow(&mut state, 8.5);
    let idle = spawn_idle_cow(&mut state, 9.5);
    let slot = state.entities.slot_of(cooling).unwrap();
    state.entities.row_mut(slot).breed_cooldown = 100;
    state.entities.body_mut(slot).alive = false;
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    journal_entries_for_tick(&mut state, &output);
    assert!(state.journal_scan.clock_recheck.contains(&cooling));
    assert!(!state.journal_scan.clock_recheck.contains(&idle));
    assert_eq!(
        image_ids(&step(&mut state)),
        vec![cooling.0],
        "stopped dead clocks still need their original per-tick absolute images"
    );
    let slot = state.entities.slot_of_any(cooling).unwrap();
    state.entities.row_mut(slot).breed_cooldown = 0;
    step(&mut state);
    assert!(state.journal_scan.clock_recheck.is_empty());
}

#[test]
fn block_entity_images_follow_container_custody_marks() {
    let mut state = fixture();
    let cell = (8, 65, 8);
    state.block_entities.insert(
        cell,
        BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let count = |entries: &[JournalEntry]| {
        entries
            .iter()
            .filter(|entry| matches!(entry, JournalEntry::BlockEntity { pos, .. } if *pos == [8, 65, 8]))
            .count()
    };
    assert_eq!(count(&journal_entries_for_tick(&mut state, &output)), 1);
    assert_eq!(count(&journal_entries_for_tick(&mut state, &output)), 0);
    let _ = state.block_entities.get_mut(&cell);
    assert_eq!(
        count(&journal_entries_for_tick(&mut state, &output)),
        0,
        "a borrow without change stays equal"
    );
    state.block_entities.get_mut(&cell).unwrap().slots[3] = mc_data::ItemStack::new(1, 2);
    assert_eq!(count(&journal_entries_for_tick(&mut state, &output)), 1);
    state.block_entities.remove(&cell);
    journal_entries_for_tick(&mut state, &output);
    assert!(!state.journaled_block_entities.contains_key(&cell));
}

/// kill -9 after a population ran with touched-only journaling: reopening
/// the WAL reproduces every villager and generic entity image exactly.
#[test]
fn kill_nine_reopen_reproduces_touched_only_images() {
    let dir = std::env::temp_dir().join(format!("mc-core-touched-kill9-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut state = fixture();
    state.tick = 100;
    state.day_time = 6500;
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state.tickets.insert("kill9".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    state.world.set_block(8, 64, 8, state.registry.states.stone);
    state.world.fold_overlay();
    let mut villagers = Vec::new();
    for i in 0..6 {
        let id = state.spawn_mob(villager_type(&state), [4.5 + f64::from(i), 65.0, 8.5], 20.0);
        let slot = state.entities.slot_of(id).unwrap();
        let row = state.entities.row_mut(slot);
        row.persistent = true;
        row.speed = 0.0;
        row.villager = Some(Box::new(crate::villager::VillagerState {
            age: -(100 + 37 * i64::from(i)) as i32,
            last_work: 23,
            last_slept: Some(17),
            ..Default::default()
        }));
        villagers.push(id);
    }
    let cows: Vec<_> = (0..3)
        .map(|i| {
            let id = spawn_idle_cow(&mut state, 4.5 + f64::from(i));
            let slot = state.entities.slot_of(id).unwrap();
            let row = state.entities.row_mut(slot);
            row.breed_cooldown = 300 + 50 * i as u32;
            row.entity_life.child_ticks = 150;
            id
        })
        .collect();
    let mut writer = JournalWriter::open(&dir, 1000).unwrap();
    for step in 0..96u64 {
        if step == 60 {
            state.tickets.remove("kill9");
        }
        if step == 80 {
            state.tickets.insert("kill9".into(), (chunk, 0));
            state
                .world
                .ensure_blocking(&[(chunk, 0)].into_iter().collect());
        }
        let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
        if step == 0 {
            std::sync::Arc::get_mut(&mut output.changes)
                .expect("fresh tick owns its changes")
                .entities_spawned
                .extend(cows.iter().copied());
        }
        // Owner events at irregular ticks: only these rows are touched by
        // the test, everything else by the simulation itself.
        if step % 17 == 5 {
            let slot = state
                .entities
                .slot_of(villagers[(step % 6) as usize])
                .unwrap();
            state.entities.row_mut(slot).villager.as_mut().unwrap().xp += 3;
        }
        if step == 40 {
            let slot = state.entities.slot_of(cows[1]).unwrap();
            state.entities.row_mut(slot).breed_cooldown = 500;
        }
        if step == 60 {
            // Evict only after this tick records the inactive boundary.
            state.world.capture_dirty(&Default::default());
            let written = state
                .world
                .resident_positions()
                .into_iter()
                .map(|pos| (pos, state.world.chunk_revision(pos.x, pos.z)))
                .collect::<Vec<_>>();
            state.world.mark_flushed(&written);
            state.world.evict_outside(&Default::default());
        }
        let entries = journal_entries_for_tick(&mut state, &output);
        assert_baselines_complete(&state, output.tick);
        writer.append(output.tick, output.tick, &entries).unwrap();
    }
    writer.flush().unwrap();
    drop(writer); // no shutdown: kill -9
    let batches = read_complete_batches_after(&dir.join("journal"), 100).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    for id in &villagers {
        let slot = state.entities.slot_of(*id).unwrap();
        let live = state.entities.row(slot).villager.as_ref().unwrap();
        let recovered = replay.entities[&id.0].villager.as_ref().unwrap();
        assert!(
            crate::villager::persistence::journal_eq(live, recovered),
            "villager {id:?} recovered differently:\nlive {live:?}\nrecovered {recovered:?}"
        );
    }
    for id in &cows {
        let slot = state.entities.slot_of(*id).unwrap();
        let row = state.entities.row(slot);
        let image = &replay.entities[&id.0].state;
        assert!(
            image.matches(state.entities.body(slot), row),
            "cow {id:?} state image diverged:\nlive {row:?}\nrecovered {image:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn portal_timer_scan_marks_only_real_changes() {
    let mut state = fixture();
    let quiet = spawn_idle_villager(&mut state, 8.5);
    let cooling = spawn_idle_villager(&mut state, 9.5);
    let decaying = spawn_idle_cow(&mut state, 10.5);
    let cooling_slot = state.entities.slot_of(cooling).unwrap();
    state.entities.row_mut(cooling_slot).entity_life.portal = crate::portals::PortalState {
        cooldown: 2,
        elapsed: 7,
    };
    let decaying_slot = state.entities.slot_of(decaying).unwrap();
    state
        .entities
        .row_mut(decaying_slot)
        .entity_life
        .portal
        .elapsed = 3;
    state.entities.take_touched();

    crate::portals::travel::finish(&mut state, &mut std::collections::BTreeMap::new());
    let mut touched = state.entities.take_touched();
    touched.sort_unstable();
    assert_eq!(
        touched,
        vec![cooling, decaying],
        "idle portal scan must not mark {quiet:?}"
    );
    assert_eq!(
        state.entities.row(cooling_slot).entity_life.portal,
        crate::portals::PortalState {
            cooldown: 1,
            elapsed: 0
        }
    );
    assert_eq!(
        state.entities.row(decaying_slot).entity_life.portal.elapsed,
        0
    );
}

#[test]
fn persistent_despawn_scan_marks_only_nonzero_idle_resets() {
    let mut state = fixture();
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    crate::tick::run_tick(
        &mut state,
        &[crate::boundary::PlayerInput::Join {
            connection: crate::boundary::ConnectionId(1),
            uuid: uuid::Uuid::from_u128(1),
            name: "JournalObserver".into(),
            permission_level: 0,
        }],
    );
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(chunk, 0)]));
    state
        .world
        .set_active_chunks([chunk].into_iter().collect(), state.tick);
    let quiet = spawn_idle_villager(&mut state, 8.5);
    let resetting = spawn_idle_villager(&mut state, 9.5);
    let slot = state.entities.slot_of(resetting).unwrap();
    state.entities.row_mut(slot).no_action_time = 20;
    state.tick = crate::natural_spawn::CYCLE;
    state.entities.take_touched();

    crate::natural_spawn::run(&mut state, &mut crate::boundary::ChangeSet::default());
    assert_eq!(
        state.entities.take_touched(),
        vec![resetting],
        "persistent idle resident {quiet:?} must stay untouched"
    );
    assert_eq!(state.entities.row(slot).no_action_time, 0);
}

#[test]
fn movement_writeback_marks_only_retained_air_changes() {
    let mut state = fixture();
    let cow = spawn_idle_cow(&mut state, 8.5);
    let slot = state.entities.slot_of(cow).unwrap();
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    journal_entries_for_tick(&mut state, &output);
    state.entities.set_motion_state(slot, [1.0, 0.0], 300, 4, 2);
    assert!(
        state.entities.touched_ids().is_empty(),
        "impulse, pickup and attack timers are not retained state-image keys"
    );
    assert!(image_ids(&journal_entries_for_tick(&mut state, &output)).is_empty());
    state.entities.set_motion_state(slot, [0.5, 0.0], 299, 3, 1);
    assert_eq!(state.entities.touched_ids(), &[cow]);
    assert_eq!(
        image_ids(&journal_entries_for_tick(&mut state, &output)),
        vec![cow.0]
    );
    assert_baselines_complete(&state, output.tick);
}

#[test]
fn queued_wal_images_share_baselines_but_clock_projection_cannot_mutate_them() {
    let mut state = fixture();
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state
        .tickets
        .insert("shared-life-clocks".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter([(chunk, 0)]));
    state.refresh_active_chunks(0);
    let cow = spawn_idle_cow(&mut state, 8.5);
    let slot = state.entities.slot_of(cow).unwrap();
    state.entities.row_mut(slot).breed_cooldown = 5;
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_spawned
        .push(cow);
    let queued = journal_entries_for_tick(&mut state, &output);
    let full = queued
        .iter()
        .find_map(|entry| match entry {
            JournalEntry::Entity { id, state, .. } if *id == cow.0 => Some(state),
            _ => None,
        })
        .unwrap();
    assert!(Arc::ptr_eq(full, &state.journaled_entity_state[&cow].2));
    let original_json = serde_json::to_vec(&queued).unwrap();
    state.entities.row_mut(slot).protected_until = 999;
    let changed = step(&mut state);
    let image = changed
        .iter()
        .find_map(|entry| match entry {
            JournalEntry::EntityState { id, state, .. } if *id == cow.0 => Some(state),
            _ => None,
        })
        .unwrap();
    assert!(Arc::ptr_eq(image, &state.journaled_entity_state[&cow].2));
    assert_eq!(image.breed_cooldown, 3);
    assert_eq!(full.breed_cooldown, 4);
    assert_eq!(serde_json::to_vec(&queued).unwrap(), original_json);
    assert!(!Arc::ptr_eq(image, full));
}

/// Journal-only costs; movement/writeback setup and output disposal are excluded.
#[test]
#[ignore = "100k-row release microbenchmark; run through mem-run -p"]
fn moving_rows_journal_microbench() {
    const COUNT: usize = 100_000;
    const WARMUP: usize = 4;
    const SAMPLES: usize = 24;
    let mut state = fixture();
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    for _ in 0..COUNT {
        spawn_idle_cow(&mut state, 8.5);
    }
    let mut entries = Vec::new();
    entity_images(&mut state, &output, &mut entries);
    assert_eq!(entries.len(), COUNT);
    println!(
        "journal_microbench rows={COUNT} baseline_value_bytes={}",
        std::mem::size_of_val(state.journaled_entity_state.values().next().unwrap()),
    );
    for mode in ["pose", "transient", "retained"] {
        let mut samples = Vec::with_capacity(SAMPLES);
        let mut mutation_samples = Vec::with_capacity(SAMPLES);
        for iteration in 0..WARMUP + SAMPLES {
            entries.clear();
            output.tick += 1;
            let phase = (iteration % 2 + 1) as u32;
            let start = std::time::Instant::now();
            for index in 0..COUNT {
                let slot = EntitySlot(index as u32);
                state
                    .entities
                    .set_pos(slot, [8.5 + f64::from(phase) * 0.1, 65.0, 9.5]);
                state.entities.set_vel(slot, [f64::from(phase), 0.0, 0.0]);
                state.entities.set_rot(slot, phase as f32, 0.0);
                if mode != "pose" {
                    state.entities.set_motion_state(
                        slot,
                        [f64::from(phase), 0.0],
                        if mode == "retained" {
                            298 + phase as i32
                        } else {
                            300
                        },
                        phase,
                        phase,
                    );
                }
            }
            let mutation_us = start.elapsed().as_micros() as u64;
            let start = std::time::Instant::now();
            entity_images(&mut state, &output, &mut entries);
            let journal_us = start.elapsed().as_micros() as u64;
            assert_eq!(entries.len(), if mode == "retained" { COUNT } else { 0 });
            std::hint::black_box(&entries);
            if iteration >= WARMUP {
                samples.push(journal_us);
                mutation_samples.push(mutation_us);
            }
        }
        samples.sort_unstable();
        mutation_samples.sort_unstable();
        println!(
            "journal_microbench mode={mode} rows={COUNT} samples={SAMPLES} journal_p50_us={} journal_p95_us={} mutation_p50_us={}",
            samples[SAMPLES / 2],
            samples[SAMPLES * 95 / 100],
            mutation_samples[SAMPLES / 2],
        );
    }
}

#[test]
#[ignore = "eight-species owner attribution; run with SOLARIS_TRACE_ROW_MUT=1"]
fn mixed_rows_journal_attribution() {
    let mut state = fixture();
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state
        .tickets
        .insert("mixed-owner-attribution".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    state.refresh_active_chunks(0);
    for z in 0..16 {
        for x in 0..16 {
            state.world.set_block(x, 64, z, state.registry.states.stone);
        }
    }
    for name in [
        "chicken", "cow", "pig", "sheep", "creeper", "skeleton", "spider", "zombie",
    ] {
        let type_id = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap();
        for _ in 0..100 {
            let id = state.spawn_mob(type_id, [8.5, 65.0, 8.5], 20.0);
            let slot = state.entities.slot_of(id).unwrap();
            let row = state.entities.row_mut(slot);
            row.persistent = true;
            row.speed = 0.0;
        }
    }
    for iteration in 0..8 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        if iteration == 7 {
            let mut by_type = std::collections::BTreeMap::<String, usize>::new();
            for id in state.entities.touched_ids() {
                let slot = state.entities.slot_of_any(*id).unwrap();
                if let EntityKind::Mob { type_id } = state.entities.body(slot).kind {
                    let name = state
                        .registry
                        .entity_types
                        .name_of(type_id)
                        .unwrap()
                        .to_string();
                    *by_type.entry(name).or_default() += 1;
                }
            }
            println!(
                "journal_attribution owner_marks={by_type:?} dead_recheck={}",
                state.journal_scan.clock_recheck.len()
            );
        }
        let entries = journal_entries_for_tick(&mut state, &output);
        assert_baselines_complete(&state, output.tick);
        if iteration == 7 {
            let compared = state
                .journaled_entity_state
                .values()
                .filter(|(tick, _, _)| *tick == output.tick)
                .count();
            println!(
                "journal_attribution compared={compared} wal_images={}",
                image_ids(&entries).len()
            );
            for ((file, line), [calls, villagers, first]) in state.entities.take_touch_sites() {
                println!(
                    "journal_attribution owner={file}:{line} calls={calls} villagers={villagers} first={first}"
                );
            }
        } else {
            state.entities.take_touch_sites();
        }
    }
}

#[test]
#[ignore = "100k-row activity churn microbenchmark; run through mem-run -p"]
fn activity_churn_journal_microbench() {
    const COUNT: usize = 100_000;
    const SAMPLES: usize = 24;
    let mut state = fixture();
    let body = crate::boundary::ChunkPos { x: 0, z: 0 };
    state
        .tickets
        .insert("activity-churn-body".into(), (body, 0));
    state
        .world
        .ensure_blocking(&[(body, 0)].into_iter().collect());
    state.refresh_active_chunks(0);
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let species: Vec<_> = [
        "chicken", "cow", "pig", "sheep", "creeper", "skeleton", "spider", "zombie",
    ]
    .iter()
    .map(|name| {
        state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap()
    })
    .collect();
    for index in 0..COUNT {
        let id = state.spawn_mob(species[index % species.len()], [8.5, 65.0, 8.5], 20.0);
        let slot = state.entities.slot_of(id).unwrap();
        let row = state.entities.row_mut(slot);
        row.persistent = true;
        row.controls.no_ai = true;
        row.entity_life.harvest_ticks = 100_000;
    }
    let mut entries = Vec::new();
    entity_images(&mut state, &output, &mut entries);
    state.world.journal_debt(&mut Vec::new());
    for mode in ["residency", "activity"] {
        let mut samples = Vec::with_capacity(SAMPLES);
        let mut compared = 0;
        for iteration in 0..4 + SAMPLES {
            entries.clear();
            output.tick += 1;
            state.tick = output.tick;
            let other = crate::boundary::ChunkPos {
                x: 100 + iteration as i32,
                z: 100,
            };
            state
                .world
                .ensure_blocking(&[(other, 0)].into_iter().collect());
            state.world.set_active_chunks(
                if mode == "activity" {
                    [body, other].into_iter().collect()
                } else {
                    [body].into_iter().collect()
                },
                output.tick,
            );
            crate::entity_life::advance(&mut state);
            assert!(state.entities.touched_ids().is_empty());
            let start = std::time::Instant::now();
            entity_images(&mut state, &output, &mut entries);
            let elapsed = start.elapsed().as_micros() as u64;
            assert!(
                entries.is_empty(),
                "unrelated activity changes emit no entity images"
            );
            compared = state
                .journaled_entity_state
                .values()
                .filter(|(tick, _, _)| *tick == output.tick)
                .count();
            state.world.journal_debt(&mut Vec::new());
            if iteration >= 4 {
                samples.push(elapsed);
            }
        }
        samples.sort_unstable();
        println!(
            "journal_activity_microbench mode={mode} rows={COUNT} compared={compared} samples={SAMPLES} journal_p50_us={} journal_p95_us={}",
            samples[SAMPLES / 2],
            samples[SAMPLES * 95 / 100]
        );
    }
}

#[test]
fn chunk_clock_boundaries_rebase_only_their_own_entities() {
    let mut state = fixture();
    let body = crate::boundary::ChunkPos { x: 0, z: 0 };
    let other = crate::boundary::ChunkPos { x: 1, z: 0 };
    state.tickets.insert("clock-body".into(), (body, 0));
    state.tickets.insert("clock-other".into(), (other, 0));
    state
        .world
        .ensure_blocking(&[(body, 0), (other, 0)].into_iter().collect());
    state.refresh_active_chunks(0);
    let cow = spawn_idle_cow(&mut state, 8.5);
    let moving = spawn_idle_cow(&mut state, 24.5);
    for id in [cow, moving] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).breed_cooldown = 100;
    }
    step(&mut state);
    let anchor = state.tick;
    let unrelated = crate::boundary::ChunkPos { x: 100, z: 100 };
    state
        .world
        .ensure_blocking(&[(unrelated, 0)].into_iter().collect());
    assert!(image_ids(&step(&mut state)).is_empty());
    for id in [cow, moving] {
        assert_eq!(
            state.journaled_entity_state[&id].0, anchor,
            "unrelated residency is not a clock boundary"
        );
    }
    state.tickets.remove("clock-other");
    assert!(image_ids(&step(&mut state)).is_empty());
    assert_eq!(state.journaled_entity_state[&moving].0, state.tick);
    assert_eq!(state.journaled_entity_state[&cow].0, anchor);
    for _ in 0..3 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        state.world.consume_simulation_debt(body, 1);
        assert_eq!(
            state.world.simulation_interval(body, anchor, output.tick),
            Some((anchor, output.tick)),
            "consuming debt must not reset a continuously active lazy projection",
        );
        let entries = journal_entries_for_tick(&mut state, &output);
        assert!(image_ids(&entries).is_empty());
        assert_baselines_complete(&state, output.tick);
        assert_eq!(state.journaled_entity_state[&cow].0, anchor);
    }
    state.tickets.insert("clock-other".into(), (other, 0));
    assert!(image_ids(&step(&mut state)).is_empty());
    assert_eq!(state.journaled_entity_state[&moving].0, state.tick);
    let slot = state.entities.slot_of(moving).unwrap();
    state.entities.set_pos(slot, [8.5, 65.0, 9.5]);
    assert_eq!(image_ids(&step(&mut state)), vec![moving.0]);
    let moved_anchor = state.tick;
    state.tickets.remove("clock-other");
    assert!(image_ids(&step(&mut state)).is_empty());
    assert_eq!(
        state.journaled_entity_state[&moving].0, moved_anchor,
        "crossing updates the existing chunk subscription"
    );
}

#[test]
fn idle_creeper_metadata_reads_do_not_mark_unchanged_rows() {
    let mut state = fixture();
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state.tickets.insert("idle-creeper".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    state.refresh_active_chunks(0);
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:creeper").unwrap())
        .unwrap();
    let id = state.spawn_mob(type_id, [8.5, 65.0, 8.5], 20.0);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).persistent = true;
    step(&mut state);
    let specials = [(id, crate::hostile::Special::Swell)];
    crate::hostile::run(&mut state, &mut Vec::new(), &specials);
    assert!(
        state.entities.touched_ids().is_empty(),
        "idle default metadata emits no dirty mark"
    );
    let extra = &mut Arc::make_mut(&mut state.entities.row_mut(slot).appearance).extra;
    extra.retain(|(index, _)| *index != crate::hostile::CREEPER_SWELL_DIR_INDEX);
    extra.push((
        crate::hostile::CREEPER_SWELL_DIR_INDEX,
        crate::boundary::EntityDataValue::VarInt(1),
    ));
    state.entities.take_touched();
    crate::hostile::run(&mut state, &mut Vec::new(), &specials);
    assert_eq!(
        state.entities.touched_ids(),
        &[id],
        "a real metadata change still publishes its wire candidate"
    );
    assert!(state.entities.row(slot).appearance.extra.contains(&(
        crate::hostile::CREEPER_SWELL_DIR_INDEX,
        crate::boundary::EntityDataValue::VarInt(-1)
    ),));
}

#[test]
fn quarantined_chunk_notifies_only_its_resident_journal_anchors() {
    let mut state = fixture();
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    state.tickets.insert("quarantine-clock".into(), (chunk, 0));
    state
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    state.refresh_active_chunks(0);
    let cow = spawn_idle_cow(&mut state, 8.5);
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    journal_entries_for_tick(&mut state, &output);
    output.tick += 1;
    state
        .world
        .reject_resident(chunk, "invalid retained metadata".into());
    assert!(image_ids(&journal_entries_for_tick(&mut state, &output)).is_empty());
    assert_eq!(state.journaled_entity_state[&cow].0, output.tick);
    assert_baselines_complete(&state, output.tick);
}

/// A restart journals no villager whose live state equals its restored
/// baseline (500k restored villagers: ~2.6 GB, 5.7 s first tick when every
/// one was re-imaged).
#[test]
fn restored_villagers_equal_to_their_baseline_are_not_reimaged_on_the_first_tick() {
    let mut source = fixture();
    let ids: Vec<EntityId> = (0..8)
        .map(|index| spawn_idle_villager(&mut source, 8.5 + f64::from(index)))
        .collect();
    let (output, _, _) = crate::tick::run_tick(&mut source, &[]);
    let entries = journal_entries_for_tick(&mut source, &output);
    assert_eq!(image_ids(&entries).len(), ids.len(), "source images them");
    let replay = replay_batches(
        Replay {
            tick: output.tick - 1,
            ..Replay::default()
        },
        &[(output.tick, output.tick, entries)],
    );
    let mut restored = fixture();
    restored.tick = output.tick;
    for id in &ids {
        crate::core::restore_entity(&mut restored, id.0, &replay.entities[&id.0]).unwrap();
    }
    assert_eq!(restored.journaled_villagers.len(), ids.len());
    let first = step(&mut restored);
    assert!(
        image_ids(&first).is_empty(),
        "restored villagers equal to their baseline stay out of the first batch"
    );
    // A real change after restore still journals.
    let slot = restored.entities.slot_of(ids[0]).unwrap();
    restored
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .xp += 5;
    assert_eq!(image_ids(&step(&mut restored)), vec![ids[0].0]);
}

#[test]
fn body_retained_fields_preserve_image_bytes_and_transient_fields_stay_out() {
    let mut state = fixture();
    let id = spawn_idle_cow(&mut state, 8.5);
    let slot = state.entities.slot_of(id).unwrap();
    let mut expected = crate::journal::EntityStateImage::from_row(
        state.entities.body(slot),
        state.entities.row(slot),
    );
    expected.air_supply = 123;
    expected.burn_ticks = 47;
    {
        let body = state.entities.body_mut(slot);
        body.air_supply = expected.air_supply;
        body.burn_ticks = expected.burn_ticks;
        body.pickup_delay = 19;
        body.attack_cooldown = 11;
        body.hurt_at = Some(0);
    }
    let image = crate::journal::EntityStateImage::from_row(
        state.entities.body(slot),
        state.entities.row(slot),
    );
    assert!(expected.matches(state.entities.body(slot), state.entities.row(slot)));
    assert_eq!(
        serde_json::to_vec(&image).unwrap(),
        serde_json::to_vec(&expected).unwrap(),
        "column relocation must not change persisted image bytes"
    );
    state.entities.body_mut(slot).air_supply += 1;
    assert!(!image.matches(state.entities.body(slot), state.entities.row(slot)));
    state.entities.body_mut(slot).air_supply -= 1;
    state.entities.body_mut(slot).burn_ticks += 1;
    assert!(!image.matches(state.entities.body(slot), state.entities.row(slot)));
}
