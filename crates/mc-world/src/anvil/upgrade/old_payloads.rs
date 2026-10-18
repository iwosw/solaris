//! 1.16.5–1.18 payload fixes; class bytecode receipts live in
//! `.analysis/codex-logs/old-dataversion/`.
use super::{NodeKind, UpgradeReport, nbt};
#[cfg(test)]
use mc_nbt::ListTag;
use mc_nbt::Tag;

pub(super) fn fix_node(tag: &mut Tag, kind: NodeKind, version: i32, report: &mut UpgradeReport) {
    if kind == NodeKind::BlockState
        && version == 2679
        && nbt::get(tag, "Name").and_then(nbt::string) == Some("minecraft:cauldron")
    {
        let empty = nbt::get(tag, "Properties")
            .and_then(|p| nbt::get(p, "level"))
            .and_then(nbt::string)
            .unwrap_or("0")
            == "0";
        if empty {
            nbt::remove(tag, "Properties");
        } else {
            nbt::set(tag, "Name", Tag::String("minecraft:water_cauldron".into()));
        }
        report.applied(version, "CauldronRenameFix");
    }
    if kind == NodeKind::Entity
        && version == 2702
        && matches!(
            nbt::get(tag, "id").and_then(nbt::string),
            Some("minecraft:arrow" | "minecraft:spectral_arrow" | "minecraft:trident")
        )
        && nbt::get(tag, "pickup").is_none()
    {
        let player = nbt::get(tag, "player")
            .and_then(nbt::number)
            .is_none_or(|value| value as i8 != 0);
        nbt::set(tag, "pickup", Tag::Byte(i8::from(player)));
        nbt::remove(tag, "player");
        report.applied(version, "AbstractArrowPickupFix");
    }
    if version == 2831
        && matches!(
            (kind, nbt::get(tag, "id").and_then(nbt::string)),
            (NodeKind::BlockEntity, Some("minecraft:mob_spawner"))
                | (NodeKind::Entity, Some("minecraft:spawner_minecart"))
        )
    {
        spawner_data(tag);
        report.applied(version, "SpawnerDataFix");
    }
    // BlockRenameFix rewrites BLOCK_NAME and BLOCK_STATE, not arbitrary strings.
    let block = kind == NodeKind::BlockState;
    let item = kind == NodeKind::Item;
    if block || item {
        rename_field(
            tag,
            if block { "Name" } else { "id" },
            version,
            item,
            report,
        );
    }
    if kind == NodeKind::Chunk {
        let chunk = if nbt::get(tag, "Level").is_some() {
            nbt::get_mut(tag, "Level").expect("checked legacy wrapper")
        } else {
            &mut *tag
        };
        for key in ["TileTicks", "block_ticks"] {
            if let Some(ticks) = nbt::get_mut(chunk, key).and_then(nbt::list_mut) {
                for tick in ticks {
                    rename_field(tick, "i", version, false, report);
                }
            }
        }
    }
    if kind == NodeKind::Item
        && let Some(legacy) = nbt::get_mut(tag, "tag")
    {
        for key in ["CanDestroy", "CanPlaceOn"] {
            if let Some(names) = nbt::get_mut(legacy, key).and_then(nbt::list_mut) {
                for name in names {
                    rename_name(name, version, false, report);
                }
            }
        }
    }
}

fn spawner_data(tag: &mut Tag) {
    if let Some(entity) = nbt::take(tag, "SpawnData") {
        nbt::set(tag, "SpawnData", nbt::compound(vec![("entity", entity)]));
    }
    if let Some(entries) = nbt::get_mut(tag, "SpawnPotentials").and_then(nbt::list_mut) {
        for entry in entries {
            let weight = nbt::take(entry, "Weight")
                .as_ref()
                .and_then(nbt::number)
                .unwrap_or(1.0) as i32;
            nbt::rename(entry, "Entity", "entity");
            let data = std::mem::replace(entry, Tag::Compound(Vec::new()));
            *entry = nbt::compound(vec![("data", data), ("weight", Tag::Int(weight))]);
        }
    }
}

