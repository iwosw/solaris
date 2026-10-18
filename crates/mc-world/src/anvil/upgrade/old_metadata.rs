//! 1.16.5 worldgen-audit fixes for the pre-1.18 level record.
//!
//! Bytecode authority: official 26.1.2 `server-26.1.2.jar`
//! (`.analysis/maps/falcons-rock/vanilla-hop/server/versions/26.1.2/server-26.1.2.jar`),
//! registrations in `.analysis/codex-logs/old-dataversion/DataFixers.javap`,
//! inventory `.analysis/codex-logs/old-dataversion/fixers.md`.
//! Applies to the `Level` node only; the typed `WORLD_GEN_SETTINGS` payload
//! lives at `Level.WorldGenSettings` in this era.
use super::{NodeKind, UpgradeError, UpgradeReport, nbt};
use mc_nbt::Tag;

/// Migrate the pre-1.18 level worldgen audit fields. All other node kinds and
/// versions are no-ops; the parent wires version order and the tree walk.
pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if kind != NodeKind::Level {
        return Ok(());
    }
    match version {
        // AddFlagIfNotPresentFix2693: WORLD_GEN_SETTINGS remainder gains
        // `has_increased_height_already: false` when absent; a stored value
        // (notably true) is authoritative and never overwritten.
        2693 => flag(tag, version, false, report)?,
        // AddFlagIfNotPresentFix2707: same key, default true.
        2707 => flag(tag, version, true, report)?,
        // AddFlagIfNotPresentFix2825: same key, default false.
        2825 => flag(tag, version, false, report)?,
        // WorldGenSettingsHeightAndBiomeFix2832: overworld generator
        // migration; see `height_and_biomes` for the vanilla branch rules.
        2832 => height_and_biomes(tag, report)?,
        // WorldGenSettingsDisallowOldCustomWorldsFix2833/2852: any dimension
        // entry without a direct `type` string is an old custom world
        // vanilla refuses to load (`Unable load old custom worlds.`).
        2833 | 2852 => require_dimension_types(tag, version, report)?,
        _ => {}
    }
    Ok(())
}

fn worldgen<'a>(
    tag: &'a mut Tag,
    version: i32,
    fix: &'static str,
) -> Result<Option<&'a mut Tag>, UpgradeError> {
    let Some(settings) = nbt::get_mut(tag, "WorldGenSettings") else {
        return Ok(None);
    };
    if nbt::fields(settings).is_none() {
        return Err(UpgradeError::unsupported(
            version,
            fix,
            "WorldGenSettings",
            "affected field is not a compound",
        ));
    }
    Ok(Some(settings))
}

fn flag(
    tag: &mut Tag,
    version: i32,
    default: bool,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    const FIX: &str = "AddFlagIfNotPresentFix";
    const KEY: &str = "has_increased_height_already";
    let Some(settings) = worldgen(tag, version, FIX)? else {
        return Ok(());
    };
    if nbt::get(settings, KEY).is_none() {
        nbt::set(settings, KEY, Tag::Byte(i8::from(default)));
        report.applied(version, FIX);
    }
    Ok(())
}

/// Reads the `has_increased_height_already` remainder flag left by the 2693 /
/// 2707 / 2825 `AddFlagIfNotPresentFix` registrations (`isEmpty` = absent,
/// `asBoolean(true)` = stored value), removes it from the remainder (vanilla
/// `lambda$makeRule$1` = `Dynamic.remove`), and migrates ONLY
/// `dimensions["minecraft:overworld"]` (vanilla `lambda$makeRule$3` =
/// `update("minecraft:overworld", ...)`; other dimensions untouched).
/// `increased` = stored flag value; absent defaults to true
/// (`asBoolean(true)`), so a world the flag fixers never stamped behaves as
/// already-increased. Evidence:
/// `.analysis/codex-logs/old-dataversion/chunks-WorldGenSettingsHeightAndBiomeFix.javap`.
fn height_and_biomes(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "WorldGenSettingsHeightAndBiomeFix";
    const VERSION: i32 = 2832;
    const KEY: &str = "has_increased_height_already";
    let Some(settings) = worldgen(tag, VERSION, FIX)? else {
        return Ok(());
    };
    // `absent` (flag never stamped: `isEmpty`) and `increased` (stored value,
    // `asBoolean(true)`) are INDEPENDENT: biome-source eligibility uses
    // absent, flat gating uses increased. A 2693-stamped `false` value means
    // NOT absent. Absent defaults increased to true.
    let (absent, increased) = match nbt::take(settings, KEY) {
        None => (true, true),
        Some(Tag::Byte(value)) => (false, value != 0),
        Some(_) => {
            return Err(UpgradeError::unsupported(
                VERSION,
                FIX,
                "WorldGenSettings.has_increased_height_already",
                "affected field is not a boolean flag",
            ));
        }
    };
    let Some(dimensions) = nbt::get_mut(settings, "dimensions") else {
        return Ok(());
    };
    if nbt::fields(dimensions).is_none() {
        return Err(UpgradeError::unsupported(
            VERSION,
            FIX,
            "WorldGenSettings.dimensions",
            "affected field is not a compound",
        ));
    }
    let Some(entry) = nbt::get_mut(dimensions, "minecraft:overworld") else {
        return Ok(());
    };
    let Tag::Compound(fields) = entry else {
        return Err(UpgradeError::unsupported(
            VERSION,
            FIX,
            "WorldGenSettings.dimensions[minecraft:overworld]",
            "affected field is not a compound",
        ));
    };
    let Some(index) = fields.iter().position(|(key, _)| key == "generator") else {
        return Ok(());
    };
    let mut generator = std::mem::replace(&mut fields[index].1, Tag::Compound(Vec::new()));
    if nbt::fields(&generator).is_none() {
        fields[index].1 = generator;
        return Err(UpgradeError::unsupported(
            VERSION,
            FIX,
            "WorldGenSettings.dimensions[minecraft:overworld].generator",
            "affected field is not a compound",
        ));
    }
    migrate_generator(&mut generator, absent, increased, VERSION, FIX)?;
    fields[index].1 = generator;
    report.applied(VERSION, FIX);
    Ok(())
}

