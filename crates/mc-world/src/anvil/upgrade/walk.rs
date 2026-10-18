//! Only schema-typed children are visited. Remainder compounds (particularly
//! minecraft:custom_data) are never interpreted by resemblance to a vanilla tag.
use super::{NodeKind, UpgradeError, UpgradeReport, nbt};
use mc_nbt::Tag;

pub(super) fn visit(
    tag: &mut Tag,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if kind == NodeKind::Text {
        super::text::fix_node(tag, kind, version, report)?;
        return text_items(tag, version, report);
    }
    if nbt::fields(tag).is_none() {
        return Err(UpgradeError::malformed(
            format!("{kind:?}"),
            "expected typed compound",
        ));
    }
    if matches!(kind, NodeKind::Entity | NodeKind::BlockEntity)
        && nbt::get(tag, "id")
            .and_then(nbt::string)
            .is_some_and(|id| id.contains(':') && !id.starts_with("minecraft:"))
    {
        report.preserved(
            "id",
            "unknown namespaced choice retained without interpreting its remainder fields",
        );
        return Ok(());
    }
    if kind == NodeKind::Chunk && version < 2860 {
        super::old_chunks::apply(tag, version, report)?;
    }
    if version < 2586 {
        super::old_player_map::fix_node(tag, kind, version, report)?;
        super::old_entities::fix_node(tag, kind, version, report)?;
        super::old_items::fix_node(tag, kind, version, report)?;
        super::old_block_states::fix_node(tag, kind, version, report)?;
    }
    if version < 2860 {
        super::old_payloads::fix_node(tag, kind, version, report);
        super::old_worldgen::fix_node(tag, kind, version, report)?;
        super::old_metadata::fix_node(tag, kind, version, report)?;
    }
    // Entity/block-entity fixes can introduce typed children (notably an arrow's
    // item at3818). Visit those children before the same-version item fix runs.
    super::entities::fix_node(tag, kind, version, report)?;
    super::block_entities::fix_node(tag, kind, version, report)?;
    if kind == NodeKind::Item {
        if version < 2586 {
            super::old_items::prepare_legacy_item(tag, version);
        } else {
            super::items::prepare_legacy_item(tag);
        }
    }
    match kind {
        NodeKind::Chunk => {
            let tag = if version < 2842 {
                nbt::get_mut(tag, "Level").ok_or_else(|| {
                    UpgradeError::malformed("Level", "expected legacy chunk compound")
                })?
            } else {
                &mut *tag
            };
            if version < 2842 {
                list_field(tag, "TileEntities", NodeKind::BlockEntity, version, report)?;
                list_field(tag, "Entities", NodeKind::Entity, version, report)?;
                if let Some(sections) = nbt::get_mut(tag, "Sections").and_then(nbt::list_mut) {
                    for section in sections {
                        if version < 2832 {
                            list_field(section, "Palette", NodeKind::BlockState, version, report)?;
                        } else if let Some(states) = nbt::get_mut(section, "block_states") {
                            list_field(states, "palette", NodeKind::BlockState, version, report)?;
                        }
                    }
                }
            }
            list_field(
                tag,
                "block_entities",
                NodeKind::BlockEntity,
                version,
                report,
            )?;
            list_field(tag, "entities", NodeKind::Entity, version, report)?;
            if let Some(sections) = nbt::get_mut(tag, "sections").and_then(nbt::list_mut) {
                for section in sections {
                    if let Some(states) = nbt::get_mut(section, "block_states") {
                        list_field(states, "palette", NodeKind::BlockState, version, report)?;
                    }
                }
            }
        }
        NodeKind::EntityRegion => list_field(tag, "Entities", NodeKind::Entity, version, report)?,
        NodeKind::Player if version < 2586 => {
            for key in ["Inventory", "EnderItems"] {
                list_field(tag, key, NodeKind::Item, version, report)?;
            }
            if version >= 135 {
                if let Some(vehicle) = nbt::get_mut(tag, "RootVehicle") {
                    field(vehicle, "Entity", NodeKind::Entity, version, report)?;
                }
                list_field(tag, "ender_pearls", NodeKind::Entity, version, report)?;
            }
            if version >= 1022 {
                for key in ["ShoulderEntityLeft", "ShoulderEntityRight"] {
                    field(tag, key, NodeKind::Entity, version, report)?;
                }
            }
        }
        NodeKind::Entity | NodeKind::Player => {
            if version < 135 {
                field(tag, "Riding", NodeKind::Entity, version, report)?;
            }
            if version < 100 {
                list_field(tag, "Equipment", NodeKind::Item, version, report)?;
            }
            if version >= 135 {
                list_field(tag, "Passengers", NodeKind::Entity, version, report)?;
            }
            if kind == NodeKind::Player {
                list_field(tag, "ender_pearls", NodeKind::Entity, version, report)?;
            }
            for key in [
                "Inventory",
                "EnderItems",
                "Items",
                "ArmorItems",
                "HandItems",
            ] {
                list_field(tag, key, NodeKind::Item, version, report)?;
            }
            for key in [
                "Item",
                "item",
                "ArmorItem",
                "DecorItem",
                "SaddleItem",
                "body_armor_item",
                "weapon",
                "SelectedItem",
            ] {
                field(tag, key, NodeKind::Item, version, report)?;
            }
            if let Some(equipment) = nbt::get_mut(tag, "equipment").and_then(nbt::fields_mut) {
                for (_, item) in equipment {
                    visit(item, NodeKind::Item, version, report)?;
                }
            }
            if version >= 2586 {
                for key in ["ShoulderEntityLeft", "ShoulderEntityRight"] {
                    field(tag, key, NodeKind::Entity, version, report)?;
                }
                if let Some(vehicle) = nbt::get_mut(tag, "RootVehicle") {
                    field(vehicle, "Entity", NodeKind::Entity, version, report)?;
                }
            }
            if let Some(offers) = nbt::get_mut(tag, "Offers")
                && let Some(recipes) = nbt::get_mut(offers, "Recipes").and_then(nbt::list_mut)
            {
                for recipe in recipes {
                    for key in ["buy", "buyB", "sell"] {
                        field(recipe, key, NodeKind::Item, version, report)?;
                    }
                }
            }
            if version >= 2586 {
                for key in [
                    "BlockState",
                    "block_state",
                    "inBlockState",
                    "DisplayState",
                    "carriedBlockState",
                ] {
                    field(tag, key, NodeKind::BlockState, version, report)?;
                }
            } else if version >= 1451 {
                let state_key = match nbt::get(tag, "id").and_then(nbt::string) {
                    Some("minecraft:falling_block") => Some("BlockState"),
                    Some("minecraft:enderman") => Some("carriedBlockState"),
                    Some("minecraft:arrow" | "minecraft:spectral_arrow") => Some("inBlockState"),
                    Some("minecraft:trident") if version >= 1470 => Some("inBlockState"),
                    Some(
                        "minecraft:minecart"
                        | "minecraft:chest_minecart"
                        | "minecraft:furnace_minecart"
                        | "minecraft:hopper_minecart"
                        | "minecraft:spawner_minecart"
                        | "minecraft:tnt_minecart",
                    ) => Some("DisplayState"),
                    Some("minecraft:commandblock_minecart") if version < 1510 => {
                        Some("DisplayState")
                    }
                    Some("minecraft:command_block_minecart") if version >= 1510 => {
                        Some("DisplayState")
                    }
                    _ => None,
                };
                if let Some(key) = state_key {
                    field(tag, key, NodeKind::BlockState, version, report)?;
                }
            }
            for key in ["Particle", "custom_particle"] {
                if let Some(particle) = nbt::get_mut(tag, key) {
                    field(particle, "item", NodeKind::Item, version, report)?;
                    field(
                        particle,
                        "block_state",
                        NodeKind::BlockState,
                        version,
                        report,
                    )?;
                }
            }
            for key in ["CustomName", "custom_name", "text"] {
                field(tag, key, NodeKind::Text, version, report)?;
            }
            if kind == NodeKind::Entity {
                let id = nbt::get(tag, "id").and_then(nbt::string);
                let rocket = match version {
                    ..705 => Some("FireworksRocketEntity"),
                    705..1510 => Some("minecraft:fireworks_rocket"),
                    _ => Some("minecraft:firework_rocket"),
                };
                let command_cart = match version {
                    ..705 => Some("MinecartCommandBlock"),
                    705..1510 => Some("minecraft:commandblock_minecart"),
                    _ => Some("minecraft:command_block_minecart"),
                };
                let falling = Some(if version < 705 {
                    "FallingSand"
                } else {
                    "minecraft:falling_block"
                });
                let spawner_cart = Some(if version < 705 {
                    "MinecartSpawner"
                } else {
                    "minecraft:spawner_minecart"
                });
                let potion = Some(if version < 705 {
                    "ThrownPotion"
                } else {
                    "minecraft:potion"
                });
                let child = if id == rocket {
                    Some(("FireworksItem", NodeKind::Item))
                } else if id == falling {
                    Some(("TileEntityData", NodeKind::BlockEntity))
                } else if id == command_cart {
                    Some(("LastOutput", NodeKind::Text))
                } else if id == Some("minecraft:trident") && (1470..3685).contains(&version) {
                    Some(("Trident", NodeKind::Item))
                } else if id == potion && version < 2511 {
                    Some(("Potion", NodeKind::Item))
                } else {
                    None
                };
                let is_spawner = id == spawner_cart;
                if let Some((key, child_kind)) = child {
                    field(tag, key, child_kind, version, report)?;
                }
                if is_spawner {
                    spawner(tag, version, report)?;
                }
            }
        }
        NodeKind::Level => {
            field(tag, "Player", NodeKind::Player, version, report)?;
        }
        NodeKind::BlockEntity => {
            list_field(tag, "Items", NodeKind::Item, version, report)?;
            field(tag, "components", NodeKind::Components, version, report)?;
            for key in ["item", "RecordItem", "Book"] {
                field(tag, key, NodeKind::Item, version, report)?;
            }
            for key in ["CustomName", "custom_name", "LastOutput"] {
                field(tag, key, NodeKind::Text, version, report)?;
            }
            for key in [
                "Text1",
                "Text2",
                "Text3",
                "Text4",
                "FilteredText1",
                "FilteredText2",
                "FilteredText3",
                "FilteredText4",
            ] {
                field(tag, key, NodeKind::Text, version, report)?;
            }
            for key in ["front_text", "back_text"] {
                if let Some(text) = nbt::get_mut(tag, key) {
                    list_field(text, "messages", NodeKind::Text, version, report)?;
                    list_field(text, "filtered_messages", NodeKind::Text, version, report)?;
                }
            }
            for key in ["Bees", "bees"] {
                bees(tag, key, version, report)?;
            }
            spawner(tag, version, report)?;
            for key in ["normal_config", "ominous_config"] {
                if let Some(config) = nbt::get_mut(tag, key) {
                    spawner(config, version, report)?;
                }
            }
            field(tag, "blockState", NodeKind::BlockState, version, report)?;
            lock_text(tag, "lock", version, report)?;
        }
        NodeKind::Item => {
            field(tag, "components", NodeKind::Components, version, report)?;
            if let Some(legacy) = nbt::get_mut(tag, "tag") {
                field(
                    legacy,
                    "BlockEntityTag",
                    NodeKind::BlockEntity,
                    version,
                    report,
                )?;
                field(legacy, "EntityTag", NodeKind::Entity, version, report)?;
                list_field(legacy, "Items", NodeKind::Item, version, report)?;
                list_field(
                    legacy,
                    "ChargedProjectiles",
                    NodeKind::Item,
                    version,
                    report,
                )?;
                if let Some(display) = nbt::get_mut(legacy, "display") {
                    field(display, "Name", NodeKind::Text, version, report)?;
                    list_field(display, "Lore", NodeKind::Text, version, report)?;
                }
            }
        }
        NodeKind::Components => {
            field(
                tag,
                "minecraft:block_entity_data",
                NodeKind::BlockEntity,
                version,
                report,
            )?;
            field(
                tag,
                "minecraft:entity_data",
                NodeKind::Entity,
                version,
                report,
            )?;
            list_field(
                tag,
                "minecraft:bundle_contents",
                NodeKind::Item,
                version,
                report,
            )?;
            list_field(
                tag,
                "minecraft:charged_projectiles",
                NodeKind::Item,
                version,
                report,
            )?;
            if let Some(container) =
                nbt::get_mut(tag, "minecraft:container").and_then(nbt::list_mut)
            {
                for slot in container {
                    field(slot, "item", NodeKind::Item, version, report)?;
                }
            }
            for key in ["minecraft:custom_name", "minecraft:item_name"] {
                field(tag, key, NodeKind::Text, version, report)?;
            }
            list_field(tag, "minecraft:lore", NodeKind::Text, version, report)?;
            if let Some(book) = nbt::get_mut(tag, "minecraft:written_book_content")
                && let Some(pages) = nbt::get_mut(book, "pages").and_then(nbt::list_mut)
            {
                for page in pages {
                    field(page, "raw", NodeKind::Text, version, report)?;
                    field(page, "filtered", NodeKind::Text, version, report)?;
                }
            }
            bees(tag, "minecraft:bees", version, report)?;
            lock_text(tag, "minecraft:lock", version, report)?;
            if let Some(predicates) = nbt::get_mut(tag, "minecraft:can_break")
                .and_then(|p| nbt::get_mut(p, "predicates"))
                .and_then(nbt::list_mut)
            {
                for predicate in predicates {
                    lock_text(predicate, "components", version, report)?;
                }
            }
        }
        NodeKind::BlockState | NodeKind::Text => {}
    }
    if version < 2586 {
        super::old_entities::after_children(tag, kind, version, report)?;
    }
    super::items::fix_node(tag, kind, version, report)?;
    super::metadata::fix_node(tag, kind, version, report)?;
    super::chunks::fix_node(tag, kind, version, report)
}

