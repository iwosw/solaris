use super::*;
use crate::boundary::{ClickMode, ConnectionId, Hand, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;
use mc_data::ItemStack;
use std::sync::{Arc, LazyLock};

const CHEST: BlockPos = BlockPos { x: 6, y: 200, z: 6 };
const COMPARATOR: BlockPos = BlockPos { x: 5, y: 200, z: 6 };
const CONNECTION: ConnectionId = ConnectionId(1);

struct Empty {
    air: BlockStateId,
}

impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            self.air,
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}

fn state_in(directory: Option<std::path::PathBuf>) -> TickState {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let air = BlockStateId(registry.states.air);
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(Empty { air });
    let mut world = if let Some(directory) = directory {
        let config = crate::CoreConfig::new(directory, 1);
        ChunkWorld::new_native(
            1,
            crate::native_checkpoint::tests::fixture_store(&config).unwrap(),
            crate::native_checkpoint::dimension_id(crate::dimensions::Dimension::Overworld),
            generator,
            &registry,
            air,
            [6, 200, 8],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            1,
            generator,
            &registry,
            air,
            [6, 200, 8],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    world.set_opaque_block_entity(BlockPos { x: 0, y: 200, z: 0 }, None);
    for x in 0..16 {
        for z in 0..16 {
            world.set_block(x, 199, z, registry.states.stone);
        }
    }
    world.fold_overlay();
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .tickets
        .insert("dirty-signals-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    state
}

fn state() -> TickState {
    state_in(None)
}

fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}

fn place(state: &mut TickState, pos: BlockPos, name: &str, props: &[(&str, &str)]) {
    let id = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
    let base = state.registry.blocks.block(&id).unwrap().default;
    let mut values = state
        .registry
        .blocks
        .by_id(base)
        .unwrap()
        .properties
        .clone();
    for (key, value) in props {
        values.iter_mut().find(|(name, _)| name == key).unwrap().1 = (*value).into();
    }
    let block = state
        .registry
        .blocks
        .by_name_and_props(&id, &values)
        .unwrap()
        .0;
    state.world.set_block(pos.x, pos.y, pos.z, block);
    state.world.fold_overlay();
    containers::register(state, pos);
}

fn stone(state: &TickState, count: i32) -> ItemStack {
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    ItemStack::new(item, count)
}

fn chest_and_comparator(state: &mut TickState, name: &str) {
    place(state, CHEST, name, &[]);
    place(state, COMPARATOR, "comparator", &[("facing", "east")]);
}

fn advance(state: &mut TickState, ticks: u64) {
    for _ in 0..ticks {
        run(state, &mut Vec::new(), &mut Vec::new());
        state.world.fold_overlay();
        state.tick += 1;
    }
}

fn join(state: &mut TickState, connection: ConnectionId) -> EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection,
            uuid: uuid::Uuid::new_v4(),
            name: format!("signals{}", connection.0),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(connection).unwrap();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [6.5, 200.0, 8.5]);
    player
}

fn open(state: &mut TickState, connection: ConnectionId, pos: BlockPos) {
    run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection,
            hand: Hand::Main,
            position: pos,
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    let player = state.connections.entity_of(connection).unwrap();
    assert!(
        state.players[&player].open.is_some(),
        "container must really open"
    );
    assert_eq!(state.open_containers.get(&player), Some(&key(pos)));
}

fn click(state: &mut TickState, player: EntityId, slot: i16) {
    let menu = state.players[&player].open.as_ref().unwrap();
    let input = PlayerInput::ContainerClick {
        connection: CONNECTION,
        window: menu.window,
        state_id: menu.state_id,
        slot,
        button: 0,
        mode: ClickMode::Pickup,
    };
    run_tick(state, &[input]);
}

fn close(state: &mut TickState, connection: ConnectionId) {
    let player = state.connections.entity_of(connection).unwrap();
    let window = state.players[&player].open.as_ref().unwrap().window;
    run_tick(state, &[PlayerInput::ContainerClose { connection, window }]);
    assert!(!state.open_containers.contains_key(&player));
}

fn command(state: &mut TickState, command: &str) {
    assert!(
        crate::commands::parse(command, true).is_ok(),
        "supported command"
    );
    run_tick(
        state,
        &[PlayerInput::Command {
            connection: CONNECTION,
            command: command.into(),
            permission_level: 4,
        }],
    );
}

