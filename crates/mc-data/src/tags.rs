//! Tag-network data for the `Update Tags` packet.
//!
//! Walks `<vanilla_dir>/data/minecraft/tags/<root>/**/*.json`, resolves
//! `#tag`-references transitively, dedupes entries, and maps each
//! entry identifier to the numeric wire id the client expects:
//!
//! - For *data-driven* registries the server itself sends via
//!   `RegistryData` (enchantment, damage_type, instrument,
//!   banner_pattern, painting_variant, worldgen/biome) — the entry id
//!   is the position of the entry in our sorted `Registry.entries`.
//!   The client indexes by send order, not by Mojang's `protocol_id`.
//! - For *built-in* registries the client knows natively (item, block,
//!   entity_type, fluid, game_event, …) — the entry id is the
//!   `protocol_id` from `<vanilla_dir>/reports/registries.json`.
//!
//! Output is a flat `TagsData` ready to feed into one
//! `mc_protocol::packets::configuration::UpdateTags` packet.

#[path = "banner_pattern_tags.rs"]
mod banner_pattern_tags;
#[path = "enchantment_tags.rs"]
mod enchantment_tags;

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;
use tracing::{debug, warn};

use crate::{
    Identifier, VanillaData, fuel_values::FuelValues, items::ItemRegistry, read_json_file,
    visit_json_files,
};

const REQUIRED_ITEM_TAGS: &str = include_str!("../data/required_item_tags.json");
const FLOWING_WATER_RAW_ID: i32 = 1;
const WATER_RAW_ID: i32 = 2;
const FLOWING_LAVA_RAW_ID: i32 = 3;
const LAVA_RAW_ID: i32 = 4;

/// `(tags/<fs subpath>, "minecraft:<registry id>")` pairs the loader
/// knows about. We deliberately limit the set to registries the
/// vanilla client either knows natively (built-in `block`, `item`,
/// `entity_type`, `fluid`, `game_event`, `point_of_interest_type`,
/// `potion`) or that *we* send via `RegistryData` (anything in
/// [`KNOWN_REGISTRIES`]). Sending tags for a registry the client has
/// never heard of trips
/// `RegistryAccess.lookupOrThrow → Missing registry: …` during the
/// Configuration → Play transition, even if the tag entry list is
/// empty — see the M3.i log for the wire repro.
///
/// Registries excluded on purpose: `enchantment_provider`,
/// `test_environment`, `test_instance`, `trade_set`,
/// `villager_trade`, `worldgen/configured_feature`,
/// `worldgen/flat_level_generator_preset`, `worldgen/structure`,
/// `worldgen/world_preset`. Solaris does not currently advertise them
/// through `RegistryData`, and shipping tags for a registry the client
/// never learned about trips the Configuration → Play transition.
const TAG_ROOTS: &[(&str, &str)] = &[
    ("banner_pattern", "minecraft:banner_pattern"),
    ("block", "minecraft:block"),
    ("damage_type", "minecraft:damage_type"),
    ("dialog", "minecraft:dialog"),
    ("enchantment", "minecraft:enchantment"),
    ("entity_type", "minecraft:entity_type"),
    ("fluid", "minecraft:fluid"),
    ("game_event", "minecraft:game_event"),
    ("instrument", "minecraft:instrument"),
    ("item", "minecraft:item"),
    ("painting_variant", "minecraft:painting_variant"),
    ("point_of_interest_type", "minecraft:point_of_interest_type"),
    ("potion", "minecraft:potion"),
    ("timeline", "minecraft:timeline"),
    ("worldgen/biome", "minecraft:worldgen/biome"),
];

const REQUIRED_CLIENT_FALLBACK_TAGS: &[(&str, &[&str])] = &[
    (
        "minecraft:banner_pattern",
        &[
            "minecraft:no_item_required",
            "minecraft:pattern_item/bordure_indented",
            "minecraft:pattern_item/creeper",
            "minecraft:pattern_item/field_masoned",
            "minecraft:pattern_item/flow",
            "minecraft:pattern_item/flower",
            "minecraft:pattern_item/globe",
            "minecraft:pattern_item/guster",
            "minecraft:pattern_item/mojang",
            "minecraft:pattern_item/piglin",
            "minecraft:pattern_item/skull",
        ],
    ),
    (
        "minecraft:block",
        &[
            "minecraft:blocks_wind_charge_explosions",
            "minecraft:infiniburn_end",
            "minecraft:infiniburn_nether",
            "minecraft:infiniburn_overworld",
            "minecraft:lightning_rods",
            "minecraft:soul_speed_blocks",
        ],
    ),
    (
        "minecraft:damage_type",
        &[
            "minecraft:always_hurts_ender_dragons",
            "minecraft:always_kills_armor_stands",
            "minecraft:always_most_significant_fall",
            "minecraft:always_triggers_silverfish",
            "minecraft:avoids_guardian_thorns",
            "minecraft:burn_from_stepping",
            "minecraft:burns_armor_stands",
            "minecraft:bypasses_armor",
            "minecraft:bypasses_effects",
            "minecraft:bypasses_enchantments",
            "minecraft:bypasses_invulnerability",
            "minecraft:bypasses_resistance",
            "minecraft:bypasses_shield",
            "minecraft:bypasses_wolf_armor",
            "minecraft:can_break_armor_stand",
            "minecraft:damages_helmet",
            "minecraft:ignites_armor_stands",
            "minecraft:is_drowning",
            "minecraft:is_explosion",
            "minecraft:is_fall",
            "minecraft:is_fire",
            "minecraft:is_freezing",
            "minecraft:is_lightning",
            "minecraft:is_player_attack",
            "minecraft:is_projectile",
            "minecraft:mace_smash",
            "minecraft:no_anger",
            "minecraft:no_impact",
            "minecraft:no_knockback",
            "minecraft:panic_causes",
            "minecraft:panic_environmental_causes",
            "minecraft:witch_resistant_to",
            "minecraft:wither_immune_to",
        ],
    ),
    (
        "minecraft:dialog",
        &[
            "minecraft:pause_screen_additions",
            "minecraft:quick_actions",
        ],
    ),
    (
        "minecraft:enchantment",
        &[
            "minecraft:exclusive_set/armor",
            "minecraft:exclusive_set/boots",
            "minecraft:exclusive_set/bow",
            "minecraft:exclusive_set/crossbow",
            "minecraft:exclusive_set/damage",
            "minecraft:exclusive_set/mining",
            "minecraft:exclusive_set/riptide",
        ],
    ),
    (
        "minecraft:entity_type",
        &[
            "minecraft:arrows",
            "minecraft:sensitive_to_bane_of_arthropods",
            "minecraft:sensitive_to_impaling",
            "minecraft:sensitive_to_smite",
        ],
    ),
    (
        "minecraft:item",
        &[
            "minecraft:enchantable/armor",
            "minecraft:enchantable/bow",
            "minecraft:enchantable/chest_armor",
            "minecraft:enchantable/crossbow",
            "minecraft:enchantable/durability",
            "minecraft:enchantable/equippable",
            "minecraft:enchantable/fire_aspect",
            "minecraft:enchantable/fishing",
            "minecraft:enchantable/foot_armor",
            "minecraft:enchantable/head_armor",
            "minecraft:enchantable/leg_armor",
            "minecraft:enchantable/lunge",
            "minecraft:enchantable/mace",
            "minecraft:enchantable/melee_weapon",
            "minecraft:enchantable/mining",
            "minecraft:enchantable/mining_loot",
            "minecraft:enchantable/sharp_weapon",
            "minecraft:enchantable/sweeping",
            "minecraft:enchantable/trident",
            "minecraft:enchantable/vanishing",
            "minecraft:enchantable/weapon",
        ],
    ),
    (
        "minecraft:timeline",
        &[
            "minecraft:in_end",
            "minecraft:in_nether",
            "minecraft:in_overworld",
            "minecraft:universal",
        ],
    ),
];

const INCORRECT_FOR_WOODEN_OR_GOLD_TOOL: &[&str] = &[
    "minecraft:iron_block",
    "minecraft:raw_iron_block",
    "minecraft:iron_ore",
    "minecraft:deepslate_iron_ore",
    "minecraft:lapis_block",
    "minecraft:lapis_ore",
    "minecraft:deepslate_lapis_ore",
    "minecraft:copper_block",
    "minecraft:raw_copper_block",
    "minecraft:copper_ore",
    "minecraft:deepslate_copper_ore",
    "minecraft:diamond_block",
    "minecraft:diamond_ore",
    "minecraft:deepslate_diamond_ore",
    "minecraft:emerald_ore",
    "minecraft:deepslate_emerald_ore",
    "minecraft:emerald_block",
    "minecraft:gold_block",
    "minecraft:raw_gold_block",
    "minecraft:gold_ore",
    "minecraft:deepslate_gold_ore",
    "minecraft:redstone_ore",
    "minecraft:deepslate_redstone_ore",
    "minecraft:obsidian",
    "minecraft:crying_obsidian",
    "minecraft:netherite_block",
    "minecraft:ancient_debris",
];

