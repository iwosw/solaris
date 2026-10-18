//! Literal lookup from TrialSpawnerConfigInRegistryFix$VanillaTrialChambers.
//! Compare borrowed constants: no runtime table construction, hashing, or cloning.
use super::{fields, get, set};
use mc_nbt::Tag;

#[derive(Clone, Copy)]
enum Value {
    Byte(i8),
    Int(i32),
    Float(f32),
    String(&'static str),
    Compound(&'static [(&'static str, Value)]),
    List(&'static [Value]),
}
macro_rules! c { ($($key:literal => $value:expr),* $(,)?) => { Value::Compound(&[$(($key, $value)),*]) }; }
macro_rules! l { ($($value:expr),* $(,)?) => { Value::List(&[$($value),*]) }; }
macro_rules! entity { ($id:literal) => { c!("id" => Value::String($id)) }; }
macro_rules! spawn { ($entity:expr) => { l!(c!("data" => c!("entity" => $entity), "weight" => Value::Int(1))) }; }
macro_rules! armed { ($entity:expr, $loot:literal) => { l!(c!("data" => c!("entity" => $entity, "equipment" => c!("loot_table" => Value::String($loot), "slot_drop_chances" => Value::Float(0.0))), "weight" => Value::Int(1))) }; }
const OMINOUS_LOOT: Value = l!(
    c!("data" => Value::String("minecraft:spawners/ominous/trial_chamber/key"), "weight" => Value::Int(3)),
    c!("data" => Value::String("minecraft:spawners/ominous/trial_chamber/consumables"), "weight" => Value::Int(7)),
);
macro_rules! melee { ($entity:expr) => { c!("simultaneous_mobs" => Value::Float(3.0), "simultaneous_mobs_added_per_player" => Value::Float(0.5), "spawn_potentials" => spawn!($entity), "ticks_between_spawn" => Value::Int(20)) }; }
macro_rules! slow { ($entity:expr) => { c!("simultaneous_mobs" => Value::Float(4.0), "simultaneous_mobs_added_per_player" => Value::Float(2.0), "spawn_potentials" => spawn!($entity), "ticks_between_spawn" => Value::Int(160)) }; }
macro_rules! armed_ominous { ($entity:expr, $loot:literal) => { c!("loot_tables_to_eject" => OMINOUS_LOOT, "spawn_potentials" => armed!($entity, $loot)) }; }
const SWARM_OMINOUS: Value = c!("loot_tables_to_eject" => OMINOUS_LOOT, "simultaneous_mobs" => Value::Float(4.0), "total_mobs" => Value::Float(12.0));
const BABY: Value = c!("IsBaby" => Value::Byte(1), "id" => Value::String("minecraft:zombie"));
const CONFIGS: &[(&str, Value, Value)] = &[
    (
        "minecraft:trial_chamber/breeze",
        c!("simultaneous_mobs" => Value::Float(1.0), "simultaneous_mobs_added_per_player" => Value::Float(0.5), "spawn_potentials" => spawn!(entity!("minecraft:breeze")), "ticks_between_spawn" => Value::Int(20), "total_mobs" => Value::Float(2.0), "total_mobs_added_per_player" => Value::Float(1.0)),
        c!("loot_tables_to_eject" => OMINOUS_LOOT, "simultaneous_mobs" => Value::Float(2.0), "total_mobs" => Value::Float(4.0)),
    ),
    (
        "minecraft:trial_chamber/melee/husk",
        melee!(entity!("minecraft:husk")),
        armed_ominous!(
            entity!("minecraft:husk"),
            "minecraft:equipment/trial_chamber_melee"
        ),
    ),
    (
        "minecraft:trial_chamber/melee/spider",
        melee!(entity!("minecraft:spider")),
        SWARM_OMINOUS,
    ),
    (
        "minecraft:trial_chamber/melee/zombie",
        melee!(entity!("minecraft:zombie")),
        armed_ominous!(
            entity!("minecraft:zombie"),
            "minecraft:equipment/trial_chamber_melee"
        ),
    ),
    (
        "minecraft:trial_chamber/ranged/poison_skeleton",
        melee!(entity!("minecraft:bogged")),
        armed_ominous!(
            entity!("minecraft:bogged"),
            "minecraft:equipment/trial_chamber_ranged"
        ),
    ),
    (
        "minecraft:trial_chamber/ranged/skeleton",
        melee!(entity!("minecraft:skeleton")),
        armed_ominous!(
            entity!("minecraft:skeleton"),
            "minecraft:equipment/trial_chamber_ranged"
        ),
    ),
    (
        "minecraft:trial_chamber/ranged/stray",
        melee!(entity!("minecraft:stray")),
        armed_ominous!(
            entity!("minecraft:stray"),
            "minecraft:equipment/trial_chamber_ranged"
        ),
    ),
    (
        "minecraft:trial_chamber/slow_ranged/poison_skeleton",
        slow!(entity!("minecraft:bogged")),
        armed_ominous!(
            entity!("minecraft:bogged"),
            "minecraft:equipment/trial_chamber_ranged"
        ),
    ),
    (
        "minecraft:trial_chamber/slow_ranged/skeleton",
        slow!(entity!("minecraft:skeleton")),
        armed_ominous!(
            entity!("minecraft:skeleton"),
            "minecraft:equipment/trial_chamber_ranged"
        ),
    ),
    (
        "minecraft:trial_chamber/slow_ranged/stray",
        slow!(entity!("minecraft:stray")),
        armed_ominous!(
            entity!("minecraft:stray"),
            "minecraft:equipment/trial_chamber_ranged"
        ),
    ),
    (
        "minecraft:trial_chamber/small_melee/baby_zombie",
        c!("simultaneous_mobs" => Value::Float(2.0), "simultaneous_mobs_added_per_player" => Value::Float(0.5), "spawn_potentials" => spawn!(BABY), "ticks_between_spawn" => Value::Int(20)),
        armed_ominous!(BABY, "minecraft:equipment/trial_chamber_melee"),
    ),
    (
        "minecraft:trial_chamber/small_melee/cave_spider",
        melee!(entity!("minecraft:cave_spider")),
        SWARM_OMINOUS,
    ),
    (
        "minecraft:trial_chamber/small_melee/silverfish",
        melee!(entity!("minecraft:silverfish")),
        SWARM_OMINOUS,
    ),
    (
        "minecraft:trial_chamber/small_melee/slime",
        c!("simultaneous_mobs" => Value::Float(3.0), "simultaneous_mobs_added_per_player" => Value::Float(0.5), "spawn_potentials" => l!(c!("data" => c!("entity" => c!("Size" => Value::Int(1), "id" => Value::String("minecraft:slime"))), "weight" => Value::Int(3)), c!("data" => c!("entity" => c!("Size" => Value::Int(2), "id" => Value::String("minecraft:slime"))), "weight" => Value::Int(1))), "ticks_between_spawn" => Value::Int(20)),
        SWARM_OMINOUS,
    ),
];

