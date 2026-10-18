use super::*;
use mc_data::item_components::{ItemFacts, ItemFactsTable};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::tags::TagsData;

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}

struct Data {
    items: ItemRegistry,
    facts: ItemFactsTable,
    tags: TagsData,
}

impl Data {
    fn new() -> Self {
        let names = [
            "book",
            "enchanted_book",
            "diamond_sword",
            "diamond_axe",
            "diamond_pickaxe",
            "golden_boots",
            "diamond_chestplate",
            "bow",
            "crossbow",
            "trident",
            "mace",
            "copper_spear",
            "wooden_spear",
            "stone",
            "shield",
            "shears",
            "fishing_rod",
        ];
        Self {
            items: ItemRegistry::from_report(
                &names
                    .iter()
                    .enumerate()
                    .map(|(index, name)| ItemReport {
                        id: id(name),
                        protocol_id: index as u32 + 1,
                    })
                    .collect::<Vec<_>>(),
            ),
            facts: ItemFactsTable::default(),
            tags: TagsData::default(),
        }
    }

    fn rules(&self) -> ItemRules<'_> {
        ItemRules {
            items: &self.items,
            facts: &self.facts,
            tags: &self.tags,
            recipes: &[],
        }
    }
    fn stack(&self, name: &str) -> ItemStack {
        ItemStack::new(self.items.id_of(&id(name)).unwrap(), 1)
    }
}

fn catalog() -> EnchantmentCatalog {
    EnchantmentCatalog::runtime()
}

#[test]
fn java_legacy_random_known_vectors() {
    let mut random = LegacyRandom::new(0);
    assert_eq!(random.next_i32(), -1_155_484_576);
    assert_eq!(random.next_i32(), -723_955_400);
    assert_eq!(random.next_i32(), 1_033_096_058);
    random.set_seed(0);
    assert_eq!(random.next_int(1_000), 360);
    assert_eq!(random.next_int(1_000), 948);
    assert_eq!(random.next_int(1_000), 29);
    random.set_seed(0);
    assert_eq!(random.next_float().to_bits(), 0.73096776_f32.to_bits());
    assert_eq!(next_seed(0), -1_155_484_576);
    assert_eq!(LegacyRandom::new(-1).next_i32(), 1_155_099_827);
    // This non-power-of-two bound rejects roughly half the 31-bit samples;
    // the next draw therefore also exercises Java's signed overflow rule.
    random.set_seed(0);
    assert_eq!(random.next_int(1_073_741_825), 516_548_029);
    assert_eq!(random.next_i32(), -1_690_734_402);
}

#[test]
fn full_registry_costs_curses_and_exclusivity_are_real_vanilla_data() {
    let catalog = catalog();
    assert_eq!(catalog.definitions.len(), 43);
    assert_eq!(catalog.table.len(), 36);
    for definition in &catalog.definitions {
        assert_eq!(
            mc_data::required_registry_entry_id("enchantment", &definition.id),
            Some(definition.protocol_id as u32)
        );
    }
    let sharpness = catalog.get(&id("sharpness")).unwrap();
    assert_eq!(
        (sharpness.max_level, sharpness.anvil_cost, sharpness.weight),
        (5, 1, 10)
    );
    assert_eq!(sharpness.min_cost(5), 45);
    assert_eq!(sharpness.max_cost(5), 65);
    assert!(catalog.get(&id("binding_curse")).unwrap().curse);
    assert!(catalog.get(&id("vanishing_curse")).unwrap().curse);
    assert!(!catalog.get(&id("mending")).unwrap().curse);
    for (a, b) in [
        ("sharpness", "smite"),
        ("fortune", "silk_touch"),
        ("mending", "infinity"),
        ("multishot", "piercing"),
        ("loyalty", "riptide"),
        ("density", "breach"),
    ] {
        assert!(!catalog.compatible(&id(a), &id(b)));
        assert!(!catalog.compatible(&id(b), &id(a)));
    }
    assert!(catalog.compatible(&id("protection"), &id("feather_falling")));
    assert!(!catalog.compatible(&id("sharpness"), &id("sharpness")));
    assert!(!catalog.compatible(&id("sharpness"), &id("unknown")));
}

