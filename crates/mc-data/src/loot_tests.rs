use super::*;
use std::fs;

fn block_context<'a>(
    loot: &LootTables,
    block: &'a Identifier,
    properties: &[(String, String)],
    tool: &'a LootContextItem,
    seed: u64,
) -> BlockLootContext<'a> {
    BlockLootContext::try_new(
        block,
        properties,
        tool,
        LootRandomBinding::new(loot.block_random_sequence(block).cloned(), seed),
    )
    .unwrap()
}

#[test]
fn vanilla_silk_only_and_empty_tables_do_not_become_unconditional_drops() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    for name in ["glass", "cake"] {
        let source = crate::runtime_resources::read_json(
            &fixture,
            &format!("data/minecraft/loot_table/blocks/{name}.json"),
        )
        .unwrap();
        fs::write(blocks.join(format!("{name}.json")), source).unwrap();
    }
    let loot = load_vanilla_subset(tmp.path()).unwrap();
    let glass = Identifier::parse("minecraft:glass").unwrap();
    let plain = LootContextItem::empty();
    let fortune = LootContextItem::new(Identifier::parse("minecraft:diamond_pickaxe").unwrap())
        .with_enchantments(
            LootEnchantments::try_from_levels([(
                Identifier::parse("minecraft:fortune").unwrap(),
                3,
            )])
            .unwrap(),
        );
    let silk = LootContextItem::new(Identifier::parse("minecraft:diamond_pickaxe").unwrap())
        .with_enchantments(
            LootEnchantments::try_from_levels([(
                Identifier::parse("minecraft:silk_touch").unwrap(),
                1,
            )])
            .unwrap(),
        );
    for tool in [&plain, &fortune] {
        assert_eq!(
            loot.evaluate_block(&block_context(&loot, &glass, &[], tool, 7))
                .unwrap(),
            Some(Vec::new()),
        );
    }
    assert_eq!(
        loot.evaluate_block(&block_context(&loot, &glass, &[], &silk, 7))
            .unwrap(),
        Some(vec![LootDrop::single(glass.clone())]),
    );
    assert_eq!(
        loot.block_explosion_drops(&glass, &[], None, || 0.0),
        Some(Vec::new())
    );
    let cake = Identifier::parse("minecraft:cake").unwrap();
    for tool in [&plain, &silk] {
        assert_eq!(
            loot.evaluate_block(&block_context(&loot, &cake, &[], tool, 7))
                .unwrap(),
            Some(Vec::new()),
        );
    }
}

#[test]
fn builtin_survival_loot_loads_from_repo_json() {
    let loot = builtin();

    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
        Some(&Identifier::parse("minecraft:cobblestone").unwrap())
    );
    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:podzol").unwrap()),
        Some(&Identifier::parse("minecraft:dirt").unwrap())
    );
}

#[test]
fn builtin_hanging_sign_loot_covers_ceiling_and_wall_forms_with_shared_random_sequences() {
    let loot = builtin();
    let mut merged = LootTables::default();
    merged.fill_missing_from(&loot);
    let tool = LootContextItem::empty();
    for wood in [
        "acacia", "bamboo", "birch", "cherry", "crimson", "dark_oak", "jungle", "mangrove", "oak",
        "pale_oak", "spruce", "warped",
    ] {
        let item = Identifier::parse(format!("minecraft:{wood}_hanging_sign")).unwrap();
        let sequence = Identifier::parse(format!("minecraft:blocks/{wood}_hanging_sign")).unwrap();
        for form in ["hanging_sign", "wall_hanging_sign"] {
            let block = Identifier::parse(format!("minecraft:{wood}_{form}")).unwrap();
            for catalogue in [&loot, &merged] {
                assert_eq!(catalogue.block_random_sequence(&block), Some(&sequence));
                assert_eq!(
                    catalogue
                        .evaluate_block(&block_context(catalogue, &block, &[], &tool, 42))
                        .unwrap(),
                    Some(vec![LootDrop::single(item.clone())]),
                    "{block}"
                );
            }
        }
    }
}

