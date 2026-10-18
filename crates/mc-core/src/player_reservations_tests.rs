use std::cell::Cell;
use std::sync::Arc;

use mc_data::statistics::{CustomStat, StatType};
use mc_data::{Identifier, ItemStack};
use mc_script::{
    ScriptBlockPosition, ScriptInventoryEndpoint, ScriptInventoryMaterial,
    ScriptInventoryResourcePlan, ScriptInventoryWorkPortion, ScriptOperation,
    ScriptOperationPayload, ScriptOperationRequest, ScriptOwnedInventoryOperation,
    ScriptOwnedInventoryResult,
};

use super::{PlayerEffects, PlayerState};
use crate::boundary::{BlockPos, ClickMode, ConnectionId, EntityId, Hand, PlayerInput};
use crate::commands::GameMode;
use crate::inventory::{ItemRules, MenuKind};
use crate::plugin_inventories as inventories;
use crate::tick::TickState;

fn state() -> (TickState, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let terrain = mc_worldgen::TerrainGenerator::new(42, registry.blocks.clone());
    let spawn = terrain.locate_safe_spawn().unwrap();
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(terrain),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(7),
            uuid: uuid::Uuid::from_u128(7),
            name: "native-player-reservations".into(),
            permission_level: 0,
        }],
    );
    let desired = state.desired_chunks();
    state.world.ensure_blocking(&desired);
    let entity = state.connections.entity_of(ConnectionId(7)).unwrap();
    (state, entity)
}

fn item(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap(),
        count,
    )
}

fn player_endpoint() -> ScriptInventoryEndpoint {
    ScriptInventoryEndpoint::PlayerInventory { player_id: 7 }
}

fn reserve(
    state: &mut TickState,
    endpoint: &ScriptInventoryEndpoint,
    materials: &[(&str, u64)],
) -> (String, u64) {
    let operation_id = uuid::Uuid::new_v4().simple().to_string();
    let request = ScriptOperationRequest::try_new(
        &operation_id,
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Reserve {
                operation_id: operation_id.clone(),
                endpoint: endpoint.clone(),
                resource_plan: ScriptInventoryResourcePlan::new(vec![
                    ScriptInventoryWorkPortion::new(
                        1,
                        materials
                            .iter()
                            .map(|(name, count)| {
                                ScriptInventoryMaterial::new((*name).into(), *count)
                            })
                            .collect(),
                    ),
                ]),
                expected_revision: inventories::load(state, "native-test", endpoint)
                    .unwrap()
                    .fence,
            },
        },
    )
    .unwrap();
    let result = inventories::execute(state, "native-test", &request);
    assert_eq!(result.failure(), None);
    let ScriptOperationPayload::OwnedInventory { result: payload } = result.payload() else {
        panic!("native inventory payload required")
    };
    let ScriptOwnedInventoryResult::Reservation { reservation } = payload.as_ref() else {
        panic!("native reservation result required")
    };
    (
        reservation.reservation_ref.clone(),
        result.revision().unwrap(),
    )
}

fn release(state: &mut TickState, reservation: (String, u64)) {
    let request = ScriptOperationRequest::try_new(
        "release-native-test",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Release {
                operation_id: "release-native-test".into(),
                reservation_ref: reservation.0,
                expected_revision: reservation.1,
            },
        },
    )
    .unwrap();
    assert_eq!(
        inventories::execute(state, "native-test", &request).failure(),
        None
    );
}

fn chest(state: &mut TickState, contents: ItemStack) -> (BlockPos, ScriptInventoryEndpoint) {
    let spawn = state.world.world_spawn();
    let pos = BlockPos {
        x: spawn[0],
        y: 200,
        z: spawn[2],
    };
    let block = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:chest").unwrap())
        .unwrap()
        .default
        .0;
    crate::block_entity_defaults::accepted_placement(state, pos, block);
    state.world.set_block(pos.x, pos.y, pos.z, block);
    crate::block_entity_defaults::after_blocks(state, &[(pos, block)]);
    state
        .block_entities
        .get_mut(&(pos.x, pos.y, pos.z))
        .unwrap()
        .slots[0] = contents;
    (
        pos,
        ScriptInventoryEndpoint::Container {
            dimension: state.dimension.name().into(),
            position: ScriptBlockPosition::new(pos.x, pos.y, pos.z),
        },
    )
}

