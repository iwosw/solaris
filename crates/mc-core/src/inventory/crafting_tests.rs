use super::*;
use mc_data::{Identifier, ItemEnchantment, ItemStack};

fn stack(registry: &crate::registry::CoreRegistry, name: &str) -> ItemStack {
    ItemStack::new(
        registry
            .items
            .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
            .unwrap(),
        1,
    )
}

#[test]
fn repair_accepts_fresh_input_and_merges_only_curses() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let rules = registry.item_rules();
    let mut grid = [
        stack(&registry, "diamond_pickaxe"),
        stack(&registry, "diamond_pickaxe"),
        ItemStack::EMPTY,
        ItemStack::EMPTY,
    ];
    grid[1].damage = Some(100);
    grid[0].enchantments.push(ItemEnchantment {
        id: Identifier::parse("minecraft:vanishing_curse").unwrap(),
        level: 1,
    });
    grid[1].enchantments.push(ItemEnchantment {
        id: Identifier::parse("minecraft:vanishing_curse").unwrap(),
        level: 2,
    });
    grid[1].enchantments.push(ItemEnchantment {
        id: Identifier::parse("minecraft:efficiency").unwrap(),
        level: 5,
    });
    let result = crafting::grid_result(&rules, &grid);
    assert_eq!(result.damage, Some(0));
    assert_eq!(
        result.enchantments,
        vec![ItemEnchantment {
            id: Identifier::parse("minecraft:vanishing_curse").unwrap(),
            level: 2
        }]
    );
}

#[test]
fn banner_clone_preserves_source_and_consumes_only_blank() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let rules = registry.item_rules();
    let mut source = stack(&registry, "white_banner");
    source
        .extended_mut()
        .banner_patterns
        .push(mc_data::item_stack::BannerPattern {
            pattern: Identifier::parse("minecraft:stripe_bottom").unwrap(),
            color: 14,
        });
    let mut grid = [
        source.clone(),
        stack(&registry, "white_banner"),
        ItemStack::EMPTY,
        ItemStack::EMPTY,
    ];
    assert_eq!(crafting::grid_result(&rules, &grid), source);
    let mut inventory = PlayerInventory::default();
    let mut drops = Vec::new();
    crafting::consume_craft(&rules, &mut grid, &mut inventory, &mut drops);
    assert_eq!(grid[0], source);
    assert!(grid[1].is_empty());
    assert!(drops.is_empty());
    grid[1] = stack(&registry, "red_banner");
    assert!(crafting::grid_result(&rules, &grid).is_empty());
    grid[1] = source;
    assert!(crafting::grid_result(&rules, &grid).is_empty());
}

#[test]
fn shield_decoration_menu_crafts_and_rejects_invalid_inputs() {
    use crate::boundary::ClickMode;

    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let rules = registry.item_rules();
    let mut banner = stack(&registry, "red_banner");
    banner
        .extended_mut()
        .banner_patterns
        .push(mc_data::item_stack::BannerPattern {
            pattern: Identifier::parse("minecraft:stripe_bottom").unwrap(),
            color: 11,
        });
    let mut shield = stack(&registry, "shield");
    shield.damage = Some(42);
    shield.custom_name = Some("Keepsake".into());
    shield.extended_mut().custom_data = Some(
        mc_data::item_stack::custom_data_from_tag(mc_nbt::Tag::Compound(vec![(
            "owner".into(),
            mc_nbt::Tag::String("player".into()),
        )]))
        .unwrap(),
    );
    for (kind, banner_slot, shield_slot) in
        [(MenuKind::Player, 3, 0), (MenuKind::CraftingTable, 8, 1)]
    {
        let mut menu = Menu::new(1, kind);
        let mut inventory = PlayerInventory::default();
        let mut container = Vec::new();
        menu.grid[banner_slot] = banner.clone();
        menu.grid[shield_slot] = shield.clone();
        // A normal click recomputes the result, then result pickup consumes it.
        for _ in 0..2 {
            menu.click(
                &rules,
                &mut inventory,
                &mut container,
                menu.state_id,
                (banner_slot + 1) as i16,
                0,
                ClickMode::Pickup,
                false,
            );
        }
        let output = menu.result.clone();
        assert_eq!(output.item_id, shield.item_id);
        assert_eq!(output.count, 1);
        assert_eq!(output.damage, shield.damage);
        assert_eq!(output.custom_name, shield.custom_name);
        assert_eq!(output.extended().custom_data, shield.extended().custom_data);
        assert_eq!(
            output.extended().banner_patterns,
            banner.extended().banner_patterns
        );
        let base = output
            .extended()
            .preserved_components
            .iter()
            .find(|c| c.name == "minecraft:base_color")
            .unwrap();
        assert_eq!(
            mc_nbt::read_network_value(&mut base.value.as_slice()).unwrap(),
            mc_nbt::Tag::String("red".into())
        );
        menu.click(
            &rules,
            &mut inventory,
            &mut container,
            menu.state_id,
            0,
            0,
            ClickMode::Pickup,
            false,
        );
        assert_eq!(menu.carried, output);
        assert!(menu.grid.iter().all(ItemStack::is_empty));
        assert!(menu.result.is_empty());

        let mut decorated = output;
        menu.grid[banner_slot] = banner.clone();
        menu.grid[shield_slot] = decorated.clone();
        assert!(crafting::grid_result(&rules, &menu.grid).is_empty());
        decorated.extended_mut().banner_patterns.clear();
        menu.grid[shield_slot] = decorated;
        assert!(crafting::grid_result(&rules, &menu.grid).is_empty());
        menu.grid[shield_slot] = shield.clone();
        let extra = (0..menu.grid.len())
            .find(|&i| i != banner_slot && i != shield_slot)
            .unwrap();
        menu.grid[extra] = stack(&registry, "stone");
        assert!(crafting::grid_result(&rules, &menu.grid).is_empty());
    }
}

