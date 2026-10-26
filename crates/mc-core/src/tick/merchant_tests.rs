use super::*;
use crate::boundary::{Hand, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::villager::{VillagerProfession, VillagerTradeOffer};
use crate::world::ChunkWorld;
use mc_data::{Identifier, ItemStack};

fn fixture() -> (TickState, EntityId, EntityId) {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().unwrap();
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [spawn.block_x, spawn.surface_y + 1, spawn.block_z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::nil(),
            name: "Trader".into(),
            permission_level: 0,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let player = state.connections.entity_of(ConnectionId(1)).unwrap();
    let position = state.entities.pos(state.entities.slot_of(player).unwrap());
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let merchant = state.spawn_mob(type_id, [position[0] + 2.0, position[1], position[2]], 20.0);
    let emerald = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:emerald").unwrap())
        .unwrap();
    let bread = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:bread").unwrap())
        .unwrap();
    let job = crate::boundary::BlockPos {
        x: position[0].floor() as i32 + 3,
        y: position[1].floor() as i32,
        z: position[2].floor() as i32,
    };
    let composter = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:composter").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(job.x, job.y, job.z, composter);
    state.world.fold_overlay();
    let slot = state.entities.slot_of(merchant).unwrap();
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.data.profession = VillagerProfession::Farmer;
    v.job_site = Some(job);
    v.offers = vec![VillagerTradeOffer {
        cost_a: ItemStack::new(emerald, 2),
        cost_b: None,
        result: ItemStack::new(bread, 3),
        uses: 0,
        max_uses: 2,
        reward_exp: true,
        xp: 1,
        special_price: 0,
        demand: 0,
        price_multiplier: 0.05,
    }];
    state.players.get_mut(&player).unwrap().inventory.slots[0] = ItemStack::new(emerald, 10);
    (state, player, merchant)
}

fn interact(state: &mut TickState, merchant: EntityId) {
    run_tick(
        state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: merchant,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
}
fn offer_uses(state: &TickState, merchant: EntityId) -> i32 {
    state
        .entities
        .row(state.entities.slot_of(merchant).unwrap())
        .villager
        .as_ref()
        .unwrap()
        .offers[0]
        .uses
}

#[test]
fn leveling_trade_grants_merchant_regeneration() {
    // VILLAGER-LEVEL-REGENERATION through the existing menu click path: a
    // level-up trade must leave Regeneration on the merchant entity row.
    let (mut state, player, merchant) = fixture();
    // Near-level XP so one trade levels the novice up.
    let slot = state.entities.slot_of(merchant).unwrap();
    state.entities.row_mut(slot).villager.as_mut().unwrap().xp = 9;
    interact(&mut state, merchant);
    run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 0,
        }],
    );
    let menu = state.players[&player].open.as_ref().unwrap();
    assert!(!menu.result.is_empty());
    let window = menu.window;
    let state_id = menu.state_id;
    run_tick(
        &mut state,
        &[PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window,
            state_id,
            slot: 2,
            button: 0,
            mode: ClickMode::Pickup,
        }],
    );
    let slot = state.entities.slot_of(merchant).unwrap();
    let level = state.entities.row(slot).villager.as_ref().unwrap().data.level;
    assert!(level >= 2, "trade must level the villager, got {level}");
    assert!(
        state.entities.row(slot).active_effects.has(crate::effects::EffectId::Regeneration),
        "level-up trade must grant the merchant Regeneration"
    );
}

#[test]
fn auth_gate_locked_trade_preserves_payment_until_unlock() {
    let (mut state, player, merchant) = fixture();
    interact(&mut state, merchant);
    state.require_plugin_login = true;
    state.locked_sessions.insert(ConnectionId(1));
    let before = state.players[&player].clone();
    run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 0,
        }],
    );
    assert_eq!(state.players[&player].inventory, before.inventory);
    assert_eq!(state.players[&player].open, before.open);
    assert_eq!(offer_uses(&state, merchant), 0);
    state.locked_sessions.remove(&ConnectionId(1));
    run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 0,
        }],
    );
    let menu = state.players[&player].open.as_ref().unwrap();
    assert!(!menu.result.is_empty());
    let click = PlayerInput::ContainerClick {
        connection: ConnectionId(1),
        window: menu.window,
        state_id: menu.state_id,
        slot: 2,
        button: 0,
        mode: ClickMode::Pickup,
    };
    run_tick(&mut state, &[click]);
    assert_eq!(offer_uses(&state, merchant), 1);
}

