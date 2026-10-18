use super::*;
use crate::boundary::{ClickMode, ConnectionId};
use crate::commands::GameMode;
use crate::player::PlayerState;
use crate::registry::CoreRegistry;
use mc_data::Identifier;

fn stack(registry: &CoreRegistry, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        registry
            .items
            .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap(),
        count,
    )
}
fn player(registry: &CoreRegistry, kind: MenuKind) -> PlayerState {
    let mut player = PlayerState::new(ConnectionId(81), GameMode::Survival);
    player.open_menu(&registry.item_rules(), kind, "Station".into(), &[]);
    player.initialize_station(&registry.enchantments, &registry.item_rules(), 0);
    player
}
fn click(
    player: &mut PlayerState,
    registry: &CoreRegistry,
    slot: i16,
    mode: ClickMode,
) -> crate::player::PlayerEffects {
    let menu = player.open.as_ref().unwrap();
    player.station_click(
        &registry.enchantments,
        &registry.item_rules(),
        menu.window,
        menu.state_id,
        slot,
        0,
        mode,
        0,
    )
}
fn select(player: &mut PlayerState, registry: &CoreRegistry, selection: i32) {
    let window = player.open.as_ref().unwrap().window;
    player.station_button(
        &registry.enchantments,
        &registry.item_rules(),
        window,
        selection,
        0,
    );
}
fn count(player: &PlayerState, item_id: u32) -> i32 {
    player
        .inventory
        .slots
        .iter()
        .filter(|s| !s.is_empty() && s.item_id == item_id)
        .map(|s| s.count)
        .sum()
}

#[test]
fn station_output_credits_follow_vanilla_only_after_accepted_extraction() {
    let registry = CoreRegistry::load(None).unwrap();
    for kind in [MenuKind::Smithing, MenuKind::Loom, MenuKind::Stonecutter] {
        for mode in [
            ClickMode::Pickup,
            ClickMode::QuickMove,
            ClickMode::Swap,
            ClickMode::Throw,
        ] {
            for blocked in [false, true] {
                let mut player = player(&registry, kind);
                let menu = player.open.as_mut().unwrap();
                match kind {
                    MenuKind::Smithing => {
                        menu.grid = vec![
                            stack(&registry, "netherite_upgrade_smithing_template", 3),
                            stack(&registry, "diamond_sword", 1),
                            stack(&registry, "netherite_ingot", 3),
                        ];
                    }
                    MenuKind::Loom => {
                        menu.grid[0] = stack(&registry, "white_banner", 3);
                        menu.grid[1] = stack(&registry, "red_dye", 3);
                    }
                    MenuKind::Stonecutter => menu.grid[0] = stack(&registry, "stone", 3),
                    _ => unreachable!(),
                }
                player.initialize_station(&registry.enchantments, &registry.item_rules(), 0);
                select(&mut player, &registry, 0);
                let menu = player.open.as_mut().unwrap();
                let output = menu.result.clone();
                assert!(!output.is_empty());
                let before = menu.grid.clone();
                if blocked {
                    let dirt = stack(&registry, "dirt", 64);
                    match mode {
                        ClickMode::Pickup | ClickMode::Throw => menu.carried = dirt,
                        ClickMode::QuickMove => player.inventory.slots[..36].fill(dirt),
                        ClickMode::Swap => player.inventory.slots[0] = dirt,
                        _ => unreachable!(),
                    }
                }
                let slot = menu.grid.len() as i16;
                let outcome = menu.click(
                    &registry.item_rules(),
                    &mut player.inventory,
                    &mut [],
                    menu.state_id,
                    slot,
                    i8::from(mode == ClickMode::Throw),
                    mode,
                    false,
                );
                let manual = if !blocked && mode == ClickMode::Throw {
                    vec![(output.item_id, output.count)]
                } else {
                    Vec::new()
                };
                assert_eq!(outcome.manual_drops, manual);
                if blocked {
                    assert_eq!(outcome.crafted_output, None);
                    assert_eq!(menu.grid, before);
                    assert_eq!(menu.result, output);
                } else {
                    let crafts = if mode == ClickMode::QuickMove && kind != MenuKind::Smithing {
                        3
                    } else {
                        1
                    };
                    let expected =
                        (kind != MenuKind::Loom).then_some((output.item_id, output.count * crafts));
                    assert_eq!(outcome.crafted_output, expected, "{kind:?} {mode:?}");
                    assert_ne!(menu.grid, before);
                }
            }
        }
    }
}

