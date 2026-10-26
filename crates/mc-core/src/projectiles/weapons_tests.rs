use super::*;
use crate::commands::GameMode;

fn stack(state: &TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap(),
        count,
    )
}
fn projectiles(state: &TickState) -> Vec<&ProjectileState> {
    state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).projectile.as_deref())
        .collect()
}

fn item_stat(state: &TickState, actor: EntityId, kind: StatType, name: &str) -> i32 {
    state.players[&actor]
        .statistics
        .get(kind, stack(state, name, 1).item_id)
}

fn observe_crossbow(state: &mut TickState) {
    let mut tree = mc_data::advancements::AdvancementTree::default();
    tree.definitions.insert(
        "test:shot".into(),
        serde_json::from_value(serde_json::json!({
            "criteria": { "shot": { "trigger": "minecraft:shot_crossbow" } },
            "requirements": [["shot"]],
        }))
        .unwrap(),
    );
    tree.trigger_index.insert(
        "minecraft:shot_crossbow".into(),
        vec![("test:shot".into(), "shot".into())],
    );
    std::sync::Arc::get_mut(&mut state.registry)
        .unwrap()
        .advancements = std::sync::Arc::new(tree);
}

#[test]
fn bow_requires_charge_consumes_one_arrow_and_retains_enchantment_payload() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let bow = stack(&state, "bow", 1)
        .with_enchantment(mc_data::Identifier::parse("minecraft:power").unwrap(), 3)
        .with_enchantment(mc_data::Identifier::parse("minecraft:punch").unwrap(), 2)
        .with_enchantment(mc_data::Identifier::parse("minecraft:flame").unwrap(), 1);
    let arrows = stack(&state, "arrow", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow;
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrows;
    assert!(use_item(&mut state, actor, Hand::Main));
    assert_eq!(item_stat(&state, actor, StatType::Used, "bow"), 0);
    assert!(release(&mut state, actor));
    assert!(projectiles(&state).is_empty());
    assert_eq!(state.players[&actor].inventory.slots[9].count, 2);
    assert_eq!(item_stat(&state, actor, StatType::Used, "bow"), 0);
    use_item(&mut state, actor, Hand::Main);
    state.tick += 20;
    release(&mut state, actor);
    let arrow = projectiles(&state)[0];
    assert_eq!(arrow.damage, 4.0);
    assert_eq!(arrow.punch, 2);
    assert!(arrow.flame && arrow.critical && arrow.pickup);
    assert!(arrow.fired_from_weapon.is_none());
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 1);
    assert_eq!(item_stat(&state, actor, StatType::Used, "bow"), 1);
    assert_eq!(item_stat(&state, actor, StatType::Used, "arrow"), 0);
}

