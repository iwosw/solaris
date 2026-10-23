//! Pure vanilla ScoreboardSaveData packed NBT codec.
//! Components are literal strings or serialized JSON compounds/lists.
//! Unsupported non-default vanilla features fail rather than being silently
//! discarded on the next checkpoint.

use std::collections::{BTreeMap, BTreeSet};

use crate::scoreboard::{Objective, Scoreboard, Team};
use mc_nbt::{ListTag, Tag, tag_type};

const COLORS: [&str; 16] = [
    "black",
    "dark_blue",
    "dark_green",
    "dark_aqua",
    "dark_red",
    "dark_purple",
    "gold",
    "gray",
    "dark_gray",
    "blue",
    "green",
    "aqua",
    "red",
    "light_purple",
    "yellow",
    "white",
];
const VISIBILITY: [&str; 4] = ["always", "never", "hideForOtherTeams", "hideForOwnTeam"];
const COLLISION: [&str; 4] = ["always", "never", "pushOtherTeams", "pushOwnTeam"];
const SLOTS: [&str; 19] = [
    "list",
    "sidebar",
    "below_name",
    "sidebar.team.black",
    "sidebar.team.dark_blue",
    "sidebar.team.dark_green",
    "sidebar.team.dark_aqua",
    "sidebar.team.dark_red",
    "sidebar.team.dark_purple",
    "sidebar.team.gold",
    "sidebar.team.gray",
    "sidebar.team.dark_gray",
    "sidebar.team.blue",
    "sidebar.team.green",
    "sidebar.team.aqua",
    "sidebar.team.red",
    "sidebar.team.light_purple",
    "sidebar.team.yellow",
    "sidebar.team.white",
];

type Fields = [(String, Tag)];
fn compound(tag: &Tag) -> Result<&Fields, String> {
    match tag {
        Tag::Compound(fields) => Ok(fields),
        _ => Err("expected scoreboard compound".into()),
    }
}
fn field<'a>(fields: &'a Fields, key: &str) -> Result<Option<&'a Tag>, String> {
    let mut matches = fields.iter().filter(|(name, _)| name == key);
    let value = matches.next().map(|(_, value)| value);
    if matches.next().is_some() {
        return Err(format!("duplicate scoreboard field {key}"));
    }
    Ok(value)
}
fn string(tag: &Tag) -> Result<&str, String> {
    match tag {
        Tag::String(value) => Ok(value),
        _ => Err("expected scoreboard string".into()),
    }
}
fn text(tag: &Tag) -> Result<String, String> {
    match tag {
        Tag::String(value) => Ok(value.clone()),
        Tag::Compound(fields) if fields.len() == 1 && fields[0].0 == "text" => {
            Ok(string(&fields[0].1)?.to_owned())
        }
        Tag::Compound(_) | Tag::List(_) => {
            serde_json::to_string(&component_json(tag)?).map_err(|error| error.to_string())
        }
        _ => Err("invalid scoreboard component".into()),
    }
}

fn component_json(tag: &Tag) -> Result<serde_json::Value, String> {
    use serde_json::Value;
    match tag {
        Tag::String(value) => Ok(Value::String(value.clone())),
        Tag::Byte(value) if *value == 0 || *value == 1 => Ok(Value::Bool(*value != 0)),
        Tag::Int(value) => Ok(Value::from(*value)),
        Tag::Long(value) => Ok(Value::from(*value)),
        Tag::Float(value) => serde_json::Number::from_f64(f64::from(*value))
            .map(Value::Number)
            .ok_or_else(|| "non-finite component number".into()),
        Tag::Double(value) => serde_json::Number::from_f64(*value)
            .map(Value::Number)
            .ok_or_else(|| "non-finite component number".into()),
        Tag::Compound(fields) => {
            let mut object = serde_json::Map::new();
            for (name, value) in fields {
                if object
                    .insert(name.clone(), component_json(value)?)
                    .is_some()
                {
                    return Err(format!("duplicate component field {name}"));
                }
            }
            Ok(Value::Object(object))
        }
        Tag::List(list) => Ok(Value::Array(
            list.elements
                .iter()
                .map(component_json)
                .collect::<Result<_, _>>()?,
        )),
        _ => Err("unsupported component NBT value".into()),
    }
}

