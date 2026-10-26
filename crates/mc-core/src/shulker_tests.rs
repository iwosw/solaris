use super::*;
use crate::boundary::{ChangeSet, ClickMode, ConnectionId, EntityId, Hand, PlayerInput};
use crate::crafter::tests::{fixture, stack};
use crate::inventory::MenuKind;
use crate::tick::run_tick;
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };
const KEY: (i32, i32, i32) = (8, 201, 8);
fn command(state: &mut TickState, actor: EntityId, text: &str) {
    let mut words = text.split_whitespace();
    let root = words.next().unwrap();
    let parsed = crate::command_world::parse(root, &words.collect::<Vec<_>>()).unwrap();
    crate::command_world::execute(
        state,
        ConnectionId(1),
        actor,
        &parsed,
        &mut ChangeSet::default(),
    );
}
fn open(state: &mut TickState) {
    run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: POS,
            face: 3,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
}
fn click(state: &mut TickState, actor: EntityId, slot: i16, mode: ClickMode) {
    let menu = state.players[&actor].open.as_ref().unwrap();
    let input = PlayerInput::ContainerClick {
        connection: ConnectionId(1),
        window: menu.window,
        state_id: menu.state_id,
        slot,
        button: 0,
        mode,
    };
    run_tick(state, &[input]);
}
fn install(state: &mut TickState, actor: EntityId, path: &str) {
    command(state, actor, &format!("setblock 8 201 8 {path}"));
    run_tick(state, &[]);
}
#[test]
fn all_seventeen_shulker_blocks_setblock_save_native_id_and_open_native_menu() {
    for path in BlockEntityKind::ShulkerBox.block_paths() {
        let (mut state, actor) = fixture(None);
        install(&mut state, actor, path);
        let chunk = state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
        assert!(chunk.block_entities.contains_key(&POS), "{path}");
        let fields = mc_nbt::read_network(&mut chunk.block_entities[&POS].as_slice()).unwrap();
        let Tag::Compound(fields) = fields else {
            panic!();
        };
        assert_eq!(
            field(&fields, "id"),
            Some(&Tag::String("minecraft:shulker_box".into()))
        );
        assert!(
            field(&fields, "Items").is_none(),
            "native empty shulker omits Items"
        );
        open(&mut state);
        let menu = state.players[&actor].open.as_ref().unwrap();
        assert_eq!(menu.kind, MenuKind::ShulkerBox, "{path}");
        assert_eq!(menu.kind.menu_type(), Some(20));
        assert_eq!(menu.kind.slot_count(), 63);
        assert_eq!(state.block_entities[&KEY].slots.len(), 27);
        let records = mc_world::wire::client_block_entities(
            state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
            &state.registry.blocks,
            &state.registry.items,
        );
        assert_eq!(
            records
                .iter()
                .find(|record| record.pos == POS)
                .unwrap()
                .type_name
                .as_str(),
            "minecraft:shulker_box"
        );
    }
}
#[test]
fn shulker_menu_refuses_nested_boxes_for_pickup_and_quick_move_without_loss() {
    let (mut state, actor) = fixture(None);
    install(&mut state, actor, "black_shulker_box");
    open(&mut state);
    for path in BlockEntityKind::ShulkerBox.block_paths() {
        let item = stack(&state, path, 1);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = item.clone();
        click(&mut state, actor, 54, ClickMode::QuickMove);
        assert_eq!(state.players[&actor].inventory.slots[0], item);
        assert!(
            state.block_entities[&KEY]
                .slots
                .iter()
                .all(ItemStack::is_empty)
        );
        click(&mut state, actor, 54, ClickMode::Pickup);
        assert_eq!(state.players[&actor].open.as_ref().unwrap().carried, item);
        click(&mut state, actor, 0, ClickMode::Pickup);
        assert_eq!(state.players[&actor].open.as_ref().unwrap().carried, item);
        assert!(
            state.block_entities[&KEY]
                .slots
                .iter()
                .all(ItemStack::is_empty)
        );
        click(&mut state, actor, 54, ClickMode::Pickup);
        assert_eq!(state.players[&actor].inventory.slots[0], item);
    }
}
#[test]
fn filled_named_box_break_pickup_and_normal_place_restore_once_and_wal_replays() {
    let (mut state, actor) = fixture(None);
    install(&mut state, actor, "black_shulker_box");
    let mut fields = identity(POS);
    fields.push(("CustomName".into(), Tag::String("Treasure".into())));
    persist_fields(&mut state, POS, fields);
    open(&mut state);
    let mut diamond = stack(&state, "diamond", 17);
    diamond.custom_name = Some("kept exactly".into());
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = diamond.clone();
    click(&mut state, actor, 54, ClickMode::Pickup);
    click(&mut state, actor, 26, ClickMode::Pickup);
    assert_eq!(state.block_entities[&KEY].slots[26], diamond);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let before = state.block_entities[&KEY].slots.clone();
    let current = state.world.block_at(8, 201, 8).unwrap();
    let air = state.registry.states.air;
    let mut intents = vec![crate::intent::Intent {
        actor,
        seq: 1,
        kind: crate::intent::IntentKind::BreakBlock { pos: POS },
        footprint: crate::intent::Footprint {
            block_expect: vec![(POS, current)],
            block_write: vec![(POS, air)],
            ..Default::default()
        },
    }];
    assert_eq!(state.merge(&mut intents), 1);
    state.world.set_block(8, 201, 8, air);
    state.world.fold_overlay();
    let (_, _, spawned, _) = state.commit_merge_effects();
    let boxes: Vec<_> = spawned
        .into_iter()
        .filter(|id| {
            state
                .entities
                .row(state.entities.slot_of(*id).unwrap())
                .item
                .is_some()
        })
        .collect();
    assert_eq!(boxes.len(), 1, "one filled box, no loose diamonds");
    let item = boxes[0];
    let row = state.entities.row(state.entities.slot_of(item).unwrap());
    let dropped = row.item.as_ref().unwrap().stack.as_ref().unwrap();
    assert!(is_item(&state.registry.items, dropped));
    assert_eq!(dropped.count, 1);
    let dropped_fields = stack_to_fields(&state.registry.items, dropped).unwrap();
    let Some(Tag::Compound(components)) = field(&dropped_fields, "components") else {
        panic!();
    };
    assert_eq!(
        field(components, "minecraft:custom_name"),
        Some(&Tag::String("Treasure".into()))
    );
    let Some(Tag::List(contents)) = field(components, "minecraft:container") else {
        panic!();
    };
    assert_eq!(contents.elements.len(), 1);
    let Tag::Compound(entry) = &contents.elements[0] else {
        panic!();
    };
    assert_eq!(field(entry, "slot"), Some(&Tag::Int(26)));
    assert!(!state.block_entities.contains_key(&KEY));
    state.merge_block_writes.clear();
    let mut pickup = vec![crate::intent::Intent {
        actor,
        seq: 2,
        kind: crate::intent::IntentKind::PickupItem { item, count: 1 },
        footprint: crate::intent::Footprint {
            entity_reads: vec![item, actor],
            item_take: Some(item),
            ..Default::default()
        },
    }];
    assert_eq!(state.merge(&mut pickup), 1);
    let (_, _, _, removed) = state.commit_merge_effects();
    assert!(removed.contains(&item));
    assert!(state.entities.slot_of(item).is_none());
    assert!(is_item(
        &state.registry.items,
        &state.players[&actor].inventory.slots[0]
    ));
    let (output, _, _) = run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: BlockPos { x: 8, y: 200, z: 8 },
            face: 1,
            cursor: [0.5; 3],
            sequence: 2,
        }],
    );
    assert_eq!(state.block_entities[&KEY].slots, before);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let batches = vec![(state.tick, state.tick, entries)];
    let once = crate::journal::replay_batches(crate::journal::Replay::default(), &batches);
    let twice = crate::journal::replay_batches(
        crate::journal::replay_batches(crate::journal::Replay::default(), &batches),
        &batches,
    );
    assert_eq!(once.opaque_block_entities, twice.opaque_block_entities);
    let bytes = twice.opaque_block_entities[&KEY].as_ref().unwrap();
    assert_eq!(decode(bytes, &state.registry.items).unwrap(), before);
    let Tag::Compound(fields) = mc_nbt::read_network(&mut bytes.as_slice()).unwrap() else {
        panic!();
    };
    assert_eq!(
        field(&fields, "CustomName"),
        Some(&Tag::String("Treasure".into()))
    );
}
#[test]
fn shulker_checkpoint_native_restart_keeps_slot_twenty_six_and_components() {
    let dir = std::env::temp_dir().join(format!("solaris-shulker-{}", uuid::Uuid::new_v4()));
    let (mut state, actor) = fixture(Some(dir.clone()));
    install(&mut state, actor, "red_shulker_box");
    let mut item = stack(&state, "diamond", 29);
    item.custom_name = Some("saved stack".into());
    state.block_entities.get_mut(&KEY).unwrap().slots[26] = item;
    let expected = state.block_entities[&KEY].slots.clone();
    checkpoint(&mut state);
    let images = crate::core::checkpoint_data_for(&state, state.tick).2;
    state.world.capture_dirty(&images).write().unwrap();
    let registry = std::sync::Arc::clone(&state.registry);
    let generator = std::sync::Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        std::sync::Arc::clone(&registry.blocks),
    ));
    let mut world = crate::world::ChunkWorld::new_native(
        42,
        crate::native_checkpoint::tests::fixture_store(&crate::CoreConfig::new(dir.clone(), 42))
            .unwrap(),
        0,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 201, 10],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let chunk = world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    assert!(!chunk.chests.contains_key(&POS));
    assert!(!chunk.furnaces.contains_key(&POS));
    let custody = crate::redstone::custody::BlockEntities::default();
    let decoded = decode_chunk(&custody, &registry, chunk).unwrap();
    assert_eq!(decoded, [(KEY, expected)]);
    drop(state);
    drop(world);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn corrupt_shulker_record_fails_closed_with_exact_slot_diagnostic() {
    let (mut state, actor) = fixture(None);
    install(&mut state, actor, "blue_shulker_box");
    let mut fields = identity(POS);
    let mut item = stack_to_fields(&state.registry.items, &stack(&state, "diamond", 1)).unwrap();
    item.push(("Slot".into(), Tag::Byte(27)));
    fields.push((
        "Items".into(),
        Tag::List(mc_nbt::ListTag {
            element_type: mc_nbt::tag_type::COMPOUND,
            elements: vec![Tag::Compound(item)],
        }),
    ));
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).unwrap();
    assert_eq!(
        decode(&bytes, &state.registry.items)
            .unwrap_err()
            .to_string(),
        "storage: shulker box: Items[].Slot: expected 0..26"
    );
    state.world.set_opaque_block_entity(POS, Some(bytes));
    state.block_entities.remove(&KEY);
    run_tick(&mut state, &[]);
    assert!(!state.world.has_chunk(0, 0));
    assert!(!state.block_entities.contains_key(&KEY));
    assert!(
        state
            .world
            .load_error(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .reason
            .contains("shulker box: Items[].Slot: expected 0..26")
    );
}

#[test]
fn hydrated_custody_skips_cached_nbt_decode_and_cold_corruption_is_sticky() {
    let (mut state, actor) = fixture(None);
    install(&mut state, actor, "black_shulker_box");
    let diamond = stack(&state, "diamond", 9);
    state.block_entities.get_mut(&KEY).unwrap().slots[26] = diamond.clone();
    // A poisoned cached image would fail decode; live custody is authoritative.
    state.world.set_opaque_block_entity(POS, Some(vec![255]));
    assert!(
        decode_chunk(
            &state.block_entities,
            &state.registry,
            state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
        )
        .unwrap()
        .is_empty()
    );
    assert_eq!(state.block_entities[&KEY].slots[26], diamond);
    state.block_entities.remove(&KEY);
    run_tick(&mut state, &[]);
    let error = state
        .world
        .load_error(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .reason
        .clone();
    assert!(!state.world.has_chunk(0, 0));
    run_tick(&mut state, &[]);
    assert!(!state.world.has_chunk(0, 0));
    assert_eq!(
        state
            .world
            .load_error(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .reason,
        error
    );
}
