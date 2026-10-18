use super::{NodeKind, UpgradeError, UpgradeReport, nbt};
use mc_nbt::Tag;

pub(super) fn fix_node(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    match (kind, version) {
        (NodeKind::Chunk, 2970) => configured_structures(tag, report)?,
        (NodeKind::Chunk, 3077) => {
            if nbt::get(tag, "isLightOn").and_then(nbt::int).unwrap_or(0) == 0 {
                delete_lights(tag);
                report.applied(version, "ChunkDeleteIgnoredLightDataFix");
            }
        }
        (NodeKind::Chunk, 3108) => {
            if nbt::get(tag, "blending_data").is_some()
                && dimension(tag, version, "BlendingDataRemoveFromNetherEndFix")?
                    != "minecraft:overworld"
            {
                nbt::remove(tag, "blending_data");
                report.applied(version, "BlendingDataRemoveFromNetherEndFix");
            }
        }
        (NodeKind::Chunk, 3450) => {
            remap_status(tag, "Status");
            if let Some(retrogen) = nbt::get_mut(tag, "below_zero_retrogen") {
                remap_status(retrogen, "target_status");
            }
            report.applied(version, "RemapChunkStatusFix");
        }
        (NodeKind::Chunk, 3451 | 4537) => {
            delete_lights(tag);
            nbt::remove(tag, "isLightOn");
            report.applied(version, "ChunkDeleteLightFix");
        }
        (NodeKind::Chunk, 4057) => {
            if let Some(mut masks) = nbt::take(tag, "CarvingMasks") {
                if let Some(air) = nbt::take(&mut masks, "AIR") {
                    nbt::set(tag, "carving_mask", air);
                }
                report.applied(version, "CarvingStepRemoveFix");
            }
        }
        (NodeKind::Chunk, 4295) => blending(tag, report)?,
        (NodeKind::BlockState, 4294) => {
            if nbt::get(tag, "Name").and_then(nbt::string) == Some("minecraft:creaking_heart")
                && let Some(properties) = nbt::get_mut(tag, "Properties")
                && let Some(active) = nbt::take(properties, "active")
            {
                let value = if nbt::string(&active) == Some("true") {
                    "awake"
                } else {
                    "uprooted"
                };
                nbt::set(
                    properties,
                    "creaking_heart_state",
                    Tag::String(value.into()),
                );
                report.applied(version, "BlockPropertyRenameAndFix");
            }
        }
        (NodeKind::BlockState, 4305) => {
            if nbt::get(tag, "Name").and_then(nbt::string) == Some("minecraft:test_block")
                && let Some(properties) = nbt::get_mut(tag, "Properties")
                && let Some(mode) = nbt::take(properties, "test_block_mode")
            {
                let value = nbt::string(&mode).unwrap_or("").to_owned();
                nbt::set(properties, "mode", Tag::String(value));
                report.applied(version, "BlockPropertyRenameAndFix");
            }
        }
        (NodeKind::Entity | NodeKind::BlockEntity, 3084) => game_event(tag, report),
        (NodeKind::Player | NodeKind::BlockEntity | NodeKind::Item, 4769) => {
            recipes(tag, kind, report)
        }
        _ => {}
    }
    if version == 3692 || version == 4541 {
        block_names(tag, kind, version, report);
    }
    Ok(())
}