/// The same three-step contract as the owner: run the actual native engine
/// once, admit both sources, then commit. No test substitute for native clicks.
fn transition(
    state: &mut TickState,
    entity: EntityId,
    container_pos: Option<BlockPos>,
    native: impl FnOnce(&mut PlayerState, &mut [ItemStack]) -> PlayerEffects,
) -> (bool, PlayerEffects) {
    let mut container = container_pos
        .map(|pos| state.block_entities[&(pos.x, pos.y, pos.z)].slots.clone())
        .unwrap_or_default();
    let plan = state.players[&entity].plan_inventory_transition(&container, native);
    let refused = inventories::below_player_floors(state, entity, plan.player_slots())
        || container_pos
            .is_some_and(|pos| inventories::below_block_floors(state, pos, plan.container_slots()));
    if refused {
        return (
            false,
            state.players[&entity].inventory_transition_refused(&container),
        );
    }
    let effects = state
        .players
        .get_mut(&entity)
        .unwrap()
        .apply_inventory_transition(plan, &mut container);
    if effects.container_changed
        && let Some(pos) = container_pos
    {
        state
            .block_entities
            .get_mut(&(pos.x, pos.y, pos.z))
            .unwrap()
            .slots = container;
    }
    (true, effects)
}

#[test]
fn native_click_throw_uses_excess_then_refuses_held_unit_and_release_unblocks() {
    let (mut state, entity) = state();
    let stack = item(&state, "minecraft:emerald", 3);
    let emerald_id = stack.item_id;
    let drop_actions = state.players[&entity]
        .statistics
        .get_custom(CustomStat::Drop);
    state
        .players
        .get_mut(&entity)
        .unwrap()
        .statistics
        .increment(StatType::Dropped, emerald_id, 4);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = stack;
    let held = reserve(&mut state, &player_endpoint(), &[("minecraft:emerald", 2)]);
    let registry = Arc::clone(&state.registry);
    let calls = Cell::new(0);
    let (accepted, effects) = transition(&mut state, entity, None, |player, container| {
        calls.set(calls.get() + 1);
        player.click(
            &registry.item_rules(),
            0,
            0,
            36,
            0,
            ClickMode::Throw,
            container,
        )
    });
    assert!(accepted);
    assert_eq!(calls.get(), 1);
    assert_eq!(effects.dropped[0].count, 1);
    assert_eq!(state.players[&entity].inventory.slots[0].count, 2);
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::Dropped, emerald_id),
        5
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::Drop),
        drop_actions + 1
    );
    assert_eq!(
        inventories::reserved(&state, &player_endpoint())["minecraft:emerald"],
        2
    );
    let before = state.players[&entity].clone();
    let (accepted, effects) = transition(&mut state, entity, None, |player, container| {
        player.click(
            &registry.item_rules(),
            0,
            before.inventory_menu.state_id,
            36,
            0,
            ClickMode::Throw,
            container,
        )
    });
    assert!(!accepted);
    assert_eq!(state.players[&entity], before);
    assert!(effects.dropped.is_empty());
    assert!(!effects.container_changed);
    assert!(!effects.furnace_output_taken);
    assert!(!effects.station_output_taken);
    assert_eq!(effects.changes.len(), 1);
    release(&mut state, held);
    let (accepted, effects) = transition(&mut state, entity, None, |player, container| {
        player.click(
            &registry.item_rules(),
            0,
            before.inventory_menu.state_id,
            36,
            0,
            ClickMode::Throw,
            container,
        )
    });
    assert!(accepted);
    assert_eq!(effects.dropped[0].count, 1);
    assert_eq!(state.players[&entity].inventory.slots[0].count, 1);
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::Dropped, emerald_id),
        6
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::Drop),
        drop_actions + 2
    );
}

#[test]
fn staged_craft_commits_statistics_and_both_observations_once() {
    let (mut state, entity) = state();
    let emerald = item(&state, "minecraft:emerald", 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = emerald;
    reserve(&mut state, &player_endpoint(), &[("minecraft:emerald", 1)]);
    let log = item(&state, "minecraft:oak_log", 1);
    let planks = item(&state, "minecraft:oak_planks", 4);
    let plank_id = planks.item_id;
    let player = state.players.get_mut(&entity).unwrap();
    player.inventory_menu.grid[0] = log.clone();
    player.inventory_menu.result = planks.clone();
    player.statistics.increment(StatType::Crafted, plank_id, 8);
    let registry = Arc::clone(&state.registry);
    let calls = Cell::new(0);
    let (accepted, effects) = transition(&mut state, entity, None, |player, container| {
        calls.set(calls.get() + 1);
        player.click(
            &registry.item_rules(),
            0,
            player.inventory_menu.state_id,
            0,
            0,
            ClickMode::Pickup,
            container,
        )
    });
    assert!(accepted);
    assert_eq!(calls.get(), 1);
    assert_eq!(effects.crafted, vec![(plank_id, 4, MenuKind::Player)]);
    assert_eq!(effects.crafted_recipes.len(), 1);
    assert_eq!(effects.crafted_recipes[0].1[0], log);
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::Crafted, plank_id),
        12
    );
    assert_eq!(state.players[&entity].inventory_menu.carried, planks);
    assert!(state.players[&entity].inventory_menu.grid[0].is_empty());
}

