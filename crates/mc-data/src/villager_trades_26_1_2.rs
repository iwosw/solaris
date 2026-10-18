//! Data-driven vanilla villager trade specs for Java Edition 26.1.2.
//!
//! Compact in-repo catalog of the deterministic offers from the
//! minecraft.wiki Trading page, keyed by profession and career level
//! (1 novice .. 5 master). Level thresholds are 0/10/70/150/250 total
//! merchant XP. Values (costs, results, max uses, villager XP, price
//! multiplier) come from the wiki JE trade tables; offers whose result
//! needs a runtime roll (enchanted gear, enchanted books, explorer maps,
//! suspicious stew) or a second cost item are omitted - the pool never
//! invents an offer vanilla does not list. Per-level unlock picks up to two
//! new trades deterministically (first two not already known), matching
//! vanilla JE "up to two new trades" shape without the random draw.

use crate::Identifier;

pub const TOOLSMITH_JOB_SITE_26_1_2: &str = "minecraft:smithing_table";

/// Total merchant XP required to reach each career level (index = level - 1).
/// From the wiki Trading page Level table: novice 0, apprentice 10,
/// journeyman 70, expert 150, master 250.
pub const VILLAGER_LEVEL_XP_THRESHOLDS_26_1_2: [i32; 5] = [0, 10, 70, 150, 250];

/// Career level (1 novice .. 5 master) for a total merchant XP value.
#[must_use]
pub const fn villager_level_for_xp_26_1_2(xp: i32) -> u8 {
    let xp = if xp < 0 { 0 } else { xp };
    if xp >= 250 {
        5
    } else if xp >= 150 {
        4
    } else if xp >= 70 {
        3
    } else if xp >= 10 {
        2
    } else {
        1
    }
}

