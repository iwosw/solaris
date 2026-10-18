//! A deliberately bounded predicate interpreter, not a loot-table executor.
use std::collections::BTreeSet;

use serde_json::Value;

use super::Criterion;

/// Authoritative snapshots only: never supply client assertions as event facts.
///
/// `event` contains named trigger fields and `player`. Item snapshots are
/// `{id,count,components,tags:["minecraft:..."]}`; entity snapshots contain
/// `{type,components,flags,equipment,location,vehicle,passenger,distance}`.
/// Locations contain `{dimension,position:{x,y,z},biomes:[id],structures:[id],
/// block:{id,tags,state},smokey,light}`. Damage contains `{dealt,taken,blocked,
/// type:{tags:[id],direct_entity,source_entity}}`. Tags must be resolved by the
/// caller; missing tag evidence (including for a negative tag test) fails.
/// Effect maps contain effect IDs mapped to `{amplifier,duration,...}`.
/// Distances are already measured by the caller, with x/y/z/horizontal/absolute
/// fields. Missing constrained facts fail; unknown predicate fields are reported
/// and prevent granting even inside an OR or inverted condition.
#[derive(Debug, Clone, Copy)]
pub struct EventContext<'a> {
    pub trigger: &'a str,
    pub event: &'a Value,
    pub inventory: &'a [Value],
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Evaluation {
    pub matches: bool,
    pub unsupported: BTreeSet<String>,
}

#[must_use]
pub fn evaluate_criterion(criterion: &Criterion, context: &EventContext<'_>) -> Evaluation {
    let mut evaluator = Evaluator {
        context,
        unsupported: BTreeSet::new(),
        missing: 0,
    };
    let supported = trigger_fields(&criterion.trigger);
    let mut matches = supported.is_some() && criterion.trigger != "minecraft:impossible";
    if supported.is_none() {
        evaluator
            .unsupported
            .insert(format!("{}.trigger", criterion.trigger));
    }
    if let Some(fields) = criterion.conditions.as_object() {
        for (field, predicate) in fields {
            let path = format!("{}.{}", criterion.trigger, field);
            if field != "player"
                && !supported.is_some_and(|fields| fields.contains(&field.as_str()))
            {
                evaluator.unsupported.insert(path);
                matches = false;
                continue;
            }
            let actual = if field == "recipe_id" {
                context
                    .event
                    .get(field)
                    .or_else(|| context.event.get("recipe"))
                    .unwrap_or(&Value::Null)
            } else {
                get(context.event, field)
            };
            let result = match field.as_str() {
                "player" | "entity" | "parent" | "partner" | "child" | "source" | "bystander"
                | "lightning" | "projectile" | "cause" | "zombie" | "villager" => {
                    evaluator.entity_conditions(predicate, actual, &path)
                }
                "item" | "rod" | "fired_from_weapon" => {
                    evaluator.predicate(Kind::Item, predicate, actual, &path)
                }
                "location" => evaluator.conditions(predicate, &path),
                "start_position" => evaluator.predicate(Kind::Location, predicate, actual, &path),
                "damage" => evaluator.predicate(Kind::Damage, predicate, actual, &path),
                "killing_blow" => evaluator.predicate(Kind::DamageSource, predicate, actual, &path),
                "distance" => evaluator.predicate(Kind::Distance, predicate, actual, &path),
                "effects" => evaluator.effects(predicate, actual, &path),
                "slots" => evaluator.slots(predicate, actual, &path),
                "items" if criterion.trigger == "minecraft:inventory_changed" => {
                    evaluator.inventory(predicate, &path)
                }
                "ingredients" => evaluator.ingredients(predicate, actual, &path),
                "victims" => evaluator.victims(predicate, actual, &path),
                "level"
                | "levels"
                | "count"
                | "duration"
                | "signal_strength"
                | "num_bees_inside"
                | "unique_entity_types"
                | "durability"
                | "delta" => evaluator.range(predicate, actual, &path),
                "block" | "recipe" | "recipe_id" | "loot_table" | "potion" | "from" | "to" => {
                    evaluator.equal(predicate, actual)
                }
                _ => {
                    evaluator.unsupported.insert(path);
                    false
                }
            };
            matches &= result;
        }
    } else if !criterion.conditions.is_null() {
        evaluator
            .unsupported
            .insert(format!("{}.conditions", criterion.trigger));
        matches = false;
    }
    Evaluation {
        matches: matches
            && criterion.trigger == context.trigger
            && evaluator.unsupported.is_empty(),
        unsupported: evaluator.unsupported,
    }
}

