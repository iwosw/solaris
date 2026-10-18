use mc_nbt::{ListTag, Tag};

use super::nbt::{
    compound, fields, fields_mut, get, get_mut, int, list, list_mut, number, remove, rename, set,
    string, take,
};
use super::{NodeKind, UpgradeError, UpgradeReport};

mod schema;
mod tables;

const COMPONENTIZE: &str = "ItemStackComponentizationFix";

// Evidence: .analysis/old-world-upgrade/*Fix.javap.txt, vanilla 26.1.2.
// Only schema-typed item/component nodes are visited; custom_data is opaque.
pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    match kind {
        NodeKind::Item => fix_item(tag, version, report),
        NodeKind::Components => fix_components(tag, version, report),
        _ => Ok(()),
    }
}

// Schema read hook, not a versioned fixer. The walker invokes this before
// visiting typed legacy EntityTag/BlockEntityTag children. Only exact vanilla
// map entries are eligible; arbitrary item names and custom_data stay opaque.
pub(super) fn prepare_legacy_item(item: &mut Tag) {
    let Some(id) = get(item, "id").and_then(string) else {
        return;
    };
    let matches =
        |candidate: &str| candidate == id || candidate.strip_prefix("minecraft:") == Some(id);
    let block_id = schema::BLOCK_ENTITY_IDS
        .iter()
        .find(|(candidate, _)| matches(candidate))
        .map(|(_, id)| *id);
    let entity_id = schema::ENTITY_IDS
        .iter()
        .find(|(candidate, _)| matches(candidate))
        .map(|(_, id)| *id);
    let Some(tag) = get_mut(item, "tag") else {
        return;
    };
    if let (Some(id), Some(block)) = (block_id, get_mut(tag, "BlockEntityTag")) {
        // V99.addNames overwrites even an existing block-entity discriminator.
        if get(block, "id").and_then(string) != Some(id) {
            set(block, "id", text(id));
        }
    }
    if let (Some(id), Some(entity)) = (entity_id, get_mut(tag, "EntityTag")) {
        // In contrast, an explicit entity discriminator is authoritative.
        if get(entity, "id").is_none() {
            set(entity, "id", text(id));
        }
    }
}

fn empty() -> Tag {
    Tag::Compound(Vec::new())
}
fn sequence(elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: elements.first().map_or(0, Tag::type_id),
        elements,
    })
}
fn text(value: &str) -> Tag {
    Tag::String(value.to_owned())
}
fn nonempty(tag: &Tag) -> bool {
    fields(tag).is_some_and(|f| !f.is_empty())
}
fn value_int(tag: &Tag) -> i32 {
    match tag {
        Tag::Long(value) => *value as i32,
        _ => int(tag).unwrap_or_else(|| number(tag).unwrap_or(0.0) as i32),
    }
}
fn bool_field(tag: &Tag, key: &str, default: bool) -> bool {
    get(tag, key)
        .filter(|value| number(value).is_some())
        .map_or(default, |value| value_int(value) as i8 != 0)
}
fn move_field(src: &mut Tag, old: &str, dst: &mut Tag, new: &str) {
    if let Some(value) = take(src, old) {
        set(dst, new, value);
    }
}
fn hidden(mut component: Tag, hide: bool) -> Tag {
    if hide {
        set(&mut component, "show_in_tooltip", Tag::Byte(0));
    }
    component
}
fn named(tag: &Tag, id: &str) -> bool {
    get(tag, "id")
        .and_then(string)
        .is_some_and(|s| s == id || id.strip_prefix("minecraft:") == Some(s))
}
fn unsupported(version: i32, fix: &'static str, path: &str, reason: &str) -> UpgradeError {
    UpgradeError::unsupported(version, fix, path, reason)
}

