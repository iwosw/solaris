//! Tick-owned pet ownership, trust and horse temperament.
use crate::boundary::{EntityDataValue, EntityId};
use crate::entities::{ColdRow, EntitySlot};
use crate::tick::TickState;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TamingState {
    pub owner: Option<Uuid>,
    #[serde(deserialize_with = "temper_image")]
    pub temper: u8,
    #[serde(deserialize_with = "collar_image")]
    pub collar: u8,
    pub trusting: bool,
    #[serde(deserialize_with = "trusted_image")]
    pub trusted: Vec<Uuid>,
    pub love_cause: Option<Uuid>,
    pub rolls: u64,
}
impl Default for TamingState {
    fn default() -> Self {
        Self {
            owner: None,
            temper: 0,
            collar: 14,
            trusting: false,
            trusted: Vec::new(),
            love_cause: None,
            rolls: 0,
        }
    }
}
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    Tame(u32),
    Trust,
    Sit,
    Collar(u8),
    HorseFeed {
        heal: f32,
        growth: u32,
        temper: u8,
        breed: bool,
    },
    Heal {
        maximum: f32,
        amount: f32,
    },
}

pub fn horse(name: &str) -> bool {
    matches!(
        name,
        "minecraft:horse"
            | "minecraft:donkey"
            | "minecraft:mule"
            | "minecraft:llama"
            | "minecraft:trader_llama"
    )
}

