use super::*;
use mc_nbt::{ListTag, Tag, tag_type};

fn uuid_tag(uuid: Uuid) -> Tag {
    Tag::IntArray(
        uuid.as_u128()
            .to_be_bytes()
            .chunks_exact(4)
            .map(|p| i32::from_be_bytes(p.try_into().unwrap()))
            .collect(),
    )
}
fn uuid(tag: &Tag, key: &str) -> Result<Uuid, String> {
    let Tag::IntArray(parts) = tag else {
        return Err(format!("DragonFight.{key}: expected UUID int array"));
    };
    if parts.len() != 4 {
        return Err(format!("DragonFight.{key}: UUID requires four ints"));
    }
    Ok(Uuid::from_u128(parts.iter().fold(0, |value, part| {
        (value << 32) | u128::from(*part as u32)
    })))
}
fn field<'a>(fields: &'a [(String, Tag)], key: &str) -> Result<Option<&'a Tag>, String> {
    let mut values = fields.iter().filter(|(k, _)| k == key).map(|(_, v)| v);
    let value = values.next();
    if values.next().is_some() {
        return Err(format!("DragonFight.{key}: duplicate field"));
    }
    Ok(value)
}
fn flag(fields: &[(String, Tag)], key: &str, default: bool) -> Result<bool, String> {
    match field(fields, key)? {
        None => Ok(default),
        Some(Tag::Byte(0)) => Ok(false),
        Some(Tag::Byte(1)) => Ok(true),
        _ => Err(format!("DragonFight.{key}: expected boolean byte")),
    }
}
pub(crate) fn to_nbt(fight: &Fight) -> Tag {
    let mut fields = vec![
        (
            "needs_state_scanning".into(),
            Tag::Byte(i8::from(!fight.initialized)),
        ),
        (
            "dragon_killed".into(),
            Tag::Byte(i8::from(fight.dragon_killed)),
        ),
        (
            "previously_killed".into(),
            Tag::Byte(i8::from(fight.previously_killed)),
        ),
        (
            "exit_portal_location".into(),
            Tag::IntArray(fight.exit_portal.to_vec()),
        ),
        (
            "gateways".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT,
                elements: fight
                    .gateways
                    .iter()
                    .map(|i| Tag::Int(i32::from(*i)))
                    .collect(),
            }),
        ),
        (
            "respawn_time".into(),
            Tag::Int(fight.respawn.as_ref().map_or(0, |r| i32::from(r.time))),
        ),
        (
            "respawn_crystals".into(),
            Tag::List(ListTag {
                element_type: tag_type::INT_ARRAY,
                elements: fight.respawn.as_ref().map_or_else(Vec::new, |r| {
                    r.crystals.iter().map(|id| uuid_tag(*id)).collect()
                }),
            }),
        ),
        // Vanilla's codec ignores this extension; it preserves the exact tick
        // phase, beam ownership, crystal membership and death award cursor.
        (
            "SolarisFightState".into(),
            Tag::String(serde_json::to_string(fight).expect("validated retained fight")),
        ),
    ];
    if let Some(id) = fight.dragon_uuid {
        fields.push(("dragon_uuid".into(), uuid_tag(id)));
    }
    if let Some(respawn) = &fight.respawn {
        fields.push((
            "respawn_stage".into(),
            Tag::String(
                [
                    "start",
                    "preparing_to_summon_pillars",
                    "summoning_pillars",
                    "summoning_dragon",
                ][usize::from(respawn.stage)]
                .into(),
            ),
        ));
    }
    Tag::Compound(vec![
        (
            "DataVersion".into(),
            Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
        ),
        ("data".into(), Tag::Compound(fields)),
    ])
}
pub(crate) fn from_nbt(root: &Tag) -> Result<Fight, String> {
    let Tag::Compound(root) = root else {
        return Err("DragonFight: expected root compound".into());
    };
    let Some(Tag::Compound(fields)) = field(root, "data")? else {
        return Err("DragonFight.data: expected compound".into());
    };
    let mut fight = Fight {
        initialized: !flag(fields, "needs_state_scanning", true)?,
        dragon_killed: flag(fields, "dragon_killed", false)?,
        previously_killed: flag(fields, "previously_killed", false)?,
        ..Default::default()
    };
    if let Some(tag) = field(fields, "dragon_uuid")? {
        fight.dragon_uuid = Some(uuid(tag, "dragon_uuid")?);
    }
    if let Some(tag) = field(fields, "exit_portal_location")? {
        let Tag::IntArray(pos) = tag else {
            return Err("DragonFight.exit_portal_location: expected int array".into());
        };
        fight.exit_portal = pos
            .as_slice()
            .try_into()
            .map_err(|_| "DragonFight.exit_portal_location: expected three ints")?;
    }
    if let Some(tag) = field(fields, "gateways")? {
        let gateway = |index: i32| {
            if (0..20).contains(&index) {
                Ok(index as u8)
            } else {
                Err("DragonFight.gateways: index outside 0..20".to_owned())
            }
        };
        fight.gateways = match tag {
            // Vanilla's NBT list codec uses primitive arrays for integer lists.
            Tag::IntArray(values) => values
                .iter()
                .copied()
                .map(gateway)
                .collect::<Result<_, _>>(),
            // Solaris's native writer above emits a wire-typed Int list.
            Tag::List(list) => list
                .elements
                .iter()
                .map(|tag| match tag {
                    Tag::Int(index) => gateway(*index),
                    _ => Err("DragonFight.gateways: expected int element".to_owned()),
                })
                .collect::<Result<_, _>>(),
            _ => return Err("DragonFight.gateways: expected int array or int list".into()),
        }?;
    }
    if let Some(tag) = field(fields, "respawn_stage")? {
        let Tag::String(stage) = tag else {
            return Err("DragonFight.respawn_stage: expected string".into());
        };
        let stage = [
            "start",
            "preparing_to_summon_pillars",
            "summoning_pillars",
            "summoning_dragon",
        ]
        .iter()
        .position(|s| *s == stage)
        .ok_or("DragonFight.respawn_stage: invalid stage")? as u8;
        let time = match field(fields, "respawn_time")? {
            Some(Tag::Int(t)) if *t >= 0 => {
                u16::try_from(*t).map_err(|_| "DragonFight.respawn_time: out of range")?
            }
            None => 0,
            _ => return Err("DragonFight.respawn_time: invalid time".into()),
        };
        let Some(Tag::List(list)) = field(fields, "respawn_crystals")? else {
            return Err("DragonFight.respawn_crystals: expected UUID list".into());
        };
        let ids = list
            .elements
            .iter()
            .map(|tag| uuid(tag, "respawn_crystals"))
            .collect::<Result<Vec<_>, _>>()?;
        let crystals: [Uuid; 4] = ids
            .try_into()
            .map_err(|_| "DragonFight.respawn_crystals: expected four UUIDs")?;
        fight.respawn = Some(Respawn {
            stage,
            time,
            crystals,
        });
        // Entity-region hydration supplies positions before cross-record validation.
        fight.crystals = crystals
            .iter()
            .map(|id| Crystal {
                uuid: *id,
                position: [0.0; 3],
                pillar: None,
                beam: Some([0, 128, 0]),
                invulnerable: false,
            })
            .collect();
    }
    if let Some(tag) = field(fields, "SolarisFightState")? {
        let Tag::String(json) = tag else {
            return Err("DragonFight.SolarisFightState: expected string".into());
        };
        let saved: Fight = serde_json::from_str(json)
            .map_err(|e| format!("DragonFight.SolarisFightState: {e}"))?;
        if saved.initialized != fight.initialized
            || saved.dragon_uuid != fight.dragon_uuid
            || saved.dragon_killed != fight.dragon_killed
            || saved.previously_killed != fight.previously_killed
            || saved.exit_portal != fight.exit_portal
            || saved.gateways != fight.gateways
            || saved.respawn != fight.respawn
        {
            return Err("DragonFight: native fields disagree with SolarisFightState".into());
        }
        return Ok(saved);
    }
    fight.native_import = true;
    fight.validate()?;
    Ok(fight)
}

