use super::*;
use crate::plugin_inventories;
use mc_script::{
    ScriptInventoryEndpoint, ScriptInventoryMaterial, ScriptInventoryResourcePlan,
    ScriptInventoryWorkPortion, ScriptOperation, ScriptOperationPayload, ScriptOperationRequest,
    ScriptOwnedInventoryOperation, ScriptOwnedInventoryResult,
};

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

pub(crate) fn reserve(state: &mut TickState, actor: EntityId, materials: &[(&str, u64)]) -> String {
    let endpoint = ScriptInventoryEndpoint::PlayerInventory {
        player_id: state.players[&actor].connection.0,
    };
    let plan = ScriptInventoryResourcePlan::new(vec![ScriptInventoryWorkPortion::new(
        1,
        materials
            .iter()
            .map(|(name, count)| ScriptInventoryMaterial::new(format!("minecraft:{name}"), *count))
            .collect(),
    )]);
    let fence = plugin_inventories::load(state, "demo", &endpoint)
        .unwrap()
        .fence;
    let request = ScriptOperationRequest::try_new(
        "reserve",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Reserve {
                operation_id: uuid::Uuid::new_v4().simple().to_string(),
                endpoint,
                resource_plan: plan,
                expected_revision: fence,
            },
        },
    )
    .unwrap();
    let outcome = plugin_inventories::execute(state, "demo", &request);
    assert_eq!(outcome.failure(), None);
    let ScriptOperationPayload::OwnedInventory { result } = outcome.payload() else {
        panic!("native inventory outcome");
    };
    let ScriptOwnedInventoryResult::Reservation { reservation } = result.as_ref() else {
        panic!("native reserve outcome");
    };
    reservation.reservation_ref.clone()
}

pub(crate) fn release_hold(state: &mut TickState, reference: &str) {
    let value = plugin_inventories::reservation(state, "demo", reference).unwrap();
    let request = ScriptOperationRequest::try_new(
        "release",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Release {
                operation_id: uuid::Uuid::new_v4().simple().to_string(),
                reservation_ref: reference.into(),
                expected_revision: value.revision,
            },
        },
    )
    .unwrap();
    assert_eq!(
        plugin_inventories::execute(state, "demo", &request).failure(),
        None
    );
    assert!(
        plugin_inventories::reservation(state, "demo", reference)
            .unwrap()
            .snapshot
            .released
    );
}

fn projectiles(state: &TickState) -> Vec<&ProjectileState> {
    state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).projectile.as_deref())
        .collect()
}

fn draw(state: &mut TickState, actor: EntityId, ticks: u64) {
    assert!(use_item(state, actor, Hand::Main));
    assert!(state.projectiles.using.contains_key(&actor));
    state.tick += ticks;
}

fn unbreaking_seed(stack: &ItemStack, amount: i32, damaging: bool) -> i32 {
    (0..10000)
        .find(|seed| {
            let mut seed = *seed;
            (crate::enchantments::durability_damage(stack, amount, false, &mut seed) > 0)
                == damaging
        })
        .unwrap()
}

#[test]
fn vanilla_bow_spends_only_aggregate_excess_and_release_unlocks_last_arrow() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let bow = stack(&state, "bow", 1);
    let arrows = stack(&state, "arrow", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow;
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrows;
    let first = reserve(&mut state, actor, &[("arrow", 1)]);
    let second = reserve(&mut state, actor, &[("arrow", 1)]);
    assert_eq!(state.players[&actor].inventory.slots[9].count, 3);
    draw(&mut state, actor, 20);
    assert!(release(&mut state, actor));
    assert_eq!(projectiles(&state).len(), 1);
    assert_eq!(state.players[&actor].inventory.slots[9].count, 2);
    let inventory = state.players[&actor].inventory.clone();
    let seed = state.players[&actor].enchantment_seed;
    let births = state.projectiles.spawned.clone();
    let appearances = state.appearance_changed.clone();
    assert!(use_item(&mut state, actor, Hand::Main));
    assert!(state.projectiles.using.is_empty());
    assert_eq!(state.players[&actor].inventory, inventory);
    assert_eq!(state.players[&actor].enchantment_seed, seed);
    assert_eq!(state.projectiles.spawned, births);
    assert_eq!(state.appearance_changed, appearances);
    release_hold(&mut state, &first);
    assert!(
        !plugin_inventories::reservation(&state, "demo", &second)
            .unwrap()
            .snapshot
            .released
    );
    draw(&mut state, actor, 20);
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 2);
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
}

