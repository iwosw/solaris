use std::collections::BTreeMap;

use mc_data::block_mining::BlockMiningTable;
use mc_data::item_components::{ItemFacts, ItemFactsTable, ToolFacts, ToolRuleFacts};
use mc_data::items::{ItemRegistry, ItemReport};
use mc_data::loot::LootTables;
use mc_data::tags::TagsData;
use mc_data::{Identifier, ItemEnchantment, ItemStack};
use mc_world::BlockRegistry;

use super::{BlockDropRules, block_drops};
use crate::inventory::ItemRules;

const STONE: u32 = 1;
const COBBLESTONE: u32 = 2;
const PICKAXE: u32 = 3;

fn id(name: &str) -> Identifier {
    Identifier::parse(name).unwrap()
}

struct Data {
    blocks: BlockRegistry,
    loot: LootTables,
    mining: BlockMiningTable,
    items: ItemRegistry,
    facts: ItemFactsTable,
    tags: TagsData,
    stone_state: u32,
}

impl Data {
    fn new() -> Self {
        let blocks =
            BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap();
        let stone_state = blocks.block(&id("minecraft:stone")).unwrap().default.0;
        let states = blocks.len();
        let mut requires = vec![false; states];
        requires[stone_state as usize] = true;
        let mining = BlockMiningTable::from_arrays("26.1.2", vec![1.5; states], requires);

        let dir = tempfile::tempdir().expect("isolated block-drop fixture");
        std::fs::create_dir_all(dir.path().join("blocks")).unwrap();
        let vanilla = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../data/vanilla/data/minecraft/loot_table/blocks/stone.json");
        std::fs::copy(vanilla, dir.path().join("blocks/stone.json")).unwrap();
        let loot = mc_data::loot::load_vanilla_subset(dir.path()).unwrap();

        let report = |name, protocol_id| ItemReport {
            id: id(name),
            protocol_id,
        };
        Self {
            blocks,
            loot,
            mining,
            items: ItemRegistry::from_report(&[
                report("minecraft:stone", STONE),
                report("minecraft:cobblestone", COBBLESTONE),
                report("minecraft:wooden_pickaxe", PICKAXE),
            ]),
            facts: ItemFactsTable::from_entries([(
                id("minecraft:wooden_pickaxe"),
                ItemFacts {
                    tool: Some(ToolFacts {
                        default_mining_speed: None,
                        damage_per_block: Some(1),
                        rules: vec![ToolRuleFacts {
                            blocks: vec!["minecraft:stone".to_owned()],
                            speed: Some(2.0),
                            correct_for_drops: Some(true),
                        }],
                    }),
                    ..ItemFacts::default()
                },
            )]),
            tags: TagsData::from_registries(BTreeMap::new()),
            stone_state,
        }
    }

    fn drops(&self, tool: &ItemStack) -> Vec<ItemStack> {
        let rules = ItemRules {
            items: &self.items,
            facts: &self.facts,
            tags: &self.tags,
            recipes: &[],
        };
        let drops = BlockDropRules {
            blocks: &self.blocks,
            loot: &self.loot,
            mining: &self.mining,
        };
        block_drops(&drops, &rules, self.stone_state, tool, 7, None).unwrap()
    }
}

#[test]
fn stone_needs_a_pickaxe_and_drops_cobblestone() {
    let data = Data::new();
    assert!(data.drops(&ItemStack::EMPTY).is_empty());
    assert_eq!(
        data.drops(&ItemStack::new(PICKAXE, 1)),
        vec![ItemStack::new(COBBLESTONE, 1)]
    );
}

#[test]
fn silk_touch_drops_the_stone_itself() {
    let data = Data::new();
    let mut pickaxe = ItemStack::new(PICKAXE, 1);
    pickaxe.enchantments = vec![ItemEnchantment {
        id: id("minecraft:silk_touch"),
        level: 1,
    }];
    assert_eq!(data.drops(&pickaxe), vec![ItemStack::new(STONE, 1)]);
}