pub(crate) fn native_appearance(
    record: &mc_data::entity_nbt::VanillaEntity,
    image: &mut crate::journal::EntityAppearanceImage,
) -> Result<(), String> {
    use crate::journal::EntityDataValueImage as V;
    if record.id == "minecraft:ender_dragon" {
        let phase = match field(&record.extra_fields, "DragonPhase")? {
            None => 10,
            Some(Tag::Int(v)) if (0..=10).contains(v) => *v,
            _ => return Err("DragonPhase: expected int 0..10".into()),
        };
        match field(&record.extra_fields, "DragonDeathTime")? {
            None => {}
            Some(Tag::Int(v)) if (0..=200).contains(v) => {}
            _ => return Err("DragonDeathTime: expected int 0..200".into()),
        }
        image.extra.push((16, V::VarInt(phase)));
    } else if record.id == "minecraft:end_crystal" {
        let beam = match field(&record.extra_fields, "beam_target")? {
            None => None,
            Some(Tag::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
            _ => return Err("beam_target: expected three-int array".into()),
        };
        image.extra.push((8, V::OptionalBlockPos(beam)));
        image.extra.push((
            9,
            V::Boolean(flag(&record.extra_fields, "ShowBottom", true)?),
        ));
    }
    Ok(())
}
pub(crate) fn entity_fields(
    state: &crate::tick::TickState,
    slot: crate::entities::EntitySlot,
) -> Vec<(String, Tag)> {
    if super::is_dragon(state, slot) {
        return vec![
            ("DragonPhase".into(), Tag::Int(state.end_fight.phase as i32)),
            (
                "DragonDeathTime".into(),
                Tag::Int(i32::from(state.end_fight.death_ticks)),
            ),
        ];
    }
    if !super::is_crystal(state, slot) {
        return Vec::new();
    }
    let row = state.entities.row(slot);
    let mut fields = vec![(
        "ShowBottom".into(),
        Tag::Byte(i8::from(
            row.appearance
                .extra
                .iter()
                .find_map(|(i, v)| match (i, v) {
                    (9, crate::boundary::EntityDataValue::Boolean(v)) => Some(*v),
                    _ => None,
                })
                .unwrap_or(true),
        )),
    )];
    if let Some(beam) = row.appearance.extra.iter().find_map(|(i, v)| match (i, v) {
        (8, crate::boundary::EntityDataValue::OptionalBlockPos(v)) => *v,
        _ => None,
    }) {
        fields.push(("beam_target".into(), Tag::IntArray(beam.to_vec())));
    }
    fields
}

pub(crate) fn hydrate_native_entity(
    fight: &mut Fight,
    record: &mc_data::entity_nbt::VanillaEntity,
) -> Result<(), String> {
    if fight.native_import && record.id == "minecraft:ender_dragon" {
        fight.death_ticks = match field(&record.extra_fields, "DragonDeathTime")? {
            None => 0,
            Some(Tag::Int(v)) if (0..=200).contains(v) => *v as u16,
            _ => return Err("DragonDeathTime: expected int 0..200".into()),
        };
    }
    Ok(())
}
