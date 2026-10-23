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
