use mc_nbt::{ListTag, Tag};

use super::nbt::{
    compound, fields, fields_mut, get, get_mut, list_mut, number, remove, rename, set, string, take,
};
use super::{NodeKind, SavedDataRecord, TARGET_DATA_VERSION, UpgradeError, UpgradeReport};

// Semantics are taken from the matching 26.1.2 javap disassemblies in
// .analysis/old-world-upgrade, including FileRelation and SavedDataNbt.
pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if kind == NodeKind::Player {
        if version == 4548
            && let Some(respawn) = get_mut(tag, "respawn")
        {
            require_map(respawn, version, "PlayerRespawnDataFix", "respawn")?;
            let dimension = get(respawn, "dimension")
                .and_then(string)
                .unwrap_or("minecraft:overworld")
                .to_owned();
            let yaw = num(respawn, "angle", 0.0) as f32;
            set(respawn, "dimension", Tag::String(dimension));
            set(respawn, "yaw", Tag::Float(yaw));
            set(respawn, "pitch", Tag::Float(0.0));
            remove(respawn, "angle");
            report.applied(version, "PlayerRespawnDataFix");
        }
        return Ok(());
    }
    if kind != NodeKind::Level {
        return Ok(());
    }
    match version {
        2967 => flatten_structures(tag, report)?,
        // StructuresBecomeConfiguredFix2970 is a CHUNK fix, not a level fix.
        3440 => {
            rename_biome_source(tag, report)?;
            remove_flag(tag, version, "minecraft:update_1_20", report)?;
        }
        3459 => legacy_dragon(tag, report)?,
        // RandomSequenceSettingsFix3565 only targets SAVED_DATA_RANDOM_SEQUENCES.
        // In particular, an opaque level field named random_sequences is not it.
        3939 => remove_flag(tag, version, "minecraft:update_1_21", report)?,
        4067 => remove_flag(tag, version, "minecraft:bundle", report)?,
        4180 => remove_flag(tag, version, "minecraft:winter_drop", report)?,
        4424 => remove_flag(tag, version, "minecraft:locator_bar", report)?,
        4544 => legacy_border(tag, report),
        4548 => world_spawn(tag, report),
        // WorldBorderWarningTimeFix4657 only targets SAVED_DATA_WORLD_BORDER.
        // SavedDataNbt.write in the4772 splitter does not apply that older fix.
        4658 => game_rules(tag, report)?,
        4766 => clock(tag, report),
        4771 => {
            difficulty(tag, report);
            prepare_saved_data(tag, report)?;
        }
        4772 => split_saved_data(tag, report)?,
        4773 => report.preserved("files/structures", "GeneratedStructuresRenameFileFix4773 requires external generated-structure file discovery; no filesystem changes are performed"),
        _ => {}
    }
    Ok(())
}

fn require_map(tag: &Tag, version: i32, fix: &'static str, path: &str) -> Result<(), UpgradeError> {
    if fields(tag).is_none() {
        return Err(UpgradeError::unsupported(
            version,
            fix,
            path,
            "affected field is not a compound",
        ));
    }
    Ok(())
}

fn num(tag: &Tag, key: &str, default: f64) -> f64 {
    get(tag, key).and_then(number).unwrap_or(default)
}

fn java_int(tag: &Tag) -> Option<i32> {
    match tag {
        Tag::Byte(v) => Some(i32::from(*v)),
        Tag::Short(v) => Some(i32::from(*v)),
        Tag::Int(v) => Some(*v),
        Tag::Long(v) => Some(*v as i32),
        Tag::Float(v) => Some(*v as i32),
        Tag::Double(v) => Some(*v as i32),
        _ => None,
    }
}

fn integer(tag: &Tag, key: &str, default: i32) -> i32 {
    get(tag, key).and_then(java_int).unwrap_or(default)
}

fn long(tag: &Tag, key: &str, default: i64) -> i64 {
    match get(tag, key) {
        Some(Tag::Long(v)) => *v,
        Some(Tag::Float(v)) => *v as i64,
        Some(Tag::Double(v)) => *v as i64,
        Some(v) => java_int(v).map_or(default, i64::from),
        None => default,
    }
}

fn boolean(tag: &Tag, key: &str) -> i8 {
    get(tag, key)
        .and_then(java_int)
        .map_or(0, |v| i8::from(v as i8 != 0))
}

fn empty_list() -> Tag {
    Tag::List(ListTag::empty())
}

fn generators(
    tag: &mut Tag,
    version: i32,
    fix: &'static str,
    mut apply: impl FnMut(&mut Tag) -> Result<bool, UpgradeError>,
) -> Result<bool, UpgradeError> {
    let Some(settings) = get_mut(tag, "WorldGenSettings") else {
        return Ok(false);
    };
    require_map(settings, version, fix, "WorldGenSettings")?;
    let Some(dimensions) = get_mut(settings, "dimensions") else {
        return Ok(false);
    };
    require_map(dimensions, version, fix, "WorldGenSettings.dimensions")?;
    let mut changed = false;
    for (_, dimension) in fields_mut(dimensions).expect("validated dimensions") {
        require_map(dimension, version, fix, "WorldGenSettings.dimensions[]")?;
        if let Some(generator) = get_mut(dimension, "generator") {
            require_map(
                generator,
                version,
                fix,
                "WorldGenSettings.dimensions[].generator",
            )?;
            match get(generator, "type").and_then(string) {
                Some("minecraft:noise" | "noise" | "minecraft:flat" | "flat") => {}
                Some(_) => continue,
                None => {
                    return Err(UpgradeError::unsupported(
                        version,
                        fix,
                        "WorldGenSettings.dimensions[].generator.type",
                        "affected generator has no string type discriminator",
                    ));
                }
            }
            changed |= apply(generator)?;
        }
    }
    Ok(changed)
}