/// Vanilla branch rules (`WorldGenSettingsHeightAndBiomeFix` javap):
/// - `generator.type == minecraft:noise`: run `biome_source` through
///   `lambda$makeRule$6(absent, changed)`. When `changed`, rewrite `settings`
///   through `lambda$makeRule$7` (flat settings STRING `minecraft:overworld`
///   becomes `minecraft:large_biomes`, anything else untouched).
/// - `generator.type == minecraft:flat`: when `increased` is false, rewrite
///   `settings` through `lambda$makeRule$8` (`layers` via `updateLayers`,
///   prepending `{height: 64, block: minecraft:air}`).
/// - any other/missing generator type: untouched.
fn migrate_generator(
    generator: &mut Tag,
    absent: bool,
    increased: bool,
    version: i32,
    fix: &'static str,
) -> Result<(), UpgradeError> {
    let is_noise = nbt::get(generator, "type").and_then(nbt::string) == Some("minecraft:noise");
    if is_noise {
        let Some(source) = nbt::get_mut(generator, "biome_source") else {
            return Ok(());
        };
        if nbt::fields(source).is_none() {
            return Err(UpgradeError::unsupported(
                version,
                fix,
                "WorldGenSettings.dimensions[minecraft:overworld].generator.biome_source",
                "affected field is not a compound",
            ));
        }
        if migrate_biome_source(source, absent) {
            update_noise_settings(generator);
        }
        return Ok(());
    }
    let is_flat = nbt::get(generator, "type").and_then(nbt::string) == Some("minecraft:flat");
    if is_flat && !increased {
        update_flat_layers(generator, version, fix)?;
    }
    Ok(())
}
/// Vanilla `lambda$makeRule$6(absent, changed)`: eligible when `type ==
/// minecraft:vanilla_layered`, or when the flag was ABSENT (never stamped,
/// independent of its stored value) AND `type == minecraft:multi_noise`.
/// An eligible source is ALWAYS replaced by a FRESH
/// `{preset: minecraft:overworld, type: minecraft:multi_noise}` map
/// (`createMap`, remainder dropped): `large_biomes.asBoolean(false)` false
/// skips only the `MutableBoolean.setTrue` (branch 44→51), then falls
/// through to the same fresh-map construction at 51..82. The returned flag
/// records only large-biomes eligibility, not whether replacement occurred.
fn migrate_biome_source(source: &mut Tag, absent: bool) -> bool {
    let source_type = nbt::get(source, "type").and_then(nbt::string);
    let eligible = source_type == Some("minecraft:vanilla_layered")
        || (absent && source_type == Some("minecraft:multi_noise"));
    if !eligible {
        return false;
    }
    let large = nbt::get(source, "large_biomes")
        .and_then(nbt::number)
        .is_some_and(|value| value as i8 != 0);
    *source = nbt::compound(vec![
        ("preset", Tag::String("minecraft:overworld".into())),
        ("type", Tag::String("minecraft:multi_noise".into())),
    ]);
    large
}

