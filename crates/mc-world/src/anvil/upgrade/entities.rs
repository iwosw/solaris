//! Entity/player remainder fixes. Evidence: `.analysis/old-world-upgrade/*.javap.txt`.
//! Only the explicitly named schema fields below are visited; opaque custom data is untouched.

use mc_nbt::{ListTag, Tag};

use super::nbt::{
    compound, fields, get, get_mut, list_mut, number, remove, rename, set, string, take,
};
use super::{NodeKind, UpgradeError, UpgradeReport};

mod particles;

const EFFECTS: [&str; 33] = [
    "minecraft:speed",
    "minecraft:slowness",
    "minecraft:haste",
    "minecraft:mining_fatigue",
    "minecraft:strength",
    "minecraft:instant_health",
    "minecraft:instant_damage",
    "minecraft:jump_boost",
    "minecraft:nausea",
    "minecraft:regeneration",
    "minecraft:resistance",
    "minecraft:fire_resistance",
    "minecraft:water_breathing",
    "minecraft:invisibility",
    "minecraft:blindness",
    "minecraft:night_vision",
    "minecraft:hunger",
    "minecraft:weakness",
    "minecraft:poison",
    "minecraft:wither",
    "minecraft:health_boost",
    "minecraft:absorption",
    "minecraft:saturation",
    "minecraft:glowing",
    "minecraft:levitation",
    "minecraft:luck",
    "minecraft:unluck",
    "minecraft:slow_falling",
    "minecraft:conduit_power",
    "minecraft:dolphins_grace",
    "minecraft:bad_omen",
    "minecraft:hero_of_the_village",
    "minecraft:darkness",
];
const ARMOR_SLOTS: [&str; 4] = ["feet", "legs", "chest", "head"];
const HAND_SLOTS: [&str; 2] = ["mainhand", "offhand"];

fn text(value: &str) -> Tag {
    Tag::String(value.to_owned())
}

// Java Number.intValue: integral narrowing wraps; floating-point narrowing saturates.
fn java_int(value: &Tag) -> Option<i32> {
    match value {
        Tag::Byte(v) => Some(i32::from(*v)),
        Tag::Short(v) => Some(i32::from(*v)),
        Tag::Int(v) => Some(*v),
        Tag::Long(v) => Some(*v as i32),
        Tag::Float(v) => Some(*v as i32),
        Tag::Double(v) => Some(*v as i32),
        _ => None,
    }
}

fn named(value: Option<&Tag>, expected: &str) -> bool {
    value.and_then(string).is_some_and(|value| {
        value == expected
            || (!value.contains(':') && expected.strip_prefix("minecraft:") == Some(value))
    })
}

fn has_any(tag: &Tag, keys: &[&str]) -> bool {
    keys.iter().any(|key| get(tag, key).is_some())
}

fn reject(version: i32, fix: &'static str, path: &str, reason: &str) -> UpgradeError {
    UpgradeError::unsupported(version, fix, path, reason)
}

fn list_field<'a>(
    tag: &'a mut Tag,
    key: &str,
    version: i32,
    fix: &'static str,
) -> Result<Option<&'a mut Vec<Tag>>, UpgradeError> {
    let Some(value) = get_mut(tag, key) else {
        return Ok(None);
    };
    let values = list_mut(value)
        .ok_or_else(|| reject(version, fix, key, "affected field is not an NBT list"))?;
    if !matches!(key, "ArmorDropChances" | "HandDropChances")
        && values.iter().any(|value| fields(value).is_none())
    {
        return Err(reject(
            version,
            fix,
            key,
            "typed list entry is not an NBT compound",
        ));
    }
    Ok(Some(values))
}

/// EffectDurationFix3322 changes only FactorCalculationData on the immediate instance,
/// not HiddenEffect (the vanilla duration fixer does not recurse there).
pub(super) fn fix_effect_duration(effect: &mut Tag) {
    let duration = get(effect, "Duration").and_then(java_int).unwrap_or(-1);
    if let Some(factor) = get_mut(effect, "FactorCalculationData") {
        let timestamp = get(factor, "effect_changed_timestamp")
            .and_then(java_int)
            .unwrap_or(-1);
        remove(factor, "effect_changed_timestamp");
        set(
            factor,
            "ticks_active",
            Tag::Int(timestamp.wrapping_sub(duration)),
        );
    }
}

fn converted_effect_id(value: &Tag, path: &str) -> Result<&'static str, UpgradeError> {
    let id = java_int(value).ok_or_else(|| {
        reject(
            3568,
            "MobEffectIdFix",
            path,
            "legacy effect ID is not numeric",
        )
    })?;
    usize::try_from(id.wrapping_sub(1))
        .ok()
        .and_then(|index| EFFECTS.get(index))
        .copied()
        .ok_or_else(|| {
            reject(
                3568,
                "MobEffectIdFix",
                path,
                "unregistered numeric effect ID cannot be losslessly upgraded",
            )
        })
}

fn rename_effect_id(tag: &mut Tag, old: &str, new: &str) -> Result<(), UpgradeError> {
    if let Some(value) = get(tag, old) {
        let id = converted_effect_id(value, old)?;
        remove(tag, old);
        set(tag, new, text(id));
    }
    Ok(())
}

pub(super) fn fix_effect_id(effect: &mut Tag) -> Result<(), UpgradeError> {
    if fields(effect).is_none() {
        return Err(reject(
            3568,
            "MobEffectIdFix",
            "HiddenEffect",
            "effect instance is not a compound",
        ));
    }
    rename_effect_id(effect, "Id", "id")?;
    for (old, new) in [
        ("Ambient", "ambient"),
        ("Amplifier", "amplifier"),
        ("Duration", "duration"),
        ("ShowParticles", "show_particles"),
        ("ShowIcon", "show_icon"),
    ] {
        rename(effect, old, new);
    }
    if let Some(hidden) = get_mut(effect, "HiddenEffect") {
        fix_effect_id(hidden)?;
    }
    rename(effect, "HiddenEffect", "hidden_effect");
    Ok(())
}

fn effect_list(tag: &mut Tag, old: &str, new: &str, version: i32) -> Result<bool, UpgradeError> {
    let fix = if version == 3322 {
        "EffectDurationFix"
    } else {
        "MobEffectIdFix"
    };
    let Some(effects) = list_field(tag, old, version, fix)? else {
        return Ok(false);
    };
    for effect in effects {
        if fields(effect).is_none() {
            return Err(reject(
                version,
                fix,
                old,
                "effect instance is not a compound",
            ));
        }
        if version == 3322 {
            if get(effect, "FactorCalculationData").is_some_and(|factor| fields(factor).is_none()) {
                return Err(reject(
                    version,
                    fix,
                    "FactorCalculationData",
                    "factor calculation data is not a compound",
                ));
            }
            fix_effect_duration(effect);
        } else {
            fix_effect_id(effect)?;
        }
    }
    if old != new {
        rename(tag, old, new);
    }
    Ok(true)
}

/// Called by the Item slice with an entire Item stack, before componentization.
pub(super) fn fix_potion_effects(
    tag: &mut Tag,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    let potion = [
        "minecraft:potion",
        "minecraft:splash_potion",
        "minecraft:lingering_potion",
        "minecraft:tipped_arrow",
    ]
    .iter()
    .any(|id| named(get(tag, "id"), id));
    let stew = named(get(tag, "id"), "minecraft:suspicious_stew");
    let Some(item_tag) = get_mut(tag, "tag") else {
        return Ok(());
    };
    let changed = if potion {
        match version {
            3322 => effect_list(
                item_tag,
                "CustomPotionEffects",
                "CustomPotionEffects",
                version,
            )?,
            3568 => effect_list(
                item_tag,
                "CustomPotionEffects",
                "custom_potion_effects",
                version,
            )?,
            _ => false,
        }
    } else if stew && version == 3568 {
        if let Some(effects) = list_field(item_tag, "Effects", version, "MobEffectIdFix")? {
            for effect in effects {
                rename_effect_id(effect, "EffectId", "id")?;
                rename(effect, "EffectDuration", "duration");
            }
            rename(item_tag, "Effects", "effects");
            true
        } else {
            false
        }
    } else {
        false
    };
    if changed {
        report.applied(
            version,
            if version == 3322 {
                "EffectDurationFix"
            } else {
                "MobEffectIdFix"
            },
        );
    }
    Ok(())
}

fn variant(
    tag: &mut Tag,
    old: &str,
    variants: &[&str],
    default: Option<&str>,
    version: i32,
) -> Result<(), UpgradeError> {
    if let Some(value) = get(tag, old) {
        // EntityVariantFix.updateAndRename sets variant but intentionally retains the old field.
        let replacement = if let Some(id) = java_int(value) {
            let mapped = usize::try_from(id)
                .ok()
                .and_then(|index| variants.get(index))
                .copied()
                .or(default)
                .ok_or_else(|| {
                    reject(
                        version,
                        "EntityVariantFix",
                        old,
                        "unregistered variant ID has no vanilla mapping",
                    )
                })?;
            text(mapped)
        } else {
            value.clone()
        };
        set(tag, "variant", replacement);
    }
    Ok(())
}

fn horse_armor(tag: &mut Tag, horse: bool) -> Result<bool, UpgradeError> {
    let key = if horse { "ArmorItem" } else { "DecorItem" };
    if get(tag, key).is_none() {
        return Ok(false);
    }
    if horse {
        if let Some(Tag::List(chances)) = get(tag, "ArmorDropChances")
            && chances.elements.len() > 2
            && chances.element_type != 5
        {
            return Err(reject(
                3808,
                "HorseBodyArmorItemFix",
                "ArmorDropChances",
                "replacing a slot in a non-float list would create heterogeneous NBT",
            ));
        }
        if let Some(items) = list_field(tag, "ArmorItems", 3808, "HorseBodyArmorItemFix")?
            && let Some(item) = items.get_mut(2)
        {
            *item = compound(vec![]);
        }
        if let Some(chances) = list_field(tag, "ArmorDropChances", 3808, "HorseBodyArmorItemFix")?
            && let Some(chance) = chances.get_mut(2)
        {
            *chance = Tag::Float(0.085);
        }
    }
    if let Some(armor) = take(tag, key) {
        set(tag, "body_armor_item", armor);
        set(tag, "body_armor_drop_chance", Tag::Float(2.0));
    }
    Ok(true)
}

