//! Import-only legacy item conversion: numeric ids, flattened variants,
//! display text, enchantments and UUIDs. Unknown optional tags remain intact.
//! Required numeric item ids without a known mapping fail at `id` rather than
//! silently becoming air. Modern conversion continues through `items`.
//!
//! Historical lookup provenance is recorded alongside `tables` and `renames`;
//! local ambiguity receipts are under `.analysis/codex-logs/old-dataversion/`.

use super::{NodeKind, UpgradeError, UpgradeReport, nbt};
use mc_nbt::Tag;

mod renames;
mod tables;
mod text;
mod uuid;

pub(super) use text::{create_text_component_json, rewrite_from_lenient};
pub(super) use uuid::parse_uuid;

#[cfg(test)]
mod tests;

/// Historical item schema hook, before walking nested entity/block-entity data.
pub(super) fn prepare_legacy_item(item: &mut Tag, version: i32) {
    if version >= 705 {
        super::items::prepare_legacy_item(item);
        return;
    }
    let Some(name) = id(item) else { return };
    let name = name.strip_prefix("minecraft:").unwrap_or(name);
    let entity_id = match name {
        "armor_stand" => Some("ArmorStand"),
        "painting" => Some("Painting"),
        _ => None,
    };
    let block_id = if version < 704 {
        match name {
            "furnace" | "lit_furnace" => Some("Furnace"),
            "chest" | "trapped_chest" => Some("Chest"),
            "ender_chest" => Some("EnderChest"),
            "jukebox" => Some("RecordPlayer"),
            "dispenser" => Some("Trap"),
            "dropper" => Some("Dropper"),
            "sign" | "standing_sign" | "wall_sign" => Some("Sign"),
            "mob_spawner" => Some("MobSpawner"),
            "noteblock" => Some("Music"),
            "brewing_stand" => Some("Cauldron"),
            "enhanting_table" => Some("EnchantTable"),
            "command_block" | "repeating_command_block" | "chain_command_block" => {
                Some("CommandBlock")
            }
            "beacon" => Some("Beacon"),
            "skull" => Some("Skull"),
            "daylight_detector" | "daylight_detector_inverted" => Some("DLDetector"),
            "hopper" => Some("Hopper"),
            "banner" | "wall_banner" | "standing_banner" | "shield" => Some("Banner"),
            "flower_pot" => Some("FlowerPot"),
            "piston_head" => Some("Piston"),
            "unpowered_comparator" | "powered_comparator" => Some("Comparator"),
            "structure_block" => Some("Structure"),
            "end_portal" => Some("Airportal"),
            "end_gateway" => Some("EndGateway"),
            _ => None,
        }
    } else {
        None
    };
    let missing_entity_id = nbt::get(item, "tag")
        .and_then(|tag| nbt::get(tag, "EntityTag"))
        .is_some_and(|entity| nbt::get(entity, "id").is_none());
    if version >= 704 {
        super::items::prepare_legacy_item(item);
    }
    let Some(tag) = nbt::get_mut(item, "tag") else {
        return;
    };
    if let (Some(id), Some(block)) = (block_id, nbt::get_mut(tag, "BlockEntityTag")) {
        nbt::set(block, "id", Tag::String(id.into()));
    }
    if missing_entity_id
        && let (Some(id), Some(entity)) = (entity_id, nbt::get_mut(tag, "EntityTag"))
    {
        nbt::set(entity, "id", Tag::String(id.into()));
    }
}

pub(super) fn numeric_item(id: i32) -> &'static str {
    tables::NUMERIC_ITEMS
        .iter()
        .find(|(candidate, _)| *candidate == id)
        .map_or("minecraft:air", |(_, name)| *name)
}

pub(super) fn flattened_item(id: &str, damage: i32) -> Option<&'static str> {
    let mut fallback = None;
    for &(key, name) in tables::FLATTEN_MAP {
        let Some((base, variant)) = key.rsplit_once('.') else {
            continue;
        };
        if base != id && base.strip_prefix("minecraft:") != Some(id) {
            continue;
        }
        if variant == "0" {
            fallback = Some(name);
        }
        if variant.parse::<i32>().ok() == Some(damage) {
            return Some(name);
        }
    }
    fallback
}

