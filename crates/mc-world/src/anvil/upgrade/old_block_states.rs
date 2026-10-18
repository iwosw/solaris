//! Exact legacy block-state lookup data extracted from the local official26.1.2 jar.

use super::{
    NodeKind, UpgradeError, UpgradeReport,
    nbt::{fields, get, get_mut, set, string, take},
};
use mc_nbt::Tag;

type State = (&'static str, &'static [(&'static str, &'static str)]);
type OldState = (&'static str, &'static [(&'static str, &'static str)], u16);
mod renames;
mod tables;

pub(super) fn renamed(name: &str, version: i32) -> Option<&'static str> {
    renames::renamed(name, version)
}

pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if kind != NodeKind::BlockState {
        return Ok(());
    }
    if version == 1450 && upgrade(tag) {
        report.applied(version, "BlockStateStructureTemplateFix");
    }
    if let Some(name) = get(tag, "Name")
        .and_then(string)
        .and_then(|name| renamed(name, version))
    {
        set(tag, "Name", Tag::String(name.into()));
        report.applied(version, "BlockRenameFix");
    }
    match version {
        2503 if get(tag, "Name")
            .and_then(string)
            .is_some_and(|name| WALLS.contains(&name)) =>
        {
            if let Some(properties) = get_mut(tag, "Properties") {
                let mut changed = false;
                for key in ["east", "west", "north", "south"] {
                    if let Some(Tag::String(value)) = get_mut(properties, key) {
                        let replacement = if value == "true" { "low" } else { "none" };
                        if value != replacement {
                            value.clear();
                            value.push_str(replacement);
                            changed = true;
                        }
                    }
                }
                if changed {
                    report.applied(version, "WallPropertyFix");
                }
            }
        }
        2518 if get(tag, "Name").and_then(string) == Some("minecraft:jigsaw") => {
            if let Some(properties) = get_mut(tag, "Properties") {
                let facing = take(properties, "facing");
                let mut facing = match facing {
                    Some(Tag::String(value)) => value,
                    _ => "north".into(),
                };
                let replacement = match facing.as_str() {
                    "down" => Some("down_south"),
                    "up" => Some("up_north"),
                    "north" => Some("north_up"),
                    "south" => Some("south_up"),
                    "west" => Some("west_up"),
                    "east" => Some("east_up"),
                    _ => None,
                };
                if let Some(replacement) = replacement {
                    facing.clear();
                    facing.push_str(replacement);
                }
                set(properties, "orientation", Tag::String(facing));
                report.applied(version, "JigsawRotationFix");
            }
        }
        2531 if get(tag, "Name").and_then(string) == Some("minecraft:redstone_wire") => {
            if let Some(properties) = get_mut(tag, "Properties") {
                // Compute both axes from the ORIGINAL values before updating.
                let connected =
                    |key| get(properties, key).and_then(string).unwrap_or("none") != "none";
                let ew = connected("east") || connected("west");
                let ns = connected("north") || connected("south");
                let mut changed = false;
                for (key, other_axis) in [("east", ns), ("west", ns), ("north", ew), ("south", ew)]
                {
                    let Some(value) = get_mut(properties, key) else {
                        continue;
                    };
                    match value {
                        Tag::String(text) if text == "none" && !other_axis => {
                            text.clear();
                            text.push_str("side");
                            changed = true;
                        }
                        Tag::String(_) => {}
                        _ => {
                            *value = Tag::String(if other_axis { "none" } else { "side" }.into());
                            changed = true;
                        }
                    }
                }
                if changed {
                    report.applied(version, "RedstoneWireConnectionsFix");
                }
            }
        }
        _ => {}
    }
    Ok(())
}

// Exact WallPropertyFix.WALL_BLOCKS set, disassembled from local26.1.2.
const WALLS: &[&str] = &[
    "minecraft:andesite_wall",
    "minecraft:brick_wall",
    "minecraft:cobblestone_wall",
    "minecraft:diorite_wall",
    "minecraft:end_stone_brick_wall",
    "minecraft:granite_wall",
    "minecraft:mossy_cobblestone_wall",
    "minecraft:mossy_stone_brick_wall",
    "minecraft:nether_brick_wall",
    "minecraft:prismarine_wall",
    "minecraft:red_nether_brick_wall",
    "minecraft:red_sandstone_wall",
    "minecraft:sandstone_wall",
    "minecraft:stone_brick_wall",
];

/// BlockStateData.getTag: invalid numeric states resolve to vanilla's state0.
pub(super) fn state(id: i32) -> Tag {
    let index = usize::try_from(id)
        .ok()
        .and_then(|id| tables::STATE_IDS.get(id))
        .copied()
        .unwrap_or(tables::STATE_IDS[0]);
    make(tables::STATES[usize::from(index)])
}

fn make((name, properties): State) -> Tag {
    let mut fields = Vec::with_capacity(if properties.is_empty() { 1 } else { 2 });
    fields.push(("Name".into(), Tag::String(name.into())));
    if !properties.is_empty() {
        let values = properties
            .iter()
            .map(|&(key, value)| (key.into(), Tag::String(value.into())))
            .collect();
        fields.push(("Properties".into(), Tag::Compound(values)));
    }
    Tag::Compound(fields)
}

