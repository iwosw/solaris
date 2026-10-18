//! Player and map fixers derived from the local 26.1.2 DataFixers bytecode.

use super::{NodeKind, TARGET_DATA_VERSION, UpgradeError, UpgradeReport, nbt::*};
use mc_nbt::Tag;

#[cfg(test)]
mod oracle_tests;
mod recipes;

pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if kind != NodeKind::Player {
        return Ok(());
    }
    match version {
        1502 | 1510 | 2100 => {
            if let Some(book) = get_mut(tag, "recipeBook") {
                let changed = fix_recipes(book, "recipes", version)
                    | fix_recipes(book, "toBeDisplayed", version);
                if changed {
                    report.applied(version, "NamespacedTypeRenameFix");
                }
            }
        }
        2514 => {
            let mut changed = replace_uuid(tag, "UUID", "UUID");
            if let Some(vehicle) = get_mut(tag, "RootVehicle") {
                changed |= replace_uuid(vehicle, "Attach", "Attach");
            }
            changed |= living_uuid(tag);
            if changed {
                report.applied(version, "PlayerUUIDFix");
            }
        }
        2523 => {
            if rename_attributes(tag) {
                report.applied(version, "AttributesRenameLegacy");
            }
        }
        2537 => {
            if dimension(tag, "Dimension") {
                report.applied(version, "PlayerLegacyDimensionFix");
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn fix_recipes(tag: &mut Tag, key: &str, version: i32) -> bool {
    let mut changed = false;
    if let Some(values) = get_mut(tag, key).and_then(list_mut) {
        for value in values {
            if let Some(name) = string(value).and_then(|name| recipes::renamed(name, version)) {
                *value = Tag::String(name.into());
                changed = true;
            }
        }
    }
    changed
}

// AbstractUUIDFix.createUUIDFromLongs rejects either zero half, not only nil UUID.
pub(super) fn replace_uuid(tag: &mut Tag, prefix: &str, target: &str) -> bool {
    let number = |suffix: &str| {
        let value = fields(tag)
            .and_then(|fields| {
                fields
                    .iter()
                    .find(|(name, _)| name.strip_suffix(suffix) == Some(prefix))
            })
            .map(|(_, value)| value);
        match value {
            Some(Tag::Long(value)) => *value,
            Some(value) => super::nbt::number(value).unwrap_or(0.0) as i64,
            None => 0,
        }
    };
    let most = number("Most");
    let least = number("Least");
    if most == 0 || least == 0 {
        return false;
    }
    if let Some(fields) = fields_mut(tag) {
        fields.retain(|(name, _)| {
            name.strip_suffix("Most") != Some(prefix) && name.strip_suffix("Least") != Some(prefix)
        });
    }
    set(
        tag,
        target,
        Tag::IntArray(vec![
            (most >> 32) as i32,
            most as i32,
            (least >> 32) as i32,
            least as i32,
        ]),
    );
    true
}

pub(super) fn living_uuid(tag: &mut Tag) -> bool {
    let mut changed = false;
    if let Some(attributes) = get_mut(tag, "Attributes").and_then(list_mut) {
        for attribute in attributes {
            if let Some(modifiers) = get_mut(attribute, "Modifiers").and_then(list_mut) {
                for modifier in modifiers {
                    changed |= replace_uuid(modifier, "UUID", "UUID");
                }
            }
        }
    }
    changed
}

pub(super) fn rename_attributes(tag: &mut Tag) -> bool {
    let mut changed = false;
    if let Some(attributes) = get_mut(tag, "Attributes").and_then(list_mut) {
        for attribute in attributes {
            if let Some(name) = get(attribute, "Name")
                .and_then(string)
                .and_then(rename_attribute_name)
            {
                set(attribute, "Name", Tag::String(name.into()));
                changed = true;
            }
        }
    }
    changed
}

// Exact, non-namespacing lookup from DataFixers.addFixers schema 2523.
pub(super) fn rename_attribute_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "generic.maxHealth" | "Max Health" => "minecraft:generic.max_health",
        "zombie.spawnReinforcements" | "Spawn Reinforcements Chance" => {
            "minecraft:zombie.spawn_reinforcements"
        }
        "horse.jumpStrength" | "Jump Strength" => "minecraft:horse.jump_strength",
        "generic.followRange" | "Follow Range" => "minecraft:generic.follow_range",
        "generic.knockbackResistance" | "Knockback Resistance" => {
            "minecraft:generic.knockback_resistance"
        }
        "generic.movementSpeed" | "Movement Speed" => "minecraft:generic.movement_speed",
        "generic.flyingSpeed" | "Flying Speed" => "minecraft:generic.flying_speed",
        "generic.attackDamage" => "minecraft:generic.attack_damage",
        "generic.attackKnockback" => "minecraft:generic.attack_knockback",
        "generic.attackSpeed" => "minecraft:generic.attack_speed",
        "generic.armorToughness" => "minecraft:generic.armor_toughness",
        _ => return None,
    })
}

