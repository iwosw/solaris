//! Vanilla `copy_state` retains named state values in `minecraft:block_state`.
use super::BlockLootContext;
use crate::Identifier;
use mc_nbt::Tag;

#[derive(Debug, Clone, PartialEq)]
pub(super) struct CopyState {
    properties: Vec<String>,
}

impl CopyState {
    pub(super) fn supported(value: &serde_json::Value) -> bool {
        let Some(fields) = value.as_object() else {
            return false;
        };
        fields
            .keys()
            .all(|key| matches!(key.as_str(), "function" | "block" | "properties"))
            && value.get("function").and_then(serde_json::Value::as_str)
                == Some("minecraft:copy_state")
            && value
                .get("block")
                .and_then(serde_json::Value::as_str)
                .is_some()
            && value
                .get("properties")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|properties| {
                    properties
                        .iter()
                        .all(|property| property.as_str().is_some())
                })
    }

    pub(super) fn parse(value: &serde_json::Value) -> Option<Self> {
        if !Self::supported(value) {
            return None;
        }
        Identifier::parse(value.get("block")?.as_str()?).ok()?;
        let properties = value
            .get("properties")?
            .as_array()?
            .iter()
            .map(|property| Some(property.as_str()?.to_owned()))
            .collect::<Option<Vec<_>>>()?;
        Some(Self { properties })
    }

    pub(super) fn apply(&self, context: &BlockLootContext<'_>, output: &mut Vec<(String, Tag)>) {
        let mut properties = Vec::new();
        for name in &self.properties {
            if let Some(value) = context.property(name) {
                properties.push((name.clone(), Tag::String(value.to_owned())));
            }
        }
        if properties.is_empty() {
            return;
        }
        if let Some((_, Tag::Compound(existing))) = output
            .iter_mut()
            .find(|(name, _)| name == "minecraft:block_state")
        {
            for (name, value) in properties {
                if let Some((_, old)) = existing.iter_mut().find(|(key, _)| key == &name) {
                    *old = value;
                } else {
                    existing.push((name, value));
                }
            }
        } else {
            output.push(("minecraft:block_state".into(), Tag::Compound(properties)));
        }
    }
}

pub(super) fn functions(value: &serde_json::Value) -> Option<Vec<CopyState>> {
    let mut output = Vec::new();
    for function in value
        .get("functions")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if function.get("function").and_then(serde_json::Value::as_str)
            == Some("minecraft:copy_state")
        {
            output.push(CopyState::parse(function)?);
        }
    }
    Some(output)
}