fn delete_lights(tag: &mut Tag) {
    if let Some(sections) = nbt::get_mut(tag, "sections").and_then(nbt::list_mut) {
        for section in sections {
            nbt::remove(section, "BlockLight");
            nbt::remove(section, "SkyLight");
        }
    }
}
fn remap_status(tag: &mut Tag, key: &str) {
    if let Some(Tag::String(status)) = nbt::get_mut(tag, key) {
        let bare = status.strip_prefix("minecraft:").unwrap_or(status);
        let mapped = match bare {
            "liquid_carvers" => "carvers",
            "heightmaps" => "spawn",
            _ => bare,
        };
        if !status.contains(':') || mapped != bare {
            *status = format!("minecraft:{mapped}");
        }
    }
}
fn dimension<'a>(tag: &'a Tag, version: i32, fix: &'static str) -> Result<&'a str, UpgradeError> {
    nbt::get(tag, "__context")
        .and_then(|context| nbt::get(context, "dimension"))
        .and_then(nbt::string)
        .ok_or_else(|| {
            UpgradeError::unsupported(
                version,
                fix,
                "__context.dimension",
                "supply the source dimension identifier; do not guess blending semantics",
            )
        })
}
fn skips_blending(status: &str) -> bool {
    matches!(
        status.strip_prefix("minecraft:").unwrap_or(status),
        "empty" | "structure_starts" | "structure_references" | "biomes"
    )
}
fn blending(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    let status = nbt::get(tag, "Status").and_then(nbt::string);
    if status.is_none() && nbt::get(tag, "blending_data").is_none() {
        return Ok(());
    }
    let overworld = dimension(tag, 4295, "BlendingDataFix")? == "minecraft:overworld";
    let range = if !overworld {
        None
    } else if let Some(status) = status {
        if !skips_blending(status) {
            Some((-4, 20))
        } else if nbt::get(tag, "below_zero_retrogen")
            .and_then(|retrogen| nbt::get(retrogen, "target_status"))
            .and_then(nbt::string)
            .is_some_and(|target| !skips_blending(target))
        {
            Some((0, 16))
        } else {
            None
        }
    } else {
        None
    };
    nbt::remove(tag, "blending_data");
    if let Some((min, max)) = range {
        nbt::set(
            tag,
            "blending_data",
            nbt::compound(vec![
                ("min_section", Tag::Int(min)),
                ("max_section", Tag::Int(max)),
            ]),
        );
    }
    report.applied(4295, "BlendingDataFix");
    Ok(())
}