fn power(state: &TickState, pos: BlockPos) -> u8 {
    prop(at(state, pos).unwrap(), "power")
        .unwrap()
        .parse()
        .unwrap()
}

/// Call after the dirty signal pass, before scheduled components and automation:
/// automation intentionally leaves its inventory changes for the next tick.
pub(super) fn assert_signal_cache_matches_full_scan(state: &TickState) {
    let expected: BTreeMap<_, _> = state
        .block_entities
        .keys()
        .map(|&(x, y, z)| {
            let pos = BlockPos { x, y, z };
            let signal = u16::from(containers::signal(state, pos)) << 8
                | u16::from(chest_viewers(state, pos));
            ((x, y, z), signal)
        })
        .collect();
    assert_eq!(
        state.redstone.signals, expected,
        "dirty cache diverged from full scan"
    );
}

#[test]
fn comparator_tracks_real_menu_pickup_and_return_without_explicit_wake() {
    let mut state = state();
    chest_and_comparator(&mut state, "chest");
    let stack = stone(&state, 64);
    state.block_entities.get_mut(&key(CHEST)).unwrap().slots[0] = stack.clone();
    let player = join(&mut state, CONNECTION);
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, COMPARATOR), 1);
    open(&mut state, CONNECTION, CHEST);
    click(&mut state, player, 0);
    assert!(state.block_entities[&key(CHEST)].slots[0].is_empty());
    assert_eq!(state.players[&player].open.as_ref().unwrap().carried, stack);
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, COMPARATOR), 0);
    assert_signal_cache_matches_full_scan(&state);
    click(&mut state, player, 0);
    assert_eq!(state.block_entities[&key(CHEST)].slots[0], stack);
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, COMPARATOR), 1);
    assert_signal_cache_matches_full_scan(&state);
}

#[test]
fn hopper_transfer_is_seen_by_signal_pass_on_the_next_tick_only() {
    let mut state = state();
    chest_and_comparator(&mut state, "chest");
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, COMPARATOR), 0);
    let hopper = BlockPos {
        y: CHEST.y + 1,
        ..CHEST
    };
    place(&mut state, hopper, "hopper", &[("facing", "down")]);
    let stack = stone(&state, 1);
    state.block_entities.get_mut(&key(hopper)).unwrap().slots[0] = stack.clone();
    advance(&mut state, 1);
    assert_eq!(state.block_entities[&key(CHEST)].slots[0], stack);
    assert!(state.block_entities[&key(hopper)].slots[0].is_empty());
    assert_eq!(state.redstone.signals[&key(CHEST)], 0);
    assert_eq!(comparator_output(&state, COMPARATOR), 0);
    advance(&mut state, 1);
    assert_eq!(state.redstone.signals[&key(CHEST)], 1 << 8);
    assert_eq!(state.redstone.signals[&key(hopper)], 0);
    assert_eq!(state.redstone.signal_recomputes, 2);
    advance(&mut state, 4);
    assert_eq!(comparator_output(&state, COMPARATOR), 1);
    assert_signal_cache_matches_full_scan(&state);
}

#[test]
fn slot_replacement_dirties_only_the_changed_container_and_idle_recomputes_nothing() {
    let mut state = state();
    chest_and_comparator(&mut state, "chest");
    for x in 8..14 {
        place(&mut state, BlockPos { x, ..CHEST }, "barrel", &[]);
    }
    advance(&mut state, 5);
    assert_eq!(state.redstone.signal_recomputes, 0);
    let stack = stone(&state, 64);
    state
        .block_entities
        .get_mut(&key(CHEST))
        .unwrap()
        .slots
        .fill(stack);
    advance(&mut state, 1);
    assert_eq!(state.redstone.signal_recomputes, 1);
    assert_eq!(state.redstone.signals[&key(CHEST)], 15 << 8);
    advance(&mut state, 4);
    assert_eq!(comparator_output(&state, COMPARATOR), 15);
    assert_eq!(state.redstone.signal_recomputes, 0);
    state
        .block_entities
        .get_mut(&key(CHEST))
        .unwrap()
        .slots
        .fill(ItemStack::EMPTY);
    advance(&mut state, 1);
    assert_eq!(state.redstone.signal_recomputes, 1);
    advance(&mut state, 4);
    assert_eq!(comparator_output(&state, COMPARATOR), 0);
    assert_signal_cache_matches_full_scan(&state);
}