/// Migrate one item-stack node at one legacy schema version. All other node
/// kinds and versions are no-ops; the parent owns version order and tree walk.
pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if !matches!(kind, NodeKind::Item) {
        return Ok(());
    }
    match version {
        102 => {
            numeric_id(tag, report)?;
            potion(tag, report)?;
        }
        105 => spawn_egg_105(tag, report)?,
        502 | 820 | 1480 | 1483 | 1484 | 1486 | 1487 | 1488 | 1490 | 1510 | 1800 | 1802 | 1928
        | 2209 | 2508 | 2509 | 2528 => rename(tag, version, report),
        804 => banner_color(tag, report),
        806 => water_potion(tag, report)?,
        813 => shulker_color(tag, report)?,
        1125 => bed_color(tag, report),
        1451 => flatten(tag, report)?,
        1458 => display_name(tag, report),
        1494 => enchant_names(tag, report)?,
        1803 => lore(tag, report),
        1904 => cat_split(tag, report),
        1948 => ominous_banner(tag, report),
        2514 => item_uuids(tag, report),
        2523 => attributes(tag, report),
        _ => {}
    }
    Ok(())
}

fn unsupported(version: i32, fix: &'static str, path: &str, reason: &str) -> UpgradeError {
    UpgradeError::unsupported(version, fix, path, reason)
}

fn id(tag: &Tag) -> Option<&str> {
    nbt::get(tag, "id").and_then(nbt::string)
}

fn set_id(tag: &mut Tag, id: &str) {
    nbt::set(tag, "id", Tag::String(id.into()));
}

fn damage(tag: &Tag) -> i32 {
    nbt::get(tag, "Damage").map_or(0, |value| match value {
        Tag::Long(value) => *value as i32,
        Tag::Double(value) => *value as i32,
        Tag::Float(value) => *value as i32,
        _ => nbt::int(value).unwrap_or_else(|| nbt::number(value).unwrap_or(0.0) as i32),
    })
}

fn legacy<'a>(
    tag: &'a mut Tag,
    version: i32,
    fix: &'static str,
) -> Result<Option<&'a mut Tag>, UpgradeError> {
    Ok(match nbt::get_mut(tag, "tag") {
        None => None,
        Some(tag) if nbt::fields(tag).is_some() => Some(tag),
        Some(_) => {
            return Err(unsupported(
                version,
                fix,
                "tag",
                "legacy item tag is not a compound",
            ));
        }
    })
}

/// Vanilla `getOrCreateTyped(tag)`: return the `tag` compound, creating it when
/// absent. Errors only when a non-compound `tag` is present.
fn legacy_or_create<'a>(
    tag: &'a mut Tag,
    version: i32,
    fix: &'static str,
) -> Result<&'a mut Tag, UpgradeError> {
    if nbt::get(tag, "tag").is_none() {
        nbt::set(tag, "tag", nbt::compound(vec![]));
    }
    match nbt::get_mut(tag, "tag") {
        Some(tag) if nbt::fields(tag).is_some() => Ok(tag),
        _ => Err(unsupported(
            version,
            fix,
            "tag",
            "legacy item tag is not a compound",
        )),
    }
}

// ItemIdFix102: ITEM_STACK `id` int -> namespaced string via ITEM_NAMES.
// makeRule's input type is `int | namespacedString`, so a string id passes
// through; every numeric id (registered or not) goes through `getItem`.
fn numeric_id(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "ItemIdFix";
    const VERSION: i32 = 102;
    let Some(id) = nbt::get(tag, "id") else {
        return Ok(());
    };
    if nbt::string(id).is_some() {
        return Ok(());
    }
    let numeric = match id {
        Tag::Long(value) => *value as i32,
        _ => nbt::int(id)
            .or_else(|| nbt::number(id).map(|value| value as i32))
            .ok_or_else(|| unsupported(VERSION, FIX, "id", "numeric item id is not an integer"))?,
    };
    let name = if numeric == 0 {
        "minecraft:air"
    } else {
        tables::NUMERIC_ITEMS
            .iter()
            .find(|(candidate, _)| *candidate == numeric)
            .map(|(_, name)| *name)
            .ok_or_else(|| unsupported(VERSION, FIX, "id", "unknown required numeric item id"))?
    };
    set_id(tag, name);
    report.applied(VERSION, FIX);
    Ok(())
}

// ItemPotionFix102: `minecraft:potion` only. Damage selects POTIONS[Damage & 127]
// when `tag.Potion` is absent; null slots fall back to water. Damage bit 16384
// (splash flag) rewrites the id to splash_potion. Nonzero Damage resets to 0.
fn potion(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "ItemPotionFix";
    const VERSION: i32 = 102;
    if id(tag) != Some("minecraft:potion") {
        return Ok(());
    }
    let damage = damage(tag);
    let changed = potion_inner(tag, damage)?;
    if damage != 0 {
        nbt::set(tag, "Damage", Tag::Short(0));
    }
    if changed || damage != 0 {
        report.applied(VERSION, FIX);
    }
    Ok(())
}

