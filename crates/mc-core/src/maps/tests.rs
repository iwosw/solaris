use super::*;
use crate::boundary::{ChangeSet, Hand};
use crate::dimensions::Dimension;
use crate::maps::{Maps, SavedMap, map_id};

#[test]
fn framed_waypoints_survive_nbt_roundtrip() {
    // MAP-FRAME-WAYPOINT-PERSISTENCE: retained markers save/load.
    let mut map = SavedMap::new([0, 0], 0, Dimension::Overworld).unwrap();
    map.frame_markers.push((10, 20, 90));
    let tag = crate::maps::nbt::to_tag(&map).unwrap();
    let loaded = crate::maps::nbt::from_tag(&tag).unwrap();
    assert_eq!(loaded.frame_markers, vec![(10, 20, 90)]);
}

#[test]
fn saved_maps_creation_grid_colors_and_checkpoint_restart() {
    let mut state = test_state();
    let actor = join(&mut state, 1, "mapper");
    let empty = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:map").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::new(empty, 1);
    let mut changes = ChangeSet::default();
    assert!(crate::maps::use_empty(
        &mut state,
        actor,
        Hand::Main,
        &mut changes
    ));
    let id = map_id(&state.players[&actor].inventory.slots[0]).unwrap();
    assert_eq!(id, 0);
    let slot = state.entities.slot_of(actor).unwrap();
    let pos = state.entities.pos(slot);
    let map = &state.maps.records[&id];
    assert_eq!(
        map.center,
        SavedMap::new([pos[0] as i32, pos[2] as i32], 0, Dimension::Overworld)
            .unwrap()
            .center
    );
    let x = pos[0].floor() as i32;
    let z = pos[2].floor() as i32;
    let index = ((z - map.center[1] + 64) * 128 + x - map.center[0] + 64) as usize;
    let stone = state.registry.states.stone;
    // Same height to the north: flat shade (base + 1), not a lit slope.
    state.world.set_block(x, 100, z, stone);
    state.world.set_block(x, 100, z - 1, stone);
    state.maps.cursors.insert(id, index);
    state.tick = 10;
    crate::maps::tick(&mut state);
    assert_eq!(
        state.maps.records[&id].colors[index],
        state.registry.map_colors[stone as usize] * 4 + 1
    );
    let mut globals = std::collections::BTreeMap::new();
    crate::maps::checkpoint(&state.maps.records, state.tick, &mut globals).unwrap();
    let mut restored = Maps::restore(
        crate::maps::recover(&globals).unwrap(),
        crate::maps::sequence(&globals).unwrap(),
    )
    .unwrap();
    assert_eq!(restored.records, state.maps.records);
    assert_eq!(
        restored
            .allocate(SavedMap::new([0, 0], 0, Dimension::Overworld).unwrap())
            .unwrap(),
        1
    );
}

#[test]
fn saved_maps_grid_negative_edges_and_exploration_target() {
    assert_eq!(
        SavedMap::new([-65, -64], 0, Dimension::Overworld)
            .unwrap()
            .center,
        [-128, 0]
    );
    assert_eq!(
        SavedMap::new([64, 63], 0, Dimension::Overworld)
            .unwrap()
            .center,
        [128, 0]
    );
    let mut state = test_state();
    let destination = [1320, 64, -2432];
    let stack = crate::maps::create_exploration_map(
        &mut state,
        destination,
        Dimension::Overworld,
        "minecraft:mansion",
        2,
    )
    .unwrap();
    let map = &state.maps.records[&map_id(&stack).unwrap()];
    let marker = &map.decorations["destination"];
    assert_eq!(
        [marker.x, marker.z],
        [f64::from(destination[0]), f64::from(destination[2])]
    );
    assert_eq!(marker.kind, "minecraft:mansion");
    let tag = crate::maps::nbt::to_tag(map).unwrap();
    let decoded = crate::maps::nbt::from_tag(&tag).unwrap();
    assert_eq!(decoded.center, map.center);
    assert_eq!(decoded.colors, map.colors);
    let mut malformed = (**map).clone();
    malformed.colors.pop();
    assert!(malformed.validate().is_err());
}