fn field(
    tag: &mut Tag,
    key: &str,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if let Some(child) = nbt::get_mut(tag, key) {
        visit(child, kind, version, report)?;
    }
    Ok(())
}
fn list_field(
    tag: &mut Tag,
    key: &str,
    kind: NodeKind,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if let Some(child) = nbt::get_mut(tag, key) {
        let Tag::List(list) = child else {
            return Err(UpgradeError::malformed(key, "expected typed list"));
        };
        for element in &mut list.elements {
            let element = if kind == NodeKind::Text && version > 4290 {
                super::text::logical_mut(element, list.element_type)
            } else {
                element
            };
            visit(element, kind, version, report)?;
        }
        if kind == NodeKind::Text && version == 4290 {
            // Unflattening changes JSON strings into arbitrary component tags.
            // Rebuild the wire list, including vanilla's heterogeneous wrappers.
            *child = super::text::wire_list(std::mem::take(&mut list.elements));
        }
    }
    Ok(())
}
fn bees(
    tag: &mut Tag,
    key: &str,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if let Some(bees) = nbt::get_mut(tag, key).and_then(nbt::list_mut) {
        for bee in bees {
            field(bee, "EntityData", NodeKind::Entity, version, report)?;
            field(bee, "entity_data", NodeKind::Entity, version, report)?;
        }
    }
    Ok(())
}
fn spawner(tag: &mut Tag, version: i32, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    if version < 2831 {
        field(tag, "SpawnData", NodeKind::Entity, version, report)?;
    } else if let Some(data) = nbt::get_mut(tag, "SpawnData") {
        field(data, "entity", NodeKind::Entity, version, report)?;
    }
    for key in ["SpawnPotentials", "spawn_potentials"] {
        if let Some(potentials) = nbt::get_mut(tag, key).and_then(nbt::list_mut) {
            for potential in potentials {
                field(potential, "Entity", NodeKind::Entity, version, report)?;
                if let Some(data) = nbt::get_mut(potential, "data") {
                    field(data, "entity", NodeKind::Entity, version, report)?;
                }
            }
        }
    }
    Ok(())
}
fn lock_text(
    tag: &mut Tag,
    key: &str,
    version: i32,
    report: &mut UpgradeReport,
) -> Result<(), UpgradeError> {
    if let Some(lock) = nbt::get_mut(tag, key)
        && let Some(components) = nbt::get_mut(lock, "components")
    {
        field(
            components,
            "minecraft:custom_name",
            NodeKind::Text,
            version,
            report,
        )?;
    }
    Ok(())
}

