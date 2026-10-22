//! Farming on a dirt platform in an otherwise empty world, full vanilla
//! data (crop loot tables, hoe durability).

use std::sync::Arc;

use mc_world::BlockStateId;

use crate::boundary::{BlockPos, ConnectionId, DigAction, EntityId, Hand, PlayerInput};
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;

/// Platform height; its top face is `Y + 1`.
pub(crate) const Y: i32 = 200;
/// Every cell of every section ticks about once per tick.
const FAST: &str = "4096";

struct EmptyWorld {
    air: u32,
}

impl mc_world::ChunkGenerator for EmptyWorld {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(
            pos,
            BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").expect("plains"),
        );
        chunk.status = "minecraft:full".into();
        chunk.dirty = true;
        chunk
    }
}

/// A joined survival player standing on a 21x21 dirt platform around the
/// origin, with a contained water source at (5, Y, 2).
pub(crate) fn setup() -> (TickState, EntityId) {
    let registry = Arc::new(crate::tick_tests::full_registry());
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(EmptyWorld {
        air: registry.states.air,
    });
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        BlockStateId(registry.states.air),
        [0, Y + 1, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "farmer".to_owned(),
            permission_level: 0,
        }],
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let player = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("joined");
    let dirt = state.registry.states.dirt;
    for x in -10..=10 {
        for z in -10..=10 {
            state.world.set_block(x, Y, z, dirt);
        }
    }
    let stone = state.registry.states.stone;
    state.world.set_block(5, Y - 1, 2, stone);
    let water = state.registry.states.water_levels[0];
    state.world.set_block(5, Y, 2, water);
    state.world.fold_overlay();
    (state, player)
}

pub(crate) fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
    let id = mc_data::Identifier::parse(format!("minecraft:{name}")).expect("block id");
    let props: Vec<_> = props
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    state
        .registry
        .blocks
        .by_name_and_props(&id, &props)
        .expect("block state")
        .0
}

pub(crate) fn put(state: &mut TickState, x: i32, y: i32, z: i32, id: u32) {
    state.world.set_block(x, y, z, id);
    state.world.fold_overlay();
}

fn plant(state: &mut TickState, x: i32, y: i32, z: i32, name: &str, props: &[(&str, &str)]) {
    let id = block(state, name, props);
    put(state, x, y, z, id);
}

fn at(state: &TickState, x: i32, y: i32, z: i32) -> u32 {
    state.world.block_at(x, y, z).expect("resident")
}

fn give(state: &mut TickState, player: EntityId, item: &str, count: i32) {
    let id = mc_data::Identifier::parse(format!("minecraft:{item}")).expect("item id");
    let item = state.registry.items.id_of(&id).expect("item");
    state
        .players
        .get_mut(&player)
        .expect("player")
        .inventory
        .slots[0] = mc_data::ItemStack::new(item, count);
}

fn held(state: &TickState, player: EntityId) -> mc_data::ItemStack {
    state.players[&player].inventory.slots[0].clone()
}

fn use_on(state: &mut TickState, x: i32, y: i32, z: i32, face: u8) {
    let _ = run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: BlockPos { x, y, z },
            face,
            cursor: [0.5, 1.0, 0.5],
            sequence: 1,
        }],
    );
}

fn dig(state: &mut TickState, x: i32, y: i32, z: i32) {
    let _ = run_tick(
        state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: BlockPos { x, y, z },
            action: DigAction::Finish,
            sequence: 1,
        }],
    );
}

fn run(state: &mut TickState, ticks: usize) {
    for _ in 0..ticks {
        let _ = run_tick(state, &[]);
    }
}

/// Item entities on the ground as (item name, count), sorted.
fn dropped(state: &TickState) -> Vec<(String, i32)> {
    let mut out: Vec<(String, i32)> = state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .map(|stack| {
            let name = state.registry.items.name_of(stack.item_id).expect("item");
            (name.path().to_owned(), stack.count)
        })
        .collect();
    out.sort();
    out
}