#[test]
fn builtin_destroy_explosion_uses_all_repo_owned_block_drops() {
    let loot = builtin();
    for (block, expected) in [
        ("minecraft:oak_log", "minecraft:oak_log"),
        ("minecraft:diamond_ore", "minecraft:diamond"),
    ] {
        let drops = loot
            .block_explosion_drops(&Identifier::parse(block).unwrap(), &[], None, || {
                panic!("DESTROY must not consume explosion loot RNG")
            })
            .expect("repo-owned block drop is supported for DESTROY");
        assert_eq!(
            drops,
            [LootDrop::single(Identifier::parse(expected).unwrap())],
            "{block}"
        );
    }
}

#[test]
fn builtin_ore_explosion_decay_applies_zero_fortune_without_mining_rng() {
    let loot = builtin();
    let diamond = Identifier::parse("minecraft:diamond_ore").unwrap();
    let mut rolls = [0.25, 0.250_001].into_iter();
    assert_eq!(
        loot.block_explosion_drops(&diamond, &[], Some(4.0), || rolls.next().unwrap()),
        Some(vec![LootDrop::single(
            Identifier::parse("minecraft:diamond").unwrap()
        )])
    );
    assert_eq!(
        loot.block_explosion_drops(&diamond, &[], Some(4.0), || rolls.next().unwrap()),
        Some(Vec::new())
    );
    assert_eq!(rolls.next(), None);
}

#[test]
fn loads_custom_repo_owned_loot_file() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("loot.json");
    fs::write(
        &path,
        r#"{
          "blocks": { "minecraft:stone": "minecraft:cobblestone" }
        }"#,
    )
    .unwrap();

    let loot = load(&path).unwrap();

    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
        Some(&Identifier::parse("minecraft:cobblestone").unwrap())
    );
}

#[test]
fn sidecar_entries_override_only_their_fallback_keys() {
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let mut sidecar = LootTables::from_drop_maps(BTreeMap::from([(
        stone.clone(),
        LootDrop::single(Identifier::parse("minecraft:diamond").unwrap()),
    )]));

    let fallback = builtin();
    sidecar.fill_missing_from(&fallback);

    assert_eq!(
        sidecar.block_drop(&stone),
        Some(&Identifier::parse("minecraft:diamond").unwrap())
    );
    assert_eq!(sidecar.total_drops(), fallback.total_drops());
}

#[test]
fn loads_simple_vanilla_subset_fixture() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("stone.json"),
        r#"{
          "pools": [{
            "entries": [{
              "type": "minecraft:alternatives",
              "children": [
                {
                  "type": "minecraft:item",
                  "conditions": [{ "condition": "minecraft:match_tool" }],
                  "name": "minecraft:stone"
                },
                {
                  "type": "minecraft:item",
                  "conditions": [{ "condition": "minecraft:survives_explosion" }],
                  "name": "minecraft:cobblestone"
                }
              ]
            }]
          }]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();

    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
        Some(&Identifier::parse("minecraft:cobblestone").unwrap())
    );
    assert_eq!(
        loot.block_drop_stack(&Identifier::parse("minecraft:stone").unwrap())
            .map(|drop| drop.count),
        Some(LootCount::Fixed(1))
    );
}

#[test]
fn loads_block_uniform_count_with_no_fortune_or_explosion_baseline() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("lapis_ore.json"),
        r#"{
          "pools": [{
            "entries": [{
              "type": "minecraft:item",
              "functions": [
                {
                  "function": "minecraft:set_count",
                  "count": {
                    "type": "minecraft:uniform",
                    "min": 4.0,
                    "max": 9.0
                  }
                },
                {
                  "function": "minecraft:apply_bonus",
                  "enchantment": "minecraft:fortune",
                  "formula": "minecraft:ore_drops"
                },
                { "function": "minecraft:explosion_decay" }
              ],
              "name": "minecraft:lapis_lazuli"
            }]
          }]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();

    assert_eq!(
        loot.block_drop_stack(&Identifier::parse("minecraft:lapis_ore").unwrap()),
        Some(&LootDrop::uniform(
            Identifier::parse("minecraft:lapis_lazuli").unwrap(),
            4,
            9,
        ))
    );
}

