use super::*;

fn run(tag: &mut Tag, kind: NodeKind, version: i32) -> Result<(), UpgradeError> {
    fix_node(tag, kind, version, &mut UpgradeReport::default())
}
fn item(id: &str, legacy: Tag) -> Tag {
    compound(vec![
        ("id", text(id)),
        ("Count", Tag::Byte(1)),
        ("tag", legacy),
    ])
}
fn components(item: &Tag) -> &Tag {
    get(item, "components").expect("components")
}
fn component<'a>(item: &'a Tag, key: &str) -> &'a Tag {
    get(components(item), key).expect(key)
}

#[test]
fn factory_item_renames_are_scoped_and_complete() {
    for (version, old, new) in [
        (3438, "pottery_shard_archer", "archer_pottery_shard"),
        (3447, "archer_pottery_shard", "archer_pottery_sherd"),
        (3692, "grass", "short_grass"),
        (3800, "scute", "turtle_scute"),
        (4541, "chain", "iron_chain"),
    ] {
        let mut stack = item(
            &format!("minecraft:{old}"),
            compound(vec![("opaque", text(old))]),
        );
        run(&mut stack, NodeKind::Item, version).unwrap();
        assert_eq!(get(&stack, "id"), Some(&text(&format!("minecraft:{new}"))));
        assert_eq!(get(get(&stack, "tag").unwrap(), "opaque"), Some(&text(old)));
        let mut modded = item(&format!("mod:{old}"), empty());
        let before = modded.clone();
        run(&mut modded, NodeKind::Item, version).unwrap();
        assert_eq!(modded, before);
    }
    for sherd in tables::SHERDS {
        let old = format!("minecraft:{sherd}_pottery_shard");
        assert_eq!(
            renamed_item(&old, 3447),
            Some(format!("minecraft:{sherd}_pottery_sherd"))
        );
    }
}

#[test]
fn goat_horn_variants_and_default_3094() {
    for variant in [-1, 0, 7, 8] {
        let mut stack = item(
            "minecraft:goat_horn",
            compound(vec![("SoundVariant", Tag::Int(variant))]),
        );
        run(&mut stack, NodeKind::Item, 3094).unwrap();
        let tag = get(&stack, "tag").unwrap();
        assert!(get(tag, "SoundVariant").is_none());
        assert_eq!(
            get(tag, "instrument"),
            Some(&text(tables::HORNS[if variant == 7 { 7 } else { 0 }]))
        );
    }
}

#[test]
fn filtered_books_3097_preserve_unfiltered() {
    for id in ["minecraft:writable_book", "minecraft:written_book"] {
        let mut stack = item(
            id,
            compound(vec![
                ("filtered_title", text("hidden")),
                ("filtered_pages", empty()),
                ("pages", sequence(vec![text("original")])),
            ]),
        );
        run(&mut stack, NodeKind::Item, 3097).unwrap();
        let tag = get(&stack, "tag").unwrap();
        assert!(get(tag, "filtered_title").is_none());
        assert!(get(tag, "filtered_pages").is_none());
        assert_eq!(get(tag, "pages"), Some(&sequence(vec![text("original")])));
    }
}

#[test]
fn spawn_egg_3209_uses_registered_map_and_pig_fallback() {
    for entity in ["minecraft:wither", "mod:custom", "minecraft:zombie"] {
        let mut stack = item(
            "minecraft:pig_spawn_egg",
            compound(vec![(
                "EntityTag",
                compound(vec![("id", text(entity)), ("opaque", Tag::Int(4))]),
            )]),
        );
        run(&mut stack, NodeKind::Item, 3209).unwrap();
        let expected = if entity.starts_with("mod:") {
            "minecraft:pig_spawn_egg".to_owned()
        } else {
            format!("{entity}_spawn_egg")
        };
        assert_eq!(get(&stack, "id"), Some(&text(&expected)));
        assert_eq!(
            get(
                get(get(&stack, "tag").unwrap(), "EntityTag").unwrap(),
                "opaque"
            ),
            Some(&Tag::Int(4))
        );
    }
}

#[test]
fn sweeping_enchantment_3803_both_lists() {
    let mut stack = item(
        "minecraft:enchanted_book",
        compound(vec![
            (
                "Enchantments",
                sequence(vec![compound(vec![
                    ("id", text("minecraft:sweeping")),
                    ("lvl", Tag::Short(2)),
                ])]),
            ),
            (
                "StoredEnchantments",
                sequence(vec![compound(vec![
                    ("id", text("minecraft:sweeping")),
                    ("lvl", Tag::Short(3)),
                ])]),
            ),
        ]),
    );
    run(&mut stack, NodeKind::Item, 3803).unwrap();
    for key in ["Enchantments", "StoredEnchantments"] {
        assert_eq!(
            get(
                &list(get(get(&stack, "tag").unwrap(), key).unwrap()).unwrap()[0],
                "id"
            ),
            Some(&text("minecraft:sweeping_edge"))
        );
    }
}