fn json_component(value: &serde_json::Value) -> Option<Tag> {
    use serde_json::Value;
    match value {
        Value::String(value) => Some(Tag::String(value.clone())),
        Value::Bool(value) => Some(Tag::Byte(i8::from(*value))),
        Value::Number(value) => value
            .as_i64()
            .map(Tag::Long)
            .or_else(|| value.as_f64().map(Tag::Double)),
        Value::Object(object) => Some(Tag::Compound(
            object
                .iter()
                .map(|(name, value)| Some((name.clone(), json_component(value)?)))
                .collect::<Option<_>>()?,
        )),
        Value::Array(array) => {
            let mut elements: Vec<Tag> = array.iter().map(json_component).collect::<Option<_>>()?;
            // Component lists may mix literal strings and objects in JSON;
            // NBT lists require a uniform type, so normalize literals.
            if elements
                .iter()
                .any(|value| matches!(value, Tag::Compound(_)))
                && elements
                    .iter()
                    .all(|value| matches!(value, Tag::String(_) | Tag::Compound(_)))
            {
                for value in &mut elements {
                    if let Tag::String(text) = value {
                        *value =
                            Tag::Compound(vec![("text".into(), Tag::String(std::mem::take(text)))]);
                    }
                }
            }
            let kind = elements.first().map_or(tag_type::END, Tag::type_id);
            if !elements.iter().all(|value| value.type_id() == kind) {
                return None;
            }
            Some(Tag::List(ListTag {
                element_type: kind,
                elements,
            }))
        }
        Value::Null => None,
    }
}

fn component(value: &str) -> Tag {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()
        .filter(|value| {
            matches!(
                value,
                serde_json::Value::Object(_) | serde_json::Value::Array(_)
            )
        })
        .as_ref()
        .and_then(json_component)
        .unwrap_or_else(|| Tag::String(value.to_owned()))
}
fn required_string(fields: &Fields, key: &str) -> Result<String, String> {
    string(field(fields, key)?.ok_or_else(|| format!("missing scoreboard field {key}"))?)
        .map(str::to_owned)
}
fn optional_string(fields: &Fields, key: &str, default: &str) -> Result<String, String> {
    field(fields, key)?
        .map(string)
        .transpose()
        .map(|value| value.unwrap_or(default).to_owned())
}
fn optional_text(fields: &Fields, key: &str, default: &str) -> Result<String, String> {
    field(fields, key)?
        .map(text)
        .transpose()
        .map(|value| value.unwrap_or_else(|| default.to_owned()))
}
fn boolean(fields: &Fields, key: &str, default: bool) -> Result<bool, String> {
    match field(fields, key)? {
        None => Ok(default),
        Some(Tag::Byte(0)) => Ok(false),
        Some(Tag::Byte(1)) => Ok(true),
        _ => Err(format!("invalid scoreboard boolean {key}")),
    }
}
fn list<'a>(fields: &'a Fields, key: &str, kind: u8) -> Result<&'a [Tag], String> {
    match field(fields, key)? {
        None => Ok(&[]),
        Some(Tag::List(list))
            if list.element_type == kind
                || (list.elements.is_empty() && list.element_type == tag_type::END) =>
        {
            Ok(&list.elements)
        }
        _ => Err(format!("invalid scoreboard list {key}")),
    }
}
fn named_index(fields: &Fields, key: &str, names: &[&str], default: &str) -> Result<i32, String> {
    let name = optional_string(fields, key, default)?;
    names
        .iter()
        .position(|candidate| *candidate == name)
        .map(|index| index as i32)
        .ok_or_else(|| format!("unknown scoreboard {key}: {name}"))
}
fn absent(fields: &Fields, key: &str) -> Result<(), String> {
    if field(fields, key)?.is_some() {
        Err(format!("unsupported scoreboard field {key}"))
    } else {
        Ok(())
    }
}
fn compounds(elements: Vec<Tag>) -> Tag {
    Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements,
    })
}
fn value(value: &str) -> Tag {
    Tag::String(value.to_owned())
}