const INCORRECT_FOR_STONE_OR_COPPER_TOOL: &[&str] = &[
    "minecraft:diamond_block",
    "minecraft:diamond_ore",
    "minecraft:deepslate_diamond_ore",
    "minecraft:emerald_ore",
    "minecraft:deepslate_emerald_ore",
    "minecraft:emerald_block",
    "minecraft:gold_block",
    "minecraft:raw_gold_block",
    "minecraft:gold_ore",
    "minecraft:deepslate_gold_ore",
    "minecraft:redstone_ore",
    "minecraft:deepslate_redstone_ore",
    "minecraft:obsidian",
    "minecraft:crying_obsidian",
    "minecraft:netherite_block",
    "minecraft:ancient_debris",
];

const REQUIRED_BLOCK_TAGS: &[(&str, &[&str])] = &[
    (
        "minecraft:climbable",
        &[
            "minecraft:ladder",
            "minecraft:vine",
            "minecraft:scaffolding",
            "minecraft:weeping_vines",
            "minecraft:weeping_vines_plant",
            "minecraft:twisting_vines",
            "minecraft:twisting_vines_plant",
            "minecraft:cave_vines",
            "minecraft:cave_vines_plant",
        ],
    ),
    (
        "minecraft:soul_speed_blocks",
        &["minecraft:soul_sand", "minecraft:soul_soil"],
    ),
    (
        "minecraft:mineable/pickaxe",
        &[
            "minecraft:stone",
            "minecraft:granite",
            "minecraft:polished_granite",
            "minecraft:diorite",
            "minecraft:polished_diorite",
            "minecraft:andesite",
            "minecraft:polished_andesite",
            "minecraft:cobblestone",
            "minecraft:mossy_cobblestone",
            "minecraft:deepslate",
            "minecraft:cobbled_deepslate",
            "minecraft:polished_deepslate",
            "minecraft:calcite",
            "minecraft:tuff",
            "minecraft:dripstone_block",
            "minecraft:coal_ore",
            "minecraft:deepslate_coal_ore",
            "minecraft:iron_ore",
            "minecraft:deepslate_iron_ore",
            "minecraft:copper_ore",
            "minecraft:deepslate_copper_ore",
            "minecraft:gold_ore",
            "minecraft:deepslate_gold_ore",
            "minecraft:redstone_ore",
            "minecraft:deepslate_redstone_ore",
            "minecraft:emerald_ore",
            "minecraft:deepslate_emerald_ore",
            "minecraft:lapis_ore",
            "minecraft:deepslate_lapis_ore",
            "minecraft:diamond_ore",
            "minecraft:deepslate_diamond_ore",
            "minecraft:nether_gold_ore",
            "minecraft:nether_quartz_ore",
            "minecraft:ancient_debris",
            "minecraft:netherrack",
            "minecraft:basalt",
            "minecraft:smooth_basalt",
            "minecraft:blackstone",
            "minecraft:end_stone",
            "minecraft:furnace",
            "minecraft:blast_furnace",
            "minecraft:smoker",
            "minecraft:iron_block",
            "minecraft:raw_iron_block",
            "minecraft:copper_block",
            "minecraft:raw_copper_block",
            "minecraft:gold_block",
            "minecraft:raw_gold_block",
            "minecraft:lapis_block",
            "minecraft:diamond_block",
            "minecraft:emerald_block",
            "minecraft:obsidian",
            "minecraft:crying_obsidian",
            "minecraft:netherite_block",
            "minecraft:activator_rail",
            "minecraft:amethyst_block",
            "minecraft:amethyst_cluster",
            "minecraft:andesite_slab",
            "minecraft:andesite_stairs",
            "minecraft:andesite_wall",
            "minecraft:anvil",
            "minecraft:bell",
            "minecraft:black_concrete",
            "minecraft:black_glazed_terracotta",
            "minecraft:black_shulker_box",
            "minecraft:black_terracotta",
            "minecraft:blackstone_slab",
            "minecraft:blackstone_stairs",
            "minecraft:blackstone_wall",
            "minecraft:blue_concrete",
            "minecraft:blue_glazed_terracotta",
            "minecraft:blue_ice",
            "minecraft:blue_shulker_box",
            "minecraft:blue_terracotta",
            "minecraft:bone_block",
            "minecraft:brain_coral_block",
            "minecraft:brewing_stand",
            "minecraft:brick_slab",
            "minecraft:brick_stairs",
            "minecraft:brick_wall",
            "minecraft:bricks",
            "minecraft:brown_concrete",
            "minecraft:brown_glazed_terracotta",
            "minecraft:brown_shulker_box",
            "minecraft:brown_terracotta",
            "minecraft:bubble_coral_block",
            "minecraft:budding_amethyst",
            "minecraft:cauldron",
            "minecraft:chipped_anvil",
            "minecraft:chiseled_copper",
            "minecraft:chiseled_deepslate",
            "minecraft:chiseled_nether_bricks",
            "minecraft:chiseled_polished_blackstone",
            "minecraft:chiseled_quartz_block",
            "minecraft:chiseled_red_sandstone",
            "minecraft:chiseled_resin_bricks",
            "minecraft:chiseled_sandstone",
            "minecraft:chiseled_stone_bricks",
            "minecraft:chiseled_tuff",
            "minecraft:chiseled_tuff_bricks",
            "minecraft:coal_block",
            "minecraft:cobbled_deepslate_slab",
            "minecraft:cobbled_deepslate_stairs",
            "minecraft:cobbled_deepslate_wall",
            "minecraft:cobblestone_slab",
            "minecraft:cobblestone_stairs",
            "minecraft:cobblestone_wall",
            "minecraft:conduit",
            "minecraft:copper_bars",
            "minecraft:copper_bulb",
            "minecraft:copper_chain",
            "minecraft:copper_chest",
            "minecraft:copper_door",
            "minecraft:copper_golem_statue",
            "minecraft:copper_grate",
            "minecraft:copper_lantern",
            "minecraft:copper_trapdoor",
            "minecraft:cracked_deepslate_bricks",
            "minecraft:cracked_deepslate_tiles",
            "minecraft:cracked_nether_bricks",
            "minecraft:cracked_polished_blackstone_bricks",
            "minecraft:cracked_stone_bricks",
            "minecraft:crafter",
            "minecraft:crimson_nylium",
            "minecraft:cut_copper",
            "minecraft:cut_copper_slab",
            "minecraft:cut_copper_stairs",
            "minecraft:cut_red_sandstone",
            "minecraft:cut_red_sandstone_slab",
            "minecraft:cut_sandstone",
            "minecraft:cut_sandstone_slab",
            "minecraft:cyan_concrete",
            "minecraft:cyan_glazed_terracotta",
            "minecraft:cyan_shulker_box",
            "minecraft:cyan_terracotta",
            "minecraft:damaged_anvil",
            "minecraft:dark_prismarine",
            "minecraft:dark_prismarine_slab",
            "minecraft:dark_prismarine_stairs",
            "minecraft:dead_brain_coral",
            "minecraft:dead_brain_coral_block",
            "minecraft:dead_brain_coral_fan",
            "minecraft:dead_brain_coral_wall_fan",
            "minecraft:dead_bubble_coral",
            "minecraft:dead_bubble_coral_block",
            "minecraft:dead_bubble_coral_fan",
            "minecraft:dead_bubble_coral_wall_fan",
            "minecraft:dead_fire_coral",
            "minecraft:dead_fire_coral_block",
            "minecraft:dead_fire_coral_fan",
            "minecraft:dead_fire_coral_wall_fan",
            "minecraft:dead_horn_coral",
            "minecraft:dead_horn_coral_block",
            "minecraft:dead_horn_coral_fan",
            "minecraft:dead_horn_coral_wall_fan",
            "minecraft:dead_tube_coral",
            "minecraft:dead_tube_coral_block",
            "minecraft:dead_tube_coral_fan",
            "minecraft:dead_tube_coral_wall_fan",
            "minecraft:deepslate_brick_slab",
            "minecraft:deepslate_brick_stairs",
            "minecraft:deepslate_brick_wall",
            "minecraft:deepslate_bricks",
            "minecraft:deepslate_tile_slab",
            "minecraft:deepslate_tile_stairs",
            "minecraft:deepslate_tile_wall",
            "minecraft:deepslate_tiles",
            "minecraft:detector_rail",
            "minecraft:diorite_slab",
            "minecraft:diorite_stairs",
            "minecraft:diorite_wall",
            "minecraft:dispenser",
            "minecraft:dropper",
            "minecraft:enchanting_table",
            "minecraft:end_stone_brick_slab",
            "minecraft:end_stone_brick_stairs",
            "minecraft:end_stone_brick_wall",
            "minecraft:end_stone_bricks",
            "minecraft:ender_chest",
            "minecraft:exposed_chiseled_copper",
            "minecraft:exposed_copper",
            "minecraft:exposed_copper_bars",
            "minecraft:exposed_copper_bulb",
            "minecraft:exposed_copper_chain",
            "minecraft:exposed_copper_chest",
            "minecraft:exposed_copper_door",
            "minecraft:exposed_copper_golem_statue",
            "minecraft:exposed_copper_grate",
            "minecraft:exposed_copper_lantern",
            "minecraft:exposed_copper_trapdoor",
            "minecraft:exposed_cut_copper",
            "minecraft:exposed_cut_copper_slab",
            "minecraft:exposed_cut_copper_stairs",
            "minecraft:exposed_lightning_rod",
            "minecraft:fire_coral_block",
            "minecraft:gilded_blackstone",
            "minecraft:granite_slab",
            "minecraft:granite_stairs",
            "minecraft:granite_wall",
            "minecraft:gray_concrete",
            "minecraft:gray_glazed_terracotta",
            "minecraft:gray_shulker_box",
            "minecraft:gray_terracotta",
            "minecraft:green_concrete",
            "minecraft:green_glazed_terracotta",
            "minecraft:green_shulker_box",
            "minecraft:green_terracotta",
            "minecraft:grindstone",
            "minecraft:heavy_core",
            "minecraft:heavy_weighted_pressure_plate",
            "minecraft:hopper",
            "minecraft:horn_coral_block",
            "minecraft:ice",
            "minecraft:infested_chiseled_stone_bricks",
            "minecraft:infested_cobblestone",
            "minecraft:infested_cracked_stone_bricks",
            "minecraft:infested_deepslate",
            "minecraft:infested_mossy_stone_bricks",
            "minecraft:infested_stone",
            "minecraft:infested_stone_bricks",
            "minecraft:iron_bars",
            "minecraft:iron_chain",
            "minecraft:iron_door",
            "minecraft:iron_trapdoor",
            "minecraft:lantern",
            "minecraft:large_amethyst_bud",
            "minecraft:lava_cauldron",
            "minecraft:light_blue_concrete",
            "minecraft:light_blue_glazed_terracotta",
            "minecraft:light_blue_shulker_box",
            "minecraft:light_blue_terracotta",
            "minecraft:light_gray_concrete",
            "minecraft:light_gray_glazed_terracotta",
            "minecraft:light_gray_shulker_box",
            "minecraft:light_gray_terracotta",
            "minecraft:light_weighted_pressure_plate",
            "minecraft:lightning_rod",
            "minecraft:lime_concrete",
            "minecraft:lime_glazed_terracotta",
            "minecraft:lime_shulker_box",
            "minecraft:lime_terracotta",
            "minecraft:lodestone",
            "minecraft:magenta_concrete",
            "minecraft:magenta_glazed_terracotta",
            "minecraft:magenta_shulker_box",
            "minecraft:magenta_terracotta",
            "minecraft:magma_block",
            "minecraft:medium_amethyst_bud",
            "minecraft:mossy_cobblestone_slab",
            "minecraft:mossy_cobblestone_stairs",
            "minecraft:mossy_cobblestone_wall",
            "minecraft:mossy_stone_brick_slab",
            "minecraft:mossy_stone_brick_stairs",
            "minecraft:mossy_stone_brick_wall",
            "minecraft:mossy_stone_bricks",
            "minecraft:mud_brick_slab",
            "minecraft:mud_brick_stairs",
            "minecraft:mud_brick_wall",
            "minecraft:mud_bricks",
            "minecraft:nether_brick_fence",
            "minecraft:nether_brick_slab",
            "minecraft:nether_brick_stairs",
            "minecraft:nether_brick_wall",
            "minecraft:nether_bricks",
            "minecraft:observer",
            "minecraft:orange_concrete",
            "minecraft:orange_glazed_terracotta",
            "minecraft:orange_shulker_box",
            "minecraft:orange_terracotta",
            "minecraft:oxidized_chiseled_copper",
            "minecraft:oxidized_copper",
            "minecraft:oxidized_copper_bars",
            "minecraft:oxidized_copper_bulb",
            "minecraft:oxidized_copper_chain",
            "minecraft:oxidized_copper_chest",
            "minecraft:oxidized_copper_door",
            "minecraft:oxidized_copper_golem_statue",
            "minecraft:oxidized_copper_grate",
            "minecraft:oxidized_copper_lantern",
            "minecraft:oxidized_copper_trapdoor",
            "minecraft:oxidized_cut_copper",
            "minecraft:oxidized_cut_copper_slab",
            "minecraft:oxidized_cut_copper_stairs",
            "minecraft:oxidized_lightning_rod",
            "minecraft:packed_ice",
            "minecraft:packed_mud",
            "minecraft:petrified_oak_slab",
            "minecraft:pink_concrete",
            "minecraft:pink_glazed_terracotta",
            "minecraft:pink_shulker_box",
            "minecraft:pink_terracotta",
            "minecraft:piston",
            "minecraft:piston_head",
            "minecraft:pointed_dripstone",
            "minecraft:polished_andesite_slab",
            "minecraft:polished_andesite_stairs",
            "minecraft:polished_basalt",
            "minecraft:polished_blackstone",
            "minecraft:polished_blackstone_brick_slab",
            "minecraft:polished_blackstone_brick_stairs",
            "minecraft:polished_blackstone_brick_wall",
            "minecraft:polished_blackstone_bricks",
            "minecraft:polished_blackstone_button",
            "minecraft:polished_blackstone_pressure_plate",
            "minecraft:polished_blackstone_slab",
            "minecraft:polished_blackstone_stairs",
            "minecraft:polished_blackstone_wall",
            "minecraft:polished_deepslate_slab",
            "minecraft:polished_deepslate_stairs",
            "minecraft:polished_deepslate_wall",
            "minecraft:polished_diorite_slab",
            "minecraft:polished_diorite_stairs",
            "minecraft:polished_granite_slab",
            "minecraft:polished_granite_stairs",
            "minecraft:polished_tuff",
            "minecraft:polished_tuff_slab",
            "minecraft:polished_tuff_stairs",
            "minecraft:polished_tuff_wall",
            "minecraft:powder_snow_cauldron",
            "minecraft:powered_rail",
            "minecraft:prismarine",
            "minecraft:prismarine_brick_slab",
            "minecraft:prismarine_brick_stairs",
            "minecraft:prismarine_bricks",
            "minecraft:prismarine_slab",
            "minecraft:prismarine_stairs",
            "minecraft:prismarine_wall",
            "minecraft:purple_concrete",
            "minecraft:purple_glazed_terracotta",
            "minecraft:purple_shulker_box",
            "minecraft:purple_terracotta",
            "minecraft:purpur_block",
            "minecraft:purpur_pillar",
            "minecraft:purpur_slab",
            "minecraft:purpur_stairs",
            "minecraft:quartz_block",
            "minecraft:quartz_bricks",
            "minecraft:quartz_pillar",
            "minecraft:quartz_slab",
            "minecraft:quartz_stairs",
            "minecraft:rail",
            "minecraft:red_concrete",
            "minecraft:red_glazed_terracotta",
            "minecraft:red_nether_brick_slab",
            "minecraft:red_nether_brick_stairs",
            "minecraft:red_nether_brick_wall",
            "minecraft:red_nether_bricks",
            "minecraft:red_sandstone",
            "minecraft:red_sandstone_slab",
            "minecraft:red_sandstone_stairs",
            "minecraft:red_sandstone_wall",
            "minecraft:red_shulker_box",
            "minecraft:red_terracotta",
            "minecraft:redstone_block",
            "minecraft:resin_brick_slab",
            "minecraft:resin_brick_stairs",
            "minecraft:resin_brick_wall",
            "minecraft:resin_bricks",
            "minecraft:respawn_anchor",
            "minecraft:sandstone",
            "minecraft:sandstone_slab",
            "minecraft:sandstone_stairs",
            "minecraft:sandstone_wall",
            "minecraft:shulker_box",
            "minecraft:small_amethyst_bud",
            "minecraft:smooth_quartz",
            "minecraft:smooth_quartz_slab",
            "minecraft:smooth_quartz_stairs",
            "minecraft:smooth_red_sandstone",
            "minecraft:smooth_red_sandstone_slab",
            "minecraft:smooth_red_sandstone_stairs",
            "minecraft:smooth_sandstone",
            "minecraft:smooth_sandstone_slab",
            "minecraft:smooth_sandstone_stairs",
            "minecraft:smooth_stone",
            "minecraft:smooth_stone_slab",
            "minecraft:soul_lantern",
            "minecraft:spawner",
            "minecraft:sticky_piston",
            "minecraft:stone_brick_slab",
            "minecraft:stone_brick_stairs",
            "minecraft:stone_brick_wall",
            "minecraft:stone_bricks",
            "minecraft:stone_button",
            "minecraft:stone_pressure_plate",
            "minecraft:stone_slab",
            "minecraft:stone_stairs",
            "minecraft:stonecutter",
            "minecraft:terracotta",
            "minecraft:tube_coral_block",
            "minecraft:tuff_brick_slab",
            "minecraft:tuff_brick_stairs",
            "minecraft:tuff_brick_wall",
            "minecraft:tuff_bricks",
            "minecraft:tuff_slab",
            "minecraft:tuff_stairs",
            "minecraft:tuff_wall",
            "minecraft:warped_nylium",
            "minecraft:water_cauldron",
            "minecraft:waxed_chiseled_copper",
            "minecraft:waxed_copper_bars",
            "minecraft:waxed_copper_block",
            "minecraft:waxed_copper_bulb",
            "minecraft:waxed_copper_chain",
            "minecraft:waxed_copper_chest",
            "minecraft:waxed_copper_door",
            "minecraft:waxed_copper_golem_statue",
            "minecraft:waxed_copper_grate",
            "minecraft:waxed_copper_lantern",
            "minecraft:waxed_copper_trapdoor",
            "minecraft:waxed_cut_copper",
            "minecraft:waxed_cut_copper_slab",
            "minecraft:waxed_cut_copper_stairs",
            "minecraft:waxed_exposed_chiseled_copper",
            "minecraft:waxed_exposed_copper",
            "minecraft:waxed_exposed_copper_bars",
            "minecraft:waxed_exposed_copper_bulb",
            "minecraft:waxed_exposed_copper_chain",
            "minecraft:waxed_exposed_copper_chest",
            "minecraft:waxed_exposed_copper_door",
            "minecraft:waxed_exposed_copper_golem_statue",
            "minecraft:waxed_exposed_copper_grate",
            "minecraft:waxed_exposed_copper_lantern",
            "minecraft:waxed_exposed_copper_trapdoor",
            "minecraft:waxed_exposed_cut_copper",
            "minecraft:waxed_exposed_cut_copper_slab",
            "minecraft:waxed_exposed_cut_copper_stairs",
            "minecraft:waxed_exposed_lightning_rod",
            "minecraft:waxed_lightning_rod",
            "minecraft:waxed_oxidized_chiseled_copper",
            "minecraft:waxed_oxidized_copper",
            "minecraft:waxed_oxidized_copper_bars",
            "minecraft:waxed_oxidized_copper_bulb",
            "minecraft:waxed_oxidized_copper_chain",
            "minecraft:waxed_oxidized_copper_chest",
            "minecraft:waxed_oxidized_copper_door",
            "minecraft:waxed_oxidized_copper_golem_statue",
            "minecraft:waxed_oxidized_copper_grate",
            "minecraft:waxed_oxidized_copper_lantern",
            "minecraft:waxed_oxidized_copper_trapdoor",
            "minecraft:waxed_oxidized_cut_copper",
            "minecraft:waxed_oxidized_cut_copper_slab",
            "minecraft:waxed_oxidized_cut_copper_stairs",
            "minecraft:waxed_oxidized_lightning_rod",
            "minecraft:waxed_weathered_chiseled_copper",
            "minecraft:waxed_weathered_copper",
            "minecraft:waxed_weathered_copper_bars",
            "minecraft:waxed_weathered_copper_bulb",
            "minecraft:waxed_weathered_copper_chain",
            "minecraft:waxed_weathered_copper_chest",
            "minecraft:waxed_weathered_copper_door",
            "minecraft:waxed_weathered_copper_golem_statue",
            "minecraft:waxed_weathered_copper_grate",
            "minecraft:waxed_weathered_copper_lantern",
            "minecraft:waxed_weathered_copper_trapdoor",
            "minecraft:waxed_weathered_cut_copper",
            "minecraft:waxed_weathered_cut_copper_slab",
            "minecraft:waxed_weathered_cut_copper_stairs",
            "minecraft:waxed_weathered_lightning_rod",
            "minecraft:weathered_chiseled_copper",
            "minecraft:weathered_copper",
            "minecraft:weathered_copper_bars",
            "minecraft:weathered_copper_bulb",
            "minecraft:weathered_copper_chain",
            "minecraft:weathered_copper_chest",
            "minecraft:weathered_copper_door",
            "minecraft:weathered_copper_golem_statue",
            "minecraft:weathered_copper_grate",
            "minecraft:weathered_copper_lantern",
            "minecraft:weathered_copper_trapdoor",
            "minecraft:weathered_cut_copper",
            "minecraft:weathered_cut_copper_slab",
            "minecraft:weathered_cut_copper_stairs",
            "minecraft:weathered_lightning_rod",
            "minecraft:white_concrete",
            "minecraft:white_glazed_terracotta",
            "minecraft:white_shulker_box",
            "minecraft:white_terracotta",
            "minecraft:yellow_concrete",
            "minecraft:yellow_glazed_terracotta",
            "minecraft:yellow_shulker_box",
            "minecraft:yellow_terracotta",
        ],
    ),
    (
        "minecraft:mineable/axe",
        &[
            "minecraft:oak_log",
            "minecraft:oak_wood",
            "minecraft:stripped_oak_log",
            "minecraft:stripped_oak_wood",
            "minecraft:spruce_log",
            "minecraft:spruce_wood",
            "minecraft:stripped_spruce_log",
            "minecraft:stripped_spruce_wood",
            "minecraft:birch_log",
            "minecraft:birch_wood",
            "minecraft:stripped_birch_log",
            "minecraft:stripped_birch_wood",
            "minecraft:jungle_log",
            "minecraft:jungle_wood",
            "minecraft:stripped_jungle_log",
            "minecraft:stripped_jungle_wood",
            "minecraft:acacia_log",
            "minecraft:acacia_wood",
            "minecraft:stripped_acacia_log",
            "minecraft:stripped_acacia_wood",
            "minecraft:dark_oak_log",
            "minecraft:dark_oak_wood",
            "minecraft:stripped_dark_oak_log",
            "minecraft:stripped_dark_oak_wood",
            "minecraft:pale_oak_log",
            "minecraft:pale_oak_wood",
            "minecraft:stripped_pale_oak_log",
            "minecraft:stripped_pale_oak_wood",
            "minecraft:mangrove_log",
            "minecraft:mangrove_wood",
            "minecraft:stripped_mangrove_log",
            "minecraft:stripped_mangrove_wood",
            "minecraft:cherry_log",
            "minecraft:cherry_wood",
            "minecraft:stripped_cherry_log",
            "minecraft:stripped_cherry_wood",
            "minecraft:crimson_stem",
            "minecraft:crimson_hyphae",
            "minecraft:stripped_crimson_stem",
            "minecraft:stripped_crimson_hyphae",
            "minecraft:warped_stem",
            "minecraft:warped_hyphae",
            "minecraft:stripped_warped_stem",
            "minecraft:stripped_warped_hyphae",
            "minecraft:bamboo_block",
            "minecraft:stripped_bamboo_block",
            "minecraft:oak_planks",
            "minecraft:spruce_planks",
            "minecraft:birch_planks",
            "minecraft:jungle_planks",
            "minecraft:acacia_planks",
            "minecraft:dark_oak_planks",
            "minecraft:pale_oak_planks",
            "minecraft:mangrove_planks",
            "minecraft:cherry_planks",
            "minecraft:bamboo_planks",
            "minecraft:crimson_planks",
            "minecraft:warped_planks",
            "minecraft:crafting_table",
            "minecraft:chest",
            "minecraft:trapped_chest",
            "minecraft:barrel",
            "minecraft:bookshelf",
            "minecraft:chiseled_bookshelf",
            "minecraft:ladder",
            "minecraft:composter",
            "minecraft:campfire",
            "minecraft:soul_campfire",
            "minecraft:bee_nest",
            "minecraft:beehive",
            "minecraft:melon",
            "minecraft:pumpkin",
            "minecraft:carved_pumpkin",
            "minecraft:jack_o_lantern",
            "minecraft:oak_door",
            "minecraft:spruce_door",
            "minecraft:birch_door",
            "minecraft:jungle_door",
            "minecraft:acacia_door",
            "minecraft:dark_oak_door",
            "minecraft:pale_oak_door",
            "minecraft:mangrove_door",
            "minecraft:cherry_door",
            "minecraft:bamboo_door",
            "minecraft:crimson_door",
            "minecraft:warped_door",
            "minecraft:oak_sign",
            "minecraft:oak_wall_sign",
            "minecraft:spruce_sign",
            "minecraft:spruce_wall_sign",
            "minecraft:birch_sign",
            "minecraft:birch_wall_sign",
            "minecraft:jungle_sign",
            "minecraft:jungle_wall_sign",
            "minecraft:acacia_sign",
            "minecraft:acacia_wall_sign",
            "minecraft:dark_oak_sign",
            "minecraft:dark_oak_wall_sign",
            "minecraft:pale_oak_sign",
            "minecraft:pale_oak_wall_sign",
            "minecraft:mangrove_sign",
            "minecraft:mangrove_wall_sign",
            "minecraft:cherry_sign",
            "minecraft:cherry_wall_sign",
            "minecraft:bamboo_sign",
            "minecraft:bamboo_wall_sign",
            "minecraft:crimson_sign",
            "minecraft:crimson_wall_sign",
            "minecraft:warped_sign",
            "minecraft:warped_wall_sign",
            "minecraft:acacia_button",
            "minecraft:acacia_fence",
            "minecraft:acacia_fence_gate",
            "minecraft:acacia_hanging_sign",
            "minecraft:acacia_pressure_plate",
            "minecraft:acacia_shelf",
            "minecraft:acacia_slab",
            "minecraft:acacia_stairs",
            "minecraft:acacia_trapdoor",
            "minecraft:acacia_wall_hanging_sign",
            "minecraft:bamboo",
            "minecraft:bamboo_button",
            "minecraft:bamboo_fence",
            "minecraft:bamboo_fence_gate",
            "minecraft:bamboo_hanging_sign",
            "minecraft:bamboo_mosaic",
            "minecraft:bamboo_mosaic_slab",
            "minecraft:bamboo_mosaic_stairs",
            "minecraft:bamboo_pressure_plate",
            "minecraft:bamboo_shelf",
            "minecraft:bamboo_slab",
            "minecraft:bamboo_stairs",
            "minecraft:bamboo_trapdoor",
            "minecraft:bamboo_wall_hanging_sign",
            "minecraft:big_dripleaf",
            "minecraft:big_dripleaf_stem",
            "minecraft:birch_button",
            "minecraft:birch_fence",
            "minecraft:birch_fence_gate",
            "minecraft:birch_hanging_sign",
            "minecraft:birch_pressure_plate",
            "minecraft:birch_shelf",
            "minecraft:birch_slab",
            "minecraft:birch_stairs",
            "minecraft:birch_trapdoor",
            "minecraft:birch_wall_hanging_sign",
            "minecraft:black_banner",
            "minecraft:black_wall_banner",
            "minecraft:blue_banner",
            "minecraft:blue_wall_banner",
            "minecraft:brown_banner",
            "minecraft:brown_mushroom_block",
            "minecraft:brown_wall_banner",
            "minecraft:cartography_table",
            "minecraft:cherry_button",
            "minecraft:cherry_fence",
            "minecraft:cherry_fence_gate",
            "minecraft:cherry_hanging_sign",
            "minecraft:cherry_pressure_plate",
            "minecraft:cherry_shelf",
            "minecraft:cherry_slab",
            "minecraft:cherry_stairs",
            "minecraft:cherry_trapdoor",
            "minecraft:cherry_wall_hanging_sign",
            "minecraft:chorus_flower",
            "minecraft:chorus_plant",
            "minecraft:cocoa",
            "minecraft:creaking_heart",
            "minecraft:crimson_button",
            "minecraft:crimson_fence",
            "minecraft:crimson_fence_gate",
            "minecraft:crimson_hanging_sign",
            "minecraft:crimson_pressure_plate",
            "minecraft:crimson_shelf",
            "minecraft:crimson_slab",
            "minecraft:crimson_stairs",
            "minecraft:crimson_trapdoor",
            "minecraft:crimson_wall_hanging_sign",
            "minecraft:cyan_banner",
            "minecraft:cyan_wall_banner",
            "minecraft:dark_oak_button",
            "minecraft:dark_oak_fence",
            "minecraft:dark_oak_fence_gate",
            "minecraft:dark_oak_hanging_sign",
            "minecraft:dark_oak_pressure_plate",
            "minecraft:dark_oak_shelf",
            "minecraft:dark_oak_slab",
            "minecraft:dark_oak_stairs",
            "minecraft:dark_oak_trapdoor",
            "minecraft:dark_oak_wall_hanging_sign",
            "minecraft:daylight_detector",
            "minecraft:fletching_table",
            "minecraft:glow_lichen",
            "minecraft:gray_banner",
            "minecraft:gray_wall_banner",
            "minecraft:green_banner",
            "minecraft:green_wall_banner",
            "minecraft:jukebox",
            "minecraft:jungle_button",
            "minecraft:jungle_fence",
            "minecraft:jungle_fence_gate",
            "minecraft:jungle_hanging_sign",
            "minecraft:jungle_pressure_plate",
            "minecraft:jungle_shelf",
            "minecraft:jungle_slab",
            "minecraft:jungle_stairs",
            "minecraft:jungle_trapdoor",
            "minecraft:jungle_wall_hanging_sign",
            "minecraft:lectern",
            "minecraft:light_blue_banner",
            "minecraft:light_blue_wall_banner",
            "minecraft:light_gray_banner",
            "minecraft:light_gray_wall_banner",
            "minecraft:lime_banner",
            "minecraft:lime_wall_banner",
            "minecraft:loom",
            "minecraft:magenta_banner",
            "minecraft:magenta_wall_banner",
            "minecraft:mangrove_button",
            "minecraft:mangrove_fence",
            "minecraft:mangrove_fence_gate",
            "minecraft:mangrove_hanging_sign",
            "minecraft:mangrove_pressure_plate",
            "minecraft:mangrove_roots",
            "minecraft:mangrove_shelf",
            "minecraft:mangrove_slab",
            "minecraft:mangrove_stairs",
            "minecraft:mangrove_trapdoor",
            "minecraft:mangrove_wall_hanging_sign",
            "minecraft:mushroom_stem",
            "minecraft:note_block",
            "minecraft:oak_button",
            "minecraft:oak_fence",
            "minecraft:oak_fence_gate",
            "minecraft:oak_hanging_sign",
            "minecraft:oak_pressure_plate",
            "minecraft:oak_shelf",
            "minecraft:oak_slab",
            "minecraft:oak_stairs",
            "minecraft:oak_trapdoor",
            "minecraft:oak_wall_hanging_sign",
            "minecraft:orange_banner",
            "minecraft:orange_wall_banner",
            "minecraft:pale_oak_button",
            "minecraft:pale_oak_fence",
            "minecraft:pale_oak_fence_gate",
            "minecraft:pale_oak_hanging_sign",
            "minecraft:pale_oak_pressure_plate",
            "minecraft:pale_oak_shelf",
            "minecraft:pale_oak_slab",
            "minecraft:pale_oak_stairs",
            "minecraft:pale_oak_trapdoor",
            "minecraft:pale_oak_wall_hanging_sign",
            "minecraft:pink_banner",
            "minecraft:pink_wall_banner",
            "minecraft:purple_banner",
            "minecraft:purple_wall_banner",
            "minecraft:red_banner",
            "minecraft:red_mushroom_block",
            "minecraft:red_wall_banner",
            "minecraft:smithing_table",
            "minecraft:spruce_button",
            "minecraft:spruce_fence",
            "minecraft:spruce_fence_gate",
            "minecraft:spruce_hanging_sign",
            "minecraft:spruce_pressure_plate",
            "minecraft:spruce_shelf",
            "minecraft:spruce_slab",
            "minecraft:spruce_stairs",
            "minecraft:spruce_trapdoor",
            "minecraft:spruce_wall_hanging_sign",
            "minecraft:vine",
            "minecraft:warped_button",
            "minecraft:warped_fence",
            "minecraft:warped_fence_gate",
            "minecraft:warped_hanging_sign",
            "minecraft:warped_pressure_plate",
            "minecraft:warped_shelf",
            "minecraft:warped_slab",
            "minecraft:warped_stairs",
            "minecraft:warped_trapdoor",
            "minecraft:warped_wall_hanging_sign",
            "minecraft:white_banner",
            "minecraft:white_wall_banner",
            "minecraft:yellow_banner",
            "minecraft:yellow_wall_banner",
        ],
    ),
    (
        "minecraft:mineable/shovel",
        &[
            "minecraft:black_concrete_powder",
            "minecraft:blue_concrete_powder",
            "minecraft:brown_concrete_powder",
            "minecraft:clay",
            "minecraft:coarse_dirt",
            "minecraft:cyan_concrete_powder",
            "minecraft:dirt",
            "minecraft:dirt_path",
            "minecraft:farmland",
            "minecraft:grass_block",
            "minecraft:gravel",
            "minecraft:gray_concrete_powder",
            "minecraft:green_concrete_powder",
            "minecraft:light_blue_concrete_powder",
            "minecraft:light_gray_concrete_powder",
            "minecraft:lime_concrete_powder",
            "minecraft:magenta_concrete_powder",
            "minecraft:mud",
            "minecraft:muddy_mangrove_roots",
            "minecraft:mycelium",
            "minecraft:orange_concrete_powder",
            "minecraft:pink_concrete_powder",
            "minecraft:podzol",
            "minecraft:purple_concrete_powder",
            "minecraft:red_concrete_powder",
            "minecraft:red_sand",
            "minecraft:rooted_dirt",
            "minecraft:sand",
            "minecraft:snow",
            "minecraft:snow_block",
            "minecraft:soul_sand",
            "minecraft:soul_soil",
            "minecraft:suspicious_gravel",
            "minecraft:suspicious_sand",
            "minecraft:white_concrete_powder",
            "minecraft:yellow_concrete_powder",
        ],
    ),
    (
        "minecraft:mineable/hoe",
        &[
            "minecraft:acacia_leaves",
            "minecraft:azalea_leaves",
            "minecraft:birch_leaves",
            "minecraft:calibrated_sculk_sensor",
            "minecraft:cherry_leaves",
            "minecraft:dark_oak_leaves",
            "minecraft:dried_kelp_block",
            "minecraft:flowering_azalea_leaves",
            "minecraft:hay_block",
            "minecraft:jungle_leaves",
            "minecraft:mangrove_leaves",
            "minecraft:moss_block",
            "minecraft:moss_carpet",
            "minecraft:nether_wart_block",
            "minecraft:oak_leaves",
            "minecraft:pale_moss_block",
            "minecraft:pale_moss_carpet",
            "minecraft:pale_oak_leaves",
            "minecraft:sculk",
            "minecraft:sculk_catalyst",
            "minecraft:sculk_sensor",
            "minecraft:sculk_shrieker",
            "minecraft:sculk_vein",
            "minecraft:shroomlight",
            "minecraft:sponge",
            "minecraft:spruce_leaves",
            "minecraft:target",
            "minecraft:warped_wart_block",
            "minecraft:wet_sponge",
        ],
    ),
    (
        "minecraft:incorrect_for_wooden_tool",
        INCORRECT_FOR_WOODEN_OR_GOLD_TOOL,
    ),
    (
        "minecraft:incorrect_for_gold_tool",
        INCORRECT_FOR_WOODEN_OR_GOLD_TOOL,
    ),
    (
        "minecraft:incorrect_for_stone_tool",
        INCORRECT_FOR_STONE_OR_COPPER_TOOL,
    ),
    (
        "minecraft:incorrect_for_copper_tool",
        INCORRECT_FOR_STONE_OR_COPPER_TOOL,
    ),
    (
        "minecraft:incorrect_for_iron_tool",
        &[
            "minecraft:obsidian",
            "minecraft:crying_obsidian",
            "minecraft:netherite_block",
            "minecraft:ancient_debris",
        ],
    ),
    ("minecraft:incorrect_for_diamond_tool", &[]),
    ("minecraft:incorrect_for_netherite_tool", &[]),
];

