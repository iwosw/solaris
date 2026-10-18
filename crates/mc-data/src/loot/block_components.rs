//! `copy_components` over the vanilla block entity's collected component map.
use super::{BlockLootContext, BlockLootEvaluationError};
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
        return Ok(Some(value.clone()));
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
) -> Result<Vec<Identifier>, BlockLootEvaluationError> {
    let fields = match context.block_entity() {
        None => &[][..],
        Some(Tag::Compound(fields)) => fields.as_slice(),
        Some(_) => return Err(invalid("root must be a compound")),
    };
    let decorations = match field(fields, "sherds") {
        None => &[][..],
        Some(Tag::List(list))
            if list.elements.len() <= 4
                && (list.elements.is_empty() || list.element_type == tag_type::STRING) =>
        {
            list.elements.as_slice()
        }
        Some(_) => {
            return Err(invalid(
                "sherds must be a string list of at most four entries",
            ));
        }
    };
    let mut output = Vec::with_capacity(4);
    for decoration in decorations {
        let Tag::String(name) = decoration else {
            return Err(invalid("sherds entries must be strings"));
        };
        let item = Identifier::parse(name)
            .map_err(|_| invalid("sherds contains an invalid item identifier"))?;
        static ITEMS: std::sync::LazyLock<crate::items::ItemRegistry> =
            std::sync::LazyLock::new(crate::items::solaris_required_items);
        if ITEMS.id_of(&item).is_none() {
            return Err(invalid("sherds contains an unknown item"));
        }
        output.push(item);
    }
    output.resize(4, brick.clone());
    Ok(output)
}

#[cfg(test)]
#[path = "block_components_tests.rs"]
mod tests;