fn potion_inner(tag: &mut Tag, damage: i32) -> Result<bool, UpgradeError> {
    let present = match legacy(tag, 102, "ItemPotionFix")? {
        Some(legacy) => {
            if nbt::get(legacy, "Potion").and_then(nbt::string).is_some() {
                return Ok(false);
            }
            let potion = tables::POTIONS[(damage & 127) as usize].unwrap_or("minecraft:water");
            nbt::set(legacy, "Potion", Tag::String(potion.into()));
            true
        }
        None => return Ok(false),
    };
    if present && damage & 16384 == 16384 {
        set_id(tag, "minecraft:splash_potion");
    }
    Ok(present)
}

// ItemSpawnEggFix105: generic `minecraft:spawn_egg` + Damage -> EntityTag id.
// ID_TO_ENTITY[Damage & 255] holds legacy (often unnamespaced) entity names; the
// item id itself is untouched at this version (per-egg item ids only arrive with
// the 1451r5 flattening map). Null slots leave the stack untouched. When the
// existing EntityTag id already equals the mapped name, vanilla still rewrites
// the tag (readAndSet) and zeroes root Damage only if it was nonzero; the steps
// below report the fix whenever the tag was (re)written.
fn spawn_egg_105(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "ItemSpawnEggFix";
    if id(tag) != Some("minecraft:spawn_egg") {
        return Ok(());
    }
    let damage = damage(tag) & 255;
    let Some(entity) = tables::SPAWN_ENTITIES_105
        .get(damage as usize)
        .copied()
        .flatten()
    else {
        return Ok(());
    };
    let legacy = legacy_or_create(tag, 105, FIX)?;
    let entity_tag = match nbt::get_mut(legacy, "EntityTag") {
        Some(entity_tag) if nbt::fields(entity_tag).is_some() => entity_tag,
        _ => {
            nbt::set(legacy, "EntityTag", nbt::compound(vec![]));
            nbt::get_mut(legacy, "EntityTag").expect("just set EntityTag")
        }
    };
    nbt::set(entity_tag, "id", Tag::String(entity.into()));
    if damage != 0 {
        nbt::set(tag, "Damage", Tag::Short(0));
    }
    report.applied(105, FIX);
    Ok(())
}

// ItemRenameFix.create registrations before 2586 (exact maps in old_items/renames.rs).
// ItemRenameFix only rewrites ITEM_NAME-typed ids; non-minecraft namespaces pass through.
fn rename(tag: &mut Tag, version: i32, report: &mut UpgradeReport) {
    let Some(current) = id(tag) else { return };
    let current = current.to_owned();
    if let Some(new) = renames::renamed(&current, version) {
        set_id(tag, new);
        report.applied(version, "ItemRenameFix");
    }
}

// ItemBannerColorFix804: `minecraft:banner` + tag.BlockEntityTag.Base number ->
// Damage = Base & 15. A display compound exactly `{Lore:["(+NBT"]}` suppresses the
// rewrite (vanilla returns the id-only update: Damage is still written, but the
// Base is kept). Otherwise Base is removed.
fn banner_color(tag: &mut Tag, report: &mut UpgradeReport) {
    const FIX: &str = "ItemBannerColorFix";
    const VERSION: i32 = 804;
    if id(tag) != Some("minecraft:banner") {
        return;
    }
    let base = nbt::get_mut(tag, "tag")
        .and_then(|tag| nbt::get_mut(tag, "BlockEntityTag"))
        .and_then(|block| nbt::get(block, "Base"))
        .and_then(nbt::number)
        .map(|value| value as i32 & 15);
    let Some(base) = base else { return };
    nbt::set(tag, "Damage", Tag::Short(base as i16));
    let sentinel = nbt::get_mut(tag, "tag")
        .and_then(|tag| nbt::get(tag, "display"))
        .is_some_and(|display| {
            nbt::fields(display).is_some_and(|fields| {
                fields.len() == 1
                    && fields[0].0 == "Lore"
                    && matches!(&fields[0].1, Tag::List(list) if list.elements.len() == 1
                        && matches!(&list.elements[0], Tag::String(line) if line == "(+NBT"))
            })
        });
    if sentinel {
        report.applied(VERSION, FIX);
        return;
    }
    if let Some(block) =
        nbt::get_mut(tag, "tag").and_then(|tag| nbt::get_mut(tag, "BlockEntityTag"))
    {
        nbt::remove(block, "Base");
    }
    report.applied(VERSION, FIX);
}