#[test]
fn loaded_and_runtime_catalogs_produce_identical_real_item_offers_and_results() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let loaded = EnchantmentCatalog::load(&root).unwrap();
    let cached = catalog();
    let data = Data::new();
    for item in [
        "book",
        "diamond_sword",
        "diamond_axe",
        "diamond_pickaxe",
        "golden_boots",
        "diamond_chestplate",
        "bow",
        "crossbow",
        "trident",
        "mace",
        "copper_spear",
        "shears",
        "fishing_rod",
    ] {
        for seed in [0, 1, -1, 17, 123_456_789, i32::MIN, i32::MAX] {
            for shelves in [0, 7, 15, 32] {
                let input = data.stack(item);
                assert_eq!(
                    TableOffers::generate(&loaded, &data.rules(), &input, seed, shelves),
                    TableOffers::generate(&cached, &data.rules(), &input, seed, shelves),
                    "{item} {seed} {shelves}"
                );
                for offer in 0..3 {
                    assert_eq!(
                        apply_offer(&loaded, &data.rules(), &input, seed, shelves, offer),
                        apply_offer(&cached, &data.rules(), &input, seed, shelves, offer),
                        "{item} {seed} {shelves} {offer}"
                    );
                }
            }
        }
    }
}

#[test]
fn table_cost_vectors_and_shelf_cap_are_deterministic() {
    let catalog = catalog();
    let data = Data::new();
    // Java seed zero draws nextInt(8), nextInt(16) as 5,13 / 1,9 / 5,4.
    assert_eq!(
        TableOffers::generate(&catalog, &data.rules(), &data.stack("diamond_sword"), 0, 15).costs,
        [8, 13, 30]
    );
    // Even nextInt(1) consumes a draw: at zero shelves the same seed gives
    // selected base costs 6,2,6, with the middle offer exactly at its minimum.
    assert_eq!(
        TableOffers::generate(&catalog, &data.rules(), &data.stack("book"), 0, 0).costs,
        [2, 2, 6]
    );
    for seed in [0, 17, -1, i32::MAX] {
        let input = data.stack("diamond_pickaxe");
        assert_eq!(
            TableOffers::generate(&catalog, &data.rules(), &input, seed, 15),
            TableOffers::generate(&catalog, &data.rules(), &input, seed, 100)
        );
        assert_eq!(
            TableOffers::generate(&catalog, &data.rules(), &input, seed, 0),
            TableOffers::generate(&catalog, &data.rules(), &input, seed, -20)
        );
    }
}

#[test]
fn eligibility_and_anvil_supported_items_differ_from_table_primary_items() {
    let catalog = catalog();
    let data = Data::new();
    assert!(catalog.supports(&data.rules(), &data.stack("diamond_axe"), &id("sharpness")));
    assert!(catalog.supports(&data.rules(), &data.stack("copper_spear"), &id("lunge")));
    assert!(!catalog.supports(&data.rules(), &data.stack("diamond_sword"), &id("lunge")));
    assert!(catalog.supports(&data.rules(), &data.stack("shield"), &id("unbreaking")));
    for item in ["stone", "shield", "enchanted_book"] {
        assert_eq!(
            TableOffers::generate(&catalog, &data.rules(), &data.stack(item), 0, 15),
            TableOffers::default()
        );
        assert!(apply_offer(&catalog, &data.rules(), &data.stack(item), 0, 15, 2).is_none());
    }
    assert_eq!(
        TableOffers::generate(&catalog, &data.rules(), &ItemStack::EMPTY, 0, 15),
        TableOffers::default()
    );
    let input = data
        .stack("diamond_sword")
        .with_enchantment(id("sharpness"), 1);
    assert_eq!(
        TableOffers::generate(&catalog, &data.rules(), &input, 0, 15),
        TableOffers::default()
    );
    assert!(
        apply_offer(
            &catalog,
            &data.rules(),
            &data.stack("diamond_sword"),
            0,
            15,
            3
        )
        .is_none()
    );
    for seed in 0..128 {
        for offer in 0..3 {
            let result = apply_offer(
                &catalog,
                &data.rules(),
                &data.stack("diamond_axe"),
                seed,
                15,
                offer,
            )
            .unwrap();
            assert!(
                result
                    .enchantments
                    .iter()
                    .all(|e| !["sharpness", "smite", "bane_of_arthropods"].contains(&e.id.path()))
            );
        }
    }
}