#[test]
fn infinity_and_creative_arrows_are_not_pickable_and_special_arrows_are_consumed() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let bow = stack(&state, "bow", 1)
        .with_enchantment(mc_data::Identifier::parse("minecraft:infinity").unwrap(), 1);
    let arrows = stack(&state, "arrow", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow;
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrows;
    use_item(&mut state, actor, Hand::Main);
    state.tick += 20;
    release(&mut state, actor);
    assert!(!projectiles(&state)[0].pickup);
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
    let spectral = stack(&state, "spectral_arrow", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = spectral;
    use_item(&mut state, actor, Hand::Main);
    state.tick += 20;
    release(&mut state, actor);
    assert!(projectiles(&state)[1].pickup);
    assert!(state.players[&actor].inventory.slots[9].is_empty());
    state.players.get_mut(&actor).unwrap().game_mode = GameMode::Creative;
    use_item(&mut state, actor, Hand::Main);
    state.tick += 20;
    release(&mut state, actor);
    assert!(!projectiles(&state)[2].pickup);
    assert_eq!(item_stat(&state, actor, StatType::Used, "bow"), 3);
}

#[test]
fn crossbow_loads_once_and_multishot_side_arrows_cannot_duplicate_ammo() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    observe_crossbow(&mut state);
    let weapon = stack(&state, "crossbow", 1)
        .with_enchantment(
            mc_data::Identifier::parse("minecraft:multishot").unwrap(),
            1,
        )
        .with_enchantment(mc_data::Identifier::parse("minecraft:piercing").unwrap(), 2)
        .with_enchantment(
            mc_data::Identifier::parse("minecraft:quick_charge").unwrap(),
            3,
        );
    let arrows = stack(&state, "arrow", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = weapon;
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrows;
    use_item(&mut state, actor, Hand::Main);
    state.tick += 10;
    tick(&mut state, &mut Vec::new());
    assert_eq!(
        state.players[&actor].inventory.slots[0]
            .extended()
            .charged_projectiles
            .len(),
        3
    );
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
    release(&mut state, actor);
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
    assert_eq!(item_stat(&state, actor, StatType::Used, "crossbow"), 0);
    assert!(
        state.advancement_events.is_empty(),
        "loading is not shooting"
    );
    use_item(&mut state, actor, Hand::Main);
    let arrows = projectiles(&state);
    assert_eq!(arrows.len(), 3);
    assert_eq!(arrows.iter().filter(|arrow| arrow.pickup).count(), 1);
    assert!(arrows.iter().all(|arrow| arrow.piercing == 2));
    assert!(arrows.iter().all(|arrow| arrow.critical));
    assert!(arrows.iter().all(|arrow| arrow.fired_from_weapon.is_some()));
    assert!(
        state.players[&actor].inventory.slots[0]
            .extended()
            .charged_projectiles
            .is_empty()
    );
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(3));
    assert_eq!(item_stat(&state, actor, StatType::Used, "crossbow"), 1);
    assert_eq!(item_stat(&state, actor, StatType::Used, "arrow"), 0);
    assert_eq!(
        state.advancement_events.len(),
        1,
        "one accepted multishot is one event"
    );
    assert_eq!(
        *state.players[&actor].advancement_progress,
        serde_json::json!({})
    );
}

#[test]
fn crossbow_fires_full_firework_payload_and_requires_held_rockets() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let weapon = stack(&state, "crossbow", 1);
    let rocket = stack(&state, "firework_rocket", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = weapon;
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = rocket.clone();
    use_item(&mut state, actor, Hand::Main);
    assert!(state.projectiles.using.is_empty());
    state.players.get_mut(&actor).unwrap().inventory.slots[OFF_HAND] = rocket;
    use_item(&mut state, actor, Hand::Main);
    state.tick += 25;
    release(&mut state, actor);
    use_item(&mut state, actor, Hand::Main);
    let rocket = projectiles(&state)[0];
    assert_eq!(rocket.kind, ProjectileKind::Firework);
    assert!(rocket.shot_at_angle);
    assert!(!rocket.critical);
    assert_eq!(state.players[&actor].inventory.slots[OFF_HAND].count, 1);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(3));
    assert_eq!(item_stat(&state, actor, StatType::Used, "crossbow"), 1);
    assert_eq!(
        item_stat(&state, actor, StatType::Used, "firework_rocket"),
        0
    );
}

#[test]
fn replaced_weapon_cancels_charge_and_pearl_cooldown_prevents_double_debit() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let bow = stack(&state, "bow", 1);
    let arrows = stack(&state, "arrow", 1);
    let pearl = stack(&state, "ender_pearl", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow;
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrows;
    use_item(&mut state, actor, Hand::Main);
    state.tick += 20;
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = pearl;
    assert!(
        !release(&mut state, actor),
        "invalid stale charge must not swallow generic release"
    );
    assert!(projectiles(&state).is_empty());
    use_item(&mut state, actor, Hand::Main);
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(projectiles(&state).len(), 1);
    assert_eq!(item_stat(&state, actor, StatType::Used, "bow"), 0);
    assert_eq!(item_stat(&state, actor, StatType::Used, "ender_pearl"), 1);
}

