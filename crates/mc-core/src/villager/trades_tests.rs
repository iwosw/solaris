use super::*;
use crate::inventory::{MenuKind, MerchantMenu};

fn registry() -> CoreRegistry {
    CoreRegistry::load(None).unwrap()
}
fn item(registry: &CoreRegistry, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap(),
        count,
    )
}
fn fixture(registry: &CoreRegistry) -> (VillagerState, Menu, PlayerInventory) {
    let mut villager = VillagerState::default();
    villager.data.profession = VillagerProfession::Farmer;
    villager.offers.push(VillagerTradeOffer {
        cost_a: item(registry, "emerald", 2),
        cost_b: Some(item(registry, "wheat", 3)),
        result: item(registry, "bread", 4).with_custom_name("Traded bread"),
        uses: 0,
        max_uses: 2,
        reward_exp: true,
        xp: 5,
        special_price: 0,
        demand: 0,
        price_multiplier: 0.05,
    });
    let mut menu = Menu::new(1, MenuKind::Merchant);
    menu.merchant = Some(MerchantMenu {
        entity: EntityId(2),
        selected: 0,
        last_restock: 0,
        reputation: 0,
        hero: None,
    });
    menu.grid = vec![item(registry, "emerald", 10), item(registry, "wheat", 9)];
    refresh(&mut menu, &villager, &registry.item_rules(), Uuid::nil());
    (villager, menu, PlayerInventory::default())
}

#[test]
fn trade_execution_is_atomic_with_two_costs_and_exact_result_components() {
    let registry = registry();
    let (mut villager, mut menu, mut inventory) = fixture(&registry);
    menu.carried = item(&registry, "stone", 1);
    let before = (villager.clone(), menu.clone(), inventory.clone());
    assert_eq!(
        execute(
            &mut menu,
            &mut inventory,
            &mut villager,
            &registry,
            Uuid::nil(),
            0,
            ClickMode::Pickup
        )
        .trades,
        0
    );
    assert_eq!((villager.clone(), menu.clone(), inventory.clone()), before);
    menu.carried = ItemStack::EMPTY;
    let outcome = execute(
        &mut menu,
        &mut inventory,
        &mut villager,
        &registry,
        Uuid::nil(),
        0,
        ClickMode::Pickup,
    );
    assert_eq!(outcome.trades, 1);
    assert!(outcome.experience > 0);
    assert_eq!(menu.grid[0].count, 8);
    assert_eq!(menu.grid[1].count, 6);
    assert_eq!(menu.carried, villager.offers[0].result);
    assert_eq!(villager.xp, 5);
    assert_eq!(villager.offers[0].uses, 1);
}

#[test]
fn trade_insufficient_second_cost_debits_nothing() {
    let registry = registry();
    let (mut v, mut m, mut i) = fixture(&registry);
    m.grid[1].count = 2;
    refresh(&mut m, &v, &registry.item_rules(), Uuid::nil());
    let before = (v.clone(), m.clone(), i.clone());
    assert_eq!(
        execute(
            &mut m,
            &mut i,
            &mut v,
            &registry,
            Uuid::nil(),
            0,
            ClickMode::QuickMove
        )
        .trades,
        0
    );
    assert_eq!((v, m, i), before);
}

#[test]
fn trade_shift_click_stops_at_max_uses_and_replay_cannot_duplicate() {
    let registry = registry();
    let (mut v, mut m, mut i) = fixture(&registry);
    let result = v.offers[0].result.clone();
    assert_eq!(
        execute(
            &mut m,
            &mut i,
            &mut v,
            &registry,
            Uuid::nil(),
            0,
            ClickMode::QuickMove
        )
        .trades,
        2
    );
    assert_eq!(v.offers[0].uses, 2);
    assert_eq!(m.grid[0].count, 6);
    assert_eq!(m.grid[1].count, 3);
    assert_eq!(
        i.slots
            .iter()
            .filter(|s| can_stack(s, &result))
            .map(|s| s.count)
            .sum::<i32>(),
        8
    );
    let before = (v.clone(), m.clone(), i.clone());
    assert_eq!(
        execute(
            &mut m,
            &mut i,
            &mut v,
            &registry,
            Uuid::nil(),
            0,
            ClickMode::QuickMove
        )
        .trades,
        0
    );
    assert_eq!((v, m, i), before);
}