fn fix_item(item: &mut Tag, version: i32, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    if matches!(version, 3438 | 3447 | 3692 | 3800 | 4541) {
        if let Some(id) = get_mut(item, "id")
            && let Some(old) = string(id)
            && let Some(new) = renamed_item(old, version)
        {
            *id = Tag::String(new);
            report.applied(version, "ItemRenameFix");
        }
        return Ok(());
    }
    match version {
        3094 if named(item, "minecraft:goat_horn") => {
            if let Some(tag) = get_mut(item, "tag") {
                let index = get(tag, "SoundVariant").map_or(0, value_int);
                let horn = tables::HORNS
                    .get(index as usize)
                    .unwrap_or(&tables::HORNS[0]);
                remove(tag, "SoundVariant");
                set(tag, "instrument", text(horn));
                report.applied(version, "GoatHornIdFix");
            }
        }
        3097 if named(item, "minecraft:writable_book") || named(item, "minecraft:written_book") => {
            if let Some(tag) = get_mut(item, "tag") {
                let affected =
                    get(tag, "filtered_title").is_some() || get(tag, "filtered_pages").is_some();
                remove(tag, "filtered_title");
                remove(tag, "filtered_pages");
                if affected {
                    report.applied(version, "FilteredBooksFix");
                }
            }
        }
        3209 if named(item, "minecraft:pig_spawn_egg") => {
            if let Some(entity_id) = get(item, "tag")
                .and_then(|t| get(t, "EntityTag"))
                .and_then(|t| get(t, "id"))
                .and_then(string)
            {
                let new = tables::SPAWN_EGGS
                    .iter()
                    .find(|(id, _)| *id == entity_id)
                    .map_or("minecraft:pig_spawn_egg", |(_, egg)| *egg);
                set(item, "id", text(new));
                report.applied(version, "ItemStackSpawnEggFix");
            }
        }
        3322 | 3568 => super::entities::fix_potion_effects(item, version, report)?,
        3803 => {
            if let Some(tag) = get_mut(item, "tag") {
                for key in ["Enchantments", "StoredEnchantments"] {
                    if let Some(entries) = get_mut(tag, key).and_then(list_mut) {
                        for entry in entries {
                            if get(entry, "id").and_then(string) == Some("minecraft:sweeping") {
                                set(entry, "id", text("minecraft:sweeping_edge"));
                                report.applied(version, "RenameEnchantmentsFix");
                            }
                        }
                    }
                }
            }
        }
        3813 if named(item, "minecraft:compass") => {
            if let Some(pos) = get_mut(item, "tag").and_then(|t| get_mut(t, "LodestonePos"))
                && let (Some(x), Some(y), Some(z)) = (
                    get(pos, "X").filter(|v| number(v).is_some()).map(value_int),
                    get(pos, "Y").filter(|v| number(v).is_some()).map(value_int),
                    get(pos, "Z").filter(|v| number(v).is_some()).map(value_int),
                )
            {
                *pos = Tag::IntArray(vec![x, y, z]);
                report.applied(version, "BlockPosFormatAndRenamesFix");
            }
        }
        3814 => {
            if let Some(modifiers) = get_mut(item, "tag")
                .and_then(|t| get_mut(t, "AttributeModifiers"))
                .and_then(list_mut)
            {
                for modifier in modifiers {
                    if matches!(
                        get(modifier, "AttributeName").and_then(string),
                        Some("minecraft:horse.jump_strength" | "horse.jump_strength")
                    ) {
                        set(
                            modifier,
                            "AttributeName",
                            text("minecraft:generic.jump_strength"),
                        );
                        report.applied(version, "AttributesRenameLegacy");
                    }
                }
            }
        }
        3818 => componentize(item, report)?,
        3825 => fix_override_name(item, report)?,
        4054 if named(item, "minecraft:white_banner") => {
            if let Some(components) = get_mut(item, "components")
                && translation(
                    get(components, "minecraft:item_name"),
                    version,
                    "OminousBannerRarityFix",
                )?
                .as_deref()
                    == Some("block.minecraft.ominous_banner")
            {
                set(
                    components,
                    "minecraft:item_name",
                    translatable("block.minecraft.ominous_banner"),
                );
                set(components, "minecraft:rarity", text("uncommon"));
                report.applied(version, "OminousBannerRarityFix");
            }
        }
        4299 => fix_variants(item, report),
        _ => {}
    }
    Ok(())
}

fn renamed_item(old: &str, version: i32) -> Option<String> {
    let name = old.strip_prefix("minecraft:").unwrap_or(old);
    if old.contains(':') && !old.starts_with("minecraft:") {
        return None;
    }
    let replacement = match (version, name) {
        (3438, "pottery_shard_archer") => "archer_pottery_shard",
        (3438, "pottery_shard_prize") => "prize_pottery_shard",
        (3438, "pottery_shard_arms_up") => "arms_up_pottery_shard",
        (3438, "pottery_shard_skull") => "skull_pottery_shard",
        (3692, "grass") => "short_grass",
        (3800, "scute") => "turtle_scute",
        (4541, "chain") => "iron_chain",
        (3447, _) => {
            let pattern = name.strip_suffix("_pottery_shard")?;
            if tables::SHERDS.contains(&pattern) {
                return Some(format!("minecraft:{pattern}_pottery_sherd"));
            }
            return None;
        }
        _ => return None,
    };
    Some(format!("minecraft:{replacement}"))
}