fn flatten_structures(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "StructureSettingsFlattenFix";
    let changed = generators(tag, 2967, FIX, |generator| {
        let Some(settings) = get_mut(generator, "settings") else {
            return Ok(false);
        };
        // Named noise settings have no inline structure settings to flatten.
        if string(settings).is_some() {
            return Ok(false);
        }
        require_map(
            settings,
            2967,
            FIX,
            "WorldGenSettings.dimensions[].generator.settings",
        )?;
        let Some(old) = get_mut(settings, "structures") else {
            return Ok(false);
        };
        require_map(old, 2967, FIX, "generator.settings.structures")?;
        if fields(old)
            .expect("validated structures")
            .iter()
            .any(|(key, _)| key != "structures" && key != "stronghold")
        {
            return Err(UpgradeError::unsupported(
                2967,
                FIX,
                "generator.settings.structures",
                "flattening would discard unknown outer structure-settings fields",
            ));
        }
        if let Some(inner) = get(old, "structures") {
            require_map(inner, 2967, FIX, "generator.settings.structures.structures")?;
            for (_, placement) in fields(inner).expect("validated placements") {
                require_map(
                    placement,
                    2967,
                    FIX,
                    "generator.settings.structures.structures[]",
                )?;
            }
        }
        if let Some(stronghold) = get(old, "stronghold") {
            require_map(
                stronghold,
                2967,
                FIX,
                "generator.settings.structures.stronghold",
            )?;
        }
        let mut flattened = take(old, "structures").unwrap_or_else(|| Tag::Compound(Vec::new()));
        for (_, placement) in fields_mut(&mut flattened).expect("validated placements") {
            set(
                placement,
                "type",
                Tag::String("minecraft:random_spread".to_owned()),
            );
        }
        if let Some(mut stronghold) = take(old, "stronghold") {
            set(
                &mut stronghold,
                "type",
                Tag::String("minecraft:concentric_rings".to_owned()),
            );
            set(&mut flattened, "minecraft:stronghold", stronghold);
        }
        *old = flattened;
        Ok(true)
    })?;
    if changed {
        report.applied(2967, FIX);
    }
    Ok(())
}

fn rename_biome_source(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "NamespacedTypeRenameFix";
    let changed = generators(tag, 3440, FIX, |generator| {
        let Some(source) = get_mut(generator, "biome_source") else {
            return Ok(false);
        };
        if !matches!(
            get(source, "type").and_then(string),
            Some("minecraft:multi_noise" | "multi_noise")
        ) {
            return Ok(false);
        }
        if matches!(
            get(source, "preset").and_then(string),
            Some("minecraft:overworld_update_1_20" | "overworld_update_1_20")
        ) {
            set(
                source,
                "preset",
                Tag::String("minecraft:overworld".to_owned()),
            );
            return Ok(true);
        }
        Ok(false)
    })?;
    if changed {
        report.applied(3440, FIX);
    }
    Ok(())
}

fn remove_flag(
    tag: &mut Tag,
    version: i32,
    flag: &str,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    let Some(enabled) = get(tag, "enabled_features") else {
        return Ok(());
    };
    let Tag::List(enabled) = enabled else {
        return Err(UpgradeError::unsupported(
            version,
            "FeatureFlagRemoveFix",
            "enabled_features",
            "expected a feature list",
        ));
    };
    if !enabled.elements.iter().any(|v| string(v) == Some(flag)) {
        return Ok(());
    }
    // Moving an element must not create an invalid heterogeneous ListTag.
    for key in ["enabled_features", "removed_features"] {
        if let Some(value) = get(tag, key) {
            let Tag::List(value) = value else {
                return Err(UpgradeError::unsupported(
                    version,
                    "FeatureFlagRemoveFix",
                    key,
                    "expected a string list",
                ));
            };
            if value.elements.iter().any(|v| string(v).is_none()) {
                return Err(UpgradeError::unsupported(
                    version,
                    "FeatureFlagRemoveFix",
                    key,
                    "cannot append strings to a non-string list",
                ));
            }
        }
    }
    let mut removed = take(tag, "removed_features").unwrap_or_else(empty_list);
    let Tag::List(removed_list) = &mut removed else {
        unreachable!("validated removed_features")
    };
    let enabled = list_mut(get_mut(tag, "enabled_features").expect("enabled list"))
        .expect("validated enabled_features");
    let mut index = 0;
    while index < enabled.len() {
        if string(&enabled[index]) == Some(flag) {
            removed_list.elements.push(enabled.remove(index));
        } else {
            index += 1;
        }
    }
    removed_list.element_type = 8;
    set(tag, "removed_features", removed);
    report.applied(version, "FeatureFlagRemoveFix");
    Ok(())
}

fn legacy_dragon(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    if get(tag, "DragonFight").is_some() {
        return Ok(());
    }
    let mut dragon = get(tag, "DimensionData")
        .and_then(|v| get(v, "1"))
        .and_then(|v| get(v, "DragonFight"))
        .cloned()
        .unwrap_or_else(|| Tag::Compound(Vec::new()));
    require_map(
        &dragon,
        3459,
        "LegacyDragonFightFix",
        "DimensionData.1.DragonFight",
    )?;
    if let Some(pos) = get_mut(&mut dragon, "ExitPortalLocation") {
        if let (Some(x), Some(y), Some(z)) = (
            get(pos, "X").and_then(java_int),
            get(pos, "Y").and_then(java_int),
            get(pos, "Z").and_then(java_int),
        ) {
            *pos = Tag::IntArray(vec![x, y, z]);
        } else if !matches!(pos, Tag::IntArray(v) if v.len() == 3) {
            return Err(UpgradeError::unsupported(
                3459,
                "LegacyDragonFightFix",
                "DimensionData.1.DragonFight.ExitPortalLocation",
                "legacy position cannot be represented as an integer triplet",
            ));
        }
    }
    set(tag, "DragonFight", dragon);
    report.applied(3459, "LegacyDragonFightFix");
    Ok(())
}

