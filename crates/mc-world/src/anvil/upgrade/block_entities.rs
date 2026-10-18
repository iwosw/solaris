//! Pure block-entity fixes. Evidence lives in `.analysis/old-world-upgrade/*javap.txt`.
use mc_nbt::{ListTag, Tag};

use super::nbt::{
    compound, fields, get, get_mut, list, list_mut, remove, rename, set, string, take,
};
use super::{NodeKind, UpgradeError, UpgradeReport};

#[path = "block_entities/trial_configs.rs"]
mod trial_configs;

const SIGN_FIELDS: [&str; 10] = [
    "Text1",
    "Text2",
    "Text3",
    "Text4",
    "FilteredText1",
    "FilteredText2",
    "FilteredText3",
    "FilteredText4",
    "Color",
    "GlowingText",
];
const EMPTY_TEXT: &str = "{\"text\":\"\"}";
const TRIAL_FIELDS: [&str; 9] = [
    "spawn_range",
    "total_mobs",
    "simultaneous_mobs",
    "total_mobs_added_per_player",
    "simultaneous_mobs_added_per_player",
    "ticks_between_spawn",
    "spawn_potentials",
    "loot_tables_to_eject",
    "items_to_drop_when_ominous",
];

pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if !matches!(kind, NodeKind::BlockEntity) {
        return Ok(());
    }
    // Tagged-choice fixes do not inspect unknown/modded block entities.
    let Some(id) = get(tag, "id").and_then(string).and_then(vanilla_id) else {
        return Ok(());
    };
    let fix = match version {
        3097 if id == "sign" => {
            for key in &SIGN_FIELDS[4..8] {
                remove(tag, key);
            }
            "FilteredSignsFix"
        }
        3438 | 3447 if id == "decorated_pot" => {
            pottery_item_names(tag, version)?;
            "ItemRenameFix"
        }
        3438 if matches!(id, "suspicious_sand" | "brushable_block") => {
            if id == "suspicious_sand" {
                set(tag, "id", Tag::String("minecraft:brushable_block".into()));
                report.applied(version, "BlockEntityRenameFix");
            }
            rename(tag, "loot_table", "LootTable");
            rename(tag, "loot_table_seed", "LootTableSeed");
            "EntityBrushableBlockFieldsRenameFix"
        }
        3439 if matches!(id, "sign" | "hanging_sign")
            && (report.from != version || id == "hanging_sign") =>
        {
            sign_sides(tag, version)?;
            "BlockEntitySignDoubleSidedEditableTextFix"
        }
        3448 if id == "decorated_pot" => {
            if get(tag, "shards").is_some_and(|shards| {
                list(shards).is_none_or(|values| values.iter().any(|value| string(value).is_none()))
            }) {
                return Err(unsupported(
                    version,
                    "DecoratedPotFieldRenameFix",
                    "shards",
                    "expected schema-defined item-name list",
                ));
            }
            rename(tag, "shards", "sherds");
            "DecoratedPotFieldRenameFix"
        }
        3564 if matches!(id, "sign" | "hanging_sign")
            && (report.from != version || id == "hanging_sign") =>
        {
            if get(tag, "_filtered_correct")
                .and_then(java_int)
                .unwrap_or(0)
                != 0
            {
                remove(tag, "_filtered_correct");
            } else {
                for side in ["front_text", "back_text"] {
                    if let Some(text) = get_mut(tag, side) {
                        clean_filtered(text, version)?;
                    }
                }
                for key in SIGN_FIELDS {
                    remove(tag, key);
                }
            }
            "DropInvalidSignDataFix"
        }
        3813 if matches!(id, "beehive" | "end_gateway") => {
            let (old, new) = if id == "beehive" {
                ("FlowerPos", "flower_pos")
            } else {
                ("ExitPortal", "exit_portal")
            };
            if let Some(mut pos) = take(tag, old) {
                if let (Some(x), Some(y), Some(z)) = (
                    get(&pos, "X").and_then(java_int),
                    get(&pos, "Y").and_then(java_int),
                    get(&pos, "Z").and_then(java_int),
                ) {
                    pos = Tag::IntArray(vec![x, y, z]);
                }
                set(tag, new, pos);
            }
            "BlockPosFormatAndRenamesFix"
        }
        3818 if id == "beehive" && report.from != version => {
            if let Some(bees) = get(tag, "Bees") {
                let entries = list(bees).ok_or_else(|| {
                    unsupported(
                        version,
                        "BeehiveFieldRenameFix",
                        "Bees",
                        "expected typed bee list",
                    )
                })?;
                if entries.iter().any(|bee| fields(bee).is_none()) {
                    return Err(unsupported(
                        version,
                        "BeehiveFieldRenameFix",
                        "Bees[]",
                        "expected typed bee compound",
                    ));
                }
            }
            if let Some(mut bees) = take(tag, "Bees") {
                if let Some(entries) = list_mut(&mut bees) {
                    for bee in entries {
                        rename(bee, "EntityData", "entity_data");
                        rename(bee, "TicksInHive", "ticks_in_hive");
                        rename(bee, "MinOccupationTicks", "min_ticks_in_hive");
                    }
                }
                set(tag, "bees", bees);
            }
            "BeehiveFieldRenameFix"
        }
        3818 if id == "banner" => {
            banner_patterns(tag, version)?;
            "BannerPatternFormatFix"
        }
        3820 if id == "skull" => {
            if let Some(owner) = take(tag, "SkullOwner").or_else(|| take(tag, "ExtraType")) {
                remove(tag, "ExtraType");
                set(tag, "profile", profile(owner));
            }
            "PlayerHeadBlockProfileFix"
        }
        3825 if id == "banner" => {
            banner_name(tag, version)?;
            "BannerEntityCustomNameToOverrideComponentFix"
        }
        4054 if id == "banner" => {
            banner_rarity(tag, version)?;
            "OminousBannerRarityFix"
        }
        3825 if id == "trial_spawner" => {
            let mut config = Vec::new();
            for key in TRIAL_FIELDS {
                if let Some(value) = take(tag, key) {
                    config.push((key.into(), value));
                }
            }
            if !config.is_empty() {
                set(tag, "normal_config", Tag::Compound(config));
            }
            "TrialSpawnerConfigFix"
        }
        3833 if id == "brushable_block" => {
            if let Some(item) = get(tag, "item") {
                let item_id = get(item, "id").and_then(string).unwrap_or("minecraft:air");
                if matches!(item_id, "air" | "minecraft:air")
                    || get(item, "count").and_then(java_int).unwrap_or(0) == 0
                {
                    remove(tag, "item");
                }
            }
            "RemoveEmptyItemInBrushableBlockFix"
        }
        3945 if id == "jukebox" => {
            let elapsed = get(tag, "TickCount")
                .and_then(java_long)
                .unwrap_or(0)
                .wrapping_sub(get(tag, "RecordStartTick").and_then(java_long).unwrap_or(0));
            for key in ["IsPlaying", "TickCount", "RecordStartTick"] {
                remove(tag, key);
            }
            if elapsed > 0 {
                set(tag, "ticks_since_song_started", Tag::Long(elapsed));
            }
            "JukeboxTicksSinceSongStartedFix"
        }
        4061 if id == "trial_spawner" => {
            trial_configs::fix(tag);
            "TrialSpawnerConfigInRegistryFix"
        }
        4068 => {
            if let Some(lock) = take(tag, "Lock")
                && let Some(name) = string(&lock).filter(|name| !name.is_empty())
            {
                // Vanilla escapes only quote/backslash, and wraps a JSON string primitive.
                let mut encoded = String::with_capacity(name.len() + 2);
                encoded.push('"');
                for ch in name.chars() {
                    if matches!(ch, '"' | '\\') {
                        encoded.push('\\');
                    }
                    encoded.push(ch);
                }
                encoded.push('"');
                set(
                    tag,
                    "lock",
                    compound(vec![(
                        "components",
                        compound(vec![("minecraft:custom_name", Tag::String(encoded))]),
                    )]),
                );
            }
            "ContainerBlockEntityLockPredicateFix"
        }
        4176 => {
            if get(tag, "lock").is_some_and(broken_lock) {
                remove(tag, "lock");
            }
            "InvalidBlockEntityLockFix"
        }
        4181 if matches!(id, "furnace" | "smoker" | "blast_furnace") => {
            rename(tag, "CookTime", "cooking_time_spent");
            rename(tag, "CookTimeTotal", "cooking_total_time");
            rename(tag, "BurnTime", "lit_time_remaining");
            if let Some(value) = get(tag, "lit_time_remaining").cloned() {
                set(tag, "lit_total_time", value);
            }
            "BlockEntityFurnaceBurnTimeFix"
        }
        // InlineBlockPosFormatFix (4314) registers PLAYER and ENTITY, not BLOCK_ENTITY.
        _ => return Ok(()),
    };
    report.applied(version, fix);
    Ok(())
}

