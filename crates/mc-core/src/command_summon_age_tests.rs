use super::*;
use crate::boundary::{ChangeSet, ConnectionId};

fn summon(state: &mut TickState, actor: EntityId, kind: &str, nbt: &str) -> EntityId {
    let parsed = crate::commands::parse(&format!("summon {kind} ~ ~ ~ {nbt}"), true).unwrap();
    let mut changes = ChangeSet::default();
    crate::command_execution::execute(state, ConnectionId(1), actor, &parsed, &mut changes);
    assert_eq!(
        changes.entities_spawned.len(),
        1,
        "{kind} {nbt}: {:?}",
        state.outbox
    );
    changes.entities_spawned[0]
}

fn age_family(kinds: &[&str], baby_model: bool) {
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    for kind in kinds {
        for age in [-1234_i32, 0, 6000] {
            let id = summon(&mut state, actor, kind, &format!("{{Age:{age}}}"));
            let slot = state.entities.slot_of(id).unwrap();
            let row = state.entities.row(slot);
            assert_eq!(
                row.entity_life.child_ticks,
                age.min(0).unsigned_abs(),
                "{kind}"
            );
            assert_eq!(row.breed_cooldown, age.max(0) as u32, "{kind}");
            assert_eq!(row.appearance.baby, age < 0 && baby_model, "{kind}");
            let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
            let native = chunks
                .values()
                .flat_map(|c| &c.entities)
                .find(|entity| entity.uuid == row.uuid)
                .unwrap();
            assert_eq!(native.age, age, "{kind}");
            let image = EntityStateImage::from_record(&state.registry, native).unwrap();
            assert_eq!(
                image.entity_life.child_ticks, row.entity_life.child_ticks,
                "{kind}"
            );
            assert_eq!(image.breed_cooldown, row.breed_cooldown, "{kind}");
            state.remove_entity(slot);
        }
    }
}

#[test]
fn summon_age_water_families() {
    age_family(
        &[
            "squid",
            "glow_squid",
            "dolphin",
            "nautilus",
            "axolotl",
            "turtle",
        ],
        true,
    );
}

#[test]
fn summon_age_horse_families() {
    age_family(
        &[
            "horse",
            "donkey",
            "mule",
            "skeleton_horse",
            "zombie_horse",
            "llama",
            "trader_llama",
            "camel",
        ],
        true,
    );
}

#[test]
fn summon_age_animal_families() {
    age_family(
        &[
            "cow",
            "mooshroom",
            "sheep",
            "pig",
            "chicken",
            "rabbit",
            "goat",
            "armadillo",
            "bee",
            "cat",
            "wolf",
            "ocelot",
            "fox",
            "panda",
            "polar_bear",
            "sniffer",
            "happy_ghast",
            "hoglin",
            "strider",
        ],
        true,
    );
}

#[test]
fn summon_age_without_baby_model_retains_age_without_resizing() {
    age_family(&["frog", "parrot", "camel_husk", "zombie_nautilus"], false);
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    for kind in ["frog", "parrot", "camel_husk", "zombie_nautilus"] {
        let id = summon(&mut state, actor, kind, "{Age:-1}");
        let slot = state.entities.slot_of(id).unwrap();
        let body = state.entities.body(slot);
        let dimensions = (body.half_width, body.height);
        crate::entity_life::advance(&mut state);
        let body = state.entities.body(slot);
        assert_eq!((body.half_width, body.height), dimensions, "{kind}");
        assert_eq!(
            state.entities.row(slot).entity_life.child_ticks,
            0,
            "{kind}"
        );
        state.remove_entity(slot);
    }
}

#[test]
fn summon_age_villager_families() {
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    for kind in ["villager", "wandering_trader"] {
        let id = summon(&mut state, actor, kind, "{Age:-1234}");
        let slot = state.entities.slot_of(id).unwrap();
        let row = state.entities.row(slot);
        assert_eq!(row.villager.as_ref().unwrap().age, -1234, "{kind}");
        assert!(row.appearance.baby, "{kind}");
        state.remove_entity(slot);
    }
}

fn boolean_baby_family(kinds: &[&str]) {
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    for kind in kinds {
        for baby in [false, true] {
            let id = summon(
                &mut state,
                actor,
                kind,
                &format!("{{IsBaby:{}b}}", i8::from(baby)),
            );
            let slot = state.entities.slot_of(id).unwrap();
            let row = state.entities.row(slot);
            assert_eq!(row.appearance.baby, baby, "{kind}");
            assert_eq!(
                row.entity_life.child_ticks, 0,
                "{kind}: boolean babies must not grow"
            );
            let dimensions = (
                state.entities.body(slot).half_width,
                state.entities.body(slot).height,
            );
            let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
            let native = chunks
                .values()
                .flat_map(|c| &c.entities)
                .find(|entity| entity.uuid == row.uuid)
                .unwrap();
            assert_eq!(
                native
                    .extra_fields
                    .iter()
                    .filter(|(key, _)| key == "IsBaby")
                    .count(),
                1
            );
            assert!(
                native
                    .extra_fields
                    .contains(&("IsBaby".into(), Tag::Byte(i8::from(baby))))
            );
            let image = EntityStateImage::from_record(&state.registry, native).unwrap();
            assert!(image.opaque_entity_fields.is_empty(), "{kind}");
            let replay = crate::journal::ReplayEntity {
                uuid: native.uuid.to_string(),
                type_id: state.registry.entity_type_id(&native.id).unwrap(),
                pos: native.pos,
                vel: native.motion.map(|v| v * 20.0),
                yaw: native.yaw,
                pitch: native.pitch,
                ground: native.on_ground,
                health: native.health,
                alive: true,
                age: image.age,
                breed_cooldown: image.breed_cooldown,
                pickup_delay: 0,
                item: None,
                appearance: crate::core::appearance_from_vanilla_entity(native),
                active_effects: Default::default(),
                villager: row.villager.as_deref().cloned(),
                projectile: None,
                state: image,
                tick: state.tick,
            };
            state.remove_entity(slot);
            crate::core::restore_entity(&mut state, id.0, &replay).unwrap();
            let slot = state.entities.slot_of(id).unwrap();
            assert_eq!(state.entities.row(slot).appearance.baby, baby, "{kind}");
            assert_eq!(
                (
                    state.entities.body(slot).half_width,
                    state.entities.body(slot).height
                ),
                dimensions,
                "{kind}"
            );
            crate::entity_life::advance(&mut state);
            assert_eq!(state.entities.row(slot).appearance.baby, baby, "{kind}");
            state.remove_entity(slot);
        }
    }
}

#[test]
fn summon_is_baby_zombie_family() {
    boolean_baby_family(&[
        "zombie",
        "husk",
        "drowned",
        "zombified_piglin",
        "zombie_villager",
    ]);
}

#[test]
fn summon_is_baby_piglin_and_zoglin() {
    boolean_baby_family(&["piglin", "zoglin"]);
}

#[test]
fn summon_baby_tags_do_not_cross_families() {
    let (state, _, _) = crate::hostile_tests::setup(8.0);
    for (kind, key) in [
        ("piglin_brute", "IsBaby"),
        ("hoglin", "IsBaby"),
        ("cow", "IsBaby"),
        ("zombie", "Age"),
    ] {
        let name = format!("minecraft:{kind}");
        let type_id = state.registry.entity_type_id(&name).unwrap();
        assert!(
            prepare(
                &state,
                type_id,
                &name,
                [0.0; 3],
                &Tag::Compound(vec![(key.into(), Tag::Int(1))])
            )
            .is_err(),
            "{kind} {key}"
        );
    }
}
