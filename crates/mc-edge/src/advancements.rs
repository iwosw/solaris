//! Advancement and recipe wire catalogs resolved once from authoritative data.
use mc_data::advancements::AdvancementTree;
use mc_protocol::codec::Identifier;
use mc_protocol::packets::play::{
    AdvancementDefinition, AdvancementDisplay, AdvancementFrame, ClientboundUpdateAdvancements,
    RecipeBookEntry,
};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Default)]
pub struct AdvancementPackets {
    pub definitions: BTreeMap<String, AdvancementDefinition>,
    pub recipes: BTreeMap<String, RecipeBookEntry>,
}
impl AdvancementPackets {
    pub fn new(registry: &mc_core::registry::CoreRegistry) -> anyhow::Result<Self> {
        let mut definitions = BTreeMap::new();
        let mut positions = BTreeMap::<String, (usize, f32)>::new();
        let mut children = BTreeMap::<Option<&str>, Vec<&str>>::new();
        for (id, definition) in &registry.advancements.definitions {
            children
                .entry(definition.parent.as_deref())
                .or_default()
                .push(id);
        }
        for id in children.get(&None).into_iter().flatten() {
            position(
                id,
                0,
                &mut 0.0,
                &registry.advancements,
                &children,
                &mut positions,
            );
        }
        for (id, definition) in &registry.advancements.definitions {
            let display = definition
                .display
                .as_ref()
                .map(|display| {
                    let icon = display
                        .get("icon")
                        .ok_or_else(|| anyhow::anyhow!("advancement {id}: missing icon"))?;
                    let fields = match json_tag(icon)? {
                        mc_nbt::Tag::Compound(fields) => fields,
                        _ => anyhow::bail!("advancement {id}: invalid icon"),
                    };
                    let icon = mc_data::item_nbt::stack_from_fields(&registry.items, &fields)
                        .map_err(|err| anyhow::anyhow!("advancement {id} icon: {err}"))?
                        .ok_or_else(|| anyhow::anyhow!("advancement {id}: empty icon"))?;
                    let frame = match display
                        .get("frame")
                        .and_then(Value::as_str)
                        .unwrap_or("task")
                    {
                        "task" => AdvancementFrame::Task,
                        "challenge" => AdvancementFrame::Challenge,
                        "goal" => AdvancementFrame::Goal,
                        _ => anyhow::bail!("advancement {id}: invalid frame"),
                    };
                    let (depth, y) = positions[id];
                    Ok::<_, anyhow::Error>(AdvancementDisplay {
                        title: mc_core::scoreboard::component(&display["title"].to_string()),
                        description: mc_core::scoreboard::component(
                            &display["description"].to_string(),
                        ),
                        icon,
                        frame,
                        background: display
                            .get("background")
                            .and_then(Value::as_str)
                            .map(Identifier::parse)
                            .transpose()?,
                        show_toast: display
                            .get("show_toast")
                            .and_then(Value::as_bool)
                            .unwrap_or(true),
                        hidden: display
                            .get("hidden")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                        x: depth as f32,
                        y,
                    })
                })
                .transpose()?;
            definitions.insert(
                id.clone(),
                AdvancementDefinition {
                    id: Identifier::parse(id)?,
                    parent: definition
                        .parent
                        .as_deref()
                        .map(Identifier::parse)
                        .transpose()?,
                    display,
                    requirements: definition.requirements.clone(),
                    sends_telemetry: definition.sends_telemetry_event.unwrap_or(false),
                },
            );
        }
        let recipes = registry
            .recipes
            .iter()
            .enumerate()
            .filter_map(|(index, recipe)| {
                crate::recipes::recipe_book_entry(
                    index,
                    recipe,
                    &registry.items,
                    &registry.item_facts,
                )
                .map(|entry| (recipe.id.to_string(), entry))
            })
            .collect();
        Ok(Self {
            definitions,
            recipes,
        })
    }
    pub fn update(
        &self,
        reset: bool,
        tree: &AdvancementTree,
        progress: &Value,
    ) -> ClientboundUpdateAdvancements {
        let added = tree
            .definitions
            .keys()
            .filter_map(|id| self.definitions.get(id).cloned())
            .collect();
        let progress = progress
            .as_object()
            .into_iter()
            .flatten()
            .filter_map(|(id, value)| {
                let definition = self.definitions.get(id)?;
                let granted = value.get("criteria").and_then(Value::as_object);
                let mut criteria = BTreeMap::new();
                for name in definition.requirements.iter().flatten() {
                    let timestamp = granted
                        .and_then(|map| map.get(name))
                        .and_then(Value::as_str)
                        .and_then(mc_core::advancements::timestamp_to_millis);
                    criteria.insert(name.clone(), timestamp);
                }
                Some((definition.id.clone(), criteria))
            })
            .collect();
        ClientboundUpdateAdvancements {
            reset,
            added,
            removed: Vec::new(),
            progress,
            show_advancements: true,
        }
    }
}
fn position<'a>(
    id: &'a str,
    depth: usize,
    next_y: &mut f32,
    tree: &AdvancementTree,
    children: &BTreeMap<Option<&'a str>, Vec<&'a str>>,
    positions: &mut BTreeMap<String, (usize, f32)>,
) -> f32 {
    let displayed = tree.definitions[id].display.is_some();
    let mut first = None;
    let mut last = 0.0;
    for child in children.get(&Some(id)).into_iter().flatten() {
        let y = position(
            child,
            depth + usize::from(displayed),
            next_y,
            tree,
            children,
            positions,
        );
        first.get_or_insert(y);
        last = y;
    }
    let y = first.map_or_else(
        || {
            let y = *next_y;
            if displayed {
                *next_y += 1.0;
            }
            y
        },
        |first| (first + last) / 2.0,
    );
    positions.insert(id.to_owned(), (depth, y));
    y
}
fn json_tag(value: &Value) -> anyhow::Result<mc_nbt::Tag> {
    use mc_nbt::{ListTag, Tag};
    Ok(match value {
        Value::String(value) => Tag::String(value.clone()),
        Value::Bool(value) => Tag::Byte(i8::from(*value)),
        Value::Number(value) => match value.as_i64() {
            Some(value) => Tag::Int(i32::try_from(value)?),
            None => Tag::Double(
                value
                    .as_f64()
                    .ok_or_else(|| anyhow::anyhow!("invalid number"))?,
            ),
        },
        Value::Object(fields) => Tag::Compound(
            fields
                .iter()
                .map(|(key, value)| Ok((key.clone(), json_tag(value)?)))
                .collect::<anyhow::Result<_>>()?,
        ),
        Value::Array(values) => {
            let elements: Vec<Tag> = values.iter().map(json_tag).collect::<anyhow::Result<_>>()?;
            let element_type = match elements.first() {
                Some(Tag::String(_)) => mc_nbt::tag_type::STRING,
                Some(Tag::Compound(_)) => mc_nbt::tag_type::COMPOUND,
                None => mc_nbt::tag_type::END,
                _ => anyhow::bail!("unsupported icon component list"),
            };
            Tag::List(ListTag {
                element_type,
                elements,
            })
        }
        Value::Null => anyhow::bail!("null icon component"),
    })
}
#[cfg(test)]
#[path = "advancements_tests.rs"]
mod tests;