#[test]
fn smithing_upgrade_preserves_metadata_and_shift_click_debits_each_input() {
    let registry = CoreRegistry::load(None).unwrap();
    let mut player = player(&registry, MenuKind::Smithing);
    let mut base = stack(&registry, "diamond_sword", 1);
    base.damage = Some(123);
    base = base.with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 3);
    let menu = player.open.as_mut().unwrap();
    menu.grid = vec![
        stack(&registry, "netherite_upgrade_smithing_template", 2),
        base.clone(),
        stack(&registry, "netherite_ingot", 2),
    ];
    let effects = click(&mut player, &registry, 3, ClickMode::QuickMove);
    assert!(effects.dropped.is_empty());
    let output = stack(&registry, "netherite_sword", 1);
    let upgraded = player
        .inventory
        .slots
        .iter()
        .find(|s| s.item_id == output.item_id && !s.is_empty())
        .unwrap();
    assert_eq!(upgraded.damage, base.damage);
    assert_eq!(upgraded.enchantments, base.enchantments);
    let menu = player.open.as_ref().unwrap();
    assert_eq!(menu.grid[0].count, 1);
    assert!(menu.grid[1].is_empty());
    assert_eq!(menu.grid[2].count, 1);
    assert!(menu.result.is_empty());
    assert_eq!(count(&player, output.item_id), 1);
}

#[test]
fn smithing_trim_rejects_identical_trim_and_preserves_base_components() {
    let registry = CoreRegistry::load(None).unwrap();
    let mut player = player(&registry, MenuKind::Smithing);
    let menu = player.open.as_mut().unwrap();
    let mut base = stack(&registry, "diamond_chestplate", 1);
    base.damage = Some(27);
    menu.grid = vec![
        stack(&registry, "sentry_armor_trim_smithing_template", 2),
        base,
        stack(&registry, "gold_ingot", 2),
    ];
    click(&mut player, &registry, 3, ClickMode::Pickup);
    let output = player.open.as_ref().unwrap().carried.clone();
    assert_eq!(output.damage, Some(27));
    let trim = output.extended().trim.as_ref().unwrap();
    assert_eq!(trim.material.as_str(), "minecraft:gold");
    assert_eq!(trim.pattern.as_str(), "minecraft:sentry");
    let menu = player.open.as_mut().unwrap();
    menu.carried = ItemStack::EMPTY;
    menu.grid[1] = output;
    click(&mut player, &registry, 3, ClickMode::Pickup);
    let menu = player.open.as_ref().unwrap();
    assert!(menu.carried.is_empty());
    assert_eq!(menu.grid[0].count, 1);
    assert_eq!(menu.grid[2].count, 1);
}

#[test]
fn loom_selection_appends_pattern_color_and_keeps_reusable_template() {
    let registry = CoreRegistry::load(None).unwrap();
    let mut player = player(&registry, MenuKind::Loom);
    let menu = player.open.as_mut().unwrap();
    let mut banner = stack(&registry, "white_banner", 2);
    banner
        .extended_mut()
        .banner_patterns
        .push(mc_data::item_stack::BannerPattern {
            pattern: Identifier::parse("minecraft:cross").unwrap(),
            color: 1,
        });
    menu.grid = vec![
        banner,
        stack(&registry, "red_dye", 2),
        stack(&registry, "creeper_banner_pattern", 1),
    ];
    select(&mut player, &registry, 0);
    let output = player.open.as_ref().unwrap().result.clone();
    assert_eq!(output.extended().banner_patterns.len(), 2);
    assert_eq!(
        output.extended().banner_patterns[1].pattern.as_str(),
        "minecraft:creeper"
    );
    assert_eq!(output.extended().banner_patterns[1].color, 14);
    click(&mut player, &registry, 3, ClickMode::QuickMove);
    let menu = player.open.as_ref().unwrap();
    assert!(menu.grid[0].is_empty());
    assert!(menu.grid[1].is_empty());
    assert_eq!(menu.grid[2].count, 1);
    assert_eq!(count(&player, output.item_id), 2);
    assert!(
        player
            .inventory
            .slots
            .iter()
            .filter(|s| !s.is_empty() && s.item_id == output.item_id)
            .all(|s| s.extended().banner_patterns == output.extended().banner_patterns)
    );
}