#[test]
fn clue_is_applied_and_all_weighted_picks_are_compatible_supported_and_non_treasure() {
    let catalog = catalog();
    let data = Data::new();
    let mut saw_multi = false;
    for item in [
        "diamond_sword",
        "diamond_pickaxe",
        "golden_boots",
        "crossbow",
        "trident",
        "mace",
        "wooden_spear",
    ] {
        for seed in 0..128 {
            let input = data
                .stack(item)
                .with_damage(17)
                .with_custom_name("Keep my name");
            let offers = TableOffers::generate(&catalog, &data.rules(), &input, seed, 15);
            for offer in 0..3 {
                let result = apply_offer(&catalog, &data.rules(), &input, seed, 15, offer).unwrap();
                assert_eq!(result.item_id, input.item_id);
                assert_eq!(result.damage, input.damage);
                assert_eq!(result.custom_name, input.custom_name);
                assert_eq!(result.count, input.count);
                let clue = catalog
                    .definitions
                    .iter()
                    .find(|e| e.protocol_id == offers.clues[offer])
                    .unwrap();
                assert!(result.enchantments.contains(&ItemEnchantment {
                    id: clue.id.clone(),
                    level: offers.levels[offer]
                }));
                saw_multi |= result.enchantments.len() > 1;
                for (index, enchantment) in result.enchantments.iter().enumerate() {
                    let definition = catalog.get(&enchantment.id).unwrap();
                    assert!((1..=definition.max_level).contains(&enchantment.level));
                    assert!(catalog.table.contains(&catalog.by_name[&enchantment.id]));
                    assert!(catalog.supports(&data.rules(), &result, &enchantment.id));
                    assert!(!definition.curse);
                    for other in &result.enchantments[index + 1..] {
                        assert!(catalog.compatible(&enchantment.id, &other.id));
                    }
                }
            }
        }
    }
    assert!(saw_multi);
}

#[test]
fn books_drop_one_pick_before_clues_and_store_the_remaining_enchantments() {
    let catalog = catalog();
    let data = Data::new();
    let mut saw_removed = false;
    let mut untrimmed = catalog.clone();
    // Same vanilla enchantability and every table candidate, but not a book:
    // the resulting stream provides an independent view of pre-removal picks.
    untrimmed.book = id("not_a_book");
    for definition in &mut untrimmed.definitions {
        definition.supported.members.insert(id("book"));
        if let Some(primary) = &mut definition.primary {
            primary.members.insert(id("book"));
        }
    }
    for seed in 0..128 {
        let input = data.stack("book").with_custom_name("Library");
        let offers = TableOffers::generate(&catalog, &data.rules(), &input, seed, 15);
        for offer in 0..3 {
            let result = apply_offer(&catalog, &data.rules(), &input, seed, 15, offer).unwrap();
            let before = apply_offer(&untrimmed, &data.rules(), &input, seed, 15, offer).unwrap();
            let stored = &result.extended().stored_enchantments;
            assert_eq!(
                stored.len(),
                before.enchantments.len() - usize::from(before.enchantments.len() > 1)
            );
            assert!(
                stored
                    .iter()
                    .all(|enchantment| before.enchantments.contains(enchantment))
            );
            saw_removed |= before.enchantments.len() > 1;
            assert_eq!(
                result.item_id,
                data.items.id_of(&id("enchanted_book")).unwrap()
            );
            assert!(result.enchantments.is_empty());
            assert_eq!(result.custom_name, input.custom_name);
            let clue = catalog
                .definitions
                .iter()
                .find(|e| e.protocol_id == offers.clues[offer])
                .unwrap();
            assert!(stored.contains(&ItemEnchantment {
                id: clue.id.clone(),
                level: offers.levels[offer]
            }));
        }
    }
    assert!(saw_removed);
}

#[test]
fn populated_item_tags_override_static_support_members() {
    let mut data = Data::new();
    let stone_id = data.stack("stone").item_id as i32;
    data.tags
        .registries
        .entry(id("minecraft:item"))
        .or_default()
        .insert(id("minecraft:enchantable/melee_weapon"), vec![stone_id]);
    let catalog = catalog();
    assert!(!catalog.supports(
        &data.rules(),
        &data.stack("diamond_sword"),
        &id("knockback")
    ));
    assert!(catalog.supports(&data.rules(), &data.stack("stone"), &id("knockback")));
    // Item enchantability is still a component, not inferred from support tags.
    assert_eq!(
        TableOffers::generate(&catalog, &data.rules(), &data.stack("stone"), 0, 15),
        TableOffers::default()
    );
}

