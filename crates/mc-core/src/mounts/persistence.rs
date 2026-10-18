//! 26.1.2 animal fields (OLD persistence.rs and local LeashData/BlockPos javap).
use crate::{CoreError, entity_life::EntityLifeState};
use mc_nbt::{ListTag, Tag, tag_type};
use uuid::Uuid;
fn invalid(field: &str) -> CoreError {
    CoreError::Storage(format!("invalid animal NBT: {field}"))
}
fn uuid(tag: &Tag, field: &str) -> Result<Uuid, CoreError> {
    match tag {
        Tag::IntArray(v) if v.len() == 4 => Ok(Uuid::from_u128(
            (u128::from(v[0] as u32) << 96)
                | (u128::from(v[1] as u32) << 64)
                | (u128::from(v[2] as u32) << 32)
                | u128::from(v[3] as u32),
        )),
        Tag::String(v) => Uuid::parse_str(v).map_err(|_| invalid(field)),
        _ => Err(invalid(field)),
    }
}
fn uuid_tag(uuid: Uuid) -> Tag {
    let n = uuid.as_u128();
    Tag::IntArray(vec![
        (n >> 96) as i32,
        (n >> 64) as i32,
        (n >> 32) as i32,
        n as i32,
    ])
}
fn field<'a>(fields: &'a [(String, Tag)], key: &str) -> Result<Option<&'a Tag>, CoreError> {
    let mut matches = fields.iter().filter(|(name, _)| name == key);
    let value = matches.next().map(|(_, tag)| tag);
    if matches.next().is_some() {
        return Err(invalid(key));
    }
    Ok(value)
}
fn integer(tag: &Tag, key: &str) -> Result<i32, CoreError> {
    match tag {
        Tag::Byte(n) => Ok(i32::from(*n)),
        Tag::Short(n) => Ok(i32::from(*n)),
        Tag::Int(n) => Ok(*n),
        _ => Err(invalid(key)),
    }
}
pub(crate) fn from_fields(
    life: &mut EntityLifeState,
    fields: &[(String, Tag)],
) -> Result<(), CoreError> {
    let mut result = life.clone();
    for key in ["Owner", "LoveCause", "solaris:Rider"] {
        if let Some(tag) = field(fields, key)? {
            let value = uuid(tag, key)?;
            match key {
                "Owner" => {
                    result.taming.owner = Some(value);
                    result.tamed = true;
                }
                "LoveCause" => result.taming.love_cause = Some(value),
                _ => {
                    result.mount.rider = Some(value);
                    result.mounted = true;
                }
            }
        }
    }
    for key in ["Tame", "Sitting", "Trusting", "Temper", "CollarColor"] {
        if let Some(tag) = field(fields, key)? {
            let value = integer(tag, key)?;
            match key {
                "Tame" => {
                    result.tamed = value != 0 || result.taming.owner.is_some();
                }
                "Sitting" => result.sitting = value != 0,
                "Trusting" => result.taming.trusting = value != 0,
                "Temper" => {
                    if !(0..=100).contains(&value) {
                        return Err(invalid(key));
                    }
                    result.taming.temper = value as u8;
                }
                _ => {
                    if !(0..=15).contains(&value) {
                        return Err(invalid(key));
                    }
                    result.taming.collar = value as u8;
                }
            }
        }
    }
    if let Some(tag) = field(fields, "Trusted")? {
        let Tag::List(list) = tag else {
            return Err(invalid("Trusted"));
        };
        if (list.element_type != tag_type::INT_ARRAY && !list.elements.is_empty())
            || list.elements.len() > 2
        {
            return Err(invalid("Trusted"));
        }
        result.taming.trusted = list
            .elements
            .iter()
            .map(|tag| uuid(tag, "Trusted"))
            .collect::<Result<_, _>>()?;
        result.taming.trusting |= !result.taming.trusted.is_empty();
    }
    if let Some(tag) = field(fields, "leash")? {
        result.leash = Some(match tag {
            Tag::Compound(fields) => crate::leash::Holder::Entity(uuid(
                field(fields, "UUID")?.ok_or_else(|| invalid("leash.UUID"))?,
                "leash.UUID",
            )?),
            Tag::IntArray(pos) if pos.len() == 3 => {
                crate::leash::Holder::Fence([pos[0], pos[1], pos[2]])
            }
            _ => return Err(invalid("leash")),
        });
    }
    if let Some(tag) = field(fields, "solaris:TameRolls")? {
        let Tag::Long(rolls) = tag else {
            return Err(invalid("solaris:TameRolls"));
        };
        result.taming.rolls = *rolls as u64;
    }
    *life = result;
    Ok(())
}
pub(crate) fn to_fields(life: &EntityLifeState) -> Vec<(String, Tag)> {
    let pet = &life.taming;
    let mut fields = Vec::new();
    if pet.temper != 0 {
        fields.push(("Temper".into(), Tag::Int(i32::from(pet.temper))));
    }
    if pet.collar != 14 {
        fields.push(("CollarColor".into(), Tag::Byte(pet.collar as i8)));
    }
    if pet.trusting && pet.trusted.is_empty() {
        fields.push(("Trusting".into(), Tag::Byte(1)));
    }
    if pet.rolls != 0 {
        fields.push(("solaris:TameRolls".into(), Tag::Long(pet.rolls as i64)));
    }
    for (key, uuid) in [
        ("Owner", pet.owner),
        ("LoveCause", pet.love_cause),
        ("solaris:Rider", life.mount.rider),
    ] {
        if let Some(uuid) = uuid {
            fields.push((key.into(), uuid_tag(uuid)));
        }
    }
    if !pet.trusted.is_empty() {
        fields.push((
            "Trusted".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT_ARRAY,
                elements: pet.trusted.iter().map(|id| uuid_tag(*id)).collect(),
            }),
        ));
    }
    if let Some(holder) = &life.leash {
        fields.push((
            "leash".into(),
            match holder {
                crate::leash::Holder::Entity(uuid) => {
                    Tag::Compound(vec![("UUID".into(), uuid_tag(*uuid))])
                }
                crate::leash::Holder::Fence(pos) => Tag::IntArray(pos.to_vec()),
            },
        ));
    }
    fields
}

pub(crate) fn validate_replay(replay: &crate::journal::Replay) -> Result<(), CoreError> {
    if !replay
        .entities
        .values()
        .any(|entity| entity.state.entity_life.mount.rider.is_some())
    {
        return Ok(());
    }
    let entity_uuids = replay
        .entities
        .values()
        .map(|entity| Uuid::parse_str(&entity.uuid).map_err(|_| invalid("UUID")))
        .collect::<Result<std::collections::HashSet<_>, _>>()?;
    let mut riders = std::collections::BTreeMap::new();
    for (id, entity) in &replay.entities {
        let Some(rider) = entity.state.entity_life.mount.rider else {
            continue;
        };
        if let Some(previous) = riders.insert(rider, *id) {
            return Err(CoreError::Storage(format!(
                "invalid mounted passenger {rider}: vehicles {previous} and {id}"
            )));
        }
        if entity_uuids.contains(&rider) {
            return Err(CoreError::Storage(format!(
                "invalid mounted passenger {rider}: expected player UUID"
            )));
        }
    }
    Ok(())
}
