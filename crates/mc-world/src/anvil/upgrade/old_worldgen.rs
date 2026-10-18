//! Pre-1.16 level.dat world-generation split: `LevelLegacyWorldGenSettingsFix`
//! (2538) and `WorldGenSettingsFix` (2550).
//!
//! Bytecode authority: official 26.1.2 `server-26.1.2.jar`
//! (`net/minecraft/util/datafix/fixes/{LevelLegacyWorldGenSettingsFix,WorldGenSettingsFix}`
//! plus `WorldGenSettingsFix$StructureFeatureConfiguration`).
//! Applies to the `Level` node only. 2538 moves the flat level keys into
//! `WorldGenSettings`; 2550 rebuilds `WorldGenSettings` as `{seed,
//! generate_features, bonus_chest, dimensions, [legacy_custom_options]}` and
//! removes the consumed legacy keys. Unrecognized `WorldGenSettings` keys are
//! preserved (vanilla drops them; owner rule).
//!
//! Deliberate difference: vanilla treats any unrecognized `generatorName` as
//! the default overworld; here it is an explicit error naming the field.
use super::{NodeKind, UpgradeError, UpgradeReport, nbt};
use mc_nbt::{ListTag, Tag, tag_type};

const LEGACY_FIX: &str = "LevelLegacyWorldGenSettingsFix";
const FIX: &str = "WorldGenSettingsFix";
const SETTINGS: &str = "WorldGenSettings";
const OLD_KEYS: [&str; 7] = [
    "RandomSeed",
    "generatorName",
    "generatorOptions",
    "generatorVersion",
    "legacy_custom_options",
    "MapFeatures",
    "BonusChest",
];

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
        2538 => move_legacy_keys(tag, report),
        2550 => build_settings(tag, report),
        _ => Ok(()),
    }
}

fn not_compound(version: i32, fix: &'static str) -> UpgradeError {
    UpgradeError::unsupported(version, fix, SETTINGS, "affected field is not a compound")
}

/// 2538: `WorldGenSettings` (empty map if absent) receives every present old key.
fn move_legacy_keys(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    let mut settings = match nbt::take(tag, SETTINGS) {
        Some(existing) if nbt::fields(&existing).is_none() => {
            return Err(not_compound(2538, LEGACY_FIX));
        }
        Some(existing) => existing,
        None => nbt::compound(vec![]),
    };
    for key in OLD_KEYS {
        if let Some(value) = nbt::take(tag, key) {
            nbt::set(&mut settings, key, value);
        }
    }
    nbt::set(tag, SETTINGS, settings);
    report.applied(2538, LEGACY_FIX);
    Ok(())
}

/// 2550: replace `WorldGenSettings` with the dimension-based form.
fn build_settings(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    let Some(old) = nbt::get(tag, SETTINGS) else {
        return Ok(());
    };
    if nbt::fields(old).is_none() {
        return Err(not_compound(2550, FIX));
    }
    let rebuilt = rebuild(old)?;
    nbt::set(tag, SETTINGS, rebuilt);
    report.applied(2550, FIX);
    Ok(())
}

fn text(value: &str) -> Tag {
    Tag::String(value.into())
}

fn flag(value: bool) -> Tag {
    Tag::Byte(i8::from(value))
}

/// `asLong(default)`: numeric tags only; floating values truncate like Java.
fn long_or(tag: Option<&Tag>, default: i64) -> i64 {
    match tag {
        Some(Tag::Byte(v)) => i64::from(*v),
        Some(Tag::Short(v)) => i64::from(*v),
        Some(Tag::Int(v)) => i64::from(*v),
        Some(Tag::Long(v)) => *v,
        Some(Tag::Float(v)) => *v as i64,
        Some(Tag::Double(v)) => *v as i64,
        _ => default,
    }
}

/// `asBoolean(default)`: numeric tags compare `intValue() != 0`.
fn bool_or(tag: Option<&Tag>, default: bool) -> bool {
    match tag {
        Some(Tag::Byte(v)) => *v != 0,
        Some(Tag::Short(v)) => *v != 0,
        Some(Tag::Int(v)) => *v != 0,
        Some(Tag::Long(v)) => *v as i32 != 0,
        Some(Tag::Float(v)) => *v as i32 != 0,
        Some(Tag::Double(v)) => *v as i32 != 0,
        _ => default,
    }
}