fn wolf_health(tag: &mut Tag) -> Result<bool, UpgradeError> {
    let mut doubled = false;
    if let Some(attributes) = list_field(tag, "Attributes", 3812, "FixWolfHealth")? {
        for attribute in attributes {
            if named(get(attribute, "Name"), "minecraft:generic.max_health")
                && let Some(base) = get_mut(attribute, "Base")
                && number(base).unwrap_or(0.0) == 20.0
            {
                *base = Tag::Double(40.0);
                doubled = true;
            }
        }
    }
    if doubled && let Some(health) = get_mut(tag, "Health") {
        *health = Tag::Float((number(health).unwrap_or(0.0) as f32) * 2.0);
    }
    Ok(doubled)
}

fn block_pos(value: &mut Tag) {
    if let (Some(x), Some(y), Some(z)) = (
        get(value, "X").and_then(java_int),
        get(value, "Y").and_then(java_int),
        get(value, "Z").and_then(java_int),
    ) {
        *value = Tag::IntArray(vec![x, y, z]);
    }
}

fn pos_field(tag: &mut Tag, old: &str, new: &str) -> bool {
    if let Some(value) = get_mut(tag, old) {
        block_pos(value);
        rename(tag, old, new);
        true
    } else {
        false
    }
}

fn entity_positions(tag: &mut Tag) -> bool {
    let mut changed = false;
    if named(get(tag, "id"), "minecraft:bee") {
        changed |= pos_field(tag, "HivePos", "hive_pos");
        changed |= pos_field(tag, "FlowerPos", "flower_pos");
    }
    if named(get(tag, "id"), "minecraft:end_crystal") {
        changed |= pos_field(tag, "BeamTarget", "beam_target");
    }
    if named(get(tag, "id"), "minecraft:wandering_trader") {
        changed |= pos_field(tag, "WanderTarget", "wander_target");
    }
    if [
        "witch",
        "ravager",
        "pillager",
        "illusioner",
        "evoker",
        "vindicator",
    ]
    .iter()
    .any(|id| {
        get(tag, "id")
            .and_then(string)
            .is_some_and(|actual| actual.strip_prefix("minecraft:").unwrap_or(actual) == *id)
    }) {
        changed |= pos_field(tag, "PatrolTarget", "patrol_target");
    }
    changed | pos_field(tag, "Leash", "leash")
}

fn legacy_attributes(tag: &mut Tag) -> Result<bool, UpgradeError> {
    let Some(attributes) = list_field(tag, "Attributes", 3814, "AttributesRenameLegacy")? else {
        return Ok(false);
    };
    for attribute in attributes {
        if named(get(attribute, "Name"), "minecraft:horse.jump_strength") {
            set(attribute, "Name", text("minecraft:generic.jump_strength"));
        }
    }
    Ok(true)
}

fn attribute_format(tag: &mut Tag) -> Result<bool, UpgradeError> {
    let Some(attributes) = list_field(tag, "Attributes", 3945, "AttributeModifierIdFix")? else {
        return Ok(false);
    };
    for attribute in attributes {
        rename(attribute, "Name", "id");
        rename(attribute, "Base", "base");
        if let Some(modifiers) = list_field(attribute, "Modifiers", 3945, "AttributeModifierIdFix")?
        {
            for modifier in modifiers {
                for (old, new) in [("UUID", "uuid"), ("Name", "name"), ("Amount", "amount")] {
                    rename(modifier, old, new);
                }
                if let Some(operation) = take(modifier, "Operation") {
                    let op = match java_int(&operation).unwrap_or(0) {
                        1 => "add_multiplied_base",
                        2 => "add_multiplied_total",
                        0 => "add_value",
                        _ => "invalid",
                    };
                    set(modifier, "operation", text(op));
                }
            }
            if let Some(modifiers) = get_mut(attribute, "Modifiers") {
                super::items::fix_modifiers(modifiers)?;
            }
        }
        rename(attribute, "Modifiers", "modifiers");
    }
    rename(tag, "Attributes", "attributes");
    Ok(true)
}

fn attribute_prefixes(tag: &mut Tag) -> Result<bool, UpgradeError> {
    let Some(attributes) = list_field(tag, "attributes", 4055, "AttributeIdPrefixFix")? else {
        return Ok(false);
    };
    for attribute in attributes {
        if let Some(Tag::String(id)) = get_mut(attribute, "id") {
            let local = id
                .strip_prefix("minecraft:")
                .or_else(|| (!id.contains(':')).then_some(id.as_str()));
            if let Some(local) = local {
                for prefix in ["generic.", "horse.", "player.", "zombie."] {
                    if let Some(suffix) = local.strip_prefix(prefix) {
                        *id = format!("minecraft:{suffix}");
                        break;
                    }
                }
            }
        }
    }
    Ok(true)
}

fn follow_range(tag: &mut Tag, old: f64, new: f64) -> Result<bool, UpgradeError> {
    let Some(attributes) = list_field(tag, "attributes", 4187, "EntityAttributeBaseValueFix")?
    else {
        return Ok(false);
    };
    for attribute in attributes {
        if named(get(attribute, "id"), "minecraft:follow_range") {
            let base = get(attribute, "base").and_then(number).unwrap_or(0.0);
            set(
                attribute,
                "base",
                Tag::Double(if base == old { new } else { base }),
            );
        }
    }
    Ok(true)
}

fn arrow_potion(tag: &mut Tag) -> Result<bool, UpgradeError> {
    if !has_any(tag, &["Potion", "custom_potion_effects", "Color"]) {
        return Ok(false);
    }
    if get(tag, "item").is_none_or(|item| fields(item).is_none()) {
        return Err(reject(
            3818,
            "TippedArrowPotionToItemFix",
            "item",
            "potion-bearing arrow lacks a typed stored item",
        ));
    }
    if get(tag, "item")
        .and_then(|item| get(item, "tag"))
        .is_some_and(|value| fields(value).is_none())
    {
        return Err(reject(
            3818,
            "TippedArrowPotionToItemFix",
            "item/tag",
            "stored item tag is not a compound",
        ));
    }
    let potion = take(tag, "Potion");
    let effects = take(tag, "custom_potion_effects");
    let color = take(tag, "Color");
    if let Some(item) = get_mut(tag, "item") {
        let mut item_tag = take(item, "tag").unwrap_or_else(|| compound(vec![]));
        if let Some(potion) = potion {
            set(&mut item_tag, "Potion", potion);
        }
        if let Some(effects) = effects {
            set(&mut item_tag, "custom_potion_effects", effects);
        }
        if let Some(color) = color {
            set(&mut item_tag, "CustomPotionColor", color);
        }
        set(item, "tag", item_tag);
    }
    Ok(true)
}

fn cloud_potion(tag: &mut Tag) -> bool {
    if !has_any(tag, &["Color", "effects", "Potion"]) {
        return false;
    }
    let mut contents = compound(vec![]);
    for (old, new) in [
        ("Color", "custom_color"),
        ("effects", "custom_effects"),
        ("Potion", "potion"),
    ] {
        if let Some(value) = take(tag, old) {
            set(&mut contents, new, value);
        }
    }
    set(tag, "potion_contents", contents);
    true
}

fn drop_chances(tag: &mut Tag) -> Result<bool, UpgradeError> {
    let affected = has_any(
        tag,
        &[
            "ArmorDropChances",
            "HandDropChances",
            "body_armor_drop_chance",
        ],
    );
    let mut chances = compound(vec![]);
    for (key, slots) in [
        ("ArmorDropChances", ARMOR_SLOTS.as_slice()),
        ("HandDropChances", HAND_SLOTS.as_slice()),
    ] {
        if let Some(values) = list_field(tag, key, 4293, "DropChancesFormatFix")? {
            for (value, slot) in values.iter().zip(slots) {
                let chance = number(value).map(|n| n as f32).unwrap_or(0.085);
                if chance != 0.085 {
                    set(&mut chances, slot, Tag::Float(chance));
                }
            }
        }
    }
    let body = get(tag, "body_armor_drop_chance")
        .and_then(number)
        .map(|n| n as f32)
        .unwrap_or(0.085);
    if body != 0.085 {
        set(&mut chances, "body", Tag::Float(body));
    }
    for key in [
        "ArmorDropChances",
        "HandDropChances",
        "body_armor_drop_chance",
    ] {
        remove(tag, key);
    }
    if fields(&chances).is_some_and(|fields| !fields.is_empty()) {
        set(tag, "drop_chances", chances);
    }
    Ok(affected)
}

fn saddle(tag: &mut Tag) -> bool {
    let mut changed = false;
    if [
        "horse",
        "skeleton_horse",
        "zombie_horse",
        "donkey",
        "mule",
        "camel",
        "llama",
        "trader_llama",
    ]
    .iter()
    .any(|id| {
        get(tag, "id")
            .and_then(string)
            .is_some_and(|actual| actual.strip_prefix("minecraft:").unwrap_or(actual) == *id)
    }) {
        if get(tag, "SaddleItem").is_some() {
            rename(tag, "SaddleItem", "saddle");
            changed = true;
        }
    } else if named(get(tag, "id"), "minecraft:pig") || named(get(tag, "id"), "minecraft:strider") {
        let present = get(tag, "Saddle").is_some();
        let saddled = get(tag, "Saddle")
            .and_then(java_int)
            .is_some_and(|n| n != 0);
        remove(tag, "Saddle");
        if saddled {
            set(
                tag,
                "saddle",
                compound(vec![
                    ("id", text("minecraft:saddle")),
                    ("count", Tag::Int(1)),
                ]),
            );
            changed = true;
        } else {
            return present;
        }
    }
    if changed {
        let mut chances = take(tag, "drop_chances").unwrap_or_else(|| compound(vec![]));
        set(&mut chances, "saddle", Tag::Float(2.0));
        set(tag, "drop_chances", chances);
    }
    changed
}

