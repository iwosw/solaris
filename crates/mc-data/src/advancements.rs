//! Data-pack advancement trees and fail-closed event predicates.
//!
//! Mojang JSON remains local runtime input, including recipe advancements.
//! Requirements are an AND of OR groups. The predicate contract is documented
//! on [`EventContext`]; unknown fields/conditions never award a criterion.

mod location;
mod predicates;
pub use predicates::{Evaluation, EventContext, evaluate_criterion};

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdvancementTree {
    pub definitions: BTreeMap<String, AdvancementDefinition>,
    /// Startup index: trigger -> (advancement id, criterion name).
    pub trigger_index: BTreeMap<String, Vec<(String, String)>>,
    /// Startup dispatch for inventory/recipe events; positions in trigger_index.
    pub candidate_index: AdvancementCandidateIndex,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AdvancementCandidateIndex {
    compiled: bool,
    inventory_items: BTreeMap<String, Vec<usize>>,
    inventory_tags: BTreeMap<String, Vec<usize>>,
    inventory_any: Vec<usize>,
    recipes: BTreeMap<String, Vec<usize>>,
    recipe_any: Vec<usize>,
    location: location::LocationIndex,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdvancementDefinition {
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub display: Option<Value>,
    pub criteria: BTreeMap<String, Criterion>,
    #[serde(default)]
    pub requirements: Vec<Vec<String>>,
    #[serde(default)]
    pub rewards: AdvancementRewards,
    #[serde(default)]
    pub sends_telemetry_event: Option<bool>,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Criterion {
    pub trigger: String,
    #[serde(default)]
    pub conditions: Value,
}

#[derive(Debug, Clone, Default, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AdvancementRewards {
    #[serde(default)]
    pub experience: i32,
    #[serde(default)]
    pub loot: Vec<String>,
    #[serde(default)]
    pub recipes: Vec<String>,
    #[serde(default)]
    pub function: Option<String>,
}

#[derive(Debug, Error)]
pub enum AdvancementDataError {
    #[error("advancement data io error at {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("advancement data parse error at {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error("invalid advancement {id}: {reason}")]
    Invalid { id: String, reason: String },
    #[error("no advancement data under {0}")]
    Empty(PathBuf),
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SupportReport {
    pub trigger_counts: BTreeMap<String, usize>,
    /// Paths include the trigger and field/condition name; counts are criteria.
    pub unsupported_fields: BTreeMap<String, usize>,
    /// Function rewards need a command-function executor, not a predicate.
    pub unsupported_functions: BTreeMap<String, String>,
    pub supported_criteria: usize,
    pub unsupported_criteria: usize,
}

impl AdvancementDefinition {
    #[must_use]
    pub fn requirements_met(&self, completed: &BTreeSet<String>) -> bool {
        !self.requirements.is_empty()
            && self
                .requirements
                .iter()
                .all(|group| group.iter().any(|criterion| completed.contains(criterion)))
    }
}

impl AdvancementTree {
    pub fn load(root: &Path) -> Result<Self, AdvancementDataError> {
        let data = root.join("data");
        let mut definitions = BTreeMap::new();
        let namespaces = std::fs::read_dir(&data).map_err(|source| io(data.clone(), source))?;
        for namespace in namespaces {
            let namespace = namespace.map_err(|source| io(data.clone(), source))?;
            if !namespace
                .file_type()
                .map_err(|source| io(namespace.path(), source))?
                .is_dir()
            {
                continue;
            }
            let name = namespace.file_name().to_string_lossy().into_owned();
            let directory = namespace.path().join("advancement");
            if !crate::sidecar::directory_exists_under(root, &directory)
                .map_err(|error| io(error.path, error.source))?
            {
                continue;
            }
            let paths = crate::sidecar::collect_files_under(root, &directory, "json", true)
                .map_err(|error| io(error.path, error.source))?;
            for path in paths {
                let relative = path
                    .strip_prefix(&directory)
                    .expect("sidecar paths stay under directory");
                let id = canonical_id(
                    &format!(
                        "{}:{}",
                        name,
                        relative
                            .with_extension("")
                            .to_string_lossy()
                            .replace('\\', "/")
                    ),
                    &path.display().to_string(),
                )?;
                let raw: Value = crate::read_json_file(&path, &io, &|path, source| {
                    AdvancementDataError::Parse { path, source }
                })?;
                if raw
                    .get("requirements")
                    .is_some_and(|requirements| requirements.as_array().is_some_and(Vec::is_empty))
                {
                    return Err(invalid(&id, "explicit empty requirements"));
                }
                let mut definition: AdvancementDefinition =
                    serde_json::from_value(raw).map_err(|source| AdvancementDataError::Parse {
                        path: path.clone(),
                        source,
                    })?;
                validate_definition(&id, &mut definition)?;
                if definitions.insert(id.clone(), definition).is_some() {
                    return Err(invalid(&id, "duplicate identifier"));
                }
            }
        }
        if definitions.is_empty() {
            return Err(AdvancementDataError::Empty(data));
        }
        // A parent edge is single-valued: iterative chain walks avoid stack
        // overflow even for a long data-pack hierarchy.
        let mut checked = BTreeSet::new();
        for id in definitions.keys() {
            let mut chain = BTreeSet::new();
            let mut current = id.as_str();
            while !checked.contains(current) {
                if !chain.insert(current.to_string()) {
                    return Err(invalid(id, "parent cycle"));
                }
                let definition = definitions
                    .get(current)
                    .ok_or_else(|| invalid(id, format!("missing parent {current}")))?;
                let Some(parent) = definition.parent.as_deref() else {
                    break;
                };
                current = parent;
            }
            checked.extend(chain);
        }
        let mut tree = Self {
            definitions,
            ..Default::default()
        };
        tree.rebuild_trigger_index();
        Ok(tree)
    }

    /// Rebuild after constructing or changing an in-memory data-pack tree.
    pub fn rebuild_trigger_index(&mut self) {
        self.trigger_index.clear();
        for (id, definition) in &self.definitions {
            for (name, criterion) in &definition.criteria {
                self.trigger_index
                    .entry(criterion.trigger.clone())
                    .or_default()
                    .push((id.clone(), name.clone()));
            }
        }
        let mut index = AdvancementCandidateIndex::default();
        if let Some(criteria) = self.trigger_index.get("minecraft:inventory_changed") {
            for (position, (id, name)) in criteria.iter().enumerate() {
                // Every inventory item predicate is mandatory. One concrete
                // identifier constraint is enough to rule out an impossible
                // candidate; the complete predicate still decides the award.
                let anchor = self.definitions[id].criteria[name]
                    .conditions
                    .get("items")
                    .and_then(Value::as_array)
                    .and_then(|items| items.iter().find_map(|item| item.get("items")));
                match anchor {
                    Some(Value::String(item)) => {
                        let (table, key) = if let Some(tag) = item.strip_prefix('#') {
                            (&mut index.inventory_tags, tag)
                        } else {
                            (&mut index.inventory_items, item.as_str())
                        };
                        table.entry(key.to_owned()).or_default().push(position);
                    }
                    Some(Value::Array(items))
                        if !items.is_empty() && items.iter().all(Value::is_string) =>
                    {
                        for item in items {
                            index
                                .inventory_items
                                .entry(item.as_str().expect("string checked").to_owned())
                                .or_default()
                                .push(position);
                        }
                    }
                    _ => index.inventory_any.push(position),
                }
            }
        }
        if let Some(criteria) = self.trigger_index.get("minecraft:recipe_unlocked") {
            for (position, (id, name)) in criteria.iter().enumerate() {
                if let Some(recipe) = self.definitions[id].criteria[name]
                    .conditions
                    .get("recipe")
                    .and_then(Value::as_str)
                {
                    index
                        .recipes
                        .entry(recipe.to_owned())
                        .or_default()
                        .push(position);
                } else {
                    index.recipe_any.push(position);
                }
            }
        }
        if let Some(criteria) = self.trigger_index.get("minecraft:location") {
            for (position, (id, name)) in criteria.iter().enumerate() {
                index
                    .location
                    .insert(position, &self.definitions[id].criteria[name]);
            }
        }
        index.compiled = true;
        self.candidate_index = index;
    }

    /// Select only criteria whose mandatory event discriminator is present.
    /// Reuses caller scratch; never changes the fail-closed predicate evaluator.
    pub fn candidate_indices(&self, context: &EventContext<'_>, selected: &mut Vec<usize>) {
        selected.clear();
        let index = &self.candidate_index;
        if index.compiled && context.trigger == "minecraft:inventory_changed" {
            selected.extend(&index.inventory_any);
            for item in context.inventory {
                if let Some(id) = item.get("id").and_then(Value::as_str)
                    && let Some(criteria) = index.inventory_items.get(id)
                {
                    selected.extend(criteria);
                }
                if let Some(tags) = item.get("tags").and_then(Value::as_array) {
                    for tag in tags.iter().filter_map(Value::as_str) {
                        if let Some(criteria) = index.inventory_tags.get(tag) {
                            selected.extend(criteria);
                        }
                    }
                }
            }
            selected.sort_unstable();
            selected.dedup();
        } else if index.compiled && context.trigger == "minecraft:recipe_unlocked" {
            selected.extend(&index.recipe_any);
            if let Some(recipe) = context.event.get("recipe").and_then(Value::as_str)
                && let Some(criteria) = index.recipes.get(recipe)
            {
                selected.extend(criteria);
            }
            selected.sort_unstable();
        } else if index.compiled && context.trigger == "minecraft:location" {
            index.location.select(context, selected);
        } else if let Some(criteria) = self.trigger_index.get(context.trigger) {
            selected.extend(0..criteria.len());
        }
    }

    /// Whether a location candidate also needs fresh non-location player facts.
    #[must_use]
    pub fn location_needs_full_context(&self, index: usize) -> bool {
        !self.candidate_index.compiled || self.candidate_index.location.needs_full_context(index)
    }

    #[must_use]
    pub fn support_report(&self) -> SupportReport {
        let mut report = SupportReport::default();
        for (id, definition) in &self.definitions {
            if let Some(function) = &definition.rewards.function {
                report
                    .unsupported_functions
                    .insert(id.clone(), function.clone());
            }
            for criterion in definition.criteria.values() {
                *report
                    .trigger_counts
                    .entry(criterion.trigger.clone())
                    .or_default() += 1;
                let evaluation = evaluate_criterion(
                    criterion,
                    &EventContext {
                        trigger: &criterion.trigger,
                        event: &Value::Null,
                        inventory: &[],
                    },
                );
                if evaluation.unsupported.is_empty() {
                    report.supported_criteria += 1;
                } else {
                    report.unsupported_criteria += 1;
                    for field in evaluation.unsupported {
                        *report.unsupported_fields.entry(field).or_default() += 1;
                    }
                }
            }
        }
        report
    }
}

fn validate_definition(
    id: &str,
    definition: &mut AdvancementDefinition,
) -> Result<(), AdvancementDataError> {
    if definition.criteria.is_empty() {
        return Err(invalid(id, "empty criteria"));
    }
    if let Some(parent) = &mut definition.parent {
        *parent = canonical_id(parent, id)?;
    }
    for (name, criterion) in &mut definition.criteria {
        if name.is_empty() {
            return Err(invalid(id, "empty criterion name"));
        }
        criterion.trigger = canonical_id(&criterion.trigger, id)?;
        if !criterion.conditions.is_null() && !criterion.conditions.is_object() {
            return Err(invalid(id, "criterion conditions must be an object"));
        }
    }
    // Vanilla's omitted requirements default to one group per criterion.
    if definition.requirements.is_empty() {
        definition.requirements = definition
            .criteria
            .keys()
            .map(|name| vec![name.clone()])
            .collect();
    }
    let mut referenced = BTreeSet::new();
    for group in &definition.requirements {
        if group.is_empty() {
            return Err(invalid(id, "empty requirement group"));
        }
        for name in group {
            if !definition.criteria.contains_key(name) {
                return Err(invalid(id, format!("unknown requirement {name}")));
            }
            referenced.insert(name);
        }
    }
    if referenced.len() != definition.criteria.len() {
        return Err(invalid(id, "criteria absent from requirements"));
    }
    for reward in definition
        .rewards
        .loot
        .iter_mut()
        .chain(&mut definition.rewards.recipes)
        .chain(definition.rewards.function.iter_mut())
    {
        *reward = canonical_id(reward, id)?;
    }
    Ok(())
}

fn canonical_id(value: &str, owner: &str) -> Result<String, AdvancementDataError> {
    crate::Identifier::parse(value)
        .map(|id| id.as_str().to_string())
        .map_err(|_| invalid(owner, format!("invalid identifier {value:?}")))
}

fn invalid(id: &str, reason: impl Into<String>) -> AdvancementDataError {
    AdvancementDataError::Invalid {
        id: id.to_string(),
        reason: reason.into(),
    }
}

fn io(path: PathBuf, source: std::io::Error) -> AdvancementDataError {
    AdvancementDataError::Io { path, source }
}

#[cfg(test)]
mod tests;