#[test]
fn loom_regular_selection_and_six_layer_limit_are_authoritative() {
    let registry = CoreRegistry::load(None).unwrap();
    let mut player = player(&registry, MenuKind::Loom);
    let menu = player.open.as_mut().unwrap();
    menu.grid[0] = stack(&registry, "blue_banner", 1);
    menu.grid[1] = stack(&registry, "yellow_dye", 1);
    select(&mut player, &registry, 1);
    let raw =
        mc_data::stations::loom_patterns(&ItemStack::EMPTY, &registry.items, &registry.tags)[1];
    let expected = registry.tags.banner_pattern(raw).unwrap().clone();
    assert_eq!(
        player
            .open
            .as_ref()
            .unwrap()
            .result
            .extended()
            .banner_patterns[0]
            .pattern,
        expected
    );
    let menu = player.open.as_mut().unwrap();
    menu.grid[0].extended_mut().banner_patterns = vec![
        mc_data::item_stack::BannerPattern {
            pattern: expected,
            color: 0
        };
        6
    ];
    click(&mut player, &registry, 3, ClickMode::Pickup);
    assert!(player.open.as_ref().unwrap().carried.is_empty());
    assert_eq!(player.open.as_ref().unwrap().grid[1].count, 1);
}

#[test]
fn loom_stripe_bottom_button_uses_configuration_registry_and_tag_order() {
    let registry = CoreRegistry::load(None).unwrap();
    let sent = mc_data::solaris_required_data();
    let patterns = sent.registry("banner_pattern").unwrap();
    let ids = &registry.tags.registries[&Identifier::parse("minecraft:banner_pattern").unwrap()]
        [&Identifier::parse("minecraft:no_item_required").unwrap()];
    assert_eq!(ids.len(), 32);
    let button = ids
        .iter()
        .position(|raw| patterns.entries[*raw as usize].as_str() == "minecraft:stripe_bottom")
        .unwrap() as i32;
    assert_eq!(
        button, 20,
        "the native Java client sees this emitted tag order"
    );
    let mut player = player(&registry, MenuKind::Loom);
    let menu = player.open.as_mut().unwrap();
    menu.grid[0] = stack(&registry, "white_banner", 2);
    menu.grid[1] = stack(&registry, "blue_dye", 2);
    select(&mut player, &registry, button);
    let output = player.open.as_ref().unwrap().result.clone();
    assert_eq!(
        output.extended().banner_patterns[0].pattern.as_str(),
        "minecraft:stripe_bottom"
    );
    assert_eq!(output.extended().banner_patterns[0].color, 11);
    click(&mut player, &registry, 3, ClickMode::Pickup);
    let menu = player.open.as_ref().unwrap();
    assert_eq!(menu.carried, output);
    assert_eq!(menu.grid[0].count, 1);
    assert_eq!(menu.grid[1].count, 1);
}

#[test]
fn loom_flower_pattern_item_uses_its_sent_tag_and_remains_reusable() {
    let registry = CoreRegistry::load(None).unwrap();
    let sent = mc_data::solaris_required_data();
    let patterns = sent.registry("banner_pattern").unwrap();
    let ids = &registry.tags.registries[&Identifier::parse("minecraft:banner_pattern").unwrap()]
        [&Identifier::parse("minecraft:pattern_item/flower").unwrap()];
    assert_eq!(ids.len(), 1);
    assert_eq!(
        patterns.entries[ids[0] as usize].as_str(),
        "minecraft:flower"
    );
    let mut player = player(&registry, MenuKind::Loom);
    let menu = player.open.as_mut().unwrap();
    menu.grid = vec![
        stack(&registry, "white_banner", 1),
        stack(&registry, "red_dye", 1),
        stack(&registry, "flower_banner_pattern", 1),
    ];
    select(&mut player, &registry, 0);
    assert_eq!(
        player
            .open
            .as_ref()
            .unwrap()
            .result
            .extended()
            .banner_patterns[0]
            .pattern
            .as_str(),
        "minecraft:flower",
    );
    click(&mut player, &registry, 3, ClickMode::Pickup);
    let menu = player.open.as_ref().unwrap();
    assert_eq!(
        menu.carried.extended().banner_patterns[0].pattern.as_str(),
        "minecraft:flower"
    );
    assert!(menu.grid[0].is_empty());
    assert!(menu.grid[1].is_empty());
    assert_eq!(menu.grid[2].count, 1);
}

