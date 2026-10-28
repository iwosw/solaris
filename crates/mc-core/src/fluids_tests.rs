//! Water/lava flow and buckets on a stone platform in open sky.

use std::sync::Arc;

use crate::boundary::{BlockPos, ConnectionId, EntityId, Hand, PlayerChange, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;

/// Platform height: far above the flat fixture floor.
const Y: i32 = 200;

fn test_state() -> TickState {
    // Wash regression assertions use the real vanilla torch/crop loot tables,
    // which are not included in the small embedded survival loot subset.
    let registry = Arc::new(crate::tick_tests::full_registry());
    let world = crate::horde_bench::flat_world(&registry);
    TickState::new(registry, world, 2, 512)
}

/// Join a player and build a 21x21 stone platform at [`Y`] under its
/// column; returns the player and the platform centre.
fn setup() -> (TickState, EntityId, BlockPos) {
    let mut state = test_state();
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "fluids".to_owned(),
            permission_level: 0,
        }],
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state.refresh_active_chunks(state.tick);
    let player = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("joined");
    let slot = state.entities.slot_of(player).expect("slot");
    let at = state.entities.pos(slot);
    let centre = BlockPos {
        x: at[0].floor() as i32,
        y: Y + 1,
        z: at[2].floor() as i32,
    };
    let stone = state.registry.states.stone;
    for dx in -10..=10 {
        for dz in -10..=10 {
            state
                .world
                .set_block(centre.x + dx, Y, centre.z + dz, stone);
        }
    }
    state.world.fold_overlay();
    (state, player, centre)
}

fn place(state: &mut TickState, pos: BlockPos, block: u32) {
    state.world.set_block(pos.x, pos.y, pos.z, block);
    state.world.fold_overlay();
    crate::fluids::schedule(state, &[(pos, block)]);
}

fn run(state: &mut TickState, ticks: usize) {
    for _ in 0..ticks {
        let _ = run_tick(state, &[]);
    }
}

fn at(state: &TickState, pos: BlockPos, dx: i32) -> u32 {
    state
        .world
        .block_at(pos.x + dx, pos.y, pos.z)
        .expect("resident")
}

#[test]
fn water_spreads_seven_blocks_and_recedes_without_its_source() {
    let (mut state, _, centre) = setup();
    let water = state.registry.states.water_levels;
    let air = state.registry.states.air;
    place(&mut state, centre, water[0]);
    run(&mut state, 60);
    for dx in 1..=7 {
        assert_eq!(at(&state, centre, dx), water[dx as usize], "level at +{dx}");
    }
    assert_eq!(at(&state, centre, 8), air, "spread stops after 7");
    place(&mut state, centre, air);
    run(&mut state, 60);
    for dx in 0..=8 {
        assert_eq!(at(&state, centre, dx), air, "receded at +{dx}");
    }
}

#[test]
fn two_water_sources_fill_the_cell_between_them() {
    let (mut state, _, centre) = setup();
    let source = state.registry.states.water_levels[0];
    place(
        &mut state,
        BlockPos {
            x: centre.x - 1,
            ..centre
        },
        source,
    );
    place(
        &mut state,
        BlockPos {
            x: centre.x + 1,
            ..centre
        },
        source,
    );
    run(&mut state, 20);
    assert_eq!(at(&state, centre, 0), source);
}

#[test]
fn lava_source_next_to_water_turns_to_obsidian_and_flowing_lava_to_cobblestone() {
    let (mut state, _, centre) = setup();
    let states = state.registry.states;
    place(&mut state, centre, states.lava_levels[0]);
    place(
        &mut state,
        BlockPos {
            x: centre.x + 2,
            ..centre
        },
        states.water_levels[0],
    );
    run(&mut state, 40);
    assert_eq!(at(&state, centre, 0), states.obsidian);

    // Flowing lava running into standing water hardens to cobblestone.
    let (mut state, _, centre) = setup();
    place(&mut state, centre, states.lava_levels[0]);
    run(&mut state, 35);
    assert_eq!(at(&state, centre, 1), states.lava_levels[2], "lava flowed");
    place(
        &mut state,
        BlockPos {
            x: centre.x + 2,
            ..centre
        },
        states.water_levels[0],
    );
    run(&mut state, 70);
    assert_eq!(at(&state, centre, 1), states.cobblestone);
}

