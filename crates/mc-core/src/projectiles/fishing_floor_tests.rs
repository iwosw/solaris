use super::super::weapons::floor_tests::{release_hold, reserve, stack};
use super::*;

fn cast(state: &mut TickState, actor: EntityId) -> EntityId {
    assert!(use_item(state, actor, Hand::Main));
    *state.projectiles.spawned.last().unwrap()
}

fn unbreaking_rod(state: &TickState, damage: i32) -> ItemStack {
    let mut rod = stack(state, "fishing_rod", 1).with_enchantment(
        mc_data::Identifier::parse("minecraft:unbreaking").unwrap(),
        3,
    );
    rod.damage = Some(damage);
    rod
}

fn seed_for(rod: &ItemStack, amount: i32, damaging: bool) -> i32 {
    (0..10000)
        .find(|seed| {
            let mut seed = *seed;
            (crate::enchantments::durability_damage(rod, amount, false, &mut seed) > 0) == damaging
        })
        .unwrap()
}

#[test]
fn held_rod_cast_is_free_but_actual_reel_break_refuses_victim_impulse_and_hook_removal() {
    let (mut state, actor, pos) = super::super::flight_tests::setup();
    let rod = unbreaking_rod(&state, 63);
    let seed = seed_for(&rod, 5, true);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = rod.clone();
    state.players.get_mut(&actor).unwrap().enchantment_seed = seed;
    let hold = reserve(&mut state, actor, &[("fishing_rod", 1)]);
    let hook = cast(&mut state, actor);
    assert_eq!(state.players[&actor].inventory.slots[0], rod);
    assert_eq!(state.players[&actor].enchantment_seed, seed);
    let cow = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let victim = state.spawn_mob(cow, [pos[0] + 4.0, pos[1], pos[2]], 20.0);
    let hook_slot = state.entities.slot_of(hook).unwrap();
    let mut record = state.entities.row_mut(hook_slot).projectile.take().unwrap();
    impact(&mut state, hook, &mut record, Some(victim), pos);
    state.entities.row_mut(hook_slot).projectile = Some(record);
    let before_hook =
        serde_json::to_value(state.entities.row(hook_slot).projectile.as_ref().unwrap()).unwrap();
    let victim_slot = state.entities.slot_of(victim).unwrap();
    let velocity = state.entities.vel(victim_slot);
    let births = state.projectiles.spawned.clone();
    let changes = state.appearance_changed.clone();
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(state.players[&actor].inventory.slots[0], rod);
    assert_eq!(state.players[&actor].enchantment_seed, seed);
    assert_eq!(state.entities.vel(victim_slot), velocity);
    assert_eq!(
        serde_json::to_value(state.entities.row(hook_slot).projectile.as_ref().unwrap()).unwrap(),
        before_hook
    );
    assert_eq!(state.projectiles.spawned, births);
    assert_eq!(state.appearance_changed, changes);
    release_hold(&mut state, &hold);
    use_item(&mut state, actor, Hand::Main);
    assert!(state.entities.slot_of(hook).is_none());
    assert!(state.entities.vel(state.entities.slot_of(victim).unwrap())[0] < velocity[0]);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let mut expected_seed = seed;
    crate::enchantments::durability_damage(&rod, 5, false, &mut expected_seed);
    assert_eq!(state.players[&actor].enchantment_seed, expected_seed);
}

