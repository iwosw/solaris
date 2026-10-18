use super::flat_player_fixture;
use crate::boundary::{Hand, PlayerChange};
use crate::effects::{ActiveEffect, EffectId};
use mc_data::{Identifier, ItemStack};

fn item(state: &crate::tick::TickState, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap(),
        count,
    )
}

#[test]
fn honey_removes_only_poison_and_returns_one_bottle() {
    let (mut state, entity, _) = flat_player_fixture();
    let honey = item(&state, "honey_bottle", 2);
    let registry = std::sync::Arc::clone(&state.registry);
    let rules = registry.item_rules();
    let player = state.players.get_mut(&entity).unwrap();
    player.inventory.slots[0] = honey;
    player
        .active_effects
        .add(ActiveEffect::new(EffectId::Poison, 100, 0));
    player
        .active_effects
        .add(ActiveEffect::new(EffectId::Speed, 100, 0));
    player.use_item(&rules, Hand::Main);
    let mut removed = false;
    for _ in 0..100 {
        removed |= player.tick_item_use(&rules).changes.iter().any(|change| {
            matches!(
                change,
                PlayerChange::RemoveMobEffect {
                    id: EffectId::Poison,
                    ..
                }
            )
        });
    }
    assert!(removed);
    assert!(!player.active_effects.has(EffectId::Poison));
    assert!(player.active_effects.has(EffectId::Speed));
    assert_eq!(player.inventory.slots[0].count, 1);
    let bottle = rules
        .items
        .id_of(&Identifier::parse("minecraft:glass_bottle").unwrap())
        .unwrap();
    assert_eq!(
        player
            .inventory
            .slots
            .iter()
            .filter(|s| s.item_id == bottle)
            .map(|s| s.count)
            .sum::<i32>(),
        1
    );
}

#[test]
fn chorus_completion_starts_cooldown_and_never_double_consumes() {
    let (mut state, entity, _) = flat_player_fixture();
    let fruit = item(&state, "chorus_fruit", 2);
    let registry = std::sync::Arc::clone(&state.registry);
    let rules = registry.item_rules();
    let player = state.players.get_mut(&entity).unwrap();
    player.inventory.slots[0] = fruit.clone();
    player.use_item(&rules, Hand::Main);
    let mut completions = 0;
    for _ in 0..100 {
        let effects = player.tick_item_use(&rules);
        if effects.chorus_consumed {
            completions += 1;
            assert!(effects.changes.iter().any(|c| matches!(c, PlayerChange::ItemCooldown { group, ticks:20, .. } if group == "minecraft:chorus_fruit")));
            assert!(player.item_on_cooldown(fruit.item_id));
            player.use_item(&rules, Hand::Main);
        }
    }
    assert_eq!(completions, 1);
    assert_eq!(player.inventory.slots[0].count, 1);
    assert!(!player.item_on_cooldown(fruit.item_id));
}

#[test]
fn cooldown_expiry_is_exact_and_local_to_item() {
    let (mut state, entity, _) = flat_player_fixture();
    let pearl = item(&state, "ender_pearl", 1);
    let fruit = item(&state, "chorus_fruit", 1);
    let registry = std::sync::Arc::clone(&state.registry);
    let rules = registry.item_rules();
    let player = state.players.get_mut(&entity).unwrap();
    player
        .start_item_cooldown(&rules, pearl.item_id, 20)
        .unwrap();
    assert!(!player.item_on_cooldown(fruit.item_id));
    for _ in 0..19 {
        player.tick_item_use(&rules);
    }
    assert!(player.item_on_cooldown(pearl.item_id));
    player.tick_item_use(&rules);
    assert!(!player.item_on_cooldown(pearl.item_id));
}

#[test]
fn chorus_destinations_are_supported_clear_and_loaded() {
    let (state, _, pos) = flat_player_fixture();
    let mut found = 0;
    for seed in 0..64 {
        if let Some(to) = crate::consumables::chorus_destination(&state, pos, seed) {
            found += 1;
            let below = state
                .world
                .block_at(to[0].floor() as i32, to[1] as i32 - 1, to[2].floor() as i32)
                .unwrap();
            assert!(
                !mc_data::collision_shapes::vanilla_collision_shapes()
                    .get(below)
                    .unwrap()
                    .is_empty()
            );
            assert!(
                state
                    .world
                    .block_at(to[0].floor() as i32, to[1] as i32, to[2].floor() as i32)
                    .is_some()
            );
            assert!(to[0] >= pos[0] - 8.0 && to[0] < pos[0] + 8.0);
            assert!(to[2] >= pos[2] - 8.0 && to[2] < pos[2] + 8.0);
        }
    }
    assert!(found > 0);
    assert!(
        crate::consumables::chorus_destination(&state, [10_000.0, 200.0, 10_000.0], 0).is_none()
    );
}

#[test]
fn chorus_teleport_updates_native_position_and_resets_fall() {
    let (mut state, entity, pos) = flat_player_fixture();
    let slot = state.entities.slot_of(entity).unwrap();
    state.players.get_mut(&entity).unwrap().fall_distance = 12.0;
    let mut success = false;
    for tick in 1..64 {
        state.tick = tick;
        crate::consumables::finish_chorus(&mut state, entity);
        if state.entities.pos(slot) != pos {
            success = true;
            break;
        }
    }
    assert!(success);
    assert_eq!(state.players[&entity].fall_distance, 0.0);
    assert_eq!(state.entities.vel(slot), [0.0; 3]);
    assert!(
        state
            .outbox
            .values()
            .flatten()
            .any(|c| matches!(c, PlayerChange::PositionCorrected { .. }))
    );
}

#[test]
fn ominous_bottle_consumption_uses_component_amplifier_at_full_hunger() {
    let (mut state, entity, _) = flat_player_fixture();
    let mut bottle = item(&state, "ominous_bottle", 1);
    bottle.extended_mut().ominous_bottle_amplifier = Some(3);
    let registry = std::sync::Arc::clone(&state.registry);
    let rules = registry.item_rules();
    let player = state.players.get_mut(&entity).unwrap();
    player.inventory.slots[0] = bottle;
    player.use_item(&rules, Hand::Main);
    for _ in 0..100 {
        player.tick_item_use(&rules);
    }
    assert_eq!(player.active_effects.level(EffectId::BadOmen), 4.0);
    assert!(player.inventory.slots[0].is_empty());
}

#[test]
fn repeated_shield_use_preserves_raise_delay_and_release_restarts_it() {
    let (mut state, entity, _) = flat_player_fixture();
    let shield = item(&state, "shield", 1);
    let registry = std::sync::Arc::clone(&state.registry);
    let rules = registry.item_rules();
    let player = state.players.get_mut(&entity).unwrap();
    player.inventory.slots[0] = shield;
    for tick in 1..=6 {
        player.use_item(&rules, Hand::Main);
        player.tick_item_use(&rules);
        assert_eq!(
            player.raised_shield(&rules),
            (tick >= 5).then_some(Hand::Main)
        );
    }
    player.release_use_item();
    assert_eq!(player.raised_shield(&rules), None);
    player.use_item(&rules, Hand::Main);
    player.tick_item_use(&rules);
    assert_eq!(player.raised_shield(&rules), None);
}