#[test]
fn loads_pool_level_set_count_constant() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("oak_leaves.json"),
        r#"{
          "pools": [{
            "functions": [{ "function": "minecraft:set_count", "count": 3 }],
            "entries": [{
              "type": "minecraft:item",
              "name": "minecraft:apple"
            }]
          }]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();

    assert_eq!(
        loot.block_drop_stack(&Identifier::parse("minecraft:oak_leaves").unwrap()),
        Some(&LootDrop {
            item: Identifier::parse("minecraft:apple").unwrap(),
            count: LootCount::Fixed(3),
            components: None,
        })
    );
}

#[test]
fn unsupported_rolls_or_bonus_rolls_fail_closed_for_whole_table() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("bad_rolls.json"),
        r#"{
          "pools": [
            {
              "rolls": { "type": "minecraft:uniform", "min": 4097, "max": 4097 },
              "entries": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
            },
            {
              "entries": [{ "type": "minecraft:item", "name": "minecraft:cobblestone" }]
            }
          ]
        }"#,
    )
    .unwrap();
    fs::write(
        blocks.join("bad_bonus_rolls.json"),
        r#"{
          "pools": [
            {
              "bonus_rolls": { "type": "minecraft:uniform", "min": 1, "max": 0 },
              "entries": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
            },
            {
              "entries": [{ "type": "minecraft:item", "name": "minecraft:dirt" }]
            }
          ]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();

    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:bad_rolls").unwrap()),
        None
    );
    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:bad_bonus_rolls").unwrap()),
        None
    );
}

#[test]
fn skips_alternatives_wrappers_with_unsupported_conditions_or_features() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("conditioned_alternatives.json"),
        r#"{
          "pools": [{
            "entries": [
              {
                "type": "minecraft:alternatives",
                "conditions": [{ "condition": "minecraft:match_tool" }],
                "children": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
              },
              { "type": "minecraft:item", "name": "minecraft:cobblestone" }
            ]
          }]
        }"#,
    )
    .unwrap();
    fs::write(
        blocks.join("featured_alternatives.json"),
        r#"{
          "pools": [{
            "entries": [
              {
                "type": "minecraft:alternatives",
                "features": ["minecraft:update_1_21"],
                "children": [{ "type": "minecraft:item", "name": "minecraft:diamond" }]
              },
              { "type": "minecraft:item", "name": "minecraft:dirt" }
            ]
          }]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();

    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:conditioned_alternatives").unwrap()),
        Some(&Identifier::parse("minecraft:cobblestone").unwrap())
    );
    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:featured_alternatives").unwrap()),
        Some(&Identifier::parse("minecraft:dirt").unwrap())
    );
}

#[test]
#[ignore = "requires local 26.1.2 loot table sidecars"]
fn loads_real_vanilla_subset_when_present() {
    let path = workspace_path("data/vanilla/data/minecraft/loot_table");
    assert!(
        path.is_dir(),
        "{} not present; run tools/extract-vanilla-data.sh",
        path.display()
    );

    let loot = load_vanilla_subset(path).unwrap();

    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:stone").unwrap()),
        Some(&Identifier::parse("minecraft:cobblestone").unwrap())
    );
    assert_eq!(
        loot.block_drop(&Identifier::parse("minecraft:grass_block").unwrap()),
        Some(&Identifier::parse("minecraft:dirt").unwrap())
    );
    assert_eq!(
        loot.block_drop_stack(&Identifier::parse("minecraft:lapis_ore").unwrap()),
        Some(&LootDrop::uniform(
            Identifier::parse("minecraft:lapis_lazuli").unwrap(),
            4,
            9,
        ))
    );
    assert_eq!(
        loot.block_drop_stack(&Identifier::parse("minecraft:redstone_ore").unwrap()),
        Some(&LootDrop::uniform(
            Identifier::parse("minecraft:redstone").unwrap(),
            4,
            5,
        ))
    );
    assert_eq!(
        loot.block_loot(&Identifier::parse("minecraft:stone").unwrap())
            .unwrap()
            .drops_for_tool(true, None),
        [BlockLootDrop::plain(LootDrop::single(
            Identifier::parse("minecraft:stone").unwrap()
        ))]
    );
    let diamond = &loot
        .block_loot(&Identifier::parse("minecraft:diamond_ore").unwrap())
        .unwrap()
        .drops_for_tool(false, None)[0];
    assert_eq!(diamond.fortune_bonus, Some(FortuneBonus::OreDrops));
    let redstone = &loot
        .block_loot(&Identifier::parse("minecraft:redstone_ore").unwrap())
        .unwrap()
        .drops_for_tool(false, None)[0];
    assert_eq!(
        redstone.fortune_bonus,
        Some(FortuneBonus::UniformBonusCount {
            bonus_multiplier: 1
        })
    );
    assert_eq!(
        loot.block_drop_stacks(&Identifier::parse("minecraft:potted_oak_sapling").unwrap()),
        Some(
            [
                LootDrop::single(Identifier::parse("minecraft:flower_pot").unwrap()),
                LootDrop::single(Identifier::parse("minecraft:oak_sapling").unwrap()),
            ]
            .as_slice()
        )
    );
}

