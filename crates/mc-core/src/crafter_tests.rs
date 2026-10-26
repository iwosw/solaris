use super::*;
use crate::boundary::{ChunkPos, ClickMode, ConnectionId, Hand, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use std::sync::{Arc, LazyLock};
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };
const KEY: (i32, i32, i32) = (8, 201, 8);
pub(crate) fn fixture(dir: Option<std::path::PathBuf>) -> (TickState, EntityId) {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let mut world = if let Some(dir) = dir {
        ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&crate::CoreConfig::new(dir, 42))
                .unwrap(),
            0,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 10],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 10],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .tickets
        .insert("storage-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    // Strict radius split: fixture intent carries a ticket; activation flows
    // through the same refresh run_tick uses (tickets simulate, Main binding).
    state.refresh_active_chunks(state.tick);
    for x in 0..16 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..206 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    state.world.fold_overlay();
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "storage".into(),
            permission_level: 0,
        }],
    );
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(slot, [8.5, 201.0, 10.5]);
    state.entities.set_ground(slot, true);
    (state, actor)
}
pub(crate) fn stack(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap(),
        count,
    )
}
pub(crate) fn block(state: &TickState, name: &str, properties: &[(&str, &str)]) -> u32 {
    let name = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
    let block = state.registry.blocks.block(&name).unwrap();
    let mut props = state
        .registry
        .blocks
        .by_id(block.default)
        .unwrap()
        .properties
        .clone();
    for (key, value) in properties {
        props.iter_mut().find(|(k, _)| k == key).unwrap().1 = (*value).into();
    }
    state
        .registry
        .blocks
        .by_name_and_props(&name, &props)
        .unwrap()
        .0
}
fn install(state: &mut TickState, orientation: &str) {
    let id = block(state, "crafter", &[("orientation", orientation)]);
    state.world.set_block(POS.x, POS.y, POS.z, id);
    state.world.fold_overlay();
    register(state, POS);
}
fn pulse(state: &mut TickState) {
    let power = block(state, "redstone_block", &[]);
    state.world.set_block(9, 201, 8, power);
    scheduled(state, POS, 0, &mut Vec::new(), &mut Vec::new());
}
fn step(state: &mut TickState) -> Vec<crate::journal::JournalEntry> {
    let (output, _, _) = run_tick(state, &[]);
    crate::core::journal_entries_for_tick(state, &output)
}
#[test]
fn pulse_crafts_once_disabled_slots_and_all_six_ejection_directions() {
    for (orientation, travel) in [
        ("north_up", (0, 0, -1)),
        ("south_up", (0, 0, 1)),
        ("west_up", (-1, 0, 0)),
        ("east_up", (1, 0, 0)),
        ("up_north", (0, 1, 0)),
        ("down_north", (0, -1, 0)),
    ] {
        let (mut state, _) = fixture(None);
        install(&mut state, orientation);
        state.block_entities.get_mut(&KEY).unwrap().slots[0] = stack(&state, "oak_log", 3);
        state
            .crafters
            .get_mut(&KEY)
            .unwrap()
            .settings
            .disabled_slots = 0x1fe;
        pulse(&mut state);
        let mut drops = Vec::new();
        scheduled(&mut state, POS, 8, &mut Vec::new(), &mut drops);
        assert_eq!(drops.len(), 1);
        assert_eq!(state.block_entities[&KEY].slots[0].count, 2);
        let slot = state.entities.slot_of(drops[0]).unwrap();
        let pos = state.entities.pos(slot);
        assert_eq!(
            pos,
            [
                8.5 + f64::from(travel.0) * 0.7,
                201.5 + f64::from(travel.1) * 0.7,
                8.5 + f64::from(travel.2) * 0.7
            ]
        );
        for _ in 0..8 {
            scheduled(&mut state, POS, 0, &mut Vec::new(), &mut drops);
        }
        assert_eq!(state.block_entities[&KEY].slots[0].count, 2);
        assert_eq!(signal(&state, POS), 9);
        assert_eq!(state.crafters[&KEY].settings.crafting_ticks_remaining, 6);
    }
}
#[test]
fn rising_edge_waits_four_ticks_and_held_power_never_repeats() {
    let (mut state, _) = fixture(None);
    install(&mut state, "north_up");
    state.block_entities.get_mut(&KEY).unwrap().slots[0] = stack(&state, "oak_log", 3);
    pulse(&mut state);
    // Complete the current merge tick in which the edge was observed; the
    // scheduled deadline is four processing ticks after that observation.
    step(&mut state);
    assert_eq!(state.block_entities[&KEY].slots[0].count, 3);
    for _ in 0..3 {
        step(&mut state);
        assert_eq!(state.block_entities[&KEY].slots[0].count, 3);
    }
    step(&mut state);
    assert_eq!(state.block_entities[&KEY].slots[0].count, 2);
    assert_eq!(state.crafters[&KEY].settings.crafting_ticks_remaining, 6);
    for _ in 0..10 {
        step(&mut state);
    }
    assert_eq!(state.block_entities[&KEY].slots[0].count, 2);
    assert_eq!(state.crafters[&KEY].settings.crafting_ticks_remaining, 0);
    state.world.set_block(9, 201, 8, state.registry.states.air);
    scheduled(&mut state, POS, 0, &mut Vec::new(), &mut Vec::new());
    pulse(&mut state);
    step(&mut state);
    for _ in 0..4 {
        step(&mut state);
    }
    assert_eq!(state.block_entities[&KEY].slots[0].count, 1);
}
#[test]
fn hopper_balances_enabled_slots_and_extracts_crafter_inputs() {
    let (mut state, _) = fixture(None);
    install(&mut state, "north_up");
    let log = stack(&state, "oak_log", 5);
    let settings = CrafterSettings {
        disabled_slots: 1 << 1,
        ..Default::default()
    };
    let slots = vec![
        log.clone(),
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
        ItemStack::EMPTY,
    ];
    assert!(!allowed(settings, &slots, 0, 64));
    assert!(!allowed(settings, &slots, 1, 64));
    assert!(allowed(settings, &slots, 2, 64));
    state.crafters.get_mut(&KEY).unwrap().settings = settings;
    state.block_entities.get_mut(&KEY).unwrap().slots = slots;
    let mut incoming = stack(&state, "oak_log", 1);
    crate::redstone::containers::insert_stack(&mut state, POS, (0, -1, 0), &mut incoming);
    assert!(incoming.is_empty());
    assert_eq!(state.block_entities[&KEY].slots[2].count, 1);
    let hopper = BlockPos { x: 8, y: 200, z: 8 };
    let id = block(&state, "hopper", &[("facing", "down")]);
    state.world.set_block(hopper.x, hopper.y, hopper.z, id);
    state.block_entities.insert(
        (8, 200, 8),
        BlockEntityState {
            slots: vec![ItemStack::EMPTY; 5],
            furnace: None,
        },
    );
    crate::redstone::containers::register(&mut state, hopper);
    step(&mut state);
    assert_eq!(state.block_entities[&KEY].slots[0].count, 4);
    assert_eq!(state.block_entities[&(8, 200, 8)].slots[0].count, 1);
}
#[test]
fn crafter_menu_toggles_empty_slots_and_preview_cannot_be_taken() {
    let (mut state, actor) = fixture(None);
    install(&mut state, "north_up");
    state.block_entities.get_mut(&KEY).unwrap().slots[0] = stack(&state, "oak_log", 2);
    run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            position: POS,
            hand: Hand::Main,
            face: 3,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    let menu = state.players[&actor].open.as_ref().unwrap();
    let window = menu.window;
    assert_eq!(menu.kind, MenuKind::Crafter);
    assert_eq!(menu.kind.menu_type(), Some(7));
    assert_eq!(menu.kind.slot_count(), 46);
    assert!(!menu.result.is_empty());
    run_tick(
        &mut state,
        &[
            PlayerInput::ContainerSlotState {
                connection: ConnectionId(1),
                window,
                slot: 1,
                enabled: false,
            },
            PlayerInput::ContainerSlotState {
                connection: ConnectionId(1),
                window,
                slot: 0,
                enabled: false,
            },
        ],
    );
    assert_eq!(state.crafters[&KEY].settings.disabled_slots, 2);
    let revision = state.players[&actor].open.as_ref().unwrap().state_id;
    run_tick(
        &mut state,
        &[PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window,
            state_id: revision,
            slot: 45,
            button: 0,
            mode: ClickMode::Pickup,
        }],
    );
    assert!(
        state.players[&actor]
            .open
            .as_ref()
            .unwrap()
            .carried
            .is_empty()
    );
    assert_eq!(state.block_entities[&KEY].slots[0].count, 2);
}
#[test]
fn crafter_wal_checkpoint_restart_retains_slots_disabled_and_pulse_latch_once() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let dir = std::env::temp_dir().join(format!("solaris-crafter-{}", uuid::Uuid::new_v4()));
    let (mut original, _) = fixture(Some(dir.clone()));
    install(&mut original, "north_up");
    let mut rich = stack(&original, "oak_log", 3);
    rich.custom_name = Some("retained".into());
    original.block_entities.get_mut(&KEY).unwrap().slots[0] = rich;
    original
        .crafters
        .get_mut(&KEY)
        .unwrap()
        .settings
        .disabled_slots = 0x1fe;
    original.crafters.get_mut(&KEY).unwrap().dirty = true;
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    let fixture_tick = original.tick;
    let entries = step(&mut original);
    writer
        .append(original.tick, original.tick, &entries)
        .unwrap();
    let base = original.tick;
    writer.gate(fixture_tick, base).unwrap();
    checkpoint(&mut original);
    let block_entities = crate::core::checkpoint_data_for(&original, original.tick).2;
    original
        .world
        .capture_dirty(&block_entities)
        .write()
        .unwrap();
    let projected = original.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    assert!(!projected.chests.contains_key(&POS));
    assert!(!projected.furnaces.contains_key(&POS));
    let records = mc_world::wire::client_block_entities(
        projected,
        &original.registry.blocks,
        &original.registry.items,
    );
    let crafter = records.iter().find(|record| record.pos == POS).unwrap();
    assert_eq!(crafter.type_name.as_str(), "minecraft:crafter");
    assert_eq!(
        mc_data::block_entity_types::solaris_required_block_entity_types()
            .id_of(&crafter.type_name),
        Some(43)
    );
    let registry = Arc::clone(&original.registry);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let mut native = ChunkWorld::new_native(
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
    native.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut native_crafters = BTreeMap::new();
    let mut native_custody = crate::redstone::custody::BlockEntities::default();
    let decoded = decode_chunk(
        &native_crafters,
        &registry,
        native.chunk(ChunkPos { x: 0, z: 0 }).unwrap(),
    )
    .unwrap();
    apply_chunk(&mut native_crafters, &mut native_custody, decoded);
    assert_eq!(
        native_crafters[&KEY].settings,
        original.crafters[&KEY].settings
    );
    assert_eq!(
        native_custody[&KEY].slots,
        original.block_entities[&KEY].slots
    );
    drop(native);
    pulse(&mut original);
    let entries = step(&mut original);
    writer
        .append(original.tick, original.tick, &entries)
        .unwrap();
    writer.gate(base, original.tick).unwrap();
    let batches = read_complete_batches_after(&dir.join("journal"), base).unwrap();
    let once = replay_batches(Replay::default(), &batches);
    let twice = replay_batches(replay_batches(Replay::default(), &batches), &batches);
    assert_eq!(once.opaque_block_entities, twice.opaque_block_entities);
    let (mut restored, _) = fixture(Some(dir.clone()));
    for (key, bytes) in twice.opaque_block_entities {
        restored.world.set_opaque_block_entity(position(key), bytes);
    }
    // fixture clears cells for player setup; restore the storage block identity.
    let id = block(
        &restored,
        "crafter",
        &[("orientation", "north_up"), ("triggered", "true")],
    );
    restored.world.set_block(POS.x, POS.y, POS.z, id);
    restored.world.fold_overlay();
    // Block writes clear opaque data; reinstall the final absolute WAL image.
    for (key, bytes) in once.opaque_block_entities {
        restored.world.set_opaque_block_entity(position(key), bytes);
    }
    let chunk = restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    let decoded = decode_chunk(&restored.crafters, &restored.registry, chunk).unwrap();
    apply_chunk(
        &mut restored.crafters,
        &mut restored.block_entities,
        decoded,
    );
    assert_eq!(
        restored.crafters[&KEY].settings,
        original.crafters[&KEY].settings
    );
    assert_eq!(
        restored.block_entities[&KEY].slots,
        original.block_entities[&KEY].slots
    );
    let before = restored.block_entities[&KEY].slots.clone();
    let redstone = block(&restored, "redstone_block", &[]);
    restored.world.set_block(9, 201, 8, redstone);
    scheduled(&mut restored, POS, 0, &mut Vec::new(), &mut Vec::new());
    assert_eq!(restored.block_entities[&KEY].slots, before);
    drop(writer);
    drop(original);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn stale_crafter_slot_toggle_after_block_removal_is_ignored() {
    let (mut state, actor) = fixture(None);
    install(&mut state, "north_up");
    run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            position: POS,
            hand: Hand::Main,
            face: 3,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    let window = state.players[&actor].open.as_ref().unwrap().window;
    state.block_entities.remove(&KEY);
    state.crafters.remove(&KEY);
    // Missing custody is rejected even while the block/menu association is stale.
    slot_state(&mut state, actor, window, 1, false);
    assert!(!state.block_entities.contains_key(&KEY));
    state
        .world
        .set_block(POS.x, POS.y, POS.z, state.registry.states.air);
    state.world.fold_overlay();
    run_tick(
        &mut state,
        &[PlayerInput::ContainerSlotState {
            connection: ConnectionId(1),
            window,
            slot: 1,
            enabled: false,
        }],
    );
    assert!(!state.block_entities.contains_key(&KEY));
    assert!(!state.crafters.contains_key(&KEY));
}

fn observe_crafter(state: &mut TickState) {
    let mut registry = CoreRegistry::load(None).unwrap();
    let mut tree = mc_data::advancements::AdvancementTree::default();
    tree.definitions.insert(
        "test:crafter".into(),
        serde_json::from_value(serde_json::json!({
            "criteria": {"craft": {"trigger": "minecraft:crafter_recipe_crafted"}},
            "requirements": [["craft"]],
        }))
        .unwrap(),
    );
    registry.advancements = Arc::new(tree);
    state.registry = Arc::new(registry);
}

#[test]
fn crafter_advancement_uses_pre_debit_recipe_and_vanilla_box_only_on_ejection() {
    for (position, awarded) in [
        ([8.5, 201.0, 10.5], true),
        ([16.9, 209.9, 16.9], true),
        ([17.2, 201.0, 8.5], true),
        ([17.4, 201.0, 8.5], false),
        ([8.5, 217.0, 8.5], false),
    ] {
        let (mut state, actor) = fixture(None);
        observe_crafter(&mut state);
        install(&mut state, "north_up");
        let slot = state.entities.slot_of(actor).unwrap();
        state.entities.set_pos(slot, position);
        state.block_entities.get_mut(&KEY).unwrap().slots[0] = stack(&state, "oak_log", 3);
        let ingredients = input(&state, POS);
        let recipe = state
            .registry
            .recipes
            .iter()
            .find(|recipe| {
                mc_data::recipes::crafting_recipe_matches(
                    &state.registry.items,
                    &state.registry.item_facts,
                    &state.registry.tags,
                    &ingredients,
                    recipe,
                )
            })
            .unwrap()
            .id
            .as_str()
            .to_owned();
        assert!(!result(&state, POS).is_empty());
        assert!(state.advancement_events.is_empty());
        pulse(&mut state);
        assert!(state.advancement_events.is_empty());
        let mut drops = Vec::new();
        scheduled(&mut state, POS, 8, &mut Vec::new(), &mut drops);
        assert_eq!(drops.len(), 1);
        assert_eq!(state.advancement_events.len(), usize::from(awarded));
        if awarded {
            let event = &state.advancement_events[0];
            assert_eq!(event.entity, actor);
            assert_eq!(event.trigger, "minecraft:crafter_recipe_crafted");
            assert_eq!(event.evidence["recipe_id"], recipe);
            assert_eq!(event.evidence["ingredients"][0]["id"], "minecraft:oak_log");
            assert_eq!(event.evidence["ingredients"][0]["count"], 3);
            assert_eq!(state.block_entities[&KEY].slots[0].count, 2);
        }
    }
}

#[test]
fn failed_and_fully_inserted_crafter_outputs_do_not_queue_advancement() {
    let (mut state, _) = fixture(None);
    observe_crafter(&mut state);
    install(&mut state, "north_up");
    let mut drops = Vec::new();
    craft(&mut state, POS, &mut Vec::new(), &mut drops);
    assert!(state.advancement_events.is_empty());
    assert!(drops.is_empty());
    let chest = block(&state, "chest", &[]);
    state.world.set_block(8, 201, 7, chest);
    state.block_entities.get_mut(&KEY).unwrap().slots[0] = stack(&state, "oak_log", 1);
    craft(&mut state, POS, &mut Vec::new(), &mut drops);
    assert!(drops.is_empty());
    assert!(state.advancement_events.is_empty());
    assert!(state.block_entities[&KEY].slots[0].is_empty());
    assert!(
        state.block_entities[&(8, 201, 7)]
            .slots
            .iter()
            .any(|item| item.count == 4)
    );
}

pub(crate) fn hold(
    state: &mut TickState,
    endpoint: mc_script::ScriptInventoryEndpoint,
    resource: &str,
    count: u64,
) {
    let operation_id = uuid::Uuid::new_v4().simple().to_string();
    let fence = crate::plugin_inventories::load(state, "custody-test", &endpoint)
        .unwrap()
        .fence;
    let request = mc_script::ScriptOperationRequest::try_new(
        &operation_id,
        mc_script::ScriptOperation::Inventory {
            operation: mc_script::ScriptOwnedInventoryOperation::Reserve {
                operation_id: operation_id.clone(),
                endpoint,
                resource_plan: mc_script::ScriptInventoryResourcePlan::new(vec![
                    mc_script::ScriptInventoryWorkPortion::new(
                        1,
                        vec![mc_script::ScriptInventoryMaterial::new(
                            format!("minecraft:{resource}"),
                            count,
                        )],
                    ),
                ]),
                expected_revision: fence,
            },
        },
    )
    .unwrap();
    let result = crate::plugin_inventories::execute(state, "custody-test", &request);
    assert_eq!(result.failure(), None);
}

pub(crate) fn hold_block(state: &mut TickState, pos: BlockPos, resource: &str, count: u64) {
    let endpoint = mc_script::ScriptInventoryEndpoint::Container {
        dimension: state.dimension.name().into(),
        position: mc_script::ScriptBlockPosition::new(pos.x, pos.y, pos.z),
    };
    hold(state, endpoint, resource, count);
}

#[test]
fn crafter_consumes_free_inputs_but_never_the_reserved_floor() {
    let (mut state, _) = fixture(None);
    install(&mut state, "north_up");
    state.block_entities.get_mut(&KEY).unwrap().slots[0] = stack(&state, "oak_log", 2);
    state
        .crafters
        .get_mut(&KEY)
        .unwrap()
        .settings
        .disabled_slots = 0x1fe;
    hold_block(&mut state, POS, "oak_log", 1);
    let mut drops = Vec::new();
    craft(&mut state, POS, &mut Vec::new(), &mut drops);
    assert_eq!(state.block_entities[&KEY].slots[0].count, 1);
    assert_eq!(drops.len(), 1);
    craft(&mut state, POS, &mut Vec::new(), &mut drops);
    assert_eq!(state.block_entities[&KEY].slots[0].count, 1);
    assert_eq!(drops.len(), 1);
    assert!(crate::plugin_inventories::has_reservations_for_block(
        &state, POS
    ));
}

#[test]
fn banner_copy_ejects_output_and_original_without_destroying_source() {
    let (mut state, _) = fixture(None);
    install(&mut state, "north_up");
    let mut source = stack(&state, "white_banner", 1);
    source
        .extended_mut()
        .banner_patterns
        .push(mc_data::item_stack::BannerPattern {
            pattern: mc_data::Identifier::parse("minecraft:stripe_bottom").unwrap(),
            color: 14,
        });
    let blank = stack(&state, "white_banner", 1);
    state.block_entities.get_mut(&KEY).unwrap().slots[0] = source.clone();
    state.block_entities.get_mut(&KEY).unwrap().slots[1] = blank;
    pulse(&mut state);
    let mut drops = Vec::new();
    scheduled(&mut state, POS, 8, &mut Vec::new(), &mut drops);
    assert!(
        state.block_entities[&KEY]
            .slots
            .iter()
            .all(ItemStack::is_empty)
    );
    assert_eq!(drops.len(), 2);
    for entity in drops {
        let slot = state.entities.slot_of(entity).unwrap();
        assert!(matches!(
            &state.entities.body(slot).kind,
            crate::entities::EntityKind::Item { .. }
        ));
        assert_eq!(
            state
                .entities
                .row(slot)
                .item
                .as_ref()
                .and_then(|item| item.stack.as_ref()),
            Some(&source)
        );
    }
}