fn rename_field(tag: &mut Tag, key: &str, version: i32, item: bool, report: &mut UpgradeReport) {
    if let Some(name) = nbt::get_mut(tag, key) {
        rename_name(name, version, item, report);
    }
}

fn rename_name(tag: &mut Tag, version: i32, item: bool, report: &mut UpgradeReport) {
    let Some(name) = nbt::string(tag) else { return };
    let Some(new) = renamed(name, version, item) else {
        return;
    };
    *tag = Tag::String(new.into());
    report.applied(
        version,
        if item {
            "ItemRenameFix"
        } else {
            "BlockRenameFix"
        },
    );
}

fn renamed(name: &str, version: i32, item: bool) -> Option<&'static str> {
    let pairs: &[(&str, &str)] = match version {
        2680 => &[("minecraft:grass_path", "minecraft:dirt_path")],
        2690 => COPPER,
        2691 => &[
            ("minecraft:waxed_copper", "minecraft:waxed_copper_block"),
            (
                "minecraft:oxidized_copper_block",
                "minecraft:oxidized_copper",
            ),
            (
                "minecraft:weathered_copper_block",
                "minecraft:weathered_copper",
            ),
            ("minecraft:exposed_copper_block", "minecraft:exposed_copper"),
        ],
        2696 => DEEPSLATE,
        2700 if !item => &[
            ("minecraft:cave_vines_head", "minecraft:cave_vines"),
            ("minecraft:cave_vines_body", "minecraft:cave_vines_plant"),
        ],
        2717 => &[(
            "minecraft:azalea_leaves_flowers",
            "minecraft:flowering_azalea_leaves",
        )],
        _ => return None,
    };
    pairs
        .iter()
        .find(|(old, _)| name == *old)
        .map(|(_, new)| *new)
}

const COPPER: &[(&str, &str)] = &[
    (
        "minecraft:weathered_copper_block",
        "minecraft:oxidized_copper_block",
    ),
    (
        "minecraft:semi_weathered_copper_block",
        "minecraft:weathered_copper_block",
    ),
    (
        "minecraft:lightly_weathered_copper_block",
        "minecraft:exposed_copper_block",
    ),
    (
        "minecraft:weathered_cut_copper",
        "minecraft:oxidized_cut_copper",
    ),
    (
        "minecraft:semi_weathered_cut_copper",
        "minecraft:weathered_cut_copper",
    ),
    (
        "minecraft:lightly_weathered_cut_copper",
        "minecraft:exposed_cut_copper",
    ),
    (
        "minecraft:weathered_cut_copper_stairs",
        "minecraft:oxidized_cut_copper_stairs",
    ),
    (
        "minecraft:semi_weathered_cut_copper_stairs",
        "minecraft:weathered_cut_copper_stairs",
    ),
    (
        "minecraft:lightly_weathered_cut_copper_stairs",
        "minecraft:exposed_cut_copper_stairs",
    ),
    (
        "minecraft:weathered_cut_copper_slab",
        "minecraft:oxidized_cut_copper_slab",
    ),
    (
        "minecraft:semi_weathered_cut_copper_slab",
        "minecraft:weathered_cut_copper_slab",
    ),
    (
        "minecraft:lightly_weathered_cut_copper_slab",
        "minecraft:exposed_cut_copper_slab",
    ),
    (
        "minecraft:waxed_semi_weathered_copper",
        "minecraft:waxed_weathered_copper",
    ),
    (
        "minecraft:waxed_lightly_weathered_copper",
        "minecraft:waxed_exposed_copper",
    ),
    (
        "minecraft:waxed_semi_weathered_cut_copper",
        "minecraft:waxed_weathered_cut_copper",
    ),
    (
        "minecraft:waxed_lightly_weathered_cut_copper",
        "minecraft:waxed_exposed_cut_copper",
    ),
    (
        "minecraft:waxed_semi_weathered_cut_copper_stairs",
        "minecraft:waxed_weathered_cut_copper_stairs",
    ),
    (
        "minecraft:waxed_lightly_weathered_cut_copper_stairs",
        "minecraft:waxed_exposed_cut_copper_stairs",
    ),
    (
        "minecraft:waxed_semi_weathered_cut_copper_slab",
        "minecraft:waxed_weathered_cut_copper_slab",
    ),
    (
        "minecraft:waxed_lightly_weathered_cut_copper_slab",
        "minecraft:waxed_exposed_cut_copper_slab",
    ),
];