// ItemWaterPotionFix806: potion/splash/lingering/tipped_arrow without tag.Potion
// gains `Potion: minecraft:water`.
fn water_potion(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "ItemWaterPotionFix";
    const VERSION: i32 = 806;
    if !matches!(
        id(tag),
        Some(
            "minecraft:potion"
                | "minecraft:splash_potion"
                | "minecraft:lingering_potion"
                | "minecraft:tipped_arrow"
        )
    ) {
        return Ok(());
    }
    let missing = match nbt::get_mut(tag, "tag") {
        Some(legacy) if nbt::fields(legacy).is_some() => {
            nbt::get(legacy, "Potion").and_then(nbt::string).is_none()
        }
        Some(_) => {
            return Err(unsupported(
                VERSION,
                FIX,
                "tag",
                "legacy item tag is not a compound",
            ));
        }
        None => true,
    };
    if !missing {
        return Ok(());
    }
    // getOrCreateTyped: a missing `tag` compound is created.
    let legacy = legacy_or_create(tag, VERSION, FIX)?;
    nbt::set(legacy, "Potion", Tag::String("minecraft:water".into()));
    report.applied(VERSION, FIX);
    Ok(())
}

// ItemShulkerBoxColorFix813: `minecraft:shulker_box` + tag.BlockEntityTag.Color int ->
// id = NAMES_BY_COLOR[Color % 16]; Color is removed.
fn shulker_color(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "ItemShulkerBoxColorFix";
    const VERSION: i32 = 813;
    if id(tag) != Some("minecraft:shulker_box") {
        return Ok(());
    }
    let color = match legacy(tag, VERSION, FIX)? {
        Some(legacy) => match nbt::get_mut(legacy, "BlockEntityTag") {
            Some(block) => match nbt::get(block, "Color") {
                Some(color) => nbt::number(color).map(|value| value as i32),
                None => return Ok(()),
            },
            None => return Ok(()),
        },
        None => return Ok(()),
    };
    let Some(color) = color else { return Ok(()) };
    if let Some(block) =
        nbt::get_mut(tag, "tag").and_then(|tag| nbt::get_mut(tag, "BlockEntityTag"))
    {
        nbt::remove(block, "Color");
    }
    let index = color.rem_euclid(16) as usize;
    set_id(tag, tables::SHULKER_COLORS[index]);
    report.applied(VERSION, FIX);
    Ok(())
}

// BedItemColorFix1125: `minecraft:bed` with Damage == 0 becomes Damage 14 (red bed).
fn bed_color(tag: &mut Tag, report: &mut UpgradeReport) {
    if id(tag) != Some("minecraft:bed") {
        return;
    }
    if damage(tag) != 0 {
        return;
    }
    nbt::set(tag, "Damage", Tag::Short(14));
    report.applied(1125, "BedItemColorFix");
}

// Flatten historical item variants; damageable items retain durability in tag.
fn flatten(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "ItemStackTheFlatteningFix";
    const VERSION: i32 = 1451;
    let Some(current) = id(tag) else {
        return Ok(());
    };
    if current.contains(':') && !current.starts_with("minecraft:") {
        return Ok(());
    }
    let name = current.strip_prefix("minecraft:").unwrap_or(current);
    let is_map = name == "filled_map";
    let remove_block = matches!(name, "noteblock" | "flower_pot");
    let spawn_egg = name == "spawn_egg";
    let damageable = tables::FLATTEN_DAMAGE_IDS.iter().any(|candidate| {
        *candidate == current || candidate.strip_prefix("minecraft:") == Some(current)
    });
    let damage = damage(tag);
    let flattened = flattened_item(current, damage);
    if is_map {
        let legacy = legacy_or_create(tag, VERSION, "ItemStackMapIdFix")?;
        nbt::set(legacy, "map", Tag::Int(damage));
        report.applied(VERSION, "ItemStackMapIdFix");
    }
    if let Some(new) = flattened {
        set_id(tag, new);
        report.applied(VERSION, FIX);
    }
    // r5 RemoveBlockEntityTagFix (item branch): drop tag.BlockEntityTag for
    // noteblock/flower_pot items. The falling-block/structure branches touch
    // entity/chunk types owned elsewhere.
    if remove_block
        && let Some(legacy) = nbt::get_mut(tag, "tag")
        && nbt::get(legacy, "BlockEntityTag").is_some()
    {
        nbt::remove(legacy, "BlockEntityTag");
        report.applied(VERSION, "RemoveBlockEntityTagFix");
    }
    // r5 ItemStackSpawnEggFix (itemType `minecraft:spawn_egg`): tag.EntityTag.id ->
    // item id via MAP, default pig_spawn_egg.
    if spawn_egg
        && let Some(entity) = nbt::get_mut(tag, "tag")
            .and_then(|tag| nbt::get(tag, "EntityTag"))
            .and_then(|entity| nbt::get(entity, "id"))
            .and_then(nbt::string)
    {
        let egg = tables::SPAWN_EGGS_1451
            .iter()
            .find(|(id, _)| *id == entity)
            .map_or("minecraft:pig_spawn_egg", |(_, egg)| *egg);
        set_id(tag, egg);
        report.applied(VERSION, "ItemStackSpawnEggFix");
    }
    // Damage routing: DAMAGE_IDS keep it as tag.Damage; everything else drops it.
    if damageable {
        let legacy = legacy_or_create(tag, VERSION, FIX)?;
        nbt::set(legacy, "Damage", Tag::Int(damage));
    }
    if nbt::get(tag, "Damage").is_some() {
        nbt::remove(tag, "Damage");
    }
    Ok(())
}