#[test]
fn native_slot_reorder_preserves_usable_reserved_stock() {
    let (mut state, entity) = state();
    let stack = item(&state, "minecraft:emerald", 2);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = stack;
    let held = reserve(&mut state, &player_endpoint(), &[("minecraft:emerald", 2)]);
    let registry = Arc::clone(&state.registry);
    // Number-key swapping slot 9 with hotbar 1 is one atomic native reorder.
    let (accepted, effects) = transition(&mut state, entity, None, |player, container| {
        player.click(
            &registry.item_rules(),
            0,
            0,
            9,
            1,
            ClickMode::Swap,
            container,
        )
    });
    assert!(accepted);
    assert!(effects.dropped.is_empty());
    assert!(state.players[&entity].inventory.slots[9].is_empty());
    assert_eq!(state.players[&entity].inventory.slots[1].count, 2);
    let current = inventories::reservation(&state, "native-test", &held.0).unwrap();
    assert!(!current.snapshot.released);
    assert_eq!(current.snapshot.quantities[0].remaining, 2);
    let usable = inventories::load(&state, "native-test", &player_endpoint()).unwrap();
    assert_eq!(usable.slots[&37].count, 2);
}

#[test]
fn physical_container_excess_take_commits_both_sides_last_unit_refuses_both() {
    let (mut state, entity) = state();
    let stock = item(&state, "minecraft:emerald", 3);
    let (pos, endpoint) = chest(&mut state, stock);
    reserve(&mut state, &endpoint, &[("minecraft:emerald", 2)]);
    let registry = Arc::clone(&state.registry);
    let initial = state.block_entities[&(pos.x, pos.y, pos.z)].slots.clone();
    state.players.get_mut(&entity).unwrap().open_menu(
        &registry.item_rules(),
        MenuKind::Chest { rows: 3 },
        "Held chest".into(),
        &initial,
    );
    let window = state.players[&entity].open.as_ref().unwrap().window;
    // Right-click picking up half of three would take two and cross the floor.
    let before = state.players[&entity].clone();
    let (accepted, effects) = transition(&mut state, entity, Some(pos), |player, container| {
        player.click(
            &registry.item_rules(),
            window,
            0,
            0,
            1,
            ClickMode::Pickup,
            container,
        )
    });
    assert!(!accepted);
    assert_eq!(state.players[&entity], before);
    assert_eq!(state.block_entities[&(pos.x, pos.y, pos.z)].slots, initial);
    assert!(effects.dropped.is_empty());
    // A native single-item throw removes only the excess from that same source.
    let (accepted, effects) = transition(&mut state, entity, Some(pos), |player, container| {
        player.click(
            &registry.item_rules(),
            window,
            0,
            0,
            0,
            ClickMode::Throw,
            container,
        )
    });
    assert!(accepted);
    assert!(effects.container_changed);
    assert_eq!(effects.dropped[0].count, 1);
    assert_eq!(
        state.block_entities[&(pos.x, pos.y, pos.z)].slots[0].count,
        2
    );
    let before = state.players[&entity].clone();
    let storage = state.block_entities[&(pos.x, pos.y, pos.z)].slots.clone();
    let revision = before.open.as_ref().unwrap().state_id;
    let (accepted, effects) = transition(&mut state, entity, Some(pos), |player, container| {
        player.click(
            &registry.item_rules(),
            window,
            revision,
            0,
            0,
            ClickMode::Throw,
            container,
        )
    });
    assert!(!accepted);
    assert_eq!(state.players[&entity], before);
    assert_eq!(state.block_entities[&(pos.x, pos.y, pos.z)].slots, storage);
    assert!(effects.dropped.is_empty());
    assert!(!effects.container_changed);
    assert_eq!(
        inventories::reserved(&state, &endpoint)["minecraft:emerald"],
        2
    );
}