fn equipment(tag: &mut Tag) -> Result<bool, UpgradeError> {
    let affected = has_any(
        tag,
        &["ArmorItems", "HandItems", "body_armor_item", "saddle"],
    );
    if !affected {
        return Ok(false);
    }
    // These lists are schema-typed Item stacks. A missing id means the placeholder is empty;
    // body/saddle are optional stacks, not list placeholders, and remain even without an id.
    for key in ["ArmorItems", "HandItems"] {
        let _ = list_field(tag, key, 4301, "EquipmentFormatFix")?;
    }
    let mut result = compound(vec![]);
    for (key, slots) in [
        ("HandItems", HAND_SLOTS.as_slice()),
        ("ArmorItems", ARMOR_SLOTS.as_slice()),
    ] {
        if let Some(Tag::List(items)) = take(tag, key) {
            for (item, slot) in items.elements.into_iter().zip(slots) {
                if get(&item, "id").is_some() {
                    set(&mut result, slot, item);
                }
            }
        }
    }
    for (old, new) in [("body_armor_item", "body"), ("saddle", "saddle")] {
        if let Some(item) = take(tag, old) {
            set(&mut result, new, item);
        }
    }
    if fields(&result).is_some_and(|fields| !fields.is_empty()) {
        set(tag, "equipment", result);
    }
    Ok(true)
}

fn player_equipment(tag: &mut Tag) -> Result<(), UpgradeError> {
    let mut equipment = compound(vec![]);
    if let Some(items) = list_field(tag, "Inventory", 4312, "PlayerEquipmentFix")? {
        let mut index = 0;
        while index < items.len() {
            let slot = match get(&items[index], "Slot").and_then(java_int).unwrap_or(-1) {
                100 => Some("feet"),
                101 => Some("legs"),
                102 => Some("chest"),
                103 => Some("head"),
                -106 => Some("offhand"),
                _ => None,
            };
            if let Some(slot) = slot {
                let mut item = items.remove(index);
                remove(&mut item, "Slot");
                set(&mut equipment, slot, item);
            } else {
                index += 1;
            }
        }
    }
    // Vanilla replaces equipment with an empty map as well when Inventory is absent.
    set(tag, "equipment", equipment);
    Ok(())
}

fn inline_pos(tag: &mut Tag, keys: [&str; 3], new: &str) -> bool {
    let values = keys.map(|key| get(tag, key).and_then(java_int));
    if let [Some(x), Some(y), Some(z)] = values {
        for key in keys {
            remove(tag, key);
        }
        set(tag, new, Tag::IntArray(vec![x, y, z]));
        true
    } else {
        false
    }
}

fn inline_positions(tag: &mut Tag, player: bool) -> bool {
    let mut changed = inline_pos(tag, ["SleepingX", "SleepingY", "SleepingZ"], "sleeping_pos");
    if player {
        let values = ["SpawnX", "SpawnY", "SpawnZ"].map(|key| get(tag, key).and_then(java_int));
        if let [Some(x), Some(y), Some(z)] = values {
            let mut respawn = compound(vec![("pos", Tag::IntArray(vec![x, y, z]))]);
            for (old, new) in [
                ("SpawnAngle", "angle"),
                ("SpawnDimension", "dimension"),
                ("SpawnForced", "forced"),
            ] {
                if let Some(value) = take(tag, old) {
                    set(&mut respawn, new, value);
                }
            }
            for key in ["SpawnX", "SpawnY", "SpawnZ"] {
                remove(tag, key);
            }
            set(tag, "respawn", respawn);
            changed = true;
        }
        if let Some(position) = take(tag, "enteredNetherPosition") {
            let values = ["x", "y", "z"]
                .map(|key| Tag::Double(get(&position, key).and_then(number).unwrap_or(0.0)));
            set(
                tag,
                "entered_nether_pos",
                Tag::List(ListTag {
                    element_type: 6,
                    elements: Vec::from(values),
                }),
            );
            changed = true;
        }
    } else if named(get(tag, "id"), "minecraft:vex") {
        changed |= get(tag, "LifeTicks").is_some();
        rename(tag, "LifeTicks", "life_ticks");
        changed |= inline_pos(tag, ["BoundX", "BoundY", "BoundZ"], "bound_pos");
    } else if named(get(tag, "id"), "minecraft:phantom") {
        changed |= get(tag, "Size").is_some();
        rename(tag, "Size", "size");
        changed |= inline_pos(tag, ["AX", "AY", "AZ"], "anchor_pos");
    } else if named(get(tag, "id"), "minecraft:turtle") {
        changed |= has_any(tag, &["TravelPosX", "TravelPosY", "TravelPosZ", "HasEgg"]);
        for key in ["TravelPosX", "TravelPosY", "TravelPosZ"] {
            remove(tag, key);
        }
        rename(tag, "HasEgg", "has_egg");
        changed |= inline_pos(tag, ["HomePosX", "HomePosY", "HomePosZ"], "home_pos");
    } else if ["item_frame", "glow_item_frame", "painting", "leash_knot"]
        .iter()
        .any(|id| {
            get(tag, "id")
                .and_then(string)
                .is_some_and(|actual| actual.strip_prefix("minecraft:").unwrap_or(actual) == *id)
        })
    {
        changed |= inline_pos(tag, ["TileX", "TileY", "TileZ"], "block_pos");
    }
    changed
}

fn empty_second_trade_item(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    let Some(offers) = get_mut(tag, "Offers") else {
        return Ok(());
    };
    let Some(recipes) = list_field(offers, "Recipes", 3828, "EmptyItemInVillagerTradeFix")? else {
        return Ok(());
    };
    for recipe in recipes {
        if let Some(item) = get(recipe, "buyB") {
            let id = get(item, "id").and_then(string).unwrap_or("minecraft:air");
            let count = get(item, "count").and_then(java_int).unwrap_or(0);
            if id == "air" || id == "minecraft:air" || count == 0 {
                remove(recipe, "buyB");
                report.applied(3828, "EmptyItemInVillagerTradeFix");
            }
        }
    }
    Ok(())
}

pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if !matches!(kind, NodeKind::Entity | NodeKind::Player) {
        return Ok(());
    }
    let player = matches!(kind, NodeKind::Player);
    match version {
        3086 if !player && named(get(tag, "id"), "minecraft:cat") => {
            variant(
                tag,
                "CatType",
                &[
                    "minecraft:tabby",
                    "minecraft:black",
                    "minecraft:red",
                    "minecraft:siamese",
                    "minecraft:british",
                    "minecraft:calico",
                    "minecraft:persian",
                    "minecraft:ragdoll",
                    "minecraft:white",
                    "minecraft:jellie",
                    "minecraft:all_black",
                ],
                Some("minecraft:tabby"),
                version,
            )?;
            report.applied(version, "EntityVariantFix");
        }
        3087 if !player && named(get(tag, "id"), "minecraft:frog") => {
            variant(
                tag,
                "Variant",
                &["minecraft:temperate", "minecraft:warm", "minecraft:cold"],
                None,
                version,
            )?;
            report.applied(version, "EntityVariantFix");
        }
        3090 if !player && named(get(tag, "id"), "minecraft:painting") => {
            rename(tag, "Motive", "variant");
            rename(tag, "Facing", "facing");
            report.applied(version, "EntityFieldsRenameFix");
        }
        3093 if !player && named(get(tag, "id"), "minecraft:goat") => {
            set(tag, "HasLeftHorn", Tag::Byte(1));
            set(tag, "HasRightHorn", Tag::Byte(1));
            report.applied(version, "EntityGoatMissingStateFix");
        }
        3097 if !player && named(get(tag, "id"), "minecraft:cat") => {
            if get(tag, "variant").and_then(string) == Some("minecraft:british") {
                set(tag, "variant", text("minecraft:british_shorthair"));
            }
            report.applied(version, "VariantRenameFix");
        }
        3322 => {
            let mut affected = false;
            for key in ["Effects", "ActiveEffects", "CustomPotionEffects"] {
                affected |= effect_list(tag, key, key, version)?;
            }
            if affected {
                report.applied(version, "EffectDurationFix");
            }
        }
        3568 => {
            let mut affected = effect_list(tag, "ActiveEffects", "active_effects", version)?;
            if !player && named(get(tag, "id"), "minecraft:arrow") {
                affected |=
                    effect_list(tag, "CustomPotionEffects", "custom_potion_effects", version)?;
            } else if !player && named(get(tag, "id"), "minecraft:area_effect_cloud") {
                affected |= effect_list(tag, "Effects", "effects", version)?;
            } else if !player && named(get(tag, "id"), "minecraft:mooshroom") {
                let mut stew = compound(vec![]);
                if let Some(id) = get(tag, "EffectId") {
                    set(&mut stew, "id", text(converted_effect_id(id, "EffectId")?));
                }
                if let Some(duration) = take(tag, "EffectDuration") {
                    set(&mut stew, "duration", duration);
                }
                affected |=
                    get(tag, "EffectId").is_some() || fields(&stew).is_some_and(|f| !f.is_empty());
                remove(tag, "EffectId");
                if fields(&stew).is_some_and(|f| !f.is_empty()) {
                    set(
                        tag,
                        "stew_effects",
                        Tag::List(ListTag {
                            element_type: 10,
                            elements: vec![stew],
                        }),
                    );
                }
            }
            if affected {
                report.applied(version, "MobEffectIdFix");
            }
        }
        3683 if !player && named(get(tag, "id"), "minecraft:tnt") => {
            set(
                tag,
                "block_state",
                compound(vec![("Name", text("minecraft:tnt"))]),
            );
            // The vanilla fixer copies Fuse, not renameField; the old remainder is retained.
            if let Some(fuse) = get(tag, "Fuse").cloned() {
                set(tag, "fuse", fuse);
            }
            report.applied(version, "PrimedTntBlockStateFixer");
        }
        3685 if !player => {
            if named(get(tag, "id"), "minecraft:arrow") {
                let tipped = get(tag, "Potion")
                    .and_then(string)
                    .is_some_and(|potion| potion != "minecraft:empty");
                set(
                    tag,
                    "item",
                    compound(vec![
                        (
                            "id",
                            text(if tipped {
                                "minecraft:tipped_arrow"
                            } else {
                                "minecraft:arrow"
                            }),
                        ),
                        ("Count", Tag::Int(1)),
                    ]),
                );
                report.applied(version, "FixProjectileStoredItem");
            } else if named(get(tag, "id"), "minecraft:spectral_arrow") {
                set(
                    tag,
                    "item",
                    compound(vec![
                        ("id", text("minecraft:spectral_arrow")),
                        ("Count", Tag::Int(1)),
                    ]),
                );
                report.applied(version, "FixProjectileStoredItem");
            } else if named(get(tag, "id"), "minecraft:trident") {
                rename(tag, "Trident", "item");
                report.applied(version, "FixProjectileStoredItem");
            }
        }
        3808 if !player => {
            let horse = named(get(tag, "id"), "minecraft:horse");
            if ((horse && report.from != 3808)
                || named(get(tag, "id"), "minecraft:llama")
                || named(get(tag, "id"), "minecraft:trader_llama"))
                && horse_armor(tag, horse)?
            {
                report.applied(version, "HorseBodyArmorItemFix");
            }
        }
        3809 if !player
            && ["donkey", "mule", "llama", "trader_llama"]
                .iter()
                .any(|id| {
                    get(tag, "id").and_then(string).is_some_and(|actual| {
                        actual.strip_prefix("minecraft:").unwrap_or(actual) == *id
                    })
                }) =>
        {
            if let Some(items) = list_field(
                tag,
                "Items",
                version,
                "ChestedHorsesInventoryZeroIndexingFix",
            )? {
                for item in items {
                    if let Some(slot) = get_mut(item, "Slot") {
                        *slot = Tag::Byte(java_int(slot).unwrap_or(2).wrapping_sub(2) as i8);
                    }
                }
                report.applied(version, "ChestedHorsesInventoryZeroIndexingFix");
            }
        }
        3812 if !player && named(get(tag, "id"), "minecraft:wolf") => {
            if wolf_health(tag)? {
                report.applied(version, "FixWolfHealth");
            }
        }
        3813 if !player => {
            if entity_positions(tag) {
                report.applied(version, "BlockPosFormatAndRenamesFix");
            }
        }
        3814 => {
            if legacy_attributes(tag)? {
                report.applied(version, "AttributesRenameLegacy");
            }
        }
        3818 if !player => {
            if named(get(tag, "id"), "minecraft:area_effect_cloud")
                && let Some(particle) = get_mut(tag, "Particle")
                && particles::unflatten(particle)?
            {
                report.applied(version, "ParticleUnflatteningFix");
            }
            if named(get(tag, "id"), "minecraft:arrow") && arrow_potion(tag)? {
                report.applied(version, "TippedArrowPotionToItemFix");
            }
            if named(get(tag, "id"), "minecraft:area_effect_cloud") && cloud_potion(tag) {
                report.applied(version, "AreaEffectCloudPotionFix");
            }
        }
        3828 if !player
            && [
                "minecraft:villager",
                "minecraft:zombie_villager",
                "minecraft:wandering_trader",
            ]
            .iter()
            .any(|id| named(get(tag, "id"), id)) =>
        {
            empty_second_trade_item(tag, report)?;
        }
        3938 if !player
            && (named(get(tag, "id"), "minecraft:arrow")
                || named(get(tag, "id"), "minecraft:spectral_arrow")) =>
        {
            // write/read identity: weapon becomes schema-typed; the parent walker upgrades it.
            if get(tag, "weapon").is_some() {
                report.applied(version, "ProjectileStoredWeaponFix");
            }
        }
        3945 => {
            if attribute_format(tag)? {
                report.applied(version, "AttributeModifierIdFix");
            }
        }
        4055 => {
            if attribute_prefixes(tag)? {
                report.applied(version, "AttributeIdPrefixFix");
            }
        }
        4067 if !player
            && (named(get(tag, "id"), "minecraft:boat")
                || named(get(tag, "id"), "minecraft:chest_boat")) =>
        {
            let chest = named(get(tag, "id"), "minecraft:chest_boat");
            let variant = get(tag, "Type").and_then(string).unwrap_or("oak");
            let wood = match variant {
                "spruce" | "birch" | "jungle" | "acacia" | "cherry" | "dark_oak" | "mangrove"
                | "bamboo" => variant,
                _ => "oak",
            };
            let id = format!(
                "minecraft:{wood}_{}",
                if wood == "bamboo" {
                    if chest { "chest_raft" } else { "raft" }
                } else if chest {
                    "chest_boat"
                } else {
                    "boat"
                }
            );
            remove(tag, "Type");
            set(tag, "id", Tag::String(id));
            report.applied(version, "BoatSplitFix");
        }
        4081 if !player && named(get(tag, "id"), "minecraft:salmon") => {
            if get(tag, "type").and_then(string) != Some("large") {
                set(tag, "type", text("medium"));
            }
            report.applied(version, "EntitySalmonSizeFix");
        }
        4173 if !player && named(get(tag, "id"), "minecraft:tnt_minecart") => {
            rename(tag, "TNTFuse", "fuse");
            report.applied(version, "EntityFieldsRenameFix");
        }
        4187 if !player => {
            let ranges = [
                ("minecraft:villager", 48.0, 16.0),
                ("minecraft:bee", 48.0, 16.0),
                ("minecraft:allay", 48.0, 16.0),
                ("minecraft:llama", 40.0, 16.0),
                ("minecraft:piglin_brute", 16.0, 12.0),
                ("minecraft:warden", 16.0, 24.0),
            ];
            for (id, old, new) in ranges {
                if named(get(tag, "id"), id) && follow_range(tag, old, new)? {
                    report.applied(version, "EntityAttributeBaseValueFix");
                }
            }
        }
        4293 if !player => {
            if drop_chances(tag)? {
                report.applied(version, "DropChancesFormatFix");
            }
        }
        4296 if !player && named(get(tag, "id"), "minecraft:area_effect_cloud") => {
            set(tag, "potion_duration_scale", Tag::Float(0.25));
            report.applied(version, "AreaEffectCloudDurationScaleFix");
        }
        4300 if !player => {
            if saddle(tag) {
                report.applied(version, "SaddleEquipmentSlotFix");
            }
        }
        4301 if !player => {
            if equipment(tag)? {
                report.applied(version, "EquipmentFormatFix");
            }
        }
        4303 => {
            if let Some(distance) = take(tag, "FallDistance") {
                set(
                    tag,
                    "fall_distance",
                    Tag::Double(f64::from(number(&distance).unwrap_or(0.0) as f32)),
                );
                report.applied(version, "EntityFallDistanceFloatToDoubleFix");
            }
        }
        4306 if !player && named(get(tag, "id"), "minecraft:potion") => {
            let lingering = get(tag, "Item")
                .and_then(|item| get(item, "id"))
                .is_some_and(|id| named(Some(id), "minecraft:lingering_potion"));
            set(
                tag,
                "id",
                text(if lingering {
                    "minecraft:lingering_potion"
                } else {
                    "minecraft:splash_potion"
                }),
            );
            report.applied(version, "ThrownPotionSplitFix");
        }
        4312 if player => {
            player_equipment(tag)?;
            report.applied(version, "PlayerEquipmentFix");
        }
        4314 => {
            if inline_positions(tag, player) {
                report.applied(version, "InlineBlockPosFormatFix");
            }
        }
        4420 if !player && named(get(tag, "id"), "minecraft:area_effect_cloud") => {
            if particles::custom_particle(tag)? {
                report.applied(version, "NamedEntityConvertUncheckedFix");
            }
        }
        4424 => {
            // AddFieldFix's disassembly returns a modified nested Dynamic instead of
            // reinserting it in its parent. Refuse affected data rather than fabricate
            // a modern icon or discard the entity/player root.
            if get(tag, "locator_bar_icon").is_some() {
                return Err(reject(
                    version,
                    "AddFieldFix",
                    "locator_bar_icon",
                    "nested vanilla default insertion cannot be applied losslessly",
                ));
            }
        }
        4535 if !player && named(get(tag, "id"), "minecraft:copper_golem") => {
            if let Some(weather) = get_mut(tag, "weather_state") {
                *weather = text(match java_int(weather).unwrap_or(0) {
                    1 => "exposed",
                    2 => "weathered",
                    3 => "oxidized",
                    _ => "unaffected",
                });
                report.applied(version, "CopperGolemWeatherStateFix");
            }
        }
        4763 if !player => {
            if named(get(tag, "id"), "minecraft:villager") {
                set(tag, "VillagerDataFinalized", Tag::Byte(1));
                report.applied(version, "VillagerSetVillagerDataFinalized");
            } else if named(get(tag, "id"), "minecraft:zombie_villager") {
                set(tag, "VillagerDataFinalized", Tag::Byte(1));
                report.applied(version, "ZombieVillagerSetVillagerDataFinalized");
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entity(id: &str, mut values: Vec<(&str, Tag)>) -> Tag {
        values.insert(0, ("id", text(id)));
        compound(values)
    }

    fn tags(elements: Vec<Tag>) -> Tag {
        Tag::List(ListTag {
            element_type: elements.first().map_or(0, Tag::type_id),
            elements,
        })
    }

    fn apply(tag: &mut Tag, kind: NodeKind, version: i32) {
        fix_node(tag, kind, version, &mut UpgradeReport::default()).unwrap();
    }

    fn run_entity(tag: &mut Tag, version: i32) {
        apply(tag, NodeKind::Entity, version);
    }

    #[test]
    fn cat_variants_3086_and_british_rename_3097() {
        let mut cat = entity("minecraft:cat", vec![("CatType", Tag::Int(4))]);
        run_entity(&mut cat, 3086);
        assert_eq!(get(&cat, "variant"), Some(&text("minecraft:british")));
        assert_eq!(get(&cat, "CatType"), Some(&Tag::Int(4)));
        run_entity(&mut cat, 3097);
        assert_eq!(
            get(&cat, "variant"),
            Some(&text("minecraft:british_shorthair"))
        );
        let mut unknown = entity("minecraft:cat", vec![("CatType", Tag::Int(100))]);
        run_entity(&mut unknown, 3086);
        assert_eq!(get(&unknown, "variant"), Some(&text("minecraft:tabby")));
    }

    #[test]
    fn frog_variants_3087_reject_unmapped_numeric_ids() {
        let mut frog = entity("minecraft:frog", vec![("Variant", Tag::Int(2))]);
        run_entity(&mut frog, 3087);
        assert_eq!(get(&frog, "variant"), Some(&text("minecraft:cold")));
        let mut unknown = entity("minecraft:frog", vec![("Variant", Tag::Int(3))]);
        let before = unknown.clone();
        assert!(
            fix_node(
                &mut unknown,
                NodeKind::Entity,
                3087,
                &mut UpgradeReport::default()
            )
            .is_err()
        );
        assert_eq!(unknown, before);
    }

    #[test]
    fn painting_fields_3090() {
        let mut painting = entity(
            "minecraft:painting",
            vec![
                ("Motive", text("minecraft:kebab")),
                ("Facing", Tag::Byte(3)),
            ],
        );
        run_entity(&mut painting, 3090);
        assert_eq!(get(&painting, "variant"), Some(&text("minecraft:kebab")));
        assert_eq!(get(&painting, "facing"), Some(&Tag::Byte(3)));
        assert!(get(&painting, "Motive").is_none());
    }

    #[test]
    fn goat_horns_3093_are_unconditional() {
        let mut goat = entity("minecraft:goat", vec![("HasLeftHorn", Tag::Byte(0))]);
        run_entity(&mut goat, 3093);
        assert_eq!(get(&goat, "HasLeftHorn"), Some(&Tag::Byte(1)));
        assert_eq!(get(&goat, "HasRightHorn"), Some(&Tag::Byte(1)));
    }

    #[test]
    fn effect_duration_3322_shared_entity_player_and_item() {
        let effect = compound(vec![
            ("Duration", Tag::Int(40)),
            (
                "FactorCalculationData",
                compound(vec![("effect_changed_timestamp", Tag::Int(95))]),
            ),
            (
                "HiddenEffect",
                compound(vec![(
                    "FactorCalculationData",
                    compound(vec![("effect_changed_timestamp", Tag::Int(99))]),
                )]),
            ),
        ]);
        for kind in [NodeKind::Entity, NodeKind::Player] {
            let mut living = compound(vec![("ActiveEffects", tags(vec![effect.clone()]))]);
            apply(&mut living, kind, 3322);
            let effect =
                &super::super::nbt::list(get(&living, "ActiveEffects").unwrap()).unwrap()[0];
            let factor = get(effect, "FactorCalculationData").unwrap();
            assert_eq!(get(factor, "ticks_active"), Some(&Tag::Int(55)));
            assert!(get(factor, "effect_changed_timestamp").is_none());
            assert!(
                get(
                    get(
                        get(effect, "HiddenEffect").unwrap(),
                        "FactorCalculationData"
                    )
                    .unwrap(),
                    "effect_changed_timestamp"
                )
                .is_some()
            );
        }
        let mut potion = entity(
            "minecraft:potion",
            vec![(
                "tag",
                compound(vec![("CustomPotionEffects", tags(vec![effect]))]),
            )],
        );
        fix_potion_effects(&mut potion, 3322, &mut UpgradeReport::default()).unwrap();
        let effects = get(get(&potion, "tag").unwrap(), "CustomPotionEffects").unwrap();
        let factor = get(
            &super::super::nbt::list(effects).unwrap()[0],
            "FactorCalculationData",
        )
        .unwrap();
        assert_eq!(get(factor, "ticks_active"), Some(&Tag::Int(55)));
    }

    #[test]
    fn mob_effect_ids_3568_and_hidden_instances() {
        let mut living = compound(vec![(
            "ActiveEffects",
            tags(vec![compound(vec![
                ("Id", Tag::Byte(1)),
                ("Duration", Tag::Int(10)),
                ("ShowIcon", Tag::Byte(1)),
                (
                    "HiddenEffect",
                    compound(vec![("Id", Tag::Byte(2)), ("Amplifier", Tag::Byte(3))]),
                ),
            ])]),
        )]);
        apply(&mut living, NodeKind::Player, 3568);
        let effect = &super::super::nbt::list(get(&living, "active_effects").unwrap()).unwrap()[0];
        assert_eq!(get(effect, "id"), Some(&text("minecraft:speed")));
        assert_eq!(get(effect, "duration"), Some(&Tag::Int(10)));
        let hidden = get(effect, "hidden_effect").unwrap();
        assert_eq!(get(hidden, "id"), Some(&text("minecraft:slowness")));
        assert_eq!(get(hidden, "amplifier"), Some(&Tag::Byte(3)));
    }

    #[test]
    fn effect_3568_mooshroom_stew_and_potion_item() {
        let mut mooshroom = entity(
            "minecraft:mooshroom",
            vec![
                ("EffectId", Tag::Byte(9)),
                ("EffectDuration", Tag::Int(100)),
            ],
        );
        run_entity(&mut mooshroom, 3568);
        let effect = &super::super::nbt::list(get(&mooshroom, "stew_effects").unwrap()).unwrap()[0];
        assert_eq!(get(effect, "id"), Some(&text("minecraft:nausea")));
        assert!(get(&mooshroom, "EffectId").is_none());
        let mut stew = entity(
            "minecraft:suspicious_stew",
            vec![(
                "tag",
                compound(vec![(
                    "Effects",
                    tags(vec![compound(vec![
                        ("EffectId", Tag::Byte(1)),
                        ("EffectDuration", Tag::Int(50)),
                    ])]),
                )]),
            )],
        );
        fix_potion_effects(&mut stew, 3568, &mut UpgradeReport::default()).unwrap();
        let item_tag = get(&stew, "tag").unwrap();
        assert!(get(item_tag, "Effects").is_none());
        let effect = &super::super::nbt::list(get(item_tag, "effects").unwrap()).unwrap()[0];
        assert_eq!(get(effect, "duration"), Some(&Tag::Int(50)));
    }

    #[test]
    fn effect_3568_unknown_ids_and_malformed_lists_fail_closed() {
        for effects in [
            tags(vec![compound(vec![("Id", Tag::Int(99))])]),
            Tag::Int(1),
        ] {
            let mut entity = compound(vec![("ActiveEffects", effects)]);
            assert!(
                fix_node(
                    &mut entity,
                    NodeKind::Entity,
                    3568,
                    &mut UpgradeReport::default()
                )
                .is_err()
            );
        }
    }

    #[test]
    fn primed_tnt_3683_keeps_legacy_fuse() {
        let mut tnt = entity("minecraft:tnt", vec![("Fuse", Tag::Short(80))]);
        run_entity(&mut tnt, 3683);
        assert_eq!(get(&tnt, "fuse"), Some(&Tag::Short(80)));
        assert_eq!(get(&tnt, "Fuse"), Some(&Tag::Short(80)));
        assert_eq!(
            get(get(&tnt, "block_state").unwrap(), "Name"),
            Some(&text("minecraft:tnt"))
        );
    }

    #[test]
    fn projectile_stored_items_3685() {
        for (id, potion, expected) in [
            ("minecraft:arrow", "minecraft:empty", "minecraft:arrow"),
            (
                "minecraft:arrow",
                "minecraft:water",
                "minecraft:tipped_arrow",
            ),
            (
                "minecraft:spectral_arrow",
                "minecraft:empty",
                "minecraft:spectral_arrow",
            ),
        ] {
            let mut arrow = entity(id, vec![("Potion", text(potion))]);
            run_entity(&mut arrow, 3685);
            assert_eq!(
                get(get(&arrow, "item").unwrap(), "id"),
                Some(&text(expected))
            );
            assert_eq!(
                get(get(&arrow, "item").unwrap(), "Count"),
                Some(&Tag::Int(1))
            );
        }
        let item = compound(vec![
            ("id", text("minecraft:trident")),
            ("Count", Tag::Byte(1)),
        ]);
        let mut trident = entity("minecraft:trident", vec![("Trident", item.clone())]);
        run_entity(&mut trident, 3685);
        assert_eq!(get(&trident, "item"), Some(&item));
        assert!(get(&trident, "Trident").is_none());
    }

    #[test]
    fn horse_body_armor_3808_and_llama_decor() {
        let item = compound(vec![("id", text("minecraft:diamond_horse_armor"))]);
        let mut horse = entity(
            "minecraft:horse",
            vec![
                ("ArmorItem", item.clone()),
                (
                    "ArmorItems",
                    tags(vec![
                        compound(vec![]),
                        compound(vec![]),
                        item.clone(),
                        compound(vec![]),
                    ]),
                ),
                (
                    "ArmorDropChances",
                    tags(vec![Tag::Float(0.0), Tag::Float(0.0), Tag::Float(2.0)]),
                ),
            ],
        );
        run_entity(&mut horse, 3808);
        assert_eq!(get(&horse, "body_armor_item"), Some(&item));
        assert_eq!(
            get(&horse, "body_armor_drop_chance"),
            Some(&Tag::Float(2.0))
        );
        assert_eq!(
            super::super::nbt::list(get(&horse, "ArmorItems").unwrap()).unwrap()[2],
            compound(vec![])
        );
        assert_eq!(
            super::super::nbt::list(get(&horse, "ArmorDropChances").unwrap()).unwrap()[2],
            Tag::Float(0.085)
        );
        for id in ["minecraft:llama", "minecraft:trader_llama"] {
            let mut llama = entity(id, vec![("DecorItem", item.clone())]);
            run_entity(&mut llama, 3808);
            assert_eq!(get(&llama, "body_armor_item"), Some(&item));
        }
    }

    #[test]
    fn chested_inventory_3809_is_byte_narrowing() {
        let mut donkey = entity(
            "minecraft:donkey",
            vec![(
                "Items",
                tags(vec![
                    compound(vec![("Slot", Tag::Byte(2))]),
                    compound(vec![("Slot", Tag::Int(130))]),
                    compound(vec![]),
                ]),
            )],
        );
        run_entity(&mut donkey, 3809);
        let items = super::super::nbt::list(get(&donkey, "Items").unwrap()).unwrap();
        assert_eq!(get(&items[0], "Slot"), Some(&Tag::Byte(0)));
        assert_eq!(get(&items[1], "Slot"), Some(&Tag::Byte(-128)));
        assert!(get(&items[2], "Slot").is_none());
    }

    #[test]
    fn wolf_health_3812_only_doubles_twenty_base() {
        for (base, expected_base, expected_health) in [(20.0, 40.0, 24.0), (19.0, 19.0, 12.0)] {
            let mut wolf = entity(
                "minecraft:wolf",
                vec![
                    ("Health", Tag::Float(12.0)),
                    (
                        "Attributes",
                        tags(vec![compound(vec![
                            ("Name", text("generic.max_health")),
                            ("Base", Tag::Double(base)),
                        ])]),
                    ),
                ],
            );
            run_entity(&mut wolf, 3812);
            assert_eq!(get(&wolf, "Health"), Some(&Tag::Float(expected_health)));
            let attribute = &super::super::nbt::list(get(&wolf, "Attributes").unwrap()).unwrap()[0];
            assert_eq!(get(attribute, "Base"), Some(&Tag::Double(expected_base)));
        }
    }

    #[test]
    fn entity_block_positions_3813_and_uuid_leash() {
        let pos = compound(vec![
            ("X", Tag::Int(1)),
            ("Y", Tag::Int(-2)),
            ("Z", Tag::Int(3)),
        ]);
        for (id, old, new) in [
            ("minecraft:bee", "HivePos", "hive_pos"),
            ("minecraft:bee", "FlowerPos", "flower_pos"),
            ("minecraft:end_crystal", "BeamTarget", "beam_target"),
            (
                "minecraft:wandering_trader",
                "WanderTarget",
                "wander_target",
            ),
            ("minecraft:pillager", "PatrolTarget", "patrol_target"),
        ] {
            let mut entity = entity(id, vec![(old, pos.clone())]);
            run_entity(&mut entity, 3813);
            assert_eq!(get(&entity, new), Some(&Tag::IntArray(vec![1, -2, 3])));
            assert!(get(&entity, old).is_none());
        }
        let leash = compound(vec![("UUID", Tag::IntArray(vec![1, 2, 3, 4]))]);
        let mut cow = entity("minecraft:cow", vec![("Leash", leash.clone())]);
        run_entity(&mut cow, 3813);
        assert_eq!(get(&cow, "leash"), Some(&leash));
    }

    #[test]
    fn legacy_jump_strength_3814_shared_player() {
        let mut player = compound(vec![(
            "Attributes",
            tags(vec![compound(vec![("Name", text("horse.jump_strength"))])]),
        )]);
        apply(&mut player, NodeKind::Player, 3814);
        let attribute = &super::super::nbt::list(get(&player, "Attributes").unwrap()).unwrap()[0];
        assert_eq!(
            get(attribute, "Name"),
            Some(&text("minecraft:generic.jump_strength"))
        );
    }

    #[test]
    fn arrow_and_cloud_potion_transfer_3818() {
        let mut arrow = entity(
            "minecraft:arrow",
            vec![
                ("Potion", text("minecraft:water")),
                ("Color", Tag::Int(42)),
                ("custom_potion_effects", tags(vec![])),
                (
                    "item",
                    compound(vec![("id", text("minecraft:tipped_arrow"))]),
                ),
            ],
        );
        run_entity(&mut arrow, 3818);
        assert!(get(&arrow, "Potion").is_none());
        let item_tag = get(get(&arrow, "item").unwrap(), "tag").unwrap();
        assert_eq!(get(item_tag, "Potion"), Some(&text("minecraft:water")));
        assert_eq!(get(item_tag, "CustomPotionColor"), Some(&Tag::Int(42)));
        let mut cloud = entity(
            "minecraft:area_effect_cloud",
            vec![
                ("Color", Tag::Int(42)),
                ("Potion", text("minecraft:water")),
                ("effects", tags(vec![])),
            ],
        );
        run_entity(&mut cloud, 3818);
        let contents = get(&cloud, "potion_contents").unwrap();
        assert_eq!(get(contents, "custom_color"), Some(&Tag::Int(42)));
        assert_eq!(get(contents, "potion"), Some(&text("minecraft:water")));
        assert!(get(contents, "custom_effects").is_some());
    }

    #[test]
    fn potion_transfer_3818_rejects_missing_item_and_unsupported_snbt() {
        for mut tag in [
            entity("minecraft:arrow", vec![("Potion", text("minecraft:water"))]),
            entity(
                "minecraft:area_effect_cloud",
                vec![("Particle", text("minecraft:item stone{Damage:1}"))],
            ),
        ] {
            let before = tag.clone();
            assert!(
                fix_node(
                    &mut tag,
                    NodeKind::Entity,
                    3818,
                    &mut UpgradeReport::default()
                )
                .is_err()
            );
            assert_eq!(tag, before);
        }
    }

    #[test]
    fn stored_weapon_3938_is_schema_identity() {
        let mut arrow = entity(
            "minecraft:arrow",
            vec![("weapon", compound(vec![("id", text("minecraft:bow"))]))],
        );
        let before = arrow.clone();
        run_entity(&mut arrow, 3938);
        assert_eq!(arrow, before);
    }

    #[test]
    fn attributes_3945_convert_entity_fields_and_operations() {
        let mut player = compound(vec![(
            "Attributes",
            tags(vec![compound(vec![
                ("Name", text("minecraft:generic.movement_speed")),
                ("Base", Tag::Double(0.1)),
                (
                    "Modifiers",
                    tags(vec![compound(vec![
                        ("UUID", Tag::IntArray(vec![0, 0, 0, 1])),
                        ("Name", text("test")),
                        ("Amount", Tag::Double(0.2)),
                        ("Operation", Tag::Int(2)),
                    ])]),
                ),
            ])]),
        )]);
        apply(&mut player, NodeKind::Player, 3945);
        assert!(get(&player, "Attributes").is_none());
        let attribute = &super::super::nbt::list(get(&player, "attributes").unwrap()).unwrap()[0];
        assert_eq!(
            get(attribute, "id"),
            Some(&text("minecraft:generic.movement_speed"))
        );
        assert_eq!(get(attribute, "base"), Some(&Tag::Double(0.1)));
        let modifier = &super::super::nbt::list(get(attribute, "modifiers").unwrap()).unwrap()[0];
        assert_eq!(
            get(modifier, "operation"),
            Some(&text("add_multiplied_total"))
        );
        assert_eq!(get(modifier, "amount"), Some(&Tag::Double(0.2)));
        assert!(get(modifier, "id").is_some());
        assert!(get(modifier, "UUID").is_none());
    }

    #[test]
    fn attribute_prefix_4055_preserves_modded_namespaces() {
        let mut entity = compound(vec![(
            "attributes",
            tags(vec![
                compound(vec![("id", text("minecraft:generic.max_health"))]),
                compound(vec![("id", text("horse.jump_strength"))]),
                compound(vec![("id", text("mod:generic.max_health"))]),
            ]),
        )]);
        run_entity(&mut entity, 4055);
        let attributes = super::super::nbt::list(get(&entity, "attributes").unwrap()).unwrap();
        assert_eq!(
            get(&attributes[0], "id"),
            Some(&text("minecraft:max_health"))
        );
        assert_eq!(
            get(&attributes[1], "id"),
            Some(&text("minecraft:jump_strength"))
        );
        assert_eq!(
            get(&attributes[2], "id"),
            Some(&text("mod:generic.max_health"))
        );
    }

    #[test]
    fn boat_split_4067_variants_and_defaults() {
        for (id, wood, expected) in [
            ("minecraft:boat", "birch", "minecraft:birch_boat"),
            (
                "minecraft:chest_boat",
                "bamboo",
                "minecraft:bamboo_chest_raft",
            ),
            ("minecraft:boat", "bamboo", "minecraft:bamboo_raft"),
            ("minecraft:boat", "unknown", "minecraft:oak_boat"),
        ] {
            let mut boat = entity(id, vec![("Type", text(wood))]);
            run_entity(&mut boat, 4067);
            assert_eq!(get(&boat, "id"), Some(&text(expected)));
            assert!(get(&boat, "Type").is_none());
        }
    }

    #[test]
    fn creaking_schema_4071_preserves_unmodified_entity_data() {
        for id in ["minecraft:creaking", "minecraft:creaking_transient"] {
            let mut creaking = entity(id, vec![("home_pos", Tag::IntArray(vec![1, 2, 3]))]);
            let before = creaking.clone();
            run_entity(&mut creaking, 4071);
            assert_eq!(creaking, before);
        }
    }

    #[test]
    fn salmon_size_4081_medium_unless_large() {
        for (old, new) in [
            ("small", "medium"),
            ("large", "large"),
            ("unknown", "medium"),
        ] {
            let mut salmon = entity("minecraft:salmon", vec![("type", text(old))]);
            run_entity(&mut salmon, 4081);
            assert_eq!(get(&salmon, "type"), Some(&text(new)));
        }
    }

    #[test]
    fn minecart_fuse_4173() {
        let mut cart = entity("minecraft:tnt_minecart", vec![("TNTFuse", Tag::Int(10))]);
        run_entity(&mut cart, 4173);
        assert_eq!(get(&cart, "fuse"), Some(&Tag::Int(10)));
        assert!(get(&cart, "TNTFuse").is_none());
    }

    #[test]
    fn follow_ranges_4187_exact_defaults_only() {
        for (id, old, new) in [
            ("minecraft:villager", 48.0, 16.0),
            ("minecraft:bee", 48.0, 16.0),
            ("minecraft:allay", 48.0, 16.0),
            ("minecraft:llama", 40.0, 16.0),
            ("minecraft:piglin_brute", 16.0, 12.0),
            ("minecraft:warden", 16.0, 24.0),
        ] {
            let mut entity = entity(
                id,
                vec![(
                    "attributes",
                    tags(vec![compound(vec![
                        ("id", text("follow_range")),
                        ("base", Tag::Double(old)),
                    ])]),
                )],
            );
            run_entity(&mut entity, 4187);
            let attribute =
                &super::super::nbt::list(get(&entity, "attributes").unwrap()).unwrap()[0];
            assert_eq!(get(attribute, "base"), Some(&Tag::Double(new)));
        }
        let mut bee = entity(
            "minecraft:bee",
            vec![(
                "attributes",
                tags(vec![compound(vec![
                    ("id", text("follow_range")),
                    ("base", Tag::Double(49.0)),
                ])]),
            )],
        );
        run_entity(&mut bee, 4187);
        let attribute = &super::super::nbt::list(get(&bee, "attributes").unwrap()).unwrap()[0];
        assert_eq!(get(attribute, "base"), Some(&Tag::Double(49.0)));
    }

    #[test]
    fn drop_chances_4293_omits_defaults_and_preserves_slot_mapping() {
        let mut entity = compound(vec![
            (
                "ArmorDropChances",
                tags(vec![
                    Tag::Float(0.085),
                    Tag::Float(0.2),
                    Tag::Float(0.3),
                    Tag::Float(0.4),
                ]),
            ),
            (
                "HandDropChances",
                tags(vec![Tag::Float(0.5), Tag::Float(0.6)]),
            ),
            ("body_armor_drop_chance", Tag::Float(2.0)),
        ]);
        run_entity(&mut entity, 4293);
        let chances = get(&entity, "drop_chances").unwrap();
        assert!(get(chances, "feet").is_none());
        for (slot, chance) in [
            ("legs", 0.2),
            ("chest", 0.3),
            ("head", 0.4),
            ("mainhand", 0.5),
            ("offhand", 0.6),
            ("body", 2.0),
        ] {
            assert_eq!(get(chances, slot), Some(&Tag::Float(chance)));
        }
        assert!(get(&entity, "ArmorDropChances").is_none());
    }

    #[test]
    fn cloud_duration_scale_4296_overwrites_previous_scale() {
        let mut cloud = entity(
            "minecraft:area_effect_cloud",
            vec![("potion_duration_scale", Tag::Float(1.0))],
        );
        run_entity(&mut cloud, 4296);
        assert_eq!(
            get(&cloud, "potion_duration_scale"),
            Some(&Tag::Float(0.25))
        );
    }

    #[test]
    fn saddle_equipment_4300_flags_and_items() {
        let mut pig = entity("minecraft:pig", vec![("Saddle", Tag::Byte(1))]);
        run_entity(&mut pig, 4300);
        assert_eq!(
            get(get(&pig, "saddle").unwrap(), "id"),
            Some(&text("minecraft:saddle"))
        );
        assert_eq!(
            get(get(&pig, "drop_chances").unwrap(), "saddle"),
            Some(&Tag::Float(2.0))
        );
        assert!(get(&pig, "Saddle").is_none());
        let mut horse = entity(
            "minecraft:horse",
            vec![(
                "SaddleItem",
                compound(vec![("id", text("minecraft:saddle"))]),
            )],
        );
        run_entity(&mut horse, 4300);
        assert!(get(&horse, "SaddleItem").is_none());
        assert!(get(&horse, "saddle").is_some());
        let mut strider = entity("minecraft:strider", vec![("Saddle", Tag::Byte(0))]);
        run_entity(&mut strider, 4300);
        assert!(get(&strider, "Saddle").is_none());
        assert!(get(&strider, "saddle").is_none());
    }

    #[test]
    fn equipment_4301_discards_only_missing_id_list_placeholders() {
        let boots = compound(vec![
            ("id", text("minecraft:iron_boots")),
            ("count", Tag::Int(1)),
        ]);
        let mut zombie = entity(
            "minecraft:zombie",
            vec![
                ("ArmorItems", tags(vec![boots.clone(), compound(vec![])])),
                (
                    "HandItems",
                    tags(vec![
                        compound(vec![]),
                        compound(vec![("id", text("minecraft:air"))]),
                    ]),
                ),
                ("body_armor_item", compound(vec![])),
            ],
        );
        run_entity(&mut zombie, 4301);
        let equipment = get(&zombie, "equipment").unwrap();
        assert_eq!(get(equipment, "feet"), Some(&boots));
        assert!(get(equipment, "legs").is_none());
        assert!(get(equipment, "mainhand").is_none());
        assert_eq!(
            get(get(equipment, "offhand").unwrap(), "id"),
            Some(&text("minecraft:air"))
        );
        assert_eq!(get(equipment, "body"), Some(&compound(vec![])));
        assert!(get(&zombie, "ArmorItems").is_none());
    }

    #[test]
    fn fall_distance_4303_float_rounding_shared_with_player() {
        for kind in [NodeKind::Entity, NodeKind::Player] {
            let mut tag = compound(vec![("FallDistance", Tag::Double(1.00000001))]);
            apply(&mut tag, kind, 4303);
            assert_eq!(get(&tag, "fall_distance"), Some(&Tag::Double(1.0)));
            assert!(get(&tag, "FallDistance").is_none());
        }
    }

    #[test]
    fn thrown_potion_split_4306() {
        for (item, expected) in [
            ("minecraft:lingering_potion", "minecraft:lingering_potion"),
            ("minecraft:potion", "minecraft:splash_potion"),
        ] {
            let mut potion = entity(
                "minecraft:potion",
                vec![("Item", compound(vec![("id", text(item))]))],
            );
            run_entity(&mut potion, 4306);
            assert_eq!(get(&potion, "id"), Some(&text(expected)));
        }
        let mut missing = entity("minecraft:potion", vec![]);
        run_entity(&mut missing, 4306);
        assert_eq!(get(&missing, "id"), Some(&text("minecraft:splash_potion")));
    }

    #[test]
    fn player_equipment_4312_extracts_signed_offhand_and_armor() {
        let mut player = compound(vec![(
            "Inventory",
            tags(vec![
                compound(vec![
                    ("Slot", Tag::Byte(0)),
                    ("id", text("minecraft:stone")),
                ]),
                compound(vec![
                    ("Slot", Tag::Byte(100)),
                    ("id", text("minecraft:iron_boots")),
                ]),
                compound(vec![
                    ("Slot", Tag::Byte(-106)),
                    ("id", text("minecraft:shield")),
                ]),
            ]),
        )]);
        apply(&mut player, NodeKind::Player, 4312);
        assert_eq!(
            super::super::nbt::list(get(&player, "Inventory").unwrap())
                .unwrap()
                .len(),
            1
        );
        let equipment = get(&player, "equipment").unwrap();
        assert_eq!(
            get(get(equipment, "feet").unwrap(), "id"),
            Some(&text("minecraft:iron_boots"))
        );
        assert!(get(get(equipment, "feet").unwrap(), "Slot").is_none());
        assert_eq!(
            get(get(equipment, "offhand").unwrap(), "id"),
            Some(&text("minecraft:shield"))
        );
    }

    #[test]
    fn inline_positions_4314_for_living_attached_and_special_entities() {
        for (id, keys, destination) in [
            (
                "minecraft:cow",
                ["SleepingX", "SleepingY", "SleepingZ"],
                "sleeping_pos",
            ),
            ("minecraft:vex", ["BoundX", "BoundY", "BoundZ"], "bound_pos"),
            ("minecraft:phantom", ["AX", "AY", "AZ"], "anchor_pos"),
            (
                "minecraft:turtle",
                ["HomePosX", "HomePosY", "HomePosZ"],
                "home_pos",
            ),
            (
                "minecraft:painting",
                ["TileX", "TileY", "TileZ"],
                "block_pos",
            ),
        ] {
            let mut tag = entity(
                id,
                vec![
                    (keys[0], Tag::Int(1)),
                    (keys[1], Tag::Int(2)),
                    (keys[2], Tag::Int(3)),
                ],
            );
            run_entity(&mut tag, 4314);
            assert_eq!(get(&tag, destination), Some(&Tag::IntArray(vec![1, 2, 3])));
            assert!(get(&tag, keys[0]).is_none());
        }
        let mut partial = entity(
            "minecraft:vex",
            vec![("BoundX", Tag::Int(1)), ("LifeTicks", Tag::Int(10))],
        );
        run_entity(&mut partial, 4314);
        assert_eq!(get(&partial, "BoundX"), Some(&Tag::Int(1)));
        assert_eq!(get(&partial, "life_ticks"), Some(&Tag::Int(10)));
    }

    #[test]
    fn player_inline_positions_4314_build_respawn_and_entered_nether() {
        let mut player = compound(vec![
            ("SpawnX", Tag::Int(1)),
            ("SpawnY", Tag::Int(2)),
            ("SpawnZ", Tag::Int(3)),
            ("SpawnAngle", Tag::Float(45.0)),
            ("SpawnDimension", text("minecraft:the_nether")),
            ("SpawnForced", Tag::Byte(1)),
            (
                "enteredNetherPosition",
                compound(vec![("x", Tag::Double(1.5)), ("z", Tag::Int(-4))]),
            ),
        ]);
        apply(&mut player, NodeKind::Player, 4314);
        let respawn = get(&player, "respawn").unwrap();
        assert_eq!(get(respawn, "pos"), Some(&Tag::IntArray(vec![1, 2, 3])));
        assert_eq!(get(respawn, "angle"), Some(&Tag::Float(45.0)));
        assert_eq!(get(respawn, "forced"), Some(&Tag::Byte(1)));
        assert_eq!(
            get(&player, "entered_nether_pos"),
            Some(&tags(vec![
                Tag::Double(1.5),
                Tag::Double(0.0),
                Tag::Double(-4.0)
            ]))
        );
        assert!(get(&player, "SpawnX").is_none());
    }

    #[test]
    fn unconverted_particle_4420_and_icon_default_4424_fail_closed() {
        for (mut tag, kind, version) in [
            (
                entity(
                    "minecraft:area_effect_cloud",
                    vec![("Particle", text("minecraft:smoke"))],
                ),
                NodeKind::Entity,
                4420,
            ),
            (
                compound(vec![("locator_bar_icon", compound(vec![]))]),
                NodeKind::Player,
                4424,
            ),
            (
                entity(
                    "minecraft:cow",
                    vec![("locator_bar_icon", compound(vec![]))],
                ),
                NodeKind::Entity,
                4424,
            ),
        ] {
            let before = tag.clone();
            assert!(fix_node(&mut tag, kind, version, &mut UpgradeReport::default()).is_err());
            assert_eq!(tag, before);
        }
        let mut absent = compound(vec![]);
        apply(&mut absent, NodeKind::Player, 4424);
        assert_eq!(absent, compound(vec![]));
    }

    #[test]
    fn copper_golem_weather_4535() {
        for (value, expected) in [
            (0, "unaffected"),
            (1, "exposed"),
            (2, "weathered"),
            (3, "oxidized"),
            (99, "unaffected"),
        ] {
            let mut golem = entity(
                "minecraft:copper_golem",
                vec![("weather_state", Tag::Int(value))],
            );
            run_entity(&mut golem, 4535);
            assert_eq!(get(&golem, "weather_state"), Some(&text(expected)));
        }
    }

    #[test]
    fn villagers_4763_are_finalized_unconditionally() {
        for id in ["minecraft:villager", "minecraft:zombie_villager"] {
            let mut villager = entity(id, vec![("VillagerDataFinalized", Tag::Byte(0))]);
            run_entity(&mut villager, 4763);
            assert_eq!(get(&villager, "VillagerDataFinalized"), Some(&Tag::Byte(1)));
        }
    }

    #[test]
    fn opaque_custom_data_and_unassigned_kinds_are_preserved() {
        let opaque = compound(vec![
            ("ActiveEffects", Tag::Int(9)),
            ("FallDistance", Tag::Float(2.0)),
            ("Motive", text("keep")),
        ]);
        let mut entity = entity("mod:unknown", vec![("custom_data", opaque.clone())]);
        for version in [
            3086, 3090, 3322, 3568, 3813, 3814, 3945, 4055, 4303, 4314, 4424, 4763,
        ] {
            run_entity(&mut entity, version);
        }
        assert_eq!(get(&entity, "custom_data"), Some(&opaque));
        let mut item = opaque.clone();
        apply(&mut item, NodeKind::Item, 4303);
        assert_eq!(item, opaque);
    }

    #[test]
    fn malformed_typed_lists_fail_with_exact_step_receipts() {
        for (id, key, version, expected_fix) in [
            (
                "minecraft:donkey",
                "Items",
                3809,
                "ChestedHorsesInventoryZeroIndexingFix",
            ),
            ("minecraft:wolf", "Attributes", 3812, "FixWolfHealth"),
            (
                "minecraft:cow",
                "Attributes",
                3814,
                "AttributesRenameLegacy",
            ),
            (
                "minecraft:cow",
                "Attributes",
                3945,
                "AttributeModifierIdFix",
            ),
            ("minecraft:cow", "attributes", 4055, "AttributeIdPrefixFix"),
            (
                "minecraft:bee",
                "attributes",
                4187,
                "EntityAttributeBaseValueFix",
            ),
            (
                "minecraft:cow",
                "ArmorDropChances",
                4293,
                "DropChancesFormatFix",
            ),
            ("minecraft:cow", "ArmorItems", 4301, "EquipmentFormatFix"),
        ] {
            let mut value = entity(id, vec![(key, Tag::Int(9))]);
            let error = fix_node(
                &mut value,
                NodeKind::Entity,
                version,
                &mut UpgradeReport::default(),
            )
            .unwrap_err();
            assert!(
                matches!(error, UpgradeError::UnsupportedField { version: actual_version, fix, .. }
                if actual_version == version && fix == expected_fix)
            );
        }
        let mut player = compound(vec![("Inventory", Tag::Int(9))]);
        assert!(matches!(
            fix_node(
                &mut player,
                NodeKind::Player,
                4312,
                &mut UpgradeReport::default()
            )
            .unwrap_err(),
            UpgradeError::UnsupportedField {
                version: 4312,
                fix: "PlayerEquipmentFix",
                ..
            }
        ));
        let mut horse = entity(
            "minecraft:horse",
            vec![("ArmorItem", compound(vec![])), ("ArmorItems", Tag::Int(9))],
        );
        assert!(matches!(
            fix_node(
                &mut horse,
                NodeKind::Entity,
                3808,
                &mut UpgradeReport::default()
            )
            .unwrap_err(),
            UpgradeError::UnsupportedField {
                version: 3808,
                fix: "HorseBodyArmorItemFix",
                ..
            }
        ));
    }

    #[test]
    fn malformed_effect_factor_3322_and_hidden_instance_3568_fail_closed() {
        let mut living = compound(vec![(
            "ActiveEffects",
            tags(vec![compound(vec![("FactorCalculationData", Tag::Int(9))])]),
        )]);
        assert!(matches!(
            fix_node(
                &mut living,
                NodeKind::Player,
                3322,
                &mut UpgradeReport::default()
            )
            .unwrap_err(),
            UpgradeError::UnsupportedField {
                version: 3322,
                fix: "EffectDurationFix",
                ..
            }
        ));
        let mut living = compound(vec![(
            "ActiveEffects",
            tags(vec![compound(vec![
                ("Id", Tag::Int(1)),
                ("HiddenEffect", Tag::Int(9)),
            ])]),
        )]);
        assert!(matches!(
            fix_node(
                &mut living,
                NodeKind::Entity,
                3568,
                &mut UpgradeReport::default()
            )
            .unwrap_err(),
            UpgradeError::UnsupportedField {
                version: 3568,
                fix: "MobEffectIdFix",
                ..
            }
        ));
    }

    #[test]
    fn particle_and_default_rejections_include_explicit_fix_receipts() {
        for (mut value, version, expected_fix) in [
            (
                entity(
                    "minecraft:area_effect_cloud",
                    vec![("Particle", text("minecraft:item stone{Damage:1}"))],
                ),
                3818,
                "ParticleUnflatteningFix",
            ),
            (
                entity(
                    "minecraft:area_effect_cloud",
                    vec![("Particle", text("minecraft:smoke"))],
                ),
                4420,
                "NamedEntityConvertUncheckedFix",
            ),
            (
                entity(
                    "minecraft:cow",
                    vec![("locator_bar_icon", compound(vec![]))],
                ),
                4424,
                "AddFieldFix",
            ),
        ] {
            let before = value.clone();
            let error = fix_node(
                &mut value,
                NodeKind::Entity,
                version,
                &mut UpgradeReport::default(),
            )
            .unwrap_err();
            assert!(
                matches!(error, UpgradeError::UnsupportedField { version: actual_version, fix, .. }
                if actual_version == version && fix == expected_fix)
            );
            assert_eq!(value, before);
        }
    }

    #[test]
    fn applied_receipts_are_deduplicated() {
        let mut goat = entity("minecraft:goat", vec![]);
        let mut report = UpgradeReport::default();
        fix_node(&mut goat, NodeKind::Entity, 3093, &mut report).unwrap();
        fix_node(&mut goat, NodeKind::Entity, 3093, &mut report).unwrap();
        assert_eq!(report.steps.len(), 1);
        assert_eq!(report.steps[0].version, 3093);
        assert_eq!(report.steps[0].fix, "EntityGoatMissingStateFix");
    }

    #[test]
    fn horse_armor_3808_rejects_heterogeneous_output_before_mutating() {
        let mut horse = entity(
            "minecraft:horse",
            vec![
                (
                    "ArmorItem",
                    compound(vec![("id", text("minecraft:iron_horse_armor"))]),
                ),
                (
                    "ArmorDropChances",
                    tags(vec![Tag::Double(0.0), Tag::Double(0.0), Tag::Double(2.0)]),
                ),
            ],
        );
        let before = horse.clone();
        let error = fix_node(
            &mut horse,
            NodeKind::Entity,
            3808,
            &mut UpgradeReport::default(),
        )
        .unwrap_err();
        assert!(matches!(
            error,
            UpgradeError::UnsupportedField {
                version: 3808,
                fix: "HorseBodyArmorItemFix",
                ..
            }
        ));
        assert_eq!(horse, before);
    }

    #[test]
    fn cloud_particle_3818_then_schema_cast_4420_preserves_simple_particle() {
        let mut cloud = entity(
            "minecraft:area_effect_cloud",
            vec![
                ("Particle", text("entity_effect")),
                ("Duration", Tag::Int(100)),
            ],
        );
        let mut report = UpgradeReport::default();
        fix_node(&mut cloud, NodeKind::Entity, 3818, &mut report).unwrap();
        let particle = get(&cloud, "Particle").unwrap().clone();
        assert_eq!(
            get(&particle, "type"),
            Some(&text("minecraft:entity_effect"))
        );
        fix_node(&mut cloud, NodeKind::Entity, 4420, &mut report).unwrap();
        assert!(get(&cloud, "Particle").is_none());
        assert_eq!(get(&cloud, "custom_particle"), Some(&particle));
        assert_eq!(get(&cloud, "Duration"), Some(&Tag::Int(100)));
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.version == 3818 && step.fix == "ParticleUnflatteningFix")
        );
        assert!(
            report
                .steps
                .iter()
                .any(|step| step.version == 4420 && step.fix == "NamedEntityConvertUncheckedFix")
        );
    }

    #[test]
    fn source_3808_skips_horse_revision_zero_but_runs_llama_revisions() {
        let item = compound(vec![("id", text("minecraft:iron_horse_armor"))]);
        let mut report = UpgradeReport {
            from: 3808,
            ..UpgradeReport::default()
        };
        let mut horse = entity("minecraft:horse", vec![("ArmorItem", item.clone())]);
        fix_node(&mut horse, NodeKind::Entity, 3808, &mut report).unwrap();
        assert_eq!(get(&horse, "ArmorItem"), Some(&item));
        assert!(get(&horse, "body_armor_item").is_none());
        for id in ["minecraft:llama", "minecraft:trader_llama"] {
            let mut llama = entity(id, vec![("DecorItem", item.clone())]);
            fix_node(&mut llama, NodeKind::Entity, 3808, &mut report).unwrap();
            assert_eq!(get(&llama, "body_armor_item"), Some(&item));
            assert!(get(&llama, "DecorItem").is_none());
        }
    }
}