#[test]
fn merchant_titles_use_vanilla_look_keys_and_literal_custom_names() {
    let (mut state, _, merchant) = fixture();
    let slot = state.entities.slot_of(merchant).unwrap();
    for profession in VillagerProfession::ALL {
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .data
            .profession = profession;
        let villager = state.entities.row(slot).villager.as_ref().unwrap();
        assert_eq!(
            crate::scoreboard::component(&menu_title(&state, slot, villager)),
            mc_nbt::Tag::Compound(vec![(
                "translate".into(),
                mc_nbt::Tag::String(format!(
                    "entity.minecraft.villager.{}",
                    profession.title_name()
                )),
            )]),
        );
    }
    assert_eq!(VillagerProfession::Miner.title_name(), "toolsmith");
    assert_eq!(VillagerProfession::Lumberjack.title_name(), "fletcher");
    let name = r#"{"translate":"not.a.profession"}"#;
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name = Some(name.into());
    let villager = state.entities.row(slot).villager.as_ref().unwrap();
    assert_eq!(
        crate::scoreboard::component(&menu_title(&state, slot, villager)),
        mc_nbt::Tag::String(name.into()),
    );
}

#[test]
fn merchant_stale_clicks_each_execute_against_current_payment_and_award_orbs_at_merchant() {
    let (mut state, player, merchant) = fixture();
    interact(&mut state, merchant);
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(mc_data::statistics::CustomStat::TalkedToVillager),
        1
    );
    run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 0,
        }],
    );
    let menu = state.players[&player].open.as_ref().unwrap();
    let click = PlayerInput::ContainerClick {
        connection: ConnectionId(1),
        window: menu.window,
        state_id: menu.state_id,
        slot: 2,
        button: 0,
        mode: ClickMode::Pickup,
    };
    let (out, _, _) = run_tick(&mut state, &[click.clone(), click]);
    assert_eq!(offer_uses(&state, merchant), 2);
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(mc_data::statistics::CustomStat::TradedWithVillager),
        2
    );
    let bread = state.players[&player]
        .open
        .as_ref()
        .unwrap()
        .carried
        .item_id;
    assert_eq!(
        state.players[&player]
            .statistics
            .get(mc_data::statistics::StatType::Crafted, bread),
        6
    );
    assert_eq!(
        state.players[&player].open.as_ref().unwrap().carried.count,
        6
    );
    assert_eq!(
        state.players[&player].open.as_ref().unwrap().grid[0].count,
        6
    );
    assert!(out.changes.entities_spawned.iter().any(|id| {
        state
            .entities
            .slot_of(*id)
            .is_some_and(|slot| state.entities.row(slot).appearance.experience_orb.is_some())
    }));
}