#[test]
fn player_to_container_move_checks_player_floor_before_either_side_changes() {
    let (mut state, entity) = state();
    let empty = ItemStack::EMPTY;
    let (pos, _) = chest(&mut state, empty);
    let stock = item(&state, "minecraft:emerald", 2);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = stock;
    reserve(&mut state, &player_endpoint(), &[("minecraft:emerald", 2)]);
    let registry = Arc::clone(&state.registry);
    let storage = state.block_entities[&(pos.x, pos.y, pos.z)].slots.clone();
    state.players.get_mut(&entity).unwrap().open_menu(
        &registry.item_rules(),
        MenuKind::Chest { rows: 3 },
        "Chest".into(),
        &storage,
    );
    let before = state.players[&entity].clone();
    let window = before.open.as_ref().unwrap().window;
    let (accepted, effects) = transition(&mut state, entity, Some(pos), |player, container| {
        player.click(
            &registry.item_rules(),
            window,
            0,
            27,
            0,
            ClickMode::QuickMove,
            container,
        )
    });
    assert!(!accepted);
    assert_eq!(state.players[&entity], before);
    assert_eq!(state.block_entities[&(pos.x, pos.y, pos.z)].slots, storage);
    assert!(!effects.container_changed);
    assert!(effects.dropped.is_empty());
}

#[test]
fn native_recipe_placement_admits_multiple_excess_resources_atomically() {
    use mc_data::recipes::{
        Ingredient, IngredientAlternative, Recipe, RecipeKind, RecipeResult, ShapelessRecipe,
    };
    let (mut state, entity) = state();
    let emerald = item(&state, "minecraft:emerald", 2);
    let diamond = item(&state, "minecraft:diamond", 2);
    let player = state.players.get_mut(&entity).unwrap();
    player.inventory.slots[9] = emerald;
    player.inventory.slots[10] = diamond;
    reserve(
        &mut state,
        &player_endpoint(),
        &[("minecraft:emerald", 1), ("minecraft:diamond", 1)],
    );
    let recipes = vec![Recipe {
        id: Identifier::parse("native-test:two-materials").unwrap(),
        show_notification: true,
        kind: RecipeKind::Shapeless(ShapelessRecipe {
            ingredients: ["minecraft:emerald", "minecraft:diamond"]
                .into_iter()
                .map(|name| Ingredient {
                    alternatives: vec![IngredientAlternative::Item(
                        Identifier::parse(name).unwrap(),
                    )],
                })
                .collect(),
        }),
        result: RecipeResult {
            item: Identifier::parse("minecraft:stick").unwrap(),
            count: 1,
            stew_effects: Vec::new(),
        },
    }];
    Arc::make_mut(&mut state.players.get_mut(&entity).unwrap().unlocked_recipes)
        .insert(recipes[0].id.as_str().to_owned());
    let registry = Arc::clone(&state.registry);
    let base = registry.item_rules();
    let rules = ItemRules {
        recipes: &recipes,
        ..base
    };
    let (accepted, _) = transition(&mut state, entity, None, |player, _| {
        player.place_recipe(&rules, 0, 0, false)
    });
    assert!(accepted);
    assert_eq!(state.players[&entity].inventory.slots[9].count, 1);
    assert_eq!(state.players[&entity].inventory.slots[10].count, 1);
    assert_eq!(
        state.players[&entity]
            .inventory_menu
            .grid
            .iter()
            .filter(|stack| !stack.is_empty())
            .count(),
        2
    );
    let before = state.players[&entity].clone();
    let (accepted, effects) = transition(&mut state, entity, None, |player, _| {
        player.place_recipe(&rules, 0, 0, false)
    });
    assert!(!accepted);
    assert_eq!(state.players[&entity], before);
    assert!(effects.dropped.is_empty());
    assert_eq!(inventories::reserved(&state, &player_endpoint()).len(), 2);
}

