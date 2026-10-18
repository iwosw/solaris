//! Canonical 26.1.2 enchantment target tags for the embedded bootstrap registry.
use crate::{Identifier, tags::TagsData};

pub(super) fn add(tags: &mut TagsData) {
    let entities = crate::entity_types::solaris_required_entity_types();
    let registry = tags
        .registries
        .entry(Identifier::parse("minecraft:entity_type").expect("static registry id"))
        .or_default();
    // Expanded from data/minecraft/tags/entity_type/{sensitive_to_*,undead,
    // skeletons,zombies,arthropod,aquatic}.json; ids come from the same report
    // used by CoreRegistry and the client's built-in entity registry.
    for (tag, members) in [
        (
            "sensitive_to_smite",
            &[
                "skeleton",
                "stray",
                "wither_skeleton",
                "skeleton_horse",
                "bogged",
                "parched",
                "zombie_horse",
                "camel_husk",
                "zombie",
                "zombie_villager",
                "zombified_piglin",
                "zoglin",
                "drowned",
                "husk",
                "zombie_nautilus",
                "wither",
                "phantom",
            ][..],
        ),
        (
            "sensitive_to_bane_of_arthropods",
            &["bee", "endermite", "silverfish", "spider", "cave_spider"][..],
        ),
        (
            "sensitive_to_impaling",
            &[
                "turtle",
                "axolotl",
                "guardian",
                "elder_guardian",
                "cod",
                "pufferfish",
                "salmon",
                "tropical_fish",
                "dolphin",
                "squid",
                "glow_squid",
                "tadpole",
                "nautilus",
                "zombie_nautilus",
            ][..],
        ),
    ] {
        let ids = members
            .iter()
            .map(|name| {
                let id = Identifier::parse(format!("minecraft:{name}")).expect("static entity id");
                entities
                    .id_of(&id)
                    .unwrap_or_else(|| panic!("required enchantment target {id} missing"))
                    as i32
            })
            .collect();
        registry.insert(
            Identifier::parse(format!("minecraft:{tag}")).expect("static tag id"),
            ids,
        );
    }
}