struct ExplorerWorld {
    terrain: Arc<dyn mc_world::ChunkGenerator>,
    chest: u32,
    table: mc_data::Identifier,
    gate: parking_lot::Mutex<std::sync::mpsc::Receiver<()>>,
}
impl mc_world::ChunkGenerator for ExplorerWorld {
    fn generate(&self, pos: ChunkPos) -> mc_world::Chunk {
        let mut chunk = self.terrain.generate(pos);
        if pos == (ChunkPos { x: 0, z: 0 }) {
            chunk.set_block(0, 80, 0, mc_world::BlockStateId(self.chest));
            chunk.chests.insert(
                mc_world::BlockPos { x: 0, y: 80, z: 0 },
                mc_world::ChestBlockEntity {
                    loot_table: Some(self.table.clone()),
                    loot_seed: 17,
                    ..Default::default()
                },
            );
        }
        chunk
    }
    fn structure_sites_in_region(
        &self,
        _: (i32, i32),
        _: (i32, i32),
    ) -> Vec<mc_world::chunk::GeneratedStructureSite> {
        self.gate.lock().recv().unwrap();
        ["minecraft:monument", "minecraft:trial_chambers"]
            .into_iter()
            .map(|name| mc_world::chunk::GeneratedStructureSite {
                structure: mc_data::Identifier::parse(name).unwrap(),
                position: mc_world::BlockPos {
                    x: 328,
                    y: 64,
                    z: 8,
                },
                start_chunk: (20, 0),
            })
            .collect()
    }
}
fn explorer_fixture() -> (TickState, std::sync::mpsc::Sender<()>) {
    let mut registry = (*test_registry()).clone();
    let table_id = mc_data::Identifier::parse("minecraft:chests/map_regression").unwrap();
    let table = mc_data::loot::chest_26_1_2::ChestLootTable::compile(table_id.clone(),r#"{
        "type":"minecraft:chest","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:map",
        "functions":[{"function":"minecraft:exploration_map","destination":"minecraft:on_ocean_explorer_maps",
        "decoration":"minecraft:monument","zoom":2,"search_radius":100,"skip_existing_chunks":true}]}]}]}"#).unwrap();
    Arc::make_mut(&mut registry.chest_loot).insert(table);
    let chest = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:chest").unwrap())
        .unwrap()
        .default
        .0;
    let registry = Arc::new(registry);
    let base = flat_world(&registry);
    let (send, recv) = std::sync::mpsc::channel();
    let world = ChunkWorld::new(
        42,
        Arc::new(ExplorerWorld {
            terrain: base.generator(),
            chest,
            table: table_id,
            gate: parking_lot::Mutex::new(recv),
        }),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 81, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    join(&mut state, 1, "first");
    join(&mut state, 2, "second");
    (state, send)
}
fn open_input(connection: u64) -> PlayerInput {
    PlayerInput::UseItemOn {
        connection: ConnectionId(connection),
        hand: Hand::Main,
        position: crate::boundary::BlockPos { x: 0, y: 80, z: 0 },
        face: 1,
        cursor: [0.5; 3],
        sequence: 1,
    }
}
fn finish_lookup(state: &mut TickState) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !state.maps.pending_loot.is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "map worker did not complete"
        );
        crate::command_locate::drain(state);
        std::thread::yield_now();
    }
}
#[test]
fn saved_maps_two_first_opens_one_map_and_wal_restart_never_regenerates() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let (mut state, gate) = explorer_fixture();
    let cells = [(0, 80, 0)];
    assert!(crate::maps::loot::before_open(
        &mut state,
        ConnectionId(1),
        &open_input(1),
        &cells
    ));
    assert!(crate::maps::loot::before_open(
        &mut state,
        ConnectionId(2),
        &open_input(2),
        &cells
    ));
    assert_eq!(state.maps.pending_loot.len(), 1);
    assert_eq!(state.maps.next_id, 0);
    gate.send(()).unwrap();
    finish_lookup(&mut state);
    assert_eq!(state.maps.next_id, 1);
    let items = state.block_entities[&(0, 80, 0)].slots.clone();
    assert_eq!(
        items.iter().filter(|item| map_id(item).is_some()).count(),
        1
    );
    let map = &state.maps.records[&0];
    assert_eq!(map.destination, Some([328, 64, 8]));
    let root = tempfile::tempdir().unwrap();
    let checkpoint_tick = state.tick;
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let mut writer = JournalWriter::open(root.path(), 1).unwrap();
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches =
        read_complete_batches_after(&root.path().join("journal"), checkpoint_tick).unwrap();
    let replay = replay_batches(
        Replay {
            tick: checkpoint_tick,
            ..Default::default()
        },
        &batches,
    );
    assert_eq!(replay.saved_maps.len(), 1);
    assert_eq!(replay.block_entities[&(0, 80, 0)].slots, items);
    let (mut restarted, _gate) = explorer_fixture();
    restarted.maps = Maps::restore(replay.saved_maps, 0).unwrap();
    restarted.block_entities.insert(
        (0, 80, 0),
        crate::tick::BlockEntityState {
            slots: items.clone(),
            furnace: None,
        },
    );
    assert!(!crate::maps::loot::before_open(
        &mut restarted,
        ConnectionId(1),
        &open_input(1),
        &cells
    ));
    assert_eq!(restarted.maps.next_id, 1);
    assert_eq!(restarted.block_entities[&(0, 80, 0)].slots, items);
}
#[test]
fn saved_maps_broken_pending_chest_discards_lookup_without_allocating() {
    let (mut state, gate) = explorer_fixture();
    assert!(crate::maps::loot::before_open(
        &mut state,
        ConnectionId(1),
        &open_input(1),
        &[(0, 80, 0)]
    ));
    state.world.set_block(0, 80, 0, state.registry.states.air);
    gate.send(()).unwrap();
    finish_lookup(&mut state);
    assert!(state.maps.records.is_empty());
    assert_eq!(state.maps.next_id, 0);
    assert!(!state.block_entities.contains_key(&(0, 80, 0)));
}