#[test]
fn stonecutter_filtered_selection_repeats_shift_debit_and_resets_for_new_item() {
    let registry = CoreRegistry::load(None).unwrap();
    let mut player = player(&registry, MenuKind::Stonecutter);
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "stone", 3);
    select(&mut player, &registry, 0);
    let output = player.open.as_ref().unwrap().result.clone();
    assert!(!output.is_empty());
    let offer = registry
        .recipes
        .iter()
        .find(|recipe| {
            mc_data::recipes::stonecutting_recipe_accepts_input(
                recipe,
                &registry.items,
                &registry.item_facts,
                &registry.tags,
                &stack(&registry, "stone", 1),
            )
        })
        .unwrap();
    assert_eq!(
        registry.items.name_of(output.item_id),
        Some(&offer.result.item)
    );
    assert_eq!(output.count, offer.result.count as i32);
    click(&mut player, &registry, 1, ClickMode::QuickMove);
    assert_eq!(count(&player, output.item_id), output.count * 3);
    let menu = player.open.as_ref().unwrap();
    assert!(menu.grid[0].is_empty());
    assert_eq!(menu.station.selected, -1);
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "granite", 1);
    click(&mut player, &registry, 1, ClickMode::Pickup);
    assert!(player.open.as_ref().unwrap().carried.is_empty());
    select(&mut player, &registry, i32::MAX);
    assert!(player.open.as_ref().unwrap().result.is_empty());
}

#[test]
fn craft_station_shift_output_is_all_or_nothing_when_inventory_is_full() {
    let registry = CoreRegistry::load(None).unwrap();
    for kind in [MenuKind::Smithing, MenuKind::Loom, MenuKind::Stonecutter] {
        let mut player = player(&registry, kind);
        player.inventory.slots[..36].fill(stack(&registry, "dirt", 64));
        let menu = player.open.as_mut().unwrap();
        match kind {
            MenuKind::Smithing => {
                menu.grid = vec![
                    stack(&registry, "netherite_upgrade_smithing_template", 1),
                    stack(&registry, "diamond_sword", 1),
                    stack(&registry, "netherite_ingot", 1),
                ]
            }
            MenuKind::Loom => {
                menu.grid[0] = stack(&registry, "white_banner", 1);
                menu.grid[1] = stack(&registry, "red_dye", 1);
            }
            MenuKind::Stonecutter => menu.grid[0] = stack(&registry, "stone", 1),
            _ => unreachable!(),
        }
        select(&mut player, &registry, 0);
        let before = player.open.as_ref().unwrap().grid.clone();
        let slot = before.len() as i16;
        let effects = click(&mut player, &registry, slot, ClickMode::QuickMove);
        assert!(effects.dropped.is_empty());
        assert_eq!(player.open.as_ref().unwrap().grid, before);
    }
}

#[test]
fn close_refunds_inputs_and_cursor_once_but_never_virtual_outputs() {
    let registry = CoreRegistry::load(None).unwrap();
    for kind in [MenuKind::Smithing, MenuKind::Loom, MenuKind::Stonecutter] {
        let mut player = player(&registry, kind);
        let input = stack(&registry, "stone", 3);
        let cursor = stack(&registry, "diamond", 2);
        let virtual_result = stack(&registry, "netherite_sword", 1);
        let menu = player.open.as_mut().unwrap();
        menu.grid[0] = input.clone();
        menu.carried = cursor.clone();
        menu.result = virtual_result.clone();
        assert!(player.close(&registry.item_rules()).dropped.is_empty());
        assert!(player.close(&registry.item_rules()).dropped.is_empty());
        assert_eq!(count(&player, input.item_id), 3);
        assert_eq!(count(&player, cursor.item_id), 2);
        assert_eq!(count(&player, virtual_result.item_id), 0);
    }
}

#[test]
fn station_hotbar_swap_merges_whole_output_and_throw_debits_one_recipe() {
    let registry = CoreRegistry::load(None).unwrap();
    let mut player = player(&registry, MenuKind::Stonecutter);
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "stone", 3);
    select(&mut player, &registry, 0);
    let output = player.open.as_ref().unwrap().result.clone();
    player.inventory.slots[0] = output.clone();
    click(&mut player, &registry, 1, ClickMode::Swap);
    assert_eq!(player.inventory.slots[0].count, output.count * 2);
    assert_eq!(player.open.as_ref().unwrap().grid[0].count, 2);
    let menu = player.open.as_ref().unwrap();
    let effects = player.station_click(
        &registry.enchantments,
        &registry.item_rules(),
        menu.window,
        menu.state_id,
        1,
        1,
        ClickMode::Throw,
        0,
    );
    assert_eq!(effects.dropped, vec![output]);
    assert_eq!(player.open.as_ref().unwrap().grid[0].count, 1);
}