#[test]
fn native_component_wear_preserves_hold_but_break_plan_has_no_rng_or_equipment_effect() {
    let (mut state, entity) = state();
    let mut tool = item(&state, "minecraft:wooden_pickaxe", 1);
    let tool_id = tool.item_id;
    tool.damage = Some(1);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = tool;
    let held = reserve(
        &mut state,
        &player_endpoint(),
        &[("minecraft:wooden_pickaxe", 1)],
    );
    let registry = Arc::clone(&state.registry);
    let plan = state.players[&entity].plan_held_tool_wear(&registry.item_rules(), 1);
    assert_eq!(plan.broken_slot(), None);
    assert!(!inventories::below_player_debit_floors(
        &state,
        entity,
        &[],
        plan.broken_slot()
    ));
    let effects = state
        .players
        .get_mut(&entity)
        .unwrap()
        .apply_tool_wear(plan);
    assert!(!effects.changes.is_empty());
    assert_eq!(state.players[&entity].inventory.slots[0].damage, Some(2));
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::Broken, tool_id),
        0
    );
    let reservation = inventories::reservation(&state, "native-test", &held.0).unwrap();
    assert!(!reservation.snapshot.released);
    let before = state.players[&entity].clone();
    let plan = before.plan_held_tool_wear(&registry.item_rules(), 1000);
    assert_eq!(plan.broken_slot(), Some(0));
    assert!(inventories::below_player_debit_floors(
        &state,
        entity,
        &[],
        plan.broken_slot()
    ));
    assert_eq!(state.players[&entity], before);
    release(&mut state, held);
    let plan = state.players[&entity].plan_held_tool_wear(&registry.item_rules(), 1000);
    assert!(!inventories::below_player_debit_floors(
        &state,
        entity,
        &[],
        plan.broken_slot()
    ));
    state
        .players
        .get_mut(&entity)
        .unwrap()
        .apply_tool_wear(plan);
    assert!(state.players[&entity].inventory.slots[0].is_empty());
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::Broken, tool_id),
        1
    );
}

#[test]
fn creative_and_spectator_tool_wear_preserve_stack_and_skip_unbreaking_rng() {
    let (mut state, entity) = state();
    let mut sword = item(&state, "minecraft:wooden_sword", 1);
    sword.damage = Some(58);
    sword.enchantments.push(mc_data::ItemEnchantment {
        id: Identifier::parse("minecraft:unbreaking").unwrap(),
        level: 3,
    });
    let registry = Arc::clone(&state.registry);
    for mode in [GameMode::Creative, GameMode::Spectator] {
        let player = state.players.get_mut(&entity).unwrap();
        player.game_mode = mode;
        player.inventory.slots[0] = sword.clone();
        player.inventory.slots[crate::inventory::OFF_HAND] = sword.clone();
        for hand in [Hand::Main, Hand::Off] {
            let before = player.clone();
            let plan = player.plan_tool_wear(&registry.item_rules(), hand, 1000);
            assert_eq!(plan.broken_slot(), None);
            let effects = player.apply_tool_wear(plan);
            assert_eq!(*player, before);
            assert!(effects.changes.is_empty());
        }
        // A preceding combat draw remains committed, but exempt durability
        // must neither advance it again nor mutate the worn weapon.
        let mut expected = player.clone();
        let seed = crate::inventory::enchanting::next_seed(player.enchantment_seed);
        expected.enchantment_seed = seed;
        let plan = player.plan_held_tool_wear_from_seed(&registry.item_rules(), 1000, seed);
        assert_eq!(plan.broken_slot(), None);
        let effects = player.apply_tool_wear(plan);
        assert_eq!(*player, expected);
        assert!(effects.changes.is_empty());
    }
}

#[test]
fn sparse_drop_creative_and_hand_swap_refuse_only_source_drawdown() {
    let (mut state, entity) = state();
    let stock = item(&state, "minecraft:emerald", 3);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = stock;
    reserve(&mut state, &player_endpoint(), &[("minecraft:emerald", 2)]);
    let debit = state.players[&entity].drop_selected_debit(false).unwrap();
    assert!(!inventories::below_player_debit_floors(
        &state,
        entity,
        &[debit],
        None
    ));
    let effects = state.players.get_mut(&entity).unwrap().drop_selected(false);
    assert_eq!(effects.dropped[0].count, 1);
    let debit = state.players[&entity].drop_selected_debit(false).unwrap();
    assert!(inventories::below_player_debit_floors(
        &state,
        entity,
        &[debit],
        None
    ));
    let replacements = state.players[&entity].hand_swap_replacements().unwrap();
    assert!(inventories::below_player_replacement_floors(
        &state,
        entity,
        &replacements
    ));
    // Spare ordinary stock allows the selected stack to move to offhand.
    let spare = item(&state, "minecraft:emerald", 2);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = spare;
    let replacements = state.players[&entity].hand_swap_replacements().unwrap();
    assert!(!inventories::below_player_replacement_floors(
        &state,
        entity,
        &replacements
    ));
    state.players.get_mut(&entity).unwrap().swap_hands();
    assert_eq!(state.players[&entity].inventory.slots[40].count, 2);
    state.players.get_mut(&entity).unwrap().game_mode = GameMode::Creative;
    let registry = Arc::clone(&state.registry);
    let replacement = ItemStack::EMPTY;
    let overlay = state.players[&entity]
        .creative_slot_replacement(&registry.item_rules(), 9, &replacement)
        .unwrap();
    assert!(inventories::below_player_replacement_floors(
        &state,
        entity,
        &[overlay]
    ));
    let retained = item(&state, "minecraft:emerald", 2);
    let overlay = state.players[&entity]
        .creative_slot_replacement(&registry.item_rules(), 9, &retained)
        .unwrap();
    assert!(!inventories::below_player_replacement_floors(
        &state,
        entity,
        &[overlay]
    ));
    assert!(
        state.players[&entity]
            .creative_slot_replacement(&registry.item_rules(), -1, &retained,)
            .is_none()
    );
}