fn trigger_fields(trigger: &str) -> Option<&'static [&'static str]> {
    Some(match trigger.strip_prefix("minecraft:")? {
        "impossible"
        | "tick"
        | "location"
        | "slept_in_bed"
        | "hero_of_the_village"
        | "avoid_vibration"
        | "kill_mob_near_sculk_catalyst"
        | "started_riding" => &[],
        "inventory_changed" => &["items", "slots"],
        "consume_item" | "filled_bucket" | "used_totem" | "shot_crossbow" | "using_item" => {
            &["item"]
        }
        "recipe_unlocked" => &["recipe"],
        "recipe_crafted" | "crafter_recipe_crafted" => &["recipe_id", "ingredients"],
        "changed_dimension" => &["from", "to"],
        "enter_block" | "slide_down_block" => &["block", "state"],
        "placed_block" | "item_used_on_block" | "allay_drop_item_on_block" => &["location"],
        "player_killed_entity" | "entity_killed_player" => &["entity", "killing_blow"],
        "player_hurt_entity" => &["entity", "damage"],
        "entity_hurt_player" => &["damage"],
        "player_interacted_with_entity"
        | "player_sheared_equipment"
        | "thrown_item_picked_up_by_entity"
        | "thrown_item_picked_up_by_player" => &["entity", "item"],
        "tame_animal" | "summoned_entity" => &["entity"],
        "bred_animals" => &["parent", "partner", "child"],
        "brewed_potion" => &["potion"],
        "construct_beacon" => &["level"],
        "enchanted_item" => &["item", "levels"],
        "bee_nest_destroyed" => &["block", "item", "num_bees_inside"],
        "effects_changed" => &["effects", "source"],
        "fall_from_height" => &["distance", "start_position"],
        "fall_after_explosion" => &["distance", "cause"],
        "levitation" | "nether_travel" | "ride_entity_in_lava" => &["distance", "duration"],
        "fishing_rod_hooked" => &["rod", "item", "entity"],
        "item_durability_changed" => &["item", "durability", "delta"],
        "killed_by_arrow" => &["fired_from_weapon", "unique_entity_types", "victims"],
        "channeled_lightning" => &["victims"],
        "lightning_strike" => &["lightning", "bystander"],
        "player_generates_container_loot" => &["loot_table"],
        "target_hit" => &["projectile", "signal_strength"],
        "spear_mobs" => &["count"],
        "villager_trade" => &["item", "villager"],
        "cured_zombie_villager" => &["zombie", "villager"],
        _ => return None,
    })
}

#[derive(Clone, Copy)]
enum Kind {
    Item,
    Entity,
    Location,
    Block,
    Damage,
    DamageSource,
    Distance,
    Flags,
    Equipment,
    Slots,
    Effect,
}

struct Evaluator<'a, 'b> {
    context: &'a EventContext<'b>,
    unsupported: BTreeSet<String>,
    missing: usize,
}

impl Evaluator<'_, '_> {
    fn equal(&mut self, expected: &Value, actual: &Value) -> bool {
        if actual.is_null() {
            self.missing += 1;
            return false;
        }
        expected == actual
    }

