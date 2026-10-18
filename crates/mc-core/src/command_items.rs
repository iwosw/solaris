//! Native item arguments for the supported give/debug-give/clear commands.
//! ItemParser$State and ComponentPredicateParser (26.1.2) use bracketed
//! component patches, SNBT values, negation, and predicate alternatives.
use crate::{commands::CommandError, inventory::ItemRules};
use mc_data::{Identifier, ItemStack, items::ItemRegistry};
use mc_nbt::Tag;

#[derive(Debug, Clone, PartialEq)]
pub struct ItemArgument {
    pub item: Identifier,
    pub components: Vec<(String, Tag)>,
}
impl ItemArgument {
    pub fn plain(item: Identifier) -> Self {
        Self {
            item,
            components: Vec::new(),
        }
    }
    pub fn parse(raw: &str) -> Result<Self, CommandError> {
        let (base, patch) = parts(raw)?;
        let item = identifier(base)?;
        let mut components = Vec::new();
        if let Some(patch) = patch {
            conditions(patch, false, |condition, _| {
                if components
                    .iter()
                    .any(|(name, _): &(String, Tag)| name.trim_start_matches('!') == condition.name)
                {
                    return Err(error(format!("Repeated item component {}", condition.name)));
                }
                let name = if condition.negated {
                    format!("!{}", condition.name)
                } else {
                    condition.name.to_owned()
                };
                components.push((
                    name,
                    condition.value.unwrap_or_else(|| Tag::Compound(Vec::new())),
                ));
                Ok(())
            })?;
        }
        Ok(Self { item, components })
    }
    pub fn stack(&self, items: &ItemRegistry) -> Result<ItemStack, String> {
        if self.components.is_empty() {
            return items
                .id_of(&self.item)
                .map(|id| ItemStack::new(id, 1))
                .ok_or_else(|| format!("Unknown item {}", self.item));
        }
        mc_data::item_nbt::stack_from_component_patch(items, &self.item, &self.components)
            .map_err(|error| error.to_string())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemPredicateBase {
    Any,
    Item(Identifier),
    Tag(Identifier),
}
#[derive(Debug, Clone, PartialEq)]
pub struct ItemPredicate {
    pub base: ItemPredicateBase,
    pub conditions: Vec<Vec<ComponentCondition>>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct ComponentCondition {
    pub name: &'static str,
    pub value: Option<Tag>,
    pub negated: bool,
}
impl ItemPredicate {
    pub fn parse(raw: &str) -> Result<Self, CommandError> {
        let (base, patch) = parts(raw)?;
        let base = if base == "*" {
            ItemPredicateBase::Any
        } else if let Some(tag) = base.strip_prefix('#') {
            ItemPredicateBase::Tag(identifier(tag)?)
        } else {
            ItemPredicateBase::Item(identifier(base)?)
        };
        let mut groups = Vec::new();
        let mut group = Vec::new();
        if let Some(patch) = patch {
            conditions(patch, true, |condition, alternative| {
                group.push(condition);
                if !alternative {
                    groups.push(std::mem::take(&mut group));
                }
                Ok(())
            })?;
        }
        Ok(Self {
            base,
            conditions: groups,
        })
    }
    pub fn validate(&self, rules: &ItemRules<'_>) -> Result<(), String> {
        match &self.base {
            ItemPredicateBase::Item(item) if rules.items.id_of(item).is_none() => {
                return Err(format!("Unknown item {item}"));
            }
            ItemPredicateBase::Tag(tag) => {
                let registry = Identifier::parse("minecraft:item").expect("built-in item registry");
                if rules
                    .tags
                    .registries
                    .get(&registry)
                    .is_none_or(|tags| !tags.contains_key(tag))
                {
                    return Err(format!("Unknown item tag {tag}"));
                }
            }
            _ => {}
        }
        let item = match &self.base {
            ItemPredicateBase::Item(item) => item.clone(),
            _ => Identifier::parse("minecraft:stone").expect("built-in validation item"),
        };
        for group in &self.conditions {
            for condition in group {
                if let Some(value) = &condition.value {
                    mc_data::item_nbt::stack_from_component_patch(
                        rules.items,
                        &item,
                        &[(condition.name.to_owned(), value.clone())],
                    )
                    .map_err(|error| error.to_string())?;
                }
            }
        }
        Ok(())
    }
    pub fn matches(&self, rules: &ItemRules<'_>, stack: &ItemStack) -> bool {
        if stack.is_empty() {
            return false;
        }
        let base = match &self.base {
            ItemPredicateBase::Any => true,
            ItemPredicateBase::Item(item) => rules.items.id_of(item) == Some(stack.item_id),
            ItemPredicateBase::Tag(tag) => {
                rules
                    .tags
                    .contains_raw_id("minecraft:item", tag.as_str(), stack.item_id as i32)
            }
        };
        if !base || self.conditions.is_empty() {
            return base;
        }
        let Ok(fields) = mc_data::item_nbt::stack_to_fields(rules.items, stack) else {
            return false;
        };
        let components = match mc_data::item_nbt::field(&fields, "components") {
            Some(Tag::Compound(values)) => values.as_slice(),
            _ => &[],
        };
        self.conditions.iter().all(|group| {
            group.iter().any(|condition| {
                let actual = mc_data::item_nbt::field(components, condition.name);
                let matches = match &condition.value {
                    Some(expected) => {
                        actual.is_some_and(|actual| equal_component(actual, expected))
                    }
                    None => actual.is_some(),
                };
                matches != condition.negated
            })
        })
    }
}
fn equal_component(actual: &Tag, expected: &Tag) -> bool {
    // Literal text's native codec accepts either shorthand string or {text:...}.
    fn literal(value: &Tag) -> Option<&str> {
        match value {
            Tag::String(text) => Some(text),
            Tag::Compound(fields) if fields.len() == 1 && fields[0].0 == "text" => {
                match &fields[0].1 {
                    Tag::String(text) => Some(text),
                    _ => None,
                }
            }
            _ => None,
        }
    }
    match (actual, expected) {
        (Tag::Compound(left), Tag::Compound(right)) => {
            left.len() == right.len()
                && left.iter().all(|(name, value)| {
                    mc_data::item_nbt::field(right, name)
                        .is_some_and(|other| equal_component(value, other))
                })
        }
        (Tag::List(left), Tag::List(right)) => {
            left.elements.len() == right.elements.len()
                && left
                    .elements
                    .iter()
                    .zip(&right.elements)
                    .all(|(left, right)| equal_component(left, right))
        }
        _ => match (literal(actual), literal(expected)) {
            (Some(left), Some(right)) => left == right,
            _ => actual == expected,
        },
    }
}
fn identifier(raw: &str) -> Result<Identifier, CommandError> {
    let name = if raw.contains(':') {
        raw.to_owned()
    } else {
        format!("minecraft:{raw}")
    };
    Identifier::parse(name).map_err(|_| error(format!("Invalid item identifier {raw}")))
}
fn error(message: impl Into<String>) -> CommandError {
    CommandError::Message(message.into())
}
fn parts(raw: &str) -> Result<(&str, Option<&str>), CommandError> {
    match raw.split_once('[') {
        None => Ok((raw, None)),
        Some((base, patch)) => patch
            .strip_suffix(']')
            .map(|patch| (base, Some(patch)))
            .ok_or_else(|| error("Unterminated item component patch")),
    }
}
fn conditions(
    mut raw: &str,
    predicate: bool,
    mut visit: impl FnMut(ComponentCondition, bool) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    raw = raw.trim_start();
    while !raw.is_empty() {
        let negated = raw.starts_with('!');
        if negated {
            raw = raw[1..].trim_start();
        }
        let end = raw
            .find(|ch: char| ch.is_whitespace() || matches!(ch, '=' | '~' | ',' | '|'))
            .unwrap_or(raw.len());
        let parsed_name = identifier(&raw[..end])?;
        let name = mc_data::data_component_types_26_1_2::ITEM_COMPONENT_CODECS
            .iter()
            .find_map(|(supported, _)| (*supported == parsed_name.as_str()).then_some(*supported))
            .ok_or_else(|| error(format!("Unsupported item component {parsed_name}")))?;
        raw = raw[end..].trim_start();
        let value = if let Some(value) = raw.strip_prefix('=') {
            if !predicate && negated {
                return Err(error("Removed components cannot have a value"));
            }
            let (value, consumed) = crate::command_snbt::parse_prefix(value).map_err(error)?;
            raw = raw[1 + consumed..].trim_start();
            Some(value)
        } else {
            if raw.starts_with('~') {
                return Err(error(format!("Unsupported item subpredicate {name}")));
            }
            if !predicate && !negated {
                return Err(error(format!("Expected value for item component {name}")));
            }
            None
        };
        let alternative = raw.starts_with('|');
        visit(
            ComponentCondition {
                name,
                value,
                negated,
            },
            alternative,
        )?;
        if let Some(rest) = raw.strip_prefix('|') {
            if !predicate {
                return Err(error("Item patches do not support alternatives"));
            }
            raw = rest.trim_start();
            if raw.is_empty() {
                return Err(error("Missing item component alternative"));
            }
        } else {
            if raw.is_empty() {
                break;
            }
            raw = raw
                .strip_prefix(',')
                .ok_or_else(|| error("Expected comma between item components"))?
                .trim_start();
            if predicate && raw.is_empty() {
                return Err(error("Missing item component condition"));
            }
            // Native ItemParser permits a trailing comma before the bracket.
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "command_items_tests.rs"]
mod tests;