// ItemStackEnchantmentNamesFix1494: tag.ench[] -> tag.Enchantments[] with numeric
// `id` mapped through MAP (`getOrDefault(id, "null")`); tag.StoredEnchantments[]
// ids are mapped in place. `id` fields are read with `asInt(0)` (short/int/long
// all coerce; strings read 0 -> protection).
fn enchant_names(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    const FIX: &str = "ItemStackEnchantmentNamesFix";
    const VERSION: i32 = 1494;
    let Some(legacy) = legacy(tag, VERSION, FIX)? else {
        return Ok(());
    };
    let mut changed = false;
    if let Some(entries) = nbt::take(legacy, "ench")
        && let Tag::List(list) = entries
    {
        let mut mapped = Vec::with_capacity(list.elements.len());
        for mut entry in list.elements {
            if nbt::fields(&entry).is_some() {
                let numeric = nbt::get(&entry, "id").map_or(0, |id| match id {
                    Tag::Long(value) => *value as i32,
                    _ => nbt::int(id).unwrap_or_else(|| nbt::number(id).unwrap_or(0.0) as i32),
                });
                let name = tables::ENCHANTMENTS
                    .iter()
                    .find(|(id, _)| *id == numeric)
                    .map_or("null", |(_, name)| *name);
                nbt::set(&mut entry, "id", Tag::String(name.into()));
                changed = true;
            }
            mapped.push(entry);
        }
        nbt::set(
            legacy,
            "Enchantments",
            Tag::List(mc_nbt::ListTag {
                element_type: mapped.first().map_or(10, Tag::type_id),
                elements: mapped,
            }),
        );
    }
    for key in ["StoredEnchantments"] {
        if let Some(entries) = nbt::get_mut(legacy, key).and_then(nbt::list_mut) {
            for entry in entries.iter_mut() {
                if nbt::fields(entry).is_none() {
                    continue;
                }
                let numeric = nbt::get(entry, "id").map_or(0, |id| match id {
                    Tag::Long(value) => *value as i32,
                    _ => nbt::int(id).unwrap_or_else(|| nbt::number(id).unwrap_or(0.0) as i32),
                });
                let name = tables::ENCHANTMENTS
                    .iter()
                    .find(|(id, _)| *id == numeric)
                    .map_or("null", |(_, name)| *name);
                nbt::set(entry, "id", Tag::String(name.into()));
                changed = true;
            }
        }
    }
    if changed {
        report.applied(VERSION, FIX);
    }
    Ok(())
}

fn display_name(tag: &mut Tag, report: &mut UpgradeReport) {
    if let Some(Tag::String(name)) = nbt::get_mut(tag, "tag")
        .and_then(|tag| nbt::get_mut(tag, "display"))
        .and_then(|display| nbt::get_mut(display, "Name"))
    {
        *name = create_text_component_json(name);
        report.applied(1458, "ItemCustomNameToComponentFix");
    }
}