pub(crate) fn food(state: &TickState, name: &str, item: u32) -> bool {
    static TAGS: std::sync::LazyLock<(mc_data::Identifier, [mc_data::Identifier; 8])> =
        std::sync::LazyLock::new(|| {
            (
                mc_data::Identifier::parse("minecraft:item").expect("static registry"),
                [
                    "wolf", "cat", "parrot", "horse", "llama", "pig", "strider", "camel",
                ]
                .map(|name| {
                    mc_data::Identifier::parse(format!("minecraft:{name}_food"))
                        .expect("static food tag")
                }),
            )
        });
    let index = match name {
        "minecraft:wolf" => 0,
        "minecraft:cat" => 1,
        "minecraft:parrot" => 2,
        "minecraft:horse" | "minecraft:donkey" | "minecraft:mule" => 3,
        "minecraft:llama" | "minecraft:trader_llama" => 4,
        "minecraft:pig" => 5,
        "minecraft:strider" => 6,
        "minecraft:camel" => 7,
        _ => return false,
    };
    let Ok(item) = i32::try_from(item) else {
        return false;
    };
    state
        .registry
        .tags
        .registries
        .get(&TAGS.0)
        .and_then(|tags| tags.get(&TAGS.1[index]))
        .is_some_and(|items| items.contains(&item))
}
pub(crate) fn action(
    state: &TickState,
    actor: Uuid,
    body: &crate::entities::Body,
    row: &ColdRow,
    name: &str,
    held: &str,
    food: bool,
) -> Option<Action> {
    let pet = &row.entity_life.taming;
    if pet.owner.is_none()
        && ((name == "minecraft:wolf" && held == "minecraft:bone")
            || (name == "minecraft:cat" && matches!(held, "minecraft:cod" | "minecraft:salmon"))
            || (name == "minecraft:parrot" && food))
    {
        return Some(Action::Tame(if name == "minecraft:parrot" {
            10
        } else {
            3
        }));
    }
    if name == "minecraft:ocelot"
        && !pet.trusting
        && matches!(held, "minecraft:cod" | "minecraft:salmon")
    {
        return Some(Action::Trust);
    }
    if matches!(name, "minecraft:cat" | "minecraft:wolf")
        && pet.owner == Some(actor)
        && let Some(color) = crate::blueprint::DYE_COLORS.iter().position(|color| {
            held.strip_prefix("minecraft:")
                .and_then(|h| h.strip_suffix("_dye"))
                == Some(*color)
        })
    {
        return (color as u8 != pet.collar).then_some(Action::Collar(color as u8));
    }
    if matches!(name, "minecraft:cat" | "minecraft:wolf") && row.entity_life.tamed && food {
        let maximum = if name == "minecraft:wolf" {
            40.0
        } else {
            max_health(state, body, row)
        };
        if row.health < maximum {
            let food = state
                .registry
                .item_facts
                .get(&mc_data::Identifier::parse(held).ok()?)?
                .food
                .as_ref()?
                .food;
            return Some(Action::Heal {
                maximum,
                amount: food as f32 * if name == "minecraft:cat" { 2.0 } else { 1.0 },
            });
        }
    }
    if matches!(
        name,
        "minecraft:wolf" | "minecraft:cat" | "minecraft:parrot"
    ) && pet.owner == Some(actor)
        && !food
    {
        return Some(Action::Sit);
    }
    if horse(name) {
        let llama = matches!(name, "minecraft:llama" | "minecraft:trader_llama");
        let (heal, growth, temper, breed) = match (llama, held) {
            (true, "minecraft:wheat") => (2.0, 200, 3, false),
            (true, "minecraft:hay_block") => (10.0, 1800, 6, true),
            (false, "minecraft:wheat") => (2.0, 400, 3, false),
            (false, "minecraft:sugar") => (1.0, 600, 3, false),
            (false, "minecraft:apple") => (3.0, 1200, 3, false),
            (false, "minecraft:hay_block") => (20.0, 3600, 0, false),
            (false, "minecraft:golden_carrot") => (4.0, 1200, 5, true),
            (false, "minecraft:golden_apple" | "minecraft:enchanted_golden_apple") => {
                (10.0, 4800, 10, true)
            }
            _ => return None,
        };
        let maximum = max_health(state, body, row);
        if row.health < maximum
            || row.entity_life.child_ticks > 0
            || (!row.entity_life.tamed && pet.temper < 100)
            || (breed
                && name != "minecraft:mule"
                && row.entity_life.tamed
                && crate::entity_life::can_feed(body, row))
        {
            return Some(Action::HorseFeed {
                heal,
                growth,
                temper,
                breed: breed && name != "minecraft:mule",
            });
        }
    }
    None
}
pub(crate) fn roll(row: &mut ColdRow, bound: u32) -> u32 {
    row.entity_life.taming.rolls = row.entity_life.taming.rolls.wrapping_add(1);
    let mut seed = row.uuid.as_u128() as u64
        ^ row
            .entity_life
            .taming
            .rolls
            .wrapping_mul(0x9E3779B97F4A7C15);
    seed = (seed ^ (seed >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
    seed = (seed ^ (seed >> 27)).wrapping_mul(0x94D049BB133111EB);
    ((seed ^ (seed >> 31)) % u64::from(bound)) as u32
}
fn max_health(state: &TickState, body: &crate::entities::Body, row: &ColdRow) -> f32 {
    let base = crate::mounts::type_id(body)
        .and_then(|id| state.registry.blueprint(id))
        .map_or(20.0, |blueprint| blueprint.params.max_health);
    crate::entity_loot::effective_attribute(
        &state.registry.item_rules(),
        row,
        "minecraft:max_health",
        f64::from(base),
    ) as f32
}
pub(crate) fn apply(state: &mut TickState, slot: EntitySlot, actor: Uuid, action: &Action) {
    let maximum = max_health(state, state.entities.body(slot), state.entities.row(slot));
    let wolf = crate::mounts::name(state, slot) == "minecraft:wolf";
    let mut half_width = None;
    let (body, row) = state.entities.parts_mut(slot);
    let event = match *action {
        Action::Tame(bound) => {
            let success = roll(row, bound) == 0;
            if success {
                row.entity_life.taming.owner = Some(actor);
                row.entity_life.tamed = true;
                row.entity_life.sitting = true;
                row.persistent = true;
                if wolf {
                    row.health = 40.0;
                    wolf_health(row);
                }
            }
            Some(if success { 7 } else { 6 })
        }
        Action::Trust => {
            row.entity_life.taming.trusting = roll(row, 3) == 0;
            Some(if row.entity_life.taming.trusting {
                41
            } else {
                40
            })
        }
        Action::Sit => {
            row.entity_life.sitting = !row.entity_life.sitting;
            None
        }
        Action::Collar(color) => {
            row.entity_life.taming.collar = color;
            None
        }
        Action::Heal { maximum, amount } => {
            row.health = (row.health + amount).min(maximum);
            None
        }
        Action::HorseFeed {
            heal,
            growth,
            temper,
            breed,
        } => {
            row.health = (row.health + heal).min(maximum);
            if !row.entity_life.age_lock.locked {
                row.entity_life.child_ticks = row.entity_life.child_ticks.saturating_sub(growth);
            }
            if row.entity_life.child_ticks == 0 && row.appearance.baby {
                std::sync::Arc::make_mut(&mut row.appearance).baby = false;
                half_width = Some(body.half_width * 2.0);
                body.height *= 2.0;
            }
            row.entity_life.taming.temper = row
                .entity_life
                .taming
                .temper
                .saturating_add(temper)
                .min(100);
            if breed && row.entity_life.tamed && crate::entity_life::can_feed(body, row) {
                row.love = crate::entity_life::LOVE_TICKS;
                row.entity_life.taming.love_cause = Some(actor);
                Some(18)
            } else {
                None
            }
        }
    };
    if let Some(half_width) = half_width {
        state.entities.set_half_width(slot, half_width);
    }
    sync(state, slot);
    if let Some(event) = event {
        crate::mounts::status(state, state.entities.body(slot).id, event);
    }
    if matches!(action, Action::Tame(_))
        && event == Some(7)
        && let Some(player_slot) = state.entities.slot_of_uuid(actor)
    {
        let player = state.entities.body(player_slot).id;
        let entity = crate::advancements::entity_snapshot(state, state.entities.body(slot).id);
        crate::advancements::trigger(
            state,
            player,
            "minecraft:tame_animal",
            serde_json::json!({ "entity": entity }),
        );
    }
}
pub(crate) fn sync(state: &mut TickState, slot: EntitySlot) {
    let name = crate::mounts::name(state, slot);
    let row = state.entities.row_mut(slot);
    restore(row, name);
    state.appearance_changed.push(state.entities.body(slot).id);
}
pub(crate) fn restore(row: &mut ColdRow, name: &str) {
    if name == "minecraft:wolf" && row.entity_life.tamed {
        wolf_health(row);
    }
    let life = &row.entity_life;
    let appearance = std::sync::Arc::make_mut(&mut row.appearance);
    if matches!(
        name,
        "minecraft:wolf" | "minecraft:cat" | "minecraft:parrot"
    ) {
        let collar = if name == "minecraft:cat" { 23 } else { 21 };
        appearance
            .extra
            .retain(|(i, _)| *i != 18 && *i != 19 && (name == "minecraft:parrot" || *i != collar));
        appearance.extra.push((
            18,
            EntityDataValue::Byte(i8::from(life.sitting) | if life.tamed { 4 } else { 0 }),
        ));
        appearance.extra.push((
            19,
            EntityDataValue::OptionalLivingEntityReference(life.taming.owner),
        ));
        if name != "minecraft:parrot" {
            appearance.extra.push((
                collar,
                EntityDataValue::VarInt(i32::from(life.taming.collar)),
            ));
        }
    } else if horse(name) {
        appearance.extra.retain(|(i, _)| *i != 18);
        appearance
            .extra
            .push((18, EntityDataValue::Byte(if life.tamed { 2 } else { 0 })));
    }
}
fn wolf_health(row: &mut ColdRow) {
    if let Some(attribute) = row
        .loot
        .attributes
        .iter_mut()
        .find(|a| a.id == "minecraft:max_health")
    {
        attribute.base = 40.0;
    } else {
        row.loot
            .attributes
            .push(crate::entity_loot::EntityAttribute {
                id: "minecraft:max_health".into(),
                base: 40.0,
                modifiers: Vec::new(),
            });
    }
}
pub(crate) fn inherit(
    state: &TickState,
    first: EntityId,
    second: EntityId,
    type_id: u32,
) -> Option<TamingState> {
    let first = &state
        .entities
        .row(state.entities.slot_of(first)?)
        .entity_life
        .taming;
    let second = &state
        .entities
        .row(state.entities.slot_of(second)?)
        .entity_life
        .taming;
    let name = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)?.name;
    if matches!(name, "minecraft:wolf" | "minecraft:cat") {
        return Some(TamingState {
            owner: first.owner,
            collar: first.collar,
            ..Default::default()
        });
    }
    if name == "minecraft:fox" {
        let mut trusted: Vec<_> = [first.love_cause, second.love_cause]
            .into_iter()
            .flatten()
            .collect();
        trusted.dedup();
        return Some(TamingState {
            trusting: !trusted.is_empty(),
            trusted,
            ..Default::default()
        });
    }
    None
}
fn temper_image<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let value = u8::deserialize(deserializer)?;
    if value > 100 {
        return Err(serde::de::Error::custom("invalid animal image: Temper"));
    }
    Ok(value)
}
fn collar_image<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<u8, D::Error> {
    let value = u8::deserialize(deserializer)?;
    if value > 15 {
        return Err(serde::de::Error::custom(
            "invalid animal image: CollarColor",
        ));
    }
    Ok(value)
}
fn trusted_image<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Vec<Uuid>, D::Error> {
    let value = Vec::<Uuid>::deserialize(deserializer)?;
    if value.len() > 2 {
        return Err(serde::de::Error::custom("invalid animal image: Trusted"));
    }
    Ok(value)
}
pub(crate) fn suppress_ai(row: &ColdRow) -> bool {
    row.entity_life.sitting
        || row.entity_life.mount.rider.is_some()
        || row.entity_life.leash.is_some()
}
pub(crate) fn excluded_sweep(row: &ColdRow) -> bool {
    row.entity_life.tamed
}

#[cfg(test)]
#[path = "taming_tests.rs"]
mod tests;