#[test]
fn finishing_food_debit_refuses_before_vitals_effects_use_or_seed_mutate() {
    let (mut state, entity) = state();
    let food = item(&state, "minecraft:apple", 1);
    let player = state.players.get_mut(&entity).unwrap();
    player.inventory.slots[0] = food;
    player.vitals.food = 10;
    let held = reserve(&mut state, &player_endpoint(), &[("minecraft:apple", 1)]);
    let registry = Arc::clone(&state.registry);
    let player = state.players.get_mut(&entity).unwrap();
    player.use_item(&registry.item_rules(), Hand::Main);
    for _ in 0..31 {
        player.tick_item_use(&registry.item_rules());
    }
    let before = state.players[&entity].clone();
    let debit = before.finishing_consumable_debit().unwrap();
    assert!(inventories::below_player_debit_floors(
        &state,
        entity,
        &[debit],
        None
    ));
    assert_eq!(state.players[&entity], before);
    release(&mut state, held);
    assert!(!inventories::below_player_debit_floors(
        &state,
        entity,
        &[debit],
        None
    ));
    let effects = state
        .players
        .get_mut(&entity)
        .unwrap()
        .tick_item_use(&registry.item_rules());
    assert!(state.players[&entity].inventory.slots[0].is_empty());
    assert!(state.players[&entity].vitals.food > 10);
    assert!(!effects.changes.is_empty());
}