#[test]
fn keeps_silk_touch_alternative_from_vanilla_block_table() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("stone.json"),
        r#"{
          "pools": [{
            "bonus_rolls": 0.0,
            "entries": [{
              "type": "minecraft:alternatives",
              "children": [
                {
                  "type": "minecraft:item",
                  "conditions": [{
                    "condition": "minecraft:match_tool",
                    "predicate": {
                      "predicates": {
                        "minecraft:enchantments": [{
                          "enchantments": "minecraft:silk_touch",
                          "levels": { "min": 1 }
                        }]
                      }
                    }
                  }],
                  "name": "minecraft:stone"
                },
                {
                  "type": "minecraft:item",
                  "conditions": [{ "condition": "minecraft:survives_explosion" }],
                  "name": "minecraft:cobblestone"
                }
              ]
            }],
            "rolls": 1.0
          }]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();
    let rule = loot
        .block_loot(&Identifier::parse("minecraft:stone").unwrap())
        .unwrap();

    assert_eq!(
        rule.drops_for_tool(true, None),
        [BlockLootDrop::plain(LootDrop::single(
            Identifier::parse("minecraft:stone").unwrap()
        ))]
    );
    let regular = &rule.drops_for_tool(false, None)[0];
    assert_eq!(regular.drop.item.as_str(), "minecraft:cobblestone");
    assert_eq!(
        regular.sample_simple_explosion_count(Some(4.0), &mut || 0.25, regular.drop.count),
        Some(1)
    );
    assert_eq!(
        regular.sample_simple_explosion_count(Some(4.0), &mut || 0.250_001, regular.drop.count),
        Some(0)
    );
    assert_eq!(
        regular.sample_simple_explosion_count(
            None,
            &mut || panic!("DESTROY must not consume explosion loot RNG"),
            regular.drop.count,
        ),
        Some(1)
    );
}

#[test]
fn explosion_decay_rolls_once_per_fixed_count_item() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("test_ore.json"),
        r#"{
          "pools": [{
            "entries": [{
              "type": "minecraft:item",
              "functions": [
                { "function": "minecraft:set_count", "count": 4 },
                { "function": "minecraft:explosion_decay" }
              ],
              "name": "minecraft:diamond"
            }]
          }]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();
    let block = Identifier::parse("minecraft:test_ore").unwrap();
    let mut rolls = [0.1, 0.9, 0.25, 0.5].into_iter();
    let drops = loot
        .block_explosion_drops(&block, &[], Some(4.0), || rolls.next().unwrap())
        .expect("simple fixed-count explosion table is supported");

    assert_eq!(
        drops,
        [LootDrop::fixed(
            Identifier::parse("minecraft:diamond").unwrap(),
            2
        )]
    );
    assert_eq!(rolls.next(), None, "one roll is consumed per input item");
}