#[test]
fn trade_shift_click_full_inventory_never_partially_delivers_or_debits() {
    let registry = registry();
    let (mut v, mut m, mut i) = fixture(&registry);
    for slot in &mut i.slots[..36] {
        *slot = item(&registry, "stone", 64);
    }
    i.slots[0] = v.offers[0].result.clone();
    i.slots[0].count = 62;
    let before = (v.clone(), m.clone(), i.clone());
    assert_eq!(
        execute(
            &mut m,
            &mut i,
            &mut v,
            &registry,
            Uuid::nil(),
            0,
            ClickMode::QuickMove
        )
        .trades,
        0
    );
    assert_eq!((v, m, i), before);
}

#[test]
fn trade_swapped_inputs_consume_exact_payment() {
    let registry = registry();
    let (mut v, mut m, mut i) = fixture(&registry);
    m.grid.swap(0, 1);
    assert_eq!(
        execute(
            &mut m,
            &mut i,
            &mut v,
            &registry,
            Uuid::nil(),
            0,
            ClickMode::Pickup
        )
        .trades,
        1
    );
    assert_eq!(m.grid[0].count, 6);
    assert_eq!(m.grid[1].count, 8);
}

#[test]
fn trade_restock_requires_job_and_respects_cooldown_two_per_day_and_demand() {
    let registry = registry();
    let (mut v, _, _) = fixture(&registry);
    v.offers[0].uses = 2;
    work(&mut v, &registry, 2_000, 2_000);
    assert_eq!(v.offers[0].uses, 2);
    v.job_site = Some(crate::boundary::BlockPos { x: 0, y: 64, z: 0 });
    work(&mut v, &registry, 2_000, 2_000);
    assert_eq!(v.offers[0].uses, 0);
    assert_eq!(v.offers[0].demand, 2);
    v.offers[0].uses = 2;
    work(&mut v, &registry, 4_400, 4_400);
    assert_eq!(v.offers[0].uses, 2);
    work(&mut v, &registry, 4_401, 4_401);
    assert_eq!(v.offers[0].uses, 0);
    v.offers[0].uses = 1;
    work(&mut v, &registry, 5_000, 5_000);
    assert_eq!(v.offers[0].uses, 1);
    work(&mut v, &registry, 24_001, 24_001);
    assert_eq!(v.offers[0].uses, 0);
    assert_eq!(v.restocks_today, 1);
    assert_eq!(v.offers[0].demand, 4);
}

#[test]
fn trade_restock_new_day_resets_budget_without_carrying_previous_day_cooldown() {
    let registry = registry();
    let (mut v, _, _) = fixture(&registry);
    v.job_site = Some(crate::boundary::BlockPos { x: 0, y: 64, z: 0 });
    v.offers[0].uses = 1;
    v.restocks_today = 2;
    v.last_restock = 23_999;
    v.last_restock_day = 0;
    work(&mut v, &registry, 24_000, 24_000);
    assert_eq!(v.offers[0].uses, 0);
    assert_eq!(v.restocks_today, 1);
    assert_eq!(v.last_restock, 24_000);
    assert_eq!(v.last_restock_day, 1);
}

#[test]
fn trade_levelling_preserves_used_offers_and_unlocks_new_level() {
    let registry = registry();
    let (mut v, mut m, mut i) = fixture(&registry);
    v.xp = 5;
    assert_eq!(
        execute(
            &mut m,
            &mut i,
            &mut v,
            &registry,
            Uuid::nil(),
            0,
            ClickMode::Pickup
        )
        .trades,
        1
    );
    assert_eq!(v.data.level, 2);
    assert_eq!(v.xp, 10);
    assert_eq!(v.offers[0].uses, 1);
    assert_eq!(v.offers.len(), 3);
    assert_eq!(v.gossips[0].value, 2);
}

