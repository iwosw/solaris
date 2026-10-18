use std::sync::{Arc, LazyLock};

use super::*;
use crate::boundary::{ChunkPos, ClickMode, DigAction, Hand, PlayerInput};
use crate::journal::{
    JournalEntry, JournalWriter, Replay, read_complete_batches_after, replay_batches,
};
use crate::tick::run_tick;
use crate::world::ChunkWorld;

const POS: BlockPos = BlockPos { x: 0, y: 201, z: 0 };
const KEY: (i32, i32, i32) = (0, 201, 0);
const CONNECTION: ConnectionId = ConnectionId(1);

fn registry() -> Arc<CoreRegistry> {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    Arc::clone(&REGISTRY)
}

fn state(dir: Option<std::path::PathBuf>) -> TickState {
    let registry = registry();
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
    // A headless stand must retain residency, including after a clean reload.
    state
        .tickets
        .insert("brewing-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    // Strict radius split: fixture intent carries a ticket; activation flows
    // through the same refresh run_tick uses (tickets simulate, Main binding).
    state.refresh_active_chunks(state.tick);
    state
}

fn stack(name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        registry()
            .items
            .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap(),
        count,
    )
}

fn potion(container: &str, contents: &str) -> ItemStack {
    let mut stack = stack(container, 1);
    stack.extended_mut().potion = Some(Identifier::parse(format!("minecraft:{contents}")).unwrap());
    stack
}

fn slots() -> Vec<ItemStack> {
    vec![
        potion("potion", "water"),
        potion("potion", "water"),
        potion("potion", "water"),
        stack("nether_wart", 3),
        stack("blaze_powder", 2),
    ]
}

fn install(state: &mut TickState, slots: Vec<ItemStack>) {
    let id = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:brewing_stand").unwrap())
        .unwrap()
        .default
        .0;
    state
        .world
        .set_block(POS.x, POS.y - 1, POS.z, state.registry.states.stone);
    state.world.set_block(POS.x, POS.y, POS.z, id);
    state.world.fold_overlay();
    state.block_entities.insert(
        KEY,
        BlockEntityState {
            slots,
            furnace: None,
        },
    );
}

fn step(state: &mut TickState) -> Vec<JournalEntry> {
    let (output, _, _) = run_tick(state, &[]);
    crate::core::journal_entries_for_tick(state, &output)
}

fn hydrate(state: &mut TickState) {
    let chunk = state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    hydrate_chunk(
        &mut state.brewing,
        &mut state.block_entities,
        &state.registry,
        chunk,
    );
}

fn assert_awkward(slots: &[ItemStack]) {
    for bottle in &slots[..3] {
        assert_eq!(
            bottle.extended().potion.as_ref().unwrap().as_str(),
            "minecraft:awkward"
        );
        assert_eq!(bottle.count, 1);
    }
}

#[test]
fn batch_uses_one_fuel_charge_and_one_ingredient_for_three_bottles_at_four_hundred_ticks() {
    let items = registry();
    let mut slots = slots();
    let mut brewing = BrewingState::default();
    assert!(brewing.tick(&mut slots, &items.items).is_none());
    assert_eq!(brewing.properties(), [400, 19]);
    assert_eq!(slots[4].count, 1);
    for _ in 0..399 {
        brewing.tick(&mut slots, &items.items);
    }
    assert_eq!(brewing.brew_time, 1);
    assert_eq!(slots[3].count, 3);
    brewing.tick(&mut slots, &items.items);
    assert_eq!(brewing.properties(), [0, 19]);
    assert_eq!(slots[3].count, 2);
    assert_awkward(&slots);
    // Awkward + nether wart has no recipe: idle ticks don't consume another charge.
    brewing.tick(&mut slots, &items.items);
    assert_eq!(brewing.fuel, 19);
}

