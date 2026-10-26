use std::sync::{Arc, LazyLock};

use super::*;
use crate::boundary::{ChunkPos, ConnectionId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;

const POS: BlockPos = BlockPos { x: 0, y: 201, z: 0 };

fn state(dir: Option<std::path::PathBuf>) -> TickState {
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
            [0, 201, 2],
            mc_world::OVERWORLD_GEOMETRY,
        ),
        None => ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [0, 201, 2],
            mc_world::OVERWORLD_GEOMETRY,
        ),
    };
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .tickets
        .insert("campfire-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    state
}

fn fire(state: &mut TickState, name: &str, lit: bool) {
    let id = mc_data::Identifier::parse(name).unwrap();
    let block = state.registry.blocks.block(&id).unwrap();
    let mut properties = state
        .registry
        .blocks
        .by_id(block.default)
        .unwrap()
        .properties
        .clone();
    properties
        .iter_mut()
        .find(|(name, _)| name == "lit")
        .unwrap()
        .1 = lit.to_string();
    let id = state
        .registry
        .blocks
        .by_name_and_props(&id, &properties)
        .unwrap()
        .0;
    state
        .world
        .set_block(POS.x, POS.y - 1, POS.z, state.registry.states.stone);
    state.world.set_block(POS.x, POS.y, POS.z, id);
    state.world.fold_overlay();
}

fn chicken(state: &TickState, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:chicken").unwrap())
            .unwrap(),
        count,
    )
}

fn joined(state: &mut TickState, count: i32) -> EntityId {
    let _ = run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "cook".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [0.5, 201.0, 2.5]);
    let stack = chicken(state, count);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = stack;
    entity
}

fn use_input() -> PlayerInput {
    PlayerInput::UseItemOn {
        connection: ConnectionId(1),
        position: POS,
        hand: Hand::Main,
        face: 1,
        cursor: [0.5, 0.5, 0.5],
        sequence: 1,
    }
}

fn cooked(state: &TickState, drops: &[EntityId]) {
    assert_eq!(drops.len(), 1);
    let row = state
        .entities
        .row(state.entities.slot_of(drops[0]).unwrap());
    let item = row.item.as_ref().unwrap();
    assert_eq!(
        state.registry.items.name_of(item.item_id).unwrap().as_str(),
        "minecraft:cooked_chicken"
    );
    assert_eq!(item.count, 1);
}

