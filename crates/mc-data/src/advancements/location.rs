//! Conservative startup dispatch for mandatory location facts. Every selected
//! criterion still passes through the ordinary fail-closed predicate evaluator.
use super::{Criterion, EventContext};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct LocationIndex {
    buckets: BTreeMap<&'static str, BTreeMap<String, Vec<usize>>>,
    any: Vec<usize>,
    positions: BTreeMap<usize, Vec<(bool, Value)>>,
    full_context: Vec<bool>,
}

impl LocationIndex {
    pub(super) fn insert(&mut self, index: usize, criterion: &Criterion) {
        let mut locations = Vec::new();
        if let Some(player) = criterion.conditions.get("player") {
            mandatory_locations(player, &mut locations);
        }
        self.full_context
            .push(!location_only(&criterion.conditions));
        for &(stepping, location) in &locations {
            if let Some(position) = location.get("position") {
                self.positions
                    .entry(index)
                    .or_default()
                    .push((stepping, position.clone()));
            }
        }
        for (stepping, location) in locations {
            if stepping {
                if let Some(block) = location.pointer("/block/blocks").and_then(Value::as_str)
                    && !block.starts_with('#')
                {
                    self.buckets
                        .entry("stepping_block")
                        .or_default()
                        .entry(block.to_owned())
                        .or_default()
                        .push(index);
                    return;
                }
                continue;
            }
            for field in ["dimension", "biomes", "structures"] {
                let Some(value) = location.get(field) else {
                    continue;
                };
                let keys: Vec<_> = match value {
                    Value::String(key) if !key.starts_with('#') => vec![key.as_str()],
                    Value::Array(keys) if !keys.is_empty() && keys.iter().all(Value::is_string) => {
                        keys.iter().filter_map(Value::as_str).collect()
                    }
                    _ => continue,
                };
                let bucket = self.buckets.entry(field).or_default();
                for key in keys {
                    bucket.entry(key.to_owned()).or_default().push(index);
                }
                return;
            }
        }
        self.any.push(index);
    }

    pub(super) fn select(&self, context: &EventContext<'_>, selected: &mut Vec<usize>) {
        selected.extend(&self.any);
        let location = current_location(context);
        for (field, bucket) in &self.buckets {
            let actual = if *field == "stepping_block" {
                context.event.pointer("/player/stepping_on/block/id")
            } else {
                location.get(*field)
            };
            match actual {
                Some(Value::String(key)) => selected.extend(bucket.get(key).into_iter().flatten()),
                Some(Value::Array(keys)) => {
                    for key in keys.iter().filter_map(Value::as_str) {
                        selected.extend(bucket.get(key).into_iter().flatten());
                    }
                }
                _ => {}
            }
        }
        selected.sort_unstable();
        selected.dedup();
        selected.retain(|index| {
            self.positions.get(index).is_none_or(|positions| {
                positions.iter().all(|(stepping, expected)| {
                    let actual = if *stepping {
                        context.event.pointer("/player/stepping_on/position")
                    } else {
                        location.get("position")
                    };
                    position_may_match(expected, actual.unwrap_or(&Value::Null))
                })
            })
        });
    }

    pub(super) fn needs_full_context(&self, index: usize) -> bool {
        self.full_context.get(index).copied().unwrap_or(true)
    }
}

fn current_location<'a>(context: &EventContext<'a>) -> &'a Value {
    context
        .event
        .pointer("/player/location")
        .or_else(|| context.event.get("location"))
        .unwrap_or(&Value::Null)
}

fn mandatory_locations<'a>(player: &'a Value, locations: &mut Vec<(bool, &'a Value)>) {
    if let Some(conditions) = player.as_array() {
        for condition in conditions {
            match condition.get("condition").and_then(Value::as_str) {
                Some("minecraft:entity_properties")
                    if condition.get("entity").and_then(Value::as_str) == Some("this") =>
                {
                    mandatory_locations(&condition["predicate"], locations);
                }
                Some("minecraft:all_of") => mandatory_locations(&condition["terms"], locations),
                _ => {} // Inversion/alternatives cannot yield a mandatory discriminator.
            }
        }
    } else if let Some(fields) = player.as_object() {
        for (field, stepping) in [("location", false), ("stepping_on", true)] {
            if let Some(location) = fields.get(field) {
                locations.push((stepping, location));
            }
        }
    }
}

fn location_only(conditions: &Value) -> bool {
    conditions.is_null()
        || conditions.as_object().is_some_and(|fields| {
            fields
                .iter()
                .all(|(key, value)| key == "player" && player_location_only(value))
        })
}

fn player_location_only(player: &Value) -> bool {
    if let Some(conditions) = player.as_array() {
        conditions.iter().all(|condition| {
            match condition.get("condition").and_then(Value::as_str) {
                Some("minecraft:entity_properties") => {
                    condition.get("entity").and_then(Value::as_str) == Some("this")
                        && player_location_only(&condition["predicate"])
                }
                Some("minecraft:location_check") => simple_location(&condition["predicate"]),
                Some("minecraft:all_of" | "minecraft:any_of") => {
                    player_location_only(&condition["terms"])
                }
                Some("minecraft:inverted") => {
                    player_location_only(&Value::Array(vec![condition["term"].clone()]))
                }
                _ => false,
            }
        })
    } else {
        player.as_object().is_some_and(|fields| {
            fields
                .iter()
                .all(|(key, value)| key == "location" && simple_location(value))
        })
    }
}

fn simple_location(location: &Value) -> bool {
    location.as_object().is_some_and(|fields| {
        fields.keys().all(|key| {
            matches!(
                key.as_str(),
                "dimension" | "position" | "biomes" | "structures"
            )
        })
    })
}

fn position_may_match(expected: &Value, actual: &Value) -> bool {
    let Some(axes) = expected.as_object() else {
        return true;
    };
    axes.iter().all(|(axis, expected)| {
        if !matches!(axis.as_str(), "x" | "y" | "z") {
            return true;
        }
        let actual = actual.get(axis).and_then(Value::as_f64);
        if let Some(expected) = expected.as_f64() {
            return actual == Some(expected);
        }
        let Some(bounds) = expected.as_object() else {
            return true;
        };
        bounds.iter().all(|(bound, value)| {
            let Some(value) = value.as_f64() else {
                return true;
            };
            match bound.as_str() {
                "min" => actual.is_some_and(|actual| actual >= value),
                "max" => actual.is_some_and(|actual| actual <= value),
                _ => true,
            }
        })
    })
}