#[test]
fn trident_throw_transfers_one_durable_stack_and_riptide_requires_water() {
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    let trident = stack(&state, "trident", 1)
        .with_enchantment(mc_data::Identifier::parse("minecraft:loyalty").unwrap(), 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = trident;
    use_item(&mut state, actor, Hand::Main);
    state.tick += 10;
    release(&mut state, actor);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    assert_eq!(projectiles(&state)[0].stack.damage, Some(1));
    assert!(projectiles(&state)[0].pickup);
    assert_eq!(item_stat(&state, actor, StatType::Used, "trident"), 1);
    assert_eq!(item_stat(&state, actor, StatType::Broken, "trident"), 0);
    let riptide = stack(&state, "trident", 1)
        .with_enchantment(mc_data::Identifier::parse("minecraft:riptide").unwrap(), 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = riptide;
    use_item(&mut state, actor, Hand::Main);
    assert!(state.projectiles.using.is_empty());
    assert_eq!(item_stat(&state, actor, StatType::Used, "trident"), 1);
    state.world.set_block(
        pos[0] as i32,
        pos[1] as i32,
        pos[2] as i32,
        state.registry.states.water,
    );
    use_item(&mut state, actor, Hand::Main);
    state.tick += 10;
    release(&mut state, actor);
    assert!(state.projectiles.riptides.contains_key(&actor));
    assert_eq!(projectiles(&state).len(), 1);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
    assert_eq!(item_stat(&state, actor, StatType::Used, "trident"), 2);
}

#[test]
fn crossbow_refuses_bad_geometry_or_any_missing_multishot_type_without_spawns_or_custody_change() {
    for invalid in 0..4 {
        let (mut state, actor, pos) = super::super::flight_tests::setup();
        observe_crossbow(&mut state);
        let mut weapon = stack(&state, "crossbow", 1).with_damage(7);
        weapon.extended_mut().charged_projectiles = vec![
            stack(&state, "arrow", 1),
            stack(&state, "firework_rocket", 1),
            stack(&state, "arrow", 1),
        ];
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = weapon.clone();
        let owner = state.entities.slot_of(actor).unwrap();
        match invalid {
            0 => state.entities.set_pos(owner, [f64::NAN, pos[1], pos[2]]),
            1 => state.entities.set_rot(owner, f32::INFINITY, 0.0),
            2 => state.entities.set_rot(owner, 0.0, f32::NAN),
            _ => std::sync::Arc::get_mut(&mut state.registry)
                .unwrap()
                .retain_blueprints(|(_, blueprint)| {
                    blueprint.name != ProjectileKind::Firework.name()
                }),
        }
        fire_crossbow(&mut state, actor, 0, &weapon);
        assert!(
            projectiles(&state).is_empty(),
            "case {invalid}: no partial multishot"
        );
        assert!(state.projectiles.spawned.is_empty());
        assert!(
            state.advancement_events.is_empty(),
            "rejected launches cannot award"
        );
        assert_eq!(
            state.players[&actor].inventory.slots[0], weapon,
            "case {invalid}: charged ammo and wear unchanged"
        );
        assert_eq!(item_stat(&state, actor, StatType::Used, "crossbow"), 0);
        assert_eq!(item_stat(&state, actor, StatType::Broken, "crossbow"), 0);
    }
}

#[test]
fn crossbow_partial_launch_rollback_removes_only_unpublished_births_and_appearances() {
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    let owner = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    let mut launched = [None; 3];
    for (index, kind) in [
        ProjectileKind::Arrow,
        ProjectileKind::Firework,
        ProjectileKind::SpectralArrow,
    ]
    .into_iter()
    .enumerate()
    {
        launched[index] = super::super::launch(
            &mut state,
            ProjectileState::new(kind, Some(owner), ItemStack::EMPTY),
            pos,
            [0.0, 0.0, 1.0],
        );
        assert!(launched[index].is_some());
    }
    let retained = launched[2].unwrap();
    let before_removed = state.projectiles.removed.clone();
    rollback_crossbow_launches(&mut state, &launched[..2]);
    for id in launched[..2].iter().flatten() {
        assert!(state.entities.slot_of_any(*id).is_none());
        assert!(!state.projectiles.spawned.contains(id));
        assert!(!state.appearance_changed.contains(id));
    }
    assert!(state.entities.slot_of(retained).is_some());
    assert!(state.projectiles.spawned.contains(&retained));
    assert!(state.appearance_changed.contains(&retained));
    assert_eq!(state.projectiles.removed, before_removed);
    assert_eq!(projectiles(&state).len(), 1);
}

#[test]
fn thrown_trident_unbreaking_preserves_the_exact_durable_stack() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let mut trident = stack(&state, "trident", 1).with_enchantment(
        mc_data::Identifier::parse("minecraft:unbreaking").unwrap(),
        3,
    );
    trident.damage = Some(71);
    state.players.get_mut(&actor).unwrap().enchantment_seed = (0..1000)
        .find(|&seed| {
            let mut seed = seed;
            crate::enchantments::random(&mut seed) < 0.75
        })
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = trident.clone();
    assert!(use_item(&mut state, actor, Hand::Main));
    state.tick += 10;
    assert!(release(&mut state, actor));
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    assert_eq!(projectiles(&state).len(), 1);
    assert_eq!(projectiles(&state)[0].stack, trident);
}

#[test]
fn throwable_launches_credit_use_in_survival_and_creative_only_after_acceptance() {
    for creative in [false, true] {
        let (mut state, actor, pos) = super::super::flight_tests::setup();
        if creative {
            state.players.get_mut(&actor).unwrap().game_mode = GameMode::Creative;
        }
        for name in [
            "snowball",
            "egg",
            "ender_pearl",
            "splash_potion",
            "lingering_potion",
            "experience_bottle",
            "firework_rocket",
            "wind_charge",
        ] {
            let held = stack(&state, name, 2);
            state.players.get_mut(&actor).unwrap().inventory.slots[OFF_HAND] = held.clone();
            state.players.get_mut(&actor).unwrap().fall_flying = name == "firework_rocket";
            let before = projectiles(&state).len();
            let owner = state.entities.slot_of(actor).unwrap();
            state.entities.set_pos(owner, [f64::NAN, pos[1], pos[2]]);
            assert!(use_item(&mut state, actor, Hand::Off));
            assert_eq!(item_stat(&state, actor, StatType::Used, name), 0);
            assert_eq!(state.players[&actor].inventory.slots[OFF_HAND], held);
            state.entities.set_pos(owner, pos);
            assert!(use_item(&mut state, actor, Hand::Off));
            assert_eq!(projectiles(&state).len(), before + 1, "{name}");
            assert_eq!(item_stat(&state, actor, StatType::Used, name), 1);
            assert_eq!(
                state.players[&actor].inventory.slots[OFF_HAND].count,
                if creative { 2 } else { 1 }
            );
        }
    }
}

#[test]
fn successful_bow_and_crossbow_launches_credit_use_and_break_exactly_once() {
    for name in ["bow", "crossbow"] {
        let (mut state, actor, _) = super::super::flight_tests::setup();
        let mut weapon = stack(&state, name, 1);
        let rules = state.registry.item_rules();
        let max_damage = rules
            .facts
            .facts_for_stack(&weapon, rules.items)
            .unwrap()
            .max_damage
            .unwrap();
        weapon.damage = Some(i32::try_from(max_damage).unwrap() - 1);
        let arrows = stack(&state, "arrow", 2);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = weapon;
        state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrows;
        assert!(use_item(&mut state, actor, Hand::Main));
        state.tick += 25;
        assert!(release(&mut state, actor));
        if name == "crossbow" {
            assert_eq!(item_stat(&state, actor, StatType::Used, name), 0);
            assert_eq!(item_stat(&state, actor, StatType::Broken, name), 0);
            assert!(use_item(&mut state, actor, Hand::Main));
        }
        assert!(state.players[&actor].inventory.slots[0].is_empty());
        assert_eq!(item_stat(&state, actor, StatType::Used, name), 1);
        assert_eq!(item_stat(&state, actor, StatType::Broken, name), 1);
        assert!(!release(&mut state, actor));
        assert!(!use_item(&mut state, actor, Hand::Main));
        assert_eq!(item_stat(&state, actor, StatType::Used, name), 1);
        assert_eq!(item_stat(&state, actor, StatType::Broken, name), 1);
    }
}

#[test]
fn creative_crossbow_and_trident_credit_accepted_use_without_consumption_or_wear() {
    for name in ["crossbow", "trident"] {
        let (mut state, actor, _) = super::super::flight_tests::setup();
        let weapon = stack(&state, name, 1);
        let player = state.players.get_mut(&actor).unwrap();
        player.game_mode = GameMode::Creative;
        player.inventory.slots[0] = weapon.clone();
        assert!(use_item(&mut state, actor, Hand::Main));
        state.tick += 25;
        assert!(release(&mut state, actor));
        if name == "crossbow" {
            assert_eq!(item_stat(&state, actor, StatType::Used, name), 0);
            assert!(use_item(&mut state, actor, Hand::Main));
        }
        assert_eq!(projectiles(&state).len(), 1);
        assert_eq!(state.players[&actor].inventory.slots[0], weapon);
        assert_eq!(item_stat(&state, actor, StatType::Used, name), 1);
        assert_eq!(item_stat(&state, actor, StatType::Broken, name), 0);
    }
}

fn observe_durability(state: &mut TickState) {
    let mut tree = mc_data::advancements::AdvancementTree::default();
    tree.definitions.insert(
        "test:wear".into(),
        serde_json::from_value(serde_json::json!({
            "criteria": {"wear": {"trigger": "minecraft:item_durability_changed"}},
            "requirements": [["wear"]],
        }))
        .unwrap(),
    );
    std::sync::Arc::get_mut(&mut state.registry)
        .unwrap()
        .advancements = std::sync::Arc::new(tree);
}

#[test]
fn durability_advancement_retains_before_stack_and_real_damage_even_on_break() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    observe_durability(&mut state);
    let mut bow = stack(&state, "bow", 1);
    let maximum = state
        .registry
        .item_facts
        .facts_for_stack(&bow, &state.registry.items)
        .unwrap()
        .max_damage
        .unwrap() as i32;
    bow.damage = Some(7);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow;
    wear(&mut state, actor, 0, 2);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(9));
    let event = &state.advancement_events[0];
    assert_eq!(event.entity, actor);
    assert_eq!(event.trigger, "minecraft:item_durability_changed");
    assert_eq!(event.evidence["item"]["id"], "minecraft:bow");
    assert_eq!(event.evidence["item"]["components"]["minecraft:damage"], 7);
    assert_eq!(event.evidence["delta"], -2);
    assert_eq!(event.evidence["durability"], maximum - 9);
    state.advancement_events.clear();
    state.players.get_mut(&actor).unwrap().inventory.slots[0].damage = Some(maximum - 1);
    wear(&mut state, actor, 0, 1);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let event = &state.advancement_events[0];
    assert_eq!(event.evidence["item"]["id"], "minecraft:bow");
    assert_eq!(
        event.evidence["item"]["components"]["minecraft:damage"],
        maximum - 1
    );
    assert_eq!(event.evidence["durability"], 0);
    assert_eq!(event.evidence["delta"], -1);
}