#[test]
fn saved_maps_cartography_clone_scale_lock_consumes_one_and_cancels_without_ids() {
    use crate::inventory::MenuKind;
    for (ingredient, new_map, locked, scale, count) in [
        ("map", false, false, 0, 2),
        ("paper", true, false, 1, 1),
        ("glass_pane", true, true, 0, 1),
    ] {
        let mut state = test_state();
        let actor = join(&mut state, 1, "cartographer");
        let mut map = SavedMap::new([0, 0], 0, Dimension::Overworld).unwrap();
        map.tracking = false;
        map.unlimited_tracking = true;
        let original = state.maps.allocate(map).unwrap();
        let map_item = crate::maps::filled_map(&state, original).unwrap();
        let input = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{ingredient}")).unwrap())
            .unwrap();
        let registry = Arc::clone(&state.registry);
        let player = state.players.get_mut(&actor).unwrap();
        player.open_menu(
            &registry.item_rules(),
            MenuKind::Cartography,
            "Cartography".into(),
            &[],
        );
        let menu = player.open.as_mut().unwrap();
        menu.grid[0] = map_item;
        menu.grid[1] = mc_data::ItemStack::new(input, 3);
        let window = menu.window;
        let revision = menu.state_id;
        let mut changes = ChangeSet::default();
        // Wrong windows cannot allocate a map or debit input.
        crate::maps::cartography::click(
            &mut state,
            actor,
            (
                window.wrapping_add(1),
                revision,
                2,
                0,
                crate::boundary::ClickMode::Pickup,
            ),
            &mut changes,
        );
        assert_eq!(state.maps.next_id, 1);
        assert_eq!(
            state.players[&actor].open.as_ref().unwrap().grid[1].count,
            3
        );
        assert!(
            state.players[&actor]
                .open
                .as_ref()
                .unwrap()
                .carried
                .is_empty()
        );
        // Like other vanilla menus, a stale revision executes against current
        // server state and resynchronizes, rather than refusing the take.
        crate::maps::cartography::click(
            &mut state,
            actor,
            (
                window,
                revision - 1,
                2,
                0,
                crate::boundary::ClickMode::Pickup,
            ),
            &mut changes,
        );
        let menu = state.players[&actor].open.as_ref().unwrap();
        assert_eq!(menu.carried.count, count);
        assert_eq!(menu.grid[1].count, 2);
        assert!(menu.grid[0].is_empty());
        let id = map_id(&menu.carried).unwrap();
        assert_eq!(id, if new_map { 1 } else { 0 });
        assert_eq!(state.maps.records[&id].locked, locked);
        assert_eq!(state.maps.records[&id].scale, scale);
        assert!(!state.maps.records[&id].tracking);
        assert!(state.maps.records[&id].unlimited_tracking);
        assert_eq!(state.maps.next_id, if new_map { 2 } else { 1 });
    }
}