#[test]
fn trial_effect_potions_brew_from_awkward_with_canonical_ingredients() {
    for (ingredient, contents) in [
        ("slime_block", "oozing"),
        ("stone", "infested"),
        ("cobweb", "weaving"),
        ("breeze_rod", "wind_charged"),
    ] {
        let registry = registry();
        let mut slots = vec![
            potion("potion", "awkward"),
            potion("potion", "awkward"),
            potion("potion", "awkward"),
            stack(ingredient, 1),
            stack("blaze_powder", 1),
        ];
        let mut brewing = BrewingState::default();
        brewing.tick(&mut slots, &registry.items);
        for _ in 0..400 {
            brewing.tick(&mut slots, &registry.items);
        }
        assert!(slots[3].is_empty());
        for bottle in &slots[..3] {
            assert_eq!(bottle.extended().potion.as_ref().unwrap().path(), contents);
            assert_eq!(crate::effects::potion_effects(contents).len(), 1);
        }
    }
}

#[test]
fn final_tick_ingredient_change_and_all_bottle_removal_cancel_without_consumption_or_fuel_refund() {
    let items = registry();
    let mut slots = slots();
    let mut brewing = BrewingState::default();
    brewing.tick(&mut slots, &items.items);
    brewing.brew_time = 1;
    slots[3] = stack("gunpowder", 2);
    brewing.tick(&mut slots, &items.items);
    assert_eq!(brewing.properties(), [0, 19]);
    assert_eq!(slots[3].count, 2);
    assert_eq!(slots[0], potion("potion", "water"));
    brewing.tick(&mut slots, &items.items);
    assert_eq!(brewing.properties(), [400, 18]);
    slots[..3].fill(ItemStack::EMPTY);
    brewing.tick(&mut slots, &items.items);
    assert_eq!(brewing.properties(), [0, 18]);
    assert_eq!(slots[3].count, 2);
    assert!(brewing.ingredient.is_none());
}

#[test]
fn empty_fuel_blocks_batches_but_powder_charges_even_without_a_recipe() {
    let items = registry();
    let mut slots = slots();
    slots[4] = ItemStack::EMPTY;
    let mut brewing = BrewingState::default();
    brewing.tick(&mut slots, &items.items);
    assert_eq!(brewing.properties(), [0, 0]);
    slots[4] = stack("blaze_powder", 2);
    // Stone brews water into mundane (minecraft.wiki/w/Brewing); dirt does not.
    slots[3] = stack("dirt", 1);
    brewing.tick(&mut slots, &items.items);
    assert_eq!(brewing.properties(), [0, 20]);
    assert_eq!(slots[4].count, 1);
}

#[test]
fn dragon_breath_changes_container_preserves_potion_and_returns_exactly_one_bottle() {
    let items = registry();
    for count in [1, 2] {
        let mut slots = vec![
            potion("splash_potion", "strength"),
            ItemStack::EMPTY,
            ItemStack::EMPTY,
            stack("dragon_breath", count),
            ItemStack::EMPTY,
        ];
        slots[0].custom_name = Some("brewer's bottle".into());
        let mut brewing = BrewingState {
            brew_time: 1,
            fuel: 8,
            ingredient: Some(slots[3].item_id),
            ..Default::default()
        };
        let dropped = brewing.tick(&mut slots, &items.items);
        assert_eq!(slots[0].item_id, stack("lingering_potion", 1).item_id);
        assert_eq!(
            slots[0].extended().potion.as_ref().unwrap().as_str(),
            "minecraft:strength"
        );
        assert_eq!(slots[0].custom_name.as_deref(), Some("brewer's bottle"));
        if count == 1 {
            assert!(dropped.is_none());
            assert_eq!(slots[3], stack("glass_bottle", 1));
        } else {
            assert_eq!(dropped, Some(stack("glass_bottle", 1)));
            assert_eq!(slots[3], stack("dragon_breath", 1));
        }
        assert_eq!(brewing.brew_time, 0);
    }
}