#[test]
fn compass_block_pos_3813_and_jump_attribute_3814() {
    let mut compass = item(
        "minecraft:compass",
        compound(vec![(
            "LodestonePos",
            compound(vec![
                ("X", Tag::Int(-4)),
                ("Y", Tag::Int(90)),
                ("Z", Tag::Int(20)),
            ]),
        )]),
    );
    run(&mut compass, NodeKind::Item, 3813).unwrap();
    assert_eq!(
        get(get(&compass, "tag").unwrap(), "LodestonePos"),
        Some(&Tag::IntArray(vec![-4, 90, 20]))
    );
    let mut stack = item(
        "minecraft:stick",
        compound(vec![(
            "AttributeModifiers",
            sequence(vec![compound(vec![(
                "AttributeName",
                text("minecraft:horse.jump_strength"),
            )])]),
        )]),
    );
    run(&mut stack, NodeKind::Item, 3814).unwrap();
    assert_eq!(
        get(
            &list(get(get(&stack, "tag").unwrap(), "AttributeModifiers").unwrap()).unwrap()[0],
            "AttributeName"
        ),
        Some(&text("minecraft:generic.jump_strength"))
    );
}

#[test]
fn componentization_3818_moves_basics_and_preserves_opaque_data() {
    let opaque = compound(vec![
        ("id", text("minecraft:grass")),
        ("Count", Tag::Byte(20)),
    ]);
    let mut stack = item(
        "minecraft:diamond_sword",
        compound(vec![
            ("Damage", Tag::Int(4)),
            ("RepairCost", Tag::Int(0)),
            ("CustomModelData", Tag::Int(7)),
            ("Unbreakable", Tag::Byte(1)),
            ("HideFlags", Tag::Int(255)),
            (
                "Trim",
                compound(vec![
                    ("material", text("minecraft:gold")),
                    ("pattern", text("minecraft:spire")),
                ]),
            ),
            (
                "display",
                compound(vec![
                    ("Name", text("{\"text\":\"Sword\"}")),
                    ("Lore", sequence(vec![text("{\"text\":\"Lore\"}")])),
                    ("unknown", Tag::Int(9)),
                ]),
            ),
            ("Opaque", opaque.clone()),
        ]),
    );
    set(&mut stack, "Slot", Tag::Byte(5));
    run(&mut stack, NodeKind::Item, 3818).unwrap();
    assert!(get(&stack, "Count").is_none());
    assert!(get(&stack, "tag").is_none());
    assert_eq!(get(&stack, "count"), Some(&Tag::Int(1)));
    assert_eq!(get(&stack, "Slot"), Some(&Tag::Byte(5)));
    assert_eq!(component(&stack, "minecraft:damage"), &Tag::Int(4));
    assert!(get(components(&stack), "minecraft:repair_cost").is_none());
    assert_eq!(
        get(
            component(&stack, "minecraft:unbreakable"),
            "show_in_tooltip"
        ),
        Some(&Tag::Byte(0))
    );
    assert_eq!(
        get(component(&stack, "minecraft:trim"), "show_in_tooltip"),
        Some(&Tag::Byte(0))
    );
    assert_eq!(
        get(component(&stack, "minecraft:custom_data"), "Opaque"),
        Some(&opaque)
    );
    assert_eq!(
        get(
            get(component(&stack, "minecraft:custom_data"), "display").unwrap(),
            "unknown"
        ),
        Some(&Tag::Int(9))
    );
    assert_eq!(
        get(component(&stack, "minecraft:dyed_color"), "rgb"),
        Some(&Tag::Int(10511680))
    );
}

#[test]
fn componentization_enchantment_clamps_duplicates_and_empty_glint() {
    let enchantment = |id, level| compound(vec![("id", text(id)), ("lvl", Tag::Int(level))]);
    let mut stack = item(
        "minecraft:enchanted_book",
        compound(vec![
            (
                "Enchantments",
                sequence(vec![
                    enchantment("minecraft:sharpness", 500),
                    enchantment("minecraft:sharpness", 2),
                    enchantment("minecraft:bad", -2),
                ]),
            ),
            ("StoredEnchantments", sequence(Vec::new())),
        ]),
    );
    run(&mut stack, NodeKind::Item, 3818).unwrap();
    assert_eq!(
        get(
            get(component(&stack, "minecraft:enchantments"), "levels").unwrap(),
            "minecraft:sharpness"
        ),
        Some(&Tag::Int(2))
    );
    assert_eq!(
        get(component(&stack, "minecraft:enchantments"), "levels")
            .and_then(fields)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        component(&stack, "minecraft:enchantment_glint_override"),
        &Tag::Byte(1)
    );
}