/// Vanilla `lambda$makeRule$7`: only eligible large-biomes sources change the
/// exact settings string. Custom compound settings remain untouched.
fn update_noise_settings(generator: &mut Tag) {
    if let Some(Tag::String(value)) = nbt::get_mut(generator, "settings")
        && value == "minecraft:overworld"
    {
        *value = "minecraft:large_biomes".into();
    }
}

/// Vanilla `lambda$makeRule$8` + `updateLayers`: prepend
/// `{height: 64, block: minecraft:air}` to the flat `layers` list
/// (`Stream.concat(Stream.of(new), existing)`). A missing `settings` block
/// means there are no layers to extend.
fn update_flat_layers(
    generator: &mut Tag,
    version: i32,
    fix: &'static str,
) -> Result<(), UpgradeError> {
    let Some(settings) = nbt::get_mut(generator, "settings") else {
        return Ok(());
    };
    if nbt::fields(settings).is_none() {
        return Err(UpgradeError::unsupported(
            version,
            fix,
            "WorldGenSettings.dimensions[minecraft:overworld].generator.settings",
            "affected field is not a compound",
        ));
    }
    let Some(layers) = nbt::get_mut(settings, "layers").and_then(nbt::list_mut) else {
        return Ok(());
    };
    layers.insert(
        0,
        nbt::compound(vec![
            ("height", Tag::Int(64)),
            ("block", Tag::String("minecraft:air".into())),
        ]),
    );
    Ok(())
}

