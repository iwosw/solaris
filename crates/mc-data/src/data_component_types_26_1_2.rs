//! Built-in component IDs are DataComponents registration order, not the
//! alphabetical IDs of dynamic Configuration registries. All 110 entries are
//! verified against javap DataComponents and the pinned 26.1.2 registry report.
use crate::Identifier;

pub const COUNT: usize = 110;
/// Components with a lossless native NBT and 26.1.2 stream codec. Command
/// admission and wire emission share this list so unsupported patches fail.
pub const ITEM_COMPONENT_CODECS: &[(&str, i32)] = &[
    ("minecraft:custom_data", 0),
    ("minecraft:max_damage", 2),
    ("minecraft:damage", 3),
    ("minecraft:custom_name", 6),
    ("minecraft:item_name", 9),
    ("minecraft:item_model", 10),
    ("minecraft:lore", 11),
    ("minecraft:rarity", 12),
    ("minecraft:enchantments", 13),
    ("minecraft:tooltip_display", 18),
    ("minecraft:repair_cost", 19),
    ("minecraft:enchantment_glint_override", 21),
    ("minecraft:use_remainder", 25),
    ("minecraft:death_protection", 36),
    ("minecraft:stored_enchantments", 42),
    ("minecraft:dyed_color", 44),
    ("minecraft:charged_projectiles", 49),
    ("minecraft:bundle_contents", 50),
    ("minecraft:potion_contents", 51),
    ("minecraft:suspicious_stew_effects", 53),
    ("minecraft:writable_book_content", 54),
    ("minecraft:written_book_content", 55),
    ("minecraft:trim", 56),
    ("minecraft:ominous_bottle_amplifier", 63),
    ("minecraft:fireworks", 69),
    ("minecraft:profile", 70),
    ("minecraft:note_block_sound", 71),
    ("minecraft:banner_patterns", 72),
    ("minecraft:pot_decorations", 74),
    ("minecraft:container", 75),
    ("minecraft:block_state", 76),
    ("minecraft:bees", 77),
    ("minecraft:lock", 78),
    ("minecraft:container_loot", 79),
];
const BY_NAME: &[(&str, u32)] = &[
    ("minecraft:additional_trade_cost", 41),
    ("minecraft:attack_range", 30),
    ("minecraft:attribute_modifiers", 16),
    ("minecraft:axolotl/variant", 104),
    ("minecraft:banner_patterns", 72),
    ("minecraft:base_color", 73),
    ("minecraft:bees", 77),
    ("minecraft:block_entity_data", 60),
    ("minecraft:block_state", 76),
    ("minecraft:blocks_attacks", 37),
    ("minecraft:break_sound", 80),
    ("minecraft:bucket_entity_data", 59),
    ("minecraft:bundle_contents", 50),
    ("minecraft:can_break", 15),
    ("minecraft:can_place_on", 14),
    ("minecraft:cat/collar", 107),
    ("minecraft:cat/sound_variant", 106),
    ("minecraft:cat/variant", 105),
    ("minecraft:charged_projectiles", 49),
    ("minecraft:chicken/sound_variant", 98),
    ("minecraft:chicken/variant", 97),
    ("minecraft:consumable", 24),
    ("minecraft:container", 75),
    ("minecraft:container_loot", 79),
    ("minecraft:cow/sound_variant", 96),
    ("minecraft:cow/variant", 95),
    ("minecraft:creative_slot_lock", 20),
    ("minecraft:custom_data", 0),
    ("minecraft:custom_model_data", 17),
    ("minecraft:custom_name", 6),
    ("minecraft:damage", 3),
    ("minecraft:damage_resistant", 27),
    ("minecraft:damage_type", 8),
    ("minecraft:death_protection", 36),
    ("minecraft:debug_stick_state", 57),
    ("minecraft:dye", 43),
    ("minecraft:dyed_color", 44),
    ("minecraft:enchantable", 31),
    ("minecraft:enchantment_glint_override", 21),
    ("minecraft:enchantments", 13),
    ("minecraft:entity_data", 58),
    ("minecraft:equippable", 32),
    ("minecraft:firework_explosion", 68),
    ("minecraft:fireworks", 69),
    ("minecraft:food", 23),
    ("minecraft:fox/variant", 85),
    ("minecraft:frog/variant", 100),
    ("minecraft:glider", 34),
    ("minecraft:horse/variant", 101),
    ("minecraft:instrument", 61),
    ("minecraft:intangible_projectile", 22),
    ("minecraft:item_model", 10),
    ("minecraft:item_name", 9),
    ("minecraft:jukebox_playable", 64),
    ("minecraft:kinetic_weapon", 39),
    ("minecraft:llama/variant", 103),
    ("minecraft:lock", 78),
    ("minecraft:lodestone_tracker", 67),
    ("minecraft:lore", 11),
    ("minecraft:map_color", 45),
    ("minecraft:map_decorations", 47),
    ("minecraft:map_id", 46),
    ("minecraft:map_post_processing", 48),
    ("minecraft:max_damage", 2),
    ("minecraft:max_stack_size", 1),
    ("minecraft:minimum_attack_charge", 7),
    ("minecraft:mooshroom/variant", 91),
    ("minecraft:note_block_sound", 71),
    ("minecraft:ominous_bottle_amplifier", 63),
    ("minecraft:painting/variant", 102),
    ("minecraft:parrot/variant", 87),
    ("minecraft:piercing_weapon", 38),
    ("minecraft:pig/sound_variant", 94),
    ("minecraft:pig/variant", 93),
    ("minecraft:pot_decorations", 74),
    ("minecraft:potion_contents", 51),
    ("minecraft:potion_duration_scale", 52),
    ("minecraft:profile", 70),
    ("minecraft:provides_banner_patterns", 65),
    ("minecraft:provides_trim_material", 62),
    ("minecraft:rabbit/variant", 92),
    ("minecraft:rarity", 12),
    ("minecraft:recipes", 66),
    ("minecraft:repair_cost", 19),
    ("minecraft:repairable", 33),
    ("minecraft:salmon/size", 86),
    ("minecraft:sheep/color", 108),
    ("minecraft:shulker/color", 109),
    ("minecraft:stored_enchantments", 42),
    ("minecraft:suspicious_stew_effects", 53),
    ("minecraft:swing_animation", 40),
    ("minecraft:tool", 28),
    ("minecraft:tooltip_display", 18),
    ("minecraft:tooltip_style", 35),
    ("minecraft:trim", 56),
    ("minecraft:tropical_fish/base_color", 89),
    ("minecraft:tropical_fish/pattern", 88),
    ("minecraft:tropical_fish/pattern_color", 90),
    ("minecraft:unbreakable", 4),
    ("minecraft:use_cooldown", 26),
    ("minecraft:use_effects", 5),
    ("minecraft:use_remainder", 25),
    ("minecraft:villager/variant", 81),
    ("minecraft:weapon", 29),
    ("minecraft:wolf/collar", 84),
    ("minecraft:wolf/sound_variant", 83),
    ("minecraft:wolf/variant", 82),
    ("minecraft:writable_book_content", 54),
    ("minecraft:written_book_content", 55),
    ("minecraft:zombie_nautilus/variant", 99),
];
const BY_ID: [&str; COUNT] = {
    let mut names = [""; COUNT];
    let mut index = 0;
    while index < BY_NAME.len() {
        names[BY_NAME[index].1 as usize] = BY_NAME[index].0;
        index += 1;
    }
    names
};
#[must_use]
pub fn id(name: &Identifier) -> Option<u32> {
    BY_NAME
        .binary_search_by(|(entry, _)| entry.cmp(&name.as_str()))
        .ok()
        .map(|index| BY_NAME[index].1)
}
#[must_use]
pub fn name(id: u32) -> Option<&'static str> {
    BY_ID.get(id as usize).copied()
}
#[cfg(test)]
#[path = "data_component_types_26_1_2_tests.rs"]
mod tests;