impl Scoreboard {
    /// Encode the full vanilla saved-data envelope, including DataVersion.
    pub fn to_nbt(&self) -> Tag {
        let objectives = self
            .objectives
            .iter()
            .map(|(name, objective)| {
                Tag::Compound(vec![
                    ("Name".into(), value(name)),
                    ("CriteriaName".into(), value(&objective.criteria)),
                    ("DisplayName".into(), component(&objective.display_name)),
                    (
                        "RenderType".into(),
                        value(if objective.criteria == "health" {
                            "hearts"
                        } else {
                            "integer"
                        }),
                    ),
                ])
            })
            .collect();
        let scores = self
            .scores
            .iter()
            .flat_map(|(owner, scores)| {
                scores.iter().map(move |(objective, score)| {
                    Tag::Compound(vec![
                        ("Name".into(), value(owner)),
                        ("Objective".into(), value(objective)),
                        ("Score".into(), Tag::Int(score.value)),
                        ("Locked".into(), Tag::Byte(i8::from(score.locked))),
                    ])
                })
            })
            .collect();
        let teams = self
            .teams
            .iter()
            .map(|(name, team)| {
                let mut fields = vec![
                    ("Name".into(), value(name)),
                    ("DisplayName".into(), component(&team.display_name)),
                    (
                        "AllowFriendlyFire".into(),
                        Tag::Byte(i8::from(team.friendly_fire)),
                    ),
                    (
                        "SeeFriendlyInvisibles".into(),
                        Tag::Byte(i8::from(team.see_friendly_invisibles)),
                    ),
                    ("MemberNamePrefix".into(), component(&team.prefix)),
                    ("MemberNameSuffix".into(), component(&team.suffix)),
                    (
                        "NameTagVisibility".into(),
                        value(VISIBILITY[team.nametag_visibility as usize]),
                    ),
                    (
                        "CollisionRule".into(),
                        value(COLLISION[team.collision_rule as usize]),
                    ),
                    (
                        "Players".into(),
                        Tag::List(ListTag {
                            element_type: tag_type::STRING,
                            elements: team.members.iter().map(|member| value(member)).collect(),
                        }),
                    ),
                ];
                if team.color != 21 {
                    fields.push(("TeamColor".into(), value(COLORS[team.color as usize])));
                }
                Tag::Compound(fields)
            })
            .collect();
        let slots = self
            .display_slots
            .iter()
            .map(|(slot, objective)| (SLOTS[*slot as usize].to_owned(), value(objective)))
            .collect();
        Tag::Compound(vec![
            (
                "DataVersion".into(),
                Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
            ),
            (
                "data".into(),
                Tag::Compound(vec![
                    ("Objectives".into(), compounds(objectives)),
                    ("PlayerScores".into(), compounds(scores)),
                    ("DisplaySlots".into(), Tag::Compound(slots)),
                    ("Teams".into(), compounds(teams)),
                ]),
            ),
        ])
    }