    fn predicate(&mut self, kind: Kind, predicate: &Value, actual: &Value, path: &str) -> bool {
        let Some(fields) = predicate.as_object() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let empty_item = matches!(kind, Kind::Item)
            && get(actual, "count")
                .as_i64()
                .is_some_and(|count| count <= 0);
        let mut matched = !actual.is_null() && !empty_item;
        if actual.is_null() {
            self.missing += 1;
        }
        for (field, value) in fields {
            let child = format!("{path}.{field}");
            let fact = get(actual, field);
            let result = match (kind, field.as_str()) {
                (Kind::Item, "items") => {
                    !empty_item && self.identifiers(value, get(actual, "id"), get(actual, "tags"))
                }
                (Kind::Item, "count") => self.range(value, fact, &child),
                (Kind::Item | Kind::Entity, "components") => self.components(value, fact, &child),
                (Kind::Item, "predicates") => {
                    self.component_predicates(value, get(actual, "components"), &child)
                }
                (Kind::Entity, "type") => self.identifiers(value, fact, get(actual, "tags")),
                (Kind::Entity, "location" | "stepping_on") => {
                    self.predicate(Kind::Location, value, fact, &child)
                }
                (Kind::Entity, "distance") => self.predicate(Kind::Distance, value, fact, &child),
                (Kind::Entity, "flags") => self.predicate(Kind::Flags, value, fact, &child),
                (Kind::Entity, "equipment") => self.predicate(Kind::Equipment, value, fact, &child),
                (Kind::Entity, "passenger") if fact.is_array() => fact
                    .as_array()
                    .expect("array checked")
                    .iter()
                    .any(|passenger| self.predicate(Kind::Entity, value, passenger, &child)),
                (Kind::Entity, "vehicle" | "passenger" | "targeted_entity") => {
                    self.predicate(Kind::Entity, value, fact, &child)
                }
                (Kind::Entity, "effects") => self.effects(value, fact, &child),
                (Kind::Location, "dimension") => self.equal(value, fact),
                (Kind::Location, "biomes" | "structures") => {
                    self.membership(value, fact, get(actual, &format!("{field}_tags")))
                }
                (Kind::Location, "position") => self.predicate(Kind::Distance, value, fact, &child),
                (Kind::Location, "block") => self.predicate(Kind::Block, value, fact, &child),
                (Kind::Location, "smokey" | "can_see_sky") => self.equal(value, fact),
                (Kind::Location, "light") => self.range(value, fact, &child),
                (Kind::Block, "blocks") => {
                    self.identifiers(value, get(actual, "id"), get(actual, "tags"))
                }
                (Kind::Block, "state") => self.state(value, fact, &child),
                (Kind::Damage, "dealt" | "taken") => self.range(value, fact, &child),
                (Kind::Damage, "blocked") => self.equal(value, fact),
                (Kind::Damage, "type") => self.predicate(Kind::DamageSource, value, fact, &child),
                (Kind::Damage, "source_entity")
                | (Kind::DamageSource, "direct_entity" | "source_entity") => {
                    self.predicate(Kind::Entity, value, fact, &child)
                }
                (Kind::DamageSource, "tags") => self.damage_tags(value, fact, &child),
                (Kind::Distance, "x" | "y" | "z" | "horizontal" | "absolute")
                | (Kind::Slots, "empty" | "occupied" | "full")
                | (Kind::Effect, "amplifier" | "duration") => self.range(value, fact, &child),
                (Kind::Effect, "ambient" | "visible")
                | (
                    Kind::Flags,
                    "is_on_ground" | "is_on_fire" | "is_sneaking" | "is_sprinting" | "is_swimming"
                    | "is_baby" | "is_flying",
                ) => self.equal(value, fact),
                (
                    Kind::Equipment,
                    "head" | "chest" | "legs" | "feet" | "body" | "mainhand" | "offhand" | "saddle",
                ) => self.predicate(Kind::Item, value, fact, &child),
                _ => {
                    self.unsupported.insert(child);
                    false
                }
            };
            matched &= result;
        }
        matched
    }

    fn range(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        let number = actual.as_f64();
        if number.is_none() {
            self.missing += 1;
        }
        if let Some(exact) = predicate.as_f64() {
            return number == Some(exact);
        }
        let Some(bounds) = predicate.as_object() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = number.is_some();
        for (bound, value) in bounds {
            let valid = match (bound.as_str(), value.as_f64(), number) {
                ("min", Some(min), Some(number)) => number >= min,
                ("max", Some(max), Some(number)) => number <= max,
                ("min" | "max", Some(_), None) => false,
                _ => {
                    self.unsupported.insert(format!("{path}.{bound}"));
                    false
                }
            };
            matches &= valid;
        }
        matches
    }