#[derive(Debug, Error)]
pub enum TagError {
    #[error("registries.json not found at {0}; run tools/extract-vanilla-data.sh --reports")]
    RegistriesMissing(PathBuf),
    #[error("registries.json at {path} is malformed: {source}")]
    RegistriesMalformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("filesystem error walking {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("tag file {path} malformed: {source}")]
    TagMalformed {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("tag file {path} uses replace=true, but layered tag replacement is not supported")]
    ReplaceUnsupported { path: PathBuf },
    #[error("required nested tag {referenced:?} referenced by {registry}#{tag} is missing")]
    MissingRequiredTag {
        registry: String,
        tag: String,
        referenced: String,
    },
    #[error("required registry entry {value:?} referenced by {registry}#{tag} is missing")]
    MissingRequiredEntry {
        registry: String,
        tag: String,
        value: String,
    },
    #[error("tag dependency cycle in {registry}: {cycle}")]
    DependencyCycle { registry: String, cycle: String },
}

/// Resolved tags, grouped by registry id, ready for the
/// `Update Tags` packet's `Map<registry, Map<tag, int[]>>` payload.
#[derive(Debug, Clone, Default)]
pub struct TagsData {
    pub registries: BTreeMap<Identifier, BTreeMap<Identifier, Vec<i32>>>,
    fuel_values: FuelValues,
    banner_pattern_entries: Vec<Identifier>,
}

impl TagsData {
    #[must_use]
    pub fn from_registries(
        registries: BTreeMap<Identifier, BTreeMap<Identifier, Vec<i32>>>,
    ) -> Self {
        Self {
            registries,
            fuel_values: FuelValues::default(),
            banner_pattern_entries: Vec::new(),
        }
    }