#[test]
fn recursive_tags_preserve_order_deduplicate_and_reject_cycles() {
    let tags = parse_tags(BTreeMap::from([
        (id("outer"), serde_json::json!({"values":["minecraft:b", "#minecraft:inner", "minecraft:a", {"id":"#minecraft:missing", "required":false}]})),
        (id("inner"), serde_json::json!({"values":["minecraft:a", "minecraft:b", "minecraft:c"]})),
    ])).unwrap();
    assert_eq!(
        resolve_tag(&id("outer"), &tags, None, &mut Vec::new()).unwrap(),
        vec![id("b"), id("a"), id("c")]
    );
    let cyclic = parse_tags(BTreeMap::from([(
        id("cycle"),
        serde_json::json!({"values":["#minecraft:cycle"]}),
    )]))
    .unwrap();
    assert!(
        resolve_tag(&id("cycle"), &cyclic, None, &mut Vec::new())
            .unwrap_err()
            .contains("cyclic")
    );
    assert!(resolve_tag(&id("missing"), &tags, None, &mut Vec::new()).is_err());
}

#[test]
fn item_facts_cannot_make_non_enchantable_carriers_table_eligible() {
    let mut data = Data::new();
    data.facts = ItemFactsTable::from_entries([(
        id("shield"),
        ItemFacts {
            max_damage: Some(336),
            ..Default::default()
        },
    )]);
    let catalog = catalog();
    assert_eq!(
        TableOffers::generate(&catalog, &data.rules(), &data.stack("shield"), 0, 15),
        TableOffers::default()
    );
}

#[test]
fn exact_vanilla_bow_offer_and_purchase_fixture() {
    let catalog = catalog();
    let data = Data::new();
    let input = data.stack("bow");
    // Reference LegacyRandom draws, with the actual ordered non_treasure tag:
    // seed 0 selects Power I + Unbreaking I; seed 1 selects Power II +
    // Unbreaking II + Punch I; seed 2 selects Power IV + Punch I.
    assert_eq!(
        TableOffers::generate(&catalog, &data.rules(), &input, 0, 15),
        TableOffers {
            costs: [8, 13, 30],
            clues: [40, 40, 29],
            levels: [1, 2, 1]
        }
    );
    for (offer, expected) in [
        vec![("power", 1), ("unbreaking", 1)],
        vec![("power", 2), ("punch", 1), ("unbreaking", 2)],
        vec![("power", 4), ("punch", 1)],
    ]
    .into_iter()
    .enumerate()
    {
        let result = apply_offer(&catalog, &data.rules(), &input, 0, 15, offer).unwrap();
        assert_eq!(
            result.enchantments,
            expected
                .into_iter()
                .map(|(name, level)| ItemEnchantment {
                    id: id(name),
                    level
                })
                .collect::<Vec<_>>()
        );
    }
}

#[test]
fn report_protocol_ids_do_not_reorder_weighted_tag_members() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let source = |relative: &str| {
        let path = root.join(relative);
        let id = Identifier::parse(format!(
            "minecraft:{}",
            path.file_stem().unwrap().to_str().unwrap()
        ))
        .unwrap();
        let value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        (id, value)
    };
    let definitions = [
        source("data/minecraft/enchantment/power.json"),
        source("data/minecraft/enchantment/punch.json"),
    ]
    .into_iter()
    .collect();
    let tags = BTreeMap::from([
        (id("curse"), serde_json::json!({"values":[]})),
        (
            id("in_enchanting_table"),
            serde_json::json!({"values":["minecraft:punch", "minecraft:power"]}),
        ),
    ]);
    let item_tags = BTreeMap::from([(
        id("enchantable/bow"),
        serde_json::json!({"values":["minecraft:bow"]}),
    )]);
    let components = [source("reports/minecraft/components/item/bow.json")]
        .into_iter()
        .collect();
    let report = serde_json::json!({"minecraft:enchantment":{"entries":{
        "minecraft:power":{"protocol_id":91},
        "minecraft:punch":{"protocol_id":7}
    }}});
    let catalog =
        EnchantmentCatalog::from_sources(definitions, tags, item_tags, components, Some(&report))
            .unwrap();
    assert_eq!(catalog.get(&id("power")).unwrap().protocol_id, 91);
    assert_eq!(catalog.get(&id("punch")).unwrap().protocol_id, 7);
    assert_eq!(
        catalog
            .table
            .iter()
            .map(|&index| catalog.definitions[index].id.clone())
            .collect::<Vec<_>>(),
        vec![id("punch"), id("power")]
    );
}
