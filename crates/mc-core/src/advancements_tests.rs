use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use mc_data::advancements::{AdvancementRewards, Criterion};
use mc_data::{Identifier, ItemStack};
use std::collections::BTreeMap;

fn fixture(mut tree: AdvancementTree) -> (TickState, EntityId) {
    tree.rebuild_trigger_index();
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    registry.advancements = Arc::new(tree);
    let registry = Arc::new(registry);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        registry.blocks.clone(),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 1, 8192);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::from_u128(1),
            name: "Advancer".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let wanted = state.desired_chunks();
    state.world.ensure_blocking(&wanted);
    state.outbox.clear();
    (state, entity)
}

fn definition(trigger_name: &str, conditions: Value) -> AdvancementDefinition {
    AdvancementDefinition {
        criteria: BTreeMap::from([(
            "test".into(),
            Criterion {
                trigger: trigger_name.into(),
                conditions,
            },
        )]),
        requirements: vec![vec!["test".into()]],
        ..Default::default()
    }
}

fn tree(
    definitions: impl IntoIterator<Item = (&'static str, AdvancementDefinition)>,
) -> AdvancementTree {
    AdvancementTree {
        definitions: definitions
            .into_iter()
            .map(|(id, definition)| (id.into(), definition))
            .collect(),
        ..Default::default()
    }
}

#[test]
fn advancement_requirements_rewards_deltas_and_recipe_unlocks_are_once_only() {
    let mut reward = definition(
        "minecraft:consume_item",
        json!({"item":{"items":"minecraft:apple"}}),
    );
    reward.criteria.insert(
        "other".into(),
        Criterion {
            trigger: "minecraft:consume_item".into(),
            conditions: json!({"item":{"items":"minecraft:carrot"}}),
        },
    );
    reward.criteria.insert(
        "dimension".into(),
        Criterion {
            trigger: "minecraft:changed_dimension".into(),
            conditions: json!({"to":"minecraft:the_nether"}),
        },
    );
    reward.requirements = vec![
        vec!["test".into(), "other".into()],
        vec!["dimension".into()],
    ];
    reward.rewards = AdvancementRewards {
        experience: 20,
        recipes: vec!["minecraft:crafting_table".into()],
        ..Default::default()
    };
    reward.display =
        Some(json!({"title":{"text":"Test reward"},"frame":"task","announce_to_chat":true}));
    let (mut state, entity) = fixture(tree([
        ("test:reward", reward),
        (
            "test:recipe",
            definition(
                "minecraft:recipe_unlocked",
                json!({"recipe":"minecraft:crafting_table"}),
            ),
        ),
    ]));
    state.wall_clock = || 1_767_225_600;
    let apple = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:apple").unwrap())
            .unwrap(),
        1,
    );
    let evidence = item_snapshot(&state, &apple);
    trigger(
        &mut state,
        entity,
        "minecraft:consume_item",
        json!({"item":evidence}),
    );
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:reward"]["done"],
        false
    );
    assert_eq!(state.players[&entity].vitals.experience.total, 0);
    trigger(
        &mut state,
        entity,
        "minecraft:changed_dimension",
        json!({"from":"minecraft:overworld","to":"minecraft:the_nether"}),
    );
    run(&mut state, &mut ChangeSet::default());
    let player = &state.players[&entity];
    assert_eq!(player.advancement_progress["test:reward"]["done"], true);
    assert_eq!(player.vitals.experience.total, 20);
    assert!(player.unlocked_recipes.contains("minecraft:crafting_table"));
    assert_eq!(player.advancement_progress["test:recipe"]["done"], true);
    let stamp = player.advancement_progress["test:reward"]["criteria"]["test"]
        .as_str()
        .unwrap();
    assert_eq!(
        stamp, "2026-01-01 00:00:00 +0000",
        "stamp follows the injected clock"
    );
    assert_eq!(timestamp_to_millis(stamp), Some(1_767_225_600_000));
    let queue = &state.outbox[&ConnectionId(1)];
    assert!(queue.iter().any(|event| matches!(event,PlayerChange::Advancements {reset:false,progress,..} if progress.get("test:reward").is_some())));
    assert_eq!(
        queue
            .iter()
            .filter(|event| matches!(event, PlayerChange::RecipesUnlocked { replace: false, .. }))
            .count(),
        1
    );
    assert!(queue.iter().any(|event| matches!(
        event,
        PlayerChange::StyledChat {
            content: mc_nbt::Tag::Compound(_),
            ..
        }
    )));
    let evidence = item_snapshot(&state, &apple);
    trigger(
        &mut state,
        entity,
        "minecraft:consume_item",
        json!({"item":evidence}),
    );
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(state.players[&entity].vitals.experience.total, 20);
    assert_eq!(
        revoke(
            &mut state,
            entity,
            &["test:reward".into()],
            Some("dimension")
        ),
        1
    );
    assert_eq!(
        state.players[&entity].advancement_progress["test:reward"]["done"],
        false
    );
}

