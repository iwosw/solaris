//! 26.1.2 vibration frequencies measured from VibrationSystem's static initializer.
#[must_use]
pub fn frequency(name: &str) -> u8 {
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    match name {
        "step" | "swim" | "flap" => 1,
        "projectile_land" | "hit_ground" | "splash" => 2,
        "item_interact_finish" | "projectile_shoot" | "instrument_play" => 3,
        "entity_action" | "elytra_glide" | "unequip" => 4,
        "equip" | "entity_dismount" => 5,
        "entity_interact" | "shear" | "entity_mount" => 6,
        "entity_damage" => 7,
        "drink" | "eat" => 8,
        "container_close" | "block_close" | "block_deactivate" | "block_detach" => 9,
        "container_open" | "block_open" | "block_activate" | "block_attach" | "prime_fuse"
        | "note_block_play" => 10,
        "block_change" => 11,
        "block_destroy" | "fluid_pickup" => 12,
        "block_place" | "fluid_place" => 13,
        "entity_place" | "lightning_strike" | "teleport" => 14,
        "entity_die" | "explode" => 15,
        _ => name
            .strip_prefix("resonate_")
            .and_then(|n| n.parse::<u8>().ok())
            .filter(|n| (1..=15).contains(n))
            .unwrap_or(0),
    }
}

pub(crate) fn add_tags(
    tags: &mut crate::tags::TagsData,
    blocks: &[crate::blocks::BlockReport],
    items: &crate::items::ItemRegistry,
) {
    use crate::Identifier;
    use std::collections::BTreeMap;
    let data: serde_json::Value =
        serde_json::from_str(include_str!("../data/sculk_tags.json")).expect("embedded sculk tags");
    let mut ordered: Vec<_> = blocks.iter().collect();
    ordered.sort_by_key(|b| b.states.iter().map(|s| s.id).min().expect("block state"));
    let raw: BTreeMap<_, _> = ordered
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id.as_str(), i as i32))
        .collect();
    for root in ["block", "item", "game_event"] {
        let registry = Identifier::parse(format!("minecraft:{root}")).expect("registry id");
        let target = tags.registries.entry(registry).or_default();
        for (name, values) in data[root].as_object().expect("tag group") {
            let mut ids: Vec<i32> = values
                .as_array()
                .expect("tag values")
                .iter()
                .map(|v| match root {
                    "block" => raw[v.as_str().expect("block id")],
                    "item" => items
                        .id_of(&Identifier::parse(v.as_str().expect("item id")).expect("id"))
                        .expect("item tag member") as i32,
                    _ => v.as_i64().expect("event protocol id") as i32,
                })
                .collect();
            ids.sort_unstable();
            ids.dedup();
            target.insert(
                Identifier::parse(format!("minecraft:{name}")).expect("tag id"),
                ids,
            );
        }
    }
}
