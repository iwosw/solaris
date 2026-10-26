use super::*;
use crate::boundary::ChunkPos;
use crate::crafter::tests::{block, fixture, stack};
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };
#[test]
fn all_twelve_wood_shelves_place_take_swap_and_comparator() {
    fn comparator_signal(state: &mut TickState) -> u8 {
        let comparator = BlockPos { x: 8, y: 201, z: 9 };
        let id = block(state, "comparator", &[("facing", "north")]);
        state
            .world
            .set_block(comparator.x, comparator.y, comparator.z, id);
        crate::redstone::schedule_cell(state, comparator, 1, 0);
        for _ in 0..4 {
            crate::tick::run_tick(state, &[]);
        }
        crate::redstone::output(
            state,
            comparator,
            BlockPos {
                x: 8,
                y: 201,
                z: 10,
            },
        )
    }
    for wood in [
        "oak", "spruce", "birch", "jungle", "acacia", "dark_oak", "mangrove", "cherry", "bamboo",
        "pale_oak", "crimson", "warped",
    ] {
        let (mut state, actor) = fixture(None);
        let id = block(&state, &format!("{wood}_shelf"), &[("facing", "south")]);
        state.world.set_block(8, 201, 8, id);
        state.world.fold_overlay();
        let diamond = stack(&state, "diamond", 64);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = diamond.clone();
        assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
        assert!(state.players[&actor].inventory.slots[0].is_empty());
        assert_eq!(state.block_entities[&key(POS)].slots[1], diamond);
        assert_eq!(comparator_signal(&mut state), 5);
        let emerald = stack(&state, "emerald", 8);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = emerald.clone();
        assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
        assert_eq!(state.players[&actor].inventory.slots[0], diamond);
        assert_eq!(state.block_entities[&key(POS)].slots[1], emerald);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
        assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
        assert_eq!(state.players[&actor].inventory.slots[0], emerald);
        assert_eq!(comparator_signal(&mut state), 0);
        assert!(!use_on(&mut state, actor, Hand::Off, POS, 3, [0.5; 3]));
        assert!(!use_on(&mut state, actor, Hand::Main, POS, 2, [0.5; 3]));
    }
}
#[test]
fn powered_chain_swaps_nine_hotbar_slots_and_disconnects_when_unpowered() {
    let (mut state, actor) = fixture(None);
    for (x, name) in [(7, "oak_shelf"), (8, "bamboo_shelf"), (9, "warped_shelf")] {
        let id = block(&state, name, &[("facing", "north")]);
        state.world.set_block(x, 201, 8, id);
        let power = block(&state, "redstone_block", &[]);
        state.world.set_block(x, 202, 8, power);
    }
    state.world.fold_overlay();
    for x in 7..10 {
        scheduled(&mut state, BlockPos { x, y: 201, z: 8 }, &mut Vec::new());
    }
    assert_eq!(
        chain(&state, POS),
        vec![
            BlockPos { x: 9, y: 201, z: 8 },
            POS,
            BlockPos { x: 7, y: 201, z: 8 }
        ]
    );
    for hotbar in 0..9 {
        let item = stack(&state, "diamond", hotbar as i32 + 1);
        state.players.get_mut(&actor).unwrap().inventory.slots[hotbar] = item;
    }
    let original = state.players[&actor].inventory.slots[..9].to_vec();
    assert!(use_on(&mut state, actor, Hand::Main, POS, 2, [0.5; 3]));
    assert!(
        state.players[&actor].inventory.slots[..9]
            .iter()
            .all(ItemStack::is_empty)
    );
    for (i, x) in [9, 8, 7].into_iter().enumerate() {
        assert_eq!(
            state.block_entities[&(x, 201, 8)].slots,
            original[i * 3..i * 3 + 3]
        );
    }
    assert!(use_on(&mut state, actor, Hand::Main, POS, 2, [0.5; 3]));
    assert_eq!(state.players[&actor].inventory.slots[..9], original);
    state.world.set_block(8, 202, 8, state.registry.states.air);
    scheduled(&mut state, POS, &mut Vec::new());
    assert_eq!(
        prop(at(&state, POS).unwrap(), "side_chain"),
        Some("unconnected")
    );
    assert_eq!(chain(&state, BlockPos { x: 9, y: 201, z: 8 }).len(), 1);
    assert_eq!(chain(&state, BlockPos { x: 7, y: 201, z: 8 }).len(), 1);
}
#[test]
fn shelf_checkpoint_native_chunk_restart_preserves_stacks_and_alignment() {
    let dir = std::env::temp_dir().join(format!("solaris-shelf-{}", uuid::Uuid::new_v4()));
    let (mut state, actor) = fixture(Some(dir.clone()));
    let id = block(&state, "pale_oak_shelf", &[("facing", "south")]);
    state.world.set_block(8, 201, 8, id);
    state.world.fold_overlay();
    let mut item = stack(&state, "diamond", 17);
    item.custom_name = Some("shelf custody".into());
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = item.clone();
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    run(&mut state);
    let update = state
        .outbox
        .values()
        .flatten()
        .find_map(|change| match change {
            PlayerChange::BlockEntityData {
                position,
                block_entity_type,
                nbt: Tag::Compound(fields),
                ..
            } if *position == POS => Some((*block_entity_type, fields)),
            _ => None,
        })
        .unwrap();
    assert_eq!(update.0, 40, "26.1.2 shelf block_entity_type");
    assert_shelf_items(update.1);
    let original = state.block_entities[&key(POS)].slots.clone();
    let bytes = state
        .world
        .chunk(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities[&POS]
        .clone();
    let Tag::Compound(mut fields) = mc_nbt::read_network(&mut &bytes[..]).unwrap() else {
        panic!()
    };
    fields
        .iter_mut()
        .find(|(k, _)| k == "align_items_to_bottom")
        .unwrap()
        .1 = Tag::Byte(1);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).unwrap();
    state.world.set_opaque_block_entity(POS, Some(bytes));
    checkpoint(&mut state);
    let block_entities = crate::core::checkpoint_data_for(&state, state.tick).2;
    state.world.capture_dirty(&block_entities).write().unwrap();
    let projected = state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    assert!(!projected.furnaces.contains_key(&POS));
    assert!(!projected.chests.contains_key(&POS));
    let records = mc_world::wire::client_block_entities(
        projected,
        &state.registry.blocks,
        &state.registry.items,
    );
    let shelf = records.iter().find(|record| record.pos == POS).unwrap();
    assert_eq!(shelf.type_name.as_str(), "minecraft:shelf");
    let registry = mc_data::block_entity_types::solaris_required_block_entity_types();
    assert_eq!(registry.id_of(&shelf.type_name), Some(40));
    let Tag::Compound(fields) = &shelf.nbt else {
        panic!("shelf update is not a compound")
    };
    assert_shelf_items(fields);
    let registry = std::sync::Arc::clone(&state.registry);
    let generator = std::sync::Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        std::sync::Arc::clone(&registry.blocks),
    ));
    let mut loaded = crate::world::ChunkWorld::new_native(
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
    loaded.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let chunk = loaded.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    let mut custody = crate::redstone::custody::BlockEntities::default();
    hydrate_chunk(&mut custody, &registry, chunk);
    assert_eq!(custody[&key(POS)].slots, original);
    let bytes = &chunk.block_entities[&POS];
    let Tag::Compound(fields) = mc_nbt::read_network(&mut &bytes[..]).unwrap() else {
        panic!()
    };
    assert_eq!(
        fields
            .iter()
            .find(|(key, _)| key == "id")
            .map(|(_, value)| value),
        Some(&Tag::String("minecraft:shelf".into()))
    );
    assert_shelf_items(&fields);
    for field in [
        "lit_time_remaining",
        "lit_total_time",
        "cooking_time_spent",
        "cooking_total_time",
        "RecipesUsed",
    ] {
        assert!(!fields.iter().any(|(key, _)| key == field), "{field}");
    }
    assert!(
        mc_data::storage_blocks::decode_shelf(&fields, &registry.items)
            .unwrap()
            .0
    );
    drop(state);
    drop(loaded);
    std::fs::remove_dir_all(dir).unwrap();
}