fn dimension(tag: &mut Tag, key: &str) -> bool {
    let Some(value) = get(tag, key).and_then(java_int) else {
        return false;
    };
    let name = match value {
        -1 => "minecraft:the_nether",
        1 => "minecraft:the_end",
        _ => "minecraft:overworld",
    };
    set(tag, key, Tag::String(name.into()));
    true
}

pub(super) fn upgrade_map_data(tag: &mut Tag, from: i32) -> Result<UpgradeReport, UpgradeError> {
    if from > TARGET_DATA_VERSION {
        return Err(UpgradeError::UnsupportedVersion(from));
    }
    if fields(tag).is_none() {
        return Err(UpgradeError::malformed("$", "expected map compound"));
    }
    if let Some(version) = get(tag, "DataVersion") {
        let actual = int(version)
            .ok_or_else(|| UpgradeError::malformed("DataVersion", "expected integer"))?;
        if actual != from {
            return Err(UpgradeError::VersionMismatch {
                supplied: from,
                actual,
            });
        }
    }
    let data = get_mut(tag, "data")
        .filter(|data| fields(data).is_some())
        .ok_or_else(|| UpgradeError::malformed("data", "expected map data compound"))?;
    let mut report = UpgradeReport {
        from,
        to: TARGET_DATA_VERSION,
        ..UpgradeReport::default()
    };
    if from < 2537 && dimension(data, "dimension") {
        report.applied(2537, "MapLegacyDimensionFix");
    }
    for &version in super::LEGACY_AUX_VERSIONS
        .iter()
        .chain(super::OLD_VERSIONS)
        .chain(super::VERSIONS)
        .filter(|&&version| version > from || (version == from && matches!(version, 1451 | 3807)))
    {
        if version == 3807
            && let Some(banners) = get_mut(data, "banners").and_then(list_mut)
        {
            for banner in banners {
                if let Some(pos) = get_mut(banner, "Pos")
                    && block_pos(pos)
                {
                    report.applied(version, "MapBannerBlockPosFormatFix");
                }
            }
        }
        if version == 3813 {
            let mut changed = false;
            if let Some(frames) = get_mut(data, "frames").and_then(list_mut) {
                for frame in frames {
                    if let Some(pos) = get_mut(frame, "Pos") {
                        block_pos(pos);
                    }
                    for (old, new) in [
                        ("Pos", "pos"),
                        ("Rotation", "rotation"),
                        ("EntityId", "entity_id"),
                    ] {
                        changed |= get(frame, old).is_some();
                        rename(frame, old, new);
                    }
                }
            }
            if let Some(banners) = get_mut(data, "banners").and_then(list_mut) {
                for banner in banners {
                    for (old, new) in [("Pos", "pos"), ("Color", "color"), ("Name", "name")] {
                        changed |= get(banner, old).is_some();
                        rename(banner, old, new);
                    }
                }
            }
            if changed {
                report.applied(version, "BlockPosFormatAndRenamesFix");
            }
        }
        if let Some(banners) = get_mut(data, "banners").and_then(list_mut) {
            for banner in banners {
                let key = if version < 3813 { "Name" } else { "name" };
                if let Some(name) = get_mut(banner, key) {
                    super::walk::visit(name, NodeKind::Text, version, &mut report)?;
                }
            }
        }
    }
    set(tag, "DataVersion", Tag::Int(TARGET_DATA_VERSION));
    Ok(report)
}

