use super::*;
use mc_data::{Identifier, ItemEnchantment};

fn enchanted(name: &str, level: i32) -> ItemStack {
    let mut stack = ItemStack::new(1, 1);
    stack.enchantments.push(ItemEnchantment {
        id: Identifier::parse(format!("minecraft:{name}")).unwrap(),
        level,
    });
    stack
}

#[test]
fn unbreaking_draws_per_damage_point_and_armor_retains_sixty_percent_floor() {
    let stack = enchanted("unbreaking", 3);
    let mut tool_seed = 149;
    let mut armor_seed = tool_seed;
    let mut expected_seed = tool_seed;
    let draws: Vec<_> = (0..1000).map(|_| random(&mut expected_seed)).collect();
    let tool = durability_damage(&stack, 1000, false, &mut tool_seed);
    let armor = durability_damage(&stack, 1000, true, &mut armor_seed);
    assert_eq!(
        tool,
        draws.iter().filter(|&&draw| draw >= 0.75).count() as i32
    );
    assert_eq!(
        armor,
        draws.iter().filter(|&&draw| draw >= 0.3).count() as i32
    );
    assert!((200..300).contains(&tool), "tool wear {tool}");
    assert!((650..750).contains(&armor), "armor wear {armor}");
    assert_eq!(tool_seed, expected_seed);
    assert_eq!(armor_seed, expected_seed);
    let mut restored_seed = tool_seed;
    assert_eq!(
        durability_damage(&stack, 20, false, &mut tool_seed),
        durability_damage(&stack, 20, false, &mut restored_seed)
    );
    assert_eq!(tool_seed, restored_seed);
    let before = tool_seed;
    assert_eq!(
        durability_damage(&ItemStack::new(1, 1), 7, false, &mut tool_seed),
        7
    );
    assert_eq!(tool_seed, before);
}

#[test]
fn unbreaking_existing_player_tool_wear_skips_and_breaks_only_on_admitted_damage() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let tags = mc_data::tags::TagsData::default();
    let rules = crate::inventory::ItemRules {
        items: &items,
        facts: &facts,
        tags: &tags,
        recipes: &[],
    };
    let id = items
        .id_of(&Identifier::parse("minecraft:iron_pickaxe").unwrap())
        .unwrap();
    let max = facts
        .facts_for_stack(&ItemStack::new(id, 1), &items)
        .unwrap()
        .max_damage
        .unwrap();
    let mut player = crate::player::PlayerState::new(
        crate::boundary::ConnectionId(4),
        crate::commands::GameMode::Survival,
    );
    let seed = (0..1000)
        .find(|&seed| {
            let mut seed = seed;
            random(&mut seed) < 0.75
        })
        .unwrap();
    player.enchantment_seed = seed;
    let mut stack = enchanted("unbreaking", 3);
    stack.item_id = id;
    stack.damage = Some(max as i32 - 1);
    player.inventory.slots[0] = stack;
    assert!(player.damage_held_tool(&rules, 1).changes.is_empty());
    assert_eq!(player.inventory.slots[0].damage, Some(max as i32 - 1));
    for _ in 0..100 {
        player.damage_held_tool(&rules, 1);
        if player.inventory.slots[0].is_empty() {
            break;
        }
    }
    assert!(player.inventory.slots[0].is_empty());
}

#[test]
fn binding_curse_blocks_every_armor_removal_path_but_allows_creative() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let tags = mc_data::tags::TagsData::default();
    let rules = crate::inventory::ItemRules {
        items: &items,
        facts: &facts,
        tags: &tags,
        recipes: &[],
    };
    let helmet = items
        .id_of(&Identifier::parse("minecraft:diamond_helmet").unwrap())
        .unwrap();
    for creative in [false, true] {
        for mode in [
            crate::boundary::ClickMode::Pickup,
            crate::boundary::ClickMode::QuickMove,
            crate::boundary::ClickMode::Swap,
            crate::boundary::ClickMode::Throw,
        ] {
            let mut player = crate::player::PlayerState::new(
                crate::boundary::ConnectionId(4),
                if creative {
                    crate::commands::GameMode::Creative
                } else {
                    crate::commands::GameMode::Survival
                },
            );
            let mut cursed = enchanted("binding_curse", 1);
            cursed.item_id = helmet;
            player.inventory.slots[39] = cursed.clone();
            let effects = player.click(&rules, 0, 0, 5, 0, mode, &mut []);
            if creative {
                assert!(player.inventory.slots[39].is_empty(), "{mode:?}");
            } else {
                assert_eq!(player.inventory.slots[39], cursed, "{mode:?}");
                assert!(effects.dropped.is_empty(), "{mode:?}");
                assert!(player.inventory_menu.carried.is_empty(), "{mode:?}");
            }
        }
    }
}

#[test]
fn vanishing_curse_destroys_inventory_and_full_inventory_cursor_on_death() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let tags = mc_data::tags::TagsData::default();
    let rules = crate::inventory::ItemRules {
        items: &items,
        facts: &facts,
        tags: &tags,
        recipes: &[],
    };
    let sword = items
        .id_of(&Identifier::parse("minecraft:diamond_sword").unwrap())
        .unwrap();
    let stone = items
        .id_of(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    let mut cursed = enchanted("vanishing_curse", 1);
    cursed.item_id = sword;
    let mut player = crate::player::PlayerState::new(
        crate::boundary::ConnectionId(4),
        crate::commands::GameMode::Survival,
    );
    player.inventory.slots.fill(ItemStack::new(stone, 64));
    player.inventory.slots[0] = cursed.clone();
    player.inventory_menu.carried = cursed.clone();
    let effects = player.die(&rules, false, false);
    assert!(player.inventory.slots.iter().all(ItemStack::is_empty));
    assert!(!effects.dropped.is_empty());
    assert!(
        effects
            .dropped
            .iter()
            .all(|stack| level(stack, "vanishing_curse") == 0)
    );
    player.inventory.slots[0] = cursed.clone();
    assert!(player.die(&rules, true, false).dropped.is_empty());
    assert_eq!(player.inventory.slots[0], cursed);
}

#[test]
fn existing_knockback_enchantment_adds_strength_without_changing_weapon_damage() {
    let items = mc_data::items::solaris_required_items();
    let facts = mc_data::item_components::solaris_required_item_facts();
    let tags = mc_data::tags::TagsData::default();
    let rules = crate::inventory::ItemRules {
        items: &items,
        facts: &facts,
        tags: &tags,
        recipes: &[],
    };
    let sword = items
        .id_of(&Identifier::parse("minecraft:diamond_sword").unwrap())
        .unwrap();
    let plain = ItemStack::new(sword, 1);
    let mut knockback = enchanted("knockback", 2);
    knockback.item_id = sword;
    let swing = crate::combat::Swing {
        ticks_since_reset: 20,
        falling: false,
        on_ground: true,
        sprinting: false,
        crit_blocked: false,
        melee_bonus: 0.0,
        attack_speed_multiplier: 1.0,
    };
    let base = crate::combat::melee_hit(&rules, &plain, swing);
    let pushed = crate::combat::melee_hit(&rules, &knockback, swing);
    assert_eq!(pushed.damage, base.damage);
    assert_eq!(pushed.bonus_knockback, base.bonus_knockback + 1.0);
}