#[test]
fn trade_generation_covers_every_old_profession_at_each_level() {
    let registry = registry();
    for profession in VillagerProfession::ALL {
        for level in 1..=5 {
            let mut v = VillagerState::default();
            v.data.profession = profession;
            v.data.level = level;
            generate(&mut v, &registry);
            if matches!(
                profession,
                VillagerProfession::None | VillagerProfession::Nitwit
            ) {
                assert!(v.offers.is_empty());
            } else {
                assert!(!v.offers.is_empty(), "{profession:?} level {level}");
                assert!(v.offers.len() <= 10);
                assert!(v.offers.iter().all(VillagerTradeOffer::valid));
            }
            let before = v.offers.clone();
            generate(&mut v, &registry);
            assert_eq!(v.offers, before);
        }
    }
    let mut baby = VillagerState {
        age: -1,
        data: super::super::VillagerData {
            profession: VillagerProfession::Farmer,
            ..Default::default()
        },
        ..Default::default()
    };
    generate(&mut baby, &registry);
    assert!(baby.offers.is_empty());
}

#[test]
fn trade_prices_follow_demand_special_reputation_stack_clamp_order() {
    let registry = registry();
    let (mut v, _, _) = fixture(&registry);
    let offer = &mut v.offers[0];
    offer.cost_a.count = 15;
    offer.price_multiplier = 0.05;
    offer.demand = 20;
    offer.special_price = -2;
    assert_eq!(offer.price(&registry.item_rules(), 20), 27);
    offer.demand = -100;
    assert_eq!(offer.price(&registry.item_rules(), 1000), 1);
    offer.demand = i32::MAX;
    assert_eq!(offer.price(&registry.item_rules(), 0), 64);
}

#[test]
fn trade_close_returns_only_real_custody_never_projected_result() {
    let registry = registry();
    let (v, mut m, mut i) = fixture(&registry);
    refresh(&mut m, &v, &registry.item_rules(), Uuid::nil());
    assert!(!m.result.is_empty());
    assert!(m.close(&registry.item_rules(), &mut i).is_empty());
    assert_eq!(i.slots.iter().map(|s| s.count).sum::<i32>(), 19);
    assert!(m.close(&registry.item_rules(), &mut i).is_empty());
    assert_eq!(i.slots.iter().map(|s| s.count).sum::<i32>(), 19);
}

#[test]
fn trade_select_moves_existing_items_and_generic_result_click_cannot_mint() {
    let registry = registry();
    let (v, mut m, mut i) = fixture(&registry);
    select(&mut m, &mut i, &v, &registry.item_rules(), Uuid::nil(), 0);
    assert_eq!(m.grid[0].count, 10);
    assert_eq!(m.grid[1].count, 9);
    assert!(!m.result.is_empty());
    for mode in [
        ClickMode::Pickup,
        ClickMode::QuickMove,
        ClickMode::Swap,
        ClickMode::Throw,
        ClickMode::PickupAll,
    ] {
        m.click(
            &registry.item_rules(),
            &mut i,
            &mut [],
            m.state_id,
            2,
            0,
            mode,
            false,
        );
        assert!(m.carried.is_empty());
        assert_eq!(i.slots.iter().map(|s| s.count).sum::<i32>(), 0);
        assert_eq!(m.grid[0].count, 10);
        assert_eq!(m.grid[1].count, 9);
    }
}

#[test]
fn trade_offers_nbt_roundtrip_preserves_counters_prices_and_components() {
    let registry = registry();
    let (mut v, _, _) = fixture(&registry);
    v.xp = 81;
    v.data.level = 3;
    v.offers[0].uses = 1;
    v.offers[0].demand = 7;
    v.offers[0].special_price = -2;
    v.restocks_today = 2;
    v.last_restock = 5_000;
    let fields =
        crate::villager::persistence::villager_to_fields(&registry.items, &v, 5_050).unwrap();
    assert!(fields.iter().any(|(name, _)| name == "Offers"));
    let restored =
        crate::villager::persistence::villager_from_fields(&registry.items, &fields, 5_050)
            .unwrap();
    assert_eq!(restored.offers, v.offers);
    assert_eq!(restored.xp, v.xp);
    assert_eq!(restored.data, v.data);
    assert_eq!(restored.last_restock, v.last_restock);
}