fn pottery_item_names(tag: &mut Tag, version: i32) -> Result<(), UpgradeError> {
    let Some(shards) = get(tag, "shards") else {
        return Ok(());
    };
    let values = list(shards).ok_or_else(|| {
        unsupported(
            version,
            "ItemRenameFix",
            "shards",
            "expected schema-defined item-name list",
        )
    })?;
    if values.iter().any(|value| string(value).is_none()) {
        return Err(unsupported(
            version,
            "ItemRenameFix",
            "shards[]",
            "expected schema-defined item name",
        ));
    }
    if let Some(values) = get_mut(tag, "shards").and_then(list_mut) {
        for value in values {
            let Some(name) = string(value) else {
                unreachable!("validated item name")
            };
            let name = name.strip_prefix("minecraft:").unwrap_or(name);
            let replacement = if version == 3438 {
                match name {
                    "pottery_shard_archer" => Some("minecraft:archer_pottery_shard"),
                    "pottery_shard_prize" => Some("minecraft:prize_pottery_shard"),
                    "pottery_shard_arms_up" => Some("minecraft:arms_up_pottery_shard"),
                    "pottery_shard_skull" => Some("minecraft:skull_pottery_shard"),
                    _ => None,
                }
            } else {
                match name {
                    "angler_pottery_shard" => Some("minecraft:angler_pottery_sherd"),
                    "archer_pottery_shard" => Some("minecraft:archer_pottery_sherd"),
                    "arms_up_pottery_shard" => Some("minecraft:arms_up_pottery_sherd"),
                    "blade_pottery_shard" => Some("minecraft:blade_pottery_sherd"),
                    "brewer_pottery_shard" => Some("minecraft:brewer_pottery_sherd"),
                    "burn_pottery_shard" => Some("minecraft:burn_pottery_sherd"),
                    "danger_pottery_shard" => Some("minecraft:danger_pottery_sherd"),
                    "explorer_pottery_shard" => Some("minecraft:explorer_pottery_sherd"),
                    "friend_pottery_shard" => Some("minecraft:friend_pottery_sherd"),
                    "heart_pottery_shard" => Some("minecraft:heart_pottery_sherd"),
                    "heartbreak_pottery_shard" => Some("minecraft:heartbreak_pottery_sherd"),
                    "howl_pottery_shard" => Some("minecraft:howl_pottery_sherd"),
                    "miner_pottery_shard" => Some("minecraft:miner_pottery_sherd"),
                    "mourner_pottery_shard" => Some("minecraft:mourner_pottery_sherd"),
                    "plenty_pottery_shard" => Some("minecraft:plenty_pottery_sherd"),
                    "prize_pottery_shard" => Some("minecraft:prize_pottery_sherd"),
                    "sheaf_pottery_shard" => Some("minecraft:sheaf_pottery_sherd"),
                    "shelter_pottery_shard" => Some("minecraft:shelter_pottery_sherd"),
                    "skull_pottery_shard" => Some("minecraft:skull_pottery_sherd"),
                    "snort_pottery_shard" => Some("minecraft:snort_pottery_sherd"),
                    _ => None,
                }
            };
            if let Some(replacement) = replacement {
                *value = Tag::String(replacement.into());
            }
        }
    }
    Ok(())
}

fn unsupported(version: i32, fix: &'static str, path: &str, reason: &str) -> UpgradeError {
    UpgradeError::unsupported(version, fix, path, reason)
}

