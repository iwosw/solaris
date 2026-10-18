//! Namespaced native attack images plus the verified vanilla save projection.
//! Vanilla keys: WitherBoss.Invul (int), EvokerFangs.Warmup (int)/Owner (UUID).
use super::Retained;
use crate::config::CoreError;
use mc_nbt::Tag;
use serde::Deserialize;
const KEY: &str = "solaris:SpecialMob";

pub(crate) fn validate(value: &Retained) -> Result<(), &'static str> {
    match value {
        Retained::Copper(value) => crate::copper_golem::validate(value),
        Retained::Warden(value) => crate::warden::validate(value),
        Retained::Fangs { warmup, life, .. }
            if !(-1..=1200).contains(warmup) || !(1..=22).contains(life) =>
        {
            Err("Fangs: warmup expected -1..=1200 and life expected 1..=22")
        }
        Retained::Vex { life, .. } if !(1..=24000).contains(life) => {
            Err("Vex.life: expected 1..=24000")
        }
        Retained::Wither { invulnerable } if *invulnerable > 220 => {
            Err("Wither.invulnerable: expected 0..=220")
        }
        _ => Ok(()),
    }
}
pub(crate) fn deserialize<'de, D: serde::Deserializer<'de>>(
    decoder: D,
) -> Result<Option<Retained>, D::Error> {
    let value = Option::<Retained>::deserialize(decoder)?;
    if let Some(value) = &value {
        validate(value).map_err(serde::de::Error::custom)?;
    }
    Ok(value)
}
pub(crate) fn uuid_tag(uuid: uuid::Uuid) -> Tag {
    Tag::IntArray(
        uuid.as_bytes()
            .chunks_exact(4)
            .map(|word| i32::from_be_bytes(word.try_into().expect("uuid word")))
            .collect(),
    )
}
fn uuid(tag: &Tag) -> Result<uuid::Uuid, CoreError> {
    let Tag::IntArray(words) = tag else {
        return Err(CoreError::Storage(
            "SpecialMob.Owner: expected UUID int array".into(),
        ));
    };
    if words.len() != 4 {
        return Err(CoreError::Storage(
            "SpecialMob.Owner: expected four UUID words".into(),
        ));
    }
    let mut bytes = [0; 16];
    for (out, word) in bytes.chunks_exact_mut(4).zip(words) {
        out.copy_from_slice(&word.to_be_bytes());
    }
    Ok(uuid::Uuid::from_bytes(bytes))
}
pub(crate) fn from_record(
    record: &mc_data::entity_nbt::VanillaEntity,
) -> Result<Option<Retained>, CoreError> {
    let fail = |message: &str| CoreError::Storage(format!("{KEY}: {message}"));
    let field = |key: &str| -> Result<Option<&Tag>, CoreError> {
        let mut matching = record.extra_fields.iter().filter(|(name, _)| name == key);
        let first = matching.next().map(|(_, value)| value);
        if matching.next().is_some() {
            let name = match key {
                "Invul" => "Wither.invulnerable",
                "Warmup" => "Fangs.warmup",
                "Owner" => "Fangs.owner",
                _ => key,
            };
            return Err(fail(&format!("{name}: duplicate retained field")));
        }
        Ok(first)
    };
    let projected = if record.id == "minecraft:wither" {
        let invulnerable = match field("Invul")? {
            None => 0,
            Some(Tag::Int(value)) => {
                u32::try_from(*value).map_err(|_| fail("Wither.invulnerable: expected 0..=220"))?
            }
            Some(_) => return Err(fail("Wither.invulnerable: expected int")),
        };
        Some(Retained::Wither { invulnerable })
    } else if record.id == "minecraft:evoker_fangs" {
        let warmup = match field("Warmup")? {
            None => 0,
            Some(Tag::Int(value)) => *value,
            Some(_) => return Err(fail("Fangs.warmup: expected int")),
        };
        Some(Retained::Fangs {
            owner: field("Owner")?.map(uuid).transpose()?,
            warmup,
            life: 22,
        })
    } else if record.id == "minecraft:copper_golem" {
        Some(Retained::Copper(
            crate::copper_golem::from_fields(&record.extra_fields).map_err(|err| fail(&err))?,
        ))
    } else if record.id == "minecraft:iron_golem" {
        match field("PlayerCreated")? {
            Some(Tag::Byte(value)) if *value != 0 => Some(Retained::PlayerIronGolem),
            Some(Tag::Byte(_)) | None => None,
            Some(_) => return Err(fail("IronGolem.PlayerCreated: expected byte")),
        }
    } else if record.id == "minecraft:warden" {
        Some(Retained::Warden(crate::warden::read_anger(field(
            "anger",
        )?)?))
    } else {
        None
    };
    if let Some(value) = &projected {
        validate(value).map_err(fail)?;
    }
    let result = if let Some(tag) = field(KEY)? {
        let Tag::Compound(fields) = tag else {
            return Err(fail("expected compound"));
        };
        if fields.len() != 2 {
            return Err(fail("expected version and state"));
        }
        if !fields
            .iter()
            .any(|(name, tag)| name == "version" && *tag == Tag::Int(1))
        {
            return Err(fail("version: expected int 1"));
        }
        let Some(Tag::String(json)) = fields
            .iter()
            .find(|(name, _)| name == "state")
            .map(|(_, tag)| tag)
        else {
            return Err(fail("state: expected string"));
        };
        Some(serde_json::from_str::<Retained>(json).map_err(|err| fail(&format!("state: {err}")))?)
    } else {
        projected.clone()
    };
    if let Some(value) = &result {
        validate(value).map_err(fail)?;
        let name = match value {
            Retained::Crossbow(_) => "minecraft:pillager",
            Retained::Piglin(_) => "minecraft:piglin",
            Retained::Enderman { .. } => "minecraft:enderman",
            Retained::Witch(_) => "minecraft:witch",
            Retained::Fangs { .. } => "minecraft:evoker_fangs",
            Retained::Vex { .. } => "minecraft:vex",
            Retained::Wither { .. } => "minecraft:wither",
            Retained::Copper(_) => "minecraft:copper_golem",
            Retained::PlayerIronGolem => "minecraft:iron_golem",
            Retained::Warden(_) => "minecraft:warden",
        };
        if record.id != name {
            return Err(fail("retained variant incompatible with entity type"));
        }
        match (value, projected) {
            (Retained::Warden(value), Some(Retained::Warden(saved)))
                if field("anger")?.is_some() && value.anger != saved.anger =>
            {
                return Err(fail("Warden.anger: native/anger mismatch"));
            }
            (Retained::Copper(value), Some(Retained::Copper(saved))) if *value != saved => {
                return Err(fail(
                    "Copper: native/weather_state/next_weather_age mismatch",
                ));
            }
            (
                Retained::Wither { invulnerable },
                Some(Retained::Wither {
                    invulnerable: saved,
                }),
            ) if field("Invul")?.is_some() && *invulnerable != saved => {
                return Err(fail("Wither.invulnerable: native/Invul mismatch"));
            }
            (
                Retained::Fangs { owner, warmup, .. },
                Some(Retained::Fangs {
                    owner: saved_owner,
                    warmup: saved_warmup,
                    ..
                }),
            ) => {
                if field("Warmup")?.is_some() && *warmup != saved_warmup {
                    return Err(fail("Fangs.warmup: native/Warmup mismatch"));
                }
                if field("Owner")?.is_some() && *owner != saved_owner {
                    return Err(fail("Fangs.owner: native/Owner mismatch"));
                }
            }
            _ => {}
        }
    }
    Ok(result)
}
pub(crate) fn to_fields(value: &Retained) -> Vec<(String, Tag)> {
    validate(value).expect("live special mob state invariant");
    let mut fields = vec![(
        KEY.into(),
        Tag::Compound(vec![
            ("version".into(), Tag::Int(1)),
            (
                "state".into(),
                Tag::String(serde_json::to_string(value).expect("retained serde")),
            ),
        ]),
    )];
    match value {
        Retained::Copper(value) => fields.extend(crate::copper_golem::to_fields(value)),
        Retained::PlayerIronGolem => fields.push(("PlayerCreated".into(), Tag::Byte(1))),
        Retained::Warden(value) => fields.push(("anger".into(), crate::warden::anger_tag(value))),
        Retained::Wither { invulnerable } => {
            fields.push(("Invul".into(), Tag::Int(*invulnerable as i32)))
        }
        Retained::Fangs { owner, warmup, .. } => {
            fields.push(("Warmup".into(), Tag::Int(*warmup)));
            if let Some(owner) = owner {
                fields.push(("Owner".into(), uuid_tag(*owner)));
            }
        }
        _ => {}
    }
    fields
}