fn int_or(tag: Option<&Tag>, default: i32) -> i32 {
    match tag {
        Some(Tag::Byte(v)) => i32::from(*v),
        Some(Tag::Short(v)) => i32::from(*v),
        Some(Tag::Int(v)) => *v,
        Some(Tag::Long(v)) => *v as i32,
        Some(Tag::Float(v)) => *v as i32,
        Some(Tag::Double(v)) => *v as i32,
        _ => default,
    }
}

fn rebuild(old: &Tag) -> Result<Tag, UpgradeError> {
    let seed = long_or(nbt::get(old, "RandomSeed"), 0);
    let name = match nbt::get(old, "generatorName") {
        None => None,
        Some(Tag::String(name)) => Some(name.to_lowercase()),
        Some(_) => {
            return Err(UpgradeError::unsupported(
                2550,
                FIX,
                "WorldGenSettings.generatorName",
                "affected field is not a string",
            ));
        }
    };
    let options = nbt::get(old, "generatorOptions");
    let legacy_options = match nbt::get(old, "legacy_custom_options").and_then(nbt::string) {
        Some(existing) => Some(existing.to_owned()),
        None if name.as_deref() == Some("customized") => {
            options.and_then(nbt::string).map(str::to_owned)
        }
        None => None,
    };
    let mut caves = false;
    let generator = match name.as_deref() {
        None | Some("customized") => default_overworld(seed),
        Some("flat") => flat(options),
        Some("debug_all_block_states") => nbt::compound(vec![("type", text("minecraft:debug"))]),
        Some("buffet") => buffet(seed, options, &mut caves),
        Some(name @ ("default" | "default_1_1" | "largebiomes" | "amplified")) => {
            let legacy_init = name == "default_1_1"
                || (name == "default" && int_or(nbt::get(old, "generatorVersion"), 0) == 0);
            let settings = if name == "amplified" {
                "minecraft:amplified"
            } else {
                "minecraft:overworld"
            };
            noise(
                seed,
                settings,
                vanilla_biome_source(seed, legacy_init, name == "largebiomes"),
            )
        }
        Some(other) => {
            return Err(UpgradeError::unsupported(
                2550,
                FIX,
                "WorldGenSettings.generatorName",
                format!("unknown generator type `{other}`"),
            ));
        }
    };
    let mut rebuilt = vec![
        ("seed", Tag::Long(seed)),
        (
            "generate_features",
            flag(bool_or(nbt::get(old, "MapFeatures"), true)),
        ),
        (
            "bonus_chest",
            flag(bool_or(nbt::get(old, "BonusChest"), false)),
        ),
        ("dimensions", vanilla_levels(seed, generator, caves)),
    ];
    if let Some(options) = legacy_options {
        rebuilt.push(("legacy_custom_options", Tag::String(options)));
    }
    // Owner rule: unrecognized WorldGenSettings keys survive (vanilla drops
    // them); only the legacy keys consumed above are removed.
    let mut result = old.clone();
    for key in OLD_KEYS {
        nbt::remove(&mut result, key);
    }
    for (key, value) in rebuilt {
        nbt::set(&mut result, key, value);
    }
    Ok(result)
}

fn vanilla_biome_source(seed: i64, legacy_init: bool, large_biomes: bool) -> Tag {
    let mut fields = vec![
        ("type", text("minecraft:vanilla_layered")),
        ("seed", Tag::Long(seed)),
        ("large_biomes", flag(large_biomes)),
    ];
    if legacy_init {
        fields.push(("legacy_biome_init_layer", flag(true)));
    }
    nbt::compound(fields)
}

fn noise(seed: i64, settings: &str, biome_source: Tag) -> Tag {
    nbt::compound(vec![
        ("type", text("minecraft:noise")),
        ("biome_source", biome_source),
        ("seed", Tag::Long(seed)),
        ("settings", text(settings)),
    ])
}

fn default_overworld(seed: i64) -> Tag {
    noise(
        seed,
        "minecraft:overworld",
        vanilla_biome_source(seed, false, false),
    )
}