#[test]
fn shield_decoration_plain_banner_and_stacked_input() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let rules = registry.item_rules();
    let mut banner = stack(&registry, "blue_banner");
    banner.count = 2;
    let mut grid = [
        banner,
        ItemStack::EMPTY,
        stack(&registry, "shield"),
        ItemStack::EMPTY,
    ];
    let output = crafting::grid_result(&rules, &grid);
    assert_eq!(output.item_id, grid[2].item_id);
    assert!(output.extended().banner_patterns.is_empty());
    let base = output
        .extended()
        .preserved_components
        .iter()
        .find(|c| c.name == "minecraft:base_color")
        .unwrap();
    assert_eq!(
        mc_nbt::read_network_value(&mut base.value.as_slice()).unwrap(),
        mc_nbt::Tag::String("blue".into())
    );
    crafting::consume_craft(
        &rules,
        &mut grid,
        &mut PlayerInventory::default(),
        &mut Vec::new(),
    );
    assert_eq!(grid[0].count, 1);
    assert!(grid[2].is_empty());
}

#[test]
fn written_book_cloning_increments_generation_and_retains_original() {
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let rules = registry.item_rules();
    for generation in 0..=2 {
        let mut original = stack(&registry, "written_book");
        let mut value = Vec::new();
        mc_nbt::write_network(
            &mut value,
            &mc_nbt::Tag::Compound(vec![
                ("generation".into(), mc_nbt::Tag::Int(generation)),
                ("author".into(), mc_nbt::Tag::String("Author".into())),
            ]),
        )
        .unwrap();
        original.extended_mut().preserved_components.push(
            mc_data::item_stack::PreservedItemComponent {
                name: "minecraft:written_book_content".into(),
                value,
                baseline: None,
            },
        );
        let mut grid = [
            original.clone(),
            stack(&registry, "writable_book"),
            stack(&registry, "writable_book"),
            ItemStack::EMPTY,
        ];
        let result = crafting::grid_result(&rules, &grid);
        if generation == 2 {
            assert!(result.is_empty());
            continue;
        }
        assert_eq!(result.count, 2);
        let component = &result.extended().preserved_components[0];
        let mc_nbt::Tag::Compound(fields) =
            mc_nbt::read_network(&mut component.value.as_slice()).unwrap()
        else {
            panic!("book component");
        };
        assert!(fields.contains(&("generation".into(), mc_nbt::Tag::Int(generation + 1))));
        assert!(fields.contains(&("author".into(), mc_nbt::Tag::String("Author".into()))));
        crafting::consume_craft(
            &rules,
            &mut grid,
            &mut PlayerInventory::default(),
            &mut Vec::new(),
        );
        assert_eq!(grid[0], original);
        assert!(grid[1..].iter().all(ItemStack::is_empty));
    }
}