fn componentize(item: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    // Vanilla reads only stacks with a string id and numeric Count.
    let Some(id) = get(item, "id").and_then(string) else {
        return Ok(());
    };
    let id = if id.contains(':') {
        id.to_owned()
    } else {
        format!("minecraft:{id}")
    };
    let Some(count) = get(item, "Count")
        .filter(|value| number(value).is_some())
        .map(value_int)
    else {
        return Ok(());
    };
    if let Some(tag) = get(item, "tag") {
        if fields(tag).is_none() {
            return Err(unsupported(
                3818,
                COMPONENTIZE,
                "tag",
                "non-compound legacy item tag cannot be componentized losslessly",
            ));
        }
        if let Some(state) = get(tag, "BlockStateTag")
            && let Some(entries) = fields(state)
        {
            for (key, value) in entries {
                if matches!(value, Tag::Float(_) | Tag::Double(_)) {
                    return Err(unsupported(
                        3818,
                        COMPONENTIZE,
                        &format!("tag.BlockStateTag.{key}"),
                        "Java floating-point string representation is not implemented",
                    ));
                }
            }
        }
    }
    if get(item, "components").is_some() || get(item, "count").is_some() {
        return Err(unsupported(
            3818,
            COMPONENTIZE,
            "components/count",
            "legacy stack contains conflicting modern fields",
        ));
    }
    let mut legacy = take(item, "tag").unwrap_or_else(empty);
    let mut components = empty();
    let hide = take(&mut legacy, "HideFlags").as_ref().map_or(0, value_int);
    for (old, new) in [
        ("Damage", "minecraft:damage"),
        ("RepairCost", "minecraft:repair_cost"),
    ] {
        if let Some(value) = take(&mut legacy, old)
            && value != Tag::Int(0)
        {
            set(&mut components, new, value);
        }
    }
    move_field(
        &mut legacy,
        "CustomModelData",
        &mut components,
        "minecraft:custom_model_data",
    );
    if let Some(mut state) = take(&mut legacy, "BlockStateTag") {
        if let Some(entries) = fields_mut(&mut state) {
            for (key, value) in entries {
                if let Some(n) = number(value) {
                    let out = if tables::BOOLEAN_PROPERTIES.contains(&key.as_str()) {
                        (value_int(value) as i8 != 0).to_string()
                    } else {
                        match value {
                            Tag::Long(n) => n.to_string(),
                            _ => (n as i32).to_string(),
                        }
                    };
                    *value = Tag::String(out);
                }
            }
        }
        set(&mut components, "minecraft:block_state", state);
    }
    move_field(
        &mut legacy,
        "EntityTag",
        &mut components,
        "minecraft:entity_data",
    );
    if let Some(mut block) = take(&mut legacy, "BlockEntityTag") {
        componentize_block(&mut block, &mut components)?;
        let only_id = fields(&block).is_some_and(|f| f.len() == 1 && f[0].0 == "id");
        if nonempty(&block) && !only_id {
            set(&mut components, "minecraft:block_entity_data", block);
        }
    }
    if take(&mut legacy, "Unbreakable")
        .as_ref()
        .filter(|value| number(value).is_some())
        .is_some_and(|value| value_int(value) as i8 != 0)
    {
        set(
            &mut components,
            "minecraft:unbreakable",
            hidden(empty(), hide & 4 != 0),
        );
    }
    enchantments(
        &mut legacy,
        "Enchantments",
        &mut components,
        "minecraft:enchantments",
        hide & 1 != 0,
    );
    if id == "minecraft:enchanted_book" {
        enchantments(
            &mut legacy,
            "StoredEnchantments",
            &mut components,
            "minecraft:stored_enchantments",
            hide & 32 != 0,
        );
    }
    let mut display = take(&mut legacy, "display").unwrap_or_else(empty);
    if let Some(name) = take(&mut display, "Name")
        && string(&name).is_some()
    {
        set(&mut components, "minecraft:custom_name", name);
    }
    if let Some(lore) = take(&mut display, "Lore") {
        let lines = into_list(lore)
            .into_iter()
            .filter(|t| string(t).is_some())
            .collect();
        set(&mut components, "minecraft:lore", sequence(lines));
    }
    let color = take(&mut display, "color")
        .as_ref()
        .and_then(number)
        .map(|v| v as i32);
    if color.is_some() || hide & 64 != 0 {
        set(
            &mut components,
            "minecraft:dyed_color",
            hidden(
                compound(vec![("rgb", Tag::Int(color.unwrap_or(10511680)))]),
                hide & 64 != 0,
            ),
        );
    }
    if let Some(loc) = take(&mut display, "LocName")
        && let Some(key) = string(&loc)
    {
        set(&mut components, "minecraft:item_name", translatable(key));
    }
    if id == "minecraft:filled_map" {
        move_field(
            &mut display,
            "MapColor",
            &mut components,
            "minecraft:map_color",
        );
    }
    if nonempty(&display) {
        set(&mut legacy, "display", display);
    }
    for (key, component, bit) in [
        ("CanDestroy", "minecraft:can_break", 8),
        ("CanPlaceOn", "minecraft:can_place_on", 16),
    ] {
        if let Some(checks) = take(&mut legacy, key) {
            let predicates = into_list(checks)
                .into_iter()
                .map(block_predicate)
                .collect::<Result<Vec<_>, _>>()?;
            set(
                &mut components,
                component,
                hidden(
                    compound(vec![("predicates", sequence(predicates))]),
                    hide & bit != 0,
                ),
            );
        }
    }
    if let Some(modifiers) = take(&mut legacy, "AttributeModifiers") {
        let modifiers = into_list(modifiers)
            .into_iter()
            .map(attribute_modifier)
            .collect();
        set(
            &mut components,
            "minecraft:attribute_modifiers",
            hidden(
                compound(vec![("modifiers", sequence(modifiers))]),
                hide & 2 != 0,
            ),
        );
    }
    if let Some(trim) = take(&mut legacy, "Trim") {
        set(
            &mut components,
            "minecraft:trim",
            hidden(trim, hide & 128 != 0),
        );
    }
    if hide & 32 != 0 {
        set(
            &mut components,
            "minecraft:hide_additional_tooltip",
            empty(),
        );
    }
    match id.as_str() {
        "minecraft:crossbow" => {
            remove(&mut legacy, "Charged");
            if let Some(projectiles) = take(&mut legacy, "ChargedProjectiles")
                && list(&projectiles).is_none_or(|v| !v.is_empty())
            {
                set(
                    &mut components,
                    "minecraft:charged_projectiles",
                    projectiles,
                );
            }
        }
        "minecraft:bundle" => {
            if let Some(items) = take(&mut legacy, "Items")
                && list(&items).is_none_or(|v| !v.is_empty())
            {
                set(&mut components, "minecraft:bundle_contents", items);
            }
        }
        "minecraft:filled_map" => {
            move_field(&mut legacy, "map", &mut components, "minecraft:map_id");
            if let Some(decorations) = take(&mut legacy, "Decorations") {
                let mut map = empty();
                for decoration in into_list(decorations) {
                    let key = get(&decoration, "id").and_then(string).unwrap_or("");
                    if get(&map, key).is_some() {
                        continue;
                    }
                    let kind = get(&decoration, "type").map_or(0, value_int);
                    let kind = tables::MAP_DECORATIONS
                        .get(kind as usize)
                        .unwrap_or(&"player");
                    set(
                        &mut map,
                        key,
                        compound(vec![
                            ("type", text(kind)),
                            (
                                "x",
                                Tag::Double(get(&decoration, "x").and_then(number).unwrap_or(0.0)),
                            ),
                            (
                                "z",
                                Tag::Double(get(&decoration, "z").and_then(number).unwrap_or(0.0)),
                            ),
                            (
                                "rotation",
                                Tag::Float(
                                    get(&decoration, "rot").and_then(number).unwrap_or(0.0) as f32
                                ),
                            ),
                        ]),
                    );
                }
                if nonempty(&map) {
                    set(&mut components, "minecraft:map_decorations", map);
                }
            }
        }
        "minecraft:potion"
        | "minecraft:splash_potion"
        | "minecraft:lingering_potion"
        | "minecraft:tipped_arrow" => {
            let mut potion = empty();
            if let Some(value) = take(&mut legacy, "Potion")
                && string(&value).is_some_and(|v| v != "minecraft:empty")
            {
                set(&mut potion, "potion", value);
            }
            move_field(
                &mut legacy,
                "CustomPotionColor",
                &mut potion,
                "custom_color",
            );
            move_field(
                &mut legacy,
                "custom_potion_effects",
                &mut potion,
                "custom_effects",
            );
            if nonempty(&potion) {
                set(&mut components, "minecraft:potion_contents", potion);
            }
        }
        "minecraft:writable_book" | "minecraft:written_book" => {
            componentize_book(&mut legacy, &mut components, id == "minecraft:written_book")
        }
        "minecraft:suspicious_stew" => move_field(
            &mut legacy,
            "effects",
            &mut components,
            "minecraft:suspicious_stew_effects",
        ),
        "minecraft:debug_stick" => move_field(
            &mut legacy,
            "DebugProperty",
            &mut components,
            "minecraft:debug_stick_state",
        ),
        "minecraft:goat_horn" => move_field(
            &mut legacy,
            "instrument",
            &mut components,
            "minecraft:instrument",
        ),
        "minecraft:knowledge_book" => {
            move_field(&mut legacy, "Recipes", &mut components, "minecraft:recipes")
        }
        "minecraft:compass" => componentize_lodestone(&mut legacy, &mut components),
        "minecraft:firework_star" => {
            if let Some(mut explosion) = take(&mut legacy, "Explosion") {
                firework_explosion(&mut explosion);
                set(&mut components, "minecraft:firework_explosion", explosion);
            }
        }
        "minecraft:firework_rocket" => {
            let mut fireworks = take(&mut legacy, "Fireworks").unwrap_or_else(empty);
            let mut explosions = into_list(
                take(&mut fireworks, "Explosions").unwrap_or_else(|| sequence(Vec::new())),
            );
            for explosion in &mut explosions {
                firework_explosion(explosion);
            }
            let flight = take(&mut fireworks, "Flight").as_ref().map_or(0, value_int) as i8;
            set(
                &mut components,
                "minecraft:fireworks",
                compound(vec![
                    ("explosions", sequence(explosions)),
                    ("flight_duration", Tag::Byte(flight)),
                ]),
            );
            if nonempty(&fireworks) {
                set(&mut legacy, "Fireworks", fireworks);
            }
        }
        "minecraft:player_head" => {
            if let Some(owner) = take(&mut legacy, "SkullOwner") {
                set(&mut components, "minecraft:profile", profile(owner));
            }
        }
        "minecraft:pufferfish_bucket"
        | "minecraft:salmon_bucket"
        | "minecraft:cod_bucket"
        | "minecraft:tropical_fish_bucket"
        | "minecraft:axolotl_bucket"
        | "minecraft:tadpole_bucket" => {
            let mut mob = empty();
            for key in [
                "NoAI",
                "Silent",
                "NoGravity",
                "Glowing",
                "Invulnerable",
                "Health",
                "Age",
                "Variant",
                "HuntingCooldown",
                "BucketVariantTag",
            ] {
                move_field(&mut legacy, key, &mut mob, key);
            }
            if nonempty(&mob) {
                set(&mut components, "minecraft:bucket_entity_data", mob);
            }
        }
        _ => {}
    }
    if nonempty(&legacy) {
        set(&mut components, "minecraft:custom_data", legacy);
    }
    remove(item, "Count");
    set(item, "id", Tag::String(id));
    set(item, "count", Tag::Int(count));
    if nonempty(&components) {
        set(item, "components", components);
    }
    report.applied(3818, COMPONENTIZE);
    Ok(())
}