#[test]
fn hoe_tills_dirt_and_grass_with_air_above_and_wears() {
    let (mut state, player) = setup();
    let farmland = block(&state, "farmland", &[("moisture", "0")]);
    give(&mut state, player, "wooden_hoe", 1);
    use_on(&mut state, 0, Y, 0, 1);
    assert_eq!(at(&state, 0, Y, 0), farmland, "dirt tilled");
    assert_eq!(held(&state, player).damage, Some(1), "one point of wear");

    let grass = state.registry.states.grass_block;
    put(&mut state, 1, Y, 0, grass);
    use_on(&mut state, 1, Y, 0, 1);
    assert_eq!(at(&state, 1, Y, 0), farmland, "grass tilled");
    assert_eq!(held(&state, player).damage, Some(2));

    let stone = state.registry.states.stone;
    let dirt = state.registry.states.dirt;
    put(&mut state, 2, Y, 0, dirt);
    put(&mut state, 2, Y + 1, 0, stone);
    use_on(&mut state, 2, Y, 0, 1);
    assert_eq!(
        at(&state, 2, Y, 0),
        state.registry.states.dirt,
        "covered dirt stays"
    );
    put(&mut state, 3, Y, 0, stone);
    use_on(&mut state, 3, Y, 0, 1);
    assert_eq!(at(&state, 3, Y, 0), stone, "stone is not tilled");
    assert_eq!(
        held(&state, player).damage,
        Some(2),
        "no wear without effect"
    );
}

#[test]
fn seeds_plant_on_farmland_not_on_dirt() {
    let (mut state, player) = setup();
    let air = state.registry.states.air;
    let farmland = block(&state, "farmland", &[("moisture", "0")]);
    put(&mut state, 1, Y, 0, farmland);
    put(&mut state, 2, Y, 0, farmland);
    put(&mut state, 3, Y, 0, farmland);
    put(&mut state, 4, Y, 0, farmland);

    give(&mut state, player, "wheat_seeds", 5);
    use_on(&mut state, 0, Y, 0, 1);
    assert_eq!(at(&state, 0, Y + 1, 0), air, "no seeds on dirt");
    assert_eq!(held(&state, player).count, 5, "nothing consumed");
    use_on(&mut state, 1, Y, 0, 1);
    assert_eq!(
        at(&state, 1, Y + 1, 0),
        block(&state, "wheat", &[("age", "0")])
    );
    assert_eq!(held(&state, player).count, 4);

    for (x, item, crop) in [
        (2, "carrot", "carrots"),
        (3, "potato", "potatoes"),
        (4, "beetroot_seeds", "beetroots"),
    ] {
        give(&mut state, player, item, 2);
        use_on(&mut state, x, Y, 0, 1);
        assert_eq!(
            at(&state, x, Y + 1, 0),
            block(&state, crop, &[("age", "0")]),
            "{item}"
        );
        assert_eq!(held(&state, player).count, 1);
    }

    give(&mut state, player, "wheat", 2);
    put(&mut state, 5, Y, 0, farmland);
    use_on(&mut state, 5, Y, 0, 1);
    assert_eq!(
        at(&state, 5, Y + 1, 0),
        air,
        "the wheat item places nothing"
    );
}

#[test]
fn random_ticks_grow_wheat_to_maturity_deterministically() {
    let ticks_to_mature = || {
        let (mut state, _) = setup();
        state
            .gamerules
            .insert("random_tick_speed".into(), FAST.into());
        let wet = block(&state, "farmland", &[("moisture", "7")]);
        for x in 1..=3 {
            for z in 1..=3 {
                put(&mut state, x, Y, z, wet);
            }
        }
        plant(&mut state, 2, Y + 1, 2, "wheat", &[("age", "0")]);
        let mature = block(&state, "wheat", &[("age", "7")]);
        for tick in 1..=600 {
            let _ = run_tick(&mut state, &[]);
            if at(&state, 2, Y + 1, 2) == mature {
                return tick;
            }
        }
        panic!("wheat did not mature in 600 ticks");
    };
    let first = ticks_to_mature();
    assert_eq!(first, ticks_to_mature(), "same world, same growth");
}