    /// Attach the immutable default-feature-set furnace-fuel snapshot.
    #[must_use]
    pub fn with_vanilla_fuel_values(mut self, items: &ItemRegistry) -> Self {
        self.fuel_values = FuelValues::vanilla_26_1_2(items, &self);
        self
    }

    #[must_use]
    pub fn fuel_values(&self) -> &FuelValues {
        &self.fuel_values
    }

    /// Borrow a tag's resolved members for reuse throughout a read-only scan.
    /// Bare registry/tag names default to `minecraft`; leading `#`s on tags
    /// are accepted just as they are by [`Self::contains_raw_id`].
    #[must_use]
    pub fn raw_ids(&self, registry: &str, tag: &str) -> Option<&[i32]> {
        // Tick profiles charge both Identifier::parse and repeated tag lookup.
        // Canonical names use Identifier's Borrow<str> ordering without parsing
        // or allocation; callers scanning blocks should resolve this slice once.
        // Validated Identifier keys cannot match malformed input. Read the public
        // registries directly, not a cache that would miss caller mutations.
        let tags = get_identifier(&self.registries, registry)?;
        get_identifier(tags, tag.trim_start_matches('#')).map(Vec::as_slice)
    }

    #[must_use]
    pub fn contains_raw_id(&self, registry: &str, tag: &str, raw_id: i32) -> bool {
        self.raw_ids(registry, tag)
            .is_some_and(|entries| entries.contains(&raw_id))
    }