#[test]
fn raw_chicken_cooks_at_recipe_time_and_syncs_items() {
    let mut state = state(None);
    let entity = joined(&mut state, 2);
    fire(&mut state, "minecraft:campfire", true);
    let (_, drains, _) = run_tick(&mut state, &[use_input()]);
    assert_eq!(state.players[&entity].inventory.slots[0].count, 1);
    assert_eq!(state.campfires[&key(POS)].total[0], 600);
    assert_eq!(state.campfires[&key(POS)].progress[0], 1);
    let sync = drains
        .iter()
        .flat_map(|(_, changes)| changes)
        .find_map(|change| match change {
            PlayerChange::BlockEntityData {
                position,
                block_entity_type: 33,
                nbt: Tag::Compound(fields),
                ..
            } if *position == POS => field(fields, "Items"),
            _ => None,
        })
        .unwrap();
    assert!(matches!(sync, Tag::List(list) if list.elements.len() == 1));
    let mut drops = Vec::new();
    for _ in 1..599 {
        run(&mut state, &mut drops);
    }
    assert!(drops.is_empty());
    assert!(!state.campfires[&key(POS)].slots[0].is_empty());
    let (output, drains, _) = run_tick(&mut state, &[]);
    cooked(&state, &output.changes.entities_spawned);
    assert!(
        state.campfires[&key(POS)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    assert!(drains.iter().flat_map(|(_, changes)| changes).any(|change| matches!(change,
        PlayerChange::BlockEntityData { nbt: Tag::Compound(fields), .. }
            if matches!(field(fields, "Items"), Some(Tag::List(list)) if list.elements.is_empty()))));
}

#[test]
fn unlit_campfires_preload_without_cooking_until_relit() {
    let mut state = state(None);
    let entity = joined(&mut state, 6);
    fire(&mut state, "minecraft:campfire", false);
    let _ = run_tick(&mut state, &[use_input()]);
    // Vanilla preloads one raw food while unlit; nothing cooks.
    assert_eq!(state.players[&entity].inventory.slots[0].count, 5);
    assert!(!state.campfires[&key(POS)].slots[0].is_empty());
    run(&mut state, &mut Vec::new());
    assert_eq!(state.campfires[&key(POS)].progress[0], 0);
    fire(&mut state, "minecraft:campfire", true);
    run(&mut state, &mut Vec::new());
    assert_eq!(state.campfires[&key(POS)].progress[0], 1);
}

#[test]
fn full_campfires_refuse_without_debit() {
    let mut state = state(None);
    let entity = joined(&mut state, 6);
    fire(&mut state, "minecraft:campfire", true);
    let stone = ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
            .unwrap(),
        1,
    );
    assert!(!use_food(&mut state, entity, Hand::Main, POS, &stone));
    fire(&mut state, "minecraft:campfire", true);
    for _ in 0..4 {
        let _ = run_tick(&mut state, &[use_input()]);
    }
    assert_eq!(state.players[&entity].inventory.slots[0].count, 2);
    assert!(
        state.campfires[&key(POS)]
            .slots
            .iter()
            .all(|stack| stack.count == 1)
    );
    let before = state.campfires[&key(POS)].slots.clone();
    let _ = run_tick(&mut state, &[use_input()]);
    assert_eq!(state.players[&entity].inventory.slots[0].count, 2);
    assert_eq!(state.campfires[&key(POS)].slots, before);
    fire(&mut state, "minecraft:campfire", false);
    let previous = state.campfires[&key(POS)].progress;
    run(&mut state, &mut Vec::new());
    assert_eq!(
        state.campfires[&key(POS)].progress,
        previous.map(|value| (value - 2).max(0))
    );
}

#[test]
fn cooking_slots_and_progress_survive_checkpoint() {
    let dir = std::env::temp_dir().join(format!("campfire-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    let _ = joined(&mut original, 1);
    fire(&mut original, "minecraft:soul_campfire", true);
    let _ = run_tick(&mut original, &[use_input()]);
    let mut drops = Vec::new();
    for _ in 1..137 {
        run(&mut original, &mut drops);
    }
    assert!(drops.is_empty());
    let before = original.campfires[&key(POS)].clone();
    checkpoint(&mut original);
    let capture = original.world.capture_dirty(&BTreeMap::new());
    assert!(!capture.write().unwrap().is_empty());
    drop(original);
    let mut restored = state(Some(dir.clone()));
    let chunk = restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    hydrate_chunk(&mut restored.campfires, &restored.registry, chunk);
    assert_eq!(restored.campfires[&key(POS)], before);
    assert_eq!(lit(&restored, POS), Some(true));
    for _ in 137..599 {
        run(&mut restored, &mut drops);
    }
    assert!(drops.is_empty());
    run(&mut restored, &mut drops);
    cooked(&restored, &drops);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn breaking_campfire_releases_uncooked_slots() {
    let mut state = state(None);
    let _ = joined(&mut state, 1);
    fire(&mut state, "minecraft:campfire", true);
    let _ = run_tick(&mut state, &[use_input()]);
    state
        .world
        .set_block(POS.x, POS.y, POS.z, state.registry.states.air);
    state.world.fold_overlay();
    let mut drops = Vec::new();
    run(&mut state, &mut drops);
    assert_eq!(drops.len(), 1);
    let item = state
        .entities
        .row(state.entities.slot_of(drops[0]).unwrap())
        .item
        .as_ref()
        .unwrap();
    assert_eq!(item.item_id, chicken(&state, 1).item_id);
    assert!(state.campfires.is_empty());
    assert!(
        !state
            .world
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities
            .contains_key(&POS)
    );
}

#[test]
fn campfire_food_insertion_uses_free_stock_without_consuming_the_hold() {
    let mut state = state(None);
    let actor = joined(&mut state, 2);
    fire(&mut state, "minecraft:campfire", true);
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "chicken",
        1,
    );
    let held = chicken(&state, 2);
    assert!(use_food(&mut state, actor, Hand::Main, POS, &held));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    let before = state.campfires[&key(POS)].slots.clone();
    let opaque_count = state.opaque_changes.len();
    let held = chicken(&state, 1);
    assert!(use_food(&mut state, actor, Hand::Main, POS, &held));
    assert_eq!(state.campfires[&key(POS)].slots, before);
    assert_eq!(state.players[&actor].inventory.slots[0], held);
    assert_eq!(state.opaque_changes.len(), opaque_count);
}

#[test]
fn resident_but_inactive_campfire_freezes_and_reactivation_resumes() {
    // G5 radius split: residency alone must not simulate.
    let mut state = state(None);
    let actor = joined(&mut state, 2);
    fire(&mut state, "minecraft:campfire", true);
    let (_, drains, _) = run_tick(&mut state, &[use_input()]);
    assert_eq!(state.campfires[&key(POS)].total[0], 600);
    let progress_live = state.campfires[&key(POS)].progress[0];
    assert!(progress_live > 0, "active chunk must cook: {progress_live}");
    // Deactivate the resident chunk: progress must freeze, drops stop.
    // run_tick recomputes active from players+tickets every tick, so drive
    // the freeze through the direct module gate instead.
    state
        .world
        .set_active_chunks(Default::default(), state.tick + 1);
    assert!(
        !state.world.is_active(0, 0),
        "fixture chunk must be resident-but-inactive"
    );
    assert!(state.world.has_chunk(0, 0), "chunk stays resident");
    let frozen = state.campfires[&key(POS)].progress[0];
    for _ in 0..8 {
        let mut drops = Vec::new();
        run(&mut state, &mut drops);
        assert!(drops.is_empty(), "inactive chunk must not cook");
    }
    assert_eq!(
        state.campfires[&key(POS)].progress[0],
        frozen,
        "progress froze while inactive"
    );
    // Reactivate: cooking resumes from the frozen image.
    state.world.set_active_chunks(
        [(ChunkPos { x: 0, z: 0 })].into_iter().collect(),
        state.tick + 1,
    );
    assert!(state.world.is_active(0, 0));
    let mut drops = Vec::new();
    run(&mut state, &mut drops);
    assert_eq!(
        state.campfires[&key(POS)].progress[0],
        frozen + 1,
        "progress resumes after reactivation"
    );
    let _ = (actor, drains);
}