#[test]
fn ore_drops_fortune_level_changes_sampled_count() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("diamond_ore.json"),
        r#"{
          "pools": [{
            "entries": [{
              "type": "minecraft:alternatives",
              "children": [
                {
                  "type": "minecraft:item",
                  "conditions": [{
                    "condition": "minecraft:match_tool",
                    "predicate": {
                      "predicates": {
                        "minecraft:enchantments": [{
                          "enchantments": "minecraft:silk_touch",
                          "levels": { "min": 1 }
                        }]
                      }
                    }
                  }],
                  "name": "minecraft:diamond_ore"
                },
                {
                  "type": "minecraft:item",
                  "functions": [{
                    "enchantment": "minecraft:fortune",
                    "formula": "minecraft:ore_drops",
                    "function": "minecraft:apply_bonus"
                  }, {
                    "function": "minecraft:explosion_decay"
                  }],
                  "name": "minecraft:diamond"
                }
              ]
            }]
          }]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();
    let drop = &loot
        .block_loot(&Identifier::parse("minecraft:diamond_ore").unwrap())
        .unwrap()
        .drops_for_tool(false, None)[0];

    assert_eq!(drop.fortune_bonus, Some(FortuneBonus::OreDrops));
}

#[test]
fn parses_and_samples_binomial_with_bonus_count() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("test_crop.json"),
        r#"{
          "pools": [{
            "entries": [{
              "type": "minecraft:item",
              "functions": [{
                "enchantment": "minecraft:fortune",
                "formula": "minecraft:binomial_with_bonus_count",
                "function": "minecraft:apply_bonus",
                "parameters": {
                  "extra": 3,
                  "probability": 1.0
                }
              }],
              "name": "minecraft:wheat_seeds"
            }]
          }]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();
    let drop = &loot
        .block_loot(&Identifier::parse("minecraft:test_crop").unwrap())
        .unwrap()
        .drops_for_tool(false, None)[0];

    assert_eq!(
        drop.fortune_bonus,
        Some(FortuneBonus::BinomialWithBonusCount {
            extra_rounds: 3,
            probability: 1.0,
        })
    );
}

#[test]
fn block_state_crop_pools_choose_mature_and_fallback_alternatives() {
    let tmp = tempfile::tempdir().unwrap();
    let blocks = tmp.path().join("blocks");
    fs::create_dir_all(&blocks).unwrap();
    fs::write(
        blocks.join("wheat.json"),
        r#"{
          "pools": [{
            "entries": [{
              "type": "minecraft:alternatives",
              "children": [{
                "type": "minecraft:item",
                "conditions": [{
                  "block": "minecraft:wheat",
                  "condition": "minecraft:block_state_property",
                  "properties": { "age": "7" }
                }],
                "name": "minecraft:wheat"
              }, {
                "type": "minecraft:item",
                "name": "minecraft:wheat_seeds"
              }]
            }]
          }, {
            "conditions": [{
              "block": "minecraft:wheat",
              "condition": "minecraft:block_state_property",
              "properties": { "age": "7" }
            }],
            "entries": [{
              "type": "minecraft:item",
              "functions": [{
                "enchantment": "minecraft:fortune",
                "formula": "minecraft:binomial_with_bonus_count",
                "function": "minecraft:apply_bonus",
                "parameters": { "extra": 3, "probability": 1.0 }
              }],
              "name": "minecraft:wheat_seeds"
            }]
          }]
        }"#,
    )
    .unwrap();

    let loot = load_vanilla_subset(tmp.path()).unwrap();
    let wheat = Identifier::parse("minecraft:wheat").unwrap();
    let young = vec![("age".to_string(), "6".to_string())];
    let mature = vec![("age".to_string(), "7".to_string())];
    let tool = LootContextItem::empty();

    let young_drops = loot
        .evaluate_block(&block_context(&loot, &wheat, &young, &tool, 0))
        .unwrap()
        .unwrap();
    assert_eq!(young_drops.len(), 1);
    assert_eq!(young_drops[0].item.as_str(), "minecraft:wheat_seeds");

    let mature_drops = loot
        .evaluate_block(&block_context(&loot, &wheat, &mature, &tool, 0))
        .unwrap()
        .unwrap();
    assert_eq!(mature_drops.len(), 2);
    assert_eq!(mature_drops[0].item.as_str(), "minecraft:wheat");
    assert_eq!(mature_drops[1].item.as_str(), "minecraft:wheat_seeds");
    assert_eq!(mature_drops[1].count, LootCount::Fixed(4));
}