#[test]
fn merchant_close_reopen_keeps_uses_without_work_and_permitted_work_restocks_the_entity() {
    for work_hours in [false, true] {
        let (mut state, player, merchant) = fixture();
        state.tick = 1_000;
        state.day_time = if work_hours { 6_000 } else { 16_000 };
        interact(&mut state, merchant);
        run_tick(
            &mut state,
            &[PlayerInput::SelectTrade {
                connection: ConnectionId(1),
                offer: 0,
            }],
        );
        let menu = state.players[&player].open.as_ref().unwrap();
        let window = menu.window;
        let click = PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window,
            state_id: menu.state_id,
            slot: 2,
            button: 0,
            mode: ClickMode::Pickup,
        };
        run_tick(&mut state, &[click]);
        assert_eq!(offer_uses(&state, merchant), 1);
        let slot = state.entities.slot_of(merchant).unwrap();
        let before = state.entities.row(slot).villager.as_ref().unwrap().clone();
        let close_tick = state.tick;
        run_tick(
            &mut state,
            &[PlayerInput::ContainerClose {
                connection: ConnectionId(1),
                window,
            }],
        );
        assert!(state.players[&player].open.is_none());
        let (_, reliable, _) = run_tick(
            &mut state,
            &[PlayerInput::InteractEntity {
                connection: ConnectionId(1),
                target: merchant,
                hand: Hand::Main,
                location: [0.0; 3],
            }],
        );
        assert_eq!(
            state.players[&player]
                .open
                .as_ref()
                .unwrap()
                .merchant
                .unwrap()
                .entity,
            merchant
        );
        let villager = state.entities.row(slot).villager.as_ref().unwrap();
        let expected_uses = i32::from(!work_hours);
        assert_eq!(villager.offers[0].uses, expected_uses);
        assert_eq!(villager.offers.len(), before.offers.len());
        assert_eq!(villager.offers[0].cost_a, before.offers[0].cost_a);
        assert_eq!(villager.offers[0].result, before.offers[0].result);
        if work_hours {
            assert_eq!(villager.restocks_today, 1);
            assert_eq!(villager.last_restock, close_tick as i64);
        } else {
            assert_eq!(villager.offers, before.offers);
            assert_eq!(villager.restocks_today, before.restocks_today);
            assert_eq!(villager.last_restock, before.last_restock);
        }
        assert!(reliable.into_iter().flat_map(|(_, changes)| changes).any(|change| {
            matches!(change, PlayerChange::MerchantOffers { offers, .. } if offers[0].uses == expected_uses)
        }));
    }
}

#[test]
fn merchant_close_disconnect_and_reconnect_return_payment_once_without_result_duplication() {
    for disconnect in [false, true] {
        let (mut state, player, merchant) = fixture();
        interact(&mut state, merchant);
        run_tick(
            &mut state,
            &[PlayerInput::SelectTrade {
                connection: ConnectionId(1),
                offer: 0,
            }],
        );
        let window = state.players[&player].open.as_ref().unwrap().window;
        if disconnect {
            run_tick(
                &mut state,
                &[PlayerInput::Leave {
                    connection: ConnectionId(1),
                }],
            );
            run_tick(
                &mut state,
                &[PlayerInput::Join {
                    connection: ConnectionId(1),
                    uuid: uuid::Uuid::nil(),
                    name: "Trader".into(),
                    permission_level: 0,
                }],
            );
        } else {
            run_tick(
                &mut state,
                &[PlayerInput::ContainerClose {
                    connection: ConnectionId(1),
                    window,
                }],
            );
            run_tick(
                &mut state,
                &[PlayerInput::ContainerClose {
                    connection: ConnectionId(1),
                    window,
                }],
            );
        }
        let player = state.connections.entity_of(ConnectionId(1)).unwrap();
        assert_eq!(
            state.players[&player]
                .inventory
                .slots
                .iter()
                .map(|s| s.count)
                .sum::<i32>(),
            10
        );
        assert!(state.players[&player].open.is_none());
        assert_eq!(offer_uses(&state, merchant), 0);
    }
}

#[test]
fn merchant_open_denies_baby_none_nitwit_and_second_customer_and_closes_out_of_reach() {
    let (mut state, player, merchant) = fixture();
    let slot = state.entities.slot_of(merchant).unwrap();
    for profession in [VillagerProfession::None, VillagerProfession::Nitwit] {
        state
            .entities
            .row_mut(slot)
            .villager
            .as_mut()
            .unwrap()
            .data
            .profession = profession;
        interact(&mut state, merchant);
        assert!(state.players[&player].open.is_none());
    }
    let v = state.entities.row_mut(slot).villager.as_mut().unwrap();
    v.data.profession = VillagerProfession::Farmer;
    v.age = -100;
    interact(&mut state, merchant);
    assert!(state.players[&player].open.is_none());
    state.entities.row_mut(slot).villager.as_mut().unwrap().age = 0;
    interact(&mut state, merchant);
    assert!(state.players[&player].open.is_some());
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(2),
            uuid: uuid::Uuid::from_u128(2),
            name: "Second".into(),
            permission_level: 0,
        }],
    );
    run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(2),
            target: merchant,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    let second = state.connections.entity_of(ConnectionId(2)).unwrap();
    assert!(state.players[&second].open.is_none());
    state.entities.set_pos(slot, [10_000.0, 64.0, 10_000.0]);
    run_tick(&mut state, &[]);
    assert!(state.players[&player].open.is_none());
}

