use super::*;
use crate::commands::GameMode;
use crate::registry::CoreRegistry;
use mc_data::{Identifier, ItemStack};

fn registry() -> CoreRegistry {
    CoreRegistry::load(None).unwrap()
}
fn stack(registry: &CoreRegistry, name: &str, count: i32) -> ItemStack {
    ItemStack::new(
        registry
            .items
            .id_of(&Identifier::parse(name).unwrap())
            .unwrap(),
        count,
    )
}
fn player(registry: &CoreRegistry, kind: MenuKind) -> PlayerState {
    let mut player = PlayerState::new(crate::boundary::ConnectionId(81), GameMode::Survival);
    player.vitals.experience.level = 40;
    player.open_menu(&registry.item_rules(), kind, "Station".into(), &[]);
    player.initialize_station(&registry.enchantments, &registry.item_rules(), 15);
    player
}
fn take(player: &mut PlayerState, registry: &CoreRegistry) -> PlayerEffects {
    let menu = player.open.as_ref().unwrap();
    player.station_click(
        &registry.enchantments,
        &registry.item_rules(),
        menu.window,
        menu.state_id,
        2,
        0,
        ClickMode::Pickup,
        15,
    )
}

#[test]
fn enchanting_debits_lapis_and_only_offer_levels_and_refreshes_seed() {
    let registry = registry();
    let mut player = player(&registry, MenuKind::EnchantingTable);
    player.enchantment_offer_seed = 12345;
    let menu = player.open.as_mut().unwrap();
    menu.grid[0] = stack(&registry, "minecraft:diamond_sword", 1);
    menu.grid[1] = stack(&registry, "minecraft:lapis_lazuli", 8);
    let window = menu.window;
    let before = player.enchantment_offer_seed;
    let effects = player.enchant_offer(
        &registry.enchantments,
        &registry.item_rules(),
        window,
        2,
        15,
    );
    let menu = player.open.as_ref().unwrap();
    assert!(!menu.grid[0].enchantments.is_empty());
    assert_eq!(menu.grid[1].count, 5);
    assert_eq!(player.vitals.experience.level, 37);
    assert_ne!(player.enchantment_offer_seed, before);
    assert_eq!(
        player
            .statistics
            .get_custom(mc_data::statistics::CustomStat::EnchantItem),
        1
    );
    assert!(
        effects
            .changes
            .iter()
            .any(|change| matches!(change, PlayerChange::MenuProperty { property: 3, .. }))
    );
    assert_eq!(menu.station.properties[0..3], [0, 0, 0]);
}

#[test]
fn enchanting_rejects_insufficient_lapis_and_level_without_mutating() {
    let registry = registry();
    let mut player = player(&registry, MenuKind::EnchantingTable);
    player.enchantment_offer_seed = 12345;
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "minecraft:diamond_sword", 1);
    let window = player.open.as_ref().unwrap().window;
    let seed = player.enchantment_offer_seed;
    assert!(
        player
            .enchant_offer(
                &registry.enchantments,
                &registry.item_rules(),
                window,
                2,
                15
            )
            .changes
            .is_empty()
    );
    assert_eq!(player.enchantment_offer_seed, seed);
    player.open.as_mut().unwrap().grid[1] = stack(&registry, "minecraft:lapis_lazuli", 3);
    player.vitals.experience.level = 2;
    assert!(
        player
            .enchant_offer(
                &registry.enchantments,
                &registry.item_rules(),
                window,
                2,
                15
            )
            .changes
            .is_empty()
    );
    assert!(
        player.open.as_ref().unwrap().grid[0]
            .enchantments
            .is_empty()
    );
    assert_eq!(
        player
            .statistics
            .get_custom(mc_data::statistics::CustomStat::EnchantItem),
        0
    );
}