#[test]
fn trapped_chest_real_open_close_and_disconnect_track_viewer_count() {
    let mut state = state();
    chest_and_comparator(&mut state, "trapped_chest");
    let wire = BlockPos {
        x: CHEST.x + 1,
        ..CHEST
    };
    place(&mut state, wire, "redstone_wire", &[]);
    let stack = stone(&state, 64);
    state
        .block_entities
        .get_mut(&key(CHEST))
        .unwrap()
        .slots
        .fill(stack);
    join(&mut state, CONNECTION);
    let second = ConnectionId(2);
    let second_player = join(&mut state, second);
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, COMPARATOR), 15);
    assert_eq!(power(&state, wire), 0);
    open(&mut state, CONNECTION, CHEST);
    advance(&mut state, 3);
    assert_eq!(power(&state, wire), 1);
    open(&mut state, second, CHEST);
    advance(&mut state, 3);
    assert_eq!(power(&state, wire), 2);
    close(&mut state, CONNECTION);
    advance(&mut state, 3);
    assert_eq!(power(&state, wire), 1);
    run_tick(&mut state, &[PlayerInput::Leave { connection: second }]);
    assert!(!state.open_containers.contains_key(&second_player));
    advance(&mut state, 3);
    assert_eq!(power(&state, wire), 0);
    assert_eq!(comparator_output(&state, COMPARATOR), 15);
    assert_signal_cache_matches_full_scan(&state);
}

fn pair(state: &mut TickState, name: &str) -> (BlockPos, BlockPos, BlockPos) {
    let right = BlockPos {
        x: CHEST.x + 1,
        ..CHEST
    };
    place(state, CHEST, name, &[("facing", "north"), ("type", "left")]);
    place(
        state,
        right,
        name,
        &[("facing", "north"), ("type", "right")],
    );
    let left_comparator = BlockPos {
        z: CHEST.z - 1,
        ..CHEST
    };
    let right_comparator = BlockPos {
        z: right.z - 1,
        ..right
    };
    for pos in [left_comparator, right_comparator] {
        place(state, pos, "comparator", &[("facing", "south")]);
    }
    (right, left_comparator, right_comparator)
}

#[test]
fn paired_chest_slot_replacement_recomputes_and_wakes_both_halves() {
    let mut state = state();
    let (right, left_comparator, right_comparator) = pair(&mut state, "chest");
    advance(&mut state, 5);
    let stack = stone(&state, 64);
    state
        .block_entities
        .get_mut(&key(CHEST))
        .unwrap()
        .slots
        .fill(stack);
    advance(&mut state, 1);
    assert_eq!(state.redstone.signal_recomputes, 2);
    assert_eq!(state.redstone.signals[&key(CHEST)], 8 << 8);
    assert_eq!(state.redstone.signals[&key(right)], 8 << 8);
    advance(&mut state, 4);
    assert_eq!(comparator_output(&state, left_comparator), 8);
    assert_eq!(comparator_output(&state, right_comparator), 8);
    assert_signal_cache_matches_full_scan(&state);
    advance(&mut state, 1);
    assert_eq!(state.redstone.signal_recomputes, 0);
}

#[test]
fn paired_chest_menu_mutation_updates_comparator_on_the_other_half() {
    let mut state = state();
    let (right, left_comparator, right_comparator) = pair(&mut state, "chest");
    let stack = stone(&state, 64);
    state
        .block_entities
        .get_mut(&key(CHEST))
        .unwrap()
        .slots
        .fill(stack);
    let player = join(&mut state, CONNECTION);
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, right_comparator), 8);
    open(&mut state, CONNECTION, right);
    // Vanilla menu order is the right half's 27 slots, then the left half.
    click(&mut state, player, 27);
    assert!(state.block_entities[&key(CHEST)].slots[0].is_empty());
    assert!(
        state.block_entities[&key(right)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, left_comparator), 7);
    assert_eq!(comparator_output(&state, right_comparator), 7);
    assert_signal_cache_matches_full_scan(&state);
}

