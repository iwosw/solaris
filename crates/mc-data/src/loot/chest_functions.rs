use super::enchantments::{self, Candidate, Resources};
use super::numbers::Number;
use super::{ChestLootContext, ChestRng, Identifier, ItemRegistry, ItemStack, parse_id};
use mc_nbt::{ListTag, Tag};
use serde_json::Value;

/// Request to the native saved-map owner. The owner supplies the origin and
/// dimension, resolves the structure through locate truth, and allocates map_id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExplorationMap {
    pub destination: Identifier,
    pub decoration: Identifier,
    pub zoom: u8,
    pub search_radius: u32,
    pub skip_existing_chunks: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Condition {
    Chance(Number),
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}
pub(super) fn conditions(value: Option<&Value>) -> Result<Vec<Condition>, String> {
    value
        .map(|v| {
            v.as_array()
                .ok_or_else(|| "conditions must be an array".to_owned())?
                .iter()
                .map(|v| condition(v, 0))
                .collect()
        })
        .unwrap_or_else(|| Ok(Vec::new()))
}
fn condition(value: &Value, depth: usize) -> Result<Condition, String> {
    if depth > 16 {
        return Err("condition nesting exceeds 16".into());
    }
    let name = value["condition"]
        .as_str()
        .ok_or("condition missing identifier")?;
    let result = match name {
        "minecraft:random_chance" => {
            Number::compile(&value["chance"], 0.0, 1.0).map(Condition::Chance)
        }
        "minecraft:all_of" | "minecraft:any_of" => {
            let terms = value["terms"]
                .as_array()
                .ok_or("condition terms must be an array")?;
            let terms = terms
                .iter()
                .map(|v| condition(v, depth + 1))
                .collect::<Result<_, _>>()?;
            Ok(if name == "minecraft:all_of" {
                Condition::All(terms)
            } else {
                Condition::Any(terms)
            })
        }
        "minecraft:inverted" => {
            condition(&value["term"], depth + 1).map(|c| Condition::Not(Box::new(c)))
        }
        other => Err(format!(
            "unsupported condition {other} (not a container-loot context)"
        )),
    };
    result.map_err(|e| format!("{name}: {e}"))
}
impl Condition {
    fn test(&self, rng: &mut ChestRng) -> bool {
        match self {
            Self::Chance(chance) => {
                let chance = chance.float(rng);
                rng.fraction() < chance
            }
            Self::All(terms) => test_conditions(terms, rng),
            Self::Any(terms) => terms.iter().any(|c| c.test(rng)),
            Self::Not(term) => !term.test(rng),
        }
    }
}
pub(super) fn test_conditions(conditions: &[Condition], rng: &mut ChestRng) -> bool {
    conditions.iter().all(|c| c.test(rng))
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChestFunction {
    name: String,
    conditions: Vec<Condition>,
    action: Action,
}
#[derive(Debug, Clone, PartialEq)]
enum Action {
    Count {
        count: Number,
        add: bool,
    },
    Limit {
        min: Option<Number>,
        max: Option<Number>,
    },
    Damage {
        damage: Number,
        add: bool,
        maximum: u32,
    },
    RandomEnchant(Vec<Candidate>),
    LevelEnchant {
        candidates: Vec<Candidate>,
        levels: Number,
        enchantability: u32,
    },
    Enchantments {
        values: Vec<(Identifier, Number)>,
        add: bool,
    },
    Potion(Identifier),
    Stew(Vec<(Identifier, Number)>),
    Amplifier(Number),
    Instrument(Vec<(String, Tag)>),
    Map(ExplorationMap),
    Components(Vec<(String, Tag)>),
    Lore {
        lines: Tag,
        append: bool,
    },
}

pub(super) fn compile(
    value: Option<&Value>,
    item: Option<&Identifier>,
    resources: &Resources,
) -> Result<Vec<ChestFunction>, String> {
    let Some(value) = value else {
        return Ok(Vec::new());
    };
    let values = value.as_array().ok_or("functions must be an array")?;
    if values.len() > 64 {
        return Err("more than 64 functions".into());
    }
    values
        .iter()
        .map(|value| {
            let name = value["function"]
                .as_str()
                .ok_or("function missing identifier")?;
            let conditions =
                conditions(value.get("conditions")).map_err(|e| format!("{name}: {e}"))?;
            let action =
                compile_action(name, value, item, resources).map_err(|e| format!("{name}: {e}"))?;
            Ok(ChestFunction {
                name: name.into(),
                conditions,
                action,
            })
        })
        .collect()
}
fn boolean(value: &Value, key: &str, default: bool) -> Result<bool, String> {
    value
        .get(key)
        .map_or(Some(default), Value::as_bool)
        .ok_or_else(|| format!("{key} must be boolean"))
}
fn compile_action(
    name: &str,
    value: &Value,
    item: Option<&Identifier>,
    resources: &Resources,
) -> Result<Action, String> {
    let require_item =
        || item.ok_or_else(|| "function needs an item entry, not an untyped pool/table".to_owned());
    match name {
        "minecraft:set_count" => Ok(Action::Count {
            count: Number::compile(&value["count"], -64.0, 64.0)?,
            add: boolean(value, "add", false)?,
        }),
        "minecraft:limit_count" => {
            let limit = &value["limit"];
            if limit.is_number() {
                let n = Number::compile(limit, 0.0, 64.0)?;
                return Ok(Action::Limit {
                    min: Some(n.clone()),
                    max: Some(n),
                });
            }
            let min = limit
                .get("min")
                .map(|v| Number::compile(v, 0.0, 64.0))
                .transpose()?;
            let max = limit
                .get("max")
                .map(|v| Number::compile(v, 0.0, 64.0))
                .transpose()?;
            if min.is_none() && max.is_none() {
                return Err("limit requires min or max".into());
            }
            if min
                .as_ref()
                .zip(max.as_ref())
                .is_some_and(|(a, b)| a.max() > b.min())
            {
                return Err("unordered limit".into());
            }
            Ok(Action::Limit { min, max })
        }
        "minecraft:set_damage" => {
            static FACTS: std::sync::LazyLock<crate::item_components::ItemFactsTable> =
                std::sync::LazyLock::new(crate::item_components::solaris_required_item_facts);
            let maximum = FACTS
                .get(require_item()?)
                .and_then(|f| f.max_damage)
                .unwrap_or(0);
            Ok(Action::Damage {
                damage: Number::compile(&value["damage"], -1.0, 1.0)?,
                add: boolean(value, "add", false)?,
                maximum,
            })
        }
        "minecraft:enchant_randomly" => Ok(Action::RandomEnchant(resources.candidates(
            require_item()?,
            value.get("options"),
            false,
            boolean(value, "only_compatible", true)?,
        )?)),
        "minecraft:enchant_with_levels" => {
            let item = require_item()?;
            Ok(Action::LevelEnchant {
                candidates: resources.candidates(item, value.get("options"), true, true)?,
                levels: Number::compile(&value["levels"], 0.0, 255.0)?,
                enchantability: enchantments::enchantability(item),
            })
        }
        "minecraft:set_enchantments" => {
            let values = value["enchantments"]
                .as_object()
                .ok_or("enchantments must be an object")?
                .iter()
                .map(|(name, value)| {
                    let id = Identifier::parse(name).map_err(|e| e.to_string())?;
                    if crate::required_registry_entry_id("enchantment", &id).is_none() {
                        return Err(format!("unknown enchantment {id}"));
                    }
                    Ok((id, Number::compile(value, 0.0, 255.0)?))
                })
                .collect::<Result<_, String>>()?;
            Ok(Action::Enchantments {
                values,
                add: boolean(value, "add", false)?,
            })
        }
        "minecraft:set_potion" => {
            let id = parse_id(&value["id"])?;
            if crate::stations::potion_id(&id).is_none() {
                return Err(format!("unknown potion {id}"));
            }
            Ok(Action::Potion(id))
        }
        "minecraft:set_stew_effect" => {
            let values = value["effects"]
                .as_array()
                .ok_or("effects must be an array")?;
            if values.is_empty() {
                return Err("effects must not be empty".into());
            }
            let effects = values
                .iter()
                .map(|effect| {
                    let id = parse_id(&effect["type"])?;
                    if crate::mob_effects_26_1_2::MobEffect::from_name(id.as_str()).is_none() {
                        return Err(format!("unknown stew effect {id}"));
                    }
                    Ok((id, Number::compile(&effect["duration"], 0.0, 32767.0)?))
                })
                .collect::<Result<_, String>>()?;
            Ok(Action::Stew(effects))
        }
        "minecraft:set_ominous_bottle_amplifier" => Ok(Action::Amplifier(Number::compile(
            &value["amplifier"],
            0.0,
            4.0,
        )?)),
        "minecraft:set_instrument" => {
            let names = resources.options(&value["options"], "instrument")?;
            if names.is_empty() {
                return Err("instrument options are empty".into());
            }
            let options = names
                .into_iter()
                .map(|id| {
                    if crate::required_registry_entry_id("instrument", &id).is_none() {
                        return Err(format!("unknown instrument {id}"));
                    }
                    Ok((
                        "minecraft:instrument".into(),
                        Tag::String(id.as_str().into()),
                    ))
                })
                .collect::<Result<_, String>>()?;
            Ok(Action::Instrument(options))
        }
        "minecraft:exploration_map" => {
            let destination = value
                .get("destination")
                .map(parse_id)
                .transpose()?
                .unwrap_or_else(|| {
                    Identifier::parse("minecraft:on_treasure_maps").expect("static id")
                });
            let decoration = value
                .get("decoration")
                .map(parse_id)
                .transpose()?
                .unwrap_or_else(|| Identifier::parse("minecraft:mansion").expect("static id"));
            let zoom = value
                .get("zoom")
                .map_or(Some(2), Value::as_u64)
                .filter(|v| *v <= 4)
                .ok_or("zoom must be 0..=4")? as u8;
            let search_radius = value
                .get("search_radius")
                .map_or(Some(50), Value::as_u64)
                .filter(|v| *v <= 1024)
                .ok_or("search_radius must be 0..=1024")? as u32;
            Ok(Action::Map(ExplorationMap {
                destination,
                decoration,
                zoom,
                search_radius,
                skip_existing_chunks: boolean(value, "skip_existing_chunks", true)?,
            }))
        }
        "minecraft:set_name" => {
            if value.get("entity").is_some() {
                return Err("entity-resolved text needs an entity loot context".into());
            }
            let key = match value
                .get("target")
                .and_then(Value::as_str)
                .unwrap_or("custom_name")
            {
                "custom_name" => "minecraft:custom_name",
                "item_name" => "minecraft:item_name",
                other => return Err(format!("unsupported name target {other}")),
            };
            let fields = vec![(key.into(), json_tag(&value["name"])?)];
            validate_patch(&fields, item)?;
            Ok(Action::Components(fields))
        }
        "minecraft:set_lore" => {
            if value.get("entity").is_some() {
                return Err("entity-resolved lore needs an entity loot context".into());
            }
            let append = match value
                .get("mode")
                .and_then(Value::as_str)
                .unwrap_or("append")
            {
                "append" => true,
                "replace_all" => false,
                other => return Err(format!("unsupported lore mode {other}")),
            };
            let lines = json_tag(&value["lore"])?;
            let fields = vec![("minecraft:lore".into(), lines.clone())];
            validate_patch(&fields, item)?;
            Ok(Action::Lore { lines, append })
        }
        "minecraft:set_components" => {
            let fields = value["components"]
                .as_object()
                .ok_or("components must be an object")?
                .iter()
                .map(|(k, v)| Ok((k.clone(), json_tag(v)?)))
                .collect::<Result<Vec<_>, String>>()?;
            validate_patch(&fields, item)?;
            Ok(Action::Components(fields))
        }
        other => Err(format!("unsupported function {other}")),
    }
}
fn validate_patch(fields: &[(String, Tag)], item: Option<&Identifier>) -> Result<(), String> {
    static ITEMS: std::sync::LazyLock<ItemRegistry> =
        std::sync::LazyLock::new(crate::items::solaris_required_items);
    let fallback = Identifier::parse("minecraft:stone").expect("static id");
    crate::item_nbt::stack_from_component_patch(&ITEMS, item.unwrap_or(&fallback), fields)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

impl ChestFunction {
    pub(super) fn exploration_map(&self) -> Option<&ExplorationMap> {
        match &self.action {
            Action::Map(request) => Some(request),
            _ => None,
        }
    }
    pub(super) fn apply(
        &self,
        stack: &mut ItemStack,
        rng: &mut ChestRng,
        items: &ItemRegistry,
        context: &mut ChestLootContext<'_>,
    ) -> Result<bool, String> {
        if !test_conditions(&self.conditions, rng) {
            return Ok(true);
        }
        self.apply_action(stack, rng, items, context)
            .map_err(|e| format!("{}: {e}", self.name))
    }
    fn apply_action(
        &self,
        stack: &mut ItemStack,
        rng: &mut ChestRng,
        items: &ItemRegistry,
        context: &mut ChestLootContext<'_>,
    ) -> Result<bool, String> {
        match &self.action {
            Action::Count { count, add } => {
                stack.count = (count.int(rng) + if *add { stack.count } else { 0 }).clamp(0, 64);
            }
            Action::Limit { min, max } => {
                if let Some(min) = min {
                    stack.count = stack.count.max(min.int(rng));
                }
                if let Some(max) = max {
                    stack.count = stack.count.min(max.int(rng));
                }
            }
            Action::Damage {
                damage,
                add,
                maximum,
            } => {
                // Vanilla applies this function to compasses in trial barrels:
                // items without max_damage stay unchanged, without sampling RNG.
                if *maximum == 0 {
                    return Ok(true);
                }
                let current = 1.0 - f64::from(stack.damage.unwrap_or(0)) / f64::from(*maximum);
                let remaining =
                    (damage.float(rng) + if *add { current } else { 0.0 }).clamp(0.0, 1.0);
                stack.damage = Some(((1.0 - remaining) * f64::from(*maximum)).floor() as i32);
            }
            Action::RandomEnchant(candidates) => {
                enchantments::random(candidates, stack, items, rng)?
            }
            Action::LevelEnchant {
                candidates,
                levels,
                enchantability,
            } => {
                let level = levels.int(rng);
                enchantments::with_levels(candidates, *enchantability, level, stack, items, rng)?;
            }
            Action::Enchantments { values, add } => {
                for (id, number) in values {
                    enchantments::put(stack, items, id, number.int(rng), *add)?;
                }
            }
            Action::Potion(potion) => {
                stack.extended_mut().potion = Some(potion.clone());
            }
            Action::Stew(effects) => {
                let (id, duration) = &effects[rng.next_bounded(effects.len() as u64) as usize];
                let ticks = duration.int(rng)
                    * if matches!(
                        id.path(),
                        "saturation" | "instant_health" | "instant_damage"
                    ) {
                        1
                    } else {
                        20
                    };
                stack.stew_effects.push(crate::item_stack::StewEffect {
                    id: id.clone(),
                    duration: ticks,
                });
            }
            Action::Amplifier(number) => {
                stack.extended_mut().ominous_bottle_amplifier = Some(number.int(rng));
            }
            Action::Instrument(options) => {
                let choice = &options[rng.next_bounded(options.len() as u64) as usize];
                apply_patch(stack, items, std::slice::from_ref(choice))?;
            }
            Action::Map(request) => {
                let resolver = context
                    .exploration_map
                    .as_mut()
                    .ok_or("world-context saved-map resolver missing")?;
                let Some(map) = resolver(request)? else {
                    return Ok(false);
                };
                *stack = map;
            }
            Action::Components(fields) => apply_patch(stack, items, fields)?,
            Action::Lore { lines, append } => {
                let mut lines = lines.clone();
                if *append {
                    let fields = crate::item_nbt::stack_to_fields(items, stack)
                        .map_err(|e| e.to_string())?;
                    if let Some(Tag::Compound(components)) =
                        crate::item_nbt::field(&fields, "components")
                        && let Some(Tag::List(old)) =
                            crate::item_nbt::field(components, "minecraft:lore")
                        && let Tag::List(new) = &mut lines
                    {
                        let mut combined = old.elements.clone();
                        combined.append(&mut new.elements);
                        new.elements = combined;
                    }
                }
                apply_patch(stack, items, &[("minecraft:lore".into(), lines)])?;
            }
        }
        Ok(true)
    }
}
fn apply_patch(
    stack: &mut ItemStack,
    items: &ItemRegistry,
    patch: &[(String, Tag)],
) -> Result<(), String> {
    let mut fields = crate::item_nbt::stack_to_fields(items, stack).map_err(|e| e.to_string())?;
    if let Some((_, Tag::Compound(components))) = fields.iter_mut().find(|(k, _)| k == "components")
    {
        for (name, value) in patch {
            if let Some((_, old)) = components.iter_mut().find(|(k, _)| k == name) {
                *old = value.clone();
            } else {
                components.push((name.clone(), value.clone()));
            }
        }
    } else {
        fields.push(("components".into(), Tag::Compound(patch.to_vec())));
    }
    *stack = crate::item_nbt::stack_from_fields(items, &fields)
        .map_err(|e| e.to_string())?
        .ok_or("missing item after component patch")?;
    Ok(())
}
fn json_tag(value: &Value) -> Result<Tag, String> {
    Ok(match value {
        Value::String(s) => Tag::String(s.clone()),
        Value::Bool(b) => Tag::Byte(i8::from(*b)),
        Value::Number(n) => {
            if let Some(v) = n.as_i64() {
                Tag::Int(i32::try_from(v).map_err(|_| "component integer overflow")?)
            } else {
                Tag::Double(n.as_f64().ok_or("invalid component number")?)
            }
        }
        Value::Object(fields) => Tag::Compound(
            fields
                .iter()
                .map(|(k, v)| Ok((k.clone(), json_tag(v)?)))
                .collect::<Result<_, String>>()?,
        ),
        Value::Array(values) => {
            let elements = values.iter().map(json_tag).collect::<Result<Vec<_>, _>>()?;
            let element_type = elements.first().map_or(mc_nbt::tag_type::END, Tag::type_id);
            if elements.iter().any(|e| e.type_id() != element_type) {
                return Err("heterogeneous component list is unsupported".into());
            }
            Tag::List(ListTag {
                element_type,
                elements,
            })
        }
        Value::Null => return Err("null component is unsupported".into()),
    })
}