    /// Number of `(registry, tag)` pairs the packet will emit.
    #[must_use]
    pub fn total_tags(&self) -> usize {
        self.registries.values().map(|m| m.len()).sum()
    }

    /// Total number of `(registry, tag, entry)` triples — handy for
    /// startup logging.
    #[must_use]
    pub fn total_entries(&self) -> usize {
        self.registries
            .values()
            .flat_map(|m| m.values())
            .map(Vec::len)
            .sum()
    }
}

fn get_identifier<'a, T>(entries: &'a BTreeMap<Identifier, T>, name: &str) -> Option<&'a T> {
    if name.contains(':') {
        entries.get(name)
    } else {
        // Only the unqualified API spelling needs a temporary canonical key.
        entries.get(format!("minecraft:{name}").as_str())
    }
}

#[cfg(test)]
mod lookup_tests {
    use super::*;

    #[test]
    fn borrowed_lookup_matches_parsed_lookup() {
        let tags = TagsData::from_registries(BTreeMap::from([
            (
                Identifier::parse("minecraft:block").unwrap(),
                BTreeMap::from([
                    (
                        Identifier::parse("minecraft:mineable/pickaxe").unwrap(),
                        vec![9, -1, 4, 9],
                    ),
                    (Identifier::parse("minecraft:empty").unwrap(), vec![]),
                    (Identifier::parse("other:tag").unwrap(), vec![3]),
                ]),
            ),
            (
                Identifier::parse("other:block").unwrap(),
                BTreeMap::from([(Identifier::parse("other:tag").unwrap(), vec![5])]),
            ),
        ]));
        for registry in [
            "minecraft:block",
            "block",
            "other:block",
            "missing",
            "",
            "#block",
            "Minecraft:block",
            ":block",
            "minecraft:",
            "minecraft:block:extra",
            "bad registry",
            "block ",
            "minecraft:blöck",
            "minecraft:block\0",
        ] {
            for tag in [
                "minecraft:mineable/pickaxe",
                "mineable/pickaxe",
                "#minecraft:mineable/pickaxe",
                "###mineable/pickaxe",
                "minecraft:empty",
                "#empty",
                "other:tag",
                "##other:tag",
                "missing",
                "",
                "#",
                "###",
                "Minecraft:mineable/pickaxe",
                ":mineable/pickaxe",
                "minecraft:",
                "minecraft:tag:extra",
                "bad tag",
                "tag ",
                "minecraft:täg",
                "minecraft:tag\0",
                "minecraft:#mineable/pickaxe",
            ] {
                let expected = Identifier::parse(registry)
                    .ok()
                    .and_then(|registry| tags.registries.get(&registry))
                    .and_then(|entries| {
                        Identifier::parse(tag.trim_start_matches('#'))
                            .ok()
                            .and_then(|tag| entries.get(&tag))
                    })
                    .map(Vec::as_slice);
                assert_eq!(
                    tags.raw_ids(registry, tag),
                    expected,
                    "{registry:?} {tag:?}"
                );
                for raw_id in [-1, 0, 3, 4, 5, 9, i32::MAX] {
                    assert_eq!(
                        tags.contains_raw_id(registry, tag, raw_id),
                        expected.is_some_and(|entries| entries.contains(&raw_id)),
                        "{registry:?} {tag:?} {raw_id}",
                    );
                }
            }
        }
        assert_eq!(tags.raw_ids("block", "##empty"), Some([].as_slice()));
    }