const DEEPSLATE: &[(&str, &str)] = &[
    ("minecraft:grimstone", "minecraft:deepslate"),
    (
        "minecraft:grimstone_slab",
        "minecraft:cobbled_deepslate_slab",
    ),
    (
        "minecraft:grimstone_stairs",
        "minecraft:cobbled_deepslate_stairs",
    ),
    (
        "minecraft:grimstone_wall",
        "minecraft:cobbled_deepslate_wall",
    ),
    (
        "minecraft:polished_grimstone",
        "minecraft:polished_deepslate",
    ),
    (
        "minecraft:polished_grimstone_slab",
        "minecraft:polished_deepslate_slab",
    ),
    (
        "minecraft:polished_grimstone_stairs",
        "minecraft:polished_deepslate_stairs",
    ),
    (
        "minecraft:polished_grimstone_wall",
        "minecraft:polished_deepslate_wall",
    ),
    ("minecraft:grimstone_tiles", "minecraft:deepslate_tiles"),
    (
        "minecraft:grimstone_tile_slab",
        "minecraft:deepslate_tile_slab",
    ),
    (
        "minecraft:grimstone_tile_stairs",
        "minecraft:deepslate_tile_stairs",
    ),
    (
        "minecraft:grimstone_tile_wall",
        "minecraft:deepslate_tile_wall",
    ),
    ("minecraft:grimstone_bricks", "minecraft:deepslate_bricks"),
    (
        "minecraft:grimstone_brick_slab",
        "minecraft:deepslate_brick_slab",
    ),
    (
        "minecraft:grimstone_brick_stairs",
        "minecraft:deepslate_brick_stairs",
    ),
    (
        "minecraft:grimstone_brick_wall",
        "minecraft:deepslate_brick_wall",
    ),
    (
        "minecraft:chiseled_grimstone",
        "minecraft:chiseled_deepslate",
    ),
];

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Tag {
        Tag::String(value.into())
    }
    fn apply_at(tag: &mut Tag, kind: NodeKind, version: i32) {
        fix_node(tag, kind, version, &mut UpgradeReport::default());
    }

    #[test]
    fn cauldron_properties_and_version_boundaries() {
        let mut empty = nbt::compound(vec![
            ("Name", text("minecraft:cauldron")),
            ("Properties", nbt::compound(vec![("level", text("0"))])),
        ]);
        let original = empty.clone();
        apply_at(&mut empty, NodeKind::BlockState, 2678);
        assert_eq!(empty, original);
        apply_at(&mut empty, NodeKind::BlockState, 2679);
        assert!(nbt::get(&empty, "Properties").is_none());
        let mut filled = nbt::compound(vec![
            ("Name", text("minecraft:cauldron")),
            ("Properties", nbt::compound(vec![("level", text("2"))])),
        ]);
        apply_at(&mut filled, NodeKind::BlockState, 2679);
        assert_eq!(
            nbt::get(&filled, "Name"),
            Some(&text("minecraft:water_cauldron"))
        );
        assert_eq!(
            nbt::get(nbt::get(&filled, "Properties").unwrap(), "level"),
            Some(&text("2"))
        );
    }

    #[test]
    fn copper_renames_are_simultaneous_not_cascaded() {
        let mut state = nbt::compound(vec![(
            "Name",
            text("minecraft:semi_weathered_copper_block"),
        )]);
        apply_at(&mut state, NodeKind::BlockState, 2690);
        assert_eq!(
            nbt::get(&state, "Name"),
            Some(&text("minecraft:weathered_copper_block"))
        );
        apply_at(&mut state, NodeKind::BlockState, 2691);
        assert_eq!(
            nbt::get(&state, "Name"),
            Some(&text("minecraft:weathered_copper"))
        );
    }

    #[test]
    fn arrow_pickup_preserves_existing_value_and_player_remainder() {
        let mut arrow = nbt::compound(vec![
            ("id", text("minecraft:arrow")),
            ("player", Tag::Byte(0)),
        ]);
        apply_at(&mut arrow, NodeKind::Entity, 2702);
        assert_eq!(nbt::get(&arrow, "pickup"), Some(&Tag::Byte(0)));
        assert!(nbt::get(&arrow, "player").is_none());
        nbt::set(&mut arrow, "player", Tag::Byte(1));
        apply_at(&mut arrow, NodeKind::Entity, 2702);
        assert_eq!(nbt::get(&arrow, "player"), Some(&Tag::Byte(1)));
        let mut trident = nbt::compound(vec![("id", text("minecraft:trident"))]);
        apply_at(&mut trident, NodeKind::Entity, 2702);
        assert_eq!(nbt::get(&trident, "pickup"), Some(&Tag::Byte(1)));
    }

    #[test]
    fn spawner_weighted_entries_preserve_data_remainder() {
        let mut spawner = nbt::compound(vec![
            (
                "SpawnData",
                nbt::compound(vec![("id", text("minecraft:zombie"))]),
            ),
            (
                "SpawnPotentials",
                Tag::List(ListTag {
                    element_type: mc_nbt::tag_type::COMPOUND,
                    elements: vec![nbt::compound(vec![
                        (
                            "Entity",
                            nbt::compound(vec![("id", text("minecraft:skeleton"))]),
                        ),
                        ("Weight", Tag::Short(4)),
                        ("custom", Tag::Int(9)),
                    ])],
                }),
            ),
        ]);
        spawner_data(&mut spawner);
        assert!(nbt::get(nbt::get(&spawner, "SpawnData").unwrap(), "entity").is_some());
        let entry = &nbt::list(nbt::get(&spawner, "SpawnPotentials").unwrap()).unwrap()[0];
        assert_eq!(nbt::get(entry, "weight"), Some(&Tag::Int(4)));
        let data = nbt::get(entry, "data").unwrap();
        assert_eq!(nbt::get(data, "custom"), Some(&Tag::Int(9)));
        assert!(nbt::get(data, "entity").is_some());
        assert!(nbt::get(data, "Entity").is_none());
        assert!(nbt::get(data, "Weight").is_none());
    }

    #[test]
    fn rename_tables_cover_both_typed_roots_and_leave_remainders_opaque() {
        for (version, pairs) in [(2690, COPPER), (2696, DEEPSLATE)] {
            for &(old, new) in pairs {
                for (kind, key) in [(NodeKind::BlockState, "Name"), (NodeKind::Item, "id")] {
                    let mut node = nbt::compound(vec![
                        (key, text(old)),
                        ("custom", nbt::compound(vec![(key, text(old))])),
                    ]);
                    apply_at(&mut node, kind, version);
                    assert_eq!(nbt::get(&node, key), Some(&text(new)));
                    assert_eq!(
                        nbt::get(nbt::get(&node, "custom").unwrap(), key),
                        Some(&text(old)),
                    );
                }
            }
        }
        assert_eq!(renamed("minecraft:cave_vines_head", 2700, true), None);
        assert_eq!(renamed("minecraft:grass_path", 2681, false), None);
    }
}