#[test]
fn merchant_restock_pushes_fresh_offers_even_with_empty_payment_slots() {
    let (mut state, player, merchant) = fixture();
    interact(&mut state, merchant);
    let slot = state.entities.slot_of(merchant).unwrap();
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .last_restock = 999;
    let mut changes = ChangeSet::default();
    maintain(&mut state, &mut changes);
    run_tick(&mut state, &[]);
    assert_eq!(
        state.players[&player]
            .open
            .as_ref()
            .unwrap()
            .merchant
            .unwrap()
            .last_restock,
        999
    );
}

#[test]
fn filtered_offer_display_indexes_trade_the_original_a_and_b_custody() {
    let (mut state, player, merchant) = fixture();
    let slot = state.entities.slot_of(merchant).unwrap();
    let a = state.entities.row(slot).villager.as_ref().unwrap().offers[0].clone();
    let emerald = a.cost_a.item_id;
    let bread = a.result.item_id;
    let apple = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let mut unsupported = a.clone();
    unsupported.cost_a = unsupported
        .cost_a
        .with_custom_name("Exact imported predicate");
    let mut b = a.clone();
    b.cost_a.count = 3;
    b.result = ItemStack::new(apple, 1);
    b.max_uses = 1;
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .offers = vec![unsupported, a, b];
    let payment = |state: &TickState| {
        let p = &state.players[&player];
        p.inventory
            .slots
            .iter()
            .chain(p.open.as_ref().unwrap().grid.iter())
            .filter(|stack| !stack.is_empty() && stack.item_id == emerald)
            .map(|stack| stack.count)
            .sum::<i32>()
    };
    let (_, outbox, _) = run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: merchant,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    let displayed = outbox
        .iter()
        .flat_map(|(_, changes)| changes)
        .find_map(|change| match change {
            PlayerChange::MerchantOffers { offers, .. } => Some(offers),
            _ => None,
        })
        .unwrap();
    assert_eq!(displayed.len(), 2);
    assert_eq!(displayed[0].result.item_id, bread);
    assert_eq!(displayed[1].result.item_id, apple);
    assert_eq!(
        state.players[&player]
            .open
            .as_ref()
            .unwrap()
            .merchant
            .unwrap()
            .selected,
        1
    );
    assert_eq!(payment(&state), 10);
    run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 0,
        }],
    );
    let menu = state.players[&player].open.as_ref().unwrap();
    let click = PlayerInput::ContainerClick {
        connection: ConnectionId(1),
        window: menu.window,
        state_id: menu.state_id,
        slot: 2,
        button: 0,
        mode: ClickMode::Pickup,
    };
    run_tick(&mut state, &[click]);
    assert_eq!(payment(&state), 8);
    let menu = state.players[&player].open.as_ref().unwrap();
    assert_eq!(menu.carried, ItemStack::new(bread, 3));
    let offers = &state.entities.row(slot).villager.as_ref().unwrap().offers;
    assert_eq!(
        offers.iter().map(|offer| offer.uses).collect::<Vec<_>>(),
        [0, 1, 0]
    );
    run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 1,
        }],
    );
    let menu = state.players[&player].open.as_ref().unwrap();
    assert_eq!(menu.merchant.unwrap().selected, 2);
    let click = PlayerInput::ContainerClick {
        connection: ConnectionId(1),
        window: menu.window,
        state_id: menu.state_id,
        slot: 2,
        button: 0,
        mode: ClickMode::QuickMove,
    };
    run_tick(&mut state, &[click]);
    assert_eq!(payment(&state), 5);
    assert_eq!(
        state.players[&player]
            .inventory
            .slots
            .iter()
            .filter(|stack| !stack.is_empty() && stack.item_id == apple)
            .map(|stack| stack.count)
            .sum::<i32>(),
        1
    );
    assert_eq!(
        state.players[&player].open.as_ref().unwrap().carried,
        ItemStack::new(bread, 3)
    );
    let offers = &state.entities.row(slot).villager.as_ref().unwrap().offers;
    assert_eq!(
        offers.iter().map(|offer| offer.uses).collect::<Vec<_>>(),
        [0, 1, 1]
    );
    assert_eq!(
        offers[0].cost_a.custom_name.as_deref(),
        Some("Exact imported predicate")
    );
    run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 2,
        }],
    );
    assert_eq!(
        state.players[&player]
            .open
            .as_ref()
            .unwrap()
            .merchant
            .unwrap()
            .selected,
        2
    );
}