fn legacy_border(tag: &mut Tag, report: &mut UpgradeReport) {
    let mut border = Tag::Compound(Vec::with_capacity(9));
    for (old, new, default) in [
        ("BorderCenterX", "center_x", 0.0),
        ("BorderCenterZ", "center_z", 0.0),
        ("BorderSize", "size", 59_999_968.0),
        ("BorderSizeLerpTarget", "lerp_target", 0.0),
        ("BorderSafeZone", "safe_zone", 5.0),
        ("BorderDamagePerBlock", "damage_per_block", 0.2),
    ] {
        set(&mut border, new, Tag::Double(num(tag, old, default)));
        remove(tag, old);
    }
    set(
        &mut border,
        "lerp_time",
        Tag::Long(long(tag, "BorderSizeLerpTime", 0)),
    );
    set(
        &mut border,
        "warning_blocks",
        Tag::Int(integer(tag, "BorderWarningBlocks", 5)),
    );
    set(
        &mut border,
        "warning_time",
        Tag::Int(integer(tag, "BorderWarningTime", 15)),
    );
    for key in [
        "BorderSizeLerpTime",
        "BorderWarningBlocks",
        "BorderWarningTime",
    ] {
        remove(tag, key);
    }
    set(tag, "world_border", border);
    report.applied(4544, "LegacyWorldBorderFix");
}

fn world_spawn(tag: &mut Tag, report: &mut UpgradeReport) {
    let spawn = compound(vec![
        ("dimension", Tag::String("minecraft:overworld".to_owned())),
        (
            "pos",
            Tag::IntArray(vec![
                integer(tag, "SpawnX", 0),
                integer(tag, "SpawnY", 0),
                integer(tag, "SpawnZ", 0),
            ]),
        ),
        ("yaw", Tag::Float(num(tag, "SpawnAngle", 0.0) as f32)),
        ("pitch", Tag::Float(0.0)),
    ]);
    for key in ["SpawnX", "SpawnY", "SpawnZ", "SpawnAngle"] {
        remove(tag, key);
    }
    set(tag, "spawn", spawn);
    report.applied(4548, "WorldSpawnDataFix");
}

#[derive(Clone, Copy)]
enum Rule {
    Bool,
    Invert,
    Integer(i32, i32),
}