#[test]
fn catch_break_preflight_does_not_spawn_loot_or_xp_then_release_commits_exact_native_catch() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let rod = unbreaking_rod(&state, 63);
    let enchantment_seed = seed_for(&rod, 1, true);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = rod.clone();
    state.players.get_mut(&actor).unwrap().enchantment_seed = enchantment_seed;
    let hold = reserve(&mut state, actor, &[("fishing_rod", 1)]);
    let hook = cast(&mut state, actor);
    let hook_slot = state.entities.slot_of(hook).unwrap();
    let seed = state.entities.row(hook_slot).uuid.as_u128() as u64 ^ state.tick;
    let expected_loot = loot(&state.registry, 0, false, false, seed).unwrap();
    let record = state
        .entities
        .row_mut(hook_slot)
        .projectile
        .as_mut()
        .unwrap();
    let ProjectileData::Fishing(fishing) = &mut record.data else {
        panic!("native hook");
    };
    fishing.bite_until = Some(state.tick + 10);
    let entities = state.entities.len();
    let births = state.projectiles.spawned.clone();
    let changes = state.appearance_changed.clone();
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(state.entities.len(), entities);
    assert_eq!(state.projectiles.spawned, births);
    assert_eq!(state.appearance_changed, changes);
    assert!(state.entities.slot_of(hook).is_some());
    assert_eq!(state.players[&actor].inventory.slots[0], rod);
    assert_eq!(state.players[&actor].enchantment_seed, enchantment_seed);
    release_hold(&mut state, &hold);
    let before = state.projectiles.spawned.len();
    use_item(&mut state, actor, Hand::Main);
    assert!(state.entities.slot_of(hook).is_none());
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let drops: Vec<_> = state.projectiles.spawned[before..]
        .iter()
        .filter_map(|id| {
            state
                .entities
                .slot_of(*id)
                .and_then(|slot| state.entities.row(slot).item.as_ref())
        })
        .collect();
    assert_eq!(drops.len(), 1);
    assert_eq!(drops[0].full_stack(), expected_loot);
    assert!(
        state.projectiles.spawned.len() > before + 1,
        "native reel also awards XP"
    );
    let mut expected_seed = enchantment_seed;
    crate::enchantments::durability_damage(&rod, 1, false, &mut expected_seed);
    assert_eq!(state.players[&actor].enchantment_seed, expected_seed);
}

#[test]
fn actual_unbreaking_skip_allows_reserved_last_rod_to_reel_a_catch() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let rod = unbreaking_rod(&state, 63);
    let seed = seed_for(&rod, 1, false);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = rod.clone();
    state.players.get_mut(&actor).unwrap().enchantment_seed = seed;
    let hold = reserve(&mut state, actor, &[("fishing_rod", 1)]);
    let hook = cast(&mut state, actor);
    let hook_slot = state.entities.slot_of(hook).unwrap();
    let record = state
        .entities
        .row_mut(hook_slot)
        .projectile
        .as_mut()
        .unwrap();
    let ProjectileData::Fishing(fishing) = &mut record.data else {
        panic!("native hook");
    };
    fishing.bite_until = Some(state.tick + 10);
    let before = state.projectiles.spawned.len();
    use_item(&mut state, actor, Hand::Main);
    assert!(state.entities.slot_of(hook).is_none());
    assert_eq!(state.players[&actor].inventory.slots[0], rod);
    assert!(state.projectiles.spawned.len() > before);
    let mut expected_seed = seed;
    crate::enchantments::durability_damage(&rod, 1, false, &mut expected_seed);
    assert_eq!(state.players[&actor].enchantment_seed, expected_seed);
    assert!(
        !crate::plugin_inventories::reservation(&state, "demo", &hold)
            .unwrap()
            .snapshot
            .released
    );
    assert!(!crate::plugin_inventories::below_player_debit_floors(
        &state,
        actor,
        &[],
        None
    ));
}

#[test]
fn fishing_aggregate_excess_allows_break_but_reserved_last_rod_allows_nonbreaking_wear() {
    let (mut state, actor, _) = super::super::flight_tests::setup();
    let mut rod = stack(&state, "fishing_rod", 1);
    rod.damage = Some(62);
    let spare = stack(&state, "fishing_rod", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = rod.clone();
    state.players.get_mut(&actor).unwrap().inventory.slots[9] = spare.clone();
    reserve(&mut state, actor, &[("fishing_rod", 1)]);
    let hook = cast(&mut state, actor);
    let slot = state.entities.slot_of(hook).unwrap();
    state
        .entities
        .row_mut(slot)
        .projectile
        .as_mut()
        .unwrap()
        .stuck = true;
    use_item(&mut state, actor, Hand::Main);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    assert_eq!(state.players[&actor].inventory.slots[9], spare);
    assert!(state.entities.slot_of(hook).is_none());
    state
        .players
        .get_mut(&actor)
        .unwrap()
        .inventory
        .slots
        .swap(0, 9);
    let hook = cast(&mut state, actor);
    let slot = state.entities.slot_of(hook).unwrap();
    state
        .entities
        .row_mut(slot)
        .projectile
        .as_mut()
        .unwrap()
        .stuck = true;
    use_item(&mut state, actor, Hand::Main);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(2));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert!(state.entities.slot_of(hook).is_none());
    assert!(!crate::plugin_inventories::below_player_debit_floors(
        &state,
        actor,
        &[],
        None
    ));
}