#[test]
fn componentization_block_container_signed_slot_and_residue() {
    let nested = compound(vec![
        ("id", text("minecraft:stone")),
        ("count", Tag::Int(4)),
        ("Slot", Tag::Byte(-1)),
    ]);
    let mut stack = item(
        "minecraft:chest",
        compound(vec![(
            "BlockEntityTag",
            compound(vec![
                ("id", text("minecraft:chest")),
                ("Items", sequence(vec![nested])),
                ("Lock", text("key")),
                ("LootTable", text("minecraft:chests/simple_dungeon")),
                ("LootTableSeed", Tag::Long(42)),
                ("opaque", Tag::Int(8)),
            ]),
        )]),
    );
    run(&mut stack, NodeKind::Item, 3818).unwrap();
    let entry = &list(component(&stack, "minecraft:container")).unwrap()[0];
    assert_eq!(get(entry, "slot"), Some(&Tag::Int(255)));
    assert!(get(get(entry, "item").unwrap(), "Slot").is_none());
    assert_eq!(component(&stack, "minecraft:lock"), &text("key"));
    assert_eq!(
        get(component(&stack, "minecraft:container_loot"), "seed"),
        Some(&Tag::Long(42))
    );
    assert_eq!(
        get(component(&stack, "minecraft:block_entity_data"), "opaque"),
        Some(&Tag::Int(8))
    );
}

#[test]
fn componentization_block_specific_components() {
    for (id, field, component_name, value) in [
        (
            "minecraft:decorated_pot",
            "sherds",
            "minecraft:pot_decorations",
            sequence(vec![text("minecraft:brick")]),
        ),
        (
            "minecraft:banner",
            "patterns",
            "minecraft:banner_patterns",
            sequence(Vec::new()),
        ),
        (
            "minecraft:beehive",
            "bees",
            "minecraft:bees",
            sequence(Vec::new()),
        ),
        (
            "minecraft:skull",
            "note_block_sound",
            "minecraft:note_block_sound",
            text("minecraft:block.note_block.harp"),
        ),
    ] {
        let mut stack = item(
            "minecraft:stone",
            compound(vec![(
                "BlockEntityTag",
                compound(vec![("id", text(id)), (field, value.clone())]),
            )]),
        );
        run(&mut stack, NodeKind::Item, 3818).unwrap();
        assert_eq!(component(&stack, component_name), &value);
        assert!(get(components(&stack), "minecraft:block_entity_data").is_none());
    }
}

#[test]
fn componentization_entity_bundle_crossbow_and_bucket() {
    for (id, old, new) in [
        ("minecraft:stone", "EntityTag", "minecraft:entity_data"),
        ("minecraft:bundle", "Items", "minecraft:bundle_contents"),
        (
            "minecraft:crossbow",
            "ChargedProjectiles",
            "minecraft:charged_projectiles",
        ),
        ("minecraft:knowledge_book", "Recipes", "minecraft:recipes"),
        ("minecraft:goat_horn", "instrument", "minecraft:instrument"),
        (
            "minecraft:debug_stick",
            "DebugProperty",
            "minecraft:debug_stick_state",
        ),
        (
            "minecraft:suspicious_stew",
            "effects",
            "minecraft:suspicious_stew_effects",
        ),
    ] {
        let value = if old == "instrument" {
            text("minecraft:ponder_goat_horn")
        } else {
            sequence(vec![empty()])
        };
        let mut stack = item(id, compound(vec![(old, value.clone())]));
        run(&mut stack, NodeKind::Item, 3818).unwrap();
        assert_eq!(component(&stack, new), &value);
    }
    let mut bucket = item(
        "minecraft:axolotl_bucket",
        compound(vec![
            ("Variant", Tag::Int(3)),
            ("Health", Tag::Float(9.0)),
            ("opaque", Tag::Int(4)),
        ]),
    );
    run(&mut bucket, NodeKind::Item, 3818).unwrap();
    assert_eq!(
        get(
            component(&bucket, "minecraft:bucket_entity_data"),
            "Variant"
        ),
        Some(&Tag::Int(3))
    );
    assert_eq!(
        get(component(&bucket, "minecraft:custom_data"), "opaque"),
        Some(&Tag::Int(4))
    );
}