#[test]
fn advancement_and_recipe_snapshots_copy_only_for_actual_changes() {
    let mut award = definition("minecraft:impossible", json!({}));
    award.rewards.recipes = vec!["minecraft:crafting_table".into()];
    let (mut state, entity) = fixture(tree([
        ("test:first", award.clone()),
        ("test:second", award.clone()),
        ("test:repair", award),
    ]));
    let empty_progress = state.players[&entity].advancement_progress.clone();
    let empty_book = state.players[&entity].unlocked_recipes.clone();
    assert_eq!(revoke(&mut state, entity, &["test:first".into()], None), 0);
    sync_join(&mut state, entity);
    assert!(Arc::ptr_eq(
        &empty_progress,
        &state.players[&entity].advancement_progress
    ));
    assert!(Arc::ptr_eq(
        &empty_book,
        &state.players[&entity].unlocked_recipes
    ));

    assert_eq!(grant(&mut state, entity, &["test:first".into()], None), 1);
    let progress = state.players[&entity].advancement_progress.clone();
    let book = state.players[&entity].unlocked_recipes.clone();
    assert!(!Arc::ptr_eq(&empty_progress, &progress));
    assert!(!Arc::ptr_eq(&empty_book, &book));
    assert_eq!(empty_progress.as_ref(), &json!({}));
    assert!(empty_book.is_empty());
    assert_eq!(grant(&mut state, entity, &["test:first".into()], None), 0);
    assert_eq!(revoke(&mut state, entity, &["test:second".into()], None), 0);
    sync_join(&mut state, entity);
    assert!(Arc::ptr_eq(
        &progress,
        &state.players[&entity].advancement_progress
    ));
    assert!(Arc::ptr_eq(&book, &state.players[&entity].unlocked_recipes));

    assert_eq!(grant(&mut state, entity, &["test:second".into()], None), 1);
    assert!(!Arc::ptr_eq(
        &progress,
        &state.players[&entity].advancement_progress
    ));
    assert!(progress.get("test:second").is_none());
    assert!(Arc::ptr_eq(&book, &state.players[&entity].unlocked_recipes));
    let awarded_progress = state.players[&entity].advancement_progress.clone();
    assert_eq!(revoke(&mut state, entity, &["test:second".into()], None), 1);
    assert!(!Arc::ptr_eq(
        &awarded_progress,
        &state.players[&entity].advancement_progress
    ));
    assert_eq!(awarded_progress["test:second"]["done"], true);
    let revoked_progress = state.players[&entity].advancement_progress.clone();
    assert_eq!(revoke(&mut state, entity, &["test:second".into()], None), 0);
    assert!(Arc::ptr_eq(
        &revoked_progress,
        &state.players[&entity].advancement_progress
    ));

    // Imported completed progress repairs a missing book once, without editing progress.
    state.players.get_mut(&entity).unwrap().advancement_progress = Arc::new(json!({
        "test:repair": {"criteria":{"test":"retained timestamp"}, "done":true},
        "unknown:pack": {"future":[1,2,3]}
    }));
    state.players.get_mut(&entity).unwrap().unlocked_recipes = empty_book.clone();
    let imported = state.players[&entity].advancement_progress.clone();
    sync_join(&mut state, entity);
    let repaired_book = state.players[&entity].unlocked_recipes.clone();
    assert!(Arc::ptr_eq(
        &imported,
        &state.players[&entity].advancement_progress
    ));
    assert!(!Arc::ptr_eq(&empty_book, &repaired_book));
    assert!(empty_book.is_empty());
    assert!(repaired_book.contains("minecraft:crafting_table"));
    sync_join(&mut state, entity);
    assert!(Arc::ptr_eq(
        &imported,
        &state.players[&entity].advancement_progress
    ));
    assert!(Arc::ptr_eq(
        &repaired_book,
        &state.players[&entity].unlocked_recipes
    ));
}

#[test]
fn advancement_announcement_gamerule_and_tab_validation_are_authoritative() {
    let mut root = definition("minecraft:slept_in_bed", json!({}));
    root.display = Some(json!({"title":{"text":"Root"}}));
    let mut child = root.clone();
    child.parent = Some("test:root".into());
    let (mut state, entity) = fixture(tree([("test:root", root), ("test:child", child)]));
    state
        .gamerules
        .insert("show_advancement_messages".into(), "false".into());
    grant(&mut state, entity, &["test:root".into()], None);
    assert!(
        !state.outbox[&ConnectionId(1)]
            .iter()
            .any(|event| matches!(event, PlayerChange::StyledChat { .. }))
    );
    seen(&mut state, entity, Some("test:child"));
    seen(&mut state, entity, Some("test:unknown"));
    seen(&mut state, entity, None);
    seen(&mut state, entity, Some("test:root"));
    assert_eq!(
        state.outbox[&ConnectionId(1)]
            .iter()
            .filter(|event| matches!(event, PlayerChange::AdvancementsTab { .. }))
            .count(),
        1
    );
}