#[test]
fn exact_vanilla_timer_nbt_and_rich_bottles_roundtrip() {
    let registry = registry();
    let mut slots = slots();
    slots[0].custom_name = Some("persistent potion".into());
    let mut brewing = BrewingState::default();
    brewing.tick(&mut slots, &registry.items);
    for _ in 0..137 {
        brewing.tick(&mut slots, &registry.items);
    }
    let bytes = brewing.encode(POS, &slots, &registry.items);
    let (decoded, decoded_slots) = BrewingState::decode(&bytes, &registry.items).unwrap();
    assert_eq!(decoded, brewing);
    assert_eq!(decoded_slots, slots);
    let Tag::Compound(fields) = mc_nbt::read_network(&mut bytes.as_slice()).unwrap() else {
        panic!("compound");
    };
    assert_eq!(field(&fields, "BrewTime"), Some(&Tag::Short(263)));
    assert_eq!(field(&fields, "Fuel"), Some(&Tag::Byte(19)));
    assert_eq!(
        field(&fields, "id"),
        Some(&Tag::String("minecraft:brewing_stand".into()))
    );
}

#[test]
fn checkpoint_native_loading_restores_slots_timers_and_finishes_once() {
    let dir = std::env::temp_dir().join(format!("brewing-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    install(&mut original, slots());
    for _ in 0..138 {
        step(&mut original);
    }
    let before = original.brewing[&KEY].clone();
    let slots_before = original.block_entities[&KEY].slots.clone();
    assert_eq!(before.brew_time, 263);
    checkpoint(&mut original);
    assert!(
        !original
            .world
            .capture_dirty(&BTreeMap::new())
            .write()
            .unwrap()
            .is_empty()
    );
    drop(original);
    let mut restored = state(Some(dir.clone()));
    hydrate(&mut restored);
    assert_eq!(restored.brewing[&KEY], before);
    assert_eq!(restored.block_entities[&KEY].slots, slots_before);
    for _ in 0..262 {
        step(&mut restored);
    }
    assert_eq!(restored.brewing[&KEY].brew_time, 1);
    assert_eq!(restored.block_entities[&KEY].slots[3].count, 3);
    step(&mut restored);
    assert_awkward(&restored.block_entities[&KEY].slots);
    assert_eq!(restored.block_entities[&KEY].slots[3].count, 2);
    step(&mut restored);
    assert_eq!(restored.block_entities[&KEY].slots[3].count, 2);
    // Imported typed stand no longer shadows current opaque timer/slot data.
    assert!(
        !restored
            .world
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .brewing_stands
            .contains_key(&POS)
    );
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn durable_wal_after_checkpoint_restores_newer_timer_and_slots_idempotently() {
    let dir = std::env::temp_dir().join(format!("brewing-replay-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    install(&mut original, slots());
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    let entries = step(&mut original);
    writer
        .append(original.tick, original.tick, &entries)
        .unwrap();
    writer.gate(0, original.tick).unwrap();
    checkpoint(&mut original);
    original
        .world
        .capture_dirty(&BTreeMap::new())
        .write()
        .unwrap();
    for _ in 0..137 {
        let entries = step(&mut original);
        writer
            .append(original.tick, original.tick, &entries)
            .unwrap();
    }
    let timer = original.brewing[&KEY].clone();
    writer.gate(0, original.tick).unwrap();
    let batches = read_complete_batches_after(&dir.join("journal"), 1).unwrap();
    let once = replay_batches(Replay::default(), &batches);
    let expected_opaque = once.opaque_block_entities.clone();
    let twice = replay_batches(once, &batches);
    assert_eq!(expected_opaque, twice.opaque_block_entities);
    let mut restored = state(Some(dir.clone()));
    for (key, bytes) in twice.opaque_block_entities {
        restored.world.set_opaque_block_entity(
            BlockPos {
                x: key.0,
                y: key.1,
                z: key.2,
            },
            bytes,
        );
    }
    hydrate(&mut restored);
    assert_eq!(restored.brewing[&KEY], timer);
    assert_eq!(
        restored.block_entities[&KEY].slots,
        original.block_entities[&KEY].slots
    );
    for _ in 0..timer.brew_time {
        step(&mut restored);
    }
    assert_awkward(&restored.block_entities[&KEY].slots);
    assert_eq!(restored.block_entities[&KEY].slots[3].count, 2);
    drop(writer);
    drop(original);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn latest_pending_owner_timer_image_wins_over_unchanged_chunk_cache() {
    let mut original = state(None);
    install(&mut original, slots());
    step(&mut original);
    let cached = original
        .world
        .chunk(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities[&POS]
        .clone();
    run_tick(&mut original, &[]);
    let (output, _, _) = run_tick(&mut original, &[]);
    let timer = original.brewing[&KEY].clone();
    assert_eq!(timer.brew_time, 398);
    assert_eq!(
        original
            .world
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities[&POS],
        cached,
    );
    let entries = crate::core::journal_entries_for_tick(&mut original, &output);
    assert_eq!(
        entries
            .iter()
            .filter(|entry| matches!(
                entry,
                JournalEntry::OpaqueBlockEntity {
                    pos: [0, 201, 0],
                    nbt: Some(_)
                },
            ))
            .count(),
        1,
    );
    let batches = [(output.tick, output.tick, entries)];
    let once = replay_batches(Replay::default(), &batches);
    let twice = replay_batches(once, &batches);
    let bytes = twice.opaque_block_entities[&KEY].as_ref().unwrap();
    assert_ne!(bytes, &cached);
    let mut restored = state(None);
    install(&mut restored, vec![ItemStack::EMPTY; 5]);
    restored.block_entities.remove(&KEY);
    restored
        .world
        .set_opaque_block_entity(POS, Some(bytes.clone()));
    hydrate(&mut restored);
    assert_eq!(restored.brewing[&KEY], timer);
    assert_eq!(
        restored.block_entities[&KEY].slots,
        original.block_entities[&KEY].slots
    );
}

fn join(state: &mut TickState, uuid: uuid::Uuid) -> EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: CONNECTION,
            uuid,
            name: "brewer".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(CONNECTION).unwrap();
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [0.5, 201.0, 2.5]);
    entity
}

fn open(state: &mut TickState) {
    run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: CONNECTION,
            position: POS,
            hand: Hand::Main,
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
}

fn click(state: &mut TickState, entity: EntityId, slot: i16, mode: ClickMode) {
    let menu = state.players[&entity].open.as_ref().unwrap();
    let input = PlayerInput::ContainerClick {
        connection: CONNECTION,
        window: menu.window,
        state_id: menu.state_id,
        slot,
        button: 0,
        mode,
    };
    run_tick(state, &[input]);
}

fn inventory_count(state: &TickState, entity: EntityId, item: u32) -> i32 {
    state.players[&entity]
        .inventory
        .slots
        .iter()
        .filter(|stack| stack.item_id == item && !stack.is_empty())
        .map(|stack| stack.count)
        .sum()
}

#[test]
fn shift_click_close_disconnect_and_rejoin_keep_single_shared_custody() {
    let mut state = state(None);
    let uuid = uuid::Uuid::new_v4();
    let entity = join(&mut state, uuid);
    install(&mut state, vec![ItemStack::EMPTY; 5]);
    let potion = potion("potion", "water");
    let ingredient = stack("nether_wart", 3);
    let fuel = stack("blaze_powder", 2);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = potion.clone();
    state.players.get_mut(&entity).unwrap().inventory.slots[1] = ingredient.clone();
    state.players.get_mut(&entity).unwrap().inventory.slots[2] = fuel.clone();
    open(&mut state);
    assert_eq!(
        state.players[&entity].open.as_ref().unwrap().kind,
        crate::inventory::MenuKind::BrewingStand
    );
    click(&mut state, entity, 32, ClickMode::QuickMove);
    click(&mut state, entity, 33, ClickMode::QuickMove);
    click(&mut state, entity, 34, ClickMode::QuickMove);
    assert_eq!(state.block_entities[&KEY].slots[0], potion);
    assert_eq!(state.block_entities[&KEY].slots[3].count, 3);
    assert_eq!(state.block_entities[&KEY].slots[4].count, 1);
    assert_eq!(state.brewing[&KEY].fuel, 19);
    let window = state.players[&entity].open.as_ref().unwrap().window;
    run_tick(
        &mut state,
        &[PlayerInput::ContainerClose {
            connection: CONNECTION,
            window,
        }],
    );
    assert_eq!(inventory_count(&state, entity, ingredient.item_id), 0);
    assert_eq!(state.block_entities[&KEY].slots[3].count, 3);
    let timer = state.brewing[&KEY].brew_time;
    step(&mut state);
    assert_eq!(state.brewing[&KEY].brew_time, timer - 1);
    open(&mut state);
    // Ingredient leaves shared storage into the cursor. Leaving must fold only
    // that cursor into the player record, never refund stand slots.
    click(&mut state, entity, 3, ClickMode::Pickup);
    assert!(state.block_entities[&KEY].slots[3].is_empty());
    assert_eq!(state.brewing[&KEY].brew_time, 0);
    run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: CONNECTION,
        }],
    );
    let entity = join(&mut state, uuid);
    assert_eq!(inventory_count(&state, entity, ingredient.item_id), 3);
    assert_eq!(inventory_count(&state, entity, potion.item_id), 0);
    assert_eq!(inventory_count(&state, entity, fuel.item_id), 0);
    assert_eq!(state.block_entities[&KEY].slots[0], potion);
    assert_eq!(state.block_entities[&KEY].slots[4].count, 1);
    open(&mut state);
    click(&mut state, entity, 0, ClickMode::QuickMove);
    assert_eq!(inventory_count(&state, entity, potion.item_id), 1);
    assert!(state.block_entities[&KEY].slots[0].is_empty());
    let block = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(
            state.world.block_at(POS.x, POS.y, POS.z).unwrap(),
        ))
        .unwrap();
    assert!(
        block
            .properties
            .iter()
            .any(|(name, value)| name == "has_bottle_0" && value == "false")
    );
}

#[test]
fn breaking_active_stand_closes_view_and_drops_each_slot_once_with_replay_tombstone() {
    let mut state = state(None);
    let entity = join(&mut state, uuid::Uuid::new_v4());
    install(&mut state, slots());
    open(&mut state);
    step(&mut state);
    let before = state.block_entities[&KEY].slots.clone();
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: CONNECTION,
            position: POS,
            action: DigAction::Finish,
            sequence: 2,
        }],
    );
    assert!(!state.block_entities.contains_key(&KEY));
    assert!(!state.brewing.contains_key(&KEY));
    assert!(state.players[&entity].open.is_none());
    assert!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change, PlayerChange::MenuClosed { .. }))
    );
    let dropped: Vec<_> = output
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| {
            state
                .entities
                .slot_of(*id)
                .and_then(|slot| state.entities.row(slot).item.as_ref())
                .map(|stack| stack.full_stack())
        })
        .collect();
    for stack in &before {
        assert_eq!(
            dropped.iter().filter(|drop| *drop == stack).count(),
            if stack.item_id == before[0].item_id {
                3
            } else {
                1
            }
        );
    }
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|entry| matches!(
        entry,
        JournalEntry::RemoveBlockEntity {
            pos: [0, 201, 0],
            ..
        }
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        JournalEntry::OpaqueBlockEntity {
            pos: [0, 201, 0],
            nbt: None
        }
    )));
    let next = run_tick(&mut state, &[]).0;
    assert!(next.changes.entities_spawned.is_empty());
}