fn vanilla_levels(seed: i64, overworld: Tag, caves: bool) -> Tag {
    let overworld_type = if caves {
        "minecraft:overworld_caves"
    } else {
        "minecraft:overworld"
    };
    nbt::compound(vec![
        (
            "minecraft:overworld",
            nbt::compound(vec![
                ("type", text(overworld_type)),
                ("generator", overworld),
            ]),
        ),
        (
            "minecraft:the_nether",
            nbt::compound(vec![
                ("type", text("minecraft:the_nether")),
                (
                    "generator",
                    noise(
                        seed,
                        "minecraft:nether",
                        nbt::compound(vec![
                            ("type", text("minecraft:multi_noise")),
                            ("seed", Tag::Long(seed)),
                            ("preset", text("minecraft:nether")),
                        ]),
                    ),
                ),
            ]),
        ),
        (
            "minecraft:the_end",
            nbt::compound(vec![
                ("type", text("minecraft:the_end")),
                (
                    "generator",
                    noise(
                        seed,
                        "minecraft:end",
                        nbt::compound(vec![
                            ("type", text("minecraft:the_end")),
                            ("seed", Tag::Long(seed)),
                        ]),
                    ),
                ),
            ]),
        ),
    ])
}

/// `buffet`: chunk generator type picks settings/caves; `fixed` biome sources
/// collapse `options.biomes[0]` into `biome`.
fn buffet(seed: i64, options: Option<&Tag>, caves: &mut bool) -> Tag {
    let chunk_type = options
        .and_then(|options| nbt::get(options, "chunk_generator"))
        .and_then(|generator| nbt::get(generator, "type"))
        .and_then(nbt::string);
    let settings = match chunk_type {
        Some("minecraft:caves") => {
            *caves = true;
            "minecraft:caves"
        }
        Some("minecraft:floating_islands") => "minecraft:floating_islands",
        _ => "minecraft:overworld",
    };
    let mut biome_source = options
        .and_then(|options| nbt::get(options, "biome_source"))
        .cloned()
        .unwrap_or_else(|| nbt::compound(vec![("type", text("minecraft:fixed"))]));
    if nbt::get(&biome_source, "type").and_then(nbt::string) == Some("minecraft:fixed") {
        let biome = nbt::get(&biome_source, "options")
            .and_then(|options| nbt::get(options, "biomes"))
            .and_then(nbt::list)
            .and_then(<[Tag]>::first)
            .and_then(nbt::string)
            .unwrap_or("minecraft:ocean")
            .to_owned();
        nbt::remove(&mut biome_source, "options");
        nbt::set(&mut biome_source, "biome", Tag::String(biome));
    }
    noise(seed, settings, biome_source)
}

#[derive(Clone, Copy)]
struct Spacing {
    spacing: i32,
    separation: i32,
    salt: i32,
}

const DEFAULTS: [(&str, Spacing); 8] = [
    (
        "minecraft:village",
        Spacing {
            spacing: 32,
            separation: 8,
            salt: 10_387_312,
        },
    ),
    (
        "minecraft:desert_pyramid",
        Spacing {
            spacing: 32,
            separation: 8,
            salt: 14_357_617,
        },
    ),
    (
        "minecraft:igloo",
        Spacing {
            spacing: 32,
            separation: 8,
            salt: 14_357_618,
        },
    ),
    (
        "minecraft:jungle_pyramid",
        Spacing {
            spacing: 32,
            separation: 8,
            salt: 14_357_619,
        },
    ),
    (
        "minecraft:swamp_hut",
        Spacing {
            spacing: 32,
            separation: 8,
            salt: 14_357_620,
        },
    ),
    (
        "minecraft:pillager_outpost",
        Spacing {
            spacing: 32,
            separation: 8,
            salt: 165_745_296,
        },
    ),
    (
        "minecraft:monument",
        Spacing {
            spacing: 32,
            separation: 5,
            salt: 10_387_313,
        },
    ),
    (
        "minecraft:endcity",
        Spacing {
            spacing: 20,
            separation: 11,
            salt: 10_387_313,
        },
    ),
];
const MANSION: Spacing = Spacing {
    spacing: 80,
    separation: 20,
    salt: 10_387_319,
};