#[test]
fn advancement_reward_loot_uses_real_inventory_custody_and_unknown_progress_survives() {
    let mut advancement = definition("minecraft:consume_item", json!({}));
    advancement.rewards.loot = vec!["test:reward".into()];
    let (mut state, entity) = fixture(tree([("test:loot", advancement)]));
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    registry.advancements = state.registry.advancements.clone();
    let table=mc_data::loot::chest_26_1_2::ChestLootTable::compile_advancement_reward(Identifier::parse("test:reward").unwrap(),r#"{"type":"minecraft:advancement_reward","pools":[{"rolls":1,"entries":[{"type":"minecraft:item","name":"minecraft:diamond"}]}]}"#).unwrap();
    registry
        .advancement_reward_loot
        .insert("test:reward".into(), table);
    state.registry = Arc::new(registry);
    Arc::make_mut(&mut state.players.get_mut(&entity).unwrap().advancement_progress)["unknown:pack"] =
        json!({"future":{"value":9}});
    assert_eq!(grant(&mut state, entity, &["test:loot".into()], None), 1);
    let diamond = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:diamond").unwrap())
        .unwrap();
    assert_eq!(
        state.players[&entity]
            .inventory
            .slots
            .iter()
            .filter(|item| item.item_id == diamond)
            .map(|item| item.count)
            .sum::<i32>(),
        1
    );
    assert_eq!(
        state.players[&entity].advancement_progress["unknown:pack"],
        json!({"future":{"value":9}})
    );
    assert_eq!(grant(&mut state, entity, &["test:loot".into()], None), 0);
}

#[test]
#[ignore = "requires local vanilla 26.1.2 advancement sidecars"]
fn vanilla_advancement_stone_age_crafting_table_mob_kill_nether_and_sleep() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let vanilla = AdvancementTree::load(&root).unwrap();
    assert_eq!(vanilla.definitions.len(), 1617);
    let (mut state, entity) = fixture(vanilla);
    let cobble = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:cobblestone").unwrap())
        .unwrap();
    let table = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:crafting_table").unwrap())
        .unwrap();
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = ItemStack::new(cobble, 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[1] = ItemStack::new(table, 1);
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["minecraft:story/mine_stone"]["done"],
        true
    );
    assert!(
        state.players[&entity]
            .unlocked_recipes
            .contains("minecraft:furnace")
    );
    let zombie = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let mob = state.spawn_mob(zombie, [0., 80., 1.], 20.);
    let mob_slot = state.entities.slot_of(mob).unwrap();
    state.merge_damage.push((
        mob_slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 40.,
            kind: crate::survival::DamageKind::Melee,
            source: Some(crate::intent::DamageSource::Melee {
                attacker: entity,
                position: [0., 80., 0.],
                yaw: 0.,
                bonus_knockback: 0.,
                sweep: false,
                fire_ticks: 0,
                bane_ticks: 0,
            }),
        },
    ));
    state.commit_merge_effects();
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["minecraft:adventure/kill_a_mob"]["done"],
        true
    );
    trigger(
        &mut state,
        entity,
        "minecraft:changed_dimension",
        json!({"from":"minecraft:overworld","to":"minecraft:the_nether"}),
    );
    trigger(&mut state, entity, "minecraft:slept_in_bed", json!({}));
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["minecraft:story/enter_the_nether"]["done"],
        true
    );
    assert_eq!(
        state.players[&entity].advancement_progress["minecraft:adventure/sleep_in_bed"]["done"],
        true
    );
}

#[test]
fn recipe_book_requires_advancement_unlock_and_crafting_awards_only_after_output_take() {
    let mut unlock = definition(
        "minecraft:inventory_changed",
        json!({"items":[{"items":"minecraft:oak_log"}]}),
    );
    unlock.rewards.recipes = vec!["minecraft:oak_planks".into()];
    let crafted = definition(
        "minecraft:recipe_crafted",
        json!({"recipe_id":"minecraft:oak_planks","ingredients":[{"items":"minecraft:oak_log"}]}),
    );
    let (mut state, entity) = fixture(tree([("test:unlock", unlock), ("test:crafted", crafted)]));
    let log = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:oak_log").unwrap())
        .unwrap();
    let planks = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:oak_planks").unwrap())
        .unwrap();
    let recipe = state
        .registry
        .recipes
        .iter()
        .position(|recipe| recipe.id.as_str() == "minecraft:oak_planks")
        .unwrap() as u32;
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = ItemStack::new(log, 1);
    let place = PlayerInput::PlaceRecipe {
        connection: ConnectionId(1),
        window: 0,
        recipe,
        make_all: false,
    };
    crate::tick::run_tick(&mut state, std::slice::from_ref(&place));
    assert!(
        state.players[&entity].inventory_menu.result.is_empty(),
        "locked request cannot place the recipe"
    );
    assert!(
        state.players[&entity]
            .unlocked_recipes
            .contains("minecraft:oak_planks")
    );
    crate::tick::run_tick(&mut state, &[place]);
    assert_eq!(
        state.players[&entity].inventory_menu.result,
        ItemStack::new(planks, 4)
    );
    assert!(
        state.players[&entity]
            .advancement_progress
            .get("test:crafted")
            .is_none(),
        "preview is not a craft"
    );
    let state_id = state.players[&entity].inventory_menu.state_id;
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window: 0,
            state_id,
            slot: 0,
            button: 0,
            mode: crate::boundary::ClickMode::QuickMove,
        }],
    );
    assert_eq!(
        state.players[&entity].advancement_progress["test:crafted"]["done"],
        true
    );
    assert!(
        state.players[&entity]
            .inventory_menu
            .grid
            .iter()
            .all(ItemStack::is_empty)
    );
    assert_eq!(
        state.players[&entity]
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == planks)
            .map(|stack| stack.count)
            .sum::<i32>(),
        4
    );
}