fn assert_shelf_items(fields: &[(String, Tag)]) {
    let Some(Tag::List(items)) = fields
        .iter()
        .find(|(key, _)| key == "Items")
        .map(|(_, value)| value)
    else {
        panic!("shelf Items is not a list")
    };
    assert_eq!(items.element_type, mc_nbt::tag_type::COMPOUND);
    assert_eq!(items.elements.len(), 1);
    let Tag::Compound(item) = &items.elements[0] else {
        panic!("shelf item is not a compound")
    };
    for (key, expected) in [
        ("Slot", Tag::Byte(1)),
        ("id", Tag::String("minecraft:diamond".into())),
        ("count", Tag::Int(17)),
    ] {
        assert_eq!(
            item.iter()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value),
            Some(&expected),
            "{key}"
        );
    }
}
#[test]
fn shelf_swap_wal_replays_absolute_custody_exactly_once() {
    let (mut state, actor) = fixture(None);
    let id = block(&state, "cherry_shelf", &[("facing", "south")]);
    state.world.set_block(8, 201, 8, id);
    state.world.fold_overlay();
    let item = stack(&state, "diamond", 13);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = item.clone();
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let batches = vec![(state.tick, state.tick, entries)];
    let once = crate::journal::replay_batches(crate::journal::Replay::default(), &batches);
    let twice = crate::journal::replay_batches(
        crate::journal::replay_batches(crate::journal::Replay::default(), &batches),
        &batches,
    );
    assert_eq!(once.opaque_block_entities, twice.opaque_block_entities);
    let bytes = twice.opaque_block_entities[&key(POS)].as_ref().unwrap();
    let Tag::Compound(fields) = mc_nbt::read_network(&mut &bytes[..]).unwrap() else {
        panic!()
    };
    let (_, slots) = mc_data::storage_blocks::decode_shelf(&fields, &state.registry.items).unwrap();
    assert_eq!(slots, vec![ItemStack::EMPTY, item, ItemStack::EMPTY]);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
}
#[test]
fn powered_five_shelf_row_swaps_only_the_last_three_connected_cells() {
    let (mut state, actor) = fixture(None);
    for x in 6..=10 {
        let part = match x {
            10 => "left",
            6 => "right",
            _ => "center",
        };
        let id = block(
            &state,
            "oak_shelf",
            &[
                ("facing", "north"),
                ("powered", "true"),
                ("side_chain", part),
            ],
        );
        let pos = BlockPos { x, y: 201, z: 8 };
        state.world.set_block(pos.x, pos.y, pos.z, id);
        register(&mut state, pos);
    }
    let untouched = stack(&state, "emerald", 11);
    for x in [9, 10] {
        state.block_entities.get_mut(&(x, 201, 8)).unwrap().slots[0] = untouched.clone();
    }
    for hotbar in 0..9 {
        let item = stack(&state, "diamond", hotbar as i32 + 1);
        state.players.get_mut(&actor).unwrap().inventory.slots[hotbar] = item;
    }
    let original = state.players[&actor].inventory.slots[..9].to_vec();
    assert_eq!(
        chain(&state, POS),
        vec![
            POS,
            BlockPos { x: 7, y: 201, z: 8 },
            BlockPos { x: 6, y: 201, z: 8 }
        ]
    );
    assert!(use_on(&mut state, actor, Hand::Main, POS, 2, [0.5; 3]));
    assert!(
        state.players[&actor].inventory.slots[..9]
            .iter()
            .all(ItemStack::is_empty)
    );
    for (i, x) in [8, 7, 6].into_iter().enumerate() {
        assert_eq!(
            state.block_entities[&(x, 201, 8)].slots,
            original[i * 3..i * 3 + 3]
        );
    }
    for x in [9, 10] {
        assert_eq!(state.block_entities[&(x, 201, 8)].slots[0], untouched);
    }
}