#[test]
fn merchant_with_only_retained_predicate_offers_has_no_selectable_or_tradeable_wire_offer() {
    let (mut state, player, merchant) = fixture();
    let slot = state.entities.slot_of(merchant).unwrap();
    let offer = &mut state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .offers[0];
    offer.cost_a = offer
        .cost_a
        .clone()
        .with_custom_name("Imported exact payment");
    let retained = offer.clone();
    let mut payment = retained.cost_a.clone();
    payment.count = 10;
    state.players.get_mut(&player).unwrap().inventory.slots[0] = payment;
    let (_, outbox, _) = run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: merchant,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    assert!(
        outbox
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(
                change, PlayerChange::MerchantOffers { offers, .. } if offers.is_empty()
            ))
    );
    let menu = state.players[&player].open.as_ref().unwrap();
    assert_eq!(menu.merchant.unwrap().selected, 1);
    assert!(menu.result.is_empty());
    run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 0,
        }],
    );
    let p = state.players.get_mut(&player).unwrap();
    p.open.as_mut().unwrap().grid[0] = std::mem::take(&mut p.inventory.slots[0]);
    for mode in [ClickMode::Pickup, ClickMode::QuickMove] {
        let menu = state.players[&player].open.as_ref().unwrap();
        let click = PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window: menu.window,
            state_id: menu.state_id,
            slot: 2,
            button: 0,
            mode,
        };
        run_tick(&mut state, &[click]);
        let menu = state.players[&player].open.as_ref().unwrap();
        assert_eq!(menu.merchant.unwrap().selected, 1);
        assert!(menu.result.is_empty());
        assert!(menu.carried.is_empty());
        assert_eq!(menu.grid[0].count, 10);
        assert_eq!(offer_uses(&state, merchant), 0);
    }
    let window = state.players[&player].open.as_ref().unwrap().window;
    run_tick(
        &mut state,
        &[PlayerInput::ContainerClose {
            connection: ConnectionId(1),
            window,
        }],
    );
    assert_eq!(
        state.players[&player]
            .inventory
            .slots
            .iter()
            .map(|s| s.count)
            .sum::<i32>(),
        10
    );
    let stored = &state
        .entities
        .row(state.entities.slot_of(merchant).unwrap())
        .villager
        .as_ref()
        .unwrap()
        .offers;
    assert_eq!(stored, &[retained]);
}

