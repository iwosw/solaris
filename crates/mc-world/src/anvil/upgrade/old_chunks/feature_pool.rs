//! Vanilla 2701 `SavedDataFeaturePoolElementFix`, restricted to typed starts.
use super::super::nbt;
use mc_nbt::Tag;

pub(super) fn apply(tag: &mut Tag) -> bool {
    let Some(starts) = nbt::get_mut(tag, "Level")
        .and_then(|level| nbt::get_mut(level, "Structures"))
        .and_then(|structures| nbt::get_mut(structures, "Starts"))
        .and_then(nbt::fields_mut)
    else {
        return false;
    };
    let mut changed = false;
    for (_, start) in starts {
        let Some(children) = nbt::get_mut(start, "Children").and_then(nbt::list_mut) else {
            continue;
        };
        for child in children {
            if !matches!(
                nbt::get(child, "id").and_then(nbt::string),
                Some(
                    "minecraft:jigsaw"
                        | "minecraft:nvi"
                        | "minecraft:pcp"
                        | "minecraft:bastionremnant"
                        | "minecraft:runtime"
                )
            ) {
                continue;
            }
            let Some(element) = nbt::get_mut(child, "pool_element") else {
                continue;
            };
            if nbt::get(element, "element_type").and_then(nbt::string)
                != Some("minecraft:feature_pool_element")
            {
                continue;
            }
            let Some(feature) = nbt::get_mut(element, "feature") else {
                continue;
            };
            if let Some(replacement) = replacement(feature) {
                *feature = Tag::String(replacement.into());
                changed = true;
            }
        }
    }
    changed
}

fn value_at<'a>(tag: &'a Tag, path: &[&str]) -> Option<&'a Tag> {
    path.iter().try_fold(tag, |value, key| nbt::get(value, key))
}

fn text_at<'a>(tag: &'a Tag, path: &[&str]) -> &'a str {
    value_at(tag, path).and_then(nbt::string).unwrap_or("")
}

fn weighted_state(feature: &Tag) -> &str {
    value_at(feature, &["config", "state_provider", "entries"])
        .and_then(nbt::list)
        .and_then(|entries| entries.first())
        .map_or("", |entry| text_at(entry, &["data", "Name"]))
}