#[test]
fn powered_shelf_chains_use_the_last_three_six_or_nine_hotbar_slots() {
    for cells in [
        vec![(8, "unconnected")],
        vec![(8, "left"), (7, "right")],
        vec![(9, "left"), (8, "center"), (7, "right")],
    ] {
        let (mut state, actor) = fixture(None);
        for (x, part) in &cells {
            let id = block(
                &state,
                "oak_shelf",
                &[
                    ("facing", "north"),
                    ("powered", "true"),
                    ("side_chain", part),
                ],
            );
            state.world.set_block(*x, 201, 8, id);
        }
        for hotbar in 0..9 {
            let item = stack(&state, "diamond", hotbar as i32 + 1);
            state.players.get_mut(&actor).unwrap().inventory.slots[hotbar] = item;
        }
        let original = state.players[&actor].inventory.slots[..9].to_vec();
        let start = match cells.len() {
            1 => 6,
            2 => 3,
            3 => 0,
            _ => unreachable!(),
        };
        assert!(use_on(&mut state, actor, Hand::Main, POS, 2, [0.5; 3]));
        assert_eq!(
            state.players[&actor].inventory.slots[..start],
            original[..start]
        );
        assert!(
            state.players[&actor].inventory.slots[start..9]
                .iter()
                .all(ItemStack::is_empty)
        );
        for ((x, _), items) in cells
            .iter()
            .zip(original[start..9].as_chunks::<3>().0.iter())
        {
            assert_eq!(state.block_entities[&(*x, 201, 8)].slots, *items);
        }
    }
}