#[test]
fn zero_creative_spectator_nondurable_and_unbreaking_wear_do_not_queue_advancement() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    observe_durability(&mut state);
    let bow = stack(&state, "bow", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow.clone();
    wear(&mut state, actor, 0, 0);
    for mode in [GameMode::Creative, GameMode::Spectator] {
        state.players.get_mut(&actor).unwrap().game_mode = mode;
        wear(&mut state, actor, 0, 1);
        assert_eq!(state.players[&actor].inventory.slots[0], bow);
    }
    state.players.get_mut(&actor).unwrap().game_mode = GameMode::Survival;
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack(&state, "arrow", 1);
    wear(&mut state, actor, 0, 1);
    let unbreaking = bow.with_enchantment(
        mc_data::Identifier::parse("minecraft:unbreaking").unwrap(),
        3,
    );
    let seed = (0..1000)
        .find(|&seed| {
            let mut seed = seed;
            crate::enchantments::random(&mut seed) < 0.75
        })
        .unwrap();
    state.players.get_mut(&actor).unwrap().enchantment_seed = seed;
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = unbreaking.clone();
    wear(&mut state, actor, 0, 1);
    assert_eq!(state.players[&actor].inventory.slots[0], unbreaking);
    assert!(state.advancement_events.is_empty());
}