#[test]
fn nonpiercing_crossbow_kill_retains_actual_launch_weapon_after_projectile_removal() {
    let killed = definition(
        "minecraft:killed_by_arrow",
        json!({"fired_from_weapon":{"items":"minecraft:crossbow"},"victims":[{"type":"minecraft:cow"}]}),
    );
    let aimed = definition(
        "minecraft:player_hurt_entity",
        json!({"damage":{"type":{"direct_entity":{"type":"#minecraft:arrows"}}}}),
    );
    let (mut state, entity) = fixture(tree([("test:crossbow", killed), ("test:aim", aimed)]));
    let arrow_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:arrow").unwrap())
        .unwrap();
    Arc::make_mut(
        &mut Arc::get_mut(&mut state.registry)
            .expect("fixture owns registry")
            .tags,
    )
    .registries
    .entry(Identifier::parse("minecraft:entity_type").unwrap())
    .or_default()
    .insert(
        Identifier::parse("minecraft:arrows").unwrap(),
        vec![arrow_type as i32],
    );
    let player_slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(player_slot, [0.5, 201., 0.5]);
    for x in 0..=5 {
        for y in 201..=204 {
            state.world.set_block(x, y, 0, state.registry.states.air);
        }
    }
    let cow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let victim = state.spawn_mob(cow, [2.5, 201., 0.5], 1.);
    let victim_slot = state.entities.slot_of(victim).unwrap();
    state.entities.row_mut(victim_slot).speed = 0.;
    let arrow = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:arrow").unwrap())
            .unwrap(),
        1,
    );
    let crossbow = ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse("minecraft:crossbow").unwrap())
            .unwrap(),
        1,
    );
    let owner = state.entities.row(player_slot).uuid;
    let mut record = crate::projectiles::ProjectileState::new(
        crate::projectiles::ProjectileKind::Arrow,
        Some(owner),
        arrow,
    );
    record.fired_from_weapon = Some(Arc::new(crossbow));
    record.damage = 20.;
    let projectile =
        crate::projectiles::launch(&mut state, record, [0.5, 201.7, 0.5], [3., 0., 0.]).unwrap();
    crate::tick::run_tick(&mut state, &[]);
    assert!(
        state.entities.slot_of_any(projectile).is_none(),
        "nonpiercing hit removed the projectile before damage admission"
    );
    assert!(
        state
            .entities
            .slot_of_any(victim)
            .is_none_or(|slot| !state.entities.body(slot).alive)
    );
    assert_eq!(
        state.players[&entity].advancement_progress["test:crossbow"]["done"],
        true
    );
    assert_eq!(
        state.players[&entity].advancement_progress["test:aim"]["done"], true,
        "lethal hits also award hurt criteria and retain removed arrow tags"
    );
    assert!(
        state
            .projectiles
            .retired_crossbows
            .contains_key(&projectile)
    );
}

#[test]
fn accepted_projectile_shield_block_awards_deflection_without_health_damage() {
    let deflect = definition(
        "minecraft:entity_hurt_player",
        json!({"damage":{"dealt":3.5,"taken":0.0,"blocked":true,"type":{"tags":[{"id":"minecraft:is_projectile","expected":true}]}}}),
    );
    let (mut state, entity) = fixture(tree([("test:deflect", deflect)]));
    // The minimal no-sidecar registry advertises empty client fallback tags.
    // Supply the authoritative vanilla arrow membership for this gameplay fixture.
    let raw = mc_data::required_registry_entry_id(
        "damage_type",
        &Identifier::parse("minecraft:arrow").unwrap(),
    )
    .unwrap();
    Arc::make_mut(
        &mut Arc::get_mut(&mut state.registry)
            .expect("fixture owns the registry")
            .tags,
    )
    .registries
    .entry(Identifier::parse("minecraft:damage_type").unwrap())
    .or_default()
    .insert(
        Identifier::parse("minecraft:is_projectile").unwrap(),
        vec![raw as i32],
    );
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    state.entities.set_rot(slot, 0., 0.);
    let shield = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:shield").unwrap())
        .unwrap();
    let player = state.players.get_mut(&entity).unwrap();
    player.inventory.slots[0] = ItemStack::new(shield, 1);
    player.use_item(&state.registry.item_rules(), crate::boundary::Hand::Main);
    for _ in 0..5 {
        player.tick_item_use(&state.registry.item_rules());
    }
    let arrow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:arrow").unwrap())
        .unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 3.5,
            kind: crate::survival::DamageKind::Projectile,
            source: Some(crate::intent::DamageSource::Projectile {
                owner: EntityId(-1),
                projectile: EntityId(-2),
                direct_type_id: arrow,
                velocity: [0., -1.],
                punch: 0,
                flame: false,
            }),
        },
    ));
    state.commit_merge_effects();
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(state.players[&entity].vitals.health, 20.);
    assert_eq!(
        state.players[&entity].advancement_progress["test:deflect"]["done"],
        true
    );
}