// Lore entries are literal text, including JSON-looking source strings.
fn lore(tag: &mut Tag, report: &mut UpgradeReport) {
    let Some(lines) = nbt::get_mut(tag, "tag")
        .and_then(|tag| nbt::get_mut(tag, "display"))
        .and_then(|display| nbt::get_mut(display, "Lore"))
        .and_then(nbt::list_mut)
    else {
        return;
    };
    let mut changed = false;
    for line in lines {
        if let Tag::String(text) = line {
            *text = create_text_component_json(text);
            changed = true;
        }
    }
    if changed {
        report.applied(1803, "ItemLoreFix");
    }
}

// EntityCatSplitFix1904 (item branch): `minecraft:cat_spawn_egg` -> ocelot egg.
// The entity split itself is owned by old_entities.
fn cat_split(tag: &mut Tag, report: &mut UpgradeReport) {
    if id(tag) != Some("minecraft:cat_spawn_egg") {
        return;
    }
    set_id(tag, "minecraft:ocelot_spawn_egg");
    report.applied(1904, "EntityCatSplitFix");
}

// OminousBannerRenameFix1948 (ItemStackTagFix on white_banner): display.Name string
// `"translate":"block.minecraft.illager_banner"` -> ominous_banner token.
fn ominous_banner(tag: &mut Tag, report: &mut UpgradeReport) {
    const FIX: &str = "OminousBannerRenameFix";
    const VERSION: i32 = 1948;
    if id(tag) != Some("minecraft:white_banner") {
        return;
    }
    let replaced = nbt::get_mut(tag, "tag")
        .and_then(|tag| nbt::get_mut(tag, "display"))
        .and_then(|display| nbt::get_mut(display, "Name"))
        .and_then(|name| match name {
            Tag::String(line)
                if line.contains("\"translate\":\"block.minecraft.illager_banner\"") =>
            {
                *line = line.replace(
                    "\"translate\":\"block.minecraft.illager_banner\"",
                    "\"translate\":\"block.minecraft.ominous_banner\"",
                );
                Some(())
            }
            _ => None,
        })
        .is_some();
    if replaced {
        report.applied(VERSION, FIX);
    }
}

// ItemStackUUIDFix2514 (AbstractUUIDFix on ITEM_STACK): tag.AttributeModifiers[].UUID
// Least/Most -> int-array UUID (zero halves stay); player_head tag.SkullOwner.Id
// string -> int-array Id. Reuses old_player_map::replace_uuid (same
// createUUIDFromLongs zero-half rejection).
fn item_uuids(tag: &mut Tag, report: &mut UpgradeReport) {
    const FIX: &str = "ItemStackUUIDFix";
    const VERSION: i32 = 2514;
    let is_head = id(tag) == Some("minecraft:player_head");
    let mut changed = false;
    if let Some(legacy) = nbt::get_mut(tag, "tag").filter(|tag| nbt::fields(tag).is_some()) {
        if let Some(modifiers) = nbt::get_mut(legacy, "AttributeModifiers").and_then(nbt::list_mut)
        {
            for modifier in modifiers.iter_mut() {
                changed |= super::old_player_map::replace_uuid(modifier, "UUID", "UUID");
            }
        }
        if is_head
            && let Some(owner) = nbt::get_mut(legacy, "SkullOwner")
            && nbt::fields(owner).is_some()
            && let Some(uuid) = nbt::get(owner, "Id")
                .and_then(nbt::string)
                .and_then(parse_uuid)
        {
            nbt::set(owner, "Id", Tag::IntArray(uuid.into()));
            changed = true;
        }
    }
    if changed {
        report.applied(VERSION, FIX);
    }
}

// AttributesRenameLegacy2523 (ITEM_STACK branch): tag.AttributeModifiers[].
// AttributeName through old_player_map::rename_attribute_name (exact,
// non-namespacing lookup shared with the player branch).
fn attributes(tag: &mut Tag, report: &mut UpgradeReport) {
    const FIX: &str = "AttributesRenameLegacy";
    const VERSION: i32 = 2523;
    let mut changed = false;
    if let Some(legacy) = nbt::get_mut(tag, "tag").filter(|tag| nbt::fields(tag).is_some())
        && let Some(modifiers) = nbt::get_mut(legacy, "AttributeModifiers").and_then(nbt::list_mut)
    {
        for modifier in modifiers.iter_mut() {
            if let Some(name) = nbt::get(modifier, "AttributeName")
                .and_then(nbt::string)
                .and_then(super::old_player_map::rename_attribute_name)
            {
                nbt::set(modifier, "AttributeName", Tag::String(name.into()));
                changed = true;
            }
        }
    }
    if changed {
        report.applied(VERSION, FIX);
    }
}