fn vanilla_id(id: &str) -> Option<&'static str> {
    let id = id.strip_prefix("minecraft:").unwrap_or(id);
    Some(match id {
        "sign" => "sign",
        "hanging_sign" => "hanging_sign",
        "suspicious_sand" => "suspicious_sand",
        "brushable_block" => "brushable_block",
        "decorated_pot" => "decorated_pot",
        "beehive" => "beehive",
        "end_gateway" => "end_gateway",
        "banner" => "banner",
        "skull" => "skull",
        "trial_spawner" => "trial_spawner",
        "jukebox" => "jukebox",
        "furnace" => "furnace",
        "smoker" => "smoker",
        "blast_furnace" => "blast_furnace",
        "chest" => "chest",
        "trapped_chest" => "trapped_chest",
        "ender_chest" => "ender_chest",
        "barrel" => "barrel",
        "hopper" => "hopper",
        "dispenser" => "dispenser",
        "dropper" => "dropper",
        "shulker_box" => "shulker_box",
        "brewing_stand" => "brewing_stand",
        "beacon" => "beacon",
        "campfire" => "campfire",
        "crafter" => "crafter",
        "vault" => "vault",
        "spawner" => "spawner",
        "mob_spawner" => "mob_spawner",
        "command_block" => "command_block",
        "conduit" => "conduit",
        "enchanting_table" => "enchanting_table",
        "end_portal" => "end_portal",
        "piston" => "piston",
        "structure_block" => "structure_block",
        "jigsaw" => "jigsaw",
        "lectern" => "lectern",
        "bell" => "bell",
        "sculk_sensor" => "sculk_sensor",
        "calibrated_sculk_sensor" => "calibrated_sculk_sensor",
        "sculk_catalyst" => "sculk_catalyst",
        "sculk_shrieker" => "sculk_shrieker",
        "daylight_detector" => "daylight_detector",
        "comparator" => "comparator",
        "bed" => "bed",
        "chiseled_bookshelf" => "chiseled_bookshelf",
        "test_block" => "test_block",
        "test_instance_block" => "test_instance_block",
        "shelf" => "shelf",
        "copper_golem_statue" => "copper_golem_statue",
        _ => return None,
    })
}

fn java_int(tag: &Tag) -> Option<i32> {
    Some(match tag {
        Tag::Byte(v) => i32::from(*v),
        Tag::Short(v) => i32::from(*v),
        Tag::Int(v) => *v,
        Tag::Long(v) => *v as i32,
        Tag::Float(v) => *v as i32,
        Tag::Double(v) => *v as i32,
        _ => return None,
    })
}

fn java_long(tag: &Tag) -> Option<i64> {
    Some(match tag {
        Tag::Byte(v) => i64::from(*v),
        Tag::Short(v) => i64::from(*v),
        Tag::Int(v) => i64::from(*v),
        Tag::Long(v) => *v,
        Tag::Float(v) => *v as i64,
        Tag::Double(v) => *v as i64,
        _ => return None,
    })
}

fn typed_list(
    elements: Vec<Tag>,
    version: i32,
    fix: &'static str,
    path: &str,
) -> Result<Tag, UpgradeError> {
    let element_type = elements.first().map_or(0, Tag::type_id);
    if elements.iter().any(|value| value.type_id() != element_type) {
        return Err(unsupported(
            version,
            fix,
            path,
            "heterogeneous dynamic list cannot be represented by NBT",
        ));
    }
    Ok(Tag::List(ListTag {
        element_type,
        elements,
    }))
}

fn sign_sides(tag: &mut Tag, version: i32) -> Result<(), UpgradeError> {
    let fix = "BlockEntitySignDoubleSidedEditableTextFix";
    for key in &SIGN_FIELDS[..8] {
        if get(tag, key).is_some_and(|value| !matches!(value, Tag::String(_))) {
            return Err(unsupported(
                version,
                fix,
                key,
                "expected schema-defined serialized text",
            ));
        }
    }
    let mut messages = Vec::with_capacity(4);
    let mut filtered = Vec::with_capacity(4);
    let has_filtered = SIGN_FIELDS[4..8].iter().any(|key| get(tag, key).is_some());
    for (key, filtered_key) in SIGN_FIELDS[..4].iter().zip(&SIGN_FIELDS[4..8]) {
        let message = get(tag, key)
            .cloned()
            .unwrap_or_else(|| Tag::String(EMPTY_TEXT.into()));
        if has_filtered {
            filtered.push(
                get(tag, filtered_key)
                    .cloned()
                    .unwrap_or_else(|| message.clone()),
            );
        }
        messages.push(message);
    }
    let mut front = compound(vec![
        (
            "messages",
            typed_list(messages, version, fix, "front_text.messages")?,
        ),
        (
            "color",
            get(tag, "Color")
                .cloned()
                .unwrap_or_else(|| Tag::String("black".into())),
        ),
        (
            "has_glowing_text",
            get(tag, "GlowingText").cloned().unwrap_or(Tag::Byte(0)),
        ),
    ]);
    if has_filtered {
        set(
            &mut front,
            "filtered_messages",
            typed_list(filtered, version, fix, "front_text.filtered_messages")?,
        );
    }
    let back = compound(vec![
        (
            "messages",
            Tag::List(ListTag {
                element_type: 8,
                elements: (0..4).map(|_| Tag::String(EMPTY_TEXT.into())).collect(),
            }),
        ),
        ("color", Tag::String("black".into())),
        ("has_glowing_text", Tag::Byte(0)),
    ]);
    set(tag, "front_text", front);
    set(tag, "back_text", back);
    set(tag, "is_waxed", Tag::Byte(0));
    set(tag, "_filtered_correct", Tag::Byte(1));
    for key in SIGN_FIELDS {
        remove(tag, key);
    }
    Ok(())
}

fn clean_filtered(text: &mut Tag, version: i32) -> Result<(), UpgradeError> {
    let Some(filtered) = get(text, "filtered_messages").and_then(list) else {
        return Ok(());
    };
    let messages = get(text, "messages").and_then(list).unwrap_or(&[]);
    let empty = Tag::String(EMPTY_TEXT.into());
    let mut repaired = Vec::with_capacity(filtered.len());
    for (index, message) in filtered.iter().enumerate() {
        repaired.push(if message == &empty {
            messages.get(index).unwrap_or(&empty).clone()
        } else {
            message.clone()
        });
    }
    if repaired == messages {
        remove(text, "filtered_messages");
    } else {
        set(
            text,
            "filtered_messages",
            typed_list(
                repaired,
                version,
                "DropInvalidSignDataFix",
                "filtered_messages",
            )?,
        );
    }
    Ok(())
}