const RULES: &[(&str, &str, Rule)] = &[
    (
        "allowEnteringNetherUsingPortals",
        "minecraft:allow_entering_nether_using_portals",
        Rule::Bool,
    ),
    (
        "announceAdvancements",
        "minecraft:show_advancement_messages",
        Rule::Bool,
    ),
    (
        "blockExplosionDropDecay",
        "minecraft:block_explosion_drop_decay",
        Rule::Bool,
    ),
    (
        "commandBlockOutput",
        "minecraft:command_block_output",
        Rule::Bool,
    ),
    (
        "enableCommandBlocks",
        "minecraft:command_blocks_work",
        Rule::Bool,
    ),
    (
        "commandBlocksEnabled",
        "minecraft:command_blocks_work",
        Rule::Bool,
    ),
    (
        "commandModificationBlockLimit",
        "minecraft:max_block_modifications",
        Rule::Integer(1, i32::MAX),
    ),
    (
        "disableElytraMovementCheck",
        "minecraft:elytra_movement_check",
        Rule::Invert,
    ),
    (
        "disablePlayerMovementCheck",
        "minecraft:player_movement_check",
        Rule::Invert,
    ),
    ("disableRaids", "minecraft:raids", Rule::Invert),
    ("doDaylightCycle", "minecraft:advance_time", Rule::Bool),
    ("doEntityDrops", "minecraft:entity_drops", Rule::Bool),
    (
        "doImmediateRespawn",
        "minecraft:immediate_respawn",
        Rule::Bool,
    ),
    ("doInsomnia", "minecraft:spawn_phantoms", Rule::Bool),
    (
        "doLimitedCrafting",
        "minecraft:limited_crafting",
        Rule::Bool,
    ),
    ("doMobLoot", "minecraft:mob_drops", Rule::Bool),
    ("doMobSpawning", "minecraft:spawn_mobs", Rule::Bool),
    ("doPatrolSpawning", "minecraft:spawn_patrols", Rule::Bool),
    ("doTileDrops", "minecraft:block_drops", Rule::Bool),
    (
        "doTraderSpawning",
        "minecraft:spawn_wandering_traders",
        Rule::Bool,
    ),
    ("doVinesSpread", "minecraft:spread_vines", Rule::Bool),
    ("doWardenSpawning", "minecraft:spawn_wardens", Rule::Bool),
    ("doWeatherCycle", "minecraft:advance_weather", Rule::Bool),
    ("drowningDamage", "minecraft:drowning_damage", Rule::Bool),
    (
        "enderPearlsVanishOnDeath",
        "minecraft:ender_pearls_vanish_on_death",
        Rule::Bool,
    ),
    ("fallDamage", "minecraft:fall_damage", Rule::Bool),
    ("fireDamage", "minecraft:fire_damage", Rule::Bool),
    (
        "forgiveDeadPlayers",
        "minecraft:forgive_dead_players",
        Rule::Bool,
    ),
    ("freezeDamage", "minecraft:freeze_damage", Rule::Bool),
    (
        "globalSoundEvents",
        "minecraft:global_sound_events",
        Rule::Bool,
    ),
    ("keepInventory", "minecraft:keep_inventory", Rule::Bool),
    (
        "lavaSourceConversion",
        "minecraft:lava_source_conversion",
        Rule::Bool,
    ),
    ("locatorBar", "minecraft:locator_bar", Rule::Bool),
    (
        "logAdminCommands",
        "minecraft:log_admin_commands",
        Rule::Bool,
    ),
    (
        "maxCommandChainLength",
        "minecraft:max_command_sequence_length",
        Rule::Integer(0, i32::MAX),
    ),
    (
        "maxCommandForkCount",
        "minecraft:max_command_forks",
        Rule::Integer(0, i32::MAX),
    ),
    (
        "maxEntityCramming",
        "minecraft:max_entity_cramming",
        Rule::Integer(0, i32::MAX),
    ),
    (
        "minecartMaxSpeed",
        "minecraft:max_minecart_speed",
        Rule::Integer(i32::MIN, i32::MAX),
    ),
    (
        "mobExplosionDropDecay",
        "minecraft:mob_explosion_drop_decay",
        Rule::Bool,
    ),
    ("mobGriefing", "minecraft:mob_griefing", Rule::Bool),
    (
        "naturalRegeneration",
        "minecraft:natural_health_regeneration",
        Rule::Bool,
    ),
    (
        "playersNetherPortalCreativeDelay",
        "minecraft:players_nether_portal_creative_delay",
        Rule::Integer(0, i32::MAX),
    ),
    (
        "playersNetherPortalDefaultDelay",
        "minecraft:players_nether_portal_default_delay",
        Rule::Integer(0, i32::MAX),
    ),
    (
        "playersSleepingPercentage",
        "minecraft:players_sleeping_percentage",
        Rule::Integer(0, i32::MAX),
    ),
    (
        "projectilesCanBreakBlocks",
        "minecraft:projectiles_can_break_blocks",
        Rule::Bool,
    ),
    ("pvp", "minecraft:pvp", Rule::Bool),
    (
        "randomTickSpeed",
        "minecraft:random_tick_speed",
        Rule::Integer(0, i32::MAX),
    ),
    (
        "reducedDebugInfo",
        "minecraft:reduced_debug_info",
        Rule::Bool,
    ),
    (
        "sendCommandFeedback",
        "minecraft:send_command_feedback",
        Rule::Bool,
    ),
    (
        "showDeathMessages",
        "minecraft:show_death_messages",
        Rule::Bool,
    ),
    (
        "snowAccumulationHeight",
        "minecraft:max_snow_accumulation_height",
        Rule::Integer(0, 8),
    ),
    ("spawnMonsters", "minecraft:spawn_monsters", Rule::Bool),
    (
        "spawnRadius",
        "minecraft:respawn_radius",
        Rule::Integer(i32::MIN, i32::MAX),
    ),
    (
        "spawnerBlocksEnabled",
        "minecraft:spawner_blocks_work",
        Rule::Bool,
    ),
    (
        "spectatorsGenerateChunks",
        "minecraft:spectators_generate_chunks",
        Rule::Bool,
    ),
    ("tntExplodes", "minecraft:tnt_explodes", Rule::Bool),
    (
        "tntExplosionDropDecay",
        "minecraft:tnt_explosion_drop_decay",
        Rule::Bool,
    ),
    ("universalAnger", "minecraft:universal_anger", Rule::Bool),
    (
        "waterSourceConversion",
        "minecraft:water_source_conversion",
        Rule::Bool,
    ),
];

fn game_rules(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    let Some(rules) = get_mut(tag, "GameRules") else {
        return Ok(());
    };
    require_map(rules, 4658, "GameRuleRegistryFix", "GameRules")?;
    let fire = get(rules, "doFireTick")
        .and_then(string)
        .unwrap_or("true")
        .eq_ignore_ascii_case("true");
    let away = get(rules, "allowFireTicksAwayFromPlayer")
        .and_then(string)
        .unwrap_or("false")
        .eq_ignore_ascii_case("true");
    if !fire || away {
        set(
            rules,
            "minecraft:fire_spread_radius_around_player",
            Tag::Int(if fire { -1 } else { 0 }),
        );
    }
    for key in [
        "spawnChunkRadius",
        "entitiesWithPassengersCanUsePortals",
        "gameLoopFunction",
        "doFireTick",
        "allowFireTicksAwayFromPlayer",
    ] {
        remove(rules, key);
    }
    for &(old, new, rule) in RULES {
        if let Some(value) = take(rules, old) {
            let text = string(&value).unwrap_or("");
            let converted = match rule {
                Rule::Bool => Tag::Byte(i8::from(text.eq_ignore_ascii_case("true"))),
                Rule::Invert => Tag::Byte(i8::from(!text.eq_ignore_ascii_case("true"))),
                Rule::Integer(min, max) => {
                    let parsed = text.parse::<i32>().map_err(|_| {
                        UpgradeError::unsupported(
                            4658,
                            "GameRuleRegistryFix",
                            format!("GameRules.{old}"),
                            "malformed integer gamerule would remain a legacy string",
                        )
                    })?;
                    Tag::Int(parsed.clamp(min, max))
                }
            };
            set(rules, new, converted);
        }
    }
    rename(tag, "GameRules", "game_rules");
    report.applied(4658, "GameRuleRegistryFix");
    Ok(())
}