#[test]
fn bucket_picks_up_a_source_and_pours_it_back() {
    let (mut state, player, centre) = setup();
    let states = state.registry.states;
    let item = |name: &str| {
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(name).expect("id"))
            .expect("item")
    };
    let (bucket, water_bucket) = (item("minecraft:bucket"), item("minecraft:water_bucket"));
    // A one-block pool sunk into the platform under the player.
    let pool = BlockPos { y: Y, ..centre };
    state.world.set_block(pool.x, Y - 1, pool.z, states.stone);
    state
        .world
        .set_block(pool.x, Y, pool.z, states.water_levels[0]);
    state.world.fold_overlay();
    let slot = state.entities.slot_of(player).expect("slot");
    state.entities.set_pos(
        slot,
        [
            f64::from(centre.x) + 0.5,
            f64::from(Y + 1),
            f64::from(centre.z) + 0.5,
        ],
    );
    state
        .players
        .get_mut(&player)
        .expect("player")
        .inventory
        .slots[0] = mc_data::ItemStack::new(bucket, 1);
    let use_down = |sequence| PlayerInput::UseItem {
        connection: ConnectionId(1),
        hand: Hand::Main,
        yaw: 0.0,
        pitch: 90.0,
        sequence,
    };
    let assert_ack = |outbox: Vec<(ConnectionId, Vec<PlayerChange>)>, expected| {
        let sequences: Vec<_> = outbox
            .into_iter()
            .filter(|(connection, _)| *connection == ConnectionId(1))
            .flat_map(|(_, changes)| changes)
            .filter_map(|change| match change {
                PlayerChange::BlockActionsAcked { sequence, .. } => Some(sequence),
                _ => None,
            })
            .collect();
        assert_eq!(sequences, vec![expected]);
    };
    let (_, outbox, _) = run_tick(&mut state, &[use_down(1)]);
    assert_ack(outbox, 1);
    assert_eq!(state.world.block_at(pool.x, Y, pool.z), Some(states.air));
    assert_eq!(
        state.players[&player].inventory.slots[0],
        mc_data::ItemStack::new(water_bucket, 1)
    );
    let (_, outbox, _) = run_tick(&mut state, &[use_down(2)]);
    assert_ack(outbox, 2);
    assert_eq!(
        state.world.block_at(pool.x, Y, pool.z),
        Some(states.water_levels[0])
    );
    assert_eq!(
        state.players[&player].inventory.slots[0],
        mc_data::ItemStack::new(bucket, 1)
    );
    // Looking into open sky refuses the empty bucket, but still acknowledges
    // the prediction sequence so the client can retire it.
    let (_, outbox, _) = run_tick(
        &mut state,
        &[PlayerInput::UseItem {
            connection: ConnectionId(1),
            hand: Hand::Main,
            yaw: 0.0,
            pitch: -90.0,
            sequence: 3,
        }],
    );
    assert_ack(outbox, 3);
    assert_eq!(
        state.players[&player].inventory.slots[0],
        mc_data::ItemStack::new(bucket, 1)
    );
}

fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(format!("minecraft:{name}")).expect("block id");
    let props = props
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect::<Vec<_>>();
    state
        .registry
        .blocks
        .by_name_and_props(&id, &props)
        .expect("block state")
        .0
}

#[test]
fn flowing_water_washes_torches_and_crops_and_drops_their_loot() {
    for (name, props, loot) in [
        ("torch", &[][..], "torch"),
        ("wheat", &[("age", "0")][..], "wheat_seeds"),
    ] {
        let (mut state, _, centre) = setup();
        let washed = block(&state, name, props);
        let source = state.registry.states.water_levels[0];
        let target = BlockPos {
            x: centre.x + 1,
            ..centre
        };
        place(&mut state, target, washed);
        place(&mut state, centre, source);
        let mut writes = Vec::new();
        let mut spawned = Vec::new();
        state.tick += 5;
        crate::fluids::run(&mut state, &mut writes, &mut spawned);
        assert_eq!(at(&state, centre, 1), state.registry.states.water_levels[1]);
        let drops: Vec<_> = spawned
            .iter()
            .map(|entity| {
                let slot = state.entities.slot_of(*entity).expect("spawned item");
                let stack = state.entities.row(slot).item.as_ref().expect("loot stack");
                (
                    state
                        .registry
                        .items
                        .name_of(stack.item_id)
                        .expect("item")
                        .path(),
                    stack.count,
                )
            })
            .collect();
        assert_eq!(drops, vec![(loot, 1)], "{name} loot");
    }
}

