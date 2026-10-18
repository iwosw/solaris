//! Failing-first regression tests for legacy item-stack fixers, one band per
//! historical fixer. Fixtures are synthetic (hand-built legacy NBT), never Mojang
//! bytes. Each test drives `super::fix_node` directly at its schema version.

use super::super::{NodeKind, UpgradeReport, nbt::*};
use mc_nbt::Tag;

fn report() -> UpgradeReport {
    UpgradeReport::default()
}

fn item(fields: Vec<(&str, Tag)>) -> Tag {
    compound(fields)
}

fn text(value: &str) -> Tag {
    Tag::String(value.into())
}

fn id_item(id: &str) -> Tag {
    item(vec![("id", Tag::String(id.into()))])
}

fn id_item_damage(id: &str, damage: i16) -> Tag {
    item(vec![
        ("id", Tag::String(id.into())),
        ("Damage", Tag::Short(damage)),
    ])
}

#[test]
fn v102_numeric_id_maps_and_unknown_is_rejected() {
    let mut stack = item(vec![("id", Tag::Short(1))]);
    super::fix_node(&mut stack, NodeKind::Item, 102, &mut report()).unwrap();
    assert_eq!(get(&stack, "id").and_then(string), Some("minecraft:stone"));
    let mut unknown = item(vec![("id", Tag::Short(30000))]);
    let error = super::fix_node(&mut unknown, NodeKind::Item, 102, &mut report()).unwrap_err();
    assert!(error.to_string().contains("id"));
    assert_eq!(get(&unknown, "id"), Some(&Tag::Short(30000)));
    // String ids pass through untouched.
    let mut named = id_item("minecraft:stone");
    super::fix_node(&mut named, NodeKind::Item, 102, &mut report()).unwrap();
    assert_eq!(get(&named, "id").and_then(string), Some("minecraft:stone"));
}

#[test]
fn v102_potion_damage_selects_potion_and_splash_flag() {
    let mut stack = item(vec![
        ("id", Tag::String("minecraft:potion".into())),
        ("Damage", Tag::Short(1)),
        ("tag", compound(vec![])),
    ]);
    let mut rep = report();
    super::fix_node(&mut stack, NodeKind::Item, 102, &mut rep).unwrap();
    let legacy = get(&stack, "tag").unwrap();
    assert_eq!(
        get(legacy, "Potion").and_then(string),
        Some("minecraft:regeneration")
    );
    assert_eq!(get(&stack, "Damage"), Some(&Tag::Short(0)));
    assert!(rep.steps.iter().any(|s| s.version == 102));
    // Splash bit rewrites the id.
    let mut splash = item(vec![
        ("id", Tag::String("minecraft:potion".into())),
        ("Damage", Tag::Short(16385)),
        ("tag", compound(vec![])),
    ]);
    super::fix_node(&mut splash, NodeKind::Item, 102, &mut report()).unwrap();
    assert_eq!(
        get(&splash, "id").and_then(string),
        Some("minecraft:splash_potion")
    );
}

#[test]
fn v105_spawn_egg_damage_names_entity_tag_only() {
    let mut stack = item(vec![
        ("id", Tag::String("minecraft:spawn_egg".into())),
        ("Damage", Tag::Short(50)),
        ("tag", compound(vec![])),
    ]);
    super::fix_node(&mut stack, NodeKind::Item, 105, &mut report()).unwrap();
    // Item id is untouched at 105; only EntityTag.id is written (legacy name).
    assert_eq!(
        get(&stack, "id").and_then(string),
        Some("minecraft:spawn_egg")
    );
    let entity = get(get(&stack, "tag").unwrap(), "EntityTag").unwrap();
    assert_eq!(get(entity, "id").and_then(string), Some("Creeper"));
    assert_eq!(get(&stack, "Damage"), Some(&Tag::Short(0)));
    // Unmapped damage leaves the stack alone.
    let mut plain = id_item_damage("minecraft:spawn_egg", 3);
    let before = plain.clone();
    super::fix_node(&mut plain, NodeKind::Item, 105, &mut report()).unwrap();
    assert_eq!(plain, before);
}