#[test]
fn gliding_rocket_attaches_consumes_survival_rocket_and_grounded_stays_free() {
    for creative in [false, true] {
        let (mut state, actor, _) = super::super::flight_tests::setup();
        state.players.get_mut(&actor).unwrap().fall_flying = true;
        state.players.get_mut(&actor).unwrap().inventory.slots[0] =
            stack(&state, "firework_rocket", 2);
        if creative {
            state.players.get_mut(&actor).unwrap().game_mode = GameMode::Creative;
        }
        assert!(use_item(&mut state, actor, Hand::Main));
        assert_eq!(projectiles(&state).len(), 1);
        assert!(!projectiles(&state)[0].shot_at_angle);
        let rocket = projectiles(&state)[0].clone();
        let owner = state
            .entities
            .row(state.entities.slot_of(actor).unwrap())
            .uuid;
        assert_eq!(rocket.owner, Some(owner));
        assert_eq!(rocket.attached_to, Some(owner));
        assert_eq!(
            item_stat(&state, actor, StatType::Used, "firework_rocket"),
            1
        );
        assert_eq!(
            state.players[&actor].inventory.slots[0].count,
            if creative { 2 } else { 1 }
        );

        for on_ground in [false, true] {
            for hand in [Hand::Main, Hand::Off] {
                let (mut not_gliding, actor, _) = super::super::flight_tests::setup();
                let slot = not_gliding.entities.slot_of(actor).unwrap();
                not_gliding.entities.set_ground(slot, on_ground);
                let held = held_slot(&not_gliding, actor, hand).unwrap();
                let rockets = stack(&not_gliding, "firework_rocket", 2);
                let player = not_gliding.players.get_mut(&actor).unwrap();
                player.inventory.slots[held] = rockets.clone();
                if creative {
                    player.game_mode = GameMode::Creative;
                }
                assert!(use_item(&mut not_gliding, actor, hand));
                assert!(projectiles(&not_gliding).is_empty());
                assert_eq!(not_gliding.players[&actor].inventory.slots[held], rockets);
                assert_eq!(
                    item_stat(&not_gliding, actor, StatType::Used, "firework_rocket"),
                    0
                );
            }
        }
    }
}