    /// Decode vanilla saved data without treating malformed records as absent.
    pub fn from_nbt(root: &Tag) -> Result<Self, String> {
        let root = compound(root)?;
        if let Some(version) = field(root, "DataVersion")?
            && !matches!(version, Tag::Int(_))
        {
            return Err("invalid scoreboard DataVersion".into());
        }
        let data = compound(field(root, "data")?.ok_or("missing scoreboard data")?)?;
        let mut result = Self::default();
        for tag in list(data, "Objectives", tag_type::COMPOUND)? {
            let fields = compound(tag)?;
            let name = required_string(fields, "Name")?;
            let criteria = optional_string(fields, "CriteriaName", "dummy")?;
            let rendering = if criteria == "health" {
                "hearts"
            } else {
                "integer"
            };
            if optional_string(fields, "RenderType", "integer")? != rendering
                || boolean(fields, "display_auto_update", false)?
            {
                return Err("unsupported scoreboard objective rendering".into());
            }
            absent(fields, "format")?;
            let objective = Objective {
                criteria,
                display_name: text(
                    field(fields, "DisplayName")?.ok_or("missing objective DisplayName")?,
                )?,
            };
            if result.objectives.insert(name, objective).is_some() {
                return Err("duplicate scoreboard objective".into());
            }
        }
        for tag in list(data, "PlayerScores", tag_type::COMPOUND)? {
            let fields = compound(tag)?;
            let owner = required_string(fields, "Name")?;
            let objective = required_string(fields, "Objective")?;
            if !result.objectives.contains_key(&objective) {
                return Err(format!("score references missing objective {objective}"));
            }
            let locked = boolean(fields, "Locked", true)?;
            absent(fields, "display")?;
            absent(fields, "format")?;
            let score = match field(fields, "Score")? {
                None => 0,
                Some(Tag::Int(score)) => *score,
                _ => return Err("invalid scoreboard Score".into()),
            };
            if result
                .scores
                .entry(owner)
                .or_insert_with(BTreeMap::new)
                .insert(
                    objective,
                    super::scoreboard::Score {
                        value: score,
                        locked,
                    },
                )
                .is_some()
            {
                return Err("duplicate scoreboard score".into());
            }
        }
        if let Some(slots) = field(data, "DisplaySlots")? {
            for (name, objective) in compound(slots)? {
                let slot = SLOTS
                    .iter()
                    .position(|candidate| *candidate == name)
                    .ok_or_else(|| format!("unknown scoreboard display slot {name}"))?
                    as i32;
                let objective = string(objective)?.to_owned();
                if !result.objectives.contains_key(&objective) {
                    return Err(format!(
                        "display slot references missing objective {objective}"
                    ));
                }
                if result.display_slots.insert(slot, objective).is_some() {
                    return Err("duplicate scoreboard display slot".into());
                }
            }
        }
        let mut members = BTreeSet::new();
        for tag in list(data, "Teams", tag_type::COMPOUND)? {
            let fields = compound(tag)?;
            let name = required_string(fields, "Name")?;
            if optional_string(fields, "DeathMessageVisibility", "always")? != "always" {
                return Err("unsupported scoreboard death message visibility".into());
            }
            let mut team = Team {
                display_name: optional_text(fields, "DisplayName", &name)?,
                color: if field(fields, "TeamColor")?.is_none() {
                    21
                } else {
                    named_index(fields, "TeamColor", &COLORS, "white")?
                },
                friendly_fire: boolean(fields, "AllowFriendlyFire", true)?,
                see_friendly_invisibles: boolean(fields, "SeeFriendlyInvisibles", true)?,
                nametag_visibility: named_index(
                    fields,
                    "NameTagVisibility",
                    &VISIBILITY,
                    "always",
                )?,
                collision_rule: named_index(fields, "CollisionRule", &COLLISION, "always")?,
                prefix: optional_text(fields, "MemberNamePrefix", "")?,
                suffix: optional_text(fields, "MemberNameSuffix", "")?,
                members: BTreeSet::new(),
            };
            for member in list(fields, "Players", tag_type::STRING)? {
                let member = string(member)?.to_owned();
                if !members.insert(member.clone()) {
                    return Err(format!("duplicate scoreboard team member {member}"));
                }
                team.members.insert(member);
            }
            if result.teams.insert(name, team).is_some() {
                return Err("duplicate scoreboard team".into());
            }
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn board() -> Scoreboard {
        let mut board = Scoreboard::default();
        board.objectives.insert(
            "kills".into(),
            Objective {
                criteria: "dummy".into(),
                display_name: "Kills ♥".into(),
            },
        );
        board.scores.insert(
            "Alice".into(),
            [(
                "kills".into(),
                crate::scoreboard::Score {
                    value: -7,
                    locked: true,
                },
            )]
            .into_iter()
            .collect(),
        );
        board.display_slots.insert(1, "kills".into());
        board.teams.insert(
            "red".into(),
            Team {
                display_name: "Red".into(),
                color: 12,
                friendly_fire: false,
                see_friendly_invisibles: true,
                nametag_visibility: 2,
                collision_rule: 3,
                prefix: "[R] ".into(),
                suffix: "!".into(),
                members: ["Alice".into()].into_iter().collect(),
            },
        );
        board
    }

    #[test]
    fn vanilla_packed_roundtrip() {
        let board = board();
        assert_eq!(Scoreboard::from_nbt(&board.to_nbt()).unwrap(), board);
        assert_eq!(
            Scoreboard::from_nbt(&Scoreboard::default().to_nbt()).unwrap(),
            Scoreboard::default()
        );
    }

    #[test]
    fn styled_components_and_health_rendering_roundtrip() {
        let mut board = board();
        let objective = board.objectives.get_mut("kills").unwrap();
        objective.criteria = "health".into();
        objective.display_name = r#"{"bold":true,"color":"red","text":"Health"}"#.into();
        board.teams.get_mut("red").unwrap().prefix = r#"{"italic":false,"text":"[R]"}"#.into();
        assert_eq!(Scoreboard::from_nbt(&board.to_nbt()).unwrap(), board);
    }

    #[test]
    fn malformed_and_unsupported_records_are_errors() {
        let mut root = board().to_nbt();
        let Tag::Compound(root_fields) = &mut root else {
            unreachable!()
        };
        let Tag::Compound(data) = &mut root_fields
            .iter_mut()
            .find(|(name, _)| name == "data")
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::List(scores) = &mut data
            .iter_mut()
            .find(|(name, _)| name == "PlayerScores")
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(score) = &mut scores.elements[0] else {
            unreachable!()
        };
        score.push(("Locked".into(), Tag::String("invalid".into())));
        assert!(Scoreboard::from_nbt(&root).is_err());
        let mut root = board().to_nbt();
        let Tag::Compound(root_fields) = &mut root else {
            unreachable!()
        };
        let Tag::Compound(data) = &mut root_fields
            .iter_mut()
            .find(|(name, _)| name == "data")
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::List(scores) = &mut data
            .iter_mut()
            .find(|(name, _)| name == "PlayerScores")
            .unwrap()
            .1
        else {
            unreachable!()
        };
        let Tag::Compound(score) = &mut scores.elements[0] else {
            unreachable!()
        };
        score.push(("Score".into(), Tag::Int(99)));
        assert!(Scoreboard::from_nbt(&root).is_err());
    }

    #[test]
    fn checkpoint_and_journal_overlay_restore_scoreboard() {
        let base = board();
        let checkpoint = base.to_nbt();
        assert_eq!(Scoreboard::from_nbt(&checkpoint).unwrap(), base);
        let tail = Scoreboard::default();
        let replay = crate::journal::Replay {
            scoreboard: Some(Scoreboard::from_nbt(&checkpoint).unwrap()),
            ..Default::default()
        };
        let batches = vec![(
            2,
            2,
            vec![crate::journal::JournalEntry::Scoreboard {
                value: tail.clone(),
            }],
        )];
        assert_eq!(
            crate::journal::replay_batches(replay, &batches).scoreboard,
            Some(tail)
        );
    }

    #[test]
    fn locked_and_enabled_scores_roundtrip_through_nbt_and_wal() {
        let mut value = board();
        value.scores.get_mut("Alice").unwrap().insert(
            "enabled".into(),
            crate::scoreboard::Score {
                value: 12,
                locked: false,
            },
        );
        value.objectives.insert(
            "enabled".into(),
            Objective {
                criteria: "trigger".into(),
                display_name: "Enabled".into(),
            },
        );
        assert_eq!(Scoreboard::from_nbt(&value.to_nbt()).unwrap(), value);
        let entries = vec![(
            1,
            1,
            vec![crate::journal::JournalEntry::Scoreboard {
                value: value.clone(),
            }],
        )];
        let replay = crate::journal::replay_batches(crate::journal::Replay::default(), &entries);
        assert_eq!(replay.scoreboard, Some(value));
    }

    #[test]
    fn absent_locked_field_defaults_to_locked() {
        let value = board();
        let mut root = value.to_nbt();
        let Tag::Compound(fields) = &mut root else {
            unreachable!()
        };
        let Tag::Compound(fields) =
            &mut fields.iter_mut().find(|(key, _)| key == "data").unwrap().1
        else {
            unreachable!()
        };
        let Tag::List(scores) = &mut fields
            .iter_mut()
            .find(|(key, _)| key == "PlayerScores")
            .unwrap()
            .1
        else {
            unreachable!()
        };
        for score in &mut scores.elements {
            let Tag::Compound(fields) = score else {
                unreachable!()
            };
            fields.retain(|(key, _)| key != "Locked");
        }
        assert_eq!(Scoreboard::from_nbt(&root).unwrap(), value);
    }

    #[test]
    fn corruption_is_not_a_missing_scoreboard() {
        assert!(Scoreboard::from_nbt(&Tag::Compound(vec![])).is_err());
    }
}