#[test]
fn select_refills_payment_returned_to_offhand_without_duplication() {
    let registry = registry();
    let (mut villager, mut menu, mut inventory) = fixture(&registry);
    villager.offers[0].cost_b = None;
    menu.grid.fill(ItemStack::EMPTY);
    for stack in &mut inventory.slots[..36] {
        *stack = item(&registry, "stone", 64);
    }
    inventory.slots[crate::inventory::OFF_HAND] = item(&registry, "emerald", 10);
    for _ in 0..2 {
        select(
            &mut menu,
            &mut inventory,
            &villager,
            &registry.item_rules(),
            Uuid::nil(),
            0,
        );
        assert_eq!(menu.grid[0], item(&registry, "emerald", 10));
        assert!(inventory.slots[crate::inventory::OFF_HAND].is_empty());
        assert_eq!(menu.result, villager.offers[0].result);
        assert!(
            inventory.slots[..36]
                .iter()
                .all(|stack| *stack == item(&registry, "stone", 64))
        );
    }
    // Vanilla close tops up a matching offhand stack, but never chooses an
    // empty offhand slot. Move one real payment there before returning the rest.
    menu.grid[0].count -= 1;
    inventory.slots[crate::inventory::OFF_HAND] = item(&registry, "emerald", 1);
    assert!(
        menu.close(&registry.item_rules(), &mut inventory)
            .is_empty()
    );
    assert_eq!(
        inventory.slots[crate::inventory::OFF_HAND],
        item(&registry, "emerald", 10)
    );
    assert!(
        menu.close(&registry.item_rules(), &mut inventory)
            .is_empty()
    );
    assert_eq!(inventory.slots[crate::inventory::OFF_HAND].count, 10);
}

#[test]
fn work_initializes_once_and_preserves_live_used_offer_restocking() {
    let registry = registry();
    let mut v = VillagerState {
        job_site: Some(crate::boundary::BlockPos { x: 0, y: 64, z: 0 }),
        ..Default::default()
    };
    v.data.profession = VillagerProfession::Farmer;
    INITIALIZATION_ATTEMPTS.set(0);
    work(&mut v, &registry, 2_000, 2_000);
    assert!(!v.offers.is_empty());
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 1);
    let offers = v.offers.clone();
    for tick in 2_001..2_010 {
        work(&mut v, &registry, tick, tick as i64);
    }
    assert_eq!(v.offers, offers);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 1);
    assert_eq!(v.last_work, 2_009);
    assert_eq!(v.restocks_today, 0);
    // Even an otherwise unchanged work call must observe authoritative uses.
    v.offers[0].uses = 1;
    work(&mut v, &registry, 2_010, 2_010);
    assert_eq!(v.offers[0].uses, 0);
    assert_eq!(v.restocks_today, 1);
    assert_eq!(v.last_restock, 2_010);
    v.offers[0].uses = 1;
    work(&mut v, &registry, 4_410, 4_410);
    assert_eq!(v.offers[0].uses, 1);
    assert_eq!(v.last_work, 4_410);
    work(&mut v, &registry, 4_411, 4_411);
    assert_eq!(v.offers[0].uses, 0);
    assert_eq!(v.restocks_today, 2);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 1);
    // Clearing successful offers is not mistaken for a previous failed attempt,
    // even when today's restock budget has already been exhausted.
    v.offers.clear();
    work(&mut v, &registry, 4_412, 4_412);
    assert!(!v.offers.is_empty());
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 2);
    assert_eq!(v.restocks_today, 2);
    assert_eq!(v.last_work, 4_412);
}