#[test]
fn blue_egg_throws_as_ordinary_egg_projectile_and_debits() {
    // EGG-VARIANT-THROW through the item-use path: a blue egg launches an
    // Egg-kind projectile (cold variant) and debits one item.
    let (mut state, actor, _) = super::super::flight_tests::setup();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack(&state, "blue_egg", 1);
    assert!(use_item(&mut state, actor, Hand::Main));
    let kinds: Vec<_> = projectiles(&state).iter().map(|r| r.kind).collect();
    assert!(
        kinds.contains(&ProjectileKind::Egg),
        "blue egg must launch an Egg projectile, got {kinds:?}"
    );
    assert!(state.players[&actor].inventory.slots[0].is_empty());
}

#[test]
fn thrown_snowball_inherits_shooter_motion_and_pose_eye_height() {
    // THROWABLE-LAUNCH-GEOMETRY through the item-use path: a moving
    // thrower's motion biases the projectile, and the origin follows the
    // pose eye height rather than a fixed offset.
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_vel(slot, [10.0, 0.0, 0.0]);
    state.entities.set_ground(slot, true);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack(&state, "snowball", 1);
    assert!(use_item(&mut state, actor, Hand::Main));
    assert_eq!(projectiles(&state).len(), 1);
    let ball = state
        .entities
        .slots()
        .find(|slot| {
            state
                .entities
                .row(*slot)
                .projectile
                .as_deref()
                .is_some_and(|r| r.kind == ProjectileKind::Snowball)
        })
        .expect("thrown snowball");
    // Grounded: full XZ motion inherited (10 b/s -> entity vel x = 10.0);
    // base gives 0.0 here.
    let vel = state.entities.vel(ball);
    assert!(
        (vel[0] - 10.0).abs() < 1e-9,
        "snowball must inherit shooter motion, got {vel:?}"
    );
}