#[test]
fn raids_hero_discount_matches_wire_offer_atomic_payment_and_expiration() {
    let (mut state, player, merchant) = fixture();
    state
        .players
        .get_mut(&player)
        .unwrap()
        .give_effect(crate::effects::ActiveEffect::new(
            crate::effects::EffectId::HeroOfTheVillage,
            48000,
            0,
        ));
    interact(&mut state, merchant);
    assert_eq!(
        state.players[&player]
            .open
            .as_ref()
            .unwrap()
            .merchant
            .unwrap()
            .hero,
        Some(0)
    );
    let (_, outbox, _) = run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 0,
        }],
    );
    let offers = outbox
        .iter()
        .flat_map(|(_, changes)| changes)
        .filter_map(|change| match change {
            PlayerChange::MerchantOffers { offers, .. } => Some(offers),
            _ => None,
        })
        .next()
        .unwrap();
    assert_eq!(offers[0].special_price, -1);
    let menu = state.players[&player].open.as_ref().unwrap();
    let before = menu.grid[0].count;
    let click = PlayerInput::ContainerClick {
        connection: ConnectionId(1),
        window: menu.window,
        state_id: menu.state_id,
        slot: 2,
        button: 0,
        mode: ClickMode::Pickup,
    };
    run_tick(&mut state, &[click]);
    assert_eq!(
        state.players[&player].open.as_ref().unwrap().grid[0].count,
        before - 1
    );
    assert_eq!(offer_uses(&state, merchant), 1);
    state
        .players
        .get_mut(&player)
        .unwrap()
        .clear_effects(Some(crate::effects::EffectId::HeroOfTheVillage));
    run_tick(&mut state, &[]);
    assert_eq!(
        state.players[&player]
            .open
            .as_ref()
            .unwrap()
            .merchant
            .unwrap()
            .hero,
        None
    );
}

#[test]
fn wandering_trader_opens_native_merchant_with_vanilla_offers() {
    let (mut state, player, resident) = fixture();
    let pos = state
        .entities
        .pos(state.entities.slot_of(resident).unwrap());
    state.remove_entity(state.entities.slot_of(resident).unwrap());
    let kind = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:wandering_trader").unwrap())
        .unwrap();
    let trader = state.spawn_mob(kind, pos, 20.0);
    let slot = state.entities.slot_of(trader).unwrap();
    let original = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .offers
        .clone();
    assert_eq!(original.len(), 9);
    assert!(original.iter().all(VillagerTradeOffer::wire_supported));
    let (_, reliable, _) = run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: trader,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    assert_eq!(
        state.players[&player].open.as_ref().unwrap().kind,
        MenuKind::Merchant
    );
    assert!(reliable
        .into_iter()
        .flat_map(|(_, changes)| changes)
        .any(|event| matches!(
            event,
            PlayerChange::MerchantOffers { offers, can_restock: false, .. } if offers == original
        )));
    assert_eq!(
        state.entities.row(slot).villager.as_ref().unwrap().offers,
        original
    );
}

#[test]
fn merchant_rejected_interaction_and_unpaid_trade_award_no_statistics() {
    use mc_data::statistics::CustomStat;
    let (mut state, player, merchant) = fixture();
    let slot = state.entities.slot_of(merchant).unwrap();
    state.entities.row_mut(slot).villager.as_mut().unwrap().age = -1;
    interact(&mut state, merchant);
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(CustomStat::TalkedToVillager),
        0
    );
    state.entities.row_mut(slot).villager.as_mut().unwrap().age = 0;
    interact(&mut state, merchant);
    let menu = state.players[&player].open.as_ref().unwrap();
    let (window, revision) = (menu.window, menu.state_id);
    let mut changes = ChangeSet::default();
    click(
        &mut state,
        player,
        ConnectionId(1),
        window,
        revision,
        2,
        0,
        ClickMode::Pickup,
        &mut changes,
    );
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(CustomStat::TalkedToVillager),
        1
    );
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(CustomStat::TradedWithVillager),
        0
    );
    let bread = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:bread").unwrap())
        .unwrap();
    assert_eq!(
        state.players[&player]
            .statistics
            .get(mc_data::statistics::StatType::Crafted, bread),
        0
    );
}