#[test]
fn bare_armour_snapshot_satisfies_piglin_inverted_safe_armour_predicates() {
    let player_conditions: Vec<_> = ["head", "chest", "legs", "feet"]
        .into_iter()
        .map(|slot| {
            json!({
                "condition":"minecraft:inverted",
                "term":{
                    "condition":"minecraft:entity_properties",
                    "entity":"this",
                    "predicate":{"equipment":{slot:{"items":"#minecraft:piglin_safe_armor"}}}
                }
            })
        })
        .collect();
    let criterion = definition(
        "minecraft:player_interacted_with_entity",
        json!({
            "player":player_conditions,
            "entity":[{"condition":"minecraft:entity_properties","entity":"this",
                "predicate":{"type":"minecraft:piglin","flags":{"is_baby":false}}}],
            "item":{"items":"minecraft:gold_ingot"}
        }),
    );
    let (mut state, entity) = fixture(tree([("test:bare_piglin", criterion)]));
    let snapshot = entity_snapshot(&state, entity);
    for slot in ["head", "chest", "legs", "feet"] {
        assert_eq!(
            snapshot["equipment"][slot],
            json!({"id":"minecraft:air","count":0,"tags":[],"components":{}})
        );
    }
    let gold = crate::bees::tests::item(&state, "minecraft:gold_ingot", 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = gold;
    let piglin_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:piglin").unwrap())
        .unwrap();
    let piglin = state.spawn_mob(piglin_type, [0.5, 80., 1.5], 16.);
    let slot = state.entities.slot_of(piglin).unwrap();
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[1] = None;
    assert!(crate::piglin::interact(
        &mut state,
        entity,
        piglin,
        crate::boundary::Hand::Main,
    ));
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:bare_piglin"]["done"],
        true
    );
}

fn powder_snow_completion(criterion: AdvancementDefinition) {
    let (mut state, entity) = fixture(tree([("test:powder_snow", criterion)]));
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [-0.25, 201., -0.25]);
    let on_pos = crate::boundary::BlockPos {
        x: -1,
        y: 200,
        z: -1,
    };
    crate::bees::tests::place(&mut state, on_pos, "minecraft:powder_snow");
    crate::bees::tests::place(
        &mut state,
        crate::boundary::BlockPos { y: 201, ..on_pos },
        "minecraft:air",
    );
    let snapshot = entity_snapshot(&state, entity);
    assert_eq!(snapshot["location"]["block"]["id"], "minecraft:air");
    assert_eq!(
        snapshot["stepping_on"]["block"]["id"],
        "minecraft:powder_snow"
    );
    assert_eq!(
        snapshot["stepping_on"]["position"],
        json!({"x":-1,"y":200,"z":-1})
    );
    assert_eq!(snapshot["stepping_on"]["dimension"], state.dimension.name());
    let biome = state.world.biome_at(-1, 200, -1).unwrap();
    assert_eq!(snapshot["stepping_on"]["biomes"], json!([biome.as_str()]));
    state.tick = 20;
    run(&mut state, &mut ChangeSet::default());
    assert!(
        state.players[&entity]
            .advancement_progress
            .get("test:powder_snow")
            .is_none()
    );
    let boots = crate::bees::tests::item(&state, "minecraft:leather_boots", 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[36] = boots;
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:powder_snow"]["done"],
        true
    );
}

#[test]
fn leather_boots_powder_snow_uses_actual_get_on_pos_not_feet_block() {
    powder_snow_completion(definition(
        "minecraft:location",
        json!({"player":[{"condition":"minecraft:entity_properties","entity":"this",
        "predicate":{
            "equipment":{"feet":{"items":"minecraft:leather_boots"}},
            "stepping_on":{"block":{"blocks":"minecraft:powder_snow"}}
        }}]}),
    ));
}

#[test]
#[ignore = "requires local vanilla 26.1.2 advancement sidecars"]
fn vanilla_leather_boots_powder_snow_criterion_completes() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let vanilla = AdvancementTree::load(&root).unwrap();
    powder_snow_completion(
        vanilla.definitions["minecraft:adventure/walk_on_powder_snow_with_leather_boots"].clone(),
    );
}