#[test]
fn equipment_swap_requires_only_addressable_source_excess_and_commits_once() {
    let (mut state, entity) = state();
    let helmet = item(&state, "minecraft:diamond_helmet", 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = helmet;
    let held = reserve(
        &mut state,
        &player_endpoint(),
        &[("minecraft:diamond_helmet", 1)],
    );
    let registry = Arc::clone(&state.registry);
    let player_type = crate::entities::player_type_id() as i32;
    let before = state.players[&entity].clone();
    let (accepted, effects) = transition(&mut state, entity, None, |player, _| {
        player
            .swap_equippable(&registry.item_rules(), Hand::Main, player_type)
            .expect("helmet owns equippable transition")
    });
    assert!(!accepted);
    assert_eq!(state.players[&entity], before);
    assert!(effects.dropped.is_empty());
    let spare = item(&state, "minecraft:diamond_helmet", 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = spare;
    let (accepted, effects) = transition(&mut state, entity, None, |player, _| {
        player
            .swap_equippable(&registry.item_rules(), Hand::Main, player_type)
            .expect("helmet owns equippable transition")
    });
    assert!(accepted);
    assert!(effects.dropped.is_empty());
    assert!(state.players[&entity].inventory.slots[0].is_empty());
    assert_eq!(state.players[&entity].inventory.slots[39].count, 1);
    assert_eq!(state.players[&entity].inventory.slots[9].count, 1);
    assert!(
        !inventories::reservation(&state, "native-test", &held.0)
            .unwrap()
            .snapshot
            .released
    );
}

#[test]
fn denied_unbreaking_break_retains_real_rng_seed_and_native_component_stack() {
    let (mut state, entity) = state();
    let mut tool = item(&state, "minecraft:wooden_pickaxe", 1);
    tool.enchantments.push(mc_data::ItemEnchantment {
        id: Identifier::parse("minecraft:unbreaking").unwrap(),
        level: 3,
    });
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = tool;
    reserve(
        &mut state,
        &player_endpoint(),
        &[("minecraft:wooden_pickaxe", 1)],
    );
    let registry = Arc::clone(&state.registry);
    let before = state.players[&entity].clone();
    let prior_draw_seed = crate::inventory::enchanting::next_seed(before.enchantment_seed);
    let plan = before.plan_held_tool_wear_from_seed(&registry.item_rules(), 1000, prior_draw_seed);
    assert_eq!(plan.broken_slot(), Some(0));
    assert!(inventories::below_player_debit_floors(
        &state,
        entity,
        &[],
        plan.broken_slot()
    ));
    let mut native_result = before.clone();
    native_result.enchantment_seed = prior_draw_seed;
    native_result.damage_held_tool(&registry.item_rules(), 1000);
    assert_ne!(native_result.enchantment_seed, before.enchantment_seed);
    assert!(native_result.inventory.slots[0].is_empty());
    // Applying the prepared result on an isolated native player gives precisely
    // the same outcome as vanilla starting after the preceding scalar draw.
    let mut planned_result = before.clone();
    planned_result.apply_tool_wear(plan);
    assert_eq!(planned_result, native_result);
    assert_eq!(state.players[&entity], before);
}

#[test]
fn enchanting_button_with_unrelated_holds_commits_native_xp_seed_and_lapis() {
    let (mut state, entity) = state();
    let emeralds = item(&state, "minecraft:emerald", 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = emeralds;
    reserve(&mut state, &player_endpoint(), &[("minecraft:emerald", 1)]);
    let sword = item(&state, "minecraft:diamond_sword", 1);
    let lapis = item(&state, "minecraft:lapis_lazuli", 3);
    let registry = Arc::clone(&state.registry);
    let player = state.players.get_mut(&entity).unwrap();
    player.open_menu(
        &registry.item_rules(),
        MenuKind::EnchantingTable,
        "Enchanting".into(),
        &[],
    );
    player.vitals.experience.add_levels(30);
    let menu = player.open.as_mut().unwrap();
    menu.grid[0] = sword;
    menu.grid[1] = lapis;
    player.initialize_station(&registry.enchantments, &registry.item_rules(), 15);
    let window = player.open.as_ref().unwrap().window;
    let before_seed = player.enchantment_seed;
    let (accepted, effects) = transition(&mut state, entity, None, |player, _| {
        player.station_button(
            &registry.enchantments,
            &registry.item_rules(),
            window,
            0,
            15,
        )
    });
    assert!(accepted);
    assert!(!effects.changes.is_empty());
    assert_eq!(state.players[&entity].vitals.experience.level, 29);
    assert_ne!(state.players[&entity].enchantment_seed, before_seed);
    let menu = state.players[&entity].open.as_ref().unwrap();
    assert_eq!(menu.grid[1].count, 2);
    assert!(!menu.grid[0].enchantments.is_empty());
    assert_eq!(state.players[&entity].inventory.slots[9].count, 1);
    assert_eq!(
        inventories::reserved(&state, &player_endpoint())["minecraft:emerald"],
        1
    );
}

#[test]
fn native_tick_drop_route_admits_excess_refuses_floor_then_release_allows_action() {
    let (mut state, entity) = state();
    let emeralds = item(&state, "minecraft:emerald", 3);
    let emerald_id = emeralds.item_id;
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = emeralds;
    let held = reserve(&mut state, &player_endpoint(), &[("minecraft:emerald", 2)]);
    let input = PlayerInput::DropSelected {
        connection: ConnectionId(7),
        whole_stack: false,
    };
    let (accepted, _, _) = crate::tick::run_tick(&mut state, std::slice::from_ref(&input));
    assert_eq!(state.players[&entity].inventory.slots[0].count, 2);
    assert_eq!(
        spawned_item_count(&state, &accepted.changes.entities_spawned, emerald_id),
        1
    );
    let before_inventory = state.players[&entity].inventory.clone();
    let before_equipment = state.players[&entity].attribute_equipment.clone();
    let before_seed = state.players[&entity].enchantment_seed;
    let (refused, _, _) = crate::tick::run_tick(&mut state, std::slice::from_ref(&input));
    assert_eq!(state.players[&entity].inventory, before_inventory);
    assert_eq!(state.players[&entity].attribute_equipment, before_equipment);
    assert_eq!(state.players[&entity].enchantment_seed, before_seed);
    assert_eq!(
        spawned_item_count(&state, &refused.changes.entities_spawned, emerald_id),
        0
    );
    assert_eq!(
        inventories::reserved(&state, &player_endpoint())["minecraft:emerald"],
        2
    );
    release(&mut state, held);
    let (accepted, _, _) = crate::tick::run_tick(&mut state, &[input]);
    assert_eq!(state.players[&entity].inventory.slots[0].count, 1);
    assert_eq!(
        spawned_item_count(&state, &accepted.changes.entities_spawned, emerald_id),
        1
    );
}

#[test]
fn refused_sparse_route_images_actual_open_container_cargo_and_cursor() {
    use crate::boundary::PlayerChange;
    let (mut state, entity) = state();
    let held_stock = item(&state, "minecraft:emerald", 2);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = held_stock;
    reserve(&mut state, &player_endpoint(), &[("minecraft:emerald", 2)]);
    let cargo = item(&state, "minecraft:diamond", 5);
    let (pos, _) = chest(&mut state, cargo.clone());
    let registry = Arc::clone(&state.registry);
    let storage = state.block_entities[&(pos.x, pos.y, pos.z)].slots.clone();
    let player = state.players.get_mut(&entity).unwrap();
    player.open_menu(
        &registry.item_rules(),
        MenuKind::Chest { rows: 3 },
        "Cargo".into(),
        &storage,
    );
    let cursor = item(&state, "minecraft:stone", 1);
    state
        .players
        .get_mut(&entity)
        .unwrap()
        .open
        .as_mut()
        .unwrap()
        .carried = cursor.clone();
    state.open_containers.insert(entity, (pos.x, pos.y, pos.z));
    let effects = crate::tick::inventory_transition_refused(&state, entity);
    let PlayerChange::MenuContents {
        window,
        slots,
        cursor: actual_cursor,
        ..
    } = &effects.changes[0]
    else {
        panic!("native refusal must resync current menu")
    };
    assert_eq!(
        *window,
        state.players[&entity].open.as_ref().unwrap().window
    );
    assert_eq!(slots[0], Some(cargo));
    assert_eq!(*actual_cursor, Some(cursor));
    assert_eq!(state.block_entities[&(pos.x, pos.y, pos.z)].slots, storage);
    assert_eq!(state.players[&entity].inventory.slots[0].count, 2);
}

fn spawned_item_count(state: &TickState, ids: &[EntityId], item: u32) -> i32 {
    ids.iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| state.entities.row(slot).appearance.item.as_ref())
        .filter(|stack| stack.item_id == item && !stack.is_empty())
        .map(|stack| stack.count)
        .sum()
}

#[test]
fn native_tick_click_route_commits_excess_and_keeps_held_cursor_menu_and_rng_on_refusal() {
    let (mut state, entity) = state();
    let stock = item(&state, "minecraft:emerald", 2);
    let emerald_id = stock.item_id;
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = stock;
    let held = reserve(&mut state, &player_endpoint(), &[("minecraft:emerald", 1)]);
    let input = |revision| PlayerInput::ContainerClick {
        connection: ConnectionId(7),
        window: 0,
        state_id: revision,
        slot: 36,
        button: 0,
        mode: ClickMode::Throw,
    };
    let revision = state.players[&entity].inventory_menu.state_id;
    let (accepted, _, _) = crate::tick::run_tick(&mut state, &[input(revision)]);
    assert_eq!(state.players[&entity].inventory.slots[0].count, 1);
    assert_eq!(
        spawned_item_count(&state, &accepted.changes.entities_spawned, emerald_id),
        1
    );
    let before_inventory = state.players[&entity].inventory.clone();
    let before_menu = state.players[&entity].inventory_menu.clone();
    let before_seed = state.players[&entity].enchantment_seed;
    let (refused, _, _) = crate::tick::run_tick(&mut state, &[input(before_menu.state_id)]);
    assert_eq!(state.players[&entity].inventory, before_inventory);
    assert_eq!(state.players[&entity].inventory_menu, before_menu);
    assert_eq!(state.players[&entity].enchantment_seed, before_seed);
    assert_eq!(
        spawned_item_count(&state, &refused.changes.entities_spawned, emerald_id),
        0
    );
    release(&mut state, held);
    let (accepted, _, _) = crate::tick::run_tick(&mut state, &[input(before_menu.state_id)]);
    assert!(state.players[&entity].inventory.slots[0].is_empty());
    assert_eq!(
        spawned_item_count(&state, &accepted.changes.entities_spawned, emerald_id),
        1
    );
}