#[test]
fn rename_bands_rewrite_exact_items_only() {
    for (version, old, new) in [
        (502, "minecraft:cooked_fished", "minecraft:cooked_fish"),
        (820, "minecraft:totem", "minecraft:totem_of_undying"),
        (1484, "minecraft:sea_grass", "minecraft:seagrass"),
        (
            1490,
            "minecraft:speckled_melon",
            "minecraft:glistering_melon_slice",
        ),
        (1510, "minecraft:clownfish", "minecraft:tropical_fish"),
        (1800, "minecraft:rose_red", "minecraft:red_dye"),
        (1802, "minecraft:sign", "minecraft:oak_sign"),
        (
            1928,
            "minecraft:illager_beast_spawn_egg",
            "minecraft:ravager_spawn_egg",
        ),
        (2209, "minecraft:bee_hive", "minecraft:beehive"),
        (2508, "minecraft:warped_fungi", "minecraft:warped_fungus"),
        (
            2509,
            "minecraft:zombie_pigman_spawn_egg",
            "minecraft:zombified_piglin_spawn_egg",
        ),
        (2528, "minecraft:soul_fire_torch", "minecraft:soul_torch"),
    ] {
        let mut stack = id_item(old);
        let mut rep = report();
        super::fix_node(&mut stack, NodeKind::Item, version, &mut rep).unwrap();
        assert_eq!(get(&stack, "id").and_then(string), Some(new), "v{version}");
        assert!(rep.steps.iter().any(|s| s.version == version));
    }
    // Non-minecraft namespaces never rewrite.
    let mut foreign = id_item("mod:sign");
    super::fix_node(&mut foreign, NodeKind::Item, 1802, &mut report()).unwrap();
    assert_eq!(get(&foreign, "id").and_then(string), Some("mod:sign"));
}

#[test]
fn v804_banner_base_moves_to_damage_and_drops() {
    let mut stack = item(vec![
        ("id", Tag::String("minecraft:banner".into())),
        (
            "tag",
            compound(vec![(
                "BlockEntityTag",
                compound(vec![("Base", Tag::Int(3))]),
            )]),
        ),
    ]);
    super::fix_node(&mut stack, NodeKind::Item, 804, &mut report()).unwrap();
    assert_eq!(get(&stack, "Damage"), Some(&Tag::Short(3)));
    let block = get(get(&stack, "tag").unwrap(), "BlockEntityTag").unwrap();
    assert!(get(block, "Base").is_none());
}

#[test]
fn v806_water_potion_defaults_missing_potion() {
    let mut stack = item(vec![
        ("id", Tag::String("minecraft:potion".into())),
        ("tag", compound(vec![])),
    ]);
    super::fix_node(&mut stack, NodeKind::Item, 806, &mut report()).unwrap();
    assert_eq!(
        get(get(&stack, "tag").unwrap(), "Potion").and_then(string),
        Some("minecraft:water")
    );
    // Present values stay.
    let mut keep = item(vec![
        ("id", Tag::String("minecraft:potion".into())),
        (
            "tag",
            compound(vec![("Potion", Tag::String("minecraft:healing".into()))]),
        ),
    ]);
    let before = keep.clone();
    super::fix_node(&mut keep, NodeKind::Item, 806, &mut report()).unwrap();
    assert_eq!(keep, before);
}

#[test]
fn v813_shulker_color_selects_id_and_drops_color() {
    let mut stack = item(vec![
        ("id", Tag::String("minecraft:shulker_box".into())),
        (
            "tag",
            compound(vec![(
                "BlockEntityTag",
                compound(vec![("Color", Tag::Byte(3))]),
            )]),
        ),
    ]);
    super::fix_node(&mut stack, NodeKind::Item, 813, &mut report()).unwrap();
    assert_eq!(
        get(&stack, "id").and_then(string),
        Some("minecraft:light_blue_shulker_box")
    );
    let block = get(get(&stack, "tag").unwrap(), "BlockEntityTag").unwrap();
    assert!(get(block, "Color").is_none());
}

#[test]
fn v1125_bed_without_damage_becomes_red() {
    let mut stack = id_item_damage("minecraft:bed", 0);
    super::fix_node(&mut stack, NodeKind::Item, 1125, &mut report()).unwrap();
    assert_eq!(get(&stack, "Damage"), Some(&Tag::Short(14)));
    let mut damaged = id_item_damage("minecraft:bed", 5);
    let before = damaged.clone();
    super::fix_node(&mut damaged, NodeKind::Item, 1125, &mut report()).unwrap();
    assert_eq!(damaged, before);
}

#[test]
fn v1451_flattening_rewrites_variant_and_moves_map_id() {
    // stone.1 -> granite; root Damage always drops for non-DAMAGE_IDS.
    let mut stack = id_item_damage("minecraft:stone", 1);
    super::fix_node(&mut stack, NodeKind::Item, 1451, &mut report()).unwrap();
    assert_eq!(
        get(&stack, "id").and_then(string),
        Some("minecraft:granite")
    );
    assert!(get(&stack, "Damage").is_none());
    // filled_map r3 copies Damage -> tag.map.
    let mut map = id_item_damage("minecraft:filled_map", 7);
    super::fix_node(&mut map, NodeKind::Item, 1451, &mut report()).unwrap();
    assert_eq!(get(get(&map, "tag").unwrap(), "map"), Some(&Tag::Int(7)));
    assert!(get(&map, "Damage").is_none());
    // Tools keep damage as tag.Damage.
    let mut bow = id_item_damage("minecraft:bow", 12);
    super::fix_node(&mut bow, NodeKind::Item, 1451, &mut report()).unwrap();
    assert_eq!(
        get(get(&bow, "tag").unwrap(), "Damage"),
        Some(&Tag::Int(12))
    );
    assert!(get(&bow, "Damage").is_none());
}

