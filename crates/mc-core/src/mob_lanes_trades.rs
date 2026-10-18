//! Wandering trader pools from the local vanilla 26.1.2 jar trade/tag JSON.
//! Two buying, two uncommon and five common offers; no resident restocking.
use crate::natural_spawn::Rng;
use crate::registry::CoreRegistry;
use crate::villager::VillagerTradeOffer;
use mc_data::{Identifier, ItemStack};

type Spec = (&'static str, i32, &'static str, i32, i32, f32);
const BUYING: &[Spec] = &[
    ("minecraft:water_bucket", 1, "minecraft:emerald", 2, 2, 0.05),
    ("minecraft:milk_bucket", 1, "minecraft:emerald", 2, 2, 0.05),
    (
        "minecraft:fermented_spider_eye",
        1,
        "minecraft:emerald",
        3,
        2,
        0.05,
    ),
    ("minecraft:baked_potato", 4, "minecraft:emerald", 1, 2, 0.05),
    ("minecraft:hay_block", 1, "minecraft:emerald", 1, 2, 0.05),
];
const UNCOMMON: &[Spec] = &[
    ("minecraft:emerald", 1, "minecraft:packed_ice", 1, 6, 0.05),
    ("minecraft:emerald", 6, "minecraft:blue_ice", 1, 6, 0.05),
    ("minecraft:emerald", 1, "minecraft:gunpowder", 4, 2, 0.05),
    ("minecraft:emerald", 3, "minecraft:podzol", 3, 6, 0.05),
    ("minecraft:emerald", 1, "minecraft:acacia_log", 8, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:birch_log", 8, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:dark_oak_log", 8, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:jungle_log", 8, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:oak_log", 8, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:spruce_log", 8, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:cherry_log", 8, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:mangrove_log", 8, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:pale_oak_log", 8, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:iron_pickaxe", 1, 1, 0.2),
    ("minecraft:emerald", 5, "minecraft:potion", 1, 1, 0.05),
];
const COMMON: &[Spec] = &[
    (
        "minecraft:emerald",
        3,
        "minecraft:tropical_fish_bucket",
        1,
        4,
        0.05,
    ),
    (
        "minecraft:emerald",
        3,
        "minecraft:pufferfish_bucket",
        1,
        4,
        0.05,
    ),
    ("minecraft:emerald", 2, "minecraft:sea_pickle", 1, 5, 0.05),
    ("minecraft:emerald", 4, "minecraft:slime_ball", 1, 5, 0.05),
    ("minecraft:emerald", 2, "minecraft:glowstone", 1, 5, 0.05),
    (
        "minecraft:emerald",
        5,
        "minecraft:nautilus_shell",
        1,
        5,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:fern", 1, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:sugar_cane", 1, 8, 0.05),
    ("minecraft:emerald", 1, "minecraft:pumpkin", 1, 4, 0.05),
    ("minecraft:emerald", 3, "minecraft:kelp", 1, 12, 0.05),
    ("minecraft:emerald", 3, "minecraft:cactus", 1, 8, 0.05),
    ("minecraft:emerald", 1, "minecraft:dandelion", 1, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:poppy", 1, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:blue_orchid", 1, 8, 0.05),
    ("minecraft:emerald", 1, "minecraft:allium", 1, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:azure_bluet", 1, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:red_tulip", 1, 12, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:orange_tulip",
        1,
        12,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:white_tulip", 1, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:pink_tulip", 1, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:oxeye_daisy", 1, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:cornflower", 1, 12, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:lily_of_the_valley",
        1,
        7,
        0.05,
    ),
    (
        "minecraft:emerald",
        1,
        "minecraft:open_eyeblossom",
        1,
        7,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:wheat_seeds", 1, 12, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:beetroot_seeds",
        1,
        12,
        0.05,
    ),
    (
        "minecraft:emerald",
        1,
        "minecraft:pumpkin_seeds",
        1,
        12,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:melon_seeds", 1, 12, 0.05),
    (
        "minecraft:emerald",
        5,
        "minecraft:acacia_sapling",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        5,
        "minecraft:birch_sapling",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        5,
        "minecraft:dark_oak_sapling",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        5,
        "minecraft:jungle_sapling",
        1,
        8,
        0.05,
    ),
    ("minecraft:emerald", 5, "minecraft:oak_sapling", 1, 8, 0.05),
    (
        "minecraft:emerald",
        5,
        "minecraft:spruce_sapling",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        5,
        "minecraft:cherry_sapling",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        5,
        "minecraft:pale_oak_sapling",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        5,
        "minecraft:mangrove_propagule",
        1,
        8,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:red_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:white_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:blue_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:pink_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:black_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:green_dye", 3, 12, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:light_gray_dye",
        3,
        12,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:magenta_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:yellow_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:gray_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:purple_dye", 3, 12, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:light_blue_dye",
        3,
        12,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:lime_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:orange_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:brown_dye", 3, 12, 0.05),
    ("minecraft:emerald", 1, "minecraft:cyan_dye", 3, 12, 0.05),
    (
        "minecraft:emerald",
        3,
        "minecraft:brain_coral_block",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        3,
        "minecraft:bubble_coral_block",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        3,
        "minecraft:fire_coral_block",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        3,
        "minecraft:horn_coral_block",
        1,
        8,
        0.05,
    ),
    (
        "minecraft:emerald",
        3,
        "minecraft:tube_coral_block",
        1,
        8,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:vine", 3, 4, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:pale_hanging_moss",
        3,
        4,
        0.05,
    ),
    (
        "minecraft:emerald",
        1,
        "minecraft:brown_mushroom",
        3,
        4,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:red_mushroom", 3, 4, 0.05),
    ("minecraft:emerald", 1, "minecraft:lily_pad", 5, 2, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:small_dripleaf",
        2,
        5,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:sand", 8, 8, 0.05),
    ("minecraft:emerald", 1, "minecraft:red_sand", 4, 6, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:pointed_dripstone",
        2,
        5,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:rooted_dirt", 2, 5, 0.05),
    ("minecraft:emerald", 1, "minecraft:moss_block", 2, 5, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:pale_moss_block",
        2,
        5,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:wildflowers", 1, 12, 0.05),
    (
        "minecraft:emerald",
        1,
        "minecraft:tall_dry_grass",
        1,
        12,
        0.05,
    ),
    (
        "minecraft:emerald",
        3,
        "minecraft:firefly_bush",
        1,
        12,
        0.05,
    ),
    (
        "minecraft:emerald",
        2,
        "minecraft:golden_dandelion",
        1,
        12,
        0.05,
    ),
    ("minecraft:emerald", 1, "minecraft:name_tag", 1, 5, 0.05),
];

pub(crate) fn generate(registry: &CoreRegistry, rng: &mut Rng) -> Vec<VillagerTradeOffer> {
    let mut offers = Vec::with_capacity(9);
    for (pool, count) in [(BUYING, 2), (UNCOMMON, 2), (COMMON, 5)] {
        let mut selected = [usize::MAX; 5];
        for index in 0..count {
            let mut pick = rng.below(pool.len() as u64) as usize;
            while selected[..index].contains(&pick) {
                pick = (pick + 1) % pool.len();
            }
            selected[index] = pick;
            let (a, cost, b, quantity, max_uses, multiplier) = pool[pick];
            let a = registry
                .items
                .id_of(&Identifier::parse(a).expect("vanilla cost"));
            let b = registry
                .items
                .id_of(&Identifier::parse(b).expect("vanilla result"));
            let (Some(a), Some(b)) = (a, b) else { continue };
            let mut result = ItemStack::new(b, quantity);
            if registry
                .items
                .name_of(b)
                .is_some_and(|name| name.path() == "potion")
            {
                result.extended_mut().potion =
                    Some(Identifier::parse("minecraft:long_invisibility").expect("vanilla potion"));
            }
            let mut cost = cost;
            if registry
                .items
                .name_of(b)
                .is_some_and(|name| name.path() == "iron_pickaxe")
            {
                // Vanilla's level-5..19 equipment roll, approximated with a useful
                // pickaxe enchantment. The enchantment cost is part of this offer.
                let level = 1 + rng.below(3) as i32;
                result = result.with_enchantment(
                    Identifier::parse("minecraft:efficiency").expect("vanilla enchantment"),
                    level,
                );
                cost += level;
            }
            offers.push(VillagerTradeOffer {
                cost_a: ItemStack::new(a, cost),
                cost_b: None,
                result,
                uses: 0,
                max_uses,
                reward_exp: true,
                xp: 0,
                special_price: 0,
                demand: 0,
                price_multiplier: multiplier,
            });
        }
    }
    offers
}