#[test]
fn shelf_swap_preserves_real_player_reservations_and_allows_surplus() {
    let (mut state, actor) = fixture(None);
    let id = block(&state, "oak_shelf", &[("facing", "south")]);
    state.world.set_block(POS.x, POS.y, POS.z, id);
    state.world.fold_overlay();
    let diamond = stack(&state, "diamond", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = diamond.clone();
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    let emerald = stack(&state, "emerald", 4);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = emerald.clone();
    let player_id = state.players[&actor].connection.0;
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id },
        "emerald",
        4,
    );
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    assert_eq!(state.block_entities[&key(POS)].slots[1], diamond);
    assert_eq!(state.players[&actor].inventory.slots[0], emerald);
    // Shelves are not native Container reservation sources. Keep the real
    // player hold active and provide surplus outside the exchanged slot.
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = emerald.clone();
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    assert_eq!(state.block_entities[&key(POS)].slots[1], emerald);
    assert_eq!(state.players[&actor].inventory.slots[0], diamond);
    assert_eq!(state.players[&actor].inventory.slots[9], emerald);
}

#[test]
fn powered_shelf_exchange_is_atomic_with_real_player_reservations() {
    let (mut state, actor) = fixture(None);
    let id = block(
        &state,
        "oak_shelf",
        &[("facing", "south"), ("powered", "true")],
    );
    state.world.set_block(POS.x, POS.y, POS.z, id);
    state.world.fold_overlay();
    let diamond = stack(&state, "diamond", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[6] = diamond.clone();
    let emerald = stack(&state, "emerald", 2);
    let gold = stack(&state, "gold_ingot", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[7] = emerald;
    state.players.get_mut(&actor).unwrap().inventory.slots[8] = gold;
    let original = state.players[&actor].inventory.slots.clone();
    let player_id = state.players[&actor].connection.0;
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id },
        "diamond",
        1,
    );
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    assert_eq!(state.players[&actor].inventory.slots, original);
    assert!(
        state.block_entities[&key(POS)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = diamond.clone();
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    assert_eq!(state.block_entities[&key(POS)].slots, original[6..9]);
    assert!(
        state.players[&actor].inventory.slots[6..9]
            .iter()
            .all(ItemStack::is_empty)
    );
    assert_eq!(state.players[&actor].inventory.slots[9], diamond);
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    let mut returned = original;
    returned[9] = diamond;
    assert_eq!(state.players[&actor].inventory.slots, returned);
    assert!(
        state.block_entities[&key(POS)]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
}

#[test]
fn chiseled_bookshelf_stores_and_returns_books() {
    // CHISELED-BOOKSHELF through the block-use path (ChiseledBookShelfBlock,
    // 26.1.2): using a BOOKSHELF_BOOKS-tag book stores it (slot_4_occupied
    // set for cursor [0.5; 3]); the comparator reads
    // getLastInteractedSlot() + 1 = 5. Cursor math: col = section(0.5, 3)
    // = 1, row = section(0.5, 2) = 1, index = 1 + 1 * 3 = 4.
    let (mut state, actor) = fixture(None);
    let id = block(&state, "chiseled_bookshelf", &[("facing", "south")]);
    state.world.set_block(8, 201, 8, id);
    state.world.fold_overlay();
    let book = stack(&state, "book", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = book.clone();
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let stored = state
        .world
        .block_at(POS.x, POS.y, POS.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .unwrap();
    assert!(
        stored
            .properties
            .iter()
            .any(|(k, v)| k == "slot_4_occupied" && v == "true"),
        "stored book must mark its slot occupied"
    );
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
}

#[test]
fn chiseled_bookshelf_comparator_reads_last_interacted_slot() {
    // Post-fix coverage (vanilla-b only): comparator = last slot + 1.
    let (mut state, actor) = fixture(None);
    let id = block(&state, "chiseled_bookshelf", &[("facing", "south")]);
    state.world.set_block(8, 201, 8, id);
    state.world.fold_overlay();
    let book = stack(&state, "book", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = book.clone();
    assert!(use_on(&mut state, actor, Hand::Main, POS, 3, [0.5; 3]));
    assert_eq!(crate::chiseled_shelf::comparator(&state, POS), 5);
}

#[test]
fn chiseled_bookshelf_front_face_cursor_selects_top_left_slot() {
    // Integrated from VanillaC-2 parity (interact_parity_chiseled_bookshelf_front_slot):
    // facing=south front face (3), cursor [0.1, 0.8, 1.0] -> col 0, row 0,
    // index 0. Covers the slot-0 cursor mapping the existing tests skip.
    let (mut state, actor) = fixture(None);
    let id = block(&state, "chiseled_bookshelf", &[("facing", "south")]);
    state.world.set_block(8, 201, 8, id);
    state.world.fold_overlay();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack(&state, "book", 6);
    assert!(use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        3,
        [0.1, 0.8, 1.0]
    ));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 5);
    assert_eq!(state.block_entities[&key(POS)].slots[0].count, 1);
}

#[test]
fn chiseled_bookshelf_fill_all_slots_comparator_and_extract() {
    // Integrated from VanillaC-2 parity (interact_parity_bookshelf_all_slots_extract_and_hydrate):
    // fill all 6 slots, comparator reads last slot + 1 each time, extract
    // slot 5. Remapped to chiseled_shelf::comparator (no special_storage here).
    let (mut state, actor) = fixture(None);
    let id = block(&state, "chiseled_bookshelf", &[("facing", "south")]);
    state.world.set_block(8, 201, 8, id);
    state.world.fold_overlay();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack(&state, "book", 6);
    for slot in 0..6 {
        let cursor = [
            [0.1, 0.5, 0.9][slot % 3],
            if slot < 3 { 0.8 } else { 0.2 },
            1.0,
        ];
        assert!(use_on(&mut state, actor, Hand::Main, POS, 3, cursor));
        assert_eq!(
            crate::chiseled_shelf::comparator(&state, POS),
            slot as u8 + 1
        );
    }
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
    assert!(use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        3,
        [0.9, 0.2, 1.0]
    ));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(state.block_entities[&key(POS)].slots[5].is_empty());
}