#[test]
fn v1451_spawn_egg_and_block_entity_drop_branches() {
    let mut stack = item(vec![
        ("id", Tag::String("minecraft:spawn_egg".into())),
        (
            "tag",
            compound(vec![(
                "EntityTag",
                compound(vec![("id", Tag::String("minecraft:creeper".into()))]),
            )]),
        ),
    ]);
    super::fix_node(&mut stack, NodeKind::Item, 1451, &mut report()).unwrap();
    assert_eq!(
        get(&stack, "id").and_then(string),
        Some("minecraft:creeper_spawn_egg")
    );
    let mut note = item(vec![
        ("id", Tag::String("minecraft:noteblock".into())),
        (
            "tag",
            compound(vec![("BlockEntityTag", compound(vec![("id", text("x"))]))]),
        ),
    ]);
    super::fix_node(&mut note, NodeKind::Item, 1451, &mut report()).unwrap();
    assert!(get(get(&note, "tag").unwrap(), "BlockEntityTag").is_none());
}

#[test]
fn v1494_ench_list_renames_and_moves() {
    let mut stack = item(vec![(
        "tag",
        compound(vec![(
            "ench",
            Tag::List(mc_nbt::ListTag {
                element_type: 10,
                elements: vec![compound(vec![
                    ("id", Tag::Short(16)),
                    ("lvl", Tag::Short(3)),
                ])],
            }),
        )]),
    )]);
    super::fix_node(&mut stack, NodeKind::Item, 1494, &mut report()).unwrap();
    let legacy = get(&stack, "tag").unwrap();
    assert!(get(legacy, "ench").is_none());
    let entries = list(get(legacy, "Enchantments").unwrap()).unwrap();
    assert_eq!(
        get(&entries[0], "id").and_then(string),
        Some("minecraft:sharpness")
    );
}

#[test]
fn v1803_lore_wraps_plain_and_json_looking_literals() {
    let mut stack = item(vec![(
        "tag",
        compound(vec![(
            "display",
            compound(vec![(
                "Lore",
                Tag::List(mc_nbt::ListTag {
                    element_type: 8,
                    elements: vec![
                        Tag::String("plain line".into()),
                        Tag::String("{\"text\":\"rich\"}".into()),
                    ],
                }),
            )]),
        )]),
    )]);
    super::fix_node(&mut stack, NodeKind::Item, 1803, &mut report()).unwrap();
    let display = get(get(&stack, "tag").unwrap(), "display").unwrap();
    let lines = list(get(display, "Lore").unwrap()).unwrap();
    assert_eq!(lines[0], Tag::String("{\"text\":\"plain line\"}".into()));
    assert_eq!(
        lines[1],
        Tag::String("{\"text\":\"{\\\"text\\\":\\\"rich\\\"}\"}".into())
    );
}

#[test]
fn v1904_cat_egg_becomes_ocelot_and_1948_renames_banner_text() {
    let mut cat = id_item("minecraft:cat_spawn_egg");
    super::fix_node(&mut cat, NodeKind::Item, 1904, &mut report()).unwrap();
    assert_eq!(
        get(&cat, "id").and_then(string),
        Some("minecraft:ocelot_spawn_egg")
    );
    let mut banner = item(vec![
        ("id", Tag::String("minecraft:white_banner".into())),
        (
            "tag",
            compound(vec![(
                "display",
                compound(vec![(
                    "Name",
                    Tag::String("{\"translate\":\"block.minecraft.illager_banner\"}".into()),
                )]),
            )]),
        ),
    ]);
    super::fix_node(&mut banner, NodeKind::Item, 1948, &mut report()).unwrap();
    assert_eq!(
        get(
            get(get(&banner, "tag").unwrap(), "display").unwrap(),
            "Name"
        )
        .and_then(string),
        Some("{\"translate\":\"block.minecraft.ominous_banner\"}")
    );
}

