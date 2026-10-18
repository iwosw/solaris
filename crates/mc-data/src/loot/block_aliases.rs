//! 26.1.2 Blocks.wallVariant overrides its loot table with the standing block.
use crate::Identifier;
use std::collections::BTreeMap;
use std::sync::LazyLock;

const PAIRS: &[(&str, &str)] = &[
    ("minecraft:wall_torch", "minecraft:torch"),
    ("minecraft:oak_wall_sign", "minecraft:oak_sign"),
    ("minecraft:spruce_wall_sign", "minecraft:spruce_sign"),
    ("minecraft:birch_wall_sign", "minecraft:birch_sign"),
    ("minecraft:acacia_wall_sign", "minecraft:acacia_sign"),
    ("minecraft:cherry_wall_sign", "minecraft:cherry_sign"),
    ("minecraft:jungle_wall_sign", "minecraft:jungle_sign"),
    ("minecraft:dark_oak_wall_sign", "minecraft:dark_oak_sign"),
    ("minecraft:pale_oak_wall_sign", "minecraft:pale_oak_sign"),
    ("minecraft:mangrove_wall_sign", "minecraft:mangrove_sign"),
    ("minecraft:bamboo_wall_sign", "minecraft:bamboo_sign"),
    (
        "minecraft:oak_wall_hanging_sign",
        "minecraft:oak_hanging_sign",
    ),
    (
        "minecraft:spruce_wall_hanging_sign",
        "minecraft:spruce_hanging_sign",
    ),
    (
        "minecraft:birch_wall_hanging_sign",
        "minecraft:birch_hanging_sign",
    ),
    (
        "minecraft:acacia_wall_hanging_sign",
        "minecraft:acacia_hanging_sign",
    ),
    (
        "minecraft:cherry_wall_hanging_sign",
        "minecraft:cherry_hanging_sign",
    ),
    (
        "minecraft:jungle_wall_hanging_sign",
        "minecraft:jungle_hanging_sign",
    ),
    (
        "minecraft:dark_oak_wall_hanging_sign",
        "minecraft:dark_oak_hanging_sign",
    ),
    (
        "minecraft:pale_oak_wall_hanging_sign",
        "minecraft:pale_oak_hanging_sign",
    ),
    (
        "minecraft:mangrove_wall_hanging_sign",
        "minecraft:mangrove_hanging_sign",
    ),
    (
        "minecraft:crimson_wall_hanging_sign",
        "minecraft:crimson_hanging_sign",
    ),
    (
        "minecraft:warped_wall_hanging_sign",
        "minecraft:warped_hanging_sign",
    ),
    (
        "minecraft:bamboo_wall_hanging_sign",
        "minecraft:bamboo_hanging_sign",
    ),
    ("minecraft:redstone_wall_torch", "minecraft:redstone_torch"),
    ("minecraft:soul_wall_torch", "minecraft:soul_torch"),
    ("minecraft:copper_wall_torch", "minecraft:copper_torch"),
    ("minecraft:skeleton_wall_skull", "minecraft:skeleton_skull"),
    (
        "minecraft:wither_skeleton_wall_skull",
        "minecraft:wither_skeleton_skull",
    ),
    ("minecraft:zombie_wall_head", "minecraft:zombie_head"),
    ("minecraft:player_wall_head", "minecraft:player_head"),
    ("minecraft:creeper_wall_head", "minecraft:creeper_head"),
    ("minecraft:dragon_wall_head", "minecraft:dragon_head"),
    ("minecraft:piglin_wall_head", "minecraft:piglin_head"),
    ("minecraft:white_wall_banner", "minecraft:white_banner"),
    ("minecraft:orange_wall_banner", "minecraft:orange_banner"),
    ("minecraft:magenta_wall_banner", "minecraft:magenta_banner"),
    (
        "minecraft:light_blue_wall_banner",
        "minecraft:light_blue_banner",
    ),
    ("minecraft:yellow_wall_banner", "minecraft:yellow_banner"),
    ("minecraft:lime_wall_banner", "minecraft:lime_banner"),
    ("minecraft:pink_wall_banner", "minecraft:pink_banner"),
    ("minecraft:gray_wall_banner", "minecraft:gray_banner"),
    (
        "minecraft:light_gray_wall_banner",
        "minecraft:light_gray_banner",
    ),
    ("minecraft:cyan_wall_banner", "minecraft:cyan_banner"),
    ("minecraft:purple_wall_banner", "minecraft:purple_banner"),
    ("minecraft:blue_wall_banner", "minecraft:blue_banner"),
    ("minecraft:brown_wall_banner", "minecraft:brown_banner"),
    ("minecraft:green_wall_banner", "minecraft:green_banner"),
    ("minecraft:red_wall_banner", "minecraft:red_banner"),
    ("minecraft:black_wall_banner", "minecraft:black_banner"),
    (
        "minecraft:dead_tube_coral_wall_fan",
        "minecraft:dead_tube_coral_fan",
    ),
    (
        "minecraft:dead_brain_coral_wall_fan",
        "minecraft:dead_brain_coral_fan",
    ),
    (
        "minecraft:dead_bubble_coral_wall_fan",
        "minecraft:dead_bubble_coral_fan",
    ),
    (
        "minecraft:dead_fire_coral_wall_fan",
        "minecraft:dead_fire_coral_fan",
    ),
    (
        "minecraft:dead_horn_coral_wall_fan",
        "minecraft:dead_horn_coral_fan",
    ),
    ("minecraft:tube_coral_wall_fan", "minecraft:tube_coral_fan"),
    (
        "minecraft:brain_coral_wall_fan",
        "minecraft:brain_coral_fan",
    ),
    (
        "minecraft:bubble_coral_wall_fan",
        "minecraft:bubble_coral_fan",
    ),
    ("minecraft:fire_coral_wall_fan", "minecraft:fire_coral_fan"),
    ("minecraft:horn_coral_wall_fan", "minecraft:horn_coral_fan"),
    ("minecraft:crimson_wall_sign", "minecraft:crimson_sign"),
    ("minecraft:warped_wall_sign", "minecraft:warped_sign"),
];

pub(super) fn canonical(block: &Identifier) -> &Identifier {
    if !block.path().contains("wall_") {
        return block;
    }
    static ALIASES: LazyLock<BTreeMap<Identifier, Identifier>> = LazyLock::new(|| {
        PAIRS
            .iter()
            .map(|(alias, source)| {
                (
                    Identifier::parse(*alias).unwrap(),
                    Identifier::parse(*source).unwrap(),
                )
            })
            .collect()
    });
    ALIASES.get(block).unwrap_or(block)
}

#[cfg(test)]
#[path = "block_aliases_tests.rs"]
mod tests;