fn block_pos(pos: &mut Tag) -> bool {
    let [Some(x), Some(y), Some(z)] = ["X", "Y", "Z"].map(|key| get(pos, key).and_then(java_int))
    else {
        return false;
    };
    *pos = Tag::IntArray(vec![x, y, z]);
    true
}

fn java_int(value: &Tag) -> Option<i32> {
    match value {
        Tag::Long(value) => Some(*value as i32),
        Tag::Float(value) => Some(*value as i32),
        Tag::Double(value) => Some(*value as i32),
        _ => int(value),
    }
}

#[cfg(test)]
mod tests {
    use super::super::{nbt::*, *};

    #[test]
    fn player_recipe_book_uses_all_three_historical_rename_tables() {
        let mut player = compound(vec![(
            "recipeBook",
            compound(vec![
                (
                    "recipes",
                    sequence(vec![
                        text("minecraft:acacia_wooden_slab"),
                        text("minecraft:acacia_bark"),
                        text("minecraft:sugar"),
                        text("mod:sugar"),
                    ]),
                ),
                ("toBeDisplayed", sequence(vec![text("minecraft:sugar")])),
            ]),
        )]);
        upgrade_player(&mut player, 1343).unwrap();
        let book = get(&player, "recipeBook").unwrap();
        assert_eq!(
            list(get(book, "recipes").unwrap()).unwrap(),
            &[
                text("minecraft:acacia_slab"),
                text("minecraft:acacia_wood"),
                text("minecraft:sugar_from_sugar_cane"),
                text("mod:sugar")
            ]
        );
        assert_eq!(
            list(get(book, "toBeDisplayed").unwrap()).unwrap(),
            &[text("minecraft:sugar_from_sugar_cane")]
        );
    }

    #[test]
    fn map_banner_text_and_frame_fields_follow_current_schema() {
        let pos = || {
            compound(vec![
                ("X", Tag::Int(2)),
                ("Y", Tag::Int(3)),
                ("Z", Tag::Int(4)),
            ])
        };
        let mut map = compound(vec![(
            "data",
            compound(vec![
                (
                    "banners",
                    sequence(vec![compound(vec![
                        ("Pos", pos()),
                        ("Name", text("{\"text\":\"mark\"}")),
                        ("Color", text("red")),
                    ])]),
                ),
                (
                    "frames",
                    sequence(vec![compound(vec![
                        ("Pos", pos()),
                        ("Rotation", Tag::Int(3)),
                        ("EntityId", Tag::Int(42)),
                    ])]),
                ),
            ]),
        )]);
        upgrade_map_data(&mut map, 2580).unwrap();
        let data = get(&map, "data").unwrap();
        let banner = &list(get(data, "banners").unwrap()).unwrap()[0];
        assert_eq!(
            get(get(banner, "name").unwrap(), "text"),
            Some(&text("mark"))
        );
        assert_eq!(get(banner, "color"), Some(&text("red")));
        let frame = &list(get(data, "frames").unwrap()).unwrap()[0];
        assert_eq!(get(frame, "pos"), Some(&Tag::IntArray(vec![2, 3, 4])));
        assert_eq!(get(frame, "rotation"), Some(&Tag::Int(3)));
        assert_eq!(get(frame, "entity_id"), Some(&Tag::Int(42)));
    }

    #[test]
    fn map_position_conversion_requires_all_three_numbers() {
        let mut incomplete = compound(vec![("X", Tag::Int(9)), ("Y", Tag::Int(8))]);
        let before = incomplete.clone();
        assert!(!super::block_pos(&mut incomplete));
        assert_eq!(incomplete, before);
    }

    #[test]
    fn legacy_player_bands_reach_current_reader() {
        for from in [-1, 510, 1139, 1343, 1976, 2230, 2580, 2584] {
            let dimension = if from < 2537 {
                Tag::Int(-1)
            } else {
                text("minecraft:the_nether")
            };
            let mut player = compound(vec![("Dimension", dimension)]);
            if from < 2514 {
                set(
                    &mut player,
                    "UUIDMost",
                    Tag::Long(0x1234_5678_9abc_def0_u64 as i64),
                );
                set(&mut player, "UUIDLeast", Tag::Long(-1));
            }
            let report = upgrade_player(&mut player, from).unwrap();
            assert_eq!(report.from, from);
            assert_eq!(
                get(&player, "DataVersion"),
                Some(&Tag::Int(TARGET_DATA_VERSION))
            );
            assert!(get(&player, "UUIDMost").is_none());
        }
    }