    #[test]
    fn lookup_observes_public_registry_mutations() {
        let registry = Identifier::parse("minecraft:block").unwrap();
        let tag = Identifier::parse("minecraft:dynamic").unwrap();
        let mut tags = TagsData::from_registries(BTreeMap::from([(
            registry.clone(),
            BTreeMap::from([(tag.clone(), vec![1])]),
        )]));
        assert!(tags.contains_raw_id("block", "#dynamic", 1));

        let entries = tags.registries.get_mut("minecraft:block").unwrap();
        *entries.get_mut("minecraft:dynamic").unwrap() = vec![9, -1, 9];
        assert!(!tags.contains_raw_id("block", "dynamic", 1));
        assert!(tags.contains_raw_id("block", "dynamic", -1));

        tags.registries
            .get_mut("minecraft:block")
            .unwrap()
            .remove("minecraft:dynamic");
        assert_eq!(tags.raw_ids("block", "dynamic"), None);
        tags.registries
            .get_mut("minecraft:block")
            .unwrap()
            .insert(tag.clone(), vec![]);
        assert_eq!(tags.raw_ids("block", "dynamic"), Some([].as_slice()));

        tags.registries
            .insert(registry.clone(), BTreeMap::from([(tag, vec![4])]));
        assert!(tags.contains_raw_id("block", "dynamic", 4));
        assert!(!tags.contains_raw_id("block", "dynamic", 9));
        tags.registries.remove(&registry);
        assert_eq!(tags.raw_ids("block", "dynamic"), None);
    }
}