    fn identifiers(&mut self, predicate: &Value, id: &Value, tags: &Value) -> bool {
        if id.is_null() {
            self.missing += 1;
            return false;
        }
        match predicate {
            Value::String(value) if value.starts_with('#') => {
                if !tags.is_array() {
                    self.missing += 1;
                    return false;
                }
                tags.as_array()
                    .is_some_and(|tags| tags.iter().any(|tag| tag.as_str() == Some(&value[1..])))
            }
            Value::String(_) => predicate == id,
            Value::Array(values) => values.iter().any(|value| value == id),
            _ => false,
        }
    }

    fn membership(&mut self, predicate: &Value, actual: &Value, tags: &Value) -> bool {
        if let Some(values) = actual.as_array() {
            let mut result = false;
            for value in values {
                result |= self.identifiers(predicate, value, tags);
            }
            result
        } else {
            self.identifiers(predicate, actual, tags)
        }
    }

    fn components(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        let Some(fields) = predicate.as_object() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = true;
        for (component, expected) in fields {
            matches &= self.equal(expected, get(actual, component));
        }
        matches
    }

    fn component_predicates(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        let Some(fields) = predicate.as_object() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = true;
        for (name, expected) in fields {
            let child = format!("{path}.{name}");
            let result = match name.as_str() {
                "minecraft:enchantments" | "minecraft:stored_enchantments" => {
                    self.enchantments(expected, get(actual, name), &child)
                }
                "minecraft:jukebox_playable" => {
                    let component = get(actual, name);
                    if let Some(fields) = expected.as_object() {
                        let mut matches = component.get("song").is_some_and(Value::is_string);
                        if !matches {
                            self.missing += 1;
                        }
                        for (field, value) in fields {
                            if field == "song" {
                                matches &= self.identifiers(
                                    value,
                                    get(component, "song"),
                                    get(component, "tags"),
                                );
                            } else {
                                self.unsupported.insert(format!("{child}.{field}"));
                                matches = false;
                            }
                        }
                        matches
                    } else {
                        self.unsupported.insert(child);
                        false
                    }
                }
                _ => {
                    self.unsupported.insert(child);
                    false
                }
            };
            matches &= result;
        }
        matches
    }

    fn slots(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        if !actual.is_null() {
            return self.predicate(Kind::Slots, predicate, actual, path);
        }
        let Some(fields) = predicate.as_object() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let occupied = self
            .context
            .inventory
            .iter()
            .filter(|stack| {
                stack
                    .get("count")
                    .and_then(Value::as_u64)
                    .is_some_and(|count| count > 0)
            })
            .count();
        let mut matches = true;
        for (field, bounds) in fields {
            let child = format!("{path}.{field}");
            matches &= match field.as_str() {
                "occupied" => self.range(bounds, &Value::from(occupied), &child),
                // Compact inventory snapshots cannot establish total slots or
                // item-specific stack capacities. Require explicit slot facts.
                "empty" | "full" => self.range(bounds, &Value::Null, &child),
                _ => {
                    self.unsupported.insert(child);
                    false
                }
            };
        }
        matches
    }

    fn enchantments(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        let Some(entries) = predicate.as_array() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = true;
        for entry in entries {
            let Some(fields) = entry.as_object() else {
                self.unsupported.insert(path.to_string());
                return false;
            };
            for field in fields.keys() {
                if field != "enchantments" && field != "levels" {
                    self.unsupported.insert(format!("{path}.{field}"));
                }
            }
            let selector = get(entry, "enchantments");
            let levels = get(entry, "levels");
            // Collect unsupported bounds even when no enchantment is present.
            if !levels.is_null() {
                let missing = self.missing;
                let _ = self.range(levels, &Value::Null, &format!("{path}.levels"));
                self.missing = missing;
            }
            // Schema inspection is not missing runtime evidence.
            // Actual missing bounds/components below still fence inversion.
            let mut found = false;
            if let Some(enchants) = actual.as_object() {
                for (id, level) in enchants {
                    if selector.is_null()
                        || self.identifiers(selector, &Value::String(id.clone()), &Value::Null)
                    {
                        found |= levels.is_null()
                            || self.range(levels, level, &format!("{path}.levels"));
                    }
                }
            } else {
                self.missing += 1;
            }
            matches &= found;
        }
        matches
    }