#[test]
fn componentization_potion_map_books_and_fireworks() {
    let mut potion = item(
        "minecraft:potion",
        compound(vec![
            ("Potion", text("minecraft:empty")),
            ("CustomPotionColor", Tag::Int(17)),
            (
                "custom_potion_effects",
                sequence(vec![compound(vec![("id", text("minecraft:speed"))])]),
            ),
        ]),
    );
    run(&mut potion, NodeKind::Item, 3818).unwrap();
    assert!(get(component(&potion, "minecraft:potion_contents"), "potion").is_none());
    assert_eq!(
        get(
            component(&potion, "minecraft:potion_contents"),
            "custom_color"
        ),
        Some(&Tag::Int(17))
    );
    let mut map = item(
        "minecraft:filled_map",
        compound(vec![
            ("map", Tag::Int(12)),
            (
                "Decorations",
                sequence(vec![compound(vec![
                    ("id", text("target")),
                    ("type", Tag::Byte(26)),
                    ("x", Tag::Double(1.0)),
                ])]),
            ),
        ]),
    );
    run(&mut map, NodeKind::Item, 3818).unwrap();
    assert_eq!(component(&map, "minecraft:map_id"), &Tag::Int(12));
    assert_eq!(
        get(
            get(component(&map, "minecraft:map_decorations"), "target").unwrap(),
            "type"
        ),
        Some(&text("red_x"))
    );
    let mut book = item(
        "minecraft:written_book",
        compound(vec![
            ("title", text("Title")),
            ("filtered_title", text("Filtered")),
            ("pages", sequence(vec![text("{\"text\":\"Page\"}")])),
            (
                "filtered_pages",
                compound(vec![("0", text("{\"text\":\"Safe\"}"))]),
            ),
            ("author", text("Author")),
        ]),
    );
    run(&mut book, NodeKind::Item, 3818).unwrap();
    let content = component(&book, "minecraft:written_book_content");
    assert_eq!(
        get(get(content, "title").unwrap(), "filtered"),
        Some(&text("Filtered"))
    );
    assert_eq!(
        get(&list(get(content, "pages").unwrap()).unwrap()[0], "raw"),
        Some(&text("{\"text\":\"Page\"}"))
    );
    let mut rocket = item(
        "minecraft:firework_rocket",
        compound(vec![(
            "Fireworks",
            compound(vec![
                ("Flight", Tag::Byte(2)),
                (
                    "Explosions",
                    sequence(vec![compound(vec![
                        ("Type", Tag::Byte(3)),
                        ("Colors", Tag::IntArray(vec![1, 2])),
                        ("Trail", Tag::Byte(1)),
                    ])]),
                ),
                ("opaque", Tag::Int(5)),
            ]),
        )]),
    );
    run(&mut rocket, NodeKind::Item, 3818).unwrap();
    assert_eq!(
        get(component(&rocket, "minecraft:fireworks"), "flight_duration"),
        Some(&Tag::Byte(2))
    );
    assert_eq!(
        get(
            &list(get(component(&rocket, "minecraft:fireworks"), "explosions").unwrap()).unwrap()
                [0],
            "shape"
        ),
        Some(&text("creeper"))
    );
    assert_eq!(
        get(
            get(component(&rocket, "minecraft:custom_data"), "Fireworks").unwrap(),
            "opaque"
        ),
        Some(&Tag::Int(5))
    );
}

#[test]
fn componentization_profile_predicates_and_attribute_schema() {
    let mut stack = item(
        "minecraft:player_head",
        compound(vec![
            (
                "SkullOwner",
                compound(vec![
                    ("Name", text("Player")),
                    ("Id", Tag::IntArray(vec![1, 2, 3, 4])),
                    (
                        "Properties",
                        compound(vec![(
                            "textures",
                            sequence(vec![compound(vec![
                                ("Value", text("skin")),
                                ("Signature", text("sig")),
                            ])]),
                        )]),
                    ),
                ]),
            ),
            (
                "CanDestroy",
                sequence(vec![text("minecraft:stone[axis=y]{a:1}")]),
            ),
            (
                "BlockStateTag",
                compound(vec![("waterlogged", Tag::Byte(1)), ("age", Tag::Int(3))]),
            ),
            (
                "AttributeModifiers",
                sequence(vec![compound(vec![
                    ("AttributeName", text("minecraft:generic.attack_damage")),
                    ("Operation", Tag::Int(2)),
                    ("Amount", Tag::Double(5.0)),
                ])]),
            ),
        ]),
    );
    run(&mut stack, NodeKind::Item, 3818).unwrap();
    assert_eq!(
        get(component(&stack, "minecraft:profile"), "name"),
        Some(&text("Player"))
    );
    assert_eq!(
        get(component(&stack, "minecraft:block_state"), "waterlogged"),
        Some(&text("true"))
    );
    let predicate =
        &list(get(component(&stack, "minecraft:can_break"), "predicates").unwrap()).unwrap()[0];
    assert_eq!(get(predicate, "nbt"), Some(&text("{a:1}")));
    assert_eq!(
        get(get(predicate, "state").unwrap(), "axis"),
        Some(&text("y"))
    );
    let modifier = &list(
        get(
            component(&stack, "minecraft:attribute_modifiers"),
            "modifiers",
        )
        .unwrap(),
    )
    .unwrap()[0];
    assert_eq!(
        get(modifier, "operation"),
        Some(&text("add_multiplied_total"))
    );
    assert_eq!(profile(text("Invalid name")), empty());
}