fn require_dimension_types(
    tag: &mut Tag,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    const FIX: &str = "WorldGenSettingsDisallowOldCustomWorldsFix";
    let Some(settings) = worldgen(tag, version, FIX)? else {
        return Ok(());
    };
    let Some(dimensions) = nbt::get(settings, "dimensions") else {
        return Ok(());
    };
    if nbt::fields(dimensions).is_none() {
        return Err(UpgradeError::unsupported(
            version,
            FIX,
            "WorldGenSettings.dimensions",
            "affected field is not a compound",
        ));
    }
    // Vanilla `lambda$makeRule$4`: iterates the dimensions MAP VALUES and
    // reads the dimension entry's direct `type` field
    // (`entry.get("type").asString().result().isEmpty()` → throw
    // `Unable load old custom worlds.`). A missing `generator` block is
    // irrelevant to this fixer.
    for (name, entry) in nbt::fields(dimensions).unwrap_or(&[]) {
        let has_type = nbt::get(entry, "type").and_then(nbt::string).is_some();
        if !has_type {
            return Err(UpgradeError::unsupported(
                version,
                FIX,
                format!("WorldGenSettings.dimensions[{name}].type"),
                "Unable load old custom worlds.",
            ));
        }
    }
    report.applied(version, FIX);
    Ok(())
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
    fn level(generator: Tag, flag: Option<i8>) -> Tag {
        let mut settings = nbt::compound(vec![(
            "dimensions",
            nbt::compound(vec![(
                "minecraft:overworld",
                nbt::compound(vec![
                    ("type", text("minecraft:overworld")),
                    ("generator", generator),
                ]),
            )]),
        )]);
        if let Some(flag) = flag {
            nbt::set(
                &mut settings,
                "has_increased_height_already",
                Tag::Byte(flag),
            );
        }
        nbt::compound(vec![("WorldGenSettings", settings)])
    }
    fn generator<'a>(tag: &'a Tag, dimension: &str) -> &'a Tag {
        nbt::get(
            nbt::get(
                nbt::get(nbt::get(tag, "WorldGenSettings").unwrap(), "dimensions").unwrap(),
                dimension,
            )
            .unwrap(),
            "generator",
        )
        .unwrap()
    }
    fn run(tag: &mut Tag, version: i32) {
        fix_node(tag, NodeKind::Level, version, &mut UpgradeReport::default()).unwrap();
    }

    #[test]
    fn noise_sources_and_large_biomes_use_independent_flags() {
        for flag in [None, Some(0), Some(1)] {
            for source_type in [
                "minecraft:vanilla_layered",
                "minecraft:multi_noise",
                "mod:custom",
            ] {
                for large in [0, 1] {
                    let original = nbt::compound(vec![
                        ("type", text(source_type)),
                        ("large_biomes", Tag::Byte(large)),
                        ("seed", Tag::Long(42)),
                    ]);
                    let mut tag = level(
                        nbt::compound(vec![
                            ("type", text("minecraft:noise")),
                            ("biome_source", original.clone()),
                            ("settings", text("minecraft:overworld")),
                        ]),
                        flag,
                    );
                    run(&mut tag, 2832);
                    let eligible = source_type == "minecraft:vanilla_layered"
                        || (flag.is_none() && source_type == "minecraft:multi_noise");
                    let expected = if eligible {
                        nbt::compound(vec![
                            ("preset", text("minecraft:overworld")),
                            ("type", text("minecraft:multi_noise")),
                        ])
                    } else {
                        original
                    };
                    let output = generator(&tag, "minecraft:overworld");
                    assert_eq!(
                        nbt::get(output, "biome_source"),
                        Some(&expected),
                        "{flag:?} {source_type} {large}"
                    );
                    assert_eq!(
                        nbt::get(output, "settings"),
                        Some(&text(if eligible && large != 0 {
                            "minecraft:large_biomes"
                        } else {
                            "minecraft:overworld"
                        })),
                        "{flag:?} {source_type} {large}"
                    );
                    assert!(
                        nbt::get(
                            nbt::get(&tag, "WorldGenSettings").unwrap(),
                            "has_increased_height_already"
                        )
                        .is_none()
                    );
                }
            }
        }
    }

    #[test]
    fn custom_noise_settings_and_other_dimensions_are_preserved() {
        let settings = nbt::compound(vec![("sea_level", Tag::Int(62))]);
        let input = nbt::compound(vec![
            ("type", text("minecraft:noise")),
            (
                "biome_source",
                nbt::compound(vec![
                    ("type", text("minecraft:vanilla_layered")),
                    ("large_biomes", Tag::Byte(1)),
                ]),
            ),
            ("settings", settings.clone()),
        ]);
        let mut tag = level(input.clone(), Some(0));
        let dimensions = nbt::get_mut(
            nbt::get_mut(&mut tag, "WorldGenSettings").unwrap(),
            "dimensions",
        )
        .unwrap();
        let nether = nbt::compound(vec![("generator", input)]);
        nbt::set(dimensions, "minecraft:the_nether", nether.clone());
        run(&mut tag, 2832);
        assert_eq!(
            nbt::get(generator(&tag, "minecraft:overworld"), "settings"),
            Some(&settings)
        );
        let dimensions =
            nbt::get(nbt::get(&tag, "WorldGenSettings").unwrap(), "dimensions").unwrap();
        assert_eq!(nbt::get(dimensions, "minecraft:the_nether"), Some(&nether));
    }

    #[test]
    fn flat_padding_requires_explicit_false_increased_flag() {
        let layer = nbt::compound(vec![
            ("height", Tag::Int(1)),
            ("block", text("minecraft:bedrock")),
        ]);
        for flag in [None, Some(0), Some(1)] {
            let mut tag = level(
                nbt::compound(vec![
                    ("type", text("minecraft:flat")),
                    (
                        "settings",
                        nbt::compound(vec![("layers", list(vec![layer.clone()]))]),
                    ),
                ]),
                flag,
            );
            run(&mut tag, 2832);
            let settings = nbt::get(generator(&tag, "minecraft:overworld"), "settings").unwrap();
            let layers = nbt::list(nbt::get(settings, "layers").unwrap()).unwrap();
            assert_eq!(layers.len(), if flag == Some(0) { 2 } else { 1 });
            assert_eq!(layers.last(), Some(&layer));
            if flag == Some(0) {
                assert_eq!(
                    layers[0],
                    nbt::compound(vec![
                        ("height", Tag::Int(64)),
                        ("block", text("minecraft:air")),
                    ])
                );
            }
        }
    }

    #[test]
    fn disallow_checks_dimension_type_not_generator_type() {
        for version in [2833, 2852] {
            let mut valid = level(nbt::compound(vec![]), None);
            run(&mut valid, version);
            let dimensions = nbt::get_mut(
                nbt::get_mut(&mut valid, "WorldGenSettings").unwrap(),
                "dimensions",
            )
            .unwrap();
            let entry = nbt::get_mut(dimensions, "minecraft:overworld").unwrap();
            nbt::remove(entry, "type");
            nbt::set(
                entry,
                "generator",
                nbt::compound(vec![("type", text("minecraft:noise"))]),
            );
            assert!(matches!(
                fix_node(
                    &mut valid,
                    NodeKind::Level,
                    version,
                    &mut UpgradeReport::default()
                ),
                Err(UpgradeError::UnsupportedField { .. }),
            ));
        }
    }

    #[test]
    fn height_flags_insert_defaults_without_overwriting() {
        for (version, default) in [(2693, 0), (2707, 1), (2825, 0)] {
            let mut tag = level(nbt::compound(vec![]), None);
            run(&mut tag, version);
            assert_eq!(
                nbt::get(
                    nbt::get(&tag, "WorldGenSettings").unwrap(),
                    "has_increased_height_already"
                ),
                Some(&Tag::Byte(default))
            );
            let mut tag = level(nbt::compound(vec![]), Some(1 - default));
            let original = tag.clone();
            run(&mut tag, version);
            assert_eq!(tag, original);
        }
    }
}