// Mojang chooses configurations by counting biome *palette entries*, not by
// packed voxel frequency. Ties retain the first encountered configuration.
fn configuration(tag: &Tag, old: &str) -> Result<&'static str, UpgradeError> {
    let fallback = match old {
        "mineshaft" => "minecraft:mineshaft",
        "shipwreck" => "minecraft:shipwreck",
        "ocean_ruin" => "minecraft:ocean_ruin_cold",
        "village" => "minecraft:village_plains",
        "ruined_portal" => "minecraft:ruined_portal",
        "pillager_outpost" => "minecraft:pillager_outpost",
        "mansion" => "minecraft:mansion",
        "jungle_pyramid" => "minecraft:jungle_pyramid",
        "desert_pyramid" => "minecraft:desert_pyramid",
        "igloo" => "minecraft:igloo",
        "swamp_hut" => "minecraft:swamp_hut",
        "stronghold" => "minecraft:stronghold",
        "monument" => "minecraft:monument",
        "fortress" => "minecraft:fortress",
        "endcity" => "minecraft:end_city",
        "buried_treasure" => "minecraft:buried_treasure",
        "nether_fossil" => "minecraft:nether_fossil",
        "bastion_remnant" => "minecraft:bastion_remnant",
        _ => {
            return Err(UpgradeError::unsupported(
                2970,
                "StructuresBecomeConfiguredFix",
                format!("structures.{old}"),
                "unknown structure configuration; vanilla would discard it",
            ));
        }
    };
    let mut counts = [("", 0usize); 4];
    let mut used = 0;
    if let Some(sections) = nbt::get(tag, "sections").and_then(nbt::list) {
        for section in sections {
            if let Some(palette) = nbt::get(section, "biomes")
                .and_then(|biomes| nbt::get(biomes, "palette"))
                .and_then(nbt::list)
            {
                for biome in palette.iter().filter_map(nbt::string) {
                    if let Some(config) = biome_configuration(old, biome) {
                        if let Some((_, count)) =
                            counts[..used].iter_mut().find(|(key, _)| *key == config)
                        {
                            *count += 1;
                        } else {
                            counts[used] = (config, 1);
                            used += 1;
                        }
                    }
                }
            }
        }
    }
    let mut best = (fallback, 0);
    for &(config, count) in &counts[..used] {
        if count > best.1 {
            best = (config, count);
        }
    }
    Ok(best.0)
}
fn biome_configuration(old: &str, biome: &str) -> Option<&'static str> {
    match (old, biome) {
        (
            "mineshaft",
            "minecraft:badlands" | "minecraft:eroded_badlands" | "minecraft:wooded_badlands",
        ) => Some("minecraft:mineshaft_mesa"),
        ("shipwreck", "minecraft:beach" | "minecraft:snowy_beach") => {
            Some("minecraft:shipwreck_beached")
        }
        (
            "ocean_ruin",
            "minecraft:warm_ocean" | "minecraft:lukewarm_ocean" | "minecraft:deep_lukewarm_ocean",
        ) => Some("minecraft:ocean_ruin_warm"),
        ("village", "minecraft:desert") => Some("minecraft:village_desert"),
        ("village", "minecraft:savanna") => Some("minecraft:village_savanna"),
        ("village", "minecraft:snowy_plains") => Some("minecraft:village_snowy"),
        ("village", "minecraft:taiga") => Some("minecraft:village_taiga"),
        ("ruined_portal", "minecraft:desert") => Some("minecraft:ruined_portal_desert"),
        (
            "ruined_portal",
            "minecraft:badlands"
            | "minecraft:eroded_badlands"
            | "minecraft:wooded_badlands"
            | "minecraft:windswept_hills"
            | "minecraft:windswept_forest"
            | "minecraft:windswept_gravelly_hills"
            | "minecraft:savanna_plateau"
            | "minecraft:windswept_savanna"
            | "minecraft:stony_shore"
            | "minecraft:meadow"
            | "minecraft:frozen_peaks"
            | "minecraft:jagged_peaks"
            | "minecraft:stony_peaks"
            | "minecraft:snowy_slopes",
        ) => Some("minecraft:ruined_portal_mountain"),
        (
            "ruined_portal",
            "minecraft:bamboo_jungle" | "minecraft:jungle" | "minecraft:sparse_jungle",
        ) => Some("minecraft:ruined_portal_jungle"),
        (
            "ruined_portal",
            "minecraft:deep_frozen_ocean"
            | "minecraft:deep_cold_ocean"
            | "minecraft:deep_ocean"
            | "minecraft:deep_lukewarm_ocean"
            | "minecraft:frozen_ocean"
            | "minecraft:ocean"
            | "minecraft:cold_ocean"
            | "minecraft:lukewarm_ocean"
            | "minecraft:warm_ocean",
        ) => Some("minecraft:ruined_portal_ocean"),
        _ => None,
    }
}
fn configured_structures(tag: &mut Tag, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    let Some(mut structures) = nbt::take(tag, "structures") else {
        return Ok(());
    };
    for key in ["starts", "References"] {
        if let Some(old) = nbt::take(&mut structures, key) {
            let Tag::Compound(entries) = old else {
                return Err(UpgradeError::malformed(
                    format!("structures.{key}"),
                    "expected compound",
                ));
            };
            let mut updated = Tag::Compound(Vec::with_capacity(entries.len()));
            for (name, mut value) in entries {
                if key == "starts"
                    && nbt::get(&value, "id")
                        .and_then(nbt::string)
                        .unwrap_or("INVALID")
                        == "INVALID"
                {
                    continue;
                }
                if key == "References"
                    && matches!(&value, Tag::LongArray(values) if values.is_empty())
                {
                    continue;
                }
                let configured = configuration(tag, &name.to_lowercase())?;
                if key == "starts" {
                    if nbt::get(&updated, configured).is_none() {
                        nbt::set(&mut value, "id", Tag::String(configured.into()));
                        nbt::set(&mut updated, configured, value);
                    }
                } else if let Some(existing) = nbt::get_mut(&mut updated, configured) {
                    let (Tag::LongArray(existing), Tag::LongArray(values)) = (existing, value)
                    else {
                        return Err(UpgradeError::malformed(
                            "structures.References",
                            "expected long arrays",
                        ));
                    };
                    existing.extend(values);
                } else {
                    nbt::set(&mut updated, configured, value);
                }
            }
            nbt::set(&mut structures, key, updated);
        }
    }
    nbt::set(tag, "structures", structures);
    report.applied(2970, "StructuresBecomeConfiguredFix");
    Ok(())
}
fn block_names(tag: &mut Tag, kind: NodeKind, version: i32, report: &mut UpgradeReport) {
    let (old, new) = if version == 3692 {
        ("minecraft:grass", "minecraft:short_grass")
    } else {
        ("minecraft:chain", "minecraft:iron_chain")
    };
    let mut changed = false;
    match kind {
        NodeKind::BlockState => changed |= rename_name(tag, "Name", old, new),
        NodeKind::Chunk => {
            for key in ["block_ticks", "fluid_ticks"] {
                if let Some(ticks) = nbt::get_mut(tag, key).and_then(nbt::list_mut) {
                    for tick in ticks {
                        changed |= rename_name(tick, "i", old, new);
                    }
                }
            }
        }
        NodeKind::BlockEntity => {
            if nbt::get(tag, "id").and_then(nbt::string) == Some("minecraft:jigsaw")
                && let Some(Tag::String(state)) = nbt::get_mut(tag, "final_state")
            {
                let end = state.find(['[', '{']).unwrap_or(state.len());
                if namespaced_eq(&state[..end], old) {
                    state.replace_range(..end, new);
                    changed = true;
                }
            }
        }
        NodeKind::Item => {
            if let Some(legacy) = nbt::get_mut(tag, "tag") {
                for key in ["CanDestroy", "CanPlaceOn"] {
                    if let Some(values) = nbt::get_mut(legacy, key).and_then(nbt::list_mut) {
                        for value in values {
                            changed |= rename_string(value, old, new);
                        }
                    }
                }
            }
        }
        NodeKind::Components => {
            for key in ["minecraft:can_break", "minecraft:can_place_on"] {
                if let Some(predicates) = nbt::get_mut(tag, key)
                    .and_then(|component| nbt::get_mut(component, "predicates"))
                    .and_then(nbt::list_mut)
                {
                    for predicate in predicates {
                        if let Some(blocks) = nbt::get_mut(predicate, "blocks") {
                            if let Some(values) = nbt::list_mut(blocks) {
                                for value in values {
                                    changed |= rename_string(value, old, new);
                                }
                            } else {
                                changed |= rename_string(blocks, old, new);
                            }
                        }
                    }
                }
            }
        }
        NodeKind::Level => {
            for key in ["WorldGenSettings", "world_gen_settings"] {
                if let Some(dimensions) = nbt::get_mut(tag, key)
                    .and_then(|settings| nbt::get_mut(settings, "dimensions"))
                    .and_then(nbt::fields_mut)
                {
                    for (_, dimension) in dimensions {
                        if let Some(settings) = nbt::get_mut(dimension, "generator")
                            .and_then(|generator| nbt::get_mut(generator, "settings"))
                        {
                            for key in ["default_block", "default_fluid"] {
                                if let Some(block) = nbt::get_mut(settings, key) {
                                    changed |= rename_string(block, old, new);
                                    changed |= rename_name(block, "Name", old, new);
                                }
                            }
                            if let Some(layers) =
                                nbt::get_mut(settings, "layers").and_then(nbt::list_mut)
                            {
                                for layer in layers {
                                    changed |= rename_name(layer, "block", old, new);
                                }
                            }
                        }
                    }
                }
            }
        }
        _ => {}
    }
    if changed {
        report.applied(version, "BlockRenameFix");
    }
}
fn namespaced_eq(value: &str, expected: &str) -> bool {
    value == expected
        || (!value.contains(':') && Some(value) == expected.strip_prefix("minecraft:"))
}
fn rename_string(tag: &mut Tag, old: &str, new: &str) -> bool {
    if let Tag::String(value) = tag
        && namespaced_eq(value, old)
    {
        *value = new.into();
        return true;
    }
    false
}
fn rename_name(tag: &mut Tag, key: &str, old: &str, new: &str) -> bool {
    nbt::get_mut(tag, key).is_some_and(|value| rename_string(value, old, new))
}
fn game_event(tag: &mut Tag, report: &mut UpgradeReport) {
    if let Some(Tag::String(event)) = nbt::get_mut(tag, "listener")
        .and_then(|listener| nbt::get_mut(listener, "event"))
        .and_then(|event| nbt::get_mut(event, "game_event"))
    {
        let new = match event.strip_prefix("minecraft:").unwrap_or(event) {
            "block_press" | "block_switch" => "block_activate",
            "block_unpress" | "block_unswitch" => "block_deactivate",
            "drinking_finish" => "drink",
            "elytra_free_fall" => "elytra_glide",
            "entity_damaged" => "entity_damage",
            "entity_dying" | "entity_killed" => "entity_die",
            "mob_interact" => "entity_interact",
            "ravager_roar" => "entity_roar",
            "ring_bell" => "block_change",
            "shulker_close" => "container_close",
            "shulker_open" => "container_open",
            _ => return,
        };
        if !event.contains(':') || event.starts_with("minecraft:") {
            *event = format!("minecraft:{new}");
            report.applied(3084, "NamespacedTypeRenameFix");
        }
    }
}
fn recipe(value: &mut Tag) -> bool {
    let Some(id) = nbt::string(value) else {
        return false;
    };
    let bare = id.strip_prefix("minecraft:").unwrap_or(id);
    let new = match bare {
        "chiseled_stone_bricks_stone_from_stonecutting" => {
            "chiseled_stone_bricks_from_stone_stonecutting"
        }
        "end_stone_brick_slab_from_end_stone_brick_stonecutting" => {
            "end_stone_brick_slab_from_end_stone_bricks_stonecutting"
        }
        "end_stone_brick_stairs_from_end_stone_brick_stonecutting" => {
            "end_stone_brick_stairs_from_end_stone_bricks_stonecutting"
        }
        "end_stone_brick_wall_from_end_stone_brick_stonecutting" => {
            "end_stone_brick_wall_from_end_stone_bricks_stonecutting"
        }
        "mossy_stone_brick_slab_from_mossy_stone_brick_stonecutting" => {
            "mossy_stone_brick_slab_from_mossy_stone_bricks_stonecutting"
        }
        "mossy_stone_brick_stairs_from_mossy_stone_brick_stonecutting" => {
            "mossy_stone_brick_stairs_from_mossy_stone_bricks_stonecutting"
        }
        "mossy_stone_brick_wall_from_mossy_stone_brick_stonecutting" => {
            "mossy_stone_brick_wall_from_mossy_stone_bricks_stonecutting"
        }
        "prismarine_brick_slab_from_prismarine_stonecutting" => {
            "prismarine_brick_slab_from_prismarine_bricks_stonecutting"
        }
        "prismarine_brick_stairs_from_prismarine_stonecutting" => {
            "prismarine_brick_stairs_from_prismarine_bricks_stonecutting"
        }
        "quartz_slab_from_stonecutting" => "quartz_slab_from_quartz_block_stonecutting",
        "stone_brick_walls_from_stone_stonecutting" => "stone_brick_wall_from_stone_stonecutting",
        _ => return false,
    };
    *value = Tag::String(format!("minecraft:{new}"));
    true
}
fn recipes(tag: &mut Tag, kind: NodeKind, report: &mut UpgradeReport) {
    let mut changed = false;
    if kind == NodeKind::Player {
        if let Some(book) = nbt::get_mut(tag, "recipeBook") {
            for key in ["recipes", "toBeDisplayed"] {
                if let Some(values) = nbt::get_mut(book, key).and_then(nbt::list_mut) {
                    for value in values {
                        changed |= recipe(value);
                    }
                }
            }
        }
    } else if kind == NodeKind::BlockEntity {
        if let Some(recipes) = nbt::get_mut(tag, "RecipesUsed").and_then(nbt::fields_mut) {
            for (key, _) in recipes {
                let mut name = Tag::String(std::mem::take(key));
                changed |= recipe(&mut name);
                if let Tag::String(value) = name {
                    *key = value;
                }
            }
        }
    } else if let Some(values) = nbt::get_mut(tag, "components")
        .and_then(|components| nbt::get_mut(components, "minecraft:recipes"))
        .and_then(nbt::list_mut)
    {
        for value in values {
            changed |= recipe(value);
        }
    }
    if changed {
        report.applied(4769, "NamespacedTypeRenameFix");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_nbt::{ListTag, tag_type};
    fn s(value: &str) -> Tag {
        Tag::String(value.into())
    }
    fn list(elements: Vec<Tag>) -> Tag {
        Tag::List(ListTag {
            element_type: elements.first().map_or(tag_type::END, Tag::type_id),
            elements,
        })
    }
    fn fix(tag: &mut Tag, kind: NodeKind, version: i32) -> Result<UpgradeReport, UpgradeError> {
        let mut report = UpgradeReport::default();
        fix_node(tag, kind, version, &mut report)?;
        Ok(report)
    }
    fn light_chunk(on: i8) -> Tag {
        nbt::compound(vec![
            ("isLightOn", Tag::Byte(on)),
            (
                "sections",
                list(vec![nbt::compound(vec![
                    ("BlockLight", Tag::ByteArray(vec![1])),
                    ("SkyLight", Tag::ByteArray(vec![2])),
                ])]),
            ),
        ])
    }
    #[test]
    fn ignored_lighting3077_preserves_enabled_light() {
        let mut tag = light_chunk(1);
        let before = tag.clone();
        fix(&mut tag, NodeKind::Chunk, 3077).unwrap();
        assert_eq!(tag, before);
        nbt::set(&mut tag, "isLightOn", Tag::Byte(0));
        fix(&mut tag, NodeKind::Chunk, 3077).unwrap();
        assert!(
            nbt::get(
                &nbt::list(nbt::get(&tag, "sections").unwrap()).unwrap()[0],
                "BlockLight"
            )
            .is_none()
        );
    }
    #[test]
    fn lights3451_and4537_reset_all_light_fields() {
        for version in [3451, 4537] {
            let mut tag = light_chunk(1);
            fix(&mut tag, NodeKind::Chunk, version).unwrap();
            assert!(nbt::get(&tag, "isLightOn").is_none());
            assert!(
                nbt::fields(&nbt::list(nbt::get(&tag, "sections").unwrap()).unwrap()[0])
                    .unwrap()
                    .is_empty()
            );
        }
    }
    #[test]
    fn blending3108_requires_dimension_and_preserves_overworld() {
        let mut tag = nbt::compound(vec![("blending_data", Tag::Compound(vec![]))]);
        assert!(fix(&mut tag, NodeKind::Chunk, 3108).is_err());
        nbt::set(
            &mut tag,
            "__context",
            nbt::compound(vec![("dimension", s("minecraft:overworld"))]),
        );
        fix(&mut tag, NodeKind::Chunk, 3108).unwrap();
        assert!(nbt::get(&tag, "blending_data").is_some());
        nbt::set(
            &mut tag,
            "__context",
            nbt::compound(vec![("dimension", s("minecraft:the_nether"))]),
        );
        fix(&mut tag, NodeKind::Chunk, 3108).unwrap();
        assert!(nbt::get(&tag, "blending_data").is_none());
    }
    #[test]
    fn statuses3450_including_retrogen() {
        let mut tag = nbt::compound(vec![
            ("Status", s("liquid_carvers")),
            (
                "below_zero_retrogen",
                nbt::compound(vec![("target_status", s("minecraft:heightmaps"))]),
            ),
        ]);
        fix(&mut tag, NodeKind::Chunk, 3450).unwrap();
        assert_eq!(nbt::get(&tag, "Status"), Some(&s("minecraft:carvers")));
        assert_eq!(
            nbt::get(
                nbt::get(&tag, "below_zero_retrogen").unwrap(),
                "target_status"
            ),
            Some(&s("minecraft:spawn"))
        );
    }
    #[test]
    fn carving4057_keeps_air_mask_only() {
        let mut tag = nbt::compound(vec![(
            "CarvingMasks",
            nbt::compound(vec![
                ("AIR", Tag::LongArray(vec![7])),
                ("LIQUID", Tag::LongArray(vec![9])),
            ]),
        )]);
        fix(&mut tag, NodeKind::Chunk, 4057).unwrap();
        assert_eq!(
            nbt::get(&tag, "carving_mask"),
            Some(&Tag::LongArray(vec![7]))
        );
        assert!(nbt::get(&tag, "CarvingMasks").is_none());
    }
    #[test]
    fn blending4295_exact_section_ranges_and_skip_set() {
        let mut tag = nbt::compound(vec![
            ("Status", s("minecraft:full")),
            (
                "__context",
                nbt::compound(vec![("dimension", s("minecraft:overworld"))]),
            ),
        ]);
        fix(&mut tag, NodeKind::Chunk, 4295).unwrap();
        assert_eq!(
            nbt::get(nbt::get(&tag, "blending_data").unwrap(), "min_section"),
            Some(&Tag::Int(-4))
        );
        nbt::set(&mut tag, "Status", s("biomes"));
        nbt::set(
            &mut tag,
            "below_zero_retrogen",
            nbt::compound(vec![("target_status", s("full"))]),
        );
        fix(&mut tag, NodeKind::Chunk, 4295).unwrap();
        assert_eq!(
            nbt::get(nbt::get(&tag, "blending_data").unwrap(), "max_section"),
            Some(&Tag::Int(16))
        );
        nbt::remove(&mut tag, "below_zero_retrogen");
        fix(&mut tag, NodeKind::Chunk, 4295).unwrap();
        assert!(nbt::get(&tag, "blending_data").is_none());
    }
    #[test]
    fn structure2970_uses_biome_palette_not_voxel_counts() {
        let mut tag = nbt::compound(vec![
            (
                "sections",
                list(vec![nbt::compound(vec![(
                    "biomes",
                    nbt::compound(vec![(
                        "palette",
                        list(vec![
                            s("minecraft:desert"),
                            s("minecraft:taiga"),
                            s("minecraft:taiga"),
                        ]),
                    )]),
                )])]),
            ),
            (
                "structures",
                nbt::compound(vec![
                    (
                        "starts",
                        nbt::compound(vec![(
                            "Village",
                            nbt::compound(vec![("id", s("Village")), ("opaque", Tag::Int(9))]),
                        )]),
                    ),
                    (
                        "References",
                        nbt::compound(vec![("Village", Tag::LongArray(vec![3]))]),
                    ),
                ]),
            ),
        ]);
        fix(&mut tag, NodeKind::Chunk, 2970).unwrap();
        let structures = nbt::get(&tag, "structures").unwrap();
        let start = nbt::get(
            nbt::get(structures, "starts").unwrap(),
            "minecraft:village_taiga",
        )
        .unwrap();
        assert_eq!(nbt::get(start, "id"), Some(&s("minecraft:village_taiga")));
        assert_eq!(nbt::get(start, "opaque"), Some(&Tag::Int(9)));
        assert_eq!(
            nbt::get(
                nbt::get(structures, "References").unwrap(),
                "minecraft:village_taiga"
            ),
            Some(&Tag::LongArray(vec![3]))
        );
    }
    #[test]
    fn structure2970_unknown_nonempty_fails_closed() {
        let mut tag = nbt::compound(vec![(
            "structures",
            nbt::compound(vec![(
                "References",
                nbt::compound(vec![("mod:structure", Tag::LongArray(vec![1]))]),
            )]),
        )]);
        assert!(matches!(
            fix(&mut tag, NodeKind::Chunk, 2970),
            Err(UpgradeError::UnsupportedField { version: 2970, .. })
        ));
    }
    #[test]
    fn block3692_and4541_cover_ticks_and_flat_states() {
        for (version, old, new) in [
            (3692, "minecraft:grass", "minecraft:short_grass"),
            (4541, "minecraft:chain", "minecraft:iron_chain"),
        ] {
            let mut state = nbt::compound(vec![("Name", s(old))]);
            fix(&mut state, NodeKind::BlockState, version).unwrap();
            assert_eq!(nbt::get(&state, "Name"), Some(&s(new)));
            let mut chunk = nbt::compound(vec![(
                "block_ticks",
                list(vec![nbt::compound(vec![
                    ("i", s(old)),
                    ("t", Tag::Int(12)),
                ])]),
            )]);
            fix(&mut chunk, NodeKind::Chunk, version).unwrap();
            assert_eq!(
                nbt::get(
                    &nbt::list(nbt::get(&chunk, "block_ticks").unwrap()).unwrap()[0],
                    "i"
                ),
                Some(&s(new))
            );
            let mut jig = nbt::compound(vec![
                ("id", s("minecraft:jigsaw")),
                (
                    "final_state",
                    s(&format!("{old}[waterlogged=false]{{custom:1}}")),
                ),
            ]);
            fix(&mut jig, NodeKind::BlockEntity, version).unwrap();
            assert_eq!(
                nbt::get(&jig, "final_state"),
                Some(&s(&format!("{new}[waterlogged=false]{{custom:1}}")))
            );
        }
    }
    #[test]
    fn block_properties4294_and4305() {
        let mut tag = nbt::compound(vec![
            ("Name", s("minecraft:creaking_heart")),
            ("Properties", nbt::compound(vec![("active", s("true"))])),
        ]);
        fix(&mut tag, NodeKind::BlockState, 4294).unwrap();
        assert_eq!(
            nbt::get(
                nbt::get(&tag, "Properties").unwrap(),
                "creaking_heart_state"
            ),
            Some(&s("awake"))
        );
        nbt::set(&mut tag, "Name", s("minecraft:test_block"));
        nbt::set(
            &mut tag,
            "Properties",
            nbt::compound(vec![("test_block_mode", s("log"))]),
        );
        fix(&mut tag, NodeKind::BlockState, 4305).unwrap();
        assert_eq!(
            nbt::get(nbt::get(&tag, "Properties").unwrap(), "mode"),
            Some(&s("log"))
        );
    }
    #[test]
    fn game_events3084_preserve_other_namespaces() {
        let mut tag = nbt::compound(vec![(
            "listener",
            nbt::compound(vec![(
                "event",
                nbt::compound(vec![("game_event", s("minecraft:block_press"))]),
            )]),
        )]);
        fix(&mut tag, NodeKind::BlockEntity, 3084).unwrap();
        assert_eq!(
            nbt::get(
                nbt::get(nbt::get(&tag, "listener").unwrap(), "event").unwrap(),
                "game_event"
            ),
            Some(&s("minecraft:block_activate"))
        );
    }
    #[test]
    fn recipes4769_rename_only_known_recipe_fields() {
        let old = "minecraft:quartz_slab_from_stonecutting";
        let new = "minecraft:quartz_slab_from_quartz_block_stonecutting";
        let mut tag = nbt::compound(vec![
            (
                "recipeBook",
                nbt::compound(vec![("recipes", list(vec![s(old)]))]),
            ),
            ("opaque", s(old)),
        ]);
        fix(&mut tag, NodeKind::Player, 4769).unwrap();
        assert_eq!(
            nbt::list(nbt::get(nbt::get(&tag, "recipeBook").unwrap(), "recipes").unwrap()).unwrap(),
            &[s(new)]
        );
        assert_eq!(nbt::get(&tag, "opaque"), Some(&s(old)));
    }
}