fn into_list(tag: Tag) -> Vec<Tag> {
    match tag {
        Tag::List(l) => l.elements,
        _ => Vec::new(),
    }
}

fn componentize_block(block: &mut Tag, components: &mut Tag) -> Result<(), UpgradeError> {
    if fields(block).is_none() {
        return Err(unsupported(
            3818,
            COMPONENTIZE,
            "tag.BlockEntityTag",
            "non-compound block entity payload",
        ));
    }
    move_field(block, "Lock", components, "minecraft:lock");
    if let Some(table) = take(block, "LootTable") {
        let mut loot = compound(vec![("loot_table", table)]);
        if let Some(seed) = take(block, "LootTableSeed") {
            let n = match seed {
                Tag::Long(n) => n,
                _ => number(&seed).unwrap_or(0.0) as i64,
            };
            if n != 0 {
                set(&mut loot, "seed", Tag::Long(n));
            }
        }
        set(components, "minecraft:container_loot", loot);
    }
    let id = get(block, "id").and_then(string).unwrap_or("");
    let id = id.strip_prefix("minecraft:").unwrap_or(id);
    match id {
        "skull" => move_field(
            block,
            "note_block_sound",
            components,
            "minecraft:note_block_sound",
        ),
        "decorated_pot" => {
            move_field(block, "sherds", components, "minecraft:pot_decorations");
            if let Some(item) = take(block, "item") {
                set(
                    components,
                    "minecraft:container",
                    sequence(vec![compound(vec![("slot", Tag::Int(0)), ("item", item)])]),
                );
            }
        }
        "banner" => {
            move_field(block, "patterns", components, "minecraft:banner_patterns");
            if let Some(base) = take(block, "Base")
                && let Some(n) = number(&base)
            {
                set(components, "minecraft:base_color", text(dye(n as i32)));
            }
        }
        "beehive" => move_field(block, "bees", components, "minecraft:bees"),
        "shulker_box" | "chest" | "trapped_chest" | "furnace" | "ender_chest" | "dispenser"
        | "dropper" | "brewing_stand" | "hopper" | "barrel" | "smoker" | "blast_furnace"
        | "campfire" | "chiseled_bookshelf" | "crafter" => {
            if let Some(items) = take(block, "Items") {
                let entries = into_list(items)
                    .into_iter()
                    .map(|mut item| {
                        let slot = take(&mut item, "Slot").as_ref().map_or(0, value_int) & 255;
                        compound(vec![("slot", Tag::Int(slot)), ("item", item)])
                    })
                    .collect::<Vec<_>>();
                if !entries.is_empty() {
                    set(components, "minecraft:container", sequence(entries));
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn enchantments(legacy: &mut Tag, old: &str, components: &mut Tag, new: &str, hide: bool) {
    let source = take(legacy, old);
    let present = source.is_some();
    let mut levels = empty();
    for enchantment in source.map(into_list).unwrap_or_default() {
        if let (Some(id), Some(level)) = (
            get(&enchantment, "id").and_then(string),
            get(&enchantment, "lvl").and_then(number),
        ) {
            let level = (level as i32).clamp(0, 255);
            if level > 0 {
                set(&mut levels, id, Tag::Int(level));
            }
        }
    }
    let has_levels = nonempty(&levels);
    if has_levels || hide {
        set(
            components,
            new,
            hidden(compound(vec![("levels", levels)]), hide),
        );
    }
    if present && !has_levels {
        set(
            components,
            "minecraft:enchantment_glint_override",
            Tag::Byte(1),
        );
    }
}

fn block_predicate(value: Tag) -> Result<Tag, UpgradeError> {
    let Some(value) = string(&value) else {
        return Ok(value);
    };
    let state_start = value.find('[');
    let nbt_start = value.find('{');
    let end = state_start
        .into_iter()
        .chain(nbt_start)
        .min()
        .unwrap_or(value.len());
    let mut predicate = compound(vec![("blocks", text(value[..end].trim()))]);
    if let (Some(start), Some(end)) = (state_start, value.find(']')) {
        if start >= end {
            return Err(unsupported(
                3818,
                COMPONENTIZE,
                "tag.CanDestroy/CanPlaceOn",
                "reversed state delimiter",
            ));
        }
        let mut state = empty();
        for property in value[start + 1..end].split(',') {
            if let Some((key, value)) = property.split_once('=') {
                set(&mut state, key.trim(), text(value.trim()));
            }
        }
        set(&mut predicate, "state", state);
    }
    if let (Some(start), Some(end)) = (nbt_start, value.find('}')) {
        if start > end {
            return Err(unsupported(
                3818,
                COMPONENTIZE,
                "tag.CanDestroy/CanPlaceOn",
                "reversed NBT delimiter",
            ));
        }
        set(&mut predicate, "nbt", text(&value[start..=end]));
    }
    Ok(predicate)
}

fn attribute_modifier(mut old: Tag) -> Tag {
    let mut new = compound(vec![
        ("name", text("")),
        ("amount", Tag::Double(0.0)),
        ("operation", text("add_value")),
    ]);
    for (old_key, new_key) in [
        ("AttributeName", "type"),
        ("Slot", "slot"),
        ("UUID", "uuid"),
        ("Name", "name"),
        ("Amount", "amount"),
    ] {
        move_field(&mut old, old_key, &mut new, new_key);
    }
    if let Some(operation) = take(&mut old, "Operation") {
        let operation = match value_int(&operation) {
            0 => "add_value",
            1 => "add_multiplied_base",
            2 => "add_multiplied_total",
            _ => "invalid",
        };
        set(&mut new, "operation", text(operation));
    }
    new
}

fn componentize_book(legacy: &mut Tag, components: &mut Tag, written: bool) {
    let pages = take(legacy, "pages").map(into_list).unwrap_or_default();
    let mut filtered = take(legacy, "filtered_pages").unwrap_or_else(empty);
    let pages = pages
        .into_iter()
        .enumerate()
        .map(|(index, page)| {
            let mut entry = compound(vec![("raw", text(string(&page).unwrap_or("")))]);
            if let Some(value) = take(&mut filtered, &index.to_string()) {
                set(&mut entry, "filtered", text(string(&value).unwrap_or("")));
            }
            entry
        })
        .collect::<Vec<_>>();
    let mut book = empty();
    if !pages.is_empty() {
        set(&mut book, "pages", sequence(pages));
    }
    if written {
        let title = take(legacy, "title").unwrap_or_else(|| text(""));
        let mut title = compound(vec![("raw", text(string(&title).unwrap_or("")))]);
        if let Some(value) = take(legacy, "filtered_title")
            && string(&value).is_some()
        {
            set(&mut title, "filtered", value);
        }
        set(&mut book, "title", title);
        for key in ["author", "resolved", "generation"] {
            move_field(legacy, key, &mut book, key);
        }
        set(components, "minecraft:written_book_content", book);
    } else if nonempty(&book) {
        set(components, "minecraft:writable_book_content", book);
    }
}

fn componentize_lodestone(legacy: &mut Tag, components: &mut Tag) {
    let pos = take(legacy, "LodestonePos");
    let dimension = take(legacy, "LodestoneDimension");
    if pos.is_none() && dimension.is_none() {
        return;
    }
    let tracked = take(legacy, "LodestoneTracked")
        .as_ref()
        .and_then(number)
        .is_none_or(|n| n != 0.0);
    let mut tracker = empty();
    if let (Some(pos), Some(dimension)) = (pos, dimension) {
        set(
            &mut tracker,
            "target",
            compound(vec![("pos", pos), ("dimension", dimension)]),
        );
    }
    if !tracked {
        set(&mut tracker, "tracked", Tag::Byte(0));
    }
    set(components, "minecraft:lodestone_tracker", tracker);
}

fn firework_explosion(explosion: &mut Tag) {
    let shape = match take(explosion, "Type").as_ref().map_or(0, value_int) {
        1 => "large_ball",
        2 => "star",
        3 => "creeper",
        4 => "burst",
        _ => "small_ball",
    };
    set(explosion, "shape", text(shape));
    for (old, new) in [
        ("Colors", "colors"),
        ("FadeColors", "fade_colors"),
        ("Trail", "has_trail"),
        ("Flicker", "has_twinkle"),
    ] {
        rename(explosion, old, new);
    }
}

pub(super) fn profile(mut owner: Tag) -> Tag {
    let valid = |s: &str| s.len() <= 16 && s.bytes().all(|b| b > 32 && b < 127);
    if let Some(name) = string(&owner) {
        return if valid(name) {
            compound(vec![("name", text(name))])
        } else {
            empty()
        };
    }
    let mut new = empty();
    let name = take(&mut owner, "Name").unwrap_or_else(|| text(""));
    let name = string(&name).unwrap_or("");
    if valid(name) {
        set(&mut new, "name", text(name));
    }
    move_field(&mut owner, "Id", &mut new, "id");
    if let Some(properties) = take(&mut owner, "Properties")
        && let Tag::Compound(entries) = properties
        && !entries.is_empty()
    {
        let mut properties = Vec::new();
        for (name, entries) in entries {
            for mut property in into_list(entries) {
                let value = take(&mut property, "Value").unwrap_or_else(|| text(""));
                let mut out = compound(vec![
                    ("name", text(&name)),
                    ("value", text(string(&value).unwrap_or(""))),
                ]);
                if let Some(signature) = take(&mut property, "Signature")
                    && string(&signature).is_some()
                {
                    set(&mut out, "signature", signature);
                }
                properties.push(out);
            }
        }
        set(&mut new, "properties", sequence(properties));
    }
    new
}

fn translatable(key: &str) -> Tag {
    Tag::String(serde_json::json!({"translate": key}).to_string())
}
fn translation(
    value: Option<&Tag>,
    version: i32,
    fix: &'static str,
) -> Result<Option<String>, UpgradeError> {
    let Some(value) = value.and_then(string) else {
        return Ok(None);
    };
    let parsed: serde_json::Value = serde_json::from_str(value).map_err(|_| {
        unsupported(
            version,
            fix,
            "components.minecraft:custom_name/item_name",
            "lenient nonstandard JSON translation detection is not implemented",
        )
    })?;
    Ok(parsed.get("translate").and_then(|v| match v {
        serde_json::Value::String(v) => Some(v.clone()),
        serde_json::Value::Number(v) => Some(v.to_string()),
        serde_json::Value::Bool(v) => Some(v.to_string()),
        _ => None,
    }))
}
fn fix_override_name(item: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    let banner = named(item, "minecraft:white_banner");
    let map = named(item, "minecraft:filled_map");
    if !banner && !map {
        return Ok(());
    }
    if let Some(components) = get_mut(item, "components")
        && let Some(key) = translation(
            get(components, "minecraft:custom_name"),
            3825,
            "ItemStackCustomNameToOverrideComponentFix",
        )?
        && ((banner && key == "block.minecraft.ominous_banner")
            || (map && tables::MAP_NAMES.contains(&key.as_str())))
    {
        rename(components, "minecraft:custom_name", "minecraft:item_name");
        report.applied(3825, "ItemStackCustomNameToOverrideComponentFix");
    }
    Ok(())
}

fn fix_components(
    components: &mut Tag,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    match version {
        3820 => {
            if let Some(mut tracker) = take(components, "minecraft:lodestone_target") {
                let pos = take(&mut tracker, "pos");
                let dimension = take(&mut tracker, "dimension");
                if let (Some(pos), Some(dimension)) = (pos, dimension) {
                    set(
                        &mut tracker,
                        "target",
                        compound(vec![("pos", pos), ("dimension", dimension)]),
                    );
                }
                set(components, "minecraft:lodestone_tracker", tracker);
                report.applied(version, "LodestoneCompassComponentFix");
            }
        }
        3945 => {
            if let Some(modifiers) = get_mut(components, "minecraft:attribute_modifiers")
                .and_then(|t| get_mut(t, "modifiers"))
            {
                fix_modifiers(modifiers)?;
                report.applied(version, "AttributeModifierIdFix");
            }
        }
        4055 => {
            if let Some(modifiers) = get_mut(components, "minecraft:attribute_modifiers")
                .and_then(|t| get_mut(t, "modifiers"))
                .and_then(list_mut)
            {
                for modifier in modifiers {
                    if let Some(value) = get_mut(modifier, "type")
                        && let Some(old) = string(value)
                    {
                        let normalized = old.strip_prefix("minecraft:").unwrap_or(old);
                        if !old.contains(':') || old.starts_with("minecraft:") {
                            for prefix in ["generic.", "horse.", "player.", "zombie."] {
                                if let Some(rest) = normalized.strip_prefix(prefix) {
                                    *value = Tag::String(format!("minecraft:{rest}"));
                                    report.applied(version, "AttributeIdPrefixFix");
                                    break;
                                }
                            }
                        }
                    }
                }
            }
        }
        4059 => {
            if let Some(mut food) = take(components, "minecraft:food") {
                let seconds = take(&mut food, "eat_seconds")
                    .as_ref()
                    .and_then(number)
                    .unwrap_or(1.6) as f32;
                let effects = take(&mut food, "effects")
                    .map(into_list)
                    .unwrap_or_default()
                    .into_iter()
                    .map(|mut entry| {
                        let effect = take(&mut entry, "effect").into_iter().collect();
                        let probability =
                            get(&entry, "probability").and_then(number).unwrap_or(1.0) as f32;
                        compound(vec![
                            ("type", text("minecraft:apply_effects")),
                            ("effects", sequence(effect)),
                            ("probability", Tag::Float(probability)),
                        ])
                    })
                    .collect();
                move_field(
                    &mut food,
                    "using_converts_to",
                    components,
                    "minecraft:use_remainder",
                );
                set(components, "minecraft:food", food);
                set(
                    components,
                    "minecraft:consumable",
                    compound(vec![
                        ("consume_seconds", Tag::Float(seconds)),
                        ("on_consume_effects", sequence(effects)),
                    ]),
                );
                report.applied(version, "FoodToConsumableFix");
            }
        }
        4064 => {
            if take(components, "minecraft:fire_resistant").is_some() {
                set(
                    components,
                    "minecraft:damage_resistant",
                    compound(vec![("types", text("#minecraft:is_fire"))]),
                );
                report.applied(version, "FireResistantToDamageResistantComponentFix");
            }
        }
        4068 => {
            if let Some(lock) = take(components, "minecraft:lock") {
                if let Some(lock) = string(&lock).filter(|s| !s.is_empty()) {
                    let escaped = lock.replace('\\', "\\\\").replace('"', "\\\"");
                    set(
                        components,
                        "minecraft:lock",
                        compound(vec![(
                            "components",
                            compound(vec![(
                                "minecraft:custom_name",
                                Tag::String(format!("\"{escaped}\"")),
                            )]),
                        )]),
                    );
                }
                report.applied(version, "LockComponentPredicateFix");
            }
        }
        4175 => {
            if let Some(equippable) = get_mut(components, "minecraft:equippable")
                && get(equippable, "model").is_some()
            {
                rename(equippable, "model", "asset_id");
                report.applied(version, "EquippableAssetRenameFix");
            }
            if let Some(value) = take(components, "minecraft:custom_model_data") {
                let value = number(&value).unwrap_or(0.0) as f32;
                set(
                    components,
                    "minecraft:custom_model_data",
                    compound(vec![("floats", sequence(vec![Tag::Float(value)]))]),
                );
                report.applied(version, "CustomModelDataExpandFix");
            }
        }
        4176 => {
            let invalid = get(components, "minecraft:lock")
                .filter(|t| fields(t).is_some_and(|f| f.len() == 1))
                .and_then(|t| get(t, "components"))
                .filter(|t| fields(t).is_some_and(|f| f.len() == 1))
                .and_then(|t| get(t, "minecraft:custom_name"))
                .and_then(string)
                == Some("\"\"");
            if invalid {
                remove(components, "minecraft:lock");
                report.applied(version, "InvalidLockComponentFix");
            }
        }
        4307 => tooltip(components, report),
        4649 => {
            if let Some(consumable) = get_mut(components, "minecraft:consumable")
                && get(consumable, "animation").and_then(string) == Some("spear")
            {
                set(consumable, "animation", text("trident"));
                report.applied(version, "TridentAnimationFix");
            }
        }
        _ => {}
    }
    Ok(())
}

pub(super) fn fix_modifiers(modifiers: &mut Tag) -> Result<(), UpgradeError> {
    let Some(entries) = list_mut(modifiers) else {
        return Ok(());
    };
    let mut output: Vec<Tag> = Vec::with_capacity(entries.len());
    for mut entry in std::mem::take(entries) {
        let uuid = get(&entry, "uuid").and_then(uuid_words);
        let name = get(&entry, "name").and_then(string).unwrap_or("");
        let mapped_uuid = uuid.and_then(|u| {
            tables::MODIFIER_UUIDS
                .iter()
                .find(|(old, _)| *old == u)
                .map(|(_, id)| *id)
        });
        let mapped_name = tables::MODIFIER_NAMES
            .iter()
            .find(|(old, _)| *old == name)
            .map(|(_, id)| *id);
        let id = mapped_uuid
            .or(mapped_name)
            .map(std::borrow::Cow::Borrowed)
            .unwrap_or_else(|| {
                uuid.map_or_else(
                    || std::borrow::Cow::Borrowed("minecraft:unknown"),
                    |words| {
                        std::borrow::Cow::Owned(format!(
                            "minecraft:{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
                            words[0] as u32,
                            (words[1] as u32) >> 16,
                            (words[1] as u32) & 65535,
                            (words[2] as u32) >> 16,
                            (((words[2] as u32) & 65535) as u64) << 32 | words[3] as u32 as u64,
                        ))
                    },
                )
            });
        let existing = output
            .iter()
            .position(|t| get(t, "id").and_then(string) == Some(id.as_ref()));
        if mapped_uuid.is_none()
            && mapped_name.is_some()
            && let Some(index) = existing
        {
            let total = get(&output[index], "amount")
                .and_then(number)
                .unwrap_or(0.0)
                + get(&entry, "amount").and_then(number).unwrap_or(0.0);
            set(&mut output[index], "amount", Tag::Double(total));
            continue;
        }
        remove(&mut entry, "uuid");
        remove(&mut entry, "name");
        set(&mut entry, "id", Tag::String(id.into_owned()));
        if let Some(index) = existing {
            output[index] = entry;
        } else {
            output.push(entry);
        }
    }
    *modifiers = sequence(output);
    Ok(())
}
fn uuid_words(tag: &Tag) -> Option<[i32; 4]> {
    match tag {
        Tag::IntArray(words) if words.len() == 4 => Some([words[0], words[1], words[2], words[3]]),
        Tag::List(words)
            if words.elements.len() == 4
                && words.elements.iter().all(|value| number(value).is_some()) =>
        {
            Some([
                value_int(&words.elements[0]),
                value_int(&words.elements[1]),
                value_int(&words.elements[2]),
                value_int(&words.elements[3]),
            ])
        }
        _ => None,
    }
}
fn dye(value: i32) -> &'static str {
    tables::DYES.get(value as usize).copied().unwrap_or("white")
}

fn fix_variants(item: &mut Tag, report: &mut UpgradeReport) {
    let id = get(item, "id").and_then(string).unwrap_or("");
    let field = match id {
        "minecraft:salmon_bucket" => "type",
        "minecraft:axolotl_bucket" => "Variant",
        "minecraft:tropical_fish_bucket" => "BucketVariantTag",
        "minecraft:painting" => {
            if let Some(components) = get_mut(item, "components")
                && let Some(mut entity) = take(components, "minecraft:entity_data")
            {
                if get(&entity, "id").and_then(string) == Some("minecraft:painting") {
                    move_field(
                        &mut entity,
                        "variant",
                        components,
                        "minecraft:painting/variant",
                    );
                    if fields(&entity).is_some_and(|f| f.len() == 1 && f[0].0 == "id") {
                        entity = empty();
                    }
                    report.applied(4299, "EntitySpawnerItemVariantComponentFix");
                }
                if nonempty(&entity) {
                    set(components, "minecraft:entity_data", entity);
                }
            }
            return;
        }
        _ => return,
    };
    let Some(components) = get_mut(item, "components") else {
        return;
    };
    let Some(mut bucket) = take(components, "minecraft:bucket_entity_data") else {
        return;
    };
    if let Some(value) = get(&bucket, field) {
        if field == "type" {
            let value = take(&mut bucket, field).expect("present bucket field");
            set(components, "minecraft:salmon/size", value);
            report.applied(4299, "EntitySpawnerItemVariantComponentFix");
        } else if let Some(n) = number(value) {
            let n = n as i32;
            remove(&mut bucket, field);
            if field == "Variant" {
                let variant = match n {
                    1 => "wild",
                    2 => "gold",
                    3 => "cyan",
                    4 => "blue",
                    _ => "lucy",
                };
                set(components, "minecraft:axolotl/variant", text(variant));
            } else {
                let pattern = match n & 65535 {
                    256 => "sunstreak",
                    512 => "snooper",
                    768 => "dasher",
                    1024 => "brinely",
                    1280 => "spotty",
                    1 => "flopper",
                    257 => "stripey",
                    513 => "glitter",
                    769 => "blockfish",
                    1025 => "betty",
                    1281 => "clayfish",
                    _ => "kob",
                };
                set(components, "minecraft:tropical_fish/pattern", text(pattern));
                set(
                    components,
                    "minecraft:tropical_fish/base_color",
                    text(dye((n >> 16) & 255)),
                );
                set(
                    components,
                    "minecraft:tropical_fish/pattern_color",
                    text(dye((n >> 24) & 255)),
                );
            }
            report.applied(4299, "EntitySpawnerItemVariantComponentFix");
        }
    }
    set(components, "minecraft:bucket_entity_data", bucket);
}

fn tooltip(components: &mut Tag, report: &mut UpgradeReport) {
    let mut hidden_components = Vec::<Tag>::new();
    let mut affected = false;
    for (component, unwrap) in [
        ("minecraft:can_place_on", "predicates"),
        ("minecraft:can_break", "predicates"),
        ("minecraft:trim", ""),
        ("minecraft:unbreakable", ""),
        ("minecraft:dyed_color", "rgb"),
        ("minecraft:attribute_modifiers", "modifiers"),
        ("minecraft:enchantments", "levels"),
        ("minecraft:stored_enchantments", "levels"),
        ("minecraft:jukebox_playable", "song"),
    ] {
        if let Some(mut value) = take(components, component) {
            let adventure = matches!(component, "minecraft:can_place_on" | "minecraft:can_break");
            if adventure && get(&value, unwrap).is_none() {
                set(components, component, value);
                continue;
            }
            if !bool_field(&value, "show_in_tooltip", true) {
                hidden_components.push(text(component));
            }
            remove(&mut value, "show_in_tooltip");
            if !unwrap.is_empty()
                && let Some(inner) = take(&mut value, unwrap)
            {
                value = inner;
            }
            set(components, component, value);
            affected = true;
        }
    }
    let hide = take(components, "minecraft:hide_tooltip").is_some();
    let additional = take(components, "minecraft:hide_additional_tooltip").is_some();
    if additional {
        for component in tables::ADDITIONAL_TOOLTIP {
            if get(components, component).is_some()
                && !hidden_components
                    .iter()
                    .any(|t| string(t) == Some(*component))
            {
                hidden_components.push(text(component));
            }
        }
    }
    if hide || !hidden_components.is_empty() {
        set(
            components,
            "minecraft:tooltip_display",
            compound(vec![
                ("hide_tooltip", Tag::Byte(i8::from(hide))),
                ("hidden_components", sequence(hidden_components)),
            ]),
        );
    }
    if affected || hide || additional {
        report.applied(4307, "TooltipDisplayComponentFix");
    }
}

#[cfg(test)]
mod tests;