fn overoverkill_definition() -> AdvancementDefinition {
    definition(
        "minecraft:player_hurt_entity",
        json!({"damage":{
            "dealt":{"min":100.0},
            "type":{
                "direct_entity":{"type":"minecraft:player","equipment":{"mainhand":{"items":"minecraft:mace"}}},
                "tags":[{"id":"minecraft:mace_smash","expected":true}]
            }
        }}),
    )
}

fn mace_fixture(criterion: AdvancementDefinition) -> (TickState, EntityId) {
    let (mut state, entity) = fixture(tree([("test:overoverkill", criterion)]));
    let raw = mc_data::required_registry_entry_id(
        "damage_type",
        &Identifier::parse("minecraft:mace_smash").unwrap(),
    )
    .unwrap();
    Arc::make_mut(&mut Arc::get_mut(&mut state.registry).unwrap().tags)
        .registries
        .entry(Identifier::parse("minecraft:damage_type").unwrap())
        .or_default()
        .insert(
            Identifier::parse("minecraft:mace_smash").unwrap(),
            vec![raw as i32],
        );
    let mace = crate::bees::tests::item(&state, "minecraft:mace", 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = mace;
    (state, entity)
}

fn accepted_overoverkill(criterion: AdvancementDefinition) {
    let (mut state, entity) = mace_fixture(criterion);
    state.tick = 100;
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [0.5, 201., 0.5]);
    state.entities.set_ground(slot, false);
    state.players.get_mut(&entity).unwrap().fall_distance = 100.;
    for x in 0..=1 {
        for z in 0..=2 {
            for y in 200..=204 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    let cow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let target = state.spawn_mob(cow, [0.5, 201., 1.5], 20.);
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Attack {
            connection: ConnectionId(1),
            target,
        }],
    );
    assert!(
        state
            .entities
            .slot_of_any(target)
            .is_none_or(|slot| !state.entities.body(slot).alive)
    );
    assert_eq!(state.players[&entity].fall_distance, 0.);
    assert_eq!(
        state.players[&entity].advancement_progress["test:overoverkill"]["done"], true,
        "accepted smash exceeds 100 raw damage even though target only had 20 health"
    );
}

#[test]
fn accepted_mace_smash_over_100_awards_overoverkill_on_health_capped_target() {
    accepted_overoverkill(overoverkill_definition());
}

#[test]
#[ignore = "requires local vanilla 26.1.2 advancement sidecars"]
fn vanilla_overoverkill_completes_on_accepted_health_capped_smash() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let vanilla = AdvancementTree::load(&root).unwrap();
    accepted_overoverkill(vanilla.definitions["minecraft:adventure/overoverkill"].clone());
}

#[test]
fn mace_dealt_damage_keeps_its_own_raw_amount_before_hurt_window_delta() {
    let (mut state, entity) = mace_fixture(overoverkill_definition());
    let mace = &state.players[&entity].inventory.slots[0];
    let attack =
        crate::combat::mace_attack(&state.registry.item_rules(), mace, 10., false).unwrap();
    let cow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let target = state.spawn_mob(cow, [0.5, 80., 1.5], 200.);
    let slot = state.entities.slot_of(target).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 50.,
            kind: crate::survival::DamageKind::Generic,
            source: None,
        },
    ));
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 60.,
            kind: crate::survival::DamageKind::Melee,
            source: Some(crate::intent::DamageSource::Mace {
                attacker: entity,
                position: [0.5, 80., 0.5],
                yaw: 0.,
                bonus_knockback: 0.,
                fire_ticks: 0,
                bane_ticks: 0,
                attack,
            }),
        },
    ));
    state.commit_merge_effects();
    assert_eq!(state.entities.row(slot).health, 140.);
    let hurt = state
        .advancement_events
        .iter()
        .find(|event| event.trigger == "minecraft:player_hurt_entity")
        .unwrap();
    assert_eq!(hurt.evidence["damage"]["dealt"], 60.);
    assert_eq!(hurt.evidence["damage"]["taken"], 10.);
    run(&mut state, &mut ChangeSet::default());
    assert!(
        state.players[&entity]
            .advancement_progress
            .get("test:overoverkill")
            .is_none()
    );
}

#[test]
fn ordinary_or_gliding_mace_attacks_never_receive_smash_damage_tag() {
    let (mut state, entity) = mace_fixture(overoverkill_definition());
    for (height, gliding) in [(1.5, false), (100., true)] {
        let attack = crate::combat::mace_attack(
            &state.registry.item_rules(),
            &state.players[&entity].inventory.slots[0],
            height,
            gliding,
        )
        .unwrap();
        assert_eq!(attack.fall_distance, 0.);
        let cow = state
            .registry
            .entity_types
            .id_of(&Identifier::parse("minecraft:cow").unwrap())
            .unwrap();
        let target = state.spawn_mob(cow, [0.5, 80., 1.5], 20.);
        let slot = state.entities.slot_of(target).unwrap();
        state.merge_damage.push((
            slot,
            crate::tick::DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 120.,
                kind: crate::survival::DamageKind::Melee,
                source: Some(crate::intent::DamageSource::Mace {
                    attacker: entity,
                    position: [0.5, 80., 0.5],
                    yaw: 0.,
                    bonus_knockback: 0.,
                    fire_ticks: 0,
                    bane_ticks: 0,
                    attack,
                }),
            },
        ));
        state.commit_merge_effects();
        run(&mut state, &mut ChangeSet::default());
        assert!(
            state.players[&entity]
                .advancement_progress
                .get("test:overoverkill")
                .is_none()
        );
    }
}

