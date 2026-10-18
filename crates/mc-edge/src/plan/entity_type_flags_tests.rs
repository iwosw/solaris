//! `entity_type_flags` oracle: real registry ids for boss/leash/ordinary,
//! out-of-range ids are false, and similar names never match.
use crate::plan::entity_type_flags::{is_boss, is_leash_knot};

fn type_id_of(name: &str) -> u32 {
    mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}

#[test]
fn wither_and_dragon_are_bosses() {
    for name in ["minecraft:wither", "minecraft:ender_dragon"] {
        let id = type_id_of(name);
        assert!(is_boss(id), "{name} (id {id}) is a boss");
        assert!(!is_leash_knot(id), "{name} (id {id}) is not a leash knot");
    }
}

#[test]
fn leash_knot_is_not_a_boss() {
    let id = type_id_of("minecraft:leash_knot");
    assert!(is_leash_knot(id));
    assert!(!is_boss(id));
}

#[test]
fn ordinary_mobs_are_neither() {
    for name in [
        "minecraft:cow",
        "minecraft:wither_skeleton",
        "minecraft:player",
    ] {
        let id = type_id_of(name);
        assert!(!is_boss(id), "{name} is not a boss");
        assert!(!is_leash_knot(id), "{name} is not a leash knot");
    }
}

#[test]
fn out_of_range_ids_are_false() {
    for id in [
        u32::MAX,
        1_000_000,
        mc_data::entity_types::ENTITY_TYPE_COUNT as u32,
    ] {
        assert!(!is_boss(id), "id {id} is not a boss");
        assert!(!is_leash_knot(id), "id {id} is not a leash knot");
    }
}