fn clock(tag: &mut Tag, report: &mut UpgradeReport) {
    let day = long(tag, "DayTime", long(tag, "Time", 0));
    remove(tag, "DayTime");
    set(
        tag,
        "world_clocks",
        compound(vec![(
            "minecraft:overworld",
            compound(vec![
                ("total_ticks", Tag::Long(day)),
                ("paused", Tag::Byte(0)),
            ]),
        )]),
    );
    report.applied(4766, "DayTimeToClockFix");
}

fn difficulty(tag: &mut Tag, report: &mut UpgradeReport) {
    let name = match integer(tag, "Difficulty", 2) {
        0 => "peaceful",
        1 => "easy",
        3 => "hard",
        _ => "normal",
    };
    let settings = compound(vec![
        ("difficulty", Tag::String(name.to_owned())),
        ("hardcore", Tag::Byte(boolean(tag, "hardcore"))),
        ("locked", Tag::Byte(boolean(tag, "DifficultyLocked"))),
    ]);
    for key in ["Difficulty", "hardcore", "DifficultyLocked"] {
        remove(tag, key);
    }
    set(tag, "difficulty_settings", settings);
    report.applied(4771, "LevelDatDifficultyFix");
}

fn prepare_saved_data(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "LevelDatToSavedDataPreparationFix";
    if let Some(dragon) = get_mut(tag, "DragonFight") {
        require_map(dragon, 4771, FIX, "DragonFight")?;
        for (old, new) in [
            ("NeedsStateScanning", "needs_state_scanning"),
            ("DragonKilled", "dragon_killed"),
            ("PreviouslyKilled", "previously_killed"),
            ("Dragon", "dragon_uuid"),
            ("ExitPortalLocation", "exit_portal_location"),
            ("Gateways", "gateways"),
        ] {
            rename(dragon, old, new);
        }
        if boolean(dragon, "IsRespawning") != 0 {
            set(dragon, "respawn_stage", Tag::String("start".to_owned()));
            set(dragon, "respawn_time", Tag::Int(0));
        }
        remove(dragon, "IsRespawning");
    }
    rename(tag, "DragonFight", "dragon_fight");
    let trader = compound(vec![
        (
            "spawn_delay",
            Tag::Int(integer(tag, "WanderingTraderSpawnDelay", 24_000)),
        ),
        (
            "spawn_chance",
            Tag::Int(integer(tag, "WanderingTraderSpawnChance", 25)),
        ),
    ]);
    for key in [
        "WanderingTraderSpawnDelay",
        "WanderingTraderSpawnChance",
        "WanderingTraderId",
    ] {
        remove(tag, key);
    }
    set(tag, "wandering_trader_migration_data", trader);
    let weather = compound(vec![
        (
            "clear_weather_time",
            Tag::Int(integer(tag, "clearWeatherTime", 0)),
        ),
        ("rain_time", Tag::Int(integer(tag, "rainTime", 0))),
        ("thunder_time", Tag::Int(integer(tag, "thunderTime", 0))),
        ("raining", Tag::Byte(boolean(tag, "raining"))),
        ("thundering", Tag::Byte(boolean(tag, "thundering"))),
    ]);
    for key in [
        "clearWeatherTime",
        "rainTime",
        "thunderTime",
        "raining",
        "thundering",
    ] {
        remove(tag, key);
    }
    set(tag, "weather_data", weather);
    let mut events = take(tag, "ScheduledEvents").unwrap_or_else(empty_list);
    let Some(event_list) = list_mut(&mut events) else {
        return Err(UpgradeError::unsupported(
            4771,
            FIX,
            "ScheduledEvents",
            "expected a scheduled-event list",
        ));
    };
    for event in event_list {
        require_map(event, 4771, FIX, "ScheduledEvents[]")?;
        rename(event, "Name", "id");
        rename(event, "TriggerTime", "trigger_time");
        if let Some(callback) = get_mut(event, "Callback") {
            require_map(callback, 4771, FIX, "ScheduledEvents[].Callback")?;
            rename(callback, "Type", "type");
            rename(callback, "Name", "id");
        }
        rename(event, "Callback", "callback");
    }
    set(tag, "scheduled_events", compound(vec![("events", events)]));
    if let Some(worldgen) = get_mut(tag, "WorldGenSettings") {
        require_map(worldgen, 4771, FIX, "WorldGenSettings")?;
        rename(worldgen, "generate_features", "generate_structures");
    }
    rename(tag, "WorldGenSettings", "world_gen_settings");
    report.applied(4771, FIX);
    Ok(())
}

fn save_payload(report: &mut UpgradeReport, path: String, payload: Tag) {
    report.saved_data.push(SavedDataRecord {
        path,
        tag: compound(vec![
            ("data", payload),
            ("DataVersion", Tag::Int(TARGET_DATA_VERSION)),
        ]),
    });
}