/// Repo-owned item tags required by [`crate::recipes::solaris_required_recipes`].
#[must_use]
pub fn solaris_required_item_tags(items: &ItemRegistry) -> TagsData {
    let tags: BTreeMap<String, Vec<String>> = serde_json::from_str(REQUIRED_ITEM_TAGS)
        .expect("embedded required item tags JSON is valid");
    let item_registry = Identifier::parse("minecraft:item").expect("static registry id");
    let mut item_tags = BTreeMap::new();
    for (tag, entries) in tags {
        let ids: Vec<_> = entries
            .iter()
            .filter_map(|entry| {
                items
                    .id_of(
                        &Identifier::parse(entry.clone())
                            .expect("embedded required item id is valid"),
                    )
                    .and_then(|id| i32::try_from(id).ok())
            })
            .collect();
        item_tags.insert(
            Identifier::parse(tag).expect("embedded required item tag id is valid"),
            ids,
        );
    }

    TagsData::from_registries(BTreeMap::from([(item_registry, item_tags)]))
        .with_vanilla_fuel_values(items)
}

/// Embedded tag set used when no full vanilla sidecar is configured.
///
/// Configuration-only tags are bound even when empty. Supported mining tags
/// contain block raw ids so the client can calculate tool speed and tier.
#[must_use]
pub fn solaris_required_client_tags(
    items: &ItemRegistry,
    blocks: &[crate::blocks::BlockReport],
) -> TagsData {
    let mut tags = solaris_required_item_tags(items);
    add_required_client_fallback_tags(&mut tags);
    add_required_fluid_tags(&mut tags);
    add_required_block_tags(&mut tags, blocks);
    add_required_timeline_tags(&mut tags);
    enchantment_tags::add(&mut tags);
    banner_pattern_tags::add(&mut tags);
    crate::game_events::add_tags(&mut tags, blocks, items);
    tags
}

fn add_required_client_fallback_tags(tags: &mut TagsData) {
    for (registry, required_tags) in REQUIRED_CLIENT_FALLBACK_TAGS {
        let registry_id = Identifier::parse(*registry).expect("static registry id is valid");
        let registry_tags = tags.registries.entry(registry_id).or_default();
        for required_tag in *required_tags {
            let tag_id = Identifier::parse(*required_tag).expect("static tag id is valid");
            registry_tags.entry(tag_id).or_default();
        }
    }
}

fn add_required_timeline_tags(tags: &mut TagsData) {
    let entries = &crate::required_registry_entries()["timeline"];
    let timeline_tags = tags
        .registries
        .entry(Identifier::parse("minecraft:timeline").expect("static registry id"))
        .or_default();
    // 26.1.2 dimension types reference these tags, not individual timelines.
    // Empty bindings disable sky animation even when SetTime is correct.
    for (tag, members) in [
        ("minecraft:universal", &["minecraft:villager_schedule"][..]),
        ("minecraft:in_end", &["minecraft:villager_schedule"][..]),
        ("minecraft:in_nether", &["minecraft:villager_schedule"][..]),
        (
            "minecraft:in_overworld",
            &[
                "minecraft:villager_schedule",
                "minecraft:day",
                "minecraft:moon",
                "minecraft:early_game",
            ][..],
        ),
    ] {
        let ids = members
            .iter()
            .map(|name| {
                entries
                    .iter()
                    .position(|entry| entry.as_str() == *name)
                    .expect("required timeline is in the embedded registry") as i32
            })
            .collect();
        timeline_tags.insert(Identifier::parse(tag).expect("static timeline tag"), ids);
    }
}

fn add_required_fluid_tags(tags: &mut TagsData) {
    let fluid_registry = Identifier::parse("minecraft:fluid").expect("static fluid registry id");
    let fluid_tags = tags.registries.entry(fluid_registry).or_default();
    fluid_tags.insert(
        Identifier::parse("minecraft:water").expect("static water tag id"),
        vec![FLOWING_WATER_RAW_ID, WATER_RAW_ID],
    );
    fluid_tags.insert(
        Identifier::parse("minecraft:lava").expect("static lava tag id"),
        vec![FLOWING_LAVA_RAW_ID, LAVA_RAW_ID],
    );
}

fn add_required_block_tags(tags: &mut TagsData, blocks: &[crate::blocks::BlockReport]) {
    let mut blocks_in_raw_id_order: Vec<_> = blocks.iter().collect();
    blocks_in_raw_id_order.sort_by_key(|block| {
        block
            .states
            .iter()
            .map(|state| state.id)
            .min()
            .expect("embedded block has at least one state")
    });
    let raw_ids: BTreeMap<_, _> = blocks_in_raw_id_order
        .into_iter()
        .enumerate()
        .map(|(raw_id, block)| {
            (
                block.id.as_str(),
                i32::try_from(raw_id).expect("embedded block raw id fits i32"),
            )
        })
        .collect();

    let block_registry = Identifier::parse("minecraft:block").expect("static registry id");
    let block_tags = tags.registries.entry(block_registry).or_default();
    for (tag, block_names) in REQUIRED_BLOCK_TAGS {
        let tag_id = Identifier::parse(*tag).expect("static block tag id");
        let mut ids: Vec<_> = block_names
            .iter()
            .map(|block_name| {
                *raw_ids
                    .get(block_name)
                    .unwrap_or_else(|| panic!("embedded block tag entry {block_name} missing"))
            })
            .collect();
        ids.sort_unstable();
        ids.dedup();
        block_tags.insert(tag_id, ids);
    }
}