fn reserve_native_merchant_stock(state: &mut TickState, count: u64) -> (String, u64) {
    use mc_script::*;
    let endpoint = ScriptInventoryEndpoint::PlayerInventory { player_id: 1 };
    let request = ScriptOperationRequest::try_new(
        "merchant-reserve",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Reserve {
                operation_id: "merchant-reserve".into(),
                endpoint: endpoint.clone(),
                resource_plan: ScriptInventoryResourcePlan::new(vec![
                    ScriptInventoryWorkPortion::new(
                        1,
                        vec![ScriptInventoryMaterial::new(
                            "minecraft:emerald".into(),
                            count,
                        )],
                    ),
                ]),
                expected_revision: crate::plugin_inventories::load(
                    state,
                    "merchant-test",
                    &endpoint,
                )
                .unwrap()
                .fence,
            },
        },
    )
    .unwrap();
    let outcome = crate::plugin_inventories::execute(state, "merchant-test", &request);
    assert_eq!(outcome.failure(), None);
    let ScriptOperationPayload::OwnedInventory { result } = outcome.payload() else {
        panic!("real native inventory result required")
    };
    let ScriptOwnedInventoryResult::Reservation { reservation } = result.as_ref() else {
        panic!("real native reservation required")
    };
    (
        reservation.reservation_ref.clone(),
        outcome.revision().unwrap(),
    )
}

#[test]
fn native_select_trade_refuses_held_payment_before_menu_mutation_release_unblocks() {
    use mc_script::*;
    let (mut state, player, merchant) = fixture();
    state.players.get_mut(&player).unwrap().inventory.slots[0].count = 4;
    let held = reserve_native_merchant_stock(&mut state, 4);
    let mut changes = ChangeSet::default();
    assert!(open(
        &mut state,
        player,
        merchant,
        ConnectionId(1),
        Hand::Main,
        &mut changes
    ));
    let before = state.players[&player].clone();
    let merchant_slot = state.entities.slot_of(merchant).unwrap();
    let before_merchant = state.entities.row(merchant_slot).villager.clone();
    select(&mut state, player, ConnectionId(1), 0, &mut changes);
    assert_eq!(state.players[&player], before);
    assert_eq!(state.entities.row(merchant_slot).villager, before_merchant);
    assert!(changes.entities_spawned.is_empty());
    assert_eq!(
        crate::plugin_inventories::reserved(
            &state,
            &ScriptInventoryEndpoint::PlayerInventory { player_id: 1 }
        )["minecraft:emerald"],
        4
    );
    let release = ScriptOperationRequest::try_new(
        "merchant-release",
        ScriptOperation::Inventory {
            operation: ScriptOwnedInventoryOperation::Release {
                operation_id: "merchant-release".into(),
                reservation_ref: held.0,
                expected_revision: held.1,
            },
        },
    )
    .unwrap();
    assert_eq!(
        crate::plugin_inventories::execute(&mut state, "merchant-test", &release).failure(),
        None
    );
    select(&mut state, player, ConnectionId(1), 0, &mut changes);
    assert!(state.players[&player].inventory.slots[0].is_empty());
    assert_eq!(
        state.players[&player].open.as_ref().unwrap().grid[0].count,
        4
    );
    assert_eq!(
        state.players[&player].open.as_ref().unwrap().result.count,
        3
    );
}

#[test]
fn merchant_offhand_opens_without_awarding_talk_statistic() {
    let (mut state, player, merchant) = fixture();
    let mut changes = ChangeSet::default();
    assert!(open(
        &mut state,
        player,
        merchant,
        ConnectionId(1),
        Hand::Off,
        &mut changes,
    ));
    assert!(state.players[&player].open.is_some());
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(mc_data::statistics::CustomStat::TalkedToVillager),
        0
    );
}

