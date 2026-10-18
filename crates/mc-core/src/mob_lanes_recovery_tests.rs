use super::*;
use crate::boundary::{ChangeSet, TickOutput, WorldSnapshot};
use crate::dimensions::Dimension;
use crate::mob_lanes::tests::spawn_visit;

fn output(state: &TickState, changes: ChangeSet) -> TickOutput {
    TickOutput {
        tick: state.tick,
        snapshot: Arc::new(WorldSnapshot::default()),
        changes: Arc::new(changes),
        times: Default::default(),
    }
}
fn checkpoint(
    root: &std::path::Path,
    state: &mut TickState,
    spawned: Vec<crate::boundary::EntityId>,
) {
    let config = CoreConfig::new(root.to_owned(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    state.native_globals = store.read_checkpoint().unwrap().globals;
    // Align the world debt clock with the journal tick before capturing
    // post-images: a stale `sim_tick` stamps SimDebt outside the appended
    // batch and recovery rejects it as an invalid boundary.
    state.world.refresh_active_clock(state.tick);
    let changes = ChangeSet {
        entities_spawned: spawned,
        ..Default::default()
    };
    let out = output(state, changes);
    let entries = journal_entries_for_tick(state, &out);
    let mut writer = JournalWriter::open(root, 1).unwrap();
    writer.append(1, state.tick, &entries).unwrap();
    for dimension in [Dimension::Nether, Dimension::End] {
        let mut child = crate::core::recover_dimension_state(&config, dimension).unwrap();
        child.tick = state.tick;
        state.other_dimensions.insert(dimension, Box::new(child));
    }
    let (work_tx, work_rx) = crate::core::journal_work_channel();
    let (done, _) = std::sync::mpsc::channel();
    assert!(start_checkpoint(state, &work_tx, &done, state.tick));
    let JournalWork::Checkpoint(job) = work_rx.recv().unwrap() else {
        panic!()
    };
    let mut manifest = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
    write_checkpoint(root, &job, &state.plugins, &mut writer, &mut manifest).unwrap();
    state.other_dimensions.clear();
}

#[test]
fn wandering_trader_native_checkpoint_wal_restart_and_anvil_roundtrip() {
    let root = tempfile::tempdir().unwrap();
    let (mut state, _, resident) = crate::raids::tests::fixture(Some(root.path()));
    state.remove_entity(state.entities.slot_of(resident).unwrap());
    let spawned = spawn_visit(&mut state);
    let trader = state.entities.slot_of(spawned[0]).unwrap();
    let owner = state.entities.row(trader).uuid;
    let expected = state.entities.row(trader).villager.clone().unwrap();
    state.trader_spawner = crate::mob_lanes::TraderSpawner {
        delay: 12_345,
        chance: 50,
    };
    checkpoint(root.path(), &mut state, spawned.clone());
    let config = CoreConfig::new(root.path().to_owned(), 42);
    let mut restarted = recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(restarted.trader_spawner, state.trader_spawner);
    assert_eq!(
        restarted
            .entities
            .row(restarted.entities.slot_of_uuid(owner).unwrap())
            .villager
            .as_ref(),
        Some(&expected)
    );
    for llama in &spawned[1..] {
        assert_eq!(
            restarted
                .entities
                .row(restarted.entities.slot_of(*llama).unwrap())
                .entity_life
                .leash,
            Some(crate::leash::Holder::Entity(owner))
        );
    }
    restarted.tick += 1;
    // The tick bump above moves the journal batch forward; re-stamp the
    // restored debt images at the new tick so the tail stays inside it.
    restarted.world.refresh_active_clock(restarted.tick);
    restarted.trader_spawner.delay -= 1;
    let trader = restarted.entities.slot_of_uuid(owner).unwrap();
    restarted
        .entities
        .row_mut(trader)
        .villager
        .as_mut()
        .unwrap()
        .offers[0]
        .uses = 1;
    let out = output(&restarted, ChangeSet::default());
    let entries = journal_entries_for_tick(&mut restarted, &out);
    // Journal capture refreshes the derived active-brain flag.
    let expected = restarted.entities.row(trader).villager.clone().unwrap();
    let mut writer = JournalWriter::open(root.path(), 1).unwrap();
    writer
        .append(restarted.tick, restarted.tick, &entries)
        .unwrap();
    drop(writer);
    let clock = restarted.trader_spawner;
    drop(restarted);
    let recovered = recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(recovered.trader_spawner, clock);
    assert_eq!(
        recovered
            .entities
            .row(recovered.entities.slot_of_uuid(owner).unwrap())
            .villager
            .as_ref(),
        Some(&expected)
    );
    drop(recovered);
    drop(state);

    let parent = tempfile::tempdir().unwrap();
    let export = parent.path().join("java");
    crate::world_conversion::export_anvil(
        &config,
        root.path(),
        &export,
        &parent.path().join("export.json"),
    )
    .unwrap();
    let imported = parent.path().join("native");
    crate::world_conversion::import_anvil(
        &config,
        &export,
        &imported,
        &parent.path().join("import.json"),
    )
    .unwrap();
    let imported_config = CoreConfig::new(imported, 42);
    let mut recovered = recover_dimension_state(&imported_config, Dimension::Overworld).unwrap();
    assert_eq!(recovered.trader_spawner, clock);
    let trader = recovered.entities.slot_of_uuid(owner).unwrap();
    assert_eq!(
        recovered.entities.row(trader).villager.as_ref(),
        Some(&expected)
    );
    let deadline = expected.despawn_at.unwrap();
    recovered
        .gamerules
        .insert("spawn_wandering_traders".into(), "false".into());
    recovered.tick = deadline;
    let wanted = recovered
        .entities
        .slots()
        .map(|slot| {
            let pos = recovered.entities.pos(slot);
            (
                crate::boundary::ChunkPos {
                    x: (pos[0].floor() as i32).div_euclid(16),
                    z: (pos[2].floor() as i32).div_euclid(16),
                },
                0,
            )
        })
        .collect();
    recovered.world.ensure_blocking(&wanted);
    recovered
        .world
        .set_active_chunks(wanted.keys().copied().collect(), recovered.tick);
    let mut changes = ChangeSet::default();
    crate::mob_lanes::traders(&mut recovered, &mut changes);
    assert_eq!(changes.entities_removed.len(), 3);
    assert!(recovered.entities.slot_of_uuid(owner).is_none());
}