    #[test]
    fn player_uuid_fix_preserves_invalid_halves_and_converts_vehicle_and_modifiers() {
        let mut player = compound(vec![
            ("UUIDMost", Tag::Long(0)),
            ("UUIDLeast", Tag::Long(2)),
            (
                "RootVehicle",
                compound(vec![
                    ("AttachMost", Tag::Long(1)),
                    ("AttachLeast", Tag::Long(2)),
                ]),
            ),
            (
                "Attributes",
                sequence(vec![compound(vec![
                    ("Name", text("generic.maxHealth")),
                    (
                        "Modifiers",
                        sequence(vec![compound(vec![
                            ("UUIDMost", Tag::Long(3)),
                            ("UUIDLeast", Tag::Long(4)),
                        ])]),
                    ),
                ])]),
            ),
        ]);
        let mut report = UpgradeReport::default();
        super::fix_node(&mut player, NodeKind::Player, 2514, &mut report).unwrap();
        assert_eq!(get(&player, "UUIDMost"), Some(&Tag::Long(0)));
        assert_eq!(
            get(get(&player, "RootVehicle").unwrap(), "Attach"),
            Some(&Tag::IntArray(vec![0, 1, 0, 2]))
        );
        let attribute = &list(get(&player, "Attributes").unwrap()).unwrap()[0];
        let modifier = &list(get(attribute, "Modifiers").unwrap()).unwrap()[0];
        assert_eq!(
            get(modifier, "UUID"),
            Some(&Tag::IntArray(vec![0, 3, 0, 4]))
        );
        super::fix_node(&mut player, NodeKind::Player, 2523, &mut report).unwrap();
        assert_eq!(
            get(
                &list(get(&player, "Attributes").unwrap()).unwrap()[0],
                "Name"
            ),
            Some(&text("minecraft:generic.max_health"))
        );
    }

    #[test]
    fn map_bands_preserve_pixels_and_upgrade_dimension_and_banner_positions() {
        for from in [-2, -1, 2230, 2580, 2586, 3807, 4790] {
            let old_dimension = from < 2537;
            let old_position = from <= 3807;
            let pos = if old_position {
                compound(vec![
                    ("X", Tag::Int(2)),
                    ("Y", Tag::Int(3)),
                    ("Z", Tag::Int(4)),
                ])
            } else {
                Tag::IntArray(vec![2, 3, 4])
            };
            let mut map = compound(vec![(
                "data",
                compound(vec![
                    (
                        "dimension",
                        if old_dimension {
                            Tag::Byte(1)
                        } else {
                            text("minecraft:the_end")
                        },
                    ),
                    ("colors", Tag::ByteArray(vec![-1, 0, 1])),
                    (
                        "banners",
                        sequence(vec![compound(vec![(
                            if from < 3813 { "Pos" } else { "pos" },
                            pos,
                        )])]),
                    ),
                ]),
            )]);
            let report = upgrade_map_data(&mut map, from).unwrap();
            assert_eq!(report.from, from);
            let data = get(&map, "data").unwrap();
            assert_eq!(get(data, "dimension"), Some(&text("minecraft:the_end")));
            assert_eq!(get(data, "colors"), Some(&Tag::ByteArray(vec![-1, 0, 1])));
            assert_eq!(
                get(&list(get(data, "banners").unwrap()).unwrap()[0], "pos"),
                Some(&Tag::IntArray(vec![2, 3, 4]))
            );
        }
    }

    fn text(value: &str) -> Tag {
        Tag::String(value.into())
    }
    fn sequence(elements: Vec<Tag>) -> Tag {
        Tag::List(mc_nbt::ListTag {
            element_type: elements.first().map_or(0, Tag::type_id),
            elements,
        })
    }
}