#[test]
fn v2501_trade_pumpkin_and_v2523_attribute_names() {
    let mut trade = item(vec![("id", Tag::String("minecraft:carved_pumpkin".into()))]);
    super::fix_node(&mut trade, NodeKind::Item, 2501, &mut report()).unwrap();
    assert_eq!(
        get(&trade, "id").and_then(string),
        Some("minecraft:carved_pumpkin")
    );
    let mut stack = item(vec![(
        "tag",
        compound(vec![(
            "AttributeModifiers",
            Tag::List(mc_nbt::ListTag {
                element_type: 10,
                elements: vec![compound(vec![(
                    "AttributeName",
                    Tag::String("generic.maxHealth".into()),
                )])],
            }),
        )]),
    )]);
    super::fix_node(&mut stack, NodeKind::Item, 2523, &mut report()).unwrap();
    let legacy = get(&stack, "tag").unwrap();
    let modifiers = list(get(legacy, "AttributeModifiers").unwrap()).unwrap();
    assert_eq!(
        get(&modifiers[0], "AttributeName").and_then(string),
        Some("minecraft:generic.max_health")
    );
}

#[test]
fn v2514_item_uuids_convert_modifiers_and_skull() {
    let mut stack = item(vec![
        ("id", Tag::String("minecraft:player_head".into())),
        (
            "tag",
            compound(vec![
                (
                    "AttributeModifiers",
                    Tag::List(mc_nbt::ListTag {
                        element_type: 10,
                        elements: vec![compound(vec![
                            ("UUIDMost", Tag::Long(1)),
                            ("UUIDLeast", Tag::Long(2)),
                        ])],
                    }),
                ),
                (
                    "SkullOwner",
                    compound(vec![(
                        "Id",
                        Tag::String("12345678-1234-1234-1234-123456789abc".into()),
                    )]),
                ),
            ]),
        ),
    ]);
    let mut rep = report();
    super::fix_node(&mut stack, NodeKind::Item, 2514, &mut rep).unwrap();
    let legacy = get(&stack, "tag").unwrap();
    let modifiers = list(get(legacy, "AttributeModifiers").unwrap()).unwrap();
    assert!(get(&modifiers[0], "UUID").is_some());
    let owner = get(legacy, "SkullOwner").unwrap();
    assert!(matches!(get(owner, "Id"), Some(Tag::IntArray(_))));
    assert!(rep.steps.iter().any(|s| s.version == 2514));
}

#[test]
fn knowledge_book_recipes_remain_opaque_to_player_recipe_fixers() {
    let mut book = item(vec![
        ("id", Tag::String("minecraft:knowledge_book".into())),
        (
            "tag",
            compound(vec![(
                "Recipes",
                Tag::List(mc_nbt::ListTag {
                    element_type: 8,
                    elements: vec![Tag::String("minecraft:sugar".into())],
                }),
            )]),
        ),
    ]);
    super::fix_node(&mut book, NodeKind::Item, 2100, &mut report()).unwrap();
    let legacy = get(&book, "tag").unwrap();
    assert_eq!(
        list(get(legacy, "Recipes").unwrap()).unwrap()[0],
        Tag::String("minecraft:sugar".into())
    );
}

#[test]
fn non_item_nodes_and_unknown_versions_are_noops() {
    let mut entity = compound(vec![("id", text("minecraft:pig"))]);
    let before = entity.clone();
    super::fix_node(&mut entity, NodeKind::Entity, 102, &mut report()).unwrap();
    assert_eq!(entity, before);
    let mut stack = id_item("minecraft:stone");
    let before = stack.clone();
    super::fix_node(&mut stack, NodeKind::Item, 9999, &mut report()).unwrap();
    assert_eq!(stack, before);
}

#[test]
fn falcons_legacy_display_names_are_literal_components() {
    for name in ["named item", r#"{"text":"literal JSON"}"#] {
        let mut stack = item(vec![
            ("id", text("minecraft:stone")),
            (
                "tag",
                compound(vec![("display", compound(vec![("Name", text(name))]))]),
            ),
        ]);
        super::fix_node(&mut stack, NodeKind::Item, 1458, &mut report()).unwrap();
        let display = get(get(&stack, "tag").unwrap(), "display").unwrap();
        assert_eq!(
            get(display, "Name"),
            Some(&Tag::String(super::create_text_component_json(name)))
        );
    }
}

#[test]
fn falcons_legacy_banner_hook_uses_source_epoch() {
    for (version, expected) in [(510, "Banner"), (704, "minecraft:banner")] {
        let mut banner = item(vec![
            ("id", text("minecraft:banner")),
            ("tag", compound(vec![("BlockEntityTag", compound(vec![]))])),
        ]);
        super::prepare_legacy_item(&mut banner, version);
        let block = get(get(&banner, "tag").unwrap(), "BlockEntityTag").unwrap();
        assert_eq!(get(block, "id"), Some(&text(expected)));
    }
}

#[test]
fn legacy_item_lookup_helpers_supply_variant_zero_fallback() {
    assert_eq!(super::numeric_item(1), "minecraft:stone");
    assert_eq!(
        super::flattened_item("minecraft:stone", 1),
        Some("minecraft:granite")
    );
    assert_eq!(
        super::flattened_item("minecraft:stone", 999),
        Some("minecraft:stone")
    );
}