#[test]
fn default_random_tick_speed_is_three_cells_per_section() {
    let (mut state, _) = setup();
    let wheat = block(&state, "wheat", &[("age", "0")]);
    let wet = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, 2, Y, 2, wet);
    put(&mut state, 2, Y + 1, 2, wheat);
    run(&mut state, 50);
    // 3 samples of 4096 cells per tick leave a lone crop almost untouched.
    assert!(at(&state, 2, Y + 1, 2) != block(&state, "wheat", &[("age", "7")]));
}

#[test]
fn zero_random_tick_speed_freezes_crop_growth_and_farmland_drying() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), "0".into());
    let wheat = block(&state, "wheat", &[("age", "0")]);
    let dry = block(&state, "farmland", &[("moisture", "0")]);
    put(&mut state, -8, Y, -8, dry);
    put(&mut state, -8, Y + 1, -8, wheat);
    run(&mut state, 100);
    assert_eq!(at(&state, -8, Y, -8), dry);
    assert_eq!(at(&state, -8, Y + 1, -8), wheat);
}

#[test]
fn bone_meal_advances_crops_and_is_spent() {
    let (mut state, player) = setup();
    let farmland = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, 1, Y, 0, farmland);
    plant(&mut state, 1, Y + 1, 0, "wheat", &[("age", "0")]);
    give(&mut state, player, "bone_meal", 3);
    use_on(&mut state, 1, Y + 1, 0, 1);
    let aged = (2..=5)
        .find(|age| at(&state, 1, Y + 1, 0) == block(&state, "wheat", &[("age", &age.to_string())]))
        .expect("bone meal grows wheat by 2 to 5");
    assert!((2..=5).contains(&aged));
    assert_eq!(held(&state, player).count, 2, "one bone meal spent");

    plant(&mut state, 1, Y + 1, 0, "wheat", &[("age", "7")]);
    use_on(&mut state, 1, Y + 1, 0, 1);
    assert_eq!(held(&state, player).count, 2, "mature crops refuse it");
}

#[test]
fn mature_wheat_drops_wheat_and_seeds_immature_only_seeds() {
    let (mut state, _) = setup();
    let farmland = block(&state, "farmland", &[("moisture", "7")]);
    put(&mut state, 1, Y, 0, farmland);
    plant(&mut state, 1, Y + 1, 0, "wheat", &[("age", "7")]);
    dig(&mut state, 1, Y + 1, 0);
    let drops = dropped(&state);
    assert!(
        drops
            .iter()
            .any(|(name, count)| name == "wheat" && *count == 1),
        "mature wheat drops wheat: {drops:?}"
    );
    assert!(
        drops
            .iter()
            .all(|(name, _)| name == "wheat" || name == "wheat_seeds"),
        "{drops:?}"
    );

    let (mut state, _) = setup();
    put(&mut state, 1, Y, 0, farmland);
    plant(&mut state, 1, Y + 1, 0, "wheat", &[("age", "3")]);
    dig(&mut state, 1, Y + 1, 0);
    assert_eq!(dropped(&state), vec![("wheat_seeds".to_owned(), 1)]);
}

#[test]
fn farmland_near_water_soaks_and_far_dry_farmland_turns_to_dirt() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    let dry = block(&state, "farmland", &[("moisture", "0")]);
    put(&mut state, 2, Y, 2, dry);
    put(&mut state, -8, Y, -8, dry);
    put(&mut state, -8, Y, -6, dry);
    plant(&mut state, -8, Y + 1, -6, "wheat", &[("age", "0")]);
    run(&mut state, 20);
    assert_eq!(
        at(&state, 2, Y, 2),
        block(&state, "farmland", &[("moisture", "7")]),
        "water within four blocks"
    );
    assert_eq!(
        at(&state, -8, Y, -8),
        state.registry.states.dirt,
        "bare dry farmland dries out"
    );
    assert_eq!(at(&state, -8, Y, -6), dry, "a crop keeps its farmland");
}

#[test]
fn crops_off_farmland_pop_with_their_loot() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    plant(&mut state, 9, Y + 1, 9, "wheat", &[("age", "2")]);
    run(&mut state, 20);
    assert_eq!(at(&state, 9, Y + 1, 9), state.registry.states.air);
    assert_eq!(dropped(&state), vec![("wheat_seeds".to_owned(), 1)]);
}