#[test]
fn accepted_armoured_damage_distinguishes_raw_dealt_from_health_taken() {
    let (mut state, entity) = fixture(tree([(
        "test:armoured",
        definition(
            "minecraft:entity_hurt_player",
            json!({"damage":{"dealt":10.0,"taken":{"min":0.01,"max":9.99},"blocked":false}}),
        ),
    )]));
    for (index, name) in [
        (36, "minecraft:diamond_boots"),
        (37, "minecraft:diamond_leggings"),
        (38, "minecraft:diamond_chestplate"),
        (39, "minecraft:diamond_helmet"),
    ] {
        let stack = crate::bees::tests::item(&state, name, 1);
        state.players.get_mut(&entity).unwrap().inventory.slots[index] = stack;
    }
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 10.,
            kind: crate::survival::DamageKind::Melee,
            source: None,
        },
    ));
    state.commit_merge_effects();
    let taken = 20. - state.players[&entity].vitals.health;
    assert!(taken > 0. && taken < 10.);
    let hurt = state
        .advancement_events
        .iter()
        .find(|event| event.trigger == "minecraft:entity_hurt_player")
        .unwrap();
    assert_eq!(hurt.evidence["damage"]["dealt"], 10.);
    assert_eq!(hurt.evidence["damage"]["taken"], json!(taken));
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:armoured"]["done"],
        true
    );
}

#[test]
fn enter_block_prefilter_preserves_changing_player_predicates_regrant_and_unknown_refusal() {
    let conditions = json!({
        "block":"minecraft:honey_block",
        "player":{"equipment":{"feet":{"items":"minecraft:leather_boots"}}}
    });
    let mut unsupported = conditions.clone();
    unsupported["unknown_custom_field"] = json!(true);
    let (mut state, entity) = fixture(tree([
        (
            "test:enter",
            definition("minecraft:enter_block", conditions),
        ),
        (
            "test:unknown",
            definition("minecraft:enter_block", unsupported),
        ),
    ]));
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [0.5, 201., 0.5]);
    let pos = crate::boundary::BlockPos { x: 0, y: 201, z: 0 };
    crate::bees::tests::place(&mut state, pos, "minecraft:stone");
    state.advancement_events.clear();
    enter_block(&mut state, entity);
    assert!(
        state.advancement_events.is_empty(),
        "unrelated block needs no snapshots"
    );
    crate::bees::tests::place(&mut state, pos, "minecraft:honey_block");
    run(&mut state, &mut ChangeSet::default());
    assert!(
        state.players[&entity]
            .advancement_progress
            .get("test:enter")
            .is_none()
    );
    let boots = crate::bees::tests::item(&state, "minecraft:leather_boots", 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[36] = boots;
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:enter"]["done"],
        true
    );
    assert!(
        state.players[&entity]
            .advancement_progress
            .get("test:unknown")
            .is_none()
    );
    assert_eq!(revoke(&mut state, entity, &["test:enter".into()], None), 1);
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:enter"]["done"],
        true
    );
    state.world.set_block(pos.x, pos.y, pos.z, u32::MAX);
    enter_block(&mut state, entity);
    assert!(
        state.advancement_events.is_empty(),
        "unknown block has no authoritative id"
    );
    assert!(
        state.players[&entity]
            .advancement_progress
            .get("test:unknown")
            .is_none()
    );
}

#[test]
fn item_snapshots_expose_only_native_playable_jukebox_song_components() {
    let (state, _) = fixture(tree([]));
    for (disc, song) in [
        ("minecraft:music_disc_cat", "minecraft:cat"),
        ("minecraft:music_disc_pigstep", "minecraft:pigstep"),
        (
            "minecraft:music_disc_lava_chicken",
            "minecraft:lava_chicken",
        ),
    ] {
        let snapshot = item_snapshot(&state, &crate::bees::tests::item(&state, disc, 1));
        assert_eq!(
            snapshot["components"]["minecraft:jukebox_playable"],
            json!({"song":song})
        );
    }
    for stack in [
        ItemStack::EMPTY,
        crate::bees::tests::item(&state, "minecraft:stick", 1),
    ] {
        assert!(
            item_snapshot(&state, &stack)["components"]
                .get("minecraft:jukebox_playable")
                .is_none()
        );
    }
}