#[test]
fn enchanting_lapis_slot_filters_items_and_creative_debits_levels_only() {
    let registry = registry();
    let mut player = player(&registry, MenuKind::EnchantingTable);
    let menu = player.open.as_mut().unwrap();
    menu.carried = stack(&registry, "minecraft:stone", 2);
    let (window, revision) = (menu.window, menu.state_id);
    player.station_click(
        &registry.enchantments,
        &registry.item_rules(),
        window,
        revision,
        1,
        0,
        ClickMode::Pickup,
        15,
    );
    assert!(player.open.as_ref().unwrap().grid[1].is_empty());
    player.game_mode = GameMode::Creative;
    player.vitals.experience.level = 0;
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "minecraft:book", 1);
    let result = player.enchant_offer(
        &registry.enchantments,
        &registry.item_rules(),
        window,
        0,
        15,
    );
    assert!(!result.changes.is_empty());
    let item = &player.open.as_ref().unwrap().grid[0];
    assert_eq!(
        registry.items.name_of(item.item_id).unwrap().as_str(),
        "minecraft:enchanted_book"
    );
    assert!(!item.extended().stored_enchantments.is_empty());
    assert_eq!(player.vitals.experience.level, 0);
    player.vitals.experience.level = 40;
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "minecraft:diamond_sword", 1);
    let result = player.enchant_offer(
        &registry.enchantments,
        &registry.item_rules(),
        window,
        2,
        15,
    );
    assert!(!result.changes.is_empty());
    assert_eq!(player.vitals.experience.level, 37);
    assert!(player.open.as_ref().unwrap().grid[1].is_empty());
}

#[test]
fn anvil_rename_pays_only_on_extraction_and_guards_levels() {
    let registry = registry();
    let mut player = player(&registry, MenuKind::Anvil);
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "minecraft:diamond_sword", 1);
    let effects = player.rename_item(&registry.enchantments, &registry.item_rules(), "Blade");
    assert!(!effects.changes.is_empty());
    assert_eq!(player.vitals.experience.level, 40);
    assert_eq!(player.open.as_ref().unwrap().station.cost, 1);
    player.vitals.experience.level = 0;
    assert!(!take(&mut player, &registry).anvil_used);
    assert!(player.open.as_ref().unwrap().carried.is_empty());
    player.vitals.experience.level = 40;
    let effects = take(&mut player, &registry);
    assert!(effects.anvil_used);
    let menu = player.open.as_ref().unwrap();
    assert_eq!(menu.carried.custom_name.as_deref(), Some("Blade"));
    assert!(menu.grid[0].is_empty());
    assert_eq!(player.vitals.experience.level, 39);
    assert!(menu.result.is_empty());
}

#[test]
fn grindstone_extraction_removes_enchants_and_requests_orbs_once() {
    let registry = registry();
    let mut player = player(&registry, MenuKind::Grindstone);
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "minecraft:diamond_sword", 1)
        .with_enchantment(Identifier::parse("minecraft:sharpness").unwrap(), 3);
    let effects = take(&mut player, &registry);
    assert!(effects.experience_orbs > 0);
    let menu = player.open.as_ref().unwrap();
    assert!(menu.carried.enchantments.is_empty());
    assert!(menu.grid.iter().all(ItemStack::is_empty));
    assert_eq!(player.vitals.experience.level, 40);
    assert_eq!(take(&mut player, &registry).experience_orbs, 0);
}

#[test]
fn closing_station_returns_inputs_and_never_returns_preview_output() {
    let registry = registry();
    let mut player = player(&registry, MenuKind::Anvil);
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "minecraft:diamond_sword", 1);
    player.rename_item(&registry.enchantments, &registry.item_rules(), "Preview");
    player.close(&registry.item_rules());
    let swords: Vec<_> = player
        .inventory
        .slots
        .iter()
        .filter(|item| !item.is_empty())
        .collect();
    assert_eq!(swords.len(), 1);
    assert!(swords[0].custom_name.is_none());
}

#[test]
fn unbreaking_does_not_reroll_enchanting_offers() {
    let registry = registry();
    let mut player = player(&registry, MenuKind::EnchantingTable);
    player.enchantment_offer_seed = 12345;
    player.open.as_mut().unwrap().grid[0] = stack(&registry, "minecraft:diamond_sword", 1);
    player.initialize_station(&registry.enchantments, &registry.item_rules(), 15);
    let before = player.open.as_ref().unwrap().station.clone();
    player.inventory.slots[0] = stack(&registry, "minecraft:diamond_pickaxe", 1).with_enchantment(
        mc_data::Identifier::parse("minecraft:unbreaking").unwrap(),
        3,
    );
    let rng = player.enchantment_seed;
    player.damage_held_tool(&registry.item_rules(), 1);
    assert_ne!(player.enchantment_seed, rng);
    assert_eq!(player.enchantment_offer_seed, 12345);
    player.initialize_station(&registry.enchantments, &registry.item_rules(), 15);
    assert_eq!(player.open.as_ref().unwrap().station, before);
}