#[derive(Deserialize)]
struct RawTag {
    #[serde(default)]
    replace: bool,
    #[serde(default)]
    values: Vec<RawTagValue>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawTagValue {
    Plain(String),
    Object {
        id: String,
        #[serde(default = "default_required")]
        required: bool,
    },
}

fn default_required() -> bool {
    true
}

#[derive(Deserialize)]
struct ProtocolIdEntry {
    protocol_id: i32,
}

#[derive(Deserialize)]
struct RegistryReport {
    #[serde(default)]
    entries: BTreeMap<String, ProtocolIdEntry>,
}

/// Numeric id of `entry_id` inside `registry_id`, preferring the
/// position our `RegistryData` will use over the vanilla `protocol_id`
/// for data-driven registries.
fn entry_id_for(
    registry_id: &str,
    entry_id: &str,
    ours: &VanillaData,
    vanilla_ids: &BTreeMap<String, BTreeMap<String, i32>>,
) -> Option<i32> {
    let path = registry_id
        .strip_prefix("minecraft:")
        .unwrap_or(registry_id);
    if let Some(reg) = ours.registry(path)
        && let Some(pos) = reg.entries.iter().position(|e| e.as_str() == entry_id)
    {
        return Some(pos as i32);
    }
    vanilla_ids.get(registry_id)?.get(entry_id).copied()
}

fn load_vanilla_id_index(
    vanilla_dir: &Path,
) -> Result<BTreeMap<String, BTreeMap<String, i32>>, TagError> {
    let path = vanilla_dir.join("reports").join("registries.json");
    if !path.is_file() {
        return Err(TagError::RegistriesMissing(path));
    }
    let report: BTreeMap<String, RegistryReport> = read_json_file(
        &path,
        &|path, source| TagError::Io { path, source },
        &|path, source| TagError::RegistriesMalformed { path, source },
    )?;
    Ok(report
        .into_iter()
        .map(|(reg, body)| {
            let entries = body
                .entries
                .into_iter()
                .map(|(id, v)| (id, v.protocol_id))
                .collect();
            (reg, entries)
        })
        .collect())
}

fn collect_tag_files(
    root: &Path,
    registry_id: &str,
    raw: &mut BTreeMap<(String, String), (PathBuf, RawTag)>,
) -> Result<(), TagError> {
    visit_json_files(
        root,
        &mut |path| {
            let rel = path
                .strip_prefix(root)
                .expect("walk yields paths under root")
                .with_extension("");
            let mut joined = String::new();
            for component in rel.components() {
                if !joined.is_empty() {
                    joined.push('/');
                }
                joined.push_str(component.as_os_str().to_string_lossy().as_ref());
            }
            let parsed: RawTag = read_json_file(
                &path,
                &|path, source| TagError::Io { path, source },
                &|path, source| TagError::TagMalformed { path, source },
            )?;
            if parsed.replace {
                return Err(TagError::ReplaceUnsupported { path });
            }
            raw.insert((registry_id.to_string(), joined), (path, parsed));
            Ok(())
        },
        &|path, source| TagError::Io { path, source },
    )
}

fn resolve(
    registry_id: &str,
    tag_path: &str,
    raw: &BTreeMap<(String, String), (PathBuf, RawTag)>,
    ours: &VanillaData,
    vanilla_ids: &BTreeMap<String, BTreeMap<String, i32>>,
    stack: &mut Vec<String>,
    cache: &mut BTreeMap<String, BTreeSet<i32>>,
) -> Result<BTreeSet<i32>, TagError> {
    let marker = format!("{registry_id}#{tag_path}");
    if let Some(resolved) = cache.get(&marker) {
        return Ok(resolved.clone());
    }
    if let Some(cycle_start) = stack.iter().position(|entry| entry == &marker) {
        let mut cycle = stack[cycle_start..].to_vec();
        cycle.push(marker.clone());
        return Err(TagError::DependencyCycle {
            registry: registry_id.to_owned(),
            cycle: cycle.join(" -> "),
        });
    }
    let Some((_, raw_tag)) = raw.get(&(registry_id.to_string(), tag_path.to_string())) else {
        return Err(TagError::MissingRequiredTag {
            registry: registry_id.to_owned(),
            tag: tag_path.to_owned(),
            referenced: tag_path.to_owned(),
        });
    };

    stack.push(marker.clone());
    let mut seen = BTreeSet::new();
    for value in &raw_tag.values {
        let (raw_value, required) = match value {
            RawTagValue::Plain(value) => (value.as_str(), true),
            RawTagValue::Object { id, required } => (id.as_str(), *required),
        };
        if let Some(tag_ref) = raw_value.strip_prefix('#') {
            let inner_path = tag_ref
                .strip_prefix("minecraft:")
                .unwrap_or_else(|| tag_ref.split_once(':').map_or(tag_ref, |(_, path)| path));
            let inner_key = (registry_id.to_owned(), inner_path.to_owned());
            if !raw.contains_key(&inner_key) {
                if required {
                    stack.pop();
                    return Err(TagError::MissingRequiredTag {
                        registry: registry_id.to_owned(),
                        tag: tag_path.to_owned(),
                        referenced: tag_ref.to_owned(),
                    });
                }
                warn!(
                    registry = %registry_id,
                    tag = %tag_path,
                    referenced = %tag_ref,
                    "optional nested tag is missing; skipping"
                );
                continue;
            }
            seen.extend(resolve(
                registry_id,
                inner_path,
                raw,
                ours,
                vanilla_ids,
                stack,
                cache,
            )?);
        } else if let Some(idx) = entry_id_for(registry_id, raw_value, ours, vanilla_ids) {
            seen.insert(idx);
        } else if required {
            stack.pop();
            return Err(TagError::MissingRequiredEntry {
                registry: registry_id.to_owned(),
                tag: tag_path.to_owned(),
                value: raw_value.to_owned(),
            });
        } else {
            warn!(
                registry = %registry_id,
                tag = %tag_path,
                value = %raw_value,
                "optional tag registry entry is missing; skipping"
            );
        }
    }
    stack.pop();
    cache.insert(marker, seen.clone());
    Ok(seen)
}

/// Load the full tag set for `vanilla_dir`. Returns an empty `TagsData`
/// when no tag-root directories exist (the sidecar was generated with
/// `--reports` only but no `tags/` directory).
pub fn load(vanilla_dir: &Path, ours: &VanillaData) -> Result<TagsData, TagError> {
    let vanilla_ids = load_vanilla_id_index(vanilla_dir)?;
    let tags_root = vanilla_dir.join("data").join("minecraft").join("tags");

    let mut raw: BTreeMap<(String, String), (PathBuf, RawTag)> = BTreeMap::new();
    for (subpath, registry_id) in TAG_ROOTS {
        let root = tags_root.join(subpath);
        if !root.is_dir() {
            continue;
        }
        collect_tag_files(&root, registry_id, &mut raw)?;
    }

    let mut registries: BTreeMap<Identifier, BTreeMap<Identifier, Vec<i32>>> = BTreeMap::new();
    let mut cache = BTreeMap::new();
    for (registry_id, tag_path) in raw.keys() {
        let seen = resolve(
            registry_id,
            tag_path,
            &raw,
            ours,
            &vanilla_ids,
            &mut Vec::new(),
            &mut cache,
        )?;

        let registry_ident =
            Identifier::parse(registry_id.clone()).expect("TAG_ROOTS provides valid identifiers");
        let tag_ident = Identifier::parse(format!("minecraft:{tag_path}"))
            .expect("tag path is a valid identifier");
        registries
            .entry(registry_ident)
            .or_default()
            .insert(tag_ident, seen.into_iter().collect());
    }

    let mut data = TagsData::from_registries(registries);
    if let Some(registry) = ours.registry("banner_pattern") {
        data = data.with_banner_pattern_registry(registry);
    }
    debug!(
        registries = data.registries.len(),
        tags = data.total_tags(),
        entries = data.total_entries(),
        "loaded vanilla tag set"
    );
    Ok(data)
}

#[cfg(test)]
#[path = "tags_tests.rs"]
mod tests;