fn banner_patterns(tag: &mut Tag, version: i32) -> Result<(), UpgradeError> {
    if let Some(patterns) = get(tag, "Patterns") {
        let entries = list(patterns).ok_or_else(|| {
            unsupported(
                version,
                "BannerPatternFormatFix",
                "Patterns",
                "expected banner layer list",
            )
        })?;
        if entries.iter().any(|layer| fields(layer).is_none()) {
            return Err(unsupported(
                version,
                "BannerPatternFormatFix",
                "Patterns[]",
                "expected layer compound",
            ));
        }
    }
    if let Some(mut patterns) = take(tag, "Patterns") {
        if let Some(entries) = list_mut(&mut patterns) {
            for layer in entries {
                if let Some(mut pattern) = take(layer, "Pattern") {
                    if let Some(name) = string(&pattern).and_then(pattern_name) {
                        pattern = Tag::String(name.into());
                    }
                    set(layer, "pattern", pattern);
                }
                let color = get(layer, "Color").and_then(java_int).unwrap_or(0);
                let colors = [
                    "white",
                    "orange",
                    "magenta",
                    "light_blue",
                    "yellow",
                    "lime",
                    "pink",
                    "gray",
                    "light_gray",
                    "cyan",
                    "purple",
                    "blue",
                    "brown",
                    "green",
                    "red",
                    "black",
                ];
                let name = usize::try_from(color)
                    .ok()
                    .and_then(|i| colors.get(i))
                    .copied()
                    .unwrap_or("white");
                set(layer, "color", Tag::String(name.into()));
                remove(layer, "Color");
            }
        }
        set(tag, "patterns", patterns);
    }
    Ok(())
}

fn pattern_name(name: &str) -> Option<&'static str> {
    Some(match name {
        "b" => "minecraft:base",
        "bl" => "minecraft:square_bottom_left",
        "br" => "minecraft:square_bottom_right",
        "tl" => "minecraft:square_top_left",
        "tr" => "minecraft:square_top_right",
        "bs" => "minecraft:stripe_bottom",
        "ts" => "minecraft:stripe_top",
        "ls" => "minecraft:stripe_left",
        "rs" => "minecraft:stripe_right",
        "cs" => "minecraft:stripe_center",
        "ms" => "minecraft:stripe_middle",
        "drs" => "minecraft:stripe_downright",
        "dls" => "minecraft:stripe_downleft",
        "ss" => "minecraft:small_stripes",
        "cr" => "minecraft:cross",
        "sc" => "minecraft:straight_cross",
        "bt" => "minecraft:triangle_bottom",
        "tt" => "minecraft:triangle_top",
        "bts" => "minecraft:triangles_bottom",
        "tts" => "minecraft:triangles_top",
        "ld" => "minecraft:diagonal_left",
        "rd" => "minecraft:diagonal_up_right",
        "lud" => "minecraft:diagonal_up_left",
        "rud" => "minecraft:diagonal_right",
        "mc" => "minecraft:circle",
        "mr" => "minecraft:rhombus",
        "vh" => "minecraft:half_vertical",
        "hh" => "minecraft:half_horizontal",
        "vhr" => "minecraft:half_vertical_right",
        "hhb" => "minecraft:half_horizontal_bottom",
        "bo" => "minecraft:border",
        "cbo" => "minecraft:curly_border",
        "gra" => "minecraft:gradient",
        "gru" => "minecraft:gradient_up",
        "bri" => "minecraft:bricks",
        "glb" => "minecraft:globe",
        "cre" => "minecraft:creeper",
        "sku" => "minecraft:skull",
        "flo" => "minecraft:flower",
        "moj" => "minecraft:mojang",
        "pig" => "minecraft:piglin",
        _ => return None,
    })
}

fn valid_name(name: &str) -> bool {
    name.len() <= 16 && name.bytes().all(|ch| ch > 32 && ch < 127)
}

fn profile(mut owner: Tag) -> Tag {
    let mut result = Tag::Compound(Vec::new());
    if let Some(name) = string(&owner) {
        if valid_name(name) {
            set(&mut result, "name", Tag::String(name.into()));
        }
        return result;
    }
    if let Some(name) = take(&mut owner, "Name") {
        if string(&name).is_some_and(valid_name) {
            set(&mut result, "name", name);
        } else if !matches!(name, Tag::String(_)) {
            set(&mut result, "name", Tag::String(String::new()));
        }
    } else {
        set(&mut result, "name", Tag::String(String::new()));
    }
    if let Some(id) = take(&mut owner, "Id") {
        set(&mut result, "id", id);
    }
    if let Some(Tag::Compound(properties)) = take(&mut owner, "Properties")
        && !properties.is_empty()
    {
        let mut converted = Vec::new();
        for (name, values) in properties {
            if let Tag::List(values) = values {
                for value in values.elements {
                    let mut property = compound(vec![
                        ("name", Tag::String(name.clone())),
                        (
                            "value",
                            Tag::String(get(&value, "Value").and_then(string).unwrap_or("").into()),
                        ),
                    ]);
                    if let Some(signature) = get(&value, "Signature").and_then(string) {
                        set(&mut property, "signature", Tag::String(signature.into()));
                    }
                    converted.push(property);
                }
            }
        }
        set(
            &mut result,
            "properties",
            Tag::List(ListTag {
                element_type: if converted.is_empty() { 0 } else { 10 },
                elements: converted,
            }),
        );
    }
    result
}