#[test]
fn native_merchant_excess_payment_and_result_preserve_hold_but_last_source_throw_is_atomic() {
    let (mut state, player, merchant) = fixture();
    let emerald = state.players[&player].inventory.slots[0].item_id;
    let p = state.players.get_mut(&player).unwrap();
    p.inventory.slots[0].count = 64;
    p.inventory.slots[9] = ItemStack::new(emerald, 40);
    let held = reserve_native_merchant_stock(&mut state, 40);
    let mut changes = ChangeSet::default();
    assert!(open(
        &mut state,
        player,
        merchant,
        ConnectionId(1),
        Hand::Main,
        &mut changes
    ));
    select(&mut state, player, ConnectionId(1), 0, &mut changes);
    let menu = state.players[&player].open.as_ref().unwrap();
    assert_eq!(menu.grid[0].count, 64);
    // Native SelectTrade fills from main inventory before the hotbar: all
    // forty in slot 9 and twenty-four in slot 0 become the sixty-four payment.
    assert!(state.players[&player].inventory.slots[9].is_empty());
    assert_eq!(state.players[&player].inventory.slots[0].count, 40);
    let window = menu.window;
    let revision = menu.state_id;
    let before = state.players[&player].clone();
    let merchant_slot = state.entities.slot_of(merchant).unwrap();
    let before_merchant = state.entities.row(merchant_slot).villager.clone();
    click(
        &mut state,
        player,
        ConnectionId(1),
        window,
        revision,
        30,
        0,
        ClickMode::Throw,
        &mut changes,
    );
    assert_eq!(state.players[&player], before);
    assert_eq!(state.entities.row(merchant_slot).villager, before_merchant);
    assert!(changes.entities_spawned.is_empty());
    click(
        &mut state,
        player,
        ConnectionId(1),
        window,
        revision,
        2,
        0,
        ClickMode::QuickMove,
        &mut changes,
    );
    assert_eq!(offer_uses(&state, merchant), 2);
    let villager = state.entities.row(merchant_slot).villager.as_ref().unwrap();
    assert_eq!(villager.xp, 2);
    assert_eq!(
        villager
            .gossips
            .iter()
            .find(|gossip| gossip.target == uuid::Uuid::nil() && gossip.kind == "trading")
            .unwrap()
            .value,
        4
    );
    assert_eq!(
        state.players[&player].open.as_ref().unwrap().grid[0].count,
        60
    );
    assert_eq!(state.players[&player].inventory.slots[0].count, 40);
    let bread = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:bread").unwrap())
        .unwrap();
    assert_eq!(
        state.players[&player]
            .inventory
            .slots
            .iter()
            .filter(|stack| stack.item_id == bread && !stack.is_empty())
            .map(|stack| stack.count)
            .sum::<i32>(),
        6
    );
    assert!(!changes.entities_spawned.is_empty());
    let reservation =
        crate::plugin_inventories::reservation(&state, "merchant-test", &held.0).unwrap();
    assert!(!reservation.snapshot.released);
    assert_eq!(reservation.snapshot.quantities[0].remaining, 40);
}

#[test]
fn accepted_trade_cancels_old_chunk_restock_debt() {
    let (mut state, player, merchant) = fixture();
    interact(&mut state, merchant);
    run_tick(
        &mut state,
        &[PlayerInput::SelectTrade {
            connection: ConnectionId(1),
            offer: 0,
        }],
    );
    let menu = state.players[&player].open.as_ref().unwrap();
    let (window, revision) = (menu.window, menu.state_id);
    let pos = state
        .entities
        .pos(state.entities.slot_of(merchant).unwrap());
    let chunk = crate::boundary::ChunkPos {
        x: (pos[0].floor() as i32).div_euclid(16),
        z: (pos[2].floor() as i32).div_euclid(16),
    };
    let tick = state.tick;
    state
        .world
        .restore_sim_debt(
            &crate::journal::Replay {
                tick,
                sim_debt: [(
                    [chunk.x, chunk.z],
                    crate::journal::SimDebtImage {
                        chunk: [chunk.x, chunk.z],
                        last_sim: tick,
                        remaining: 600,
                        active: false,
                        tick,
                    },
                )]
                .into(),
                ..Default::default()
            },
            None,
        )
        .unwrap();
    state.world.refresh_active_clock(tick);
    assert_eq!(state.world.simulation_debt(chunk), 600);
    click(
        &mut state,
        player,
        ConnectionId(1),
        window,
        revision,
        2,
        0,
        ClickMode::Pickup,
        &mut ChangeSet::default(),
    );
    assert_eq!(offer_uses(&state, merchant), 1);
    assert_eq!(
        state.world.simulation_debt(chunk),
        0,
        "a fresh trade cannot spend dormant restock time"
    );
}