#[test]
fn componentization_rejects_exact_unsupported_fields() {
    for legacy in [
        Tag::String("not compound".into()),
        compound(vec![(
            "BlockStateTag",
            compound(vec![("age", Tag::Double(1.0))]),
        )]),
        compound(vec![("BlockEntityTag", Tag::Int(8))]),
    ] {
        let mut stack = item("minecraft:stone", legacy);
        assert!(run(&mut stack, NodeKind::Item, 3818).is_err());
    }
    let mut conflicting = item("minecraft:stone", empty());
    set(&mut conflicting, "components", empty());
    assert!(run(&mut conflicting, NodeKind::Item, 3818).is_err());
}

#[test]
fn lodestone_component_3820_and_legacy_tracker() {
    let target = compound(vec![
        ("pos", Tag::IntArray(vec![1, 2, 3])),
        ("dimension", text("minecraft:overworld")),
        ("tracked", Tag::Byte(0)),
    ]);
    let mut data = compound(vec![("minecraft:lodestone_target", target)]);
    run(&mut data, NodeKind::Components, 3820).unwrap();
    let tracker = get(&data, "minecraft:lodestone_tracker").unwrap();
    assert!(get(tracker, "pos").is_none());
    assert_eq!(get(tracker, "tracked"), Some(&Tag::Byte(0)));
    assert_eq!(
        get(get(tracker, "target").unwrap(), "pos"),
        Some(&Tag::IntArray(vec![1, 2, 3]))
    );
    let mut compass = item(
        "minecraft:compass",
        compound(vec![
            ("LodestonePos", Tag::IntArray(vec![1, 2, 3])),
            ("LodestoneDimension", text("minecraft:overworld")),
        ]),
    );
    run(&mut compass, NodeKind::Item, 3818).unwrap();
    assert!(get(component(&compass, "minecraft:lodestone_tracker"), "target").is_some());
}

#[test]
fn name_override_3825_and_banner_rarity_4054() {
    let mut banner = compound(vec![
        ("id", text("minecraft:white_banner")),
        (
            "components",
            compound(vec![(
                "minecraft:custom_name",
                translatable("block.minecraft.ominous_banner"),
            )]),
        ),
    ]);
    run(&mut banner, NodeKind::Item, 3825).unwrap();
    assert!(get(components(&banner), "minecraft:custom_name").is_none());
    run(&mut banner, NodeKind::Item, 4054).unwrap();
    assert_eq!(component(&banner, "minecraft:rarity"), &text("uncommon"));
    let mut map = compound(vec![
        ("id", text("minecraft:filled_map")),
        (
            "components",
            compound(vec![(
                "minecraft:custom_name",
                translatable("filled_map.mansion"),
            )]),
        ),
    ]);
    run(&mut map, NodeKind::Item, 3825).unwrap();
    assert!(get(components(&map), "minecraft:item_name").is_some());
    for version in [3825, 4054] {
        let key = if version == 3825 {
            "minecraft:custom_name"
        } else {
            "minecraft:item_name"
        };
        let mut invalid = compound(vec![
            ("id", text("minecraft:white_banner")),
            (
                "components",
                compound(vec![(
                    key,
                    text("{translate:'block.minecraft.ominous_banner'}"),
                )]),
            ),
        ]);
        assert!(run(&mut invalid, NodeKind::Item, version).is_err());
    }
}

