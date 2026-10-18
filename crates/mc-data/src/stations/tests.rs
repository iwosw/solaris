use super::*;

fn stack(items: &ItemRegistry, name: &str, count: i32) -> ItemStack {
    ItemStack::new(items.id_of(&identifier(name)).expect("vanilla item"), count)
}

#[test]
fn station_brewing_transforms_contents_and_container_without_inventing_invalid_mix() {
    let items = crate::items::solaris_required_items();
    let mut bottle = stack(&items, "minecraft:potion", 1);
    bottle.extended_mut().potion = Some(identifier("minecraft:water"));
    let awkward =
        brewing_result(&bottle, &stack(&items, "minecraft:nether_wart", 1), &items).unwrap();
    assert_eq!(
        awkward.extended().potion,
        Some(identifier("minecraft:awkward"))
    );
    let strength = brewing_result(
        &awkward,
        &stack(&items, "minecraft:blaze_powder", 1),
        &items,
    )
    .unwrap();
    assert_eq!(
        strength.extended().potion,
        Some(identifier("minecraft:strength"))
    );
    let splash =
        brewing_result(&strength, &stack(&items, "minecraft:gunpowder", 1), &items).unwrap();
    assert_eq!(
        splash.item_id,
        items.id_of(&identifier("minecraft:splash_potion")).unwrap()
    );
    assert_eq!(splash.extended().potion, strength.extended().potion);
    assert!(brewing_result(&awkward, &stack(&items, "minecraft:diamond", 1), &items).is_none());
}

#[test]
fn station_loom_appends_color_and_pattern_preserving_banner_and_rejects_seventh_layer() {
    let items = crate::items::solaris_required_items();
    let blocks = crate::blocks::solaris_required_blocks_report();
    let tags = crate::tags::solaris_required_client_tags(&items, &blocks);
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = stack(&items, "minecraft:white_banner", 2).with_custom_name("Flag");
    input[1] = stack(&items, "minecraft:red_dye", 3);
    let raw = loom_patterns(&ItemStack::EMPTY, &items, &tags)
        .iter()
        .position(|raw| {
            tags.banner_pattern(*raw) == Some(&identifier("minecraft:square_bottom_left"))
        })
        .unwrap() as i32;
    let out = loom_result(&input, raw, &items, &tags).unwrap();
    assert_eq!(out.count, 1);
    assert_eq!(out.custom_name.as_deref(), Some("Flag"));
    assert_eq!(
        out.extended().banner_patterns,
        vec![BannerPattern {
            pattern: identifier("minecraft:square_bottom_left"),
            color: 14
        }]
    );
    input[0].extended_mut().banner_patterns = vec![out.extended().banner_patterns[0].clone(); 6];
    assert!(loom_result(&input, raw, &items, &tags).is_none());
}

#[test]
fn station_anvil_material_repair_and_rename_preserve_enchantments_and_charge_actual_materials() {
    let items = crate::items::solaris_required_items();
    let facts = crate::item_components::solaris_required_item_facts();
    let tags = crate::tags::solaris_required_item_tags(&items);
    let left = stack(&items, "minecraft:diamond_sword", 1)
        .with_damage(800)
        .with_enchantment(identifier("minecraft:sharpness"), 2);
    let right = stack(&items, "minecraft:diamond", 5);
    let out = anvil_result(
        &left,
        &right,
        Some("Repaired"),
        false,
        &items,
        &facts,
        &tags,
    )
    .unwrap();
    assert_eq!(out.stack.damage, Some(0));
    assert_eq!(out.material_count, 3);
    assert_eq!(out.cost, 4);
    assert_eq!(out.stack.custom_name.as_deref(), Some("Repaired"));
    assert_eq!(out.stack.enchantments, left.enchantments);
    assert_eq!(out.stack.extended().repair_cost, 1);
    let unchanged = anvil_result(&left, &ItemStack::EMPTY, None, false, &items, &facts, &tags);
    assert!(unchanged.is_none());
}

#[test]
fn station_loom_sidecar_resolves_button_through_actual_sent_registry_and_tag_vectors() {
    let dir = tempfile::TempDir::new().unwrap();
    std::fs::create_dir_all(dir.path().join("reports")).unwrap();
    std::fs::write(dir.path().join("reports/registries.json"), "{}").unwrap();
    let root = dir.path().join("data/minecraft/tags/banner_pattern");
    std::fs::create_dir_all(root.join("pattern_item")).unwrap();
    std::fs::write(
        root.join("no_item_required.json"),
        r#"{"values":["minecraft:stripe_bottom","minecraft:diagonal_up_right"]}"#,
    )
    .unwrap();
    std::fs::write(
        root.join("pattern_item/flower.json"),
        r#"{"values":["minecraft:flower"]}"#,
    )
    .unwrap();
    let data = crate::VanillaData::from_registries(
        dir.path(),
        vec![crate::Registry {
            id: identifier("minecraft:banner_pattern"),
            entries: ["diagonal_up_right", "flower", "stripe_bottom"]
                .map(|name| identifier(&format!("minecraft:{name}")))
                .to_vec(),
        }],
    );
    let tags = crate::tags::load(dir.path(), &data).unwrap();
    let items = crate::items::solaris_required_items();
    let sent_registry = data.registry("banner_pattern").unwrap();
    let sent_tags = &tags.registries[&identifier("minecraft:banner_pattern")];
    let sent_ids = &sent_tags[&identifier("minecraft:no_item_required")];
    assert_eq!(sent_ids, &[0, 2]);
    let button = sent_ids
        .iter()
        .position(|raw| sent_registry.entries[*raw as usize].path() == "stripe_bottom")
        .unwrap() as i32;
    assert_eq!(
        button, 1,
        "neither the old catalogue nor the embedded registry order"
    );
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = stack(&items, "minecraft:white_banner", 1);
    input[1] = stack(&items, "minecraft:red_dye", 1);
    let output = loom_result(&input, button, &items, &tags).unwrap();
    assert_eq!(
        output.extended().banner_patterns[0].pattern.path(),
        "stripe_bottom"
    );
    assert!(loom_result(&input, 2, &items, &tags).is_none());
    input[2] = stack(&items, "minecraft:flower_banner_pattern", 1);
    assert_eq!(
        loom_patterns(&input[2], &items, &tags),
        sent_tags[&identifier("minecraft:pattern_item/flower")]
    );
    let output = loom_result(&input, -1, &items, &tags).unwrap();
    assert_eq!(
        output.extended().banner_patterns[0].pattern.path(),
        "flower"
    );
    assert_eq!(output.extended().banner_patterns[0].color, 14);
}

#[test]
fn station_loom_missing_registry_binding_and_invalid_tag_ids_fail_closed() {
    let items = crate::items::solaris_required_items();
    let blocks = crate::blocks::solaris_required_blocks_report();
    let mut tags = crate::tags::solaris_required_client_tags(&items, &blocks);
    let mut input = std::array::from_fn(|_| ItemStack::EMPTY);
    input[0] = stack(&items, "minecraft:white_banner", 1);
    input[1] = stack(&items, "minecraft:red_dye", 1);
    let ids = tags
        .registries
        .get_mut(&identifier("minecraft:banner_pattern"))
        .unwrap()
        .get_mut(&identifier("minecraft:no_item_required"))
        .unwrap();
    *ids = vec![-1, i32::MAX];
    assert!(loom_result(&input, 0, &items, &tags).is_none());
    assert!(loom_result(&input, 1, &items, &tags).is_none());
    let tags = TagsData::from_registries(tags.registries);
    assert!(loom_result(&input, 0, &items, &tags).is_none());
}