fn matches_value(actual: &Tag, expected: Value) -> bool {
    match (actual, expected) {
        (Tag::Byte(a), Value::Byte(b)) => *a == b,
        (Tag::Int(a), Value::Int(b)) => *a == b,
        (Tag::Float(a), Value::Float(b)) => a.to_bits() == b.to_bits(),
        (Tag::String(a), Value::String(b)) => a == b,
        (Tag::Compound(actual), Value::Compound(expected)) => {
            actual.len() == expected.len()
                && expected.iter().all(|(key, value)| {
                    actual
                        .iter()
                        .find(|(name, _)| name == key)
                        .is_some_and(|(_, actual)| matches_value(actual, *value))
                })
        }
        (Tag::List(actual), Value::List(expected)) => {
            actual.elements.len() == expected.len()
                && actual
                    .elements
                    .iter()
                    .zip(expected)
                    .all(|(actual, expected)| matches_value(actual, *expected))
        }
        _ => false,
    }
}

fn is_default(key: &str, value: Value) -> bool {
    match key {
        "spawn_range" => matches!(value, Value::Int(4)),
        "ticks_between_spawn" => matches!(value, Value::Int(40)),
        "total_mobs" => matches!(value, Value::Float(v) if v.to_bits() == 6.0_f32.to_bits()),
        "simultaneous_mobs" | "total_mobs_added_per_player" => {
            matches!(value, Value::Float(v) if v.to_bits() == 2.0_f32.to_bits())
        }
        "simultaneous_mobs_added_per_player" => {
            matches!(value, Value::Float(v) if v.to_bits() == 1.0_f32.to_bits())
        }
        _ => false,
    }
}