/// EntityBlockStateFix.getBlockId uses the exact name table, default0.
pub(super) fn block_id(name: &str) -> i32 {
    tables::BLOCK_IDS
        .binary_search_by_key(&name, |&(name, _)| name)
        .map_or(0, |index| tables::BLOCK_IDS[index].1)
}

/// BlockStateData.upgradeBlockStateTag matches the COMPLETE old Dynamic value,
/// not just a familiar Name/Properties fragment inside an opaque compound.
pub(super) fn upgrade(tag: &mut Tag) -> bool {
    let Some(name) = get(tag, "Name").and_then(string) else {
        return false;
    };
    let first = tables::OLD_STATES.partition_point(|&(old, _, _)| old < name);
    for &(old, properties, index) in &tables::OLD_STATES[first..] {
        if old != name {
            break;
        }
        if matches(tag, properties) {
            *tag = make(tables::STATES[usize::from(index)]);
            return true;
        }
    }
    false
}

fn matches(tag: &Tag, expected: &[(&str, &str)]) -> bool {
    let Some(root) = fields(tag) else {
        return false;
    };
    if root.len() != if expected.is_empty() { 1 } else { 2 } {
        return false;
    }
    if expected.is_empty() {
        return get(tag, "Properties").is_none();
    }
    let Some(properties) = get(tag, "Properties").and_then(fields) else {
        return false;
    };
    properties.len() == expected.len()
        && expected.iter().all(|&(key, value)| {
            properties
                .iter()
                .find(|(name, _)| name == key)
                .and_then(|(_, value)| string(value))
                == Some(value)
        })
}

#[cfg(test)]
mod tests {
    use super::super::nbt::*;
    use super::*;

    #[test]
    fn legacy_property_fixers_preserve_schema_specific_scope() {
        let mut wall = compound(vec![
            ("Name", Tag::String("minecraft:cobblestone_wall".into())),
            (
                "Properties",
                compound(vec![
                    ("east", Tag::String("true".into())),
                    ("west", Tag::String("false".into())),
                ]),
            ),
        ]);
        fix_node(
            &mut wall,
            super::super::NodeKind::BlockState,
            2503,
            &mut super::super::UpgradeReport::default(),
        )
        .unwrap();
        let props = get(&wall, "Properties").unwrap();
        assert_eq!(get(props, "east").and_then(string), Some("low"));
        assert_eq!(get(props, "west").and_then(string), Some("none"));
        assert!(get(props, "north").is_none());
        let mut jigsaw = compound(vec![
            ("Name", Tag::String("minecraft:jigsaw".into())),
            (
                "Properties",
                compound(vec![("facing", Tag::String("down".into()))]),
            ),
        ]);
        fix_node(
            &mut jigsaw,
            super::super::NodeKind::BlockState,
            2518,
            &mut super::super::UpgradeReport::default(),
        )
        .unwrap();
        assert_eq!(
            get(get(&jigsaw, "Properties").unwrap(), "orientation").and_then(string),
            Some("down_south")
        );
        let mut wire = compound(vec![
            ("Name", Tag::String("minecraft:redstone_wire".into())),
            (
                "Properties",
                compound(vec![
                    ("east", Tag::String("none".into())),
                    ("west", Tag::String("none".into())),
                    ("north", Tag::String("none".into())),
                    ("south", Tag::String("none".into())),
                ]),
            ),
        ]);
        fix_node(
            &mut wire,
            super::super::NodeKind::BlockState,
            2531,
            &mut super::super::UpgradeReport::default(),
        )
        .unwrap();
        for key in ["east", "west", "north", "south"] {
            assert_eq!(
                get(get(&wire, "Properties").unwrap(), key).and_then(string),
                Some("side")
            );
        }
    }

    #[test]
    fn numeric_states_preserve_metadata_and_vanilla_air_fallback() {
        assert_eq!(
            get(&state(16), "Name").and_then(string),
            Some("minecraft:stone")
        );
        assert_eq!(
            get(&state(17), "Name").and_then(string),
            Some("minecraft:granite")
        );
        assert_eq!(state(-1), state(0));
        assert_eq!(state(4096), state(0));
        assert_eq!(block_id("minecraft:stone"), 1);
        assert_eq!(block_id("mod:stone"), 0);
    }

    #[test]
    fn exact_old_state_matches_do_not_interpret_opaque_fields() {
        let mut old = compound(vec![
            ("Name", Tag::String("minecraft:stone".into())),
            (
                "Properties",
                compound(vec![("variant", Tag::String("granite".into()))]),
            ),
        ]);
        assert!(upgrade(&mut old));
        assert_eq!(
            get(&old, "Name").and_then(string),
            Some("minecraft:granite")
        );
        let mut opaque = compound(vec![
            ("Name", Tag::String("minecraft:stone".into())),
            (
                "Properties",
                compound(vec![("variant", Tag::String("granite".into()))]),
            ),
            ("opaque", Tag::Int(1)),
        ]);
        let before = opaque.clone();
        assert!(!upgrade(&mut opaque));
        assert_eq!(opaque, before);
    }
}