#[test]
fn ingredient_remove_and_replace_in_one_batch_restarts_and_spends_a_new_charge() {
    let mut state = state(None);
    install(&mut state, slots());
    step(&mut state);
    assert_eq!(state.brewing[&KEY].properties(), [400, 19]);
    let ingredient = std::mem::take(&mut state.block_entities.get_mut(&KEY).unwrap().slots[3]);
    slots_replaced(&mut state.brewing, KEY, &state.block_entities[&KEY].slots);
    assert_eq!(state.brewing[&KEY].brew_time, 0);
    state.block_entities.get_mut(&KEY).unwrap().slots[3] = ingredient;
    slots_replaced(&mut state.brewing, KEY, &state.block_entities[&KEY].slots);
    step(&mut state);
    assert_eq!(state.brewing[&KEY].properties(), [400, 18]);
    assert_eq!(state.block_entities[&KEY].slots[3].count, 3);
    assert_eq!(
        state.block_entities[&KEY].slots[0],
        potion("potion", "water")
    );
}

#[test]
fn bottle_presence_replaces_prior_same_position_write_once() {
    let mut state = state(None);
    install(&mut state, slots());
    let current = state.world.block_at(POS.x, POS.y, POS.z).unwrap();
    let mut writes = vec![(POS, current), (POS, current)];
    run(&mut state, &mut writes, &mut Vec::new());
    let at_pos: Vec<_> = writes.iter().filter(|(pos, _)| *pos == POS).collect();
    assert_eq!(at_pos.len(), 1);
    assert_eq!(
        at_pos[0].1,
        state.world.block_at(POS.x, POS.y, POS.z).unwrap()
    );
    let block = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(at_pos[0].1))
        .unwrap();
    for i in 0..3 {
        assert!(
            block
                .properties
                .iter()
                .any(|(key, value)| key == &format!("has_bottle_{i}") && value == "true")
        );
    }
}