#[test]
fn sapling_grows_an_oak_tree() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    plant(&mut state, -5, Y + 1, -5, "oak_sapling", &[("stage", "0")]);
    let log = block(&state, "oak_log", &[("axis", "y")]);
    for _ in 0..600 {
        let _ = run_tick(&mut state, &[]);
        if at(&state, -5, Y + 1, -5) == log {
            break;
        }
    }
    let trunk = (Y + 1..)
        .take_while(|y| at(&state, -5, *y, -5) == log)
        .count();
    assert!((4..=6).contains(&trunk), "trunk of {trunk} logs");
    let top = Y + trunk as i32;
    let leaves = |dx: i32, y: i32, dz: i32| {
        state
            .registry
            .blocks
            .by_id(BlockStateId(at(&state, -5 + dx, y, -5 + dz)))
            .is_some_and(|found| found.block.id.path() == "oak_leaves")
    };
    assert!(leaves(0, top + 1, 0), "crown above the trunk");
    assert!(leaves(2, top - 1, 0), "5x5 layer");
    assert!(!leaves(3, top - 1, 0), "no leaves beyond radius 2");
    assert_eq!(at(&state, -5, Y, -5), state.registry.states.dirt);
}

#[test]
fn grass_spreads_to_dirt_with_air_above() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    let grass = state.registry.states.grass_block;
    put(&mut state, -9, Y, -9, grass);
    let stone = state.registry.states.stone;
    put(&mut state, -8, Y + 1, -9, stone);
    run(&mut state, 30);
    assert_eq!(at(&state, -9, Y, -8), grass, "open dirt next to grass");
    assert_eq!(
        at(&state, -8, Y, -9),
        state.registry.states.dirt,
        "dirt with a block above stays dirt"
    );
}

#[test]
fn inert_section_becomes_tickable_only_after_overlay_commit() {
    let (mut state, _) = setup();
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    // Prime the negative eligibility cache on a section of inert dirt/air.
    run(&mut state, 1);
    let wheat = block(&state, "wheat", &[("age", "2")]);
    state.world.set_block(9, Y + 1, 9, wheat);
    let mut writes = Vec::new();
    let mut spawned = Vec::new();
    crate::farming::run(&mut state, &mut writes, &mut spawned);
    assert_eq!(
        at(&state, 9, Y + 1, 9),
        wheat,
        "same-tick overlay does not enable a previously inert section"
    );
    assert!(writes.is_empty());
    state.world.fold_overlay();
    run(&mut state, 20);
    assert_eq!(
        at(&state, 9, Y + 1, 9),
        state.registry.states.air,
        "new committed revision enables random ticks"
    );
    assert_eq!(dropped(&state), vec![("wheat_seeds".to_owned(), 1)]);
}

#[test]
fn open_sky_crops_grow_at_night_because_crops_gate_on_undarkened_light() {
    // Vanilla CropBlock.randomTick tests `getRawBrightness(pos, 0)`: no sky
    // darkening, so a night field under open sky keeps growing even though the
    // darkened spawn brightness there is 4.
    let (mut state, _) = setup();
    state.day_time = 18_000;
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state
        .gamerules
        .insert("random_tick_speed".into(), FAST.into());
    plant(&mut state, 0, Y, 0, "farmland", &[("moisture", "7")]);
    plant(&mut state, 0, Y + 1, 0, "wheat", &[("age", "0")]);
    let young = at(&state, 0, Y + 1, 0);
    assert_eq!(state.brightness(0, Y + 1, 0), 4);
    assert!(state.raw_brightness(0, Y + 1, 0) >= 9);
    run(&mut state, 40);
    assert_ne!(
        at(&state, 0, Y + 1, 0),
        young,
        "night does not stop an open-sky crop"
    );
}

#[path = "leaves_tests.rs"]
mod leaves_tests;
#[path = "plants_tests.rs"]
mod plants_tests;
#[path = "signs_tests.rs"]
mod signs_tests;