fn default_spacing(key: &str) -> Spacing {
    if key == "minecraft:mansion" {
        return MANSION;
    }
    DEFAULTS
        .iter()
        .find(|(name, _)| *name == key)
        .expect("vanilla structure default")
        .1
}

/// Insertion-ordered map of the structure spacings the fix has touched.
#[derive(Default)]
struct Structures(Vec<(&'static str, Spacing)>);

impl Structures {
    fn get(&self, key: &'static str) -> Spacing {
        self.0
            .iter()
            .find(|(name, _)| *name == key)
            .map_or_else(|| default_spacing(key), |(_, value)| *value)
    }

    fn put(&mut self, key: &'static str, value: Spacing) {
        match self.0.iter_mut().find(|(name, _)| *name == key) {
            Some((_, existing)) => *existing = value,
            None => self.0.push((key, value)),
        }
    }

    /// `setSpacing`: spacing = max(min, toInt(value, default spacing)).
    fn set_spacing(&mut self, key: &'static str, value: &str, min: i32) {
        let current = self.get(key);
        self.put(
            key,
            Spacing {
                spacing: to_int(value, current.spacing).max(min),
                ..current
            },
        );
    }
}

/// `NumberUtils.toInt(str, default)`: parse failure yields the default.
fn to_int(value: &str, default: i32) -> i32 {
    value.parse().unwrap_or(default)
}

/// `flat`: `generatorOptions` is only read when it is a compound. A legacy
/// string (or any other tag) yields no structures, default layers and plains,
/// because vanilla's `Dynamic.get` on a non-map is empty; a missing field
/// yields the default village + stronghold.
fn flat(options: Option<&Tag>) -> Tag {
    let (mut distance, mut spread, mut count) = (32, 3, 128);
    let mut stronghold = false;
    let mut structures = Structures::default();
    if options.is_none() {
        stronghold = true;
        structures.put("minecraft:village", default_spacing("minecraft:village"));
    }
    if let Some(configs) = options
        .and_then(|options| nbt::get(options, "structures"))
        .and_then(nbt::fields)
    {
        for (name, config) in configs {
            let Some(config) = nbt::fields(config) else {
                continue;
            };
            for (key, value) in config {
                let value = nbt::string(value).unwrap_or("");
                if name == "stronghold" {
                    stronghold = true;
                    match key.as_str() {
                        "distance" => distance = to_int(value, distance).max(1),
                        "spread" => spread = to_int(value, spread).max(1),
                        "count" => count = to_int(value, count).max(1),
                        _ => {}
                    }
                } else {
                    structure_option(&mut structures, name, key, value);
                }
            }
        }
    }
    let mut serialized: Vec<(&str, Tag)> = structures
        .0
        .iter()
        .map(|(name, value)| {
            (
                *name,
                nbt::compound(vec![
                    ("spacing", Tag::Int(value.spacing)),
                    ("separation", Tag::Int(value.separation)),
                    ("salt", Tag::Int(value.salt)),
                ]),
            )
        })
        .collect();
    if stronghold {
        serialized.push((
            "stronghold",
            nbt::compound(vec![
                ("distance", Tag::Int(distance)),
                ("spread", Tag::Int(spread)),
                ("count", Tag::Int(count)),
            ]),
        ));
    }
    let layers = options
        .and_then(|options| nbt::get(options, "layers"))
        .cloned()
        .unwrap_or_else(default_layers);
    let biome = options
        .and_then(|options| nbt::get(options, "biome"))
        .and_then(nbt::string)
        .unwrap_or("minecraft:plains");
    nbt::compound(vec![
        ("type", text("minecraft:flat")),
        (
            "settings",
            nbt::compound(vec![
                ("structures", nbt::compound(serialized)),
                ("layers", layers),
                ("biome", text(biome)),
            ]),
        ),
    ])
}

/// One non-stronghold `structures.<name>.<key> = value` entry.
fn structure_option(structures: &mut Structures, name: &str, key: &str, value: &str) {
    match key {
        "distance" => match name {
            "village" => structures.set_spacing("minecraft:village", value, 9),
            "biome_1" => {
                for key in [
                    "minecraft:desert_pyramid",
                    "minecraft:igloo",
                    "minecraft:jungle_pyramid",
                    "minecraft:swamp_hut",
                    "minecraft:pillager_outpost",
                ] {
                    structures.set_spacing(key, value, 9);
                }
            }
            "endcity" => structures.set_spacing("minecraft:endcity", value, 1),
            "mansion" => structures.set_spacing("minecraft:mansion", value, 1),
            _ => {}
        },
        // Vanilla quirk, reproduced as in the bytecode: the monument's
        // `spacing` is overwritten from the parsed `separation` value.
        "separation" if name == "oceanmonument" => {
            let current = structures.get("minecraft:monument");
            structures.put(
                "minecraft:monument",
                Spacing {
                    spacing: to_int(value, current.separation).max(1),
                    ..current
                },
            );
        }
        "spacing" if name == "oceanmonument" => {
            structures.set_spacing("minecraft:monument", value, 1);
        }
        _ => {}
    }
}

fn default_layers() -> Tag {
    let layer = |height: i32, block: &str| {
        nbt::compound(vec![("height", Tag::Int(height)), ("block", text(block))])
    };
    Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements: vec![
            layer(1, "minecraft:bedrock"),
            layer(2, "minecraft:dirt"),
            layer(1, "minecraft:grass_block"),
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(level: &mut Tag, version: i32) -> Result<UpgradeReport, UpgradeError> {
        let mut report = UpgradeReport::default();
        fix_node(level, NodeKind::Level, version, &mut report).map(|()| report)
    }

    fn legacy_level(name: &str, extra: Vec<(&str, Tag)>) -> Tag {
        let mut fields = vec![
            ("generatorName", text(name)),
            ("RandomSeed", Tag::Long(-4_172_144_997_902_289_642)),
            ("MapFeatures", Tag::Byte(0)),
            ("BonusChest", Tag::Byte(1)),
            ("Other", Tag::Int(7)),
        ];
        fields.extend(extra);
        nbt::compound(fields)
    }

    /// Level after the full 2538 + 2550 chain.
    fn upgraded(mut level: Tag) -> Result<Tag, UpgradeError> {
        run(&mut level, 2538)?;
        run(&mut level, 2550)?;
        Ok(level)
    }

    fn at<'a>(tag: &'a Tag, path: &[&str]) -> &'a Tag {
        path.iter().fold(tag, |tag, key| {
            nbt::get(tag, key).unwrap_or_else(|| panic!("missing {key} in {path:?}"))
        })
    }

    fn overworld_generator(level: &Tag) -> &Tag {
        at(
            level,
            &[SETTINGS, "dimensions", "minecraft:overworld", "generator"],
        )
    }

    fn string_at<'a>(tag: &'a Tag, path: &[&str]) -> &'a str {
        nbt::string(at(tag, path)).unwrap_or_else(|| panic!("{path:?} not a string"))
    }

    const SEED: i64 = -4_172_144_997_902_289_642;

    fn assert_common(level: &Tag, caves: bool) {
        assert_eq!(*at(level, &[SETTINGS, "seed"]), Tag::Long(SEED));
        assert_eq!(*at(level, &[SETTINGS, "generate_features"]), Tag::Byte(0));
        assert_eq!(*at(level, &[SETTINGS, "bonus_chest"]), Tag::Byte(1));
        for gone in OLD_KEYS {
            assert!(nbt::get(level, gone).is_none(), "{gone} stayed on Data");
            if gone != "legacy_custom_options" {
                assert!(nbt::get(at(level, &[SETTINGS]), gone).is_none());
            }
        }
        assert_eq!(*at(level, &["Other"]), Tag::Int(7));
        let dims = at(level, &[SETTINGS, "dimensions"]);
        assert_eq!(
            string_at(dims, &["minecraft:overworld", "type"]),
            if caves {
                "minecraft:overworld_caves"
            } else {
                "minecraft:overworld"
            }
        );
        assert_eq!(
            string_at(dims, &["minecraft:the_nether", "type"]),
            "minecraft:the_nether"
        );
        let nether = at(dims, &["minecraft:the_nether", "generator"]);
        assert_eq!(string_at(nether, &["settings"]), "minecraft:nether");
        assert_eq!(
            string_at(nether, &["biome_source", "preset"]),
            "minecraft:nether"
        );
        assert_eq!(*at(nether, &["biome_source", "seed"]), Tag::Long(SEED));
        let end = at(dims, &["minecraft:the_end", "generator"]);
        assert_eq!(string_at(end, &["settings"]), "minecraft:end");
        assert_eq!(
            string_at(end, &["biome_source", "type"]),
            "minecraft:the_end"
        );
    }

    fn layered(level: &Tag) -> &Tag {
        at(overworld_generator(level), &["biome_source"])
    }

    #[test]
    fn default_generator_with_version_one_has_no_legacy_layer() {
        let level = upgraded(legacy_level(
            "default",
            vec![("generatorVersion", Tag::Int(1))],
        ))
        .unwrap();
        assert_common(&level, false);
        let generator = overworld_generator(&level);
        assert_eq!(string_at(generator, &["type"]), "minecraft:noise");
        assert_eq!(string_at(generator, &["settings"]), "minecraft:overworld");
        let source = layered(&level);
        assert_eq!(string_at(source, &["type"]), "minecraft:vanilla_layered");
        assert_eq!(*at(source, &["large_biomes"]), Tag::Byte(0));
        assert!(nbt::get(source, "legacy_biome_init_layer").is_none());
    }

    #[test]
    fn default_generator_without_version_and_default_1_1_set_legacy_layer() {
        for (name, version) in [("default", None), ("default_1_1", Some(1))] {
            let extra = version
                .map(|version| vec![("generatorVersion", Tag::Int(version))])
                .unwrap_or_default();
            let level = upgraded(legacy_level(name, extra)).unwrap();
            assert_eq!(
                *at(layered(&level), &["legacy_biome_init_layer"]),
                Tag::Byte(1),
                "{name}"
            );
        }
    }

    #[test]
    fn large_biomes_generator_is_case_insensitive() {
        let level = upgraded(legacy_level("largeBiomes", vec![])).unwrap();
        assert_common(&level, false);
        assert_eq!(*at(layered(&level), &["large_biomes"]), Tag::Byte(1));
        assert_eq!(
            string_at(overworld_generator(&level), &["settings"]),
            "minecraft:overworld"
        );
    }

    #[test]
    fn amplified_generator_uses_amplified_settings() {
        let level = upgraded(legacy_level("amplified", vec![])).unwrap();
        assert_common(&level, false);
        assert_eq!(
            string_at(overworld_generator(&level), &["settings"]),
            "minecraft:amplified"
        );
        assert_eq!(*at(layered(&level), &["large_biomes"]), Tag::Byte(0));
    }

    fn structure(spacing: i32, separation: i32, salt: i32) -> Tag {
        nbt::compound(vec![
            ("spacing", Tag::Int(spacing)),
            ("separation", Tag::Int(separation)),
            ("salt", Tag::Int(salt)),
        ])
    }

    #[test]
    fn flat_generator_with_nbt_options_keeps_layers_biome_and_structures() {
        let layers = Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: vec![nbt::compound(vec![
                ("height", Tag::Int(3)),
                ("block", text("minecraft:stone")),
            ])],
        });
        let options = nbt::compound(vec![
            ("biome", text("minecraft:desert")),
            ("layers", layers.clone()),
            (
                "structures",
                nbt::compound(vec![
                    ("village", nbt::compound(vec![("distance", text("20"))])),
                    ("biome_1", nbt::compound(vec![("distance", text("4"))])),
                    ("mansion", nbt::compound(vec![("distance", text("0"))])),
                    (
                        "oceanmonument",
                        nbt::compound(vec![("spacing", text("12")), ("separation", text("x"))]),
                    ),
                    (
                        "stronghold",
                        nbt::compound(vec![("distance", text("0")), ("count", text("5"))]),
                    ),
                ]),
            ),
        ]);
        let level = upgraded(legacy_level("flat", vec![("generatorOptions", options)])).unwrap();
        assert_common(&level, false);
        let generator = overworld_generator(&level);
        assert_eq!(string_at(generator, &["type"]), "minecraft:flat");
        let settings = at(generator, &["settings"]);
        assert_eq!(string_at(settings, &["biome"]), "minecraft:desert");
        assert_eq!(*at(settings, &["layers"]), layers);
        let structures = at(settings, &["structures"]);
        assert_eq!(
            *at(structures, &["minecraft:village"]),
            structure(20, 8, 10_387_312)
        );
        // min 9 for temples/outposts; each starts from its own default
        assert_eq!(
            *at(structures, &["minecraft:igloo"]),
            structure(9, 8, 14_357_618)
        );
        assert_eq!(
            *at(structures, &["minecraft:pillager_outpost"]),
            structure(9, 8, 165_745_296)
        );
        assert_eq!(
            *at(structures, &["minecraft:mansion"]),
            structure(1, 20, 10_387_319)
        );
        // `spacing` 12 sets spacing; unparsable `separation` falls back to the default
        // separation 5 and (vanilla quirk) overwrites spacing with it afterwards.
        assert_eq!(
            *at(structures, &["minecraft:monument"]),
            structure(5, 5, 10_387_313)
        );
        assert_eq!(
            *at(structures, &["stronghold"]),
            nbt::compound(vec![
                ("distance", Tag::Int(1)),
                ("spread", Tag::Int(3)),
                ("count", Tag::Int(5)),
            ])
        );
    }

    #[test]
    fn flat_generator_with_legacy_string_options_gets_vanilla_defaults() {
        let level = upgraded(legacy_level(
            "flat",
            vec![(
                "generatorOptions",
                text("3;minecraft:bedrock,2*minecraft:dirt,minecraft:grass_block;1;village"),
            )],
        ))
        .unwrap();
        let settings = at(overworld_generator(&level), &["settings"]);
        assert_eq!(string_at(settings, &["biome"]), "minecraft:plains");
        assert_eq!(*at(settings, &["layers"]), default_layers());
        // options present but not a compound: no structures, no stronghold
        assert_eq!(*at(settings, &["structures"]), nbt::compound(vec![]));
    }

    #[test]
    fn flat_generator_without_options_defaults_village_and_stronghold() {
        let level = upgraded(legacy_level("flat", vec![])).unwrap();
        let structures = at(overworld_generator(&level), &["settings", "structures"]);
        assert_eq!(
            *at(structures, &["minecraft:village"]),
            structure(32, 8, 10_387_312)
        );
        assert_eq!(
            *at(structures, &["stronghold"]),
            nbt::compound(vec![
                ("distance", Tag::Int(32)),
                ("spread", Tag::Int(3)),
                ("count", Tag::Int(128)),
            ])
        );
    }

    #[test]
    fn buffet_single_biome_collapses_options_into_biome() {
        let options = nbt::compound(vec![
            (
                "biome_source",
                nbt::compound(vec![
                    ("type", text("minecraft:fixed")),
                    (
                        "options",
                        nbt::compound(vec![(
                            "biomes",
                            Tag::List(ListTag {
                                element_type: tag_type::STRING,
                                elements: vec![text("minecraft:jungle")],
                            }),
                        )]),
                    ),
                ]),
            ),
            (
                "chunk_generator",
                nbt::compound(vec![("type", text("minecraft:floating_islands"))]),
            ),
        ]);
        let level = upgraded(legacy_level("buffet", vec![("generatorOptions", options)])).unwrap();
        assert_common(&level, false);
        let generator = overworld_generator(&level);
        assert_eq!(
            string_at(generator, &["settings"]),
            "minecraft:floating_islands"
        );
        let source = at(generator, &["biome_source"]);
        assert_eq!(string_at(source, &["biome"]), "minecraft:jungle");
        assert!(nbt::get(source, "options").is_none());
    }

    #[test]
    fn buffet_caves_switches_dimension_type_and_keeps_other_sources() {
        let options = nbt::compound(vec![
            (
                "biome_source",
                nbt::compound(vec![("type", text("minecraft:checkerboard"))]),
            ),
            (
                "chunk_generator",
                nbt::compound(vec![("type", text("minecraft:caves"))]),
            ),
        ]);
        let level = upgraded(legacy_level("buffet", vec![("generatorOptions", options)])).unwrap();
        assert_common(&level, true);
        let generator = overworld_generator(&level);
        assert_eq!(string_at(generator, &["settings"]), "minecraft:caves");
        assert_eq!(
            string_at(generator, &["biome_source", "type"]),
            "minecraft:checkerboard"
        );
        // no options at all: fixed source defaults to the ocean
        let level = upgraded(legacy_level("buffet", vec![])).unwrap();
        let source = at(overworld_generator(&level), &["biome_source"]);
        assert_eq!(string_at(source, &["biome"]), "minecraft:ocean");
    }

    #[test]
    fn customized_generator_becomes_default_and_keeps_legacy_options() {
        let level = upgraded(legacy_level(
            "customized",
            vec![("generatorOptions", text("{\"coordinateScale\":684}"))],
        ))
        .unwrap();
        assert_common(&level, false);
        let source = layered(&level);
        assert!(nbt::get(source, "legacy_biome_init_layer").is_none());
        assert_eq!(
            string_at(&level, &[SETTINGS, "legacy_custom_options"]),
            "{\"coordinateScale\":684}"
        );
    }

    #[test]
    fn debug_generator_is_the_debug_chunk_generator() {
        let level = upgraded(legacy_level("debug_all_block_states", vec![])).unwrap();
        assert_common(&level, false);
        assert_eq!(
            *overworld_generator(&level),
            nbt::compound(vec![("type", text("minecraft:debug"))])
        );
    }

    #[test]
    fn missing_generator_name_and_absent_flags_use_vanilla_defaults() {
        let level = upgraded(nbt::compound(vec![])).unwrap();
        assert_eq!(*at(&level, &[SETTINGS, "seed"]), Tag::Long(0));
        assert_eq!(*at(&level, &[SETTINGS, "generate_features"]), Tag::Byte(1));
        assert_eq!(*at(&level, &[SETTINGS, "bonus_chest"]), Tag::Byte(0));
        assert_eq!(
            string_at(overworld_generator(&level), &["settings"]),
            "minecraft:overworld"
        );
    }

    #[test]
    fn unknown_generator_type_is_an_error_naming_the_field() {
        let error = upgraded(legacy_level("mystery", vec![])).unwrap_err();
        match error {
            UpgradeError::UnsupportedField {
                version,
                fix,
                path,
                reason,
            } => {
                assert_eq!((version, fix), (2550, FIX));
                assert_eq!(path, "WorldGenSettings.generatorName");
                assert!(reason.contains("mystery"), "{reason}");
            }
            other => panic!("unexpected error {other:?}"),
        }
        let mut level = legacy_level("default", vec![]);
        nbt::set(&mut level, "generatorName", Tag::Int(1));
        assert!(upgraded(level).is_err());
    }

    #[test]
    fn legacy_move_keeps_existing_settings_and_non_level_nodes_are_untouched() {
        let mut level = legacy_level("flat", vec![]);
        nbt::set(
            &mut level,
            SETTINGS,
            nbt::compound(vec![("kept", Tag::Int(1))]),
        );
        let report = run(&mut level, 2538).unwrap();
        assert_eq!(report.steps.len(), 1);
        assert_eq!(*at(&level, &[SETTINGS, "kept"]), Tag::Int(1));
        assert_eq!(string_at(&level, &[SETTINGS, "generatorName"]), "flat");
        assert!(nbt::get(&level, "generatorName").is_none());
        // the 2550 rebuild keeps unrecognized keys and consumes the legacy ones
        run(&mut level, 2550).unwrap();
        assert_eq!(*at(&level, &[SETTINGS, "kept"]), Tag::Int(1));
        assert!(nbt::get(at(&level, &[SETTINGS]), "generatorName").is_none());

        let mut entity = legacy_level("flat", vec![]);
        let before = entity.clone();
        let mut report = UpgradeReport::default();
        fix_node(&mut entity, NodeKind::Entity, 2538, &mut report).unwrap();
        assert_eq!(entity, before);
        // 2550 without WorldGenSettings is a no-op (vanilla Typed has no such field)
        let mut bare = nbt::compound(vec![]);
        assert!(run(&mut bare, 2550).unwrap().steps.is_empty());
    }
}