#[test]
fn brewing_checks_reserved_fuel_and_completion_before_commit() {
    let mut state = state(None);
    let original = slots();
    install(&mut state, original.clone());
    crate::crafter::tests::hold_block(&mut state, POS, "blaze_powder", 2);
    let mut drops = Vec::new();
    run(&mut state, &mut Vec::new(), &mut drops);
    assert_eq!(state.block_entities[&KEY].slots, original);
    assert_eq!(state.brewing[&KEY].properties(), [0, 0]);
    assert!(drops.is_empty());

    crate::plugin_inventories::invalidate_block(&mut state, POS);
    run(&mut state, &mut Vec::new(), &mut drops);
    assert_eq!(state.block_entities[&KEY].slots[4].count, 1);
    assert_eq!(state.brewing[&KEY].properties(), [400, 19]);
    crate::crafter::tests::hold_block(&mut state, POS, "nether_wart", 3);
    state.brewing.get_mut(&KEY).unwrap().brew_time = 1;
    let before = state.block_entities[&KEY].slots.clone();
    run(&mut state, &mut Vec::new(), &mut drops);
    assert_eq!(state.block_entities[&KEY].slots, before);
    assert_eq!(state.brewing[&KEY].properties(), [1, 19]);
    assert!(drops.is_empty());

    crate::plugin_inventories::invalidate_block(&mut state, POS);
    run(&mut state, &mut Vec::new(), &mut drops);
    assert_eq!(state.block_entities[&KEY].slots[3].count, 2);
    assert_awkward(&state.block_entities[&KEY].slots);
}