#[test]
fn setblock_pair_shape_change_dirties_both_halves_without_inventory_changes() {
    let mut state = state();
    let (right, left_comparator, right_comparator) = pair(&mut state, "chest");
    let stack = stone(&state, 64);
    state
        .block_entities
        .get_mut(&key(CHEST))
        .unwrap()
        .slots
        .fill(stack.clone());
    join(&mut state, CONNECTION);
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, left_comparator), 8);
    assert_eq!(comparator_output(&state, right_comparator), 8);
    command(
        &mut state,
        "setblock 7 200 6 chest[facing=north,type=single]",
    );
    assert!(
        state.block_entities[&key(CHEST)]
            .slots
            .iter()
            .all(|slot| slot == &stack)
    );
    assert!(
        state.block_entities[&key(right)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, left_comparator), 15);
    assert_eq!(comparator_output(&state, right_comparator), 0);
    assert_signal_cache_matches_full_scan(&state);
}

#[test]
fn setblock_removal_forgets_cached_signal_and_unpowers_comparator() {
    let mut state = state();
    chest_and_comparator(&mut state, "chest");
    let stack = stone(&state, 64);
    state.block_entities.get_mut(&key(CHEST)).unwrap().slots[0] = stack;
    join(&mut state, CONNECTION);
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, COMPARATOR), 1);
    command(&mut state, "setblock 6 200 6 air");
    advance(&mut state, 5);
    assert!(!state.block_entities.contains_key(&key(CHEST)));
    assert!(!state.redstone.signals.contains_key(&key(CHEST)));
    assert_eq!(comparator_output(&state, COMPARATOR), 0);
    assert_signal_cache_matches_full_scan(&state);
}

#[test]
fn explosion_removal_forgets_cached_signal_and_unpowers_surviving_comparator() {
    let mut state = state();
    chest_and_comparator(&mut state, "chest");
    let stack = stone(&state, 64);
    state.block_entities.get_mut(&key(CHEST)).unwrap().slots[0] = stack;
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, COMPARATOR), 1);
    // Starting inside the chest destroys it immediately. Its resistance then
    // exhausts these short rays before they can reach the adjacent comparator.
    crate::explosion::apply(&mut state, None, [6.5, 200.5, 6.5], 1.5, None, None);
    run_tick(&mut state, &[]);
    assert_eq!(
        state.world.block_at(CHEST.x, CHEST.y, CHEST.z),
        Some(state.registry.states.air)
    );
    assert_eq!(
        at(&state, COMPARATOR).unwrap().block.id.path(),
        "comparator"
    );
    advance(&mut state, 5);
    assert!(!state.block_entities.contains_key(&key(CHEST)));
    assert!(!state.redstone.signals.contains_key(&key(CHEST)));
    assert_eq!(comparator_output(&state, COMPARATOR), 0);
    assert_signal_cache_matches_full_scan(&state);
}

#[test]
fn native_restart_recovers_pending_comparator_update_but_not_trapped_chest_viewers() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = state_in(Some(directory.path().to_owned()));
    chest_and_comparator(&mut state, "trapped_chest");
    let wire = BlockPos {
        x: CHEST.x + 1,
        ..CHEST
    };
    place(&mut state, wire, "redstone_wire", &[]);
    let stack = stone(&state, 64);
    state
        .block_entities
        .get_mut(&key(CHEST))
        .unwrap()
        .slots
        .fill(stack);
    let player = join(&mut state, CONNECTION);
    open(&mut state, CONNECTION, CHEST);
    advance(&mut state, 5);
    assert_eq!(comparator_output(&state, COMPARATOR), 15);
    assert_eq!(power(&state, wire), 1);
    click(&mut state, player, 0);
    assert!(state.block_entities[&key(CHEST)].slots[0].is_empty());
    assert_eq!(comparator_output(&state, COMPARATOR), 15);
    let records = state
        .block_entities
        .iter()
        .map(|(pos, record)| {
            (
                *pos,
                crate::journal::ReplayBlockEntity {
                    slots: record.slots.clone(),
                    furnace: record.furnace_image(),
                    tick: state.tick,
                },
            )
        })
        .collect();
    let absent = state.block_ticks.checkpoint(&mut state.world, state.tick);
    let mut capture = state.world.capture_dirty(&records);
    state
        .world
        .capture_scheduled_block_ticks(&mut capture, absent);
    capture.write().unwrap();
    drop(state);
    let mut restarted = state_in(Some(directory.path().to_owned()));
    assert!(restarted.open_containers.is_empty());
    run_tick(&mut restarted, &[]);
    assert!(restarted.block_entities[&key(CHEST)].slots[0].is_empty());
    advance(&mut restarted, 6);
    assert_eq!(comparator_output(&restarted, COMPARATOR), 14);
    assert_eq!(power(&restarted, wire), 0);
    assert_signal_cache_matches_full_scan(&restarted);
}