#[test]
fn bow_ammo_and_actual_weapon_break_are_admitted_atomically_without_seed_or_use_changes() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let mut bow = stack(&state, "bow", 1).with_enchantment(
        mc_data::Identifier::parse("minecraft:unbreaking").unwrap(),
        3,
    );
    bow.damage = Some(383);
    let seed = unbreaking_seed(&bow, 1, true);
    let arrows = stack(&state, "arrow", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow;
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrows;
    state.players.get_mut(&actor).unwrap().enchantment_seed = seed;
    let hold = reserve(&mut state, actor, &[("bow", 1), ("arrow", 1)]);
    draw(&mut state, actor, 20);
    let inventory = state.players[&actor].inventory.clone();
    let started = state.projectiles.using[&actor].started;
    let births = state.projectiles.spawned.clone();
    let changes = state.appearance_changed.clone();
    let extra = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .appearance
        .extra
        .clone();
    release(&mut state, actor);
    assert!(projectiles(&state).is_empty());
    assert_eq!(state.players[&actor].inventory, inventory);
    assert_eq!(state.players[&actor].enchantment_seed, seed);
    assert_eq!(state.projectiles.using[&actor].started, started);
    assert_eq!(state.projectiles.spawned, births);
    assert_eq!(state.appearance_changed, changes);
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(actor).unwrap())
            .appearance
            .extra,
        extra
    );
    release_hold(&mut state, &hold);
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 1);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
    assert_ne!(state.players[&actor].enchantment_seed, seed);
}

#[test]
fn reserved_last_bow_allows_real_unbreaking_no_damage_but_refuses_actual_break() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let mut bow = stack(&state, "bow", 1).with_enchantment(
        mc_data::Identifier::parse("minecraft:unbreaking").unwrap(),
        3,
    );
    bow.damage = Some(383);
    let harmless_seed = unbreaking_seed(&bow, 1, false);
    let breaking_seed = unbreaking_seed(&bow, 1, true);
    let arrows = stack(&state, "arrow", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow.clone();
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrows;
    state.players.get_mut(&actor).unwrap().enchantment_seed = harmless_seed;
    let hold = reserve(&mut state, actor, &[("bow", 1)]);
    draw(&mut state, actor, 20);
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 1);
    assert_eq!(state.players[&actor].inventory.slots[0], bow);
    let mut expected_seed = harmless_seed;
    crate::enchantments::durability_damage(&bow, 1, false, &mut expected_seed);
    assert_eq!(state.players[&actor].enchantment_seed, expected_seed);
    state.players.get_mut(&actor).unwrap().enchantment_seed = breaking_seed;
    draw(&mut state, actor, 20);
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 1);
    assert_eq!(state.players[&actor].inventory.slots[0], bow);
    assert_eq!(state.players[&actor].inventory.slots[9].count, 2);
    assert_eq!(state.players[&actor].enchantment_seed, breaking_seed);
    release_hold(&mut state, &hold);
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 2);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
}

#[test]
fn infinity_uses_pinned_normal_arrow_without_debit_and_legal_reserved_bow_wear() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let bow = stack(&state, "bow", 1)
        .with_enchantment(mc_data::Identifier::parse("minecraft:infinity").unwrap(), 1);
    let arrow = stack(&state, "arrow", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow;
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrow;
    reserve(&mut state, actor, &[("bow", 1), ("arrow", 1)]);
    draw(&mut state, actor, 20);
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 1);
    assert!(!projectiles(&state)[0].pickup);
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
    assert!(!plugin_inventories::below_player_debit_floors(
        &state,
        actor,
        &[],
        None
    ));
}

#[test]
fn native_throwables_and_fireworks_refuse_pinned_unit_before_birth_or_cooldown() {
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
        let (mut state, actor, pos) = super::super::flight_tests::setup();
        let item = stack(&state, name, 1);
        let item_id = item.item_id;
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = item.clone();
        state.players.get_mut(&actor).unwrap().fall_flying = name == "firework_rocket";
        let hold = reserve(&mut state, actor, &[(name, 1)]);
        let births = state.projectiles.spawned.clone();
        let appearances = state.appearance_changed.clone();
        let seed = state.players[&actor].enchantment_seed;
        let block = state
            .world
            .block_at(pos[0] as i32, pos[1] as i32 - 1, pos[2] as i32);
        assert!(use_item(&mut state, actor, Hand::Main));
        assert!(projectiles(&state).is_empty(), "{name}");
        assert_eq!(state.players[&actor].inventory.slots[0], item, "{name}");
        assert!(!state.players[&actor].item_on_cooldown(item_id), "{name}");
        assert_eq!(state.players[&actor].enchantment_seed, seed);
        assert_eq!(state.projectiles.spawned, births);
        assert_eq!(state.appearance_changed, appearances);
        assert_eq!(
            state
                .world
                .block_at(pos[0] as i32, pos[1] as i32 - 1, pos[2] as i32),
            block
        );
        release_hold(&mut state, &hold);
        use_item(&mut state, actor, Hand::Main);
        assert_eq!(projectiles(&state).len(), 1, "{name}");
        assert!(
            state.players[&actor].inventory.slots[0].is_empty(),
            "{name}"
        );
        assert_eq!(
            state.players[&actor].item_on_cooldown(item_id),
            matches!(name, "ender_pearl" | "wind_charge")
        );
    }
}