#[test]
fn attribute_modifier_id_3945_merges_named_and_deduplicates_uuid() {
    let mut data = compound(vec![(
        "minecraft:attribute_modifiers",
        compound(vec![(
            "modifiers",
            sequence(vec![
                compound(vec![
                    ("name", text("Random spawn bonus")),
                    ("amount", Tag::Double(2.0)),
                ]),
                compound(vec![
                    ("name", text("Random spawn bonus")),
                    ("amount", Tag::Double(3.0)),
                ]),
                compound(vec![
                    ("uuid", Tag::IntArray(vec![1, 2, 3, 4])),
                    ("name", text("custom")),
                ]),
            ]),
        )]),
    )]);
    run(&mut data, NodeKind::Components, 3945).unwrap();
    let modifiers = list(
        get(
            get(&data, "minecraft:attribute_modifiers").unwrap(),
            "modifiers",
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(modifiers.len(), 2);
    assert_eq!(
        get(&modifiers[0], "id"),
        Some(&text("minecraft:random_spawn_bonus"))
    );
    assert_eq!(get(&modifiers[0], "amount"), Some(&Tag::Double(5.0)));
    assert_eq!(
        get(&modifiers[1], "id"),
        Some(&text("minecraft:00000001-0000-0002-0000-000300000004"))
    );
    assert!(get(&modifiers[1], "uuid").is_none());
    for (uuid, id) in tables::MODIFIER_IDS {
        let compact = uuid.replace('-', "");
        let numeric = u128::from_str_radix(&compact, 16).unwrap();
        let parts = (0..4)
            .map(|i| (numeric >> (96 - i * 32)) as u32 as i32)
            .collect();
        let mut modifiers = sequence(vec![compound(vec![("uuid", Tag::IntArray(parts))])]);
        fix_modifiers(&mut modifiers).unwrap();
        assert_eq!(get(&list(&modifiers).unwrap()[0], "id"), Some(&text(id)));
    }
}

#[test]
fn attribute_type_prefix_4055_preserves_mod_namespace() {
    let mut data = compound(vec![(
        "minecraft:attribute_modifiers",
        compound(vec![(
            "modifiers",
            sequence(vec![
                compound(vec![("type", text("minecraft:generic.attack_damage"))]),
                compound(vec![("type", text("mod:generic.attack_damage"))]),
            ]),
        )]),
    )]);
    run(&mut data, NodeKind::Components, 4055).unwrap();
    let modifiers = list(
        get(
            get(&data, "minecraft:attribute_modifiers").unwrap(),
            "modifiers",
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        get(&modifiers[0], "type"),
        Some(&text("minecraft:attack_damage"))
    );
    assert_eq!(
        get(&modifiers[1], "type"),
        Some(&text("mod:generic.attack_damage"))
    );
}

#[test]
fn food_4059_splits_consumable_and_item_remainder() {
    let remainder = compound(vec![("id", text("minecraft:bowl")), ("count", Tag::Int(1))]);
    let mut data = compound(vec![(
        "minecraft:food",
        compound(vec![
            ("nutrition", Tag::Int(4)),
            ("eat_seconds", Tag::Float(2.0)),
            ("using_converts_to", remainder.clone()),
            (
                "effects",
                sequence(vec![compound(vec![(
                    "effect",
                    compound(vec![("id", text("minecraft:speed"))]),
                )])]),
            ),
        ]),
    )]);
    run(&mut data, NodeKind::Components, 4059).unwrap();
    assert_eq!(get(&data, "minecraft:use_remainder"), Some(&remainder));
    assert!(get(get(&data, "minecraft:food").unwrap(), "eat_seconds").is_none());
    let consumable = get(&data, "minecraft:consumable").unwrap();
    assert_eq!(get(consumable, "consume_seconds"), Some(&Tag::Float(2.0)));
    assert_eq!(
        get(
            &list(get(consumable, "on_consume_effects").unwrap()).unwrap()[0],
            "probability"
        ),
        Some(&Tag::Float(1.0))
    );
}

#[test]
fn fire_resistance_4064_and_lock_predicate_4068() {
    let mut data = compound(vec![
        ("minecraft:fire_resistant", empty()),
        ("minecraft:lock", text("a\"b\\c")),
    ]);
    run(&mut data, NodeKind::Components, 4064).unwrap();
    assert!(get(&data, "minecraft:fire_resistant").is_none());
    assert_eq!(
        get(get(&data, "minecraft:damage_resistant").unwrap(), "types"),
        Some(&text("#minecraft:is_fire"))
    );
    run(&mut data, NodeKind::Components, 4068).unwrap();
    let name = get(
        get(get(&data, "minecraft:lock").unwrap(), "components").unwrap(),
        "minecraft:custom_name",
    )
    .unwrap();
    assert_eq!(name, &text("\"a\\\"b\\\\c\""));
    let mut empty_lock = compound(vec![("minecraft:lock", text(""))]);
    run(&mut empty_lock, NodeKind::Components, 4068).unwrap();
    assert!(get(&empty_lock, "minecraft:lock").is_none());
}

#[test]
fn asset_model_data_4175_and_invalid_lock_4176() {
    let mut data = compound(vec![
        (
            "minecraft:equippable",
            compound(vec![("model", text("minecraft:diamond"))]),
        ),
        ("minecraft:custom_model_data", Tag::Int(25)),
        (
            "minecraft:lock",
            compound(vec![(
                "components",
                compound(vec![("minecraft:custom_name", text("\"\""))]),
            )]),
        ),
    ]);
    run(&mut data, NodeKind::Components, 4175).unwrap();
    assert_eq!(
        get(get(&data, "minecraft:equippable").unwrap(), "asset_id"),
        Some(&text("minecraft:diamond"))
    );
    assert_eq!(
        get(get(&data, "minecraft:custom_model_data").unwrap(), "floats"),
        Some(&sequence(vec![Tag::Float(25.0)]))
    );
    run(&mut data, NodeKind::Components, 4176).unwrap();
    assert!(get(&data, "minecraft:lock").is_none());
}

#[test]
fn bucket_and_painting_variants_4299() {
    for (id, old, value, new, expected) in [
        (
            "minecraft:axolotl_bucket",
            "Variant",
            Tag::Int(4),
            "minecraft:axolotl/variant",
            text("blue"),
        ),
        (
            "minecraft:salmon_bucket",
            "type",
            text("large"),
            "minecraft:salmon/size",
            text("large"),
        ),
        (
            "minecraft:tropical_fish_bucket",
            "BucketVariantTag",
            Tag::Int(0x04030101),
            "minecraft:tropical_fish/pattern",
            text("stripey"),
        ),
    ] {
        let mut stack = compound(vec![
            ("id", text(id)),
            (
                "components",
                compound(vec![(
                    "minecraft:bucket_entity_data",
                    compound(vec![(old, value), ("Health", Tag::Float(10.0))]),
                )]),
            ),
        ]);
        run(&mut stack, NodeKind::Item, 4299).unwrap();
        assert_eq!(component(&stack, new), &expected);
        assert!(get(component(&stack, "minecraft:bucket_entity_data"), old).is_none());
        assert_eq!(
            get(component(&stack, "minecraft:bucket_entity_data"), "Health"),
            Some(&Tag::Float(10.0))
        );
    }
    let mut painting = compound(vec![
        ("id", text("minecraft:painting")),
        (
            "components",
            compound(vec![(
                "minecraft:entity_data",
                compound(vec![
                    ("id", text("minecraft:painting")),
                    ("variant", text("minecraft:kebab")),
                ]),
            )]),
        ),
    ]);
    run(&mut painting, NodeKind::Item, 4299).unwrap();
    assert_eq!(
        component(&painting, "minecraft:painting/variant"),
        &text("minecraft:kebab")
    );
    assert!(get(components(&painting), "minecraft:entity_data").is_none());
}

#[test]
fn tooltip_4307_unwraps_and_records_hidden_components() {
    let mut data = compound(vec![
        (
            "minecraft:enchantments",
            hidden(
                compound(vec![(
                    "levels",
                    compound(vec![("minecraft:sharpness", Tag::Int(4))]),
                )]),
                true,
            ),
        ),
        (
            "minecraft:can_break",
            hidden(
                compound(vec![(
                    "predicates",
                    sequence(vec![compound(vec![("blocks", text("minecraft:stone"))])]),
                )]),
                true,
            ),
        ),
        ("minecraft:hide_additional_tooltip", empty()),
        ("minecraft:map_id", Tag::Int(3)),
        ("minecraft:hide_tooltip", empty()),
    ]);
    run(&mut data, NodeKind::Components, 4307).unwrap();
    assert_eq!(
        get(
            get(&data, "minecraft:enchantments").unwrap(),
            "minecraft:sharpness"
        ),
        Some(&Tag::Int(4))
    );
    assert!(list(get(&data, "minecraft:can_break").unwrap()).is_some());
    let display = get(&data, "minecraft:tooltip_display").unwrap();
    assert_eq!(get(display, "hide_tooltip"), Some(&Tag::Byte(1)));
    let hidden = list(get(display, "hidden_components").unwrap()).unwrap();
    for key in [
        "minecraft:enchantments",
        "minecraft:can_break",
        "minecraft:map_id",
    ] {
        assert!(hidden.contains(&text(key)));
    }
    assert!(get(&data, "minecraft:hide_additional_tooltip").is_none());
}

#[test]
fn trident_animation_4649_only_changes_spear() {
    for old in ["spear", "eat", "mod:custom"] {
        let mut data = compound(vec![(
            "minecraft:consumable",
            compound(vec![("animation", text(old))]),
        )]);
        run(&mut data, NodeKind::Components, 4649).unwrap();
        assert_eq!(
            get(get(&data, "minecraft:consumable").unwrap(), "animation"),
            Some(&text(if old == "spear" { "trident" } else { old }))
        );
    }
}

#[test]
fn potion_effect_steps_3322_and_3568_use_shared_vanilla_helpers() {
    let mut potion = item(
        "minecraft:potion",
        compound(vec![(
            "CustomPotionEffects",
            sequence(vec![compound(vec![
                ("Id", Tag::Byte(1)),
                ("Duration", Tag::Int(30)),
                (
                    "FactorCalculationData",
                    compound(vec![("effect_changed_timestamp", Tag::Int(100))]),
                ),
            ])]),
        )]),
    );
    run(&mut potion, NodeKind::Item, 3322).unwrap();
    let effect =
        &list(get(get(&potion, "tag").unwrap(), "CustomPotionEffects").unwrap()).unwrap()[0];
    assert_eq!(
        get(
            get(effect, "FactorCalculationData").unwrap(),
            "ticks_active"
        ),
        Some(&Tag::Int(70))
    );
    run(&mut potion, NodeKind::Item, 3568).unwrap();
    let effect =
        &list(get(get(&potion, "tag").unwrap(), "custom_potion_effects").unwrap()).unwrap()[0];
    assert_eq!(get(effect, "id"), Some(&text("minecraft:speed")));
    assert_eq!(get(effect, "duration"), Some(&Tag::Int(30)));
    let mut invalid = item(
        "minecraft:potion",
        compound(vec![(
            "CustomPotionEffects",
            sequence(vec![compound(vec![("Id", Tag::Int(999))])]),
        )]),
    );
    assert!(run(&mut invalid, NodeKind::Item, 3568).is_err());
    let mut stew = item(
        "minecraft:suspicious_stew",
        compound(vec![(
            "Effects",
            sequence(vec![compound(vec![
                ("EffectId", Tag::Byte(1)),
                ("EffectDuration", Tag::Int(20)),
            ])]),
        )]),
    );
    run(&mut stew, NodeKind::Item, 3568).unwrap();
    let effect = &list(get(get(&stew, "tag").unwrap(), "effects").unwrap()).unwrap()[0];
    assert_eq!(get(effect, "id"), Some(&text("minecraft:speed")));
    assert_eq!(get(effect, "duration"), Some(&Tag::Int(20)));
}

#[test]
fn unsupported_fields_report_exact_fix_version_and_path() {
    let mut stack = item(
        "minecraft:stone",
        compound(vec![(
            "BlockStateTag",
            compound(vec![("age", Tag::Double(1.0))]),
        )]),
    );
    let error = run(&mut stack, NodeKind::Item, 3818).unwrap_err();
    assert!(matches!(error, UpgradeError::UnsupportedField {
        version: 3818, fix: COMPONENTIZE, ref path, ref reason
    } if path == "tag.BlockStateTag.age" && reason.contains("floating-point")));
    let mut banner = compound(vec![
        ("id", text("minecraft:white_banner")),
        (
            "components",
            compound(vec![(
                "minecraft:custom_name",
                text("{translate:'block.minecraft.ominous_banner'}"),
            )]),
        ),
    ]);
    let error = run(&mut banner, NodeKind::Item, 3825).unwrap_err();
    assert!(matches!(error, UpgradeError::UnsupportedField {
        version: 3825, fix: "ItemStackCustomNameToOverrideComponentFix", ref path, ref reason
    } if path == "components.minecraft:custom_name/item_name" && reason.contains("lenient")));
}

#[test]
fn schema_hook_uses_exact_block_maps_and_overwrites_discriminator() {
    for (id, expected) in schema::BLOCK_ENTITY_IDS {
        let mut stack = item(
            id,
            compound(vec![(
                "BlockEntityTag",
                compound(vec![("id", text("mod:previous")), ("opaque", Tag::Int(7))]),
            )]),
        );
        prepare_legacy_item(&mut stack);
        let block = get(get(&stack, "tag").unwrap(), "BlockEntityTag").unwrap();
        assert_eq!(get(block, "id"), Some(&text(expected)));
        assert_eq!(get(block, "opaque"), Some(&Tag::Int(7)));
    }
    for id in [
        "mod:oak_sign",
        "minecraft:oak_hanging_sign",
        "minecraft:chiseled_bookshelf",
        "minecraft:mangrove_sign",
    ] {
        let mut stack = item(
            id,
            compound(vec![(
                "BlockEntityTag",
                compound(vec![("opaque", Tag::Int(7))]),
            )]),
        );
        let original = stack.clone();
        prepare_legacy_item(&mut stack);
        assert_eq!(stack, original);
    }
}

#[test]
fn schema_hook_infers_entity_id_only_when_absent_and_never_walks_custom_data() {
    for (id, expected) in schema::ENTITY_IDS {
        let mut stack = item(
            id,
            compound(vec![("EntityTag", compound(vec![("opaque", Tag::Int(7))]))]),
        );
        prepare_legacy_item(&mut stack);
        let entity = get(get(&stack, "tag").unwrap(), "EntityTag").unwrap();
        assert_eq!(get(entity, "id"), Some(&text(expected)));
        let mut explicit = item(
            id,
            compound(vec![(
                "EntityTag",
                compound(vec![("id", text("mod:explicit"))]),
            )]),
        );
        let original = explicit.clone();
        prepare_legacy_item(&mut explicit);
        assert_eq!(explicit, original);
    }
    let mut modern = compound(vec![
        ("id", text("minecraft:oak_sign")),
        (
            "components",
            compound(vec![(
                "minecraft:custom_data",
                compound(vec![("BlockEntityTag", empty()), ("EntityTag", empty())]),
            )]),
        ),
    ]);
    let original = modern.clone();
    prepare_legacy_item(&mut modern);
    assert_eq!(modern, original);
}