fn banner_name(tag: &mut Tag, version: i32) -> Result<(), UpgradeError> {
    let Some(name) = get(tag, "CustomName") else {
        return Ok(());
    };
    let source = string(name).ok_or_else(|| {
        unsupported(
            version,
            "BannerEntityCustomNameToOverrideComponentFix",
            "CustomName",
            "expected serialized text component",
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(source).map_err(|_| {
        unsupported(
            version,
            "BannerEntityCustomNameToOverrideComponentFix",
            "CustomName",
            "lenient JSON text translation extraction is unsupported",
        )
    })?;
    if value.get("translate").and_then(serde_json::Value::as_str)
        == Some("block.minecraft.ominous_banner")
    {
        let name = take(tag, "CustomName").expect("checked above");
        set(
            tag,
            "components",
            compound(vec![
                ("minecraft:item_name", name),
                (
                    "minecraft:hide_additional_tooltip",
                    Tag::Compound(Vec::new()),
                ),
            ]),
        );
    }
    Ok(())
}

fn banner_rarity(tag: &mut Tag, version: i32) -> Result<(), UpgradeError> {
    let Some(components) = get_mut(tag, "components") else {
        return Ok(());
    };
    let Some(name) = get(components, "minecraft:item_name") else {
        return Ok(());
    };
    let source = string(name).ok_or_else(|| {
        unsupported(
            version,
            "OminousBannerRarityFix",
            "components.minecraft:item_name",
            "expected serialized text component",
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(source).map_err(|_| {
        unsupported(
            version,
            "OminousBannerRarityFix",
            "components.minecraft:item_name",
            "lenient JSON translation extraction is unsupported",
        )
    })?;
    if value.get("translate").and_then(serde_json::Value::as_str)
        == Some("block.minecraft.ominous_banner")
    {
        set(
            components,
            "minecraft:item_name",
            Tag::String("{\"translate\":\"block.minecraft.ominous_banner\"}".into()),
        );
        set(
            components,
            "minecraft:rarity",
            Tag::String("uncommon".into()),
        );
    }
    Ok(())
}

fn single_field<'a>(tag: &'a Tag, key: &str) -> Option<&'a Tag> {
    let fields = fields(tag)?;
    if fields.len() == 1 && fields[0].0 == key {
        Some(&fields[0].1)
    } else {
        None
    }
}

fn broken_lock(tag: &Tag) -> bool {
    single_field(tag, "components")
        .and_then(|components| single_field(components, "minecraft:custom_name"))
        .and_then(string)
        == Some("\"\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(id: &str, mut data: Vec<(&str, Tag)>) -> Tag {
        data.insert(0, ("id", Tag::String(format!("minecraft:{id}"))));
        compound(data)
    }
    fn apply(tag: &mut Tag, version: i32) {
        fix_node(
            tag,
            NodeKind::BlockEntity,
            version,
            &mut UpgradeReport::default(),
        )
        .unwrap();
    }
    fn strings(values: &[&str]) -> Tag {
        Tag::List(ListTag {
            element_type: 8,
            elements: values
                .iter()
                .map(|value| Tag::String((*value).into()))
                .collect(),
        })
    }
    fn compounds(values: Vec<Tag>) -> Tag {
        Tag::List(ListTag {
            element_type: 10,
            elements: values,
        })
    }

    #[test]
    fn filtered_signs_3097_only_drops_filtered_fields() {
        let mut tag = block(
            "sign",
            vec![
                ("Text1", Tag::String("hello".into())),
                ("FilteredText1", Tag::String("filtered".into())),
            ],
        );
        apply(&mut tag, 3097);
        assert!(get(&tag, "FilteredText1").is_none());
        assert_eq!(get(&tag, "Text1"), Some(&Tag::String("hello".into())));
    }
    #[test]
    fn brushable_3438_renames_choice_and_loot_fields() {
        let mut tag = block(
            "suspicious_sand",
            vec![
                ("loot_table", Tag::String("custom:loot".into())),
                ("loot_table_seed", Tag::Long(9)),
            ],
        );
        apply(&mut tag, 3438);
        assert_eq!(
            get(&tag, "id").and_then(string),
            Some("minecraft:brushable_block")
        );
        assert_eq!(get(&tag, "LootTable").and_then(string), Some("custom:loot"));
        assert_eq!(get(&tag, "LootTableSeed"), Some(&Tag::Long(9)));
        assert!(get(&tag, "loot_table").is_none());
    }
    #[test]
    fn sign_3439_constructs_both_sides_and_filtered_fallbacks() {
        for id in ["sign", "hanging_sign"] {
            let mut tag = block(
                id,
                vec![
                    ("Text1", Tag::String("first".into())),
                    ("FilteredText2", Tag::String("second".into())),
                    ("Color", Tag::String("red".into())),
                ],
            );
            apply(&mut tag, 3439);
            let front = get(&tag, "front_text").unwrap();
            assert_eq!(
                get(front, "messages"),
                Some(&strings(&["first", EMPTY_TEXT, EMPTY_TEXT, EMPTY_TEXT]))
            );
            assert_eq!(
                get(front, "filtered_messages"),
                Some(&strings(&["first", "second", EMPTY_TEXT, EMPTY_TEXT]))
            );
            assert_eq!(get(front, "color").and_then(string), Some("red"));
            assert_eq!(get(&tag, "is_waxed"), Some(&Tag::Byte(0)));
            assert!(get(&tag, "Text1").is_none());
            assert_eq!(
                get(get(&tag, "back_text").unwrap(), "messages"),
                Some(&strings(&[EMPTY_TEXT; 4]))
            );
        }
    }
    #[test]
    fn sign_3439_rejects_unrepresentable_dynamic_lists() {
        let mut tag = block("sign", vec![("Text1", Tag::Int(4))]);
        let before = tag.clone();
        assert!(
            fix_node(
                &mut tag,
                NodeKind::BlockEntity,
                3439,
                &mut UpgradeReport::default()
            )
            .is_err()
        );
        assert_eq!(tag, before);
    }
    #[test]
    fn pottery_3438_renames_only_the_four_registered_item_names() {
        let mut tag = block(
            "decorated_pot",
            vec![
                (
                    "shards",
                    strings(&[
                        "minecraft:pottery_shard_archer",
                        "minecraft:pottery_shard_prize",
                        "minecraft:pottery_shard_arms_up",
                        "minecraft:pottery_shard_skull",
                        "mod:pottery_shard_archer",
                        "minecraft:brick",
                    ]),
                ),
                ("opaque", strings(&["minecraft:pottery_shard_archer"])),
            ],
        );
        apply(&mut tag, 3438);
        assert_eq!(
            get(&tag, "shards"),
            Some(&strings(&[
                "minecraft:archer_pottery_shard",
                "minecraft:prize_pottery_shard",
                "minecraft:arms_up_pottery_shard",
                "minecraft:skull_pottery_shard",
                "mod:pottery_shard_archer",
                "minecraft:brick",
            ]))
        );
        assert_eq!(
            get(&tag, "opaque"),
            Some(&strings(&["minecraft:pottery_shard_archer"]))
        );
    }
    #[test]
    fn pottery_3447_renames_the_twenty_registered_shards() {
        let names = [
            "angler",
            "archer",
            "arms_up",
            "blade",
            "brewer",
            "burn",
            "danger",
            "explorer",
            "friend",
            "heart",
            "heartbreak",
            "howl",
            "miner",
            "mourner",
            "plenty",
            "prize",
            "sheaf",
            "shelter",
            "skull",
            "snort",
        ];
        let old = Tag::List(ListTag {
            element_type: 8,
            elements: names
                .iter()
                .map(|name| Tag::String(format!("minecraft:{name}_pottery_shard")))
                .collect(),
        });
        let expected = Tag::List(ListTag {
            element_type: 8,
            elements: names
                .iter()
                .map(|name| Tag::String(format!("minecraft:{name}_pottery_sherd")))
                .collect(),
        });
        let mut tag = block("decorated_pot", vec![("shards", old)]);
        apply(&mut tag, 3447);
        assert_eq!(get(&tag, "shards"), Some(&expected));
        apply(&mut tag, 3448);
        assert_eq!(get(&tag, "sherds"), Some(&expected));
    }
    #[test]
    fn decorated_pot_3448_rejects_untyped_shards() {
        let mut tag = block("decorated_pot", vec![("shards", Tag::Int(3))]);
        let error = fix_node(
            &mut tag,
            NodeKind::BlockEntity,
            3448,
            &mut UpgradeReport::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            UpgradeError::UnsupportedField {
                fix: "DecoratedPotFieldRenameFix",
                ..
            }
        ));
    }
    #[test]
    fn pottery_item_names_fail_closed_for_untyped_lists() {
        for version in [3438, 3447] {
            let mut tag = block("decorated_pot", vec![("shards", Tag::Int(3))]);
            let error = fix_node(
                &mut tag,
                NodeKind::BlockEntity,
                version,
                &mut UpgradeReport::default(),
            )
            .unwrap_err();
            assert!(matches!(
                error,
                UpgradeError::UnsupportedField {
                    fix: "ItemRenameFix",
                    ..
                }
            ));
        }
    }
    #[test]
    fn decorated_pot_3448_renames_typed_sherds() {
        let mut tag = block(
            "decorated_pot",
            vec![("shards", strings(&["minecraft:brick"]))],
        );
        apply(&mut tag, 3448);
        assert!(get(&tag, "shards").is_none());
        assert_eq!(get(&tag, "sherds"), Some(&strings(&["minecraft:brick"])));
    }
    #[test]
    fn sign_3564_repairs_empty_filtered_lines_or_removes_marker() {
        let mut tag = block(
            "sign",
            vec![
                (
                    "front_text",
                    compound(vec![
                        ("messages", strings(&["one", "two"])),
                        ("filtered_messages", strings(&[EMPTY_TEXT, EMPTY_TEXT])),
                    ]),
                ),
                ("Text1", Tag::String("obsolete".into())),
            ],
        );
        apply(&mut tag, 3564);
        assert!(get(get(&tag, "front_text").unwrap(), "filtered_messages").is_none());
        assert!(get(&tag, "Text1").is_none());
        let mut marked = block(
            "hanging_sign",
            vec![
                ("_filtered_correct", Tag::Byte(1)),
                ("Text1", Tag::String("left alone".into())),
            ],
        );
        apply(&mut marked, 3564);
        assert!(get(&marked, "_filtered_correct").is_none());
        assert!(get(&marked, "Text1").is_some());
    }
    #[test]
    fn sign_3564_rejects_unrepresentable_filtered_repairs() {
        let mut tag = block(
            "sign",
            vec![(
                "front_text",
                compound(vec![
                    (
                        "messages",
                        Tag::List(ListTag {
                            element_type: 3,
                            elements: vec![Tag::Int(1), Tag::Int(2)],
                        }),
                    ),
                    ("filtered_messages", strings(&[EMPTY_TEXT, "filtered"])),
                ]),
            )],
        );
        let before = tag.clone();
        let error = fix_node(
            &mut tag,
            NodeKind::BlockEntity,
            3564,
            &mut UpgradeReport::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            UpgradeError::UnsupportedField {
                version: 3564,
                fix: "DropInvalidSignDataFix",
                ..
            }
        ));
        assert_eq!(tag, before);
    }
    #[test]
    fn block_positions_3813_support_both_blockentity_choices() {
        for (id, old, new) in [
            ("beehive", "FlowerPos", "flower_pos"),
            ("end_gateway", "ExitPortal", "exit_portal"),
        ] {
            let mut tag = block(
                id,
                vec![(
                    old,
                    compound(vec![
                        ("X", Tag::Int(-2)),
                        ("Y", Tag::Short(64)),
                        ("Z", Tag::Long(7)),
                    ]),
                )],
            );
            apply(&mut tag, 3813);
            assert_eq!(get(&tag, new), Some(&Tag::IntArray(vec![-2, 64, 7])));
            assert!(get(&tag, old).is_none());
        }
    }
    #[test]
    fn beehive_3818_renames_typed_entity_data_and_timing() {
        let entity = compound(vec![("id", Tag::String("minecraft:bee".into()))]);
        let mut tag = block(
            "beehive",
            vec![(
                "Bees",
                compounds(vec![compound(vec![
                    ("EntityData", entity.clone()),
                    ("TicksInHive", Tag::Int(8)),
                    ("MinOccupationTicks", Tag::Int(12)),
                    ("unknown", Tag::Byte(3)),
                ])]),
            )],
        );
        apply(&mut tag, 3818);
        assert!(get(&tag, "Bees").is_none());
        let bee = &get(&tag, "bees").and_then(list).unwrap()[0];
        assert_eq!(get(bee, "entity_data"), Some(&entity));
        assert_eq!(get(bee, "ticks_in_hive"), Some(&Tag::Int(8)));
        assert_eq!(get(bee, "min_ticks_in_hive"), Some(&Tag::Int(12)));
        assert_eq!(get(bee, "unknown"), Some(&Tag::Byte(3)));
    }
    #[test]
    fn banner_3818_formats_patterns_and_preserves_unknown_pattern_ids() {
        let mut tag = block(
            "banner",
            vec![(
                "Patterns",
                compounds(vec![
                    compound(vec![
                        ("Pattern", Tag::String("cre".into())),
                        ("Color", Tag::Int(14)),
                    ]),
                    compound(vec![
                        ("Pattern", Tag::String("mod:pattern".into())),
                        ("Color", Tag::Int(98)),
                    ]),
                ]),
            )],
        );
        apply(&mut tag, 3818);
        let layers = get(&tag, "patterns").and_then(list).unwrap();
        assert_eq!(
            get(&layers[0], "pattern").and_then(string),
            Some("minecraft:creeper")
        );
        assert_eq!(get(&layers[0], "color").and_then(string), Some("red"));
        assert_eq!(
            get(&layers[1], "pattern").and_then(string),
            Some("mod:pattern")
        );
        assert_eq!(get(&layers[1], "color").and_then(string), Some("white"));
    }
    #[test]
    fn typed_beehive_and_banner_lists_fail_closed_when_malformed() {
        for (id, key) in [("beehive", "Bees"), ("banner", "Patterns")] {
            let mut tag = block(id, vec![(key, Tag::Int(1))]);
            assert!(
                fix_node(
                    &mut tag,
                    NodeKind::BlockEntity,
                    3818,
                    &mut UpgradeReport::default()
                )
                .is_err()
            );
        }
    }
    #[test]
    fn player_head_3820_converts_name_uuid_and_signed_properties() {
        let owner = compound(vec![
            ("Name", Tag::String("Steve".into())),
            ("Id", Tag::IntArray(vec![1, 2, 3, 4])),
            (
                "Properties",
                compound(vec![(
                    "textures",
                    compounds(vec![compound(vec![
                        ("Value", Tag::String("skin".into())),
                        ("Signature", Tag::String("signed".into())),
                    ])]),
                )]),
            ),
        ]);
        let mut tag = block(
            "skull",
            vec![
                ("SkullOwner", owner),
                ("ExtraType", Tag::String("ignored".into())),
            ],
        );
        apply(&mut tag, 3820);
        let profile = get(&tag, "profile").unwrap();
        assert_eq!(get(profile, "name").and_then(string), Some("Steve"));
        assert_eq!(get(profile, "id"), Some(&Tag::IntArray(vec![1, 2, 3, 4])));
        assert_eq!(
            get(
                &get(profile, "properties").and_then(list).unwrap()[0],
                "signature"
            )
            .and_then(string),
            Some("signed")
        );
        assert!(get(&tag, "SkullOwner").is_none());
        assert!(get(&tag, "ExtraType").is_none());
        let mut tag = block("skull", vec![("ExtraType", Tag::String("bad name".into()))]);
        apply(&mut tag, 3820);
        assert_eq!(get(&tag, "profile"), Some(&Tag::Compound(Vec::new())));
    }
    #[test]
    fn ominous_banner_3825_moves_only_matching_translation() {
        let source = "{\"translate\":\"block.minecraft.ominous_banner\",\"color\":\"gold\"}";
        let mut tag = block("banner", vec![("CustomName", Tag::String(source.into()))]);
        apply(&mut tag, 3825);
        assert!(get(&tag, "CustomName").is_none());
        assert_eq!(
            get(get(&tag, "components").unwrap(), "minecraft:item_name").and_then(string),
            Some(source)
        );
        let mut plain = block(
            "banner",
            vec![("CustomName", Tag::String("{\"text\":\"ordinary\"}".into()))],
        );
        let before = plain.clone();
        apply(&mut plain, 3825);
        assert_eq!(plain, before);
    }
    #[test]
    fn banner_3825_rejects_unsupported_lenient_json() {
        let mut tag = block(
            "banner",
            vec![(
                "CustomName",
                Tag::String("{translate:'block.minecraft.ominous_banner'}".into()),
            )],
        );
        assert!(
            fix_node(
                &mut tag,
                NodeKind::BlockEntity,
                3825,
                &mut UpgradeReport::default()
            )
            .is_err()
        );
    }
    #[test]
    fn trial_spawner_3825_moves_config_fields_not_runtime_fields() {
        let mut tag = block(
            "trial_spawner",
            vec![
                ("spawn_range", Tag::Int(4)),
                ("total_mobs", Tag::Float(6.0)),
                ("next_mob_spawns_at", Tag::Long(25)),
                ("custom", Tag::Byte(3)),
            ],
        );
        apply(&mut tag, 3825);
        assert_eq!(
            get(get(&tag, "normal_config").unwrap(), "spawn_range"),
            Some(&Tag::Int(4))
        );
        assert!(get(&tag, "spawn_range").is_none());
        assert_eq!(get(&tag, "next_mob_spawns_at"), Some(&Tag::Long(25)));
        assert_eq!(get(&tag, "custom"), Some(&Tag::Byte(3)));
    }
    #[test]
    fn brushable_3833_removes_only_empty_modern_item() {
        let mut tag = block(
            "brushable_block",
            vec![(
                "item",
                compound(vec![
                    ("id", Tag::String("minecraft:stone".into())),
                    ("count", Tag::Int(0)),
                ]),
            )],
        );
        apply(&mut tag, 3833);
        assert!(get(&tag, "item").is_none());
        set(
            &mut tag,
            "item",
            compound(vec![
                ("id", Tag::String("stone".into())),
                ("count", Tag::Int(2)),
            ]),
        );
        apply(&mut tag, 3833);
        assert!(get(&tag, "item").is_some());
    }
    #[test]
    fn jukebox_3945_subtracts_ticks_and_removes_legacy_playback_fields() {
        for (tick, start, expected) in [
            (100, 20, Some(Tag::Long(80))),
            (20, 100, None),
            (i64::MIN, 1, Some(Tag::Long(i64::MAX))),
        ] {
            let mut tag = block(
                "jukebox",
                vec![
                    ("TickCount", Tag::Long(tick)),
                    ("RecordStartTick", Tag::Long(start)),
                    ("IsPlaying", Tag::Byte(1)),
                ],
            );
            apply(&mut tag, 3945);
            assert_eq!(get(&tag, "ticks_since_song_started"), expected.as_ref());
            assert!(get(&tag, "TickCount").is_none());
            assert!(get(&tag, "RecordStartTick").is_none());
            assert!(get(&tag, "IsPlaying").is_none());
        }
    }
    #[test]
    fn trial_spawner_4061_preserves_unmatched_inline_pairs_and_unaffected_shapes() {
        let mut tag = block(
            "trial_spawner",
            vec![
                ("normal_config", Tag::Compound(Vec::new())),
                ("ominous_config", Tag::Compound(Vec::new())),
            ],
        );
        let before = tag.clone();
        apply(&mut tag, 4061);
        assert_eq!(tag, before);
        remove(&mut tag, "ominous_config");
        let before = tag.clone();
        apply(&mut tag, 4061);
        assert_eq!(tag, before);
        set(
            &mut tag,
            "normal_config",
            Tag::String("minecraft:trial_chamber/melee/zombie/normal".into()),
        );
        set(
            &mut tag,
            "ominous_config",
            Tag::String("minecraft:trial_chamber/melee/zombie/ominous".into()),
        );
        let before = tag.clone();
        apply(&mut tag, 4061);
        assert_eq!(tag, before);
    }
    #[test]
    fn container_lock_4068_escapes_exactly_quotes_and_backslashes() {
        let mut tag = block("chest", vec![("Lock", Tag::String("a\"b\\c".into()))]);
        apply(&mut tag, 4068);
        let components = get(get(&tag, "lock").unwrap(), "components").unwrap();
        assert_eq!(
            get(components, "minecraft:custom_name").and_then(string),
            Some("\"a\\\"b\\\\c\"")
        );
        assert!(get(&tag, "Lock").is_none());
        let mut empty = block("barrel", vec![("Lock", Tag::String(String::new()))]);
        apply(&mut empty, 4068);
        assert!(get(&empty, "Lock").is_none());
        assert!(get(&empty, "lock").is_none());
    }
    #[test]
    fn invalid_lock_4176_removes_only_exact_empty_predicate() {
        let lock = compound(vec![(
            "components",
            compound(vec![("minecraft:custom_name", Tag::String("\"\"".into()))]),
        )]);
        let mut tag = block("chest", vec![("lock", lock.clone())]);
        apply(&mut tag, 4176);
        assert!(get(&tag, "lock").is_none());
        let mut extra = lock;
        set(&mut extra, "items", Tag::String("minecraft:stone".into()));
        set(&mut tag, "lock", extra.clone());
        apply(&mut tag, 4176);
        assert_eq!(get(&tag, "lock"), Some(&extra));
    }
    #[test]
    fn furnace_4181_renames_all_times_and_copies_remaining_time() {
        for id in ["furnace", "smoker", "blast_furnace"] {
            let mut tag = block(
                id,
                vec![
                    ("CookTime", Tag::Short(10)),
                    ("CookTimeTotal", Tag::Int(200)),
                    ("BurnTime", Tag::Short(40)),
                ],
            );
            apply(&mut tag, 4181);
            assert_eq!(get(&tag, "cooking_time_spent"), Some(&Tag::Short(10)));
            assert_eq!(get(&tag, "cooking_total_time"), Some(&Tag::Int(200)));
            assert_eq!(get(&tag, "lit_time_remaining"), Some(&Tag::Short(40)));
            assert_eq!(get(&tag, "lit_total_time"), Some(&Tag::Short(40)));
            assert!(get(&tag, "BurnTime").is_none());
        }
    }
    #[test]
    fn same_integer_sources_skip_revision_zero_but_apply_later_revisions() {
        // Source keys are DataVersion*10. Signs are revision0; hanging signs revision1.
        for version in [3439, 3564] {
            let mut sign = block(
                "sign",
                vec![
                    ("Text1", Tag::String("revision-zero".into())),
                    ("_filtered_correct", Tag::Byte(1)),
                ],
            );
            let before = sign.clone();
            let mut report = UpgradeReport {
                from: version,
                ..UpgradeReport::default()
            };
            fix_node(&mut sign, NodeKind::BlockEntity, version, &mut report).unwrap();
            assert_eq!(sign, before);
            assert!(report.steps.is_empty());

            let mut hanging = block(
                "hanging_sign",
                vec![
                    ("Text1", Tag::String("revision-one".into())),
                    ("_filtered_correct", Tag::Byte(1)),
                ],
            );
            fix_node(&mut hanging, NodeKind::BlockEntity, version, &mut report).unwrap();
            if version == 3439 {
                assert!(get(&hanging, "front_text").is_some());
            } else {
                assert!(get(&hanging, "_filtered_correct").is_none());
            }
            assert!(report.steps.iter().any(|step| step.version == version));
        }
        // Beehive is revision0; BannerPatternFormatFix is3818 revision1.
        let mut hive = block(
            "beehive",
            vec![(
                "Bees",
                compounds(vec![compound(vec![("TicksInHive", Tag::Int(5))])]),
            )],
        );
        let before = hive.clone();
        let mut report = UpgradeReport {
            from: 3818,
            ..UpgradeReport::default()
        };
        fix_node(&mut hive, NodeKind::BlockEntity, 3818, &mut report).unwrap();
        assert_eq!(hive, before);
        assert!(report.steps.is_empty());
        let mut banner = block(
            "banner",
            vec![(
                "Patterns",
                compounds(vec![compound(vec![("Pattern", Tag::String("cre".into()))])]),
            )],
        );
        fix_node(&mut banner, NodeKind::BlockEntity, 3818, &mut report).unwrap();
        assert!(get(&banner, "Patterns").is_none());
        assert!(get(&banner, "patterns").is_some());
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.fix == "BannerPatternFormatFix")
        );
    }
    #[test]
    fn unknown_choices_opaque_fields_and_inline_blockpos_are_unchanged() {
        let mut unknown = compound(vec![
            ("id", Tag::String("mod:chest".into())),
            ("Lock", Tag::String("secret".into())),
            ("Text1", Tag::String("untouched".into())),
        ]);
        let before = unknown.clone();
        for version in [
            3097, 3438, 3439, 3448, 3564, 3813, 3818, 3820, 3825, 3833, 3945, 4061, 4068, 4176,
            4181, 4314,
        ] {
            apply(&mut unknown, version);
        }
        assert_eq!(unknown, before);
        let mut known = block(
            "chest",
            vec![
                (
                    "custom",
                    compound(vec![("Lock", Tag::String("untouched".into()))]),
                ),
                ("x", Tag::Int(1)),
                ("y", Tag::Int(2)),
                ("z", Tag::Int(3)),
            ],
        );
        let before = known.clone();
        apply(&mut known, 4314);
        assert_eq!(known, before);
        fix_node(
            &mut known,
            NodeKind::Entity,
            4068,
            &mut UpgradeReport::default(),
        )
        .unwrap();
        assert_eq!(known, before);
    }
}