#[test]
fn paired_trapped_chest_real_lid_changes_dirty_both_halves() {
    let mut state = state();
    let (right, _, _) = pair(&mut state, "trapped_chest");
    let left_wire = BlockPos {
        z: CHEST.z + 1,
        ..CHEST
    };
    let right_wire = BlockPos {
        z: right.z + 1,
        ..right
    };
    for pos in [left_wire, right_wire] {
        place(&mut state, pos, "redstone_wire", &[]);
    }
    join(&mut state, CONNECTION);
    advance(&mut state, 5);
    open(&mut state, CONNECTION, right);
    advance(&mut state, 3);
    assert_eq!(state.redstone.signals[&key(CHEST)] & 0xff, 1);
    assert_eq!(state.redstone.signals[&key(right)] & 0xff, 1);
    assert_eq!(power(&state, left_wire), 1);
    assert_eq!(power(&state, right_wire), 1);
    assert_signal_cache_matches_full_scan(&state);
    close(&mut state, CONNECTION);
    advance(&mut state, 3);
    assert_eq!(state.redstone.signals[&key(CHEST)] & 0xff, 0);
    assert_eq!(state.redstone.signals[&key(right)] & 0xff, 0);
    assert_eq!(power(&state, left_wire), 0);
    assert_eq!(power(&state, right_wire), 0);
    assert_signal_cache_matches_full_scan(&state);
}

#[test]
fn cross_chunk_double_chest_residency_invalidates_the_resident_half() {
    for along_x in [true, false] {
        let mut state = state();
        let partner_chunk = ChunkPos {
            x: i32::from(along_x),
            z: i32::from(!along_x),
        };
        let all_chunks = [(ChunkPos { x: 0, z: 0 }, 0), (partner_chunk, 0)]
            .into_iter()
            .collect();
        state.world.ensure_blocking(&all_chunks);
        let left = BlockPos {
            x: if along_x { 15 } else { 6 },
            y: 200,
            z: if along_x { 6 } else { 15 },
        };
        let right = BlockPos {
            x: left.x + i32::from(along_x),
            z: left.z + i32::from(!along_x),
            ..left
        };
        let comparator = BlockPos {
            x: left.x - i32::from(along_x),
            z: left.z - i32::from(!along_x),
            ..left
        };
        let facing = if along_x { "north" } else { "east" };
        for (pos, kind) in [(left, "left"), (right, "right")] {
            place(
                &mut state,
                pos,
                "trapped_chest",
                &[("facing", facing), ("type", kind)],
            );
        }
        place(
            &mut state,
            comparator,
            "comparator",
            &[("facing", if along_x { "east" } else { "south" })],
        );
        let stack = stone(&state, 64);
        state
            .block_entities
            .get_mut(&key(left))
            .unwrap()
            .slots
            .fill(stack);
        state.open_containers.insert(EntityId(90), key(right));
        advance(&mut state, 5);
        assert_eq!(comparator_output(&state, comparator), 8);
        assert_eq!(state.redstone.signals[&key(left)] & 0xff, 1);

        let written: Vec<_> = state
            .world
            .snapshot()
            .iter()
            .map(|chunk| (chunk.pos, state.world.storage_revision(chunk.pos)))
            .collect();
        state.world.mark_flushed(&written);
        let resident = [(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect();
        assert_eq!(state.world.evict_outside(&resident), vec![partner_chunk]);
        advance(&mut state, 5);
        assert_eq!(comparator_output(&state, comparator), 15);
        assert_eq!(state.redstone.signals[&key(left)] & 0xff, 0);
        assert_eq!(state.redstone.signals[&key(right)], 0);
        // Signal residency must not reset the original lifetime hydration fence.
        assert!(state.redstone.loaded.contains(&partner_chunk));

        state.world.ensure_blocking(&all_chunks);
        advance(&mut state, 5);
        assert_eq!(comparator_output(&state, comparator), 8);
        assert_eq!(state.redstone.signals[&key(left)] & 0xff, 1);
        assert_signal_cache_matches_full_scan(&state);
    }
}