#[test]
fn full_inventory_close_drops_real_inputs_once_and_discards_projection() {
    let registry = CoreRegistry::load(None).unwrap();
    for kind in [MenuKind::Smithing, MenuKind::Loom, MenuKind::Stonecutter] {
        let mut player = player(&registry, kind);
        let filler = stack(&registry, "dirt", 1);
        let maximum = registry.item_rules().max_stack(&filler);
        player
            .inventory
            .slots
            .fill(stack(&registry, "dirt", maximum));
        let input = stack(&registry, "diamond", 2);
        let cursor = stack(&registry, "gold_ingot", 3);
        let menu = player.open.as_mut().unwrap();
        menu.grid[0] = input.clone();
        menu.carried = cursor.clone();
        menu.result = stack(&registry, "netherite_sword", 1);
        let effects = player.close(&registry.item_rules());
        assert_eq!(effects.dropped, vec![cursor, input]);
        assert!(player.close(&registry.item_rules()).dropped.is_empty());
    }
}

#[test]
fn brewing_shared_menu_routes_bottles_fuel_and_ingredients_without_refunding_storage() {
    let registry = CoreRegistry::load(None).unwrap();
    let rules = registry.item_rules();
    let mut inventory = PlayerInventory::default();
    let mut menu = Menu::new(1, MenuKind::BrewingStand);
    let mut storage = vec![ItemStack::EMPTY; 5];
    for (item, count) in [("potion", 3), ("blaze_powder", 7), ("nether_wart", 4)] {
        inventory.slots[9] = stack(&registry, item, count);
        menu.click(
            &rules,
            &mut inventory,
            &mut storage,
            menu.state_id,
            5,
            0,
            ClickMode::QuickMove,
            false,
        );
        assert!(inventory.slots[9].is_empty());
    }
    assert!(storage[..3].iter().all(|s| s.count == 1));
    assert_eq!(storage[3], stack(&registry, "nether_wart", 4));
    assert_eq!(storage[4], stack(&registry, "blaze_powder", 7));
    menu.carried = stack(&registry, "dirt", 1);
    menu.click(
        &rules,
        &mut inventory,
        &mut storage,
        menu.state_id,
        0,
        0,
        ClickMode::Pickup,
        false,
    );
    assert_eq!(menu.carried, stack(&registry, "dirt", 1));
    let before = storage.clone();
    menu.close(&rules, &mut inventory);
    assert_eq!(storage, before);
    assert!(menu.close(&rules, &mut inventory).is_empty());
}

#[test]
fn hopper_and_dispenser_layouts_keep_native_storage_and_shift_click_custody() {
    let registry = CoreRegistry::load(None).unwrap();
    let rules = registry.item_rules();
    for (kind, slots, menu_type) in [(MenuKind::Hopper, 5, 16), (MenuKind::Dispenser, 9, 6)] {
        let mut inventory = PlayerInventory::default();
        let mut menu = Menu::new(1, kind);
        let mut storage = vec![ItemStack::EMPTY; slots];
        assert_eq!(kind.menu_type(), Some(menu_type));
        assert_eq!(kind.slot_count(), slots + 36);
        inventory.slots[9] = stack(&registry, "stone", 7);
        menu.click(
            &rules,
            &mut inventory,
            &mut storage,
            menu.state_id,
            slots as i16,
            0,
            ClickMode::QuickMove,
            false,
        );
        assert_eq!(storage[0].count, 7);
        assert!(inventory.slots[9].is_empty());
        menu.close(&rules, &mut inventory);
        assert_eq!(storage[0].count, 7);
        menu.click(
            &rules,
            &mut inventory,
            &mut storage,
            menu.state_id,
            0,
            0,
            ClickMode::QuickMove,
            false,
        );
        assert!(storage[0].is_empty());
        assert_eq!(inventory.slots.iter().map(|s| s.count).sum::<i32>(), 7);
    }
}