fn split_saved_data(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "LevelDatToSavedDataFileFix";
    let prior_records = report.saved_data.len();
    for (key, path) in [
        (
            "dragon_fight",
            "dimensions/minecraft/the_end/data/minecraft/ender_dragon_fight.dat",
        ),
        (
            "wandering_trader_migration_data",
            "data/minecraft/wandering_trader.dat",
        ),
        ("CustomBossEvents", "data/minecraft/custom_boss_events.dat"),
        ("weather_data", "data/minecraft/weather.dat"),
        ("scheduled_events", "data/minecraft/scheduled_events.dat"),
        ("game_rules", "data/minecraft/game_rules.dat"),
        (
            "world_gen_settings",
            "data/minecraft/world_gen_settings.dat",
        ),
        ("world_clocks", "data/minecraft/world_clocks.dat"),
    ] {
        if let Some(payload) = get(tag, key) {
            require_map(payload, 4772, FIX, key)?;
        }
        if let Some(payload) = take(tag, key) {
            save_payload(report, path.to_owned(), payload);
        }
    }
    if let Some(border) = get(tag, "world_border") {
        require_map(border, 4772, FIX, "world_border")?;
    }
    if let Some(border) = take(tag, "world_border") {
        let borders = [
            ("overworld", 1.0, border.clone()),
            ("the_nether", 8.0, border.clone()),
            ("the_end", 1.0, border),
        ];
        for (dimension, scale, mut payload) in borders {
            for key in ["center_x", "center_z"] {
                if let Some(value) = get_mut(&mut payload, key) {
                    *value = Tag::Double(number(value).unwrap_or(0.0) / scale);
                }
            }
            save_payload(
                report,
                format!("dimensions/minecraft/{dimension}/data/minecraft/world_border.dat"),
                payload,
            );
        }
        report.preserved("saved_data/world_border.data.warning_time", "LevelDatToSavedDataFileFix4772 copies legacy level warning_time unchanged; WorldBorderWarningTimeFix4657 only applies to previously existing saved-data files");
    }
    if get(tag, "Player").is_some() {
        report.preserved("Player", "LevelDatToSavedDataFileFix4772 player-file extraction and singleplayer_uuid require importer player-path handling; embedded upgraded payload retained");
    }
    report.preserved("files", "ResourcePackLocationFileFix4772, DimensionStorageFileFix4772, PlayerStorageFileFix4772 and RemoveObsoleteFilesFileFix4772 require importer follow-through; this transformation emits only extracted NBT records");
    if report.saved_data.len() != prior_records {
        report.applied(4772, FIX);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(tag: &mut Tag, kind: NodeKind, version: i32) -> Result<UpgradeReport, UpgradeError> {
        let mut report = UpgradeReport::default();
        fix_node(tag, kind, version, &mut report)?;
        Ok(report)
    }

    fn worldgen(structures: Tag) -> Tag {
        compound(vec![(
            "WorldGenSettings",
            compound(vec![(
                "dimensions",
                compound(vec![(
                    "minecraft:overworld",
                    compound(vec![(
                        "generator",
                        compound(vec![
                            ("type", Tag::String("minecraft:noise".to_owned())),
                            ("settings", compound(vec![("structures", structures)])),
                        ]),
                    )]),
                )]),
            )]),
        )])
    }

    #[test]
    fn flatten_2967_and_reject_discarded_unknown_fields() {
        let mut level = worldgen(compound(vec![
            (
                "structures",
                compound(vec![(
                    "minecraft:village",
                    compound(vec![("salt", Tag::Int(3))]),
                )]),
            ),
            ("stronghold", compound(vec![("count", Tag::Int(128))])),
        ]));
        run(&mut level, NodeKind::Level, 2967).unwrap();
        let structures = get(
            get(
                get(
                    get(
                        get(get(&level, "WorldGenSettings").unwrap(), "dimensions").unwrap(),
                        "minecraft:overworld",
                    )
                    .unwrap(),
                    "generator",
                )
                .unwrap(),
                "settings",
            )
            .unwrap(),
            "structures",
        )
        .unwrap();
        assert_eq!(
            get(get(structures, "minecraft:village").unwrap(), "type").and_then(string),
            Some("minecraft:random_spread")
        );
        assert_eq!(
            get(get(structures, "minecraft:stronghold").unwrap(), "type").and_then(string),
            Some("minecraft:concentric_rings")
        );
        let mut bad = worldgen(compound(vec![("mod_data", Tag::Int(1))]));
        assert!(matches!(
            run(&mut bad, NodeKind::Level, 2967),
            Err(UpgradeError::UnsupportedField { version: 2967, .. })
        ));
    }

    #[test]
    fn chunk_and_saved_data_fixes_do_not_rewrite_level_opaque_fields() {
        let mut level = compound(vec![
            ("structures", Tag::String("opaque".to_owned())),
            ("random_sequences", Tag::Int(5)),
            ("data", compound(vec![("warning_time", Tag::Int(15))])),
        ]);
        let original = level.clone();
        for step in [2970, 3565, 4657] {
            assert!(
                run(&mut level, NodeKind::Level, step)
                    .unwrap()
                    .steps
                    .is_empty()
            );
        }
        assert_eq!(level, original);
    }

    #[test]
    fn biome_preset_3440_changes_only_typed_source() {
        let mut level = compound(vec![(
            "WorldGenSettings",
            compound(vec![(
                "dimensions",
                compound(vec![(
                    "minecraft:overworld",
                    compound(vec![(
                        "generator",
                        compound(vec![
                            ("type", Tag::String("minecraft:noise".to_owned())),
                            (
                                "biome_source",
                                compound(vec![
                                    ("type", Tag::String("minecraft:multi_noise".to_owned())),
                                    (
                                        "preset",
                                        Tag::String("minecraft:overworld_update_1_20".to_owned()),
                                    ),
                                ]),
                            ),
                        ]),
                    )]),
                )]),
            )]),
        )]);
        assert_eq!(
            run(&mut level, NodeKind::Level, 3440).unwrap().steps[0].fix,
            "NamespacedTypeRenameFix"
        );
    }

    #[test]
    fn flags_each_registered_version_moves_duplicates_and_preserves_others() {
        for (version, flag) in [
            (3440, "minecraft:update_1_20"),
            (3939, "minecraft:update_1_21"),
            (4067, "minecraft:bundle"),
            (4180, "minecraft:winter_drop"),
            (4424, "minecraft:locator_bar"),
        ] {
            let mut level = compound(vec![(
                "enabled_features",
                Tag::List(ListTag {
                    element_type: 8,
                    elements: vec![
                        Tag::String(flag.to_owned()),
                        Tag::String("mod:opaque".to_owned()),
                        Tag::String(flag.to_owned()),
                    ],
                }),
            )]);
            run(&mut level, NodeKind::Level, version).unwrap();
            assert_eq!(
                super::super::nbt::list(get(&level, "enabled_features").unwrap())
                    .unwrap()
                    .len(),
                1
            );
            assert_eq!(
                super::super::nbt::list(get(&level, "removed_features").unwrap())
                    .unwrap()
                    .len(),
                2
            );
            let mut bad = compound(vec![("enabled_features", Tag::Int(2))]);
            assert!(run(&mut bad, NodeKind::Level, version).is_err());
        }
    }

    #[test]
    fn dragon_3459_existing_wins_and_legacy_position_converts() {
        let legacy = compound(vec![(
            "ExitPortalLocation",
            compound(vec![
                ("X", Tag::Int(1)),
                ("Y", Tag::Int(64)),
                ("Z", Tag::Int(2)),
            ]),
        )]);
        let mut level = compound(vec![(
            "DimensionData",
            compound(vec![("1", compound(vec![("DragonFight", legacy)]))]),
        )]);
        run(&mut level, NodeKind::Level, 3459).unwrap();
        assert_eq!(
            get(get(&level, "DragonFight").unwrap(), "ExitPortalLocation"),
            Some(&Tag::IntArray(vec![1, 64, 2]))
        );
        let previous = level.clone();
        run(&mut level, NodeKind::Level, 3459).unwrap();
        assert_eq!(level, previous);
        let mut malformed = compound(vec![(
            "DimensionData",
            compound(vec![(
                "1",
                compound(vec![(
                    "DragonFight",
                    compound(vec![("ExitPortalLocation", Tag::String("bad".to_owned()))]),
                )]),
            )]),
        )]);
        assert!(run(&mut malformed, NodeKind::Level, 3459).is_err());
    }

    #[test]
    fn border4544_defaults_spawn4548_and_respawn4548() {
        let mut level = compound(vec![
            ("BorderCenterX", Tag::Double(80.0)),
            ("SpawnY", Tag::Int(70)),
            ("SpawnAngle", Tag::Float(45.0)),
        ]);
        run(&mut level, NodeKind::Level, 4544).unwrap();
        assert_eq!(
            get(get(&level, "world_border").unwrap(), "size"),
            Some(&Tag::Double(59_999_968.0))
        );
        run(&mut level, NodeKind::Level, 4548).unwrap();
        assert_eq!(
            get(get(&level, "spawn").unwrap(), "pos"),
            Some(&Tag::IntArray(vec![0, 70, 0]))
        );
        let mut player = compound(vec![(
            "respawn",
            compound(vec![("angle", Tag::Float(90.0)), ("forced", Tag::Byte(1))]),
        )]);
        run(&mut player, NodeKind::Player, 4548).unwrap();
        let respawn = get(&player, "respawn").unwrap();
        assert_eq!(get(respawn, "yaw"), Some(&Tag::Float(90.0)));
        assert_eq!(
            get(respawn, "dimension").and_then(string),
            Some("minecraft:overworld")
        );
        assert_eq!(get(respawn, "forced"), Some(&Tag::Byte(1)));
        let mut bad = compound(vec![("respawn", Tag::Int(0))]);
        assert!(run(&mut bad, NodeKind::Player, 4548).is_err());
    }

    #[test]
    fn gamerules4658_registry_types_bounds_inversion_and_unknown_preservation() {
        let mut level = compound(vec![(
            "GameRules",
            compound(vec![
                ("keepInventory", Tag::String("TRUE".to_owned())),
                ("disableRaids", Tag::String("true".to_owned())),
                ("snowAccumulationHeight", Tag::String("999".to_owned())),
                ("spawnRadius", Tag::String("-7".to_owned())),
                ("mod:opaque", Tag::String("keep".to_owned())),
            ]),
        )]);
        run(&mut level, NodeKind::Level, 4658).unwrap();
        let rules = get(&level, "game_rules").unwrap();
        assert_eq!(get(rules, "minecraft:keep_inventory"), Some(&Tag::Byte(1)));
        assert_eq!(get(rules, "minecraft:raids"), Some(&Tag::Byte(0)));
        assert_eq!(
            get(rules, "minecraft:max_snow_accumulation_height"),
            Some(&Tag::Int(8))
        );
        assert_eq!(get(rules, "minecraft:respawn_radius"), Some(&Tag::Int(-7)));
        assert_eq!(get(rules, "mod:opaque").and_then(string), Some("keep"));
        let mut bad = compound(vec![(
            "GameRules",
            compound(vec![("randomTickSpeed", Tag::String("invalid".to_owned()))]),
        )]);
        assert!(run(&mut bad, NodeKind::Level, 4658).is_err());
    }

    #[test]
    fn clocks4766_keep_time_and_preserve_exact_long() {
        let ticks = i64::MAX - 10;
        let mut level = compound(vec![("Time", Tag::Long(9)), ("DayTime", Tag::Long(ticks))]);
        run(&mut level, NodeKind::Level, 4766).unwrap();
        assert_eq!(get(&level, "Time"), Some(&Tag::Long(9)));
        assert_eq!(
            get(
                get(get(&level, "world_clocks").unwrap(), "minecraft:overworld").unwrap(),
                "total_ticks"
            ),
            Some(&Tag::Long(ticks))
        );
    }

    #[test]
    fn preparation4771_and_split4772_emit_complete_wrapped_payloads() {
        let mut level = compound(vec![
            ("Difficulty", Tag::Byte(3)),
            ("hardcore", Tag::Byte(1)),
            (
                "DragonFight",
                compound(vec![
                    ("IsRespawning", Tag::Byte(1)),
                    ("DragonKilled", Tag::Byte(0)),
                ]),
            ),
            (
                "ScheduledEvents",
                Tag::List(ListTag {
                    element_type: 10,
                    elements: vec![compound(vec![
                        ("Name", Tag::String("test".to_owned())),
                        (
                            "Callback",
                            compound(vec![
                                ("Type", Tag::String("minecraft:function".to_owned())),
                                ("Name", Tag::String("test:fn".to_owned())),
                            ]),
                        ),
                    ])],
                }),
            ),
            (
                "world_border",
                compound(vec![
                    ("center_x", Tag::Double(80.0)),
                    ("center_z", Tag::Double(-16.0)),
                    ("warning_time", Tag::Int(15)),
                ]),
            ),
            ("opaque", Tag::Int(8)),
        ]);
        run(&mut level, NodeKind::Level, 4771).unwrap();
        assert_eq!(
            get(get(&level, "difficulty_settings").unwrap(), "difficulty").and_then(string),
            Some("hard")
        );
        assert_eq!(
            get(get(&level, "dragon_fight").unwrap(), "respawn_stage").and_then(string),
            Some("start")
        );
        let report = run(&mut level, NodeKind::Level, 4772).unwrap();
        assert_eq!(report.saved_data.len(), 7);
        let nether = report
            .saved_data
            .iter()
            .find(|r| r.path.contains("the_nether"))
            .unwrap();
        assert_eq!(
            get(get(&nether.tag, "data").unwrap(), "center_x"),
            Some(&Tag::Double(10.0))
        );
        assert_eq!(
            get(get(&nether.tag, "data").unwrap(), "warning_time"),
            Some(&Tag::Int(15))
        );
        assert_eq!(
            get(&nether.tag, "DataVersion"),
            Some(&Tag::Int(TARGET_DATA_VERSION))
        );
        assert_eq!(get(&level, "opaque"), Some(&Tag::Int(8)));
        assert!(get(&level, "world_border").is_none());
        assert!(
            !run(&mut level, NodeKind::Level, 4773)
                .unwrap()
                .preserved
                .is_empty()
        );
    }

    #[test]
    fn preparation_and_split_reject_unrepresentable_affected_payloads() {
        let mut level = compound(vec![("ScheduledEvents", Tag::Int(1))]);
        assert!(run(&mut level, NodeKind::Level, 4771).is_err());
        let mut level = compound(vec![("weather_data", Tag::Int(1))]);
        assert!(run(&mut level, NodeKind::Level, 4772).is_err());
    }

    #[test]
    fn gamerules4658_covers_every_registered_rename_in_vanilla_order() {
        let rules = Tag::Compound(
            RULES
                .iter()
                .map(|(old, _, rule)| {
                    let value = match rule {
                        Rule::Bool | Rule::Invert => "true",
                        Rule::Integer(_, _) => "-1",
                    };
                    ((*old).to_owned(), Tag::String(value.to_owned()))
                })
                .collect(),
        );
        let mut level = compound(vec![("GameRules", rules)]);
        run(&mut level, NodeKind::Level, 4658).unwrap();
        let migrated = get(&level, "game_rules").unwrap();
        for &(_, new, rule) in RULES {
            let expected = match rule {
                Rule::Bool => Tag::Byte(1),
                Rule::Invert => Tag::Byte(0),
                Rule::Integer(min, max) => Tag::Int((-1_i32).clamp(min, max)),
            };
            assert_eq!(get(migrated, new), Some(&expected), "{new}");
        }
    }

    #[test]
    fn split4772_emits_all_eight_named_payloads_and_three_dimension_borders() {
        let mut level = compound(vec![
            ("dragon_fight", compound(Vec::new())),
            ("wandering_trader_migration_data", compound(Vec::new())),
            ("CustomBossEvents", compound(Vec::new())),
            ("weather_data", compound(Vec::new())),
            ("scheduled_events", compound(Vec::new())),
            ("game_rules", compound(Vec::new())),
            ("world_gen_settings", compound(Vec::new())),
            ("world_clocks", compound(Vec::new())),
            ("world_border", compound(vec![("center_x", Tag::Int(80))])),
            ("Player", compound(vec![("opaque", Tag::Int(1))])),
        ]);
        let report = run(&mut level, NodeKind::Level, 4772).unwrap();
        assert_eq!(report.saved_data.len(), 11);
        assert!(
            report
                .saved_data
                .iter()
                .all(|record| get(&record.tag, "data").is_some())
        );
        assert!(get(&level, "Player").is_some());
        assert!(
            report
                .preserved
                .iter()
                .any(|receipt| receipt.path == "Player")
        );
        assert!(report.saved_data.iter().any(|record| record.path
            == "dimensions/minecraft/the_end/data/minecraft/ender_dragon_fight.dat"));
        assert!(
            report
                .saved_data
                .iter()
                .any(|record| record.path == "data/minecraft/world_gen_settings.dat")
        );
    }

    #[test]
    fn unknown_worldgen_generator_fields_are_not_flattened() {
        let mut level = worldgen(compound(vec![("mod:opaque", Tag::Int(1))]));
        let generator = get_mut(
            get_mut(
                get_mut(
                    get_mut(&mut level, "WorldGenSettings").unwrap(),
                    "dimensions",
                )
                .unwrap(),
                "minecraft:overworld",
            )
            .unwrap(),
            "generator",
        )
        .unwrap();
        set(generator, "type", Tag::String("mod:custom".to_owned()));
        let original = level.clone();
        run(&mut level, NodeKind::Level, 2967).unwrap();
        assert_eq!(level, original);
    }
}