    fn effects(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        let Some(fields) = predicate.as_object() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = true;
        for (effect, expected) in fields {
            matches &= self.predicate(
                Kind::Effect,
                expected,
                get(actual, effect),
                &format!("{path}.{effect}"),
            );
        }
        matches
    }

    fn state(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        let Some(fields) = predicate.as_object() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = true;
        for (property, value) in fields {
            if value.is_object() {
                self.unsupported.insert(format!("{path}.{property}.range"));
                matches = false;
            } else {
                matches &= self.equal(value, get(actual, property));
            }
        }
        matches
    }

    fn damage_tags(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        let Some(entries) = predicate.as_array() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = true;
        if !actual.is_array() {
            self.missing += 1;
            matches = false;
        }
        for entry in entries {
            let Some(fields) = entry.as_object() else {
                self.unsupported.insert(path.to_string());
                return false;
            };
            for key in fields.keys() {
                if key != "id" && key != "expected" {
                    self.unsupported.insert(format!("{path}.{key}"));
                }
            }
            let found = actual
                .as_array()
                .is_some_and(|tags| tags.contains(get(entry, "id")));
            matches &= get(entry, "id").is_string()
                && get(entry, "expected")
                    .as_bool()
                    .is_some_and(|expected| found == expected);
        }
        matches
    }

    fn entity_conditions(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        if let Some(conditions) = predicate.as_array() {
            let mut matches = true;
            for condition in conditions {
                matches &= self.condition(condition, Some(actual), path);
            }
            matches
        } else {
            self.predicate(Kind::Entity, predicate, actual, path)
        }
    }

    fn conditions(&mut self, predicate: &Value, path: &str) -> bool {
        let Some(conditions) = predicate.as_array() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = true;
        for condition in conditions {
            matches &= self.condition(condition, None, path);
        }
        matches
    }

    fn condition(&mut self, condition: &Value, entity: Option<&Value>, path: &str) -> bool {
        let Some(fields) = condition.as_object() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let name = get(condition, "condition").as_str().unwrap_or("");
        let child = format!("{path}[{name}]");
        let allowed: &[&str] = match name {
            "minecraft:entity_properties" => &["condition", "entity", "predicate"],
            "minecraft:match_tool" | "minecraft:location_check" => &["condition", "predicate"],
            "minecraft:block_state_property" => &["condition", "block", "properties"],
            "minecraft:inverted" => &["condition", "term"],
            "minecraft:all_of" | "minecraft:any_of" => &["condition", "terms"],
            _ => {
                self.unsupported.insert(child);
                return false;
            }
        };
        for field in fields.keys() {
            if !allowed.contains(&field.as_str()) {
                self.unsupported.insert(format!("{child}.{field}"));
            }
        }
        match name {
            "minecraft:entity_properties" => {
                let actual = match get(condition, "entity").as_str() {
                    Some("this") => entity.unwrap_or_else(|| get(self.context.event, "player")),
                    Some("attacker") => get(self.context.event, "attacker"),
                    Some("direct_attacker") => get(self.context.event, "direct_attacker"),
                    Some("attacking_player") => get(self.context.event, "player"),
                    _ => {
                        self.unsupported.insert(format!("{child}.entity"));
                        &Value::Null
                    }
                };
                self.predicate(
                    Kind::Entity,
                    get(condition, "predicate"),
                    actual,
                    &format!("{child}.predicate"),
                )
            }
            "minecraft:match_tool" => self.predicate(
                Kind::Item,
                get(condition, "predicate"),
                get(self.context.event, "item"),
                &format!("{child}.predicate"),
            ),
            "minecraft:location_check" => self.predicate(
                Kind::Location,
                get(condition, "predicate"),
                get(self.context.event, "location"),
                &format!("{child}.predicate"),
            ),
            "minecraft:block_state_property" => {
                let block = get(get(self.context.event, "location"), "block");
                let id = self.equal(get(condition, "block"), get(block, "id"));
                let state = if fields.contains_key("properties") {
                    self.state(
                        get(condition, "properties"),
                        get(block, "state"),
                        &format!("{child}.properties"),
                    )
                } else {
                    true
                };
                id && state
            }
            "minecraft:inverted" => {
                let before = self.missing;
                let matched =
                    self.condition(get(condition, "term"), entity, &format!("{child}.term"));
                !matched && before == self.missing
            }
            "minecraft:all_of" | "minecraft:any_of" => {
                let Some(terms) = get(condition, "terms").as_array() else {
                    self.unsupported.insert(format!("{child}.terms"));
                    return false;
                };
                let mut result = name == "minecraft:all_of";
                for term in terms {
                    let matched = self.condition(term, entity, &format!("{child}.terms"));
                    if name == "minecraft:all_of" {
                        result &= matched;
                    } else {
                        result |= matched;
                    }
                }
                result
            }
            _ => false,
        }
    }

