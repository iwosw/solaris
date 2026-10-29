//! `copy_components` over the vanilla block entity's collected component map.
use super::{BlockLootContext, BlockLootEvaluationError, LootCount, LootDrop};
use crate::Identifier;
use mc_nbt::{ListTag, Tag, tag_type};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CopyComponents {
    include: Vec<Identifier>,
}

impl CopyComponents {
    pub(super) fn supported(value: &serde_json::Value) -> bool {
        let Some(fields) = value.as_object() else {
            return false;
        };
        fields
            .keys()
            .all(|key| matches!(key.as_str(), "function" | "source" | "include"))
            && value.get("source").and_then(serde_json::Value::as_str) == Some("block_entity")
            && value.get("function").and_then(serde_json::Value::as_str)
                == Some("minecraft:copy_components")
            && value
                .get("include")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|include| include.iter().all(|name| name.as_str().is_some()))
    }

    pub(super) fn parse(value: &serde_json::Value) -> Option<Self> {
        let fields = value.as_object()?;
        if fields
            .keys()
            .any(|key| !matches!(key.as_str(), "function" | "source" | "include"))
            || value.get("source")?.as_str()? != "block_entity"
            || value.get("function")?.as_str()? != "minecraft:copy_components"
        {
            return None;
        }
        let include = value
            .get("include")?
            .as_array()?
            .iter()
            .map(|name| {
                let id = Identifier::parse(name.as_str()?).ok()?;
                crate::data_component_types_26_1_2::id(&id)?;
                Some(id)
            })
            .collect::<Option<Vec<_>>>()?;
        Some(Self { include })
    }

    pub(super) fn copies(&self, name: &str) -> bool {
        self.include.iter().any(|id| id.as_str() == name)
    }

    pub(super) fn apply(
        &self,
        context: &BlockLootContext<'_>,
        output: &mut Vec<(String, Tag)>,
    ) -> Result<(), BlockLootEvaluationError> {
        let Some(entity) = context.block_entity() else {
            return Ok(());
        };
        let Tag::Compound(fields) = entity else {
            return Err(invalid("root must be a compound"));
        };
        let components = match field(fields, "components") {
            None => &[][..],
            Some(Tag::Compound(components)) => components.as_slice(),
            Some(_) => return Err(invalid("components must be a compound")),
        };
        for (index, (name, _)) in components.iter().enumerate() {
            if components[..index]
                .iter()
                .any(|(previous, _)| previous == name)
            {
                return Err(invalid("components contains duplicate names"));
            }
        }
        for name in &self.include {
            let value = implicit(fields, name.as_str())?
                .or_else(|| field(components, name.as_str()).cloned());
            if let Some(value) = value {
                if let Some((_, old)) = output.iter_mut().find(|(key, _)| key == name.as_str()) {
                    *old = value;
                } else {
                    output.push((name.as_str().to_owned(), value));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn functions(value: &serde_json::Value) -> Option<Vec<CopyComponents>> {
    let mut output = Vec::new();
    for function in value
        .get("functions")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if function.get("function").and_then(serde_json::Value::as_str)
            == Some("minecraft:copy_components")
        {
            output.push(CopyComponents::parse(function)?);
        }
    }
    Some(output)
}

fn invalid(reason: &'static str) -> BlockLootEvaluationError {
    BlockLootEvaluationError::InvalidBlockEntity { reason }
}

fn field<'a>(fields: &'a [(String, Tag)], name: &str) -> Option<&'a Tag> {
    fields
        .iter()
        .find(|(key, _)| key == name)
        .map(|(_, value)| value)
}

fn implicit(
    fields: &[(String, Tag)],
    component: &str,
) -> Result<Option<Tag>, BlockLootEvaluationError> {
    let direct = match component {
        "minecraft:custom_name" => {
            field(fields, "CustomName").or_else(|| field(fields, "custom_name"))
        }
        "minecraft:banner_patterns" => field(fields, "patterns"),
        "minecraft:bees" => field(fields, "bees"),
        "minecraft:pot_decorations" => field(fields, "sherds"),
        "minecraft:profile" => field(fields, "profile"),
        "minecraft:note_block_sound" => field(fields, "note_block_sound"),
        "minecraft:lock" => field(fields, "lock"),
        _ => None,
    };
    if let Some(value) = direct {
        let mut value = value.clone();
        if component == "minecraft:pot_decorations" {
            crate::item_nbt::upgrade_pot_decorations(&mut value)
                .map_err(|_| invalid("invalid pot decorations"))?;
        }
        return Ok(Some(value));
    }
    if component == "minecraft:container_loot" {
        let Some(table) = field(fields, "LootTable") else {
            return Ok(None);
        };
        if !matches!(table, Tag::String(_)) {
            return Err(invalid("LootTable must be a string"));
        }
        let seed = match field(fields, "LootTableSeed") {
            Some(Tag::Long(seed)) => *seed,
            None => 0,
            Some(_) => return Err(invalid("LootTableSeed must be a long")),
        };
        return Ok(Some(Tag::Compound(vec![
            ("loot_table".into(), table.clone()),
            ("seed".into(), Tag::Long(seed)),
        ])));
    }
    if component == "minecraft:container" {
        let Some(items) = field(fields, "Items") else {
            return Ok(None);
        };
        let Tag::List(items) = items else {
            return Err(invalid("Items must be a list"));
        };
        let mut entries = Vec::with_capacity(items.elements.len());
        for item in &items.elements {
            let Tag::Compound(item) = item else {
                return Err(invalid("Items entries must be compounds"));
            };
            let slot = match field(item, "Slot") {
                Some(Tag::Byte(slot)) => i32::from(*slot as u8),
                _ => return Err(invalid("Items Slot must be a byte")),
            };
            let stack = Tag::Compound(
                item.iter()
                    .filter(|(key, _)| key != "Slot")
                    .cloned()
                    .collect(),
            );
            entries.push(Tag::Compound(vec![
                ("slot".into(), Tag::Int(slot)),
                ("item".into(), stack),
            ]));
        }
        return Ok(Some(Tag::List(ListTag {
            element_type: tag_type::COMPOUND,
            elements: entries,
        })));
    }
    Ok(None)
}

pub(super) fn sherds(
    context: &BlockLootContext<'_>,
    brick: &Identifier,
) -> Result<Vec<LootDrop>, BlockLootEvaluationError> {
    let fields = match context.block_entity() {
        None => &[][..],
        Some(Tag::Compound(fields)) => fields.as_slice(),
        Some(_) => return Err(invalid("root must be a compound")),
    };
    let Some(decorations) = field(fields, "sherds") else {
        return Ok(Vec::new());
    };
    let mut output = Vec::with_capacity(4);
    // 26.3 DecoratedPotBlock.getDrops emits optional templates in this order.
    for (side, index) in [("left", 1), ("back", 0), ("front", 3), ("right", 2)] {
        let decoration = match decorations {
            Tag::List(list) if list.elements.len() <= 4 => list.elements.get(index),
            Tag::Compound(sides) => {
                let Some(value) = field(sides, side) else {
                    continue;
                };
                Some(value)
            }
            _ => return Err(invalid("sherds must be named templates or a legacy list")),
        };
        let mut count = 1;
        let mut components = None;
        let name = match decoration {
            Some(Tag::String(name)) => name.as_str(),
            Some(Tag::Compound(template)) => {
                count = match field(template, "count") {
                    None => 1,
                    Some(Tag::Byte(count)) => i32::from(*count),
                    Some(Tag::Short(count)) => i32::from(*count),
                    Some(Tag::Int(count)) => *count,
                    _ => return Err(invalid("sherd count must be an integer")),
                };
                components = match field(template, "components") {
                    None => None,
                    Some(value @ Tag::Compound(_)) => Some(value),
                    _ => return Err(invalid("sherd components must be a compound")),
                };
                match field(template, "id") {
                    Some(Tag::String(name)) => name.as_str(),
                    _ => return Err(invalid("sherd template requires an item id")),
                }
            }
            None => brick.as_str(), // The pre-26.3 list pads missing sides.
            _ => return Err(invalid("invalid sherd template")),
        };
        if !(1..=99).contains(&count) {
            return Err(invalid("invalid sherd template count"));
        }
        let item = Identifier::parse(name)
            .map_err(|_| invalid("sherds contains an invalid item identifier"))?;
        static ITEMS: std::sync::LazyLock<crate::items::ItemRegistry> =
            std::sync::LazyLock::new(crate::items::solaris_required_items);
        if ITEMS.id_of(&item).is_none() {
            return Err(invalid("sherds contains an unknown item"));
        }
        output.push(LootDrop {
            item,
            count: LootCount::Fixed(count as u32),
            components: components.map(|value| Box::new(value.clone())),
        });
    }
    Ok(output)
}

#[cfg(test)]
#[path = "block_components_tests.rs"]
mod tests;