#[test]
fn saved_maps_cartographer_offer_and_map_share_replayed_wal_commit() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let (mut state, gate) = explorer_fixture();
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let merchant = state.spawn_mob(type_id, [2.0, 81.0, 0.0], 20.0);
    let slot = state.entities.slot_of(merchant).unwrap();
    let row = state.entities.row_mut(slot);
    let uuid = row.uuid;
    let villager = row.villager.as_mut().unwrap();
    villager.data.profession = crate::villager::VillagerProfession::Cartographer;
    villager.data.level = 3;
    villager.xp = 70;
    villager.offers.clear();
    state.maps.attempted_trades.insert((uuid, 2));
    let checkpoint_tick = state.tick;
    let root = tempfile::tempdir().unwrap();
    let mut writer = JournalWriter::open(root.path(), 1).unwrap();
    let (birth, _, _) = run_tick(&mut state, &[]);
    let birth_entries = crate::core::journal_entries_for_tick(&mut state, &birth);
    writer
        .append(birth.tick, birth.tick, &birth_entries)
        .unwrap();
    assert!(crate::maps::trades::before_open(
        &mut state,
        merchant,
        ConnectionId(1),
        Hand::Main
    ));
    assert!(crate::maps::trades::before_open(
        &mut state,
        merchant,
        ConnectionId(2),
        Hand::Main
    ));
    assert_eq!(state.maps.pending_trades.len(), 1);
    gate.send(()).unwrap();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !state.maps.pending_trades.is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "merchant map worker did not complete"
        );
        crate::command_locate::drain(&mut state);
        std::thread::yield_now();
    }
    assert!(state.appearance_changed.contains(&merchant));
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches =
        read_complete_batches_after(&root.path().join("journal"), checkpoint_tick).unwrap();
    let replay = replay_batches(
        Replay {
            tick: checkpoint_tick,
            ..Default::default()
        },
        &batches,
    );
    let villager = replay.entities[&merchant.0].villager.as_ref().unwrap();
    let offers: Vec<_> = villager
        .offers
        .iter()
        .filter_map(|offer| map_id(&offer.result))
        .collect();
    assert_eq!(offers, [0]);
    assert_eq!(
        replay.saved_maps[&offers[0]].destination,
        Some([328, 64, 8])
    );
    state.maps.attempted_trades.clear();
    state.maps.attempted_trades.insert((uuid, 2));
    assert!(!crate::maps::trades::before_open(
        &mut state,
        merchant,
        ConnectionId(1),
        Hand::Main
    ));
    assert_eq!(state.maps.next_id, 1);
}