// TEXT owns its internal conversion. Only visit item-stack optics here, never
// reparse resulting literal strings as JSON after the4290 structural conversion.
fn text_items(tag: &mut Tag, version: i32, report: &mut UpgradeReport) -> Result<(), UpgradeError> {
    if let Some(elements) = nbt::list_mut(tag) {
        for element in elements {
            text_items(element, version, report)?;
        }
        return Ok(());
    }
    if let Some(extra) = nbt::get_mut(tag, "extra").and_then(nbt::list_mut) {
        for child in extra {
            text_items(child, version, report)?;
        }
    }
    if let Some(separator) = nbt::get_mut(tag, "separator") {
        text_items(separator, version, report)?;
    }
    for key in ["hoverEvent", "hover_event"] {
        if let Some(hover) = nbt::get_mut(tag, key) {
            match nbt::get(hover, "action").and_then(nbt::string) {
                Some("show_item") if key == "hover_event" => {
                    visit(hover, NodeKind::Item, version, report)?
                }
                Some("show_item") => field(hover, "contents", NodeKind::Item, version, report)?,
                Some("show_text") => {
                    let value = if key == "hover_event" {
                        "value"
                    } else {
                        "contents"
                    };
                    if let Some(child) = nbt::get_mut(hover, value) {
                        text_items(child, version, report)?;
                    }
                }
                Some("show_entity") => {
                    if let Some(name) = nbt::get_mut(hover, "name") {
                        text_items(name, version, report)?;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}