#[test]
fn work_memoizes_only_unchanged_empty_initialization_attempts() {
    let mut registry = registry();
    registry.items = std::sync::Arc::new(mc_data::items::ItemRegistry::default());
    let mut v = VillagerState {
        job_site: Some(crate::boundary::BlockPos { x: 0, y: 64, z: 0 }),
        ..Default::default()
    };
    v.data.profession = VillagerProfession::Farmer;
    INITIALIZATION_ATTEMPTS.set(0);
    for tick in 2_000..2_010 {
        work(&mut v, &registry, tick, tick as i64);
    }
    assert!(v.offers.is_empty());
    assert!(v.trade_work.is_some());
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 1);
    assert_eq!(v.last_work, 2_009);
    assert_eq!(v.restocks_today, 0);
    work(&mut v, &registry, 24_000, 24_000);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 2);
    assert_eq!(v.last_restock_day, 1);
    v.data.profession = VillagerProfession::Fletcher;
    work(&mut v, &registry, 24_001, 24_001);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 3);
    v.data.level = 2;
    work(&mut v, &registry, 24_002, 24_002);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 4);
    v.wandering = true;
    work(&mut v, &registry, 24_003, 24_003);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 5);
    v.despawn_at = Some(30_000);
    work(&mut v, &registry, 24_004, 24_004);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 6);
    // Registry identity, not just profession/level/day, owns resolved item IDs.
    registry.items = std::sync::Arc::new(mc_data::items::ItemRegistry::default());
    work(&mut v, &registry, 24_005, 24_005);
    work(&mut v, &registry, 24_006, 24_006);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 7);
    assert!(v.offers.is_empty());
}

#[test]
fn merchant_initialization_bypasses_failed_work_and_changed_items_retry() {
    let registry = registry();
    let mut empty = registry.clone();
    empty.items = std::sync::Arc::new(mc_data::items::ItemRegistry::default());
    let mut v = VillagerState::default();
    v.data.profession = VillagerProfession::Farmer;
    INITIALIZATION_ATTEMPTS.set(0);
    work(&mut v, &empty, 2_000, 2_000);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 1);
    assert!(v.trade_work.is_some());
    assert_eq!(v.last_work, 0, "initialization needs no job; work does");
    // The merchant-open caller uses generate, not the failed-work memo.
    generate(&mut v, &empty);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 2);
    assert!(v.trade_work.is_none());
    work(&mut v, &empty, 2_001, 2_001);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 3);
    work(&mut v, &registry, 2_002, 2_002);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 4);
    assert!(!v.offers.is_empty());
    assert!(v.trade_work.is_none());
    let offers = v.offers.clone();
    generate(&mut v, &registry);
    assert_eq!(v.offers, offers);
}

#[test]
fn empty_work_memo_survives_noai_resume_but_not_durable_projection() {
    let mut registry = registry();
    registry.items = std::sync::Arc::new(mc_data::items::ItemRegistry::default());
    let mut v = VillagerState {
        job_site: Some(crate::boundary::BlockPos { x: 0, y: 64, z: 0 }),
        ..Default::default()
    };
    v.data.profession = VillagerProfession::Farmer;
    INITIALIZATION_ATTEMPTS.set(0);
    work(&mut v, &registry, 2_000, 2_000);
    v.brain_clock = false;
    crate::villager::persistence::advance_clocks(&mut v, 2_001, 3_000);
    assert_eq!(v.last_work, 2_000);
    v.brain_clock = true;
    work(&mut v, &registry, 3_000, 3_000);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 1);
    assert_eq!(v.last_work, 3_000);
    v.sleeping = true;
    work(&mut v, &registry, 24_000, 24_000);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 1);
    assert_eq!(v.last_work, 3_000);
    v.sleeping = false;
    work(&mut v, &registry, 24_001, 24_001);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 2);
    assert_eq!(v.last_work, 24_001);
    let mut loaded = crate::villager::persistence::journal_image(&v);
    assert!(loaded.trade_work.is_none());
    assert!(crate::villager::persistence::journal_eq(&v, &loaded));
    work(&mut loaded, &registry, 24_002, 24_002);
    assert_eq!(INITIALIZATION_ATTEMPTS.get(), 3);
    assert_eq!(loaded.last_work, 24_002);
    assert_eq!(loaded.restocks_today, 0);
}