#[test]
fn resident_but_inactive_brewing_stand_freezes_and_reactivation_resumes() {
    // G5 radius split: residency alone must not simulate.
    // Note: the fixture ticket keeps the chunk resident; activation is
    // explicit via set_active_chunks (strict empty default).
    let mut state = state(None);
    let original = slots();
    install(&mut state, original.clone());
    crate::crafter::tests::hold_block(&mut state, POS, "blaze_powder", 2);
    crate::plugin_inventories::invalidate_block(&mut state, POS);
    // Fixture ticket+refresh already activated (0,0); no override needed here.
    let mut drops = Vec::new();
    run(&mut state, &mut Vec::new(), &mut drops);
    let first = state.brewing[&KEY].brew_time;
    run(&mut state, &mut Vec::new(), &mut drops);
    let live = state.brewing[&KEY].brew_time;
    assert!(
        first == 400 && live == 399,
        "active stand must brew, got {first} -> {live}"
    );
    // Deactivate: unit freeze overrides world membership after fixture setup
    // (common fixture intent stays ticket-carried per final contract).
    state
        .world
        .set_active_chunks(Default::default(), state.tick + 1);
    assert!(!state.world.is_active(0, 0));
    assert!(state.world.has_chunk(0, 0), "chunk stays resident");
    let frozen = state.brewing[&KEY].brew_time;
    for _ in 0..4 {
        run(&mut state, &mut Vec::new(), &mut drops);
    }
    assert_eq!(
        state.brewing[&KEY].brew_time, frozen,
        "brew_time froze while inactive"
    );
    // Reactivate: resumes.
    state.world.set_active_chunks(
        [(ChunkPos { x: 0, z: 0 })].into_iter().collect(),
        state.tick + 1,
    );
    run(&mut state, &mut Vec::new(), &mut drops);
    assert_ne!(
        state.brewing[&KEY].brew_time, frozen,
        "brew_time resumes after reactivation"
    );
}