#[test]
fn periodic_location_rechecks_position_revoke_and_mutable_equipment_in_place() {
    let bounded = json!({"location":{"position":{"x":{"min":0,"max":1}}}});
    let mut equipped = bounded.clone();
    equipped["equipment"] = json!({"feet":{"items":"minecraft:leather_boots"}});
    let (mut state, entity) = fixture(tree([
        (
            "test:position",
            definition("minecraft:location", json!({"player":bounded})),
        ),
        (
            "test:equipment",
            definition("minecraft:location", json!({"player":equipped})),
        ),
    ]));
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [0.5, 201.0, 0.5]);
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:position"]["done"],
        true
    );
    assert!(
        state.players[&entity]
            .advancement_progress
            .get("test:equipment")
            .is_none()
    );
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    let boots = crate::bees::tests::item(&state, "minecraft:leather_boots", 1);
    state.players.get_mut(&entity).unwrap().inventory.slots[36] = boots;
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:equipment"]["done"],
        true
    );
    assert_eq!(
        revoke(&mut state, entity, &["test:position".into()], None),
        1
    );
    state.entities.set_pos(slot, [2.5, 201.0, 0.5]);
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:position"]["done"],
        false
    );
    state.entities.set_pos(slot, [0.5, 201.0, 0.5]);
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:position"]["done"],
        true
    );
}

#[test]
fn periodic_location_rechecks_underfoot_block_dimension_and_catalog() {
    let conditions = json!({"player":{"stepping_on":{"block":{"blocks":"minecraft:honey_block"}}}});
    let (mut state, entity) = fixture(tree([
        (
            "test:standing",
            definition("minecraft:location", conditions),
        ),
        (
            "test:dimension",
            definition(
                "minecraft:location",
                json!({"player":{"location":{"dimension":"minecraft:the_nether"}}}),
            ),
        ),
    ]));
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [0.5, 201.0, 0.5]);
    let below = crate::boundary::BlockPos { x: 0, y: 200, z: 0 };
    crate::bees::tests::place(&mut state, below, "minecraft:stone");
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert!(
        state.players[&entity]
            .advancement_progress
            .get("test:standing")
            .is_none()
    );
    crate::bees::tests::place(&mut state, below, "minecraft:honey_block");
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:standing"]["done"],
        true
    );
    state.dimension = crate::dimensions::Dimension::Nether;
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:dimension"]["done"],
        true
    );
    let mut replacement = tree([(
        "test:replacement",
        definition("minecraft:location", json!({})),
    )]);
    replacement.rebuild_trigger_index();
    Arc::make_mut(&mut state.registry).advancements = Arc::new(replacement);
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:replacement"]["done"],
        true
    );
}

#[test]
fn periodic_location_reads_changed_current_block_properties_and_tags_without_movement() {
    let (mut state, entity) = fixture(tree([
        (
            "test:property",
            definition(
                "minecraft:location",
                json!({"player":{"location":{"block":{"blocks":"minecraft:composter","state":{"level":"1"}}}}}),
            ),
        ),
        (
            "test:tag",
            definition(
                "minecraft:location",
                json!({"player":{"location":{"block":{"blocks":"#minecraft:wool"}}}}),
            ),
        ),
    ]));
    let mut full = crate::tick_tests::full_registry();
    full.advancements = state.registry.advancements.clone();
    state.registry = Arc::new(full);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [0.5, 201.0, 0.5]);
    let pos = crate::boundary::BlockPos { x: 0, y: 201, z: 0 };
    crate::bees::tests::place(&mut state, pos, "minecraft:composter");
    let original = location_snapshot(&state, entity);
    assert_eq!(original["block"]["state"]["level"], "0");
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert!(
        state.players[&entity]
            .advancement_progress
            .get("test:property")
            .is_none()
    );
    let filled = state
        .registry
        .blocks
        .by_name_and_props(
            &Identifier::parse("minecraft:composter").unwrap(),
            &[("level".into(), "1".into())],
        )
        .unwrap()
        .0;
    state.world.set_block(pos.x, pos.y, pos.z, filled);
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:property"]["done"],
        true
    );
    crate::bees::tests::place(&mut state, pos, "minecraft:white_wool");
    location::periodic(&mut state, entity, &mut Vec::new());
    run(&mut state, &mut ChangeSet::default());
    let changed = location_snapshot(&state, entity);
    assert_eq!(original["position"], changed["position"]);
    assert_eq!(original.get("biomes"), changed.get("biomes"));
    assert_eq!(
        state.players[&entity].advancement_progress["test:tag"]["done"],
        true
    );
}

#[test]
fn periodic_location_keeps_evaluating_players_on_the_death_screen() {
    let (mut state, entity) = fixture(tree([(
        "test:dead_location",
        definition("minecraft:location", json!({})),
    )]));
    let slot = state.entities.slot_of_any(entity).unwrap();
    state.entities.body_mut(slot).alive = false;
    state.tick = 20;
    run(&mut state, &mut ChangeSet::default());
    assert_eq!(
        state.players[&entity].advancement_progress["test:dead_location"]["done"],
        true
    );
}