#[test]
fn native_throwable_excess_is_not_escrow_and_can_be_launched() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let eggs = stack(&state, "egg", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = eggs;
    reserve(&mut state, actor, &[("egg", 1)]);
    for expected in [2, 1] {
        use_item(&mut state, actor, Hand::Main);
        assert_eq!(state.players[&actor].inventory.slots[0].count, expected);
    }
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(projectiles(&state).len(), 2);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
}

#[test]
fn crossbow_load_rechecks_late_hold_without_charge_mutation_then_multishot_loads_once() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let bow = stack(&state, "crossbow", 1).with_enchantment(
        mc_data::Identifier::parse("minecraft:multishot").unwrap(),
        1,
    );
    let arrow = stack(&state, "arrow", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow.clone();
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = arrow;
    draw(&mut state, actor, 25);
    let hold = reserve(&mut state, actor, &[("arrow", 1)]);
    let started = state.projectiles.using[&actor].started;
    let appearances = state.appearance_changed.clone();
    tick(&mut state, &mut Vec::new());
    assert_eq!(state.projectiles.using[&actor].started, started);
    assert_eq!(state.players[&actor].inventory.slots[0], bow);
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
    assert_eq!(state.appearance_changed, appearances);
    release(&mut state, actor);
    assert_eq!(state.projectiles.using[&actor].started, started);
    release_hold(&mut state, &hold);
    tick(&mut state, &mut Vec::new());
    assert!(state.projectiles.using.is_empty());
    assert!(state.players[&actor].inventory.slots[9].is_empty());
    assert_eq!(
        state.players[&actor].inventory.slots[0]
            .extended()
            .charged_projectiles
            .len(),
        3
    );
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(projectiles(&state).len(), 3);
    assert_eq!(projectiles(&state).iter().filter(|p| p.pickup).count(), 1);
}

#[test]
fn loaded_crossbow_refuses_actual_multishot_break_before_any_volleys_or_rng() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let mut weapon = stack(&state, "crossbow", 1).with_enchantment(
        mc_data::Identifier::parse("minecraft:unbreaking").unwrap(),
        3,
    );
    weapon.damage = Some(464);
    weapon.extended_mut().charged_projectiles = vec![stack(&state, "arrow", 1); 3];
    let seed = unbreaking_seed(&weapon, 3, true);
    state.players.get_mut(&actor).unwrap().enchantment_seed = seed;
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = weapon.clone();
    let hold = reserve(&mut state, actor, &[("crossbow", 1)]);
    let births = state.projectiles.spawned.clone();
    let changes = state.appearance_changed.clone();
    use_item(&mut state, actor, Hand::Main);
    assert!(projectiles(&state).is_empty());
    assert_eq!(state.players[&actor].inventory.slots[0], weapon);
    assert_eq!(state.players[&actor].enchantment_seed, seed);
    assert_eq!(state.projectiles.spawned, births);
    assert_eq!(state.appearance_changed, changes);
    release_hold(&mut state, &hold);
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(projectiles(&state).len(), 3);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let mut expected_seed = seed;
    crate::enchantments::durability_damage(&weapon, 3, false, &mut expected_seed);
    assert_eq!(state.players[&actor].enchantment_seed, expected_seed);
}

#[test]
fn loaded_fireworks_apply_canonical_three_points_each_and_allow_component_wear() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let mut weapon = stack(&state, "crossbow", 1);
    weapon.extended_mut().charged_projectiles = vec![stack(&state, "firework_rocket", 1); 3];
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = weapon;
    reserve(&mut state, actor, &[("crossbow", 1)]);
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(projectiles(&state).len(), 3);
    assert!(
        projectiles(&state)
            .iter()
            .all(|p| p.kind == ProjectileKind::Firework)
    );
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(9));
    assert!(
        state.players[&actor].inventory.slots[0]
            .extended()
            .charged_projectiles
            .is_empty()
    );
    assert!(!plugin_inventories::below_player_debit_floors(
        &state,
        actor,
        &[],
        None
    ));
}