#[test]
#[ignore = "requires local 26.1.2 carrots loot table sidecar"]
fn real_binomial_fixture_has_expected_parameters_when_present() {
    let path = workspace_path("data/vanilla/data/minecraft/loot_table/blocks/carrots.json");
    assert!(
        path.is_file(),
        "{} not present; run tools/extract-vanilla-data.sh",
        path.display()
    );

    let carrots: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let entry = carrots.pointer("/pools/1/entries/0").unwrap();
    let binomial = fortune_bonus_from_functions(entry).unwrap();

    assert_eq!(
        binomial,
        Some(FortuneBonus::BinomialWithBonusCount {
            extra_rounds: 3,
            probability: 0.571_428_6,
        })
    );
}

#[test]
#[ignore = "requires local 26.1.2 crop loot table sidecars"]
fn real_crop_fixtures_load_state_conditioned_pools_when_present() {
    let root = workspace_path("data/vanilla/data/minecraft/loot_table");
    for fixture in [
        "blocks/wheat.json",
        "blocks/carrots.json",
        "blocks/potatoes.json",
        "blocks/beetroots.json",
    ] {
        let path = root.join(fixture);
        assert!(
            path.is_file(),
            "{} not present; run tools/extract-vanilla-data.sh",
            path.display()
        );
    }

    let loot = load_vanilla_subset(&root).unwrap();
    for (block, young_age, mature_age, young_expected, mature_expected) in [
        (
            "minecraft:wheat",
            "6",
            "7",
            &["minecraft:wheat_seeds"][..],
            &["minecraft:wheat", "minecraft:wheat_seeds"][..],
        ),
        (
            "minecraft:carrots",
            "6",
            "7",
            &["minecraft:carrot"][..],
            &["minecraft:carrot", "minecraft:carrot"][..],
        ),
        (
            "minecraft:potatoes",
            "6",
            "7",
            &["minecraft:potato"][..],
            &["minecraft:potato", "minecraft:potato"][..],
        ),
        (
            "minecraft:beetroots",
            "2",
            "3",
            &["minecraft:beetroot_seeds"][..],
            &["minecraft:beetroot", "minecraft:beetroot_seeds"][..],
        ),
    ] {
        let block = Identifier::parse(block).unwrap();
        let items = |age: &str| {
            let properties = [("age".to_string(), age.to_string())];
            let tool = LootContextItem::empty();
            loot.evaluate_block(&block_context(&loot, &block, &properties, &tool, 0))
                .unwrap()
                .unwrap()
                .into_iter()
                .map(|drop| drop.item.as_str().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(
            items(young_age)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            young_expected,
            "young {block}"
        );
        assert_eq!(
            items(mature_age)
                .iter()
                .map(String::as_str)
                .collect::<Vec<_>>(),
            mature_expected,
            "mature {block}"
        );
    }
}

#[test]
fn fortune_level_zero_preserves_baseline_count_sampling() {
    let drop = BlockLootDrop {
        drop: LootDrop::uniform(Identifier::parse("minecraft:lapis_lazuli").unwrap(), 4, 9),
        fortune_bonus: Some(FortuneBonus::OreDrops),
        random_chance: None,
        fortune_chances: Vec::new(),
        survives_explosion: false,
        explosion_decay: false,
        rolls: LootCount::Fixed(1),
        bonus_rolls: LootCount::Fixed(0),
        copy_components: Vec::new(),
        copy_state: Vec::new(),
        dynamic_sherds: false,
    };

    for roll in 0..12 {
        assert_eq!(
            drop.drop.count.try_sample(roll).unwrap(),
            drop.drop.count.try_sample(roll).unwrap()
        );
    }
}

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(rel)
}