#[test]
fn flowing_water_preserves_stone_doors_and_waterloggable_blocks() {
    let (mut state, _, centre) = setup();
    for (name, props) in [
        ("stone", &[][..]),
        (
            "oak_door",
            &[
                ("facing", "north"),
                ("half", "lower"),
                ("hinge", "left"),
                ("open", "true"),
                ("powered", "false"),
            ][..],
        ),
        (
            "ladder",
            &[("facing", "north"), ("waterlogged", "false")][..],
        ),
    ] {
        let target = BlockPos {
            x: centre.x + 1,
            ..centre
        };
        let protected = block(&state, name, props);
        let source = state.registry.states.water_levels[0];
        place(&mut state, target, protected);
        place(&mut state, centre, source);
        let mut writes = Vec::new();
        let mut spawned = Vec::new();
        state.tick += 5;
        crate::fluids::run(&mut state, &mut writes, &mut spawned);
        assert_eq!(at(&state, centre, 1), protected, "{name} is not washed");
        assert!(spawned.is_empty(), "{name} has no loot drop");
    }
}

#[test]
fn two_water_sources_do_not_form_a_source_on_a_noncolliding_block() {
    let (mut state, _, centre) = setup();
    let water = state.registry.states.water_levels;
    let torch = block(&state, "torch", &[]);
    place(&mut state, BlockPos { y: Y, ..centre }, torch);
    // Queue the middle cell first so its regeneration checks the torch below
    // before the neighbouring sources wash it away.
    place(&mut state, centre, water[1]);
    place(
        &mut state,
        BlockPos {
            x: centre.x - 1,
            ..centre
        },
        water[0],
    );
    place(
        &mut state,
        BlockPos {
            x: centre.x + 1,
            ..centre
        },
        water[0],
    );
    state.tick += 5;
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    crate::fluids::run(&mut state, &mut writes, &mut spawned);
    assert_eq!(at(&state, centre, 0), water[1]);
}

#[test]
fn full_bucket_targets_a_washable_block_instead_of_the_cell_before_it() {
    let (mut state, _, centre) = setup();
    let torch = block(&state, "torch", &[]);
    place(&mut state, centre, torch);
    let eye = [
        f64::from(centre.x) + 0.5,
        f64::from(centre.y) + 0.5,
        f64::from(centre.z) - 1.5,
    ];
    assert_eq!(
        crate::fluids::bucket_target(&state, eye, 0.0, 0.0, Some(crate::fluids::Fluid::Water)),
        Some((centre, state.registry.states.water_levels[0]))
    );
}

#[path = "bottles_tests.rs"]
mod bottles;
#[path = "bubble_columns_tests.rs"]
mod bubble_columns;
#[path = "fluid_parity_tests.rs"]
mod parity;
#[path = "pov_ray_tests.rs"]
mod pov_ray;

fn use_block(state: &mut TickState, pos: BlockPos, face: u8, sequence: i32) {
    let _ = run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: pos,
            face,
            cursor: [0.5, 0.5, 0.5],
            sequence,
        }],
    );
}

fn hold(state: &mut TickState, player: EntityId, item: &str, count: i32) {
    let id = mc_data::Identifier::parse(format!("minecraft:{item}"))
        .ok()
        .and_then(|name| state.registry.items.id_of(&name))
        .expect("item");
    state
        .players
        .get_mut(&player)
        .expect("player")
        .inventory
        .slots[0] = mc_data::ItemStack::new(id, count);
}

fn held_item(state: &TickState, player: EntityId) -> mc_data::ItemStack {
    state.players[&player].inventory.slots[0].clone()
}

#[test]
fn empty_bucket_collects_full_powder_snow_cauldron_and_refills_it() {
    let (mut state, player, centre) = setup();
    let snow_cauldron = block(&state, "powder_snow_cauldron", &[("level", "3")]);
    let cauldron = block(&state, "cauldron", &[]);
    let at = BlockPos { y: Y + 1, ..centre };
    place(&mut state, at, snow_cauldron);
    let slot = state.entities.slot_of(player).expect("slot");
    state.entities.set_pos(
        slot,
        [
            f64::from(at.x) + 0.5,
            f64::from(at.y) + 1.0,
            f64::from(at.z) + 0.5,
        ],
    );
    hold(&mut state, player, "bucket", 1);
    use_block(&mut state, at, 1, 1);
    assert_eq!(state.world.block_at(at.x, at.y, at.z), Some(cauldron));
    assert_eq!(
        held_item(&state, player).item_id,
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:powder_snow_bucket").unwrap())
            .unwrap()
    );
    // Pour it back into the empty cauldron.
    use_block(&mut state, at, 1, 2);
    assert_eq!(state.world.block_at(at.x, at.y, at.z), Some(snow_cauldron));
    assert_eq!(
        held_item(&state, player).item_id,
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:bucket").unwrap())
            .unwrap()
    );
    // Partial snow refuses collection.
    let partial = block(&state, "powder_snow_cauldron", &[("level", "1")]);
    place(&mut state, at, partial);
    hold(&mut state, player, "bucket", 1);
    use_block(&mut state, at, 1, 3);
    assert_eq!(state.world.block_at(at.x, at.y, at.z), Some(partial));
}