#[test]
fn thrown_trident_moves_exact_worn_components_only_when_aggregate_custody_allows() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let mut trident = stack(&state, "trident", 1)
        .with_enchantment(
            mc_data::Identifier::parse("minecraft:unbreaking").unwrap(),
            3,
        )
        .with_enchantment(mc_data::Identifier::parse("minecraft:loyalty").unwrap(), 2);
    trident.damage = Some(71);
    let seed = unbreaking_seed(&trident, 1, false);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = trident.clone();
    state.players.get_mut(&actor).unwrap().enchantment_seed = seed;
    let hold = reserve(&mut state, actor, &[("trident", 1)]);
    use_item(&mut state, actor, Hand::Main);
    assert!(state.projectiles.using.is_empty());
    assert_eq!(state.players[&actor].enchantment_seed, seed);
    let spare = stack(&state, "trident", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = spare.clone();
    draw(&mut state, actor, 10);
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 1);
    assert_eq!(projectiles(&state)[0].stack, trident);
    assert!(projectiles(&state)[0].pickup);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    assert_eq!(state.players[&actor].inventory.slots[9], spare);
    assert!(
        !plugin_inventories::reservation(&state, "demo", &hold)
            .unwrap()
            .snapshot
            .released
    );
    assert!(!plugin_inventories::below_player_debit_floors(
        &state,
        actor,
        &[],
        None
    ));
}

#[test]
fn held_trident_late_reserve_refuses_transfer_without_rng_and_release_enables_it() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let trident = stack(&state, "trident", 1).with_enchantment(
        mc_data::Identifier::parse("minecraft:unbreaking").unwrap(),
        3,
    );
    let seed = unbreaking_seed(&trident, 1, true);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = trident.clone();
    state.players.get_mut(&actor).unwrap().enchantment_seed = seed;
    draw(&mut state, actor, 10);
    let hold = reserve(&mut state, actor, &[("trident", 1)]);
    let appearances = state.appearance_changed.clone();
    release(&mut state, actor);
    assert!(projectiles(&state).is_empty());
    assert!(state.projectiles.using.contains_key(&actor));
    assert_eq!(state.players[&actor].inventory.slots[0], trident);
    assert_eq!(state.players[&actor].enchantment_seed, seed);
    assert_eq!(state.appearance_changed, appearances);
    release_hold(&mut state, &hold);
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 1);
    let mut expected = trident;
    expected.damage = Some(1);
    assert_eq!(projectiles(&state)[0].stack, expected);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
}

#[test]
fn creative_free_arrow_does_not_break_or_spend_reserved_crossbow() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let mut weapon = stack(&state, "crossbow", 1);
    weapon.damage = Some(464);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = weapon;
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    reserve(&mut state, actor, &[("crossbow", 1)]);
    draw(&mut state, actor, 25);
    release(&mut state, actor);
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(projectiles(&state).len(), 1);
    assert!(!projectiles(&state)[0].pickup);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(464));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
}

#[test]
fn held_riptide_trident_keeps_source_and_native_spin_metadata_after_legal_wear() {
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    let mut trident = stack(&state, "trident", 1)
        .with_enchantment(mc_data::Identifier::parse("minecraft:riptide").unwrap(), 3);
    trident.damage = Some(248);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = trident;
    reserve(&mut state, actor, &[("trident", 1)]);
    state.world.set_block(
        pos[0] as i32,
        pos[1] as i32,
        pos[2] as i32,
        state.registry.states.water,
    );
    let actor_slot = state.entities.slot_of(actor).unwrap();
    let old_velocity = state.entities.vel(actor_slot);
    draw(&mut state, actor, 10);
    release(&mut state, actor);
    assert!(projectiles(&state).is_empty());
    assert!(state.projectiles.riptides.contains_key(&actor));
    assert_ne!(state.entities.vel(actor_slot), old_velocity);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(249));
    assert!(
        state
            .entities
            .row(actor_slot)
            .appearance
            .extra
            .iter()
            .any(|(index, value)| { *index == 8 && matches!(value, EntityDataValue::Byte(4)) })
    );
    assert!(!plugin_inventories::below_player_debit_floors(
        &state,
        actor,
        &[],
        None
    ));
}

#[test]
fn bow_uses_canonical_offhand_ammo_not_reserved_ordinary_stock_and_infinity_is_not_free_for_spectral()
 {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let bow = stack(&state, "bow", 1)
        .with_enchantment(mc_data::Identifier::parse("minecraft:infinity").unwrap(), 1);
    let spectral = stack(&state, "spectral_arrow", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = bow;
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = spectral.clone();
    state.players.get_mut(&actor).unwrap().inventory.slots[OFF_HAND] = spectral;
    reserve(&mut state, actor, &[("spectral_arrow", 1)]);
    draw(&mut state, actor, 20);
    release(&mut state, actor);
    assert_eq!(projectiles(&state).len(), 1);
    assert!(projectiles(&state)[0].pickup);
    assert!(state.players[&actor].inventory.slots[OFF_HAND].is_empty());
    assert_eq!(state.players[&actor].inventory.slots[9].count, 1);
    let bow = state.players[&actor].inventory.slots[0].clone();
    use_item(&mut state, actor, Hand::Main);
    assert!(state.projectiles.using.is_empty());
    assert_eq!(projectiles(&state).len(), 1);
    assert_eq!(state.players[&actor].inventory.slots[0], bow);
}