fn merged_fields(
    normal: Value,
    ominous: Value,
    strip_defaults: bool,
) -> impl Iterator<Item = (&'static str, Value)> {
    let Value::Compound(normal) = normal else {
        unreachable!("literal config is a compound")
    };
    let Value::Compound(ominous) = ominous else {
        unreachable!("literal config is a compound")
    };
    normal
        .iter()
        .filter(move |(key, _)| !ominous.iter().any(|(overridden, _)| overridden == key))
        .chain(ominous)
        .copied()
        .filter(move |(key, value)| !strip_defaults || !is_default(key, *value))
}

fn matches_merged(actual: &Tag, normal: Value, ominous: Value, strip_defaults: bool) -> bool {
    let Some(actual) = fields(actual) else {
        return false;
    };
    let mut count = 0;
    for (key, expected) in merged_fields(normal, ominous, strip_defaults) {
        count += 1;
        if !actual
            .iter()
            .find(|(name, _)| name == key)
            .is_some_and(|(_, actual)| matches_value(actual, expected))
        {
            return false;
        }
    }
    count == actual.len()
}

pub(super) fn fix(tag: &mut Tag) {
    let (Some(normal), Some(ominous)) = (get(tag, "normal_config"), get(tag, "ominous_config"))
    else {
        return;
    };
    let key = CONFIGS
        .iter()
        .find_map(|(key, expected_normal, expected_ominous)| {
            (matches_value(normal, *expected_normal)
                && (matches_value(ominous, *expected_ominous)
                    || matches_merged(ominous, *expected_normal, *expected_ominous, false)
                    || matches_merged(ominous, *expected_normal, *expected_ominous, true)))
            .then_some(*key)
        });
    if let Some(key) = key {
        set(tag, "normal_config", Tag::String(format!("{key}/normal")));
        set(tag, "ominous_config", Tag::String(format!("{key}/ominous")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mc_nbt::ListTag;
    fn tag(value: Value) -> Tag {
        match value {
            Value::Byte(v) => Tag::Byte(v),
            Value::Int(v) => Tag::Int(v),
            Value::Float(v) => Tag::Float(v),
            Value::String(v) => Tag::String(v.into()),
            Value::Compound(v) => Tag::Compound(
                v.iter()
                    .rev()
                    .map(|(key, value)| ((*key).into(), tag(*value)))
                    .collect(),
            ),
            Value::List(v) => {
                let elements: Vec<Tag> = v.iter().map(|value| tag(*value)).collect();
                Tag::List(ListTag {
                    element_type: elements.first().map_or(0, Tag::type_id),
                    elements,
                })
            }
        }
    }
    #[test]
    fn all_literal_lookup_pairs_include_overlay_merged_and_default_stripped_forms() {
        for (key, normal, ominous) in CONFIGS {
            for form in 0..3 {
                let ominous_tag = if form == 0 {
                    tag(*ominous)
                } else {
                    Tag::Compound(
                        merged_fields(*normal, *ominous, form == 2)
                            .map(|(key, value)| (key.into(), tag(value)))
                            .collect(),
                    )
                };
                let mut block = Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:trial_spawner".into())),
                    ("normal_config".into(), tag(*normal)),
                    ("ominous_config".into(), ominous_tag),
                ]);
                super::super::fix_node(
                    &mut block,
                    crate::anvil::upgrade::NodeKind::BlockEntity,
                    4061,
                    &mut crate::anvil::upgrade::UpgradeReport::default(),
                )
                .unwrap();
                assert_eq!(
                    get(&block, "normal_config"),
                    Some(&Tag::String(format!("{key}/normal")))
                );
                assert_eq!(
                    get(&block, "ominous_config"),
                    Some(&Tag::String(format!("{key}/ominous")))
                );
            }
        }
    }
    #[test]
    fn extra_fields_type_changes_and_reordered_list_elements_do_not_match() {
        let (_, normal, ominous) = CONFIGS[0];
        for change in 0..3 {
            let mut n = tag(normal);
            let mut o = tag(ominous);
            match change {
                0 => set(&mut n, "custom", Tag::Int(1)),
                1 => set(&mut n, "simultaneous_mobs", Tag::Double(1.0)),
                _ => {
                    if let Some(Tag::List(list)) =
                        super::super::get_mut(&mut o, "loot_tables_to_eject")
                    {
                        list.elements.reverse();
                    }
                }
            }
            let mut block = Tag::Compound(vec![
                ("normal_config".into(), n),
                ("ominous_config".into(), o),
            ]);
            let before = block.clone();
            fix(&mut block);
            assert_eq!(block, before);
        }
    }
}