fn replacement(feature: &Tag) -> Option<&'static str> {
    let mut kind = text_at(feature, &["type"]);
    if kind.is_empty() {
        kind = text_at(feature, &["name"]);
        if kind == "minecraft:normal_tree" {
            kind = "minecraft:tree";
        }
    }
    match kind {
        "minecraft:random_patch" | "minecraft:block_pile" => {
            match (
                kind,
                text_at(feature, &["config", "state_provider", "type"]),
            ) {
                ("minecraft:random_patch", "minecraft:simple_state_provider") => {
                    match text_at(feature, &["config", "state_provider", "state", "Name"]) {
                        "minecraft:sweet_berry_bush" => Some("minecraft:patch_berry_bush"),
                        "minecraft:cactus" => Some("minecraft:patch_cactus"),
                        _ => None,
                    }
                }
                ("minecraft:random_patch", "minecraft:weighted_state_provider") => {
                    match weighted_state(feature) {
                        "minecraft:grass" | "minecraft:fern" => Some("minecraft:patch_taiga_grass"),
                        _ => None,
                    }
                }
                (
                    "minecraft:block_pile",
                    "minecraft:simple_state_provider" | "minecraft:rotated_block_provider",
                ) => match text_at(feature, &["config", "state_provider", "state", "Name"]) {
                    "minecraft:hay_block" => Some("minecraft:pile_hay"),
                    "minecraft:melon" => Some("minecraft:pile_melon"),
                    "minecraft:snow" => Some("minecraft:pile_snow"),
                    _ => None,
                },
                ("minecraft:block_pile", "minecraft:weighted_state_provider") => {
                    match weighted_state(feature) {
                        "minecraft:packed_ice" | "minecraft:blue_ice" => Some("minecraft:pile_ice"),
                        "minecraft:jack_o_lantern" | "minecraft:pumpkin" => {
                            Some("minecraft:pile_pumpkin")
                        }
                        _ => None,
                    }
                }
                _ => None,
            }
        }
        "minecraft:flower" => Some("minecraft:flower_plain"),
        "minecraft:tree" => match text_at(feature, &["config", "foliage_placer", "type"]) {
            "minecraft:acacia_foliage_placer" => Some("minecraft:acacia"),
            "minecraft:blob_foliage_placer"
                if text_at(feature, &["config", "leaves_provider", "state", "Name"])
                    == "minecraft:oak_leaves" =>
            {
                Some("minecraft:oak")
            }
            "minecraft:pine_foliage_placer" => Some("minecraft:pine"),
            "minecraft:spruce_foliage_placer" => Some("minecraft:spruce"),
            _ => None,
        },
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_nbt::{ListTag, tag_type};

    fn text(value: &str) -> Tag {
        Tag::String(value.into())
    }
    fn list(elements: Vec<Tag>) -> Tag {
        Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements,
        })
    }
    fn feature(kind: &str, provider: &str, state: &str, foliage: &str, leaves: &str) -> Tag {
        nbt::compound(vec![
            ("type", text(kind)),
            (
                "config",
                nbt::compound(vec![
                    (
                        "state_provider",
                        nbt::compound(vec![
                            ("type", text(provider)),
                            ("state", nbt::compound(vec![("Name", text(state))])),
                            (
                                "entries",
                                list(vec![nbt::compound(vec![
                                    ("data", nbt::compound(vec![("Name", text(state))])),
                                    ("weight", Tag::Int(1)),
                                ])]),
                            ),
                        ]),
                    ),
                    (
                        "foliage_placer",
                        nbt::compound(vec![("type", text(foliage))]),
                    ),
                    (
                        "leaves_provider",
                        nbt::compound(vec![("state", nbt::compound(vec![("Name", text(leaves))]))]),
                    ),
                ]),
            ),
            ("opaque", Tag::Long(42)),
        ])
    }
    fn chunk(feature: Tag, piece: &str, element: &str) -> Tag {
        nbt::compound(vec![(
            "Level",
            nbt::compound(vec![(
                "Structures",
                nbt::compound(vec![(
                    "Starts",
                    nbt::compound(vec![(
                        "Village",
                        nbt::compound(vec![(
                            "Children",
                            list(vec![nbt::compound(vec![
                                ("id", text(piece)),
                                ("marker", Tag::Int(17)),
                                (
                                    "pool_element",
                                    nbt::compound(vec![
                                        ("element_type", text(element)),
                                        ("projection", text("rigid")),
                                        ("feature", feature),
                                    ]),
                                ),
                            ])]),
                        )]),
                    )]),
                )]),
            )]),
        )])
    }
    fn assert_replacement(input: Tag, expected: &str) {
        let mut tag = chunk(input, "minecraft:jigsaw", "minecraft:feature_pool_element");
        assert!(apply(&mut tag));
        assert_eq!(
            tag,
            chunk(
                text(expected),
                "minecraft:jigsaw",
                "minecraft:feature_pool_element"
            )
        );
        assert!(!apply(&mut tag));
    }

    #[test]
    fn java_1165_feature_pool_patch_and_pile_replacements_match_vanilla() {
        for (kind, provider, block, expected) in [
            (
                "random_patch",
                "simple_state_provider",
                "sweet_berry_bush",
                "patch_berry_bush",
            ),
            (
                "random_patch",
                "simple_state_provider",
                "cactus",
                "patch_cactus",
            ),
            (
                "random_patch",
                "weighted_state_provider",
                "grass",
                "patch_taiga_grass",
            ),
            (
                "random_patch",
                "weighted_state_provider",
                "fern",
                "patch_taiga_grass",
            ),
            (
                "block_pile",
                "simple_state_provider",
                "hay_block",
                "pile_hay",
            ),
            (
                "block_pile",
                "rotated_block_provider",
                "hay_block",
                "pile_hay",
            ),
            ("block_pile", "simple_state_provider", "melon", "pile_melon"),
            (
                "block_pile",
                "rotated_block_provider",
                "melon",
                "pile_melon",
            ),
            ("block_pile", "simple_state_provider", "snow", "pile_snow"),
            ("block_pile", "rotated_block_provider", "snow", "pile_snow"),
            (
                "block_pile",
                "weighted_state_provider",
                "packed_ice",
                "pile_ice",
            ),
            (
                "block_pile",
                "weighted_state_provider",
                "blue_ice",
                "pile_ice",
            ),
            (
                "block_pile",
                "weighted_state_provider",
                "jack_o_lantern",
                "pile_pumpkin",
            ),
            (
                "block_pile",
                "weighted_state_provider",
                "pumpkin",
                "pile_pumpkin",
            ),
        ] {
            assert_replacement(
                feature(
                    &format!("minecraft:{kind}"),
                    &format!("minecraft:{provider}"),
                    &format!("minecraft:{block}"),
                    "",
                    "",
                ),
                &format!("minecraft:{expected}"),
            );
        }
    }

    #[test]
    fn java_1165_feature_pool_flowers_trees_and_legacy_name_precedence() {
        assert_replacement(
            feature("minecraft:flower", "", "", "", ""),
            "minecraft:flower_plain",
        );
        for (foliage, leaves, expected) in [
            ("acacia_foliage_placer", "", "acacia"),
            ("blob_foliage_placer", "minecraft:oak_leaves", "oak"),
            ("pine_foliage_placer", "", "pine"),
            ("spruce_foliage_placer", "", "spruce"),
        ] {
            assert_replacement(
                feature(
                    "minecraft:tree",
                    "",
                    "",
                    &format!("minecraft:{foliage}"),
                    leaves,
                ),
                &format!("minecraft:{expected}"),
            );
        }
        let mut legacy = feature("", "", "", "minecraft:pine_foliage_placer", "");
        nbt::remove(&mut legacy, "type");
        nbt::set(&mut legacy, "name", text("minecraft:normal_tree"));
        assert_replacement(legacy.clone(), "minecraft:pine");
        nbt::set(&mut legacy, "type", text("mod:custom"));
        let mut tag = chunk(legacy, "minecraft:jigsaw", "minecraft:feature_pool_element");
        let before = tag.clone();
        assert!(!apply(&mut tag));
        assert_eq!(tag, before);
    }

    #[test]
    fn java_1165_feature_pool_respects_piece_element_and_feature_choices() {
        for piece in [
            "minecraft:jigsaw",
            "minecraft:nvi",
            "minecraft:pcp",
            "minecraft:bastionremnant",
            "minecraft:runtime",
        ] {
            let mut tag = chunk(
                feature("minecraft:flower", "", "", "", ""),
                piece,
                "minecraft:feature_pool_element",
            );
            assert!(apply(&mut tag));
            assert_eq!(
                tag,
                chunk(
                    text("minecraft:flower_plain"),
                    piece,
                    "minecraft:feature_pool_element"
                )
            );
        }
        for (piece, element, input) in [
            (
                "mod:jigsaw",
                "minecraft:feature_pool_element",
                feature("minecraft:flower", "", "", "", ""),
            ),
            (
                "minecraft:jigsaw",
                "minecraft:single_pool_element",
                feature("minecraft:flower", "", "", "", ""),
            ),
            (
                "minecraft:jigsaw",
                "minecraft:feature_pool_element",
                feature("mod:custom", "", "", "", ""),
            ),
            (
                "minecraft:jigsaw",
                "minecraft:feature_pool_element",
                feature(
                    "minecraft:tree",
                    "",
                    "",
                    "minecraft:blob_foliage_placer",
                    "minecraft:birch_leaves",
                ),
            ),
            (
                "minecraft:jigsaw",
                "minecraft:feature_pool_element",
                feature(
                    "minecraft:random_patch",
                    "mod:custom",
                    "minecraft:cactus",
                    "",
                    "",
                ),
            ),
            (
                "minecraft:jigsaw",
                "minecraft:feature_pool_element",
                text("minecraft:flower_plain"),
            ),
        ] {
            let mut tag = chunk(input, piece, element);
            let before = tag.clone();
            assert!(!apply(&mut tag));
            assert_eq!(tag, before);
        }
        let opaque = chunk(
            feature("minecraft:flower", "", "", "", ""),
            "minecraft:jigsaw",
            "minecraft:feature_pool_element",
        );
        let mut tag = nbt::compound(vec![("opaque", opaque)]);
        let before = tag.clone();
        assert!(!apply(&mut tag));
        assert_eq!(tag, before);
    }
}
