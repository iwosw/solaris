use super::*;
use crate::crafter::tests::{block, fixture, stack};
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };
const KEY: (i32, i32, i32) = (8, 201, 8);
const RECIPE: &str = "minecraft:iron_ingot_from_smelting_raw_iron";

fn install(state: &mut TickState) {
    let furnace = block(state, "furnace", &[]);
    state.world.set_block(POS.x, POS.y, POS.z, furnace);
    state.world.fold_overlay();
    state.block_entities.insert(
        KEY,
        BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 3],
            furnace: Some(crate::inventory::furnace::FurnaceState {
                recipes_used: Arc::new([(RECIPE.into(), 10)].into_iter().collect()),
                ..Default::default()
            }),
        },
    );
}

#[test]
fn furnace_bank_survives_wal_and_native_chunk_checkpoint() {
    let dir = std::env::temp_dir().join(format!("furnace-bank-{}", uuid::Uuid::new_v4()));
    let (mut state, _) = fixture(Some(dir.clone()));
    install(&mut state);
    let record = crate::journal::ReplayBlockEntity {
        slots: state.block_entities[&KEY].slots.clone(),
        furnace: state.block_entities[&KEY].furnace_image(),
        tick: state.tick,
    };
    let encoded = serde_json::to_vec(&crate::journal::JournalEntry::BlockEntity {
        pos: [POS.x, POS.y, POS.z],
        slots: record.slots.clone(),
        furnace: record.furnace.clone(),
        tick: state.tick,
    })
    .unwrap();
    let entry: crate::journal::JournalEntry = serde_json::from_slice(&encoded).unwrap();
    let crate::journal::JournalEntry::BlockEntity {
        furnace: Some(image),
        ..
    } = entry
    else {
        panic!("furnace postimage");
    };
    assert_eq!(image.recipes_used[RECIPE], 10);
    state.world.mark_block_entity_dirty(POS);
    state
        .world
        .capture_dirty(&[(KEY, record)].into_iter().collect())
        .write()
        .unwrap();
    let registry = Arc::clone(&state.registry);
    drop(state);
    let config = crate::CoreConfig::new(dir.clone(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let mut world = crate::world::ChunkWorld::new_native(
        42,
        store,
        0,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 201, 10],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut restored = TickState::new(registry, world, 0, 512);
    sync_chunk_block_entities(&mut restored);
    let rules = restored.registry.item_rules();
    let bank = restored
        .block_entities
        .get_mut(&KEY)
        .unwrap()
        .furnace
        .as_mut()
        .unwrap();
    assert_eq!(bank.take_experience(&rules, 0), 7);
    assert_eq!(bank.take_experience(&rules, 0), 0);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn breaking_furnace_releases_banked_experience_once_even_after_hopper_extraction() {
    let (mut state, actor) = fixture(None);
    install(&mut state);
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    let input = PlayerInput::Dig {
        connection: ConnectionId(1),
        position: POS,
        action: DigAction::Start,
        sequence: 1,
    };
    let (output, _, _) = run_tick(&mut state, std::slice::from_ref(&input));
    let xp: i32 = output
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| {
            let slot = state.entities.slot_of(*id)?;
            state
                .entities
                .row(slot)
                .appearance
                .experience_orb
                .map(|(value, count)| value * count)
        })
        .sum();
    assert_eq!(xp, 7);
    assert!(state.block_entities.get(&KEY).is_none());
    let (second, _, _) = run_tick(&mut state, &[input]);
    assert!(second.changes.entities_spawned.is_empty());
}

#[test]
fn furnace_recipe_book_places_input_and_preserves_fuel_output() {
    let (mut state, actor) = fixture(None);
    install(&mut state);
    let raw = stack(&state, "raw_iron", 5);
    let coal = stack(&state, "coal", 2);
    let output = stack(&state, "iron_ingot", 3);
    state.block_entities.get_mut(&KEY).unwrap().slots =
        vec![mc_data::ItemStack::EMPTY, coal.clone(), output.clone()];
    let registry = Arc::clone(&state.registry);
    let recipe = registry
        .recipes
        .iter()
        .position(|r| r.id.as_str() == RECIPE)
        .unwrap() as u32;
    let player = state.players.get_mut(&actor).unwrap();
    Arc::make_mut(&mut player.unlocked_recipes).insert(RECIPE.into());
    player.inventory.slots[0] = raw;
    let slots = state.block_entities[&KEY].slots.clone();
    state.players.get_mut(&actor).unwrap().open_menu(
        &registry.item_rules(),
        crate::inventory::MenuKind::Furnace(mc_data::recipes::CookingKind::Furnace),
        "Furnace".into(),
        &slots,
    );
    state.open_containers.insert(actor, KEY);
    let window = state.players[&actor].open.as_ref().unwrap().window;
    input::inventory::place_recipe(
        &mut state,
        (&ConnectionId(1), &window, &recipe, &true),
        &mut ChangeSet::default(),
    );
    assert_eq!(
        state.block_entities[&KEY].slots,
        vec![stack(&state, "raw_iron", 5), coal, output]
    );
    assert!(state.players[&actor].inventory.slots[0].is_empty());
}