/// 26.1.2 job-site POI block to `villager_profession` registry name. Every
/// vanilla 26.1.2 job site maps to its profession; unknown blocks fail
/// closed so unsupported job sites never assign a profession.
#[must_use]
pub fn supported_profession_for_job_site_26_1_2(block: &Identifier) -> Option<&'static str> {
    match block.as_str() {
        "minecraft:composter" => Some("farmer"),
        "minecraft:barrel" => Some("fisherman"),
        "minecraft:cartography_table" => Some("cartographer"),
        "minecraft:brewing_stand" => Some("cleric"),
        "minecraft:lectern" => Some("librarian"),
        "minecraft:fletching_table" => Some("fletcher"),
        "minecraft:loom" => Some("shepherd"),
        "minecraft:smithing_table" => Some("toolsmith"),
        "minecraft:blast_furnace" => Some("armorer"),
        "minecraft:grindstone" => Some("weaponsmith"),
        "minecraft:smoker" => Some("butcher"),
        "minecraft:cauldron" => Some("leatherworker"),
        "minecraft:stonecutter" => Some("mason"),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VillagerTradeCostSpec {
    pub item: Identifier,
    pub count: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VillagerTradeOfferSpec {
    pub key: &'static str,
    pub cost_a: VillagerTradeCostSpec,
    pub cost_b: Option<VillagerTradeCostSpec>,
    pub result_item: Identifier,
    pub result_count: i32,
    pub max_uses: i32,
    pub xp: i32,
    pub price_multiplier: f32,
}

/// Deterministic offer pool for a profession at one career level (1-5).
/// Values are the deterministic rows of the minecraft.wiki Trading page JE
/// tables (costs, results, max uses, villager XP, price multiplier); offers
/// whose result needs a runtime roll (enchanted gear/books, explorer maps,
/// suspicious stew) or a second cost item are omitted. Returns `None` for
/// unknown professions or out-of-range levels.
#[must_use]
pub fn profession_level_offers_26_1_2(
    profession: &str,
    level: u8,
) -> Option<Vec<VillagerTradeOfferSpec>> {
    if !(1..=5).contains(&level) {
        return None;
    }
    let offers: Vec<VillagerTradeOfferSpec> = match (profession, level) {
        ("armorer", 1) => vec![
            offer(
                "minecraft:smith/1/coal_emerald",
                ("minecraft:coal", 15),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:armorer/1/emerald_iron_leggings",
                ("minecraft:emerald", 7),
                ("minecraft:iron_leggings", 1),
                (12, 1, 0.2),
            ),
            offer(
                "minecraft:armorer/1/emerald_iron_boots",
                ("minecraft:emerald", 4),
                ("minecraft:iron_boots", 1),
                (12, 1, 0.2),
            ),
            offer(
                "minecraft:armorer/1/emerald_iron_helmet",
                ("minecraft:emerald", 5),
                ("minecraft:iron_helmet", 1),
                (12, 1, 0.2),
            ),
            offer(
                "minecraft:armorer/1/emerald_iron_chestplate",
                ("minecraft:emerald", 9),
                ("minecraft:iron_chestplate", 1),
                (12, 1, 0.2),
            ),
        ],
        ("armorer", 2) => vec![
            offer(
                "minecraft:smith/2/iron_ingot_emerald",
                ("minecraft:iron_ingot", 4),
                ("minecraft:emerald", 1),
                (12, 10, 0.05),
            ),
            offer(
                "minecraft:smith/2/emerald_bell",
                ("minecraft:emerald", 36),
                ("minecraft:bell", 1),
                (12, 5, 0.2),
            ),
        ],
        ("armorer", 3) => vec![
            offer(
                "minecraft:armorer/3/lava_bucket_emerald",
                ("minecraft:lava_bucket", 1),
                ("minecraft:emerald", 1),
                (12, 20, 0.05),
            ),
            offer(
                "minecraft:armorer/3/diamond_emerald",
                ("minecraft:diamond", 1),
                ("minecraft:emerald", 1),
                (12, 20, 0.05),
            ),
            offer(
                "minecraft:armorer/3/emerald_shield",
                ("minecraft:emerald", 5),
                ("minecraft:shield", 1),
                (12, 10, 0.2),
            ),
        ],
        ("butcher", 1) => vec![
            offer(
                "minecraft:butcher/1/chicken_emerald",
                ("minecraft:chicken", 14),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:butcher/1/porkchop_emerald",
                ("minecraft:porkchop", 7),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:butcher/1/rabbit_emerald",
                ("minecraft:rabbit", 4),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:butcher/1/emerald_rabbit_stew",
                ("minecraft:emerald", 1),
                ("minecraft:rabbit_stew", 1),
                (12, 1, 0.05),
            ),
        ],
        ("butcher", 2) => vec![
            offer(
                "minecraft:butcher/2/coal_emerald",
                ("minecraft:coal", 15),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:butcher/2/emerald_cooked_porkchop",
                ("minecraft:emerald", 1),
                ("minecraft:cooked_porkchop", 5),
                (16, 5, 0.05),
            ),
            offer(
                "minecraft:butcher/2/emerald_cooked_chicken",
                ("minecraft:emerald", 1),
                ("minecraft:cooked_chicken", 8),
                (16, 5, 0.05),
            ),
        ],
        ("butcher", 3) => vec![
            offer(
                "minecraft:butcher/3/mutton_emerald",
                ("minecraft:mutton", 7),
                ("minecraft:emerald", 1),
                (16, 20, 0.05),
            ),
            offer(
                "minecraft:butcher/3/beef_emerald",
                ("minecraft:beef", 10),
                ("minecraft:emerald", 1),
                (16, 20, 0.05),
            ),
        ],
        ("butcher", 4) => vec![offer(
            "minecraft:butcher/4/dried_kelp_block_emerald",
            ("minecraft:dried_kelp_block", 10),
            ("minecraft:emerald", 1),
            (12, 30, 0.05),
        )],
        ("butcher", 5) => vec![offer(
            "minecraft:butcher/5/sweet_berries_emerald",
            ("minecraft:sweet_berries", 10),
            ("minecraft:emerald", 1),
            (12, 30, 0.05),
        )],
        ("cartographer", 1) => vec![
            offer(
                "minecraft:cartographer/1/paper_emerald",
                ("minecraft:paper", 24),
                ("minecraft:emerald", 1),
                (12, 2, 0.05),
            ),
            offer(
                "minecraft:cartographer/1/emerald_map",
                ("minecraft:emerald", 7),
                ("minecraft:map", 1),
                (12, 1, 0.05),
            ),
        ],
        ("cartographer", 2) => vec![offer(
            "minecraft:cartographer/2/glass_pane_emerald",
            ("minecraft:glass_pane", 11),
            ("minecraft:emerald", 1),
            (12, 10, 0.05),
        )],
        ("cartographer", 3) => vec![offer(
            "minecraft:cartographer/3/compass_emerald",
            ("minecraft:compass", 1),
            ("minecraft:emerald", 1),
            (12, 10, 0.05),
        )],
        ("cartographer", 4) => vec![offer(
            "minecraft:cartographer/4/emerald_item_frame",
            ("minecraft:emerald", 7),
            ("minecraft:item_frame", 1),
            (12, 15, 0.05),
        )],
        ("cartographer", 5) => vec![offer(
            "minecraft:cartographer/5/emerald_globe_banner_pattern",
            ("minecraft:emerald", 8),
            ("minecraft:globe_banner_pattern", 1),
            (12, 30, 0.05),
        )],
        ("cleric", 1) => vec![
            offer(
                "minecraft:cleric/1/rotten_flesh_emerald",
                ("minecraft:rotten_flesh", 32),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:cleric/1/emerald_redstone",
                ("minecraft:emerald", 1),
                ("minecraft:redstone", 2),
                (12, 1, 0.05),
            ),
        ],
        ("cleric", 2) => vec![
            offer(
                "minecraft:cleric/2/gold_ingot_emerald",
                ("minecraft:gold_ingot", 3),
                ("minecraft:emerald", 1),
                (12, 10, 0.05),
            ),
            offer(
                "minecraft:cleric/2/emerald_lapis_lazuli",
                ("minecraft:emerald", 1),
                ("minecraft:lapis_lazuli", 1),
                (12, 5, 0.05),
            ),
        ],
        ("cleric", 3) => vec![
            offer(
                "minecraft:cleric/3/rabbit_foot_emerald",
                ("minecraft:rabbit_foot", 2),
                ("minecraft:emerald", 1),
                (12, 20, 0.05),
            ),
            offer(
                "minecraft:cleric/3/emerald_glowstone",
                ("minecraft:emerald", 4),
                ("minecraft:glowstone", 1),
                (12, 10, 0.05),
            ),
        ],
        ("cleric", 4) => vec![
            offer(
                "minecraft:cleric/4/turtle_scute_emerald",
                ("minecraft:turtle_scute", 4),
                ("minecraft:emerald", 1),
                (12, 30, 0.05),
            ),
            offer(
                "minecraft:cleric/4/glass_bottle_emerald",
                ("minecraft:glass_bottle", 9),
                ("minecraft:emerald", 1),
                (12, 30, 0.05),
            ),
            offer(
                "minecraft:cleric/4/emerald_ender_pearl",
                ("minecraft:emerald", 5),
                ("minecraft:ender_pearl", 1),
                (12, 15, 0.05),
            ),
        ],
        ("cleric", 5) => vec![
            offer(
                "minecraft:cleric/5/nether_wart_emerald",
                ("minecraft:nether_wart", 22),
                ("minecraft:emerald", 1),
                (12, 30, 0.05),
            ),
            offer(
                "minecraft:cleric/5/emerald_experience_bottle",
                ("minecraft:emerald", 3),
                ("minecraft:experience_bottle", 1),
                (12, 30, 0.05),
            ),
        ],
        ("farmer", 1) => vec![
            offer(
                "minecraft:farmer/1/wheat_emerald",
                ("minecraft:wheat", 20),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:farmer/1/potato_emerald",
                ("minecraft:potato", 26),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:farmer/1/carrot_emerald",
                ("minecraft:carrot", 22),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:farmer/1/beetroot_emerald",
                ("minecraft:beetroot", 15),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:farmer/1/emerald_bread",
                ("minecraft:emerald", 1),
                ("minecraft:bread", 6),
                (16, 1, 0.05),
            ),
        ],
        ("farmer", 2) => vec![
            offer(
                "minecraft:farmer/2/pumpkin_emerald",
                ("minecraft:pumpkin", 6),
                ("minecraft:emerald", 1),
                (12, 10, 0.05),
            ),
            offer(
                "minecraft:farmer/2/emerald_pumpkin_pie",
                ("minecraft:emerald", 1),
                ("minecraft:pumpkin_pie", 4),
                (12, 5, 0.05),
            ),
            offer(
                "minecraft:farmer/2/emerald_apple",
                ("minecraft:emerald", 1),
                ("minecraft:apple", 4),
                (16, 5, 0.05),
            ),
        ],
        ("farmer", 3) => vec![
            offer(
                "minecraft:farmer/3/melon_emerald",
                ("minecraft:melon", 4),
                ("minecraft:emerald", 1),
                (12, 20, 0.05),
            ),
            offer(
                "minecraft:farmer/3/emerald_cookie",
                ("minecraft:emerald", 3),
                ("minecraft:cookie", 18),
                (12, 10, 0.05),
            ),
        ],
        ("farmer", 4) => vec![offer(
            "minecraft:farmer/4/emerald_cake",
            ("minecraft:emerald", 1),
            ("minecraft:cake", 1),
            (12, 15, 0.05),
        )],
        ("farmer", 5) => vec![
            offer(
                "minecraft:farmer/5/emerald_golden_carrot",
                ("minecraft:emerald", 3),
                ("minecraft:golden_carrot", 3),
                (12, 30, 0.05),
            ),
            offer(
                "minecraft:farmer/5/emerald_glistening_melon_slice",
                ("minecraft:emerald", 4),
                ("minecraft:glistering_melon_slice", 3),
                (12, 30, 0.05),
            ),
        ],
        ("fisherman", 1) => vec![
            offer(
                "minecraft:fisherman/1/string_emerald",
                ("minecraft:string", 20),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:fisherman/1/coal_emerald",
                ("minecraft:coal", 10),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:fisherman/1/emerald_cod_bucket",
                ("minecraft:emerald", 3),
                ("minecraft:cod_bucket", 1),
                (16, 1, 0.05),
            ),
        ],
        ("fisherman", 2) => vec![
            offer(
                "minecraft:fisherman/2/cod_emerald",
                ("minecraft:cod", 15),
                ("minecraft:emerald", 1),
                (16, 10, 0.05),
            ),
            offer(
                "minecraft:fisherman/2/emerald_campfire",
                ("minecraft:emerald", 2),
                ("minecraft:campfire", 1),
                (12, 5, 0.05),
            ),
        ],
        ("fisherman", 3) => vec![offer(
            "minecraft:fisherman/3/salmon_emerald",
            ("minecraft:salmon", 13),
            ("minecraft:emerald", 1),
            (16, 20, 0.05),
        )],
        ("fisherman", 4) => vec![offer(
            "minecraft:fisherman/4/tropical_fish_emerald",
            ("minecraft:tropical_fish", 6),
            ("minecraft:emerald", 1),
            (12, 30, 0.05),
        )],
        ("fisherman", 5) => vec![
            offer(
                "minecraft:fisherman/5/pufferfish_emerald",
                ("minecraft:pufferfish", 4),
                ("minecraft:emerald", 1),
                (12, 30, 0.05),
            ),
            offer(
                "minecraft:fisherman/5/oak_boat_emerald",
                ("minecraft:oak_boat", 1),
                ("minecraft:emerald", 1),
                (12, 30, 0.05),
            ),
        ],
        ("fletcher", 1) => vec![
            offer(
                "minecraft:fletcher/1/stick_emerald",
                ("minecraft:stick", 32),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:fletcher/1/emerald_arrow",
                ("minecraft:emerald", 1),
                ("minecraft:arrow", 16),
                (12, 1, 0.05),
            ),
        ],
        ("fletcher", 2) => vec![
            offer(
                "minecraft:fletcher/2/flint_emerald",
                ("minecraft:flint", 26),
                ("minecraft:emerald", 1),
                (12, 10, 0.05),
            ),
            offer(
                "minecraft:fletcher/2/emerald_bow",
                ("minecraft:emerald", 2),
                ("minecraft:bow", 1),
                (12, 5, 0.05),
            ),
        ],
        ("fletcher", 3) => vec![
            offer(
                "minecraft:fletcher/3/string_emerald",
                ("minecraft:string", 14),
                ("minecraft:emerald", 1),
                (16, 20, 0.05),
            ),
            offer(
                "minecraft:fletcher/3/emerald_crossbow",
                ("minecraft:emerald", 3),
                ("minecraft:crossbow", 1),
                (12, 10, 0.05),
            ),
        ],
        ("fletcher", 4) => vec![offer(
            "minecraft:fletcher/4/feather_emerald",
            ("minecraft:feather", 24),
            ("minecraft:emerald", 1),
            (16, 30, 0.05),
        )],
        ("fletcher", 5) => vec![offer(
            "minecraft:fletcher/5/tripwire_hook_emerald",
            ("minecraft:tripwire_hook", 8),
            ("minecraft:emerald", 1),
            (12, 30, 0.05),
        )],
        ("leatherworker", 1) => vec![
            offer(
                "minecraft:leatherworker/1/leather_emerald",
                ("minecraft:leather", 6),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:leatherworker/1/emerald_dyed_leather_leggings",
                ("minecraft:emerald", 3),
                ("minecraft:leather_leggings", 1),
                (12, 1, 0.2),
            ),
            offer(
                "minecraft:leatherworker/1/emerald_dyed_leather_chestplate",
                ("minecraft:emerald", 7),
                ("minecraft:leather_chestplate", 1),
                (12, 1, 0.2),
            ),
        ],
        ("leatherworker", 2) => vec![
            offer(
                "minecraft:leatherworker/2/flint_emerald",
                ("minecraft:flint", 26),
                ("minecraft:emerald", 1),
                (12, 10, 0.05),
            ),
            offer(
                "minecraft:leatherworker/2/emerald_dyed_leather_helmet",
                ("minecraft:emerald", 5),
                ("minecraft:leather_helmet", 1),
                (12, 5, 0.2),
            ),
            offer(
                "minecraft:leatherworker/2/emerald_dyed_leather_boots",
                ("minecraft:emerald", 4),
                ("minecraft:leather_boots", 1),
                (12, 5, 0.2),
            ),
        ],
        ("leatherworker", 3) => vec![
            offer(
                "minecraft:leatherworker/3/rabbit_hide_emerald",
                ("minecraft:rabbit_hide", 9),
                ("minecraft:emerald", 1),
                (12, 20, 0.05),
            ),
            offer(
                "minecraft:leatherworker/3/emerald_dyed_leather_chestplate",
                ("minecraft:emerald", 7),
                ("minecraft:leather_chestplate", 1),
                (12, 10, 0.2),
            ),
        ],
        ("leatherworker", 4) => vec![
            offer(
                "minecraft:leatherworker/4/turtle_scute_emerald",
                ("minecraft:turtle_scute", 4),
                ("minecraft:emerald", 1),
                (12, 30, 0.05),
            ),
            offer(
                "minecraft:leatherworker/4/emerald_dyed_leather_horse_armor",
                ("minecraft:emerald", 6),
                ("minecraft:leather_horse_armor", 1),
                (12, 15, 0.2),
            ),
        ],
        ("leatherworker", 5) => vec![
            offer(
                "minecraft:leatherworker/5/emerald_dyed_leather_helmet",
                ("minecraft:emerald", 5),
                ("minecraft:leather_helmet", 1),
                (12, 30, 0.2),
            ),
            offer(
                "minecraft:leatherworker/5/emerald_saddle",
                ("minecraft:emerald", 6),
                ("minecraft:saddle", 1),
                (12, 30, 0.2),
            ),
        ],
        ("librarian", 1) => vec![
            offer(
                "minecraft:librarian/1/paper_emerald",
                ("minecraft:paper", 24),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:librarian/1/emerald_bookshelf",
                ("minecraft:emerald", 9),
                ("minecraft:bookshelf", 1),
                (12, 1, 0.05),
            ),
        ],
        ("librarian", 2) => vec![
            offer(
                "minecraft:librarian/2/book_emerald",
                ("minecraft:book", 4),
                ("minecraft:emerald", 1),
                (12, 10, 0.05),
            ),
            offer(
                "minecraft:librarian/2/emerald_lantern",
                ("minecraft:emerald", 1),
                ("minecraft:lantern", 1),
                (12, 5, 0.05),
            ),
        ],
        ("librarian", 3) => vec![
            offer(
                "minecraft:librarian/3/ink_sac_emerald",
                ("minecraft:ink_sac", 5),
                ("minecraft:emerald", 1),
                (12, 20, 0.05),
            ),
            offer(
                "minecraft:librarian/3/emerald_glass",
                ("minecraft:emerald", 1),
                ("minecraft:glass", 4),
                (12, 10, 0.05),
            ),
        ],
        ("librarian", 4) => vec![
            offer(
                "minecraft:librarian/4/emerald_compass",
                ("minecraft:emerald", 4),
                ("minecraft:compass", 1),
                (12, 15, 0.05),
            ),
            offer(
                "minecraft:librarian/4/emerald_clock",
                ("minecraft:emerald", 5),
                ("minecraft:clock", 1),
                (12, 15, 0.05),
            ),
        ],
        ("librarian", 5) => vec![
            offer(
                "minecraft:librarian/5/emerald_red_candle",
                ("minecraft:emerald", 3),
                ("minecraft:red_candle", 1),
                (12, 30, 0.05),
            ),
            offer(
                "minecraft:librarian/5/emerald_yellow_candle",
                ("minecraft:emerald", 3),
                ("minecraft:yellow_candle", 1),
                (12, 30, 0.05),
            ),
        ],
        ("mason", 1) => vec![
            offer(
                "minecraft:mason/1/clay_ball_emerald",
                ("minecraft:clay_ball", 10),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:mason/1/emerald_brick",
                ("minecraft:emerald", 1),
                ("minecraft:brick", 10),
                (16, 1, 0.05),
            ),
        ],
        ("mason", 2) => vec![
            offer(
                "minecraft:mason/2/stone_emerald",
                ("minecraft:stone", 20),
                ("minecraft:emerald", 1),
                (16, 10, 0.05),
            ),
            offer(
                "minecraft:mason/2/emerald_chiseled_stone_bricks",
                ("minecraft:emerald", 1),
                ("minecraft:chiseled_stone_bricks", 4),
                (16, 5, 0.05),
            ),
        ],
        ("mason", 3) => vec![
            offer(
                "minecraft:mason/3/granite_emerald",
                ("minecraft:granite", 16),
                ("minecraft:emerald", 1),
                (16, 20, 0.05),
            ),
            offer(
                "minecraft:mason/3/andesite_emerald",
                ("minecraft:andesite", 16),
                ("minecraft:emerald", 1),
                (16, 20, 0.05),
            ),
            offer(
                "minecraft:mason/3/diorite_emerald",
                ("minecraft:diorite", 16),
                ("minecraft:emerald", 1),
                (16, 20, 0.05),
            ),
            offer(
                "minecraft:mason/3/emerald_dripstone_block",
                ("minecraft:emerald", 1),
                ("minecraft:dripstone_block", 4),
                (16, 10, 0.05),
            ),
            offer(
                "minecraft:mason/3/emerald_polished_andesite",
                ("minecraft:emerald", 1),
                ("minecraft:polished_andesite", 4),
                (16, 10, 0.05),
            ),
            offer(
                "minecraft:mason/3/emerald_polished_diorite",
                ("minecraft:emerald", 1),
                ("minecraft:polished_diorite", 4),
                (16, 10, 0.05),
            ),
            offer(
                "minecraft:mason/3/emerald_polished_granite",
                ("minecraft:emerald", 1),
                ("minecraft:polished_granite", 4),
                (16, 10, 0.05),
            ),
        ],
        ("mason", 4) => vec![offer(
            "minecraft:mason/4/quartz_emerald",
            ("minecraft:quartz", 12),
            ("minecraft:emerald", 1),
            (12, 30, 0.05),
        )],
        ("mason", 5) => vec![
            offer(
                "minecraft:mason/5/emerald_quartz_pillar",
                ("minecraft:emerald", 1),
                ("minecraft:quartz_pillar", 1),
                (12, 30, 0.05),
            ),
            offer(
                "minecraft:mason/5/emerald_quartz_block",
                ("minecraft:emerald", 1),
                ("minecraft:quartz_block", 1),
                (12, 30, 0.05),
            ),
        ],
        ("shepherd", 1) => vec![
            offer(
                "minecraft:shepherd/1/white_wool_emerald",
                ("minecraft:white_wool", 18),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:shepherd/1/brown_wool_emerald",
                ("minecraft:brown_wool", 18),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:shepherd/1/gray_wool_emerald",
                ("minecraft:gray_wool", 18),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:shepherd/1/black_wool_emerald",
                ("minecraft:black_wool", 18),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:shepherd/1/emerald_shears",
                ("minecraft:emerald", 2),
                ("minecraft:shears", 1),
                (12, 1, 0.05),
            ),
        ],
        ("shepherd", 2) => vec![
            offer(
                "minecraft:shepherd/2/white_dye_emerald",
                ("minecraft:white_dye", 12),
                ("minecraft:emerald", 1),
                (16, 10, 0.05),
            ),
            offer(
                "minecraft:shepherd/2/gray_dye_emerald",
                ("minecraft:gray_dye", 12),
                ("minecraft:emerald", 1),
                (16, 10, 0.05),
            ),
        ],
        ("shepherd", 3) => vec![
            offer(
                "minecraft:shepherd/3/yellow_dye_emerald",
                ("minecraft:yellow_dye", 12),
                ("minecraft:emerald", 1),
                (16, 20, 0.05),
            ),
            offer(
                "minecraft:shepherd/3/orange_dye_emerald",
                ("minecraft:orange_dye", 12),
                ("minecraft:emerald", 1),
                (16, 20, 0.05),
            ),
        ],
        ("shepherd", 4) => vec![
            offer(
                "minecraft:shepherd/4/brown_dye_emerald",
                ("minecraft:brown_dye", 12),
                ("minecraft:emerald", 1),
                (16, 30, 0.05),
            ),
            offer(
                "minecraft:shepherd/4/blue_dye_emerald",
                ("minecraft:blue_dye", 12),
                ("minecraft:emerald", 1),
                (16, 30, 0.05),
            ),
        ],
        ("shepherd", 5) => vec![offer(
            "minecraft:shepherd/5/emerald_painting",
            ("minecraft:emerald", 2),
            ("minecraft:painting", 3),
            (12, 30, 0.05),
        )],
        ("toolsmith", 1) => toolsmith_novice_offers_26_1_2(),
        ("toolsmith", 2) => vec![
            offer(
                "minecraft:smith/2/iron_ingot_emerald",
                ("minecraft:iron_ingot", 4),
                ("minecraft:emerald", 1),
                (12, 10, 0.05),
            ),
            offer(
                "minecraft:smith/2/emerald_bell",
                ("minecraft:emerald", 36),
                ("minecraft:bell", 1),
                (12, 5, 0.2),
            ),
        ],
        ("toolsmith", 3) => vec![
            offer(
                "minecraft:toolsmith/3/flint_emerald",
                ("minecraft:flint", 30),
                ("minecraft:emerald", 1),
                (12, 20, 0.05),
            ),
            offer(
                "minecraft:toolsmith/3/emerald_diamond_hoe",
                ("minecraft:emerald", 4),
                ("minecraft:diamond_hoe", 1),
                (3, 10, 0.2),
            ),
        ],
        ("toolsmith", 4) => vec![offer(
            "minecraft:toolsmith/4/diamond_emerald",
            ("minecraft:diamond", 1),
            ("minecraft:emerald", 1),
            (12, 30, 0.05),
        )],
        ("weaponsmith", 1) => vec![
            offer(
                "minecraft:smith/1/coal_emerald",
                ("minecraft:coal", 15),
                ("minecraft:emerald", 1),
                (16, 2, 0.05),
            ),
            offer(
                "minecraft:weaponsmith/1/emerald_iron_axe",
                ("minecraft:emerald", 3),
                ("minecraft:iron_axe", 1),
                (12, 1, 0.2),
            ),
        ],
        ("weaponsmith", 2) => vec![
            offer(
                "minecraft:smith/2/iron_ingot_emerald",
                ("minecraft:iron_ingot", 4),
                ("minecraft:emerald", 1),
                (12, 10, 0.05),
            ),
            offer(
                "minecraft:smith/2/emerald_bell",
                ("minecraft:emerald", 36),
                ("minecraft:bell", 1),
                (12, 5, 0.2),
            ),
        ],
        ("weaponsmith", 3) => vec![offer(
            "minecraft:weaponsmith/3/flint_emerald",
            ("minecraft:flint", 24),
            ("minecraft:emerald", 1),
            (12, 20, 0.05),
        )],
        ("weaponsmith", 4) => vec![offer(
            "minecraft:weaponsmith/4/diamond_emerald",
            ("minecraft:diamond", 1),
            ("minecraft:emerald", 1),
            (12, 30, 0.05),
        )],
        ("armorer", 4) | ("armorer", 5) | ("toolsmith", 5) | ("weaponsmith", 5) => vec![],
        _ => return None,
    };
    Some(offers)
}

pub fn toolsmith_novice_offers_26_1_2() -> Vec<VillagerTradeOfferSpec> {
    vec![
        // Vanilla names the shared smith offer `smith/1/coal_emerald` and
        // includes it in toolsmith level 1 through the `common_smith` trade tag;
        // it is intentionally not under the `toolsmith/` namespace.
        offer(
            "minecraft:smith/1/coal_emerald",
            ("minecraft:coal", 15),
            ("minecraft:emerald", 1),
            (16, 2, 0.05),
        ),
        offer(
            "minecraft:toolsmith/1/emerald_stone_axe",
            ("minecraft:emerald", 1),
            ("minecraft:stone_axe", 1),
            (12, 1, 0.2),
        ),
        offer(
            "minecraft:toolsmith/1/emerald_stone_shovel",
            ("minecraft:emerald", 1),
            ("minecraft:stone_shovel", 1),
            (12, 1, 0.2),
        ),
        offer(
            "minecraft:toolsmith/1/emerald_stone_pickaxe",
            ("minecraft:emerald", 1),
            ("minecraft:stone_pickaxe", 1),
            (12, 1, 0.2),
        ),
        offer(
            "minecraft:toolsmith/1/emerald_stone_hoe",
            ("minecraft:emerald", 1),
            ("minecraft:stone_hoe", 1),
            (12, 1, 0.2),
        ),
    ]
}

fn offer(
    key: &'static str,
    cost: (&str, i32),
    result: (&str, i32),
    policy: (i32, i32, f32),
) -> VillagerTradeOfferSpec {
    let (cost_item, cost_count) = cost;
    let (result_item, result_count) = result;
    let (max_uses, xp, price_multiplier) = policy;
    VillagerTradeOfferSpec {
        key,
        cost_a: VillagerTradeCostSpec {
            item: Identifier::parse(cost_item).expect("static trade cost identifier"),
            count: cost_count,
        },
        cost_b: None,
        result_item: Identifier::parse(result_item).expect("static trade result identifier"),
        result_count,
        max_uses,
        xp,
        price_multiplier,
    }
}

#[cfg(test)]
mod villager_level_unlock_tests {
    use super::*;

    #[test]
    fn level_thresholds_match_wiki_trading_page() {
        // https://minecraft.wiki/w/Trading Level table: 0/10/70/150/250.
        assert_eq!(VILLAGER_LEVEL_XP_THRESHOLDS_26_1_2, [0, 10, 70, 150, 250]);
        assert_eq!(villager_level_for_xp_26_1_2(0), 1);
        assert_eq!(villager_level_for_xp_26_1_2(9), 1);
        assert_eq!(villager_level_for_xp_26_1_2(10), 2);
        assert_eq!(villager_level_for_xp_26_1_2(69), 2);
        assert_eq!(villager_level_for_xp_26_1_2(70), 3);
        assert_eq!(villager_level_for_xp_26_1_2(149), 3);
        assert_eq!(villager_level_for_xp_26_1_2(150), 4);
        assert_eq!(villager_level_for_xp_26_1_2(249), 4);
        assert_eq!(villager_level_for_xp_26_1_2(250), 5);
        assert_eq!(villager_level_for_xp_26_1_2(10_000), 5);
    }

    #[test]
    fn every_profession_has_novice_pool_and_farmer_matches_wiki() {
        for profession in [
            "armorer",
            "butcher",
            "cartographer",
            "cleric",
            "farmer",
            "fisherman",
            "fletcher",
            "leatherworker",
            "librarian",
            "mason",
            "shepherd",
            "toolsmith",
            "weaponsmith",
        ] {
            let novice = profession_level_offers_26_1_2(profession, 1)
                .unwrap_or_else(|| panic!("{profession} novice pool"));
            assert!(!novice.is_empty(), "{profession}");
        }
        // Wiki Farmer novice rows: 20 wheat / 26 potato / 22 carrot /
        // 15 beetroot -> emerald, 1 emerald -> 6 bread.
        let farmer = profession_level_offers_26_1_2("farmer", 1).unwrap();
        assert_eq!(farmer.len(), 5);
        assert_eq!(farmer[0].cost_a.item.as_str(), "minecraft:wheat");
        assert_eq!(farmer[0].cost_a.count, 20);
        assert_eq!(farmer[4].cost_a.item.as_str(), "minecraft:emerald");
        assert_eq!(farmer[4].result_item.as_str(), "minecraft:bread");
        assert_eq!(farmer[4].result_count, 6);
        // Unknown profession / level fails closed.
        assert!(profession_level_offers_26_1_2("nitwit", 1).is_none());
        assert!(profession_level_offers_26_1_2("farmer", 0).is_none());
        assert!(profession_level_offers_26_1_2("farmer", 6).is_none());
    }

    #[test]
    fn toolsmith_unlock_path_matches_wiki_rows() {
        let apprentice = profession_level_offers_26_1_2("toolsmith", 2).unwrap();
        assert_eq!(apprentice.len(), 2);
        assert_eq!(apprentice[0].cost_a.item.as_str(), "minecraft:iron_ingot");
        assert_eq!(apprentice[0].cost_a.count, 4);
        assert_eq!(apprentice[0].xp, 10);
        let journeyman = profession_level_offers_26_1_2("toolsmith", 3).unwrap();
        assert!(
            journeyman
                .iter()
                .any(|o| o.key == "minecraft:toolsmith/3/flint_emerald")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn novice_toolsmith_catalog_matches_local_26_1_2_oracle() {
        let offers = toolsmith_novice_offers_26_1_2();
        assert_eq!(offers.len(), 5);
        assert_eq!(offers[0].key, "minecraft:smith/1/coal_emerald");
        assert_eq!(offers[0].cost_a.item.as_str(), "minecraft:coal");
        assert_eq!(offers[0].cost_a.count, 15);
        assert_eq!(offers[0].result_item.as_str(), "minecraft:emerald");
        assert_eq!(offers[0].max_uses, 16);
        assert_eq!(offers[0].xp, 2);
        assert_eq!(offers[0].price_multiplier, 0.05);
        assert!(offers[1..].iter().all(|offer| {
            offer.cost_a.item.as_str() == "minecraft:emerald"
                && offer.cost_a.count == 1
                && offer.result_count == 1
                && offer.max_uses == 12
                && offer.xp == 1
                && offer.price_multiplier == 0.2
        }));
    }

    #[test]
    fn every_vanilla_job_site_maps_to_its_registry_profession() {
        let cases = [
            ("minecraft:composter", "farmer"),
            ("minecraft:barrel", "fisherman"),
            ("minecraft:cartography_table", "cartographer"),
            ("minecraft:brewing_stand", "cleric"),
            ("minecraft:lectern", "librarian"),
            ("minecraft:fletching_table", "fletcher"),
            ("minecraft:loom", "shepherd"),
            ("minecraft:smithing_table", "toolsmith"),
            ("minecraft:blast_furnace", "armorer"),
            ("minecraft:grindstone", "weaponsmith"),
            ("minecraft:smoker", "butcher"),
            ("minecraft:cauldron", "leatherworker"),
            ("minecraft:stonecutter", "mason"),
        ];
        for (job_site, profession) in cases {
            assert_eq!(
                supported_profession_for_job_site_26_1_2(&Identifier::parse(job_site).unwrap()),
                Some(profession),
                "{job_site}"
            );
        }
        // Non-job-site blocks fail closed.
        assert_eq!(
            supported_profession_for_job_site_26_1_2(
                &Identifier::parse("minecraft:stone").unwrap()
            ),
            None
        );
    }
}