    fn inventory(&mut self, predicate: &Value, path: &str) -> bool {
        let Some(predicates) = predicate.as_array() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = true;
        for predicate in predicates {
            // Analyze all fields even if the inventory is empty.
            let _ = self.predicate(Kind::Item, predicate, &Value::Null, path);
            let mut found = false;
            for stack in self.context.inventory {
                let accepted = self.predicate(Kind::Item, predicate, stack, path);
                found |= stack
                    .get("count")
                    .and_then(Value::as_u64)
                    .is_some_and(|count| count > 0)
                    && accepted;
            }
            matches &= found;
        }
        matches
    }

    fn ingredients(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        let Some(predicates) = predicate.as_array() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        // Vanilla recipe predicates need distinct ingredient instances. A
        // bounded bipartite search handles overlapping selectors without a
        // greedy false negative (the crafting grid has at most nine entries).
        if predicates.len() > 9 {
            self.unsupported.insert(format!("{path}.more_than_nine"));
            return false;
        }
        let ingredients = actual.as_array().map(Vec::as_slice).unwrap_or(&[]);
        let mut edges = Vec::with_capacity(predicates.len());
        for predicate in predicates {
            let _ = self.predicate(Kind::Item, predicate, &Value::Null, path);
            let mut candidates = Vec::new();
            for (index, ingredient) in ingredients.iter().enumerate() {
                if self.predicate(Kind::Item, predicate, ingredient, path) {
                    candidates.push(index);
                }
            }
            edges.push(candidates);
        }
        assign_ingredients(&edges, 0, &mut BTreeSet::new())
    }

    fn victims(&mut self, predicate: &Value, actual: &Value, path: &str) -> bool {
        let Some(predicates) = predicate.as_array() else {
            self.unsupported.insert(path.to_string());
            return false;
        };
        let mut matches = true;
        for predicate in predicates {
            let _ = self.entity_conditions(predicate, &Value::Null, path);
            let mut found = false;
            if let Some(victims) = actual.as_array() {
                for victim in victims {
                    found |= self.entity_conditions(predicate, victim, path);
                }
            }
            matches &= found;
        }
        matches
    }
}

fn assign_ingredients(edges: &[Vec<usize>], row: usize, used: &mut BTreeSet<usize>) -> bool {
    if row == edges.len() {
        return true;
    }
    for &index in &edges[row] {
        if used.insert(index) {
            if assign_ingredients(edges, row + 1, used) {
                return true;
            }
            used.remove(&index);
        }
    }
    false
}

fn get<'a>(value: &'a Value, field: &str) -> &'a Value {
    value.get(field).unwrap_or(&Value::Null)
}
