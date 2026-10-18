//! 26.1.2 Raids.CODEC/Raid.MAP_CODEC (javap receipt in parity/raids-*.javap).
use super::*;
use crate::config::CoreError;
use mc_nbt::{ListTag, Tag, tag_type};
type Fields = [(String, Tag)];
fn compound(tag: &Tag) -> Result<&Fields, String> {
    match tag {
        Tag::Compound(f) => Ok(f),
        _ => Err("expected raids compound".into()),
    }
}
fn field<'a>(f: &'a Fields, key: &str) -> Result<Option<&'a Tag>, String> {
    let mut it = f.iter().filter(|(k, _)| k == key);
    let value = it.next().map(|(_, v)| v);
    if it.next().is_some() {
        return Err(format!("duplicate raids field {key}"));
    }
    Ok(value)
}
fn required<'a>(f: &'a Fields, key: &str) -> Result<&'a Tag, String> {
    field(f, key)?.ok_or_else(|| format!("missing raids field {key}"))
}
fn int(f: &Fields, key: &str) -> Result<i32, String> {
    match required(f, key)? {
        Tag::Int(v) => Ok(*v),
        _ => Err(format!("invalid raids integer {key}")),
    }
}
fn boolean(f: &Fields, key: &str) -> Result<bool, String> {
    match required(f, key)? {
        Tag::Byte(0) => Ok(false),
        Tag::Byte(1) => Ok(true),
        _ => Err(format!("invalid raids boolean {key}")),
    }
}
fn array<const N: usize>(tag: &Tag, key: &str) -> Result<[i32; N], String> {
    match tag {
        Tag::IntArray(a) => a
            .as_slice()
            .try_into()
            .map_err(|_| format!("invalid raids {key} length")),
        _ => Err(format!("invalid raids int array {key}")),
    }
}
fn uuid(tag: &Tag) -> Result<Uuid, String> {
    let a = array::<4>(tag, "UUID")?;
    let value = a
        .into_iter()
        .fold(0u128, |v, n| (v << 32) | u128::from(n as u32));
    Ok(Uuid::from_u128(value))
}
fn uuid_tag(u: Uuid) -> Tag {
    let n = u.as_u128();
    Tag::IntArray((0..4).map(|i| (n >> (96 - 32 * i)) as i32).collect())
}
fn list<'a>(f: &'a Fields, key: &str, kind: u8) -> Result<&'a [Tag], String> {
    match field(f, key)? {
        None => Ok(&[]),
        Some(Tag::List(l))
            if l.element_type == kind
                || (l.elements.is_empty() && l.element_type == tag_type::END) =>
        {
            Ok(&l.elements)
        }
        _ => Err(format!("invalid raids list {key}")),
    }
}
fn uuids(f: &Fields, key: &str) -> Result<BTreeSet<Uuid>, String> {
    let mut out = BTreeSet::new();
    for tag in list(f, key, tag_type::INT_ARRAY)? {
        if !out.insert(uuid(tag)?) {
            return Err(format!("duplicate raids UUID in {key}"));
        }
    }
    Ok(out)
}
fn uuid_list(s: &BTreeSet<Uuid>) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::INT_ARRAY,
        elements: s.iter().map(|u| uuid_tag(*u)).collect(),
    })
}
fn tags(kind: u8, elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: kind,
        elements,
    })
}
impl Raids {
    pub(crate) fn validate(&self) -> Result<(), CoreError> {
        let err = |s: String| CoreError::Storage(format!("raids: {s}"));
        if self.next_id < 1 {
            return Err(err("invalid next_id".into()));
        }
        if self.clock_tick.is_some_and(|tick| tick > i64::MAX as u64) {
            return Err(err("invalid clock_tick".into()));
        }
        if self
            .pending
            .values()
            .any(|pending| pending.deadline > i64::MAX as u64)
        {
            return Err(err("invalid pending omen deadline".into()));
        }
        let mut members = BTreeSet::new();
        for (&id, r) in &self.raids {
            if id <= 0 || id > self.next_id {
                return Err(err(format!("raid {id}: invalid id")));
            }
            if !(0..=5).contains(&r.raid_omen_level)
                || !(0..=8).contains(&r.groups_spawned)
                || ![0, 3, 5, 7].contains(&r.group_count)
                || r.groups_spawned > r.group_count + 1
                || r.cooldown_ticks < 0
                || r.post_raid_ticks < 0
                || r.ticks_active > i64::MAX as u64
                || !r.total_health.is_finite()
                || r.total_health < 0.0
                || !["ongoing", "victory", "loss", "stopped"].contains(&r.status.as_str())
            {
                return Err(err(format!("raid {id}: invalid retained fields")));
            }
            for u in &r.members {
                if !members.insert(*u) {
                    return Err(err(format!("raid {id}: duplicate member UUID {u}")));
                }
            }
        }
        Ok(())
    }
    pub(crate) fn to_nbt(&self) -> Tag {
        let raids = self
            .raids
            .iter()
            .map(|(&id, r)| {
                Tag::Compound(vec![
                    ("id".into(), Tag::Int(id)),
                    ("started".into(), Tag::Byte(i8::from(r.started))),
                    ("active".into(), Tag::Byte(i8::from(r.active))),
                    ("ticks_active".into(), Tag::Long(r.ticks_active as i64)),
                    ("raid_omen_level".into(), Tag::Int(r.raid_omen_level)),
                    ("groups_spawned".into(), Tag::Int(r.groups_spawned)),
                    ("cooldown_ticks".into(), Tag::Int(r.cooldown_ticks)),
                    ("post_raid_ticks".into(), Tag::Int(r.post_raid_ticks)),
                    ("total_health".into(), Tag::Float(r.total_health)),
                    ("group_count".into(), Tag::Int(r.group_count)),
                    ("status".into(), Tag::String(r.status.clone())),
                    ("center".into(), Tag::IntArray(r.center.to_vec())),
                    ("heroes_of_the_village".into(), uuid_list(&r.heroes)),
                    ("solaris:members".into(), uuid_list(&r.members)),
                ])
            })
            .collect();
        let pending = self
            .pending
            .iter()
            .map(|(&u, p)| {
                Tag::Compound(vec![
                    ("UUID".into(), uuid_tag(u)),
                    ("center".into(), Tag::IntArray(p.center.to_vec())),
                    ("deadline".into(), Tag::Long(p.deadline as i64)),
                ])
            })
            .collect();
        let mut data = vec![
            ("raids".into(), tags(tag_type::COMPOUND, raids)),
            ("next_id".into(), Tag::Int(self.next_id)),
            ("tick".into(), Tag::Int(self.tick)),
            (
                "solaris:pending_omens".into(),
                tags(tag_type::COMPOUND, pending),
            ),
        ];
        if let Some(tick) = self.clock_tick {
            data.push(("solaris:clock_tick".into(), Tag::Long(tick as i64)));
        }
        Tag::Compound(vec![
            (
                "DataVersion".into(),
                Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
            ),
            ("data".into(), Tag::Compound(data)),
        ])
    }
    pub(crate) fn from_nbt(root: &Tag) -> Result<Self, String> {
        let root = compound(root)?;
        if let Some(version) = field(root, "DataVersion")?
            && !matches!(version, Tag::Int(_))
        {
            return Err("invalid raids DataVersion".into());
        }
        let data = compound(required(root, "data")?)?;
        let mut out = Self {
            next_id: int(data, "next_id")?,
            tick: int(data, "tick")?,
            ..Default::default()
        };
        out.clock_tick = match field(data, "solaris:clock_tick")? {
            None => None,
            Some(Tag::Long(tick)) if *tick >= 0 => Some(*tick as u64),
            _ => return Err("invalid raids clock_tick".into()),
        };
        for (index, tag) in list(data, "raids", tag_type::COMPOUND)?.iter().enumerate() {
            let decode = || -> Result<(i32, Raid), String> {
                let f = compound(tag)?;
                required(f, "heroes_of_the_village")?;
                let ticks = match required(f, "ticks_active")? {
                    Tag::Long(v) if *v >= 0 => *v as u64,
                    _ => return Err("invalid raids ticks_active".into()),
                };
                let health = match required(f, "total_health")? {
                    Tag::Float(v) => *v,
                    _ => return Err("invalid raids total_health".into()),
                };
                let status = match required(f, "status")? {
                    Tag::String(v) => v.clone(),
                    _ => return Err("invalid raids status".into()),
                };
                Ok((
                    int(f, "id")?,
                    Raid {
                        center: array(required(f, "center")?, "center")?,
                        started: boolean(f, "started")?,
                        active: boolean(f, "active")?,
                        ticks_active: ticks,
                        raid_omen_level: int(f, "raid_omen_level")?,
                        groups_spawned: int(f, "groups_spawned")?,
                        cooldown_ticks: int(f, "cooldown_ticks")?,
                        post_raid_ticks: int(f, "post_raid_ticks")?,
                        total_health: health,
                        group_count: int(f, "group_count")?,
                        status,
                        heroes: uuids(f, "heroes_of_the_village")?,
                        members: uuids(f, "solaris:members")?,
                    },
                ))
            };
            let (id, raid) = decode().map_err(|e| format!("raid-index {index}: {e}"))?;
            if out.raids.insert(id, raid).is_some() {
                return Err(format!("raid-index {index}: duplicate raid id {id}"));
            }
        }
        for tag in list(data, "solaris:pending_omens", tag_type::COMPOUND)? {
            let f = compound(tag)?;
            let u = uuid(required(f, "UUID")?)?;
            let deadline = match required(f, "deadline")? {
                Tag::Long(v) if *v >= 0 => *v as u64,
                _ => return Err("invalid raids omen deadline".into()),
            };
            if out
                .pending
                .insert(
                    u,
                    PendingOmen {
                        center: array(required(f, "center")?, "center")?,
                        deadline,
                    },
                )
                .is_some()
            {
                return Err(format!("duplicate raids pending UUID {u}"));
            }
        }
        out.validate().map_err(|e| e.to_string())?;
        Ok(out)
    }
}
pub(crate) fn to_nbt(raids: &Raids) -> Tag {
    raids.to_nbt()
}

pub(crate) fn from_nbt(root: &Tag) -> Result<Raids, String> {
    Raids::from_nbt(root)
}
