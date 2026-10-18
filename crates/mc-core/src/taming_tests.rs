use crate::mounts::tests::{fixture, hold, slot, spawn, use_entity};
use crate::tick::run_tick;
#[test]
fn tame_rolls_owner_sit_and_collar_survive_checkpoint_fields_and_replay_image() {
    let (mut state, actor, pos) = fixture();
    let wolf = spawn(&mut state, "wolf", pos);
    let w = slot(&state, wolf);
    hold(&mut state, actor, "bone", 64);
    for _ in 0..32 {
        run_tick(&mut state, &[use_entity(wolf)]);
        if state.entities.row(w).entity_life.tamed {
            break;
        }
    }
    let body = state.entities.body(w);
    let row = state.entities.row(w);
    assert!(row.entity_life.tamed);
    assert!(row.entity_life.sitting);
    assert_eq!(row.health, 40.0);
    assert_eq!(
        row.entity_life.taming.owner,
        Some(state.entities.row(slot(&state, actor)).uuid)
    );
    let image = crate::journal::EntityStateImage::from_row(body, row);
    let bytes = serde_json::to_vec(&image).unwrap();
    let recovered: crate::journal::EntityStateImage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(recovered.entity_life, row.entity_life);
    let record = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:wolf".into(),
        extra_fields: image.to_fields(&state.registry.blocks),
        ..Default::default()
    };
    let back = crate::journal::EntityStateImage::from_record(&state.registry, &record).unwrap();
    assert_eq!(back.entity_life.taming, row.entity_life.taming);
    assert_eq!(back.entity_life.sitting, row.entity_life.sitting);
    hold(&mut state, actor, "blue_dye", 1);
    run_tick(&mut state, &[use_entity(wolf)]);
    assert_eq!(state.entities.row(w).entity_life.taming.collar, 11);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::EMPTY;
    run_tick(&mut state, &[use_entity(wolf)]);
    assert!(!state.entities.row(w).entity_life.sitting);
}
#[test]
fn sitting_suppresses_wander_and_panic_without_disabling_gravity() {
    let (mut state, _, pos) = fixture();
    let cat = spawn(&mut state, "cat", pos);
    let c = slot(&state, cat);
    state.entities.row_mut(c).entity_life.sitting = true;
    state.entities.body_mut(c).hurt_at = Some(state.tick);
    for _ in 0..10 {
        run_tick(&mut state, &[]);
    }
    let at = state.entities.pos(c);
    assert!((at[0] - pos[0]).abs() < 1e-6);
    assert!((at[2] - pos[2]).abs() < 1e-6);
}
#[test]
fn malformed_present_animal_fields_fail_closed_transactionally() {
    use mc_nbt::Tag;
    let mut life = crate::entity_life::EntityLifeState::default();
    let original = life.clone();
    for (key, tag) in [
        ("Owner", Tag::IntArray(vec![1])),
        ("Temper", Tag::Int(101)),
        ("CollarColor", Tag::Byte(16)),
        ("leash", Tag::Compound(vec![])),
        ("solaris:Rider", Tag::String("broken".into())),
    ] {
        let error =
            crate::mounts::persistence::from_fields(&mut life, &[(key.into(), tag)]).unwrap_err();
        assert!(error.to_string().contains("invalid animal NBT:"));
        assert_eq!(life, original);
    }
}
#[test]
fn horse_feed_raises_temper_and_mounted_taming_rolls_are_retained() {
    let (mut state, actor, pos) = fixture();
    let horse = spawn(&mut state, "horse", pos);
    let h = slot(&state, horse);
    hold(&mut state, actor, "wheat", 1);
    run_tick(&mut state, &[use_entity(horse)]);
    assert_eq!(state.entities.row(h).entity_life.taming.temper, 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::EMPTY;
    state.entities.row_mut(h).entity_life.taming.temper = 100;
    run_tick(&mut state, &[use_entity(horse)]);
    for _ in 0..500 {
        if state.entities.row(h).entity_life.tamed {
            break;
        }
        run_tick(&mut state, &[]);
    }
    assert!(state.entities.row(h).entity_life.tamed);
    assert!(state.entities.row(h).entity_life.taming.rolls > 0);
}

#[test]
fn cat_and_parrot_food_rolls_set_owner_and_ocelot_trust_persists() {
    let (mut state, actor, pos) = fixture();
    let uuid = state.entities.row(slot(&state, actor)).uuid;
    for (kind, item) in [
        ("cat", "cod"),
        ("parrot", "wheat_seeds"),
        ("ocelot", "salmon"),
    ] {
        let animal = spawn(&mut state, kind, pos);
        let s = slot(&state, animal);
        hold(&mut state, actor, item, 64);
        for _ in 0..64 {
            // Exercise the same exact-stack intent/merge path without unrelated AI.
            let mut intents = Vec::new();
            assert!(
                crate::mounts::interact(
                    &mut state,
                    actor,
                    animal,
                    crate::boundary::Hand::Main,
                    false,
                    &mut intents
                ),
                "tame interaction rejected for {kind} holding {item}"
            );
            state.merge(&mut intents);
            let row = state.entities.row(s);
            if row.entity_life.tamed || row.entity_life.taming.trusting {
                break;
            }
        }
        let row = state.entities.row(s);
        if kind == "ocelot" {
            assert!(row.entity_life.taming.trusting);
            assert!(row.entity_life.taming.owner.is_none());
        } else {
            assert!(row.entity_life.tamed);
            assert_eq!(row.entity_life.taming.owner, Some(uuid));
            assert!(row.entity_life.sitting);
        }
        let record = mc_data::entity_nbt::VanillaEntity {
            id: format!("minecraft:{kind}"),
            extra_fields: crate::entity_life::to_fields(&row.entity_life, 0),
            ..Default::default()
        };
        let recovered =
            crate::journal::EntityStateImage::from_record(&state.registry, &record).unwrap();
        assert_eq!(recovered.entity_life.taming, row.entity_life.taming);
    }
}

#[test]
fn fox_birth_inherits_distinct_feeding_players_as_trusted_uuids() {
    let (mut state, actor, pos) = fixture();
    let first = spawn(&mut state, "fox", pos);
    let second = spawn(&mut state, "fox", pos);
    hold(&mut state, actor, "sweet_berries", 1);
    run_tick(&mut state, &[use_entity(first)]);
    let owner = state.entities.row(slot(&state, actor)).uuid;
    assert_eq!(
        state
            .entities
            .row(slot(&state, first))
            .entity_life
            .taming
            .love_cause,
        Some(owner)
    );
    let other = uuid::Uuid::new_v4();
    let second_slot = slot(&state, second);
    state
        .entities
        .row_mut(second_slot)
        .entity_life
        .taming
        .love_cause = Some(other);
    let first_slot = slot(&state, first);
    let kind = crate::mounts::type_id(state.entities.body(first_slot)).unwrap();
    crate::entity_life::breed(&mut state, first, second, pos, kind);
    state.commit_merge_effects();
    let child = state
        .entities
        .slots()
        .find(|s| {
            state.entities.row(*s).appearance.baby
                && crate::mounts::name(&state, *s) == "minecraft:fox"
        })
        .unwrap();
    assert_eq!(
        state.entities.row(child).entity_life.taming.trusted,
        vec![owner, other]
    );
    assert!(state.entities.row(child).entity_life.taming.trusting);
}

#[test]
fn retained_tame_roll_counter_and_invalid_images_do_not_reset_or_silently_pass() {
    let (mut state, _, pos) = fixture();
    let wolf = spawn(&mut state, "wolf", pos);
    let slot = slot(&state, wolf);
    let row = state.entities.row_mut(slot);
    super::roll(row, 3);
    let mut recovered = row.clone();
    let json = serde_json::to_vec(&row.entity_life.taming).unwrap();
    recovered.entity_life.taming = serde_json::from_slice(&json).unwrap();
    assert_eq!(super::roll(row, 3), super::roll(&mut recovered, 3));
    for (field, value, message) in [("temper", 101, "Temper"), ("collar", 16, "CollarColor")] {
        let mut image = serde_json::to_value(&recovered.entity_life.taming).unwrap();
        image[field] = serde_json::json!(value);
        let error = serde_json::from_value::<super::TamingState>(image).unwrap_err();
        assert!(error.to_string().contains(message));
    }
}

#[test]
fn failed_tame_roll_spends_one_food_without_granting_ownership() {
    let (mut state, actor, pos) = fixture();
    let wolf = spawn(&mut state, "wolf", pos);
    let slot = slot(&state, wolf);
    let mut probe = state.entities.row(slot).clone();
    let before = (0..16)
        .find_map(|_| {
            let before = probe.entity_life.taming.rolls;
            (super::roll(&mut probe, 3) != 0).then_some(before)
        })
        .expect("fixed fixture includes a failed tame roll");
    state.entities.row_mut(slot).entity_life.taming.rolls = before;
    hold(&mut state, actor, "bone", 1);
    let (_, drains, _) = run_tick(&mut state, &[use_entity(wolf)]);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    assert!(state.entities.row(slot).entity_life.taming.owner.is_none());
    assert!(!state.entities.row(slot).entity_life.tamed);
    assert!(drains.iter().filter(|(connection,_)|*connection==crate::boundary::ConnectionId(1))
        .flat_map(|(_,changes)|changes).any(|change|
        matches!(change,crate::boundary::PlayerChange::ProjectileStatus { entity,event:6,.. } if *entity==wolf)));
}

#[test]
fn bootstrap_food_tags_cover_every_animal_interaction_species() {
    let (state, _, _) = fixture();
    for (species, food) in [
        ("wolf", "beef"),
        ("cat", "cod"),
        ("parrot", "wheat_seeds"),
        ("horse", "wheat"),
        ("llama", "wheat"),
        ("pig", "carrot"),
        ("strider", "warped_fungus"),
        ("camel", "cactus"),
    ] {
        let item = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{food}")).unwrap())
            .unwrap();
        assert!(
            super::food(&state, &format!("minecraft:{species}"), item),
            "bootstrap registry lacks canonical {species} food {food}"
        );
    }
}
